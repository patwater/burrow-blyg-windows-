//! The browser pane's GPUI side: its state, the chrome (back, forward,
//! reload/stop, address, shield, open in the default browser, → Notes,
//! close), and the `MainView` hooks `app.rs` calls.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use gpui_kit::base::input::{
    Copy, Cut, Escape, Input, InputEvent, InputState, Paste, Redo, SelectAll, Undo,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::surface::{self, BrowserEvent, BrowserSurface, PageState};
use super::{OpenMode, rules_handle};
use crate::app::MainView;
use crate::theme::Palette;
use crate::theme::Rule; // --- themes --- dividers

gpui_kit::actions!(
    blygger,
    [
        ToggleBrowser,
        BrowserBack,
        BrowserForward,
        BrowserReload,
        BrowserAddress
    ]
);

/// The key context inside the pane (keymap `Scope::Browser`).
pub const CONTEXT: &str = "Browser";

/// The slide-over's share of the window width.
pub const SLIDE_WIDTH: f32 = 0.66;

const CHROME_H: f32 = 38.;

/// --- onboarding --- The tutorial's sample page.
const TOUR_TITLE: &str = "Sample page · a link from a post";
const TOUR_BODY: &str = "A link you click in a post opens here. (Tour sample: nothing is loaded.)";

/// The web view and where it was last put, shared with the pane's canvas.
#[derive(Default)]
struct Placed {
    surface: Option<Box<dyn BrowserSurface>>,
    frame: Option<Bounds<Pixels>>,
    visible: bool,
    /// Closed, or something GPUI draws is over it: stay hidden.
    suppressed: bool,
    /// --- notes --- The notes drawer's left edge while it's out: the view
    /// is cut off there (a native view would cover the drawer).
    clip_right: Option<Pixels>,
}

impl Placed {
    fn place(&mut self, bounds: Bounds<Pixels>) {
        let Some(s) = self.surface.as_mut() else {
            return;
        };
        // --- notes ---
        let mut bounds = bounds;
        if let Some(edge) = self.clip_right {
            bounds.size.width = bounds.size.width.min(edge - bounds.origin.x);
            if bounds.size.width < px(40.) {
                if self.visible {
                    s.set_visible(false);
                    self.visible = false;
                }
                return;
            }
        }
        if self.frame != Some(bounds) {
            s.set_frame(bounds);
            self.frame = Some(bounds);
        }
        if !self.visible && !self.suppressed {
            s.set_visible(true);
            self.visible = true;
        }
    }

    fn hide(&mut self) {
        if let (Some(s), true) = (self.surface.as_mut(), self.visible) {
            s.set_visible(false);
            self.visible = false;
        }
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut dyn BrowserSurface) -> R) -> Option<R> {
        self.surface.as_mut().map(|s| f(s.as_mut()))
    }

    /// --- notes --- See `clip_right`.
    fn set_clip(&mut self, edge: Option<Pixels>) {
        if self.clip_right != edge {
            self.clip_right = edge;
            self.frame = None;
        }
    }
}

pub struct Browser {
    /// The pane is on screen (it may still be hidden under a sheet).
    pub open: bool,
    pub mode: OpenMode,
    placed: Rc<RefCell<Placed>>,
    /// Why there's no web view (shown in the pane).
    failed: Option<String>,
    pub page: PageState,
    events: Sender<BrowserEvent>,
    events_rx: Option<Receiver<BrowserEvent>>,
    address: Option<Entity<InputState>>,
    editing: bool,
    focus: Option<FocusHandle>,
    /// A URL waiting for the block lists (at most `RULES_WAIT`).
    pending: Option<String>,
    rules: rules_handle::Handle,
    blocker_started: bool,
    /// Hosts whose shield is off (`state.json`).
    unblocked: Rc<RefCell<BTreeSet<String>>>,
    /// `content-blocking` in the config.
    global: Rc<Cell<bool>>,
    data_dir: Option<PathBuf>,
    close_gen: u64,
    /// Times the pane was shown (a new slide-in animation each time).
    shown: u64,
    viewport_w: Pixels,
    poll: Option<Task<()>>,
    /// --- selection --- The page has text selected (checked while it has
    /// the keyboard, which it has once it's been clicked): "→ Draft" says so.
    pub(crate) selected: bool,
    sel_watch: Option<Task<()>>,
    /// A press on "→ Notes" or "→ Draft": the pane's own mouse-down leaves
    /// the page its keyboard this once, or WebKit would drop the page's
    /// selection before the click reads it.
    keep_keyboard: bool,
    teardown: Option<Task<()>>,
    dark: Option<bool>,
    /// --- onboarding --- The tutorial's sample page is up: what it replaced
    /// (the page, the mode, and whether the pane was open), put back when
    /// the tour ends.
    tour: Option<(PageState, OpenMode, bool)>,
    /// URLs loaded (tests).
    #[cfg(test)]
    pub(crate) loads: Vec<String>,
}

impl Browser {
    pub fn new(data_dir: Option<PathBuf>, content_blocking: bool) -> Browser {
        let (tx, rx) = async_channel::unbounded();
        let unblocked = data_dir
            .as_deref()
            .map(blyg_core::state::AppState::load)
            .map(|s| s.browser_unblocked_hosts.into_iter().collect())
            .unwrap_or_default();
        Browser {
            open: false,
            mode: OpenMode::Slide,
            placed: Rc::default(),
            failed: None,
            page: PageState::default(),
            events: tx,
            events_rx: Some(rx),
            address: None,
            editing: false,
            focus: None,
            pending: None,
            rules: Rc::default(),
            blocker_started: false,
            unblocked: Rc::new(RefCell::new(unblocked)),
            global: Rc::new(Cell::new(content_blocking)),
            data_dir,
            close_gen: 0,
            shown: 0,
            viewport_w: px(0.),
            poll: None,
            selected: false,
            sel_watch: None,
            keep_keyboard: false,
            teardown: None,
            dark: None,
            tour: None,
            #[cfg(test)]
            loads: Vec::new(),
        }
    }

    /// The web view exists (it's dropped a while after the pane closes).
    pub fn alive(&self) -> bool {
        self.placed.borrow().surface.is_some()
    }

    #[cfg(test)]
    pub fn visible(&self) -> bool {
        self.placed.borrow().visible
    }

    /// Blocking applies to the current page.
    pub fn blocking_here(&self) -> bool {
        super::blocking_applies(self.global.get(), &self.unblocked.borrow(), &self.page.url)
    }

    /// The window became active again: if the keyboard fell to the window
    /// itself, give it to GPUI (never take it from a page's text field).
    pub fn reclaim_lost_keyboard(&self) {
        self.placed.borrow_mut().with(|s| s.reclaim_lost_keyboard());
    }

    /// Where the pane's left edge is, for a window `width` wide.
    pub fn left_edge(&self, width: Pixels) -> Pixels {
        match self.mode {
            OpenMode::Slide => (width * (1. - SLIDE_WIDTH)).round(),
            OpenMode::Full => px(0.),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut dyn BrowserSurface) -> R) -> Option<R> {
        self.placed.borrow_mut().with(f)
    }

    /// --- notes --- Ask the page for its selected text (`false`: no page).
    pub fn selection(&self, reply: async_channel::Sender<String>) -> bool {
        self.open && self.with(|s| s.selection(reply)).is_some()
    }
}

impl MainView {
    /// Wire the web view's events in (called once from `new`).
    pub(crate) fn browser_init(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rx) = self.browser.events_rx.take() {
            self._tasks.push(cx.spawn_in(window, async move |this, cx| {
                while let Ok(ev) = rx.recv().await {
                    if this
                        .update_in(cx, |v, window, cx| v.browser_event(ev, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
    }

    /// Hook: a link was clicked in a post (the reader; later the stream).
    /// The click's modifiers and `open-links` decide where it goes.
    pub fn open_link(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        if !super::is_web_url(&url) {
            // mailto: and the like: the system's handler.
            if matches!(super::nav_policy(&url), super::Nav::System(_)) {
                cx.open_url(&url);
            }
            return;
        }
        let (cmd, alt) = crate::app::studio::webview::take_click_modifiers().unwrap_or_else(|| {
            let m = window.modifiers();
            (m.platform, m.alt)
        });
        match super::link_action(self.prefs.open_links, cmd, alt) {
            super::LinkAction::Pane(mode) => self.open_url_in_app(&url, mode, window, cx),
            super::LinkAction::DefaultBrowser => cx.open_url(&url),
        }
    }

    /// Show `url` in the browser pane (http(s) only).
    pub fn open_url_in_app(
        &mut self,
        url: &str,
        mode: OpenMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !super::is_web_url(url) {
            return;
        }
        // The pane is a WKWebView, macOS only for now: elsewhere links go to
        // the default browser, as they did before the pane existed.
        if cfg!(all(not(target_os = "macos"), not(test))) {
            cx.open_url(url);
            return;
        }
        let b = &mut self.browser;
        if !b.open || b.mode != mode {
            b.shown += 1;
        }
        b.open = true;
        b.mode = mode;
        b.editing = false;
        b.close_gen += 1;
        b.teardown = None;
        self.browser_ensure(window, cx);
        let same = self.browser.alive() && self.browser.page.url == url;
        if !same {
            self.browser.page.url = url.to_string();
            self.browser.page.title.clear();
            self.browser_load(url.to_string(), cx);
        }
        if let Some(f) = self.browser.focus.clone() {
            window.focus(&f, cx);
        }
        cx.notify();
    }

    /// ⇧⌘B: close the pane, or bring back the last page.
    pub(crate) fn toggle_browser(
        &mut self,
        _: &ToggleBrowser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.browser.open {
            self.close_browser(window, cx);
        } else if !self.browser.page.url.is_empty() {
            let url = self.browser.page.url.clone();
            let mode = self.browser.mode;
            if self.browser.alive() {
                self.browser.open = true;
                self.browser.shown += 1;
                self.browser.close_gen += 1;
                self.browser.teardown = None;
                if let Some(f) = self.browser.focus.clone() {
                    window.focus(&f, cx);
                }
                cx.notify();
            } else {
                self.browser.page.url.clear();
                self.open_url_in_app(&url, mode, window, cx);
            }
        }
    }

    /// esc: hide the pane; its page stays for `TEARDOWN_AFTER`.
    pub(crate) fn close_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = &mut self.browser;
        if !b.open {
            return;
        }
        b.open = false;
        b.editing = false;
        {
            let mut p = b.placed.borrow_mut();
            p.suppressed = true;
            p.hide();
        }
        b.close_gen += 1;
        let gen_ = b.close_gen;
        b.teardown = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(super::teardown_after())
                .await;
            let _ = this.update(cx, |v, _| {
                if !v.browser.open && v.browser.close_gen == gen_ {
                    // Frees the WebContent process; the URL is kept for ⇧⌘B.
                    v.browser.placed.borrow_mut().surface = None;
                    v.browser.placed.borrow_mut().frame = None;
                    v.browser.poll = None;
                    if std::env::var_os("BLYGGER_TIMING").is_some() {
                        println!("browser-teardown");
                    }
                }
            });
        }));
        // The keyboard back where it was before the pane.
        if self.reading.view == super::super::reading::View::Reading {
            window.focus(&self.reading.focus, cx);
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Make the web view, the address field and the focus handle, and start
    /// keeping the block lists current (all once).
    fn browser_ensure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.focus.is_none() {
            self.browser.focus = Some(cx.focus_handle());
        }
        self.browser_watch_selection(cx); // --- selection ---
        if self.browser.address.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Address"));
            self._subs.push(cx.subscribe_in(
                &input,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::PressEnter { .. } => this.browser_submit_address(window, cx),
                    InputEvent::Blur => {
                        this.browser.editing = false;
                        cx.notify();
                    }
                    _ => {}
                },
            ));
            self.browser.address = Some(input);
        }
        self.browser.global.set(self.prefs.content_blocking);
        self.browser_start_blocker(cx);
        let b = &mut self.browser;
        if b.failed.is_some() || b.alive() {
            return;
        }
        let factory = cx
            .try_global::<surface::FactoryGlobal>()
            .map(|g| g.0.clone())
            .unwrap_or_else(surface::default_factory);
        let (global, unblocked) = (b.global.clone(), b.unblocked.clone());
        let blocking: surface::BlockingFor =
            Rc::new(move |url| super::blocking_applies(global.get(), &unblocked.borrow(), url));
        let t0 = std::time::Instant::now();
        match factory(window, b.events.clone(), b.rules.clone(), blocking) {
            Ok(s) => {
                let mut p = b.placed.borrow_mut();
                p.surface = Some(s);
                p.frame = None;
                p.visible = false;
                b.dark = None;
                if std::env::var_os("BLYGGER_TIMING").is_some() {
                    println!("browser-webview-created ms={}", t0.elapsed().as_millis());
                }
            }
            Err(msg) => {
                eprintln!("blygger: {msg}");
                b.failed = Some(msg);
            }
        }
    }

    /// Load now if the block lists are ready (or blocking is off), else
    /// after they are, waiting at most `RULES_WAIT`.
    fn browser_load(&mut self, url: String, cx: &mut Context<Self>) {
        let ready = self.browser.rules.borrow().ready() || !self.browser.global.get();
        if ready || cfg!(test) {
            #[cfg(test)]
            self.browser.loads.push(url.clone());
            self.browser.with(|s| s.load_url(&url));
            self.browser.page.loading = true;
            self.browser_poll(cx);
            return;
        }
        self.browser.pending = Some(url);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(super::RULES_WAIT).await;
            let _ = this.update(cx, |v, cx| v.browser_flush_pending(cx));
        })
        .detach();
    }

    fn browser_flush_pending(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = self.browser.pending.take() {
            self.browser.with(|s| s.load_url(&url));
            self.browser.page.loading = true;
            self.browser_poll(cx);
            if std::env::var_os("BLYGGER_TIMING").is_some() {
                let ready = self.browser.rules.borrow().ready();
                println!("browser-first-load lists_ready={ready}");
            }
        }
    }

    /// Start keeping the compiled block lists current (once per launch).
    fn browser_start_blocker(&mut self, cx: &mut Context<Self>) {
        if self.browser.blocker_started || !self.prefs.content_blocking {
            return;
        }
        let Some(data_dir) = self.browser.data_dir.clone() else {
            return;
        };
        self.browser.blocker_started = true;
        #[cfg(all(target_os = "macos", not(test)))]
        {
            let fake = matches!(
                crate::connection::mode(cx),
                Some(crate::connection::Mode::Fake) | None
            );
            let download = !fake && std::env::var_os("BLYGGER_NO_BLOCKLIST_DOWNLOAD").is_none();
            let this = cx.entity().downgrade();
            let driver = super::driver::Driver {
                data_dir,
                handle: self.browser.rules.clone(),
                download,
                on_change: Box::new(move |cx| {
                    let _ = this.update(cx, |v, cx| {
                        v.browser.with(|s| s.refresh_blocking());
                        v.browser_flush_pending(cx);
                        cx.notify();
                    });
                }),
            };
            self._tasks
                .push(cx.spawn(async move |_, cx| driver.run(cx).await));
        }
        #[cfg(any(not(target_os = "macos"), test))]
        {
            let _ = (data_dir, cx);
        }
    }

    /// --- selection --- While the pane is open, look every so often at
    /// whether the page has text selected, so "→ Draft" can say it will
    /// quote it. The page has no bridge to the app (see `surface`), so it
    /// can't say so itself; a selection can only be made while the page has
    /// the keyboard, and WebKit drops it when the page loses it.
    fn browser_watch_selection(&mut self, cx: &mut Context<Self>) {
        if self.browser.sel_watch.is_some() {
            return;
        }
        self.browser.sel_watch = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let more = this
                    .update(cx, |v, cx| {
                        if !v.browser.open {
                            v.browser_note_selection(false, cx);
                            v.browser.sel_watch = None;
                            return false;
                        }
                        v.browser_check_selection(cx);
                        true
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        }));
    }

    fn browser_check_selection(&mut self, cx: &mut Context<Self>) {
        let page = self
            .browser
            .with(|s| s.page_has_keyboard())
            .unwrap_or(false);
        if !page {
            return self.browser_note_selection(false, cx);
        }
        let (tx, rx) = async_channel::bounded::<String>(1);
        if !self.browser.selection(tx) {
            return self.browser_note_selection(false, cx);
        }
        cx.spawn(async move |this, cx| {
            let text = rx.recv().await.unwrap_or_default();
            let _ = this.update(cx, |v, cx| {
                v.browser_note_selection(!text.trim().is_empty(), cx)
            });
        })
        .detach();
    }

    /// The page has (or no longer has) a selection.
    pub(crate) fn browser_note_selection(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.browser.selected != on {
            self.browser.selected = on;
            cx.notify();
        }
    }

    /// Read the page's state while it loads (the progress bar, the title).
    fn browser_poll(&mut self, cx: &mut Context<Self>) {
        if self.browser.poll.is_some() {
            return;
        }
        self.browser.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let more = this
                    .update(cx, |v, cx| {
                        v.browser_refresh_state(cx);
                        let loading = v.browser.page.loading;
                        if !loading {
                            v.browser.poll = None;
                        }
                        loading
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        }));
    }

    pub(crate) fn browser_refresh_state(&mut self, cx: &mut Context<Self>) {
        if let Some(st) = self.browser.with(|s| s.state()) {
            // about:blank before the first page: keep the URL we asked for.
            let blank = st.url.is_empty() || st.url == "about:blank";
            let url = if blank {
                self.browser.page.url.clone()
            } else {
                st.url.clone()
            };
            let next = PageState { url, ..st };
            if next != self.browser.page {
                self.browser.page = next;
                cx.notify();
            }
        }
    }

    fn browser_event(&mut self, ev: BrowserEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            BrowserEvent::Changed => {
                self.browser_refresh_state(cx);
                if self.browser.page.loading {
                    self.browser_poll(cx);
                }
            }
            BrowserEvent::OpenExternally(url) => cx.open_url(&url),
            BrowserEvent::NewWindow(url) => {
                self.browser.with(|s| s.load_url(&url));
                self.browser_poll(cx);
            }
            BrowserEvent::Crashed => {
                // The page's process died (memory pressure): load it again.
                let url = self.browser.page.url.clone();
                if self.browser.open && super::is_web_url(&url) {
                    self.browser.with(|s| s.load_url(&url));
                    self.browser_poll(cx);
                }
            }
        }
    }

    fn browser_submit_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.browser.address.clone() else {
            return;
        };
        let text = input.read(cx).value().to_string();
        match super::parse_address(&text) {
            Some(url) => {
                self.browser.editing = false;
                self.browser.page.url = url.clone();
                self.browser.page.title.clear();
                self.browser_load(url, cx);
                if let Some(f) = self.browser.focus.clone() {
                    window.focus(&f, cx);
                }
            }
            None => self.show_toast("Only web addresses (http, https) open here", None, cx),
        }
        cx.notify();
    }

    fn browser_edit_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.browser.address.clone() else {
            return;
        };
        self.browser.with(|s| s.focus_parent());
        self.browser.editing = true;
        let url = self.browser.page.url.clone();
        input.update(cx, |s, cx| {
            s.set_value(url, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    fn browser_back(&mut self, _: &BrowserBack, _: &mut Window, cx: &mut Context<Self>) {
        self.browser.with(|s| s.back());
        self.browser_poll(cx);
    }

    fn browser_forward(&mut self, _: &BrowserForward, _: &mut Window, cx: &mut Context<Self>) {
        self.browser.with(|s| s.forward());
        self.browser_poll(cx);
    }

    fn browser_reload(&mut self, _: &BrowserReload, _: &mut Window, cx: &mut Context<Self>) {
        if self.browser.page.loading {
            self.browser.with(|s| s.stop());
        } else {
            self.browser.with(|s| s.reload());
            self.browser.page.loading = true;
            self.browser_poll(cx);
        }
        cx.notify();
    }

    fn browser_address(&mut self, _: &BrowserAddress, window: &mut Window, cx: &mut Context<Self>) {
        self.browser_edit_address(window, cx);
    }

    /// 🛡: blocking off (or back on) for this page's host, remembered in
    /// `state.json`; the page reloads under the new setting.
    pub(crate) fn browser_toggle_shield(&mut self, cx: &mut Context<Self>) {
        let Some(host) = super::host_of(&self.browser.page.url) else {
            return;
        };
        if !self.browser.global.get() {
            self.show_toast(
                "Content blocking is off in the config",
                Some("content-blocking = true turns it on".into()),
                cx,
            );
            return;
        }
        let hosts: Vec<String> = {
            let mut set = self.browser.unblocked.borrow_mut();
            if !set.remove(&host) {
                set.insert(host);
            }
            set.iter().cloned().collect()
        };
        if let Some(d) = &self.browser.data_dir {
            let _ = blyg_core::state::AppState::update(d, |s| s.browser_unblocked_hosts = hosts);
        }
        self.browser.with(|s| {
            s.refresh_blocking();
            s.reload();
        });
        self.browser.page.loading = true;
        self.browser_poll(cx);
        cx.notify();
    }

    /// "→ Notes": `[title](url)` for the page into the notes drawer (or
    /// the page's selection, quoted with that link).
    pub(crate) fn browser_send_to_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(link) = self.browser_note_link() else {
            return;
        };
        self.notes_add_page(link, window, cx); // --- notes ---
    }

    /// The Markdown link for the current page, if it's a web page.
    pub fn browser_note_link(&self) -> Option<String> {
        let url = &self.browser.page.url;
        super::is_web_url(url).then(|| super::notes_link(&self.browser.page.title, url))
    }

    // ------------------------------------------------------------ onboarding

    /// --- onboarding --- The tutorial's browser step: the pane slides in on
    /// a blank sample page. No web view is made and nothing is loaded (the
    /// tour hides web views anyway); what it replaces comes back with
    /// [`Self::browser_tour_end`].
    pub(crate) fn browser_tour_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser_tour_park(cx);
        let b = &mut self.browser;
        b.page = PageState {
            url: "about:blank".into(),
            title: TOUR_TITLE.into(),
            ..PageState::default()
        };
        if !b.open || b.mode != OpenMode::Slide {
            b.shown += 1;
        }
        b.open = true;
        b.mode = OpenMode::Slide;
        b.editing = false;
        b.close_gen += 1;
        let focus = b.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// --- onboarding --- The tutorial starts: remember the page, mode and
    /// pane (once), and take the pane away.
    pub(crate) fn browser_tour_park(&mut self, cx: &mut Context<Self>) {
        let b = &mut self.browser;
        if b.tour.is_none() {
            b.tour = Some((b.page.clone(), b.mode, b.open));
        }
        self.browser_tour_hide(cx);
    }

    /// --- onboarding --- Take the pane away between the tour's steps (no
    /// teardown: a web view from before the tour keeps its page).
    pub(crate) fn browser_tour_hide(&mut self, cx: &mut Context<Self>) {
        let b = &mut self.browser;
        if b.open {
            b.open = false;
            b.editing = false;
            cx.notify();
        }
    }

    /// --- onboarding --- The tour is over: the page, mode and pane the
    /// user had before it.
    pub(crate) fn browser_tour_end(&mut self, cx: &mut Context<Self>) {
        let b = &mut self.browser;
        let Some((page, mode, open)) = b.tour.take() else {
            return;
        };
        b.page = page;
        b.mode = mode;
        b.editing = false;
        if open {
            b.open = true;
            b.close_gen += 1; // a pending teardown no longer applies
            b.teardown = None;
        } else {
            b.open = false;
        }
        cx.notify();
    }

    /// --- onboarding --- Where the pane is (x, y, w, h) in a window `w`×`h`,
    /// and its toolbar, while it's open.
    pub(crate) fn browser_rects(&self, w: f32, h: f32) -> Option<[(f32, f32, f32, f32); 2]> {
        if !self.browser.open {
            return None;
        }
        let room_w = w - f32::from(self.notes.room());
        let left = f32::from(self.browser.left_edge(px(room_w)));
        let top = crate::app::TITLEBAR_H;
        let pane = (left, top, room_w - left, h - top);
        let chrome = (left, top, room_w - left, CHROME_H);
        Some([pane, chrome])
    }

    /// Per frame, before layout (after `studio_frame`): hide the web view
    /// when the pane is closed or covered, and keep the reader's and the
    /// preview's web views out from under the pane.
    pub(crate) fn browser_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let covered = self.sheet.is_some()
            || self.ai.has_overlay()
            || self.reading.sheet.is_some()
            || self.onboarding.flow.is_some()
            || self.onboarding.tutorial.is_some()
            || self.profile_sheet_open();
        let showing = self.browser.open && !covered;
        {
            let mut p = self.browser.placed.borrow_mut();
            p.suppressed = !showing;
            if !showing {
                p.hide();
            }
        }
        self.browser.viewport_w = window.viewport_size().width;
        // --- notes --- the pane makes room for the drawer (`notes_wrap_browser`),
        // and the drawer cuts the web views off at its edge too.
        let room = self.notes.room();
        let edge = showing.then(|| self.browser.left_edge(self.browser.viewport_w - room));
        let notes_edge = self.notes.left_edge(self.browser.viewport_w);
        self.browser.placed.borrow_mut().set_clip(notes_edge);
        let edge = match (edge, notes_edge) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.studio.clip_webviews(edge);
        let dark = self.palette.dark;
        if self.browser.dark != Some(dark) && self.browser.alive() {
            self.browser.dark = Some(dark);
            self.browser.with(|s| s.set_dark(dark));
        }
        if let Some(input) = &self.browser.address {
            let p = self.palette;
            input.update(cx, |s, _| s.set_editor_style(p.field()));
        }
    }

    /// The Edit menu sends GPUI actions; while a page field has the
    /// keyboard, they go to the page instead (copy, paste, …).
    pub(crate) fn browser_actions(
        &self,
        d: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        fn fwd<A: Action>(
            sel: &'static str,
            cx: &mut Context<MainView>,
        ) -> impl Fn(&A, &mut Window, &mut App) + 'static {
            cx.listener(move |this: &mut MainView, _: &A, _, cx| {
                let page = this.browser.open
                    && this
                        .browser
                        .with(|s| s.page_has_keyboard())
                        .unwrap_or(false);
                if page {
                    this.browser.with(|s| s.edit_command(sel));
                    cx.stop_propagation();
                } else {
                    cx.propagate();
                }
            })
        }
        d.on_action(cx.listener(Self::toggle_browser))
            .capture_action(fwd::<Copy>("copy:", cx))
            .capture_action(fwd::<Cut>("cut:", cx))
            .capture_action(fwd::<Paste>("paste:", cx))
            .capture_action(fwd::<SelectAll>("selectAll:", cx))
            .capture_action(fwd::<Undo>("undo:", cx))
            .capture_action(fwd::<Redo>("redo:", cx))
    }

    /// The pane, over everything below the title bar.
    pub(crate) fn render_browser_pane(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let b = &self.browser;
        if !b.open {
            return None;
        }
        let p = self.palette;
        let focus = b.focus.clone()?;
        let full = b.mode == OpenMode::Full;
        let body: AnyElement = match &b.failed {
            Some(msg) => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(p.muted)
                .text_size(px(13.))
                .child(msg.clone())
                .into_any_element(),
            None => {
                let placed = b.placed.clone();
                div()
                    .id("browser-body")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .bg(p.bg)
                    // --- onboarding --- the tour's sample: nothing is loaded.
                    .when(b.tour.is_some(), |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(p.muted)
                                .text_size(px(13.))
                                .child(TOUR_BODY),
                        )
                    })
                    .child(
                        canvas(
                            |bounds, _, _| bounds,
                            move |_, bounds, _, _| placed.borrow_mut().place(bounds),
                        )
                        .size_full(),
                    )
                    .into_any_element()
            }
        };
        Some(
            self.notes_wrap_browser(
                div()
                    .id("browser-pane")
                    .key_context(CONTEXT)
                    .track_focus(&focus)
                    .on_action(cx.listener(Self::browser_back))
                    .on_action(cx.listener(Self::browser_forward))
                    .on_action(cx.listener(Self::browser_reload))
                    .on_action(cx.listener(Self::browser_address))
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        let k = &ev.keystroke;
                        let bare = !(k.modifiers.platform
                            || k.modifiers.control
                            || k.modifiers.alt
                            || k.modifiers.shift);
                        if bare && k.key == "escape" && !this.browser.editing {
                            cx.stop_propagation();
                            this.close_browser(window, cx);
                        }
                    }))
                    // Clicks on the chrome take the keyboard back from the page.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| {
                            if std::mem::take(&mut this.browser.keep_keyboard) {
                                return; // --- selection ---
                            }
                            this.browser.with(|s| s.focus_parent());
                        }),
                    )
                    .absolute()
                    .top_0() // --- notes --- inside `notes_wrap_browser`'s box
                    .bottom_0()
                    .right_0()
                    .when(full, |d| d.left_0())
                    .when(!full, |d| d.w(relative(SLIDE_WIDTH)))
                    .flex()
                    .flex_col()
                    .bg(p.bg)
                    .rule_l(&p)
                    .shadow_lg()
                    .font_family(SharedString::from(self.prefs.ui().family))
                    .child(self.render_browser_chrome(&p, cx))
                    .child(body)
                    // Slides in from the right (the web view follows the
                    // pane's bounds frame by frame).
                    .map(|d| {
                        if full {
                            return d.into_any_element();
                        }
                        let w = f32::from(b.viewport_w - self.notes.room()) * SLIDE_WIDTH;
                        d.with_animation(
                            ElementId::NamedInteger("browser-slide".into(), b.shown),
                            Animation::new(Duration::from_millis(180))
                                .with_easing(ease_out_quint()),
                            move |d, t| d.right(px(-(1.0 - t) * w)),
                        )
                        .into_any_element()
                    }),
                cx,
            ),
        )
    }

    fn render_browser_chrome(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let b = &self.browser;
        // --- selection --- See `Browser::keep_keyboard` (a child's
        // mouse-down runs before the pane's).
        let keep_selection = || {
            cx.listener(|this: &mut MainView, _: &MouseDownEvent, _, _| {
                this.browser.keep_keyboard = true;
            })
        };
        let pg = &b.page;
        let button = |id: &'static str, label: &'static str, enabled: bool, tip: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into()) // --- notes --- (tests)
                .flex_none()
                .px(px(7.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(13.))
                .text_color(if enabled { p.ink } else { p.muted.opacity(0.5) })
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(p.hover())))
                .tooltip(move |_, cx| cx.new(|_| crate::app::reading::Tip(tip.into())).into())
                .child(label)
        };
        let loading = pg.loading;
        let shield_on = b.blocking_here();
        let has_page = super::is_web_url(&pg.url);
        let address: AnyElement = match (&b.address, b.editing) {
            (Some(input), true) => div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    cx.stop_propagation();
                    this.browser.editing = false;
                    if let Some(f) = this.browser.focus.clone() {
                        window.focus(&f, cx);
                    }
                    cx.notify();
                }))
                .child(Input::new(input))
                .into_any_element(),
            _ => {
                let host = super::host_of(&pg.url).unwrap_or_default();
                let label = super::display_label(&pg.title, &pg.url);
                div()
                    .id("browser-address")
                    .flex_1()
                    .min_w_0()
                    .h(px(26.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .rounded(px(6.))
                    .bg(p.panel())
                    .cursor_text()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.browser_edit_address(window, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.5))
                            .text_color(p.ink)
                            .child(label),
                    )
                    .when(!host.is_empty() && !pg.title.trim().is_empty(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(px(11.5))
                                .text_color(p.muted)
                                .child(host),
                        )
                    })
                    .into_any_element()
            }
        };
        let progress = if loading {
            pg.progress.clamp(0.05, 1.0)
        } else {
            0.0
        };
        div()
            .id("browser-chrome")
            .relative()
            .flex_none()
            .h(px(CHROME_H))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(2.))
            .bg(p.bg)
            .rule_b(p)
            .child(
                button("browser-back", "←", pg.can_back, "Back  ⌘[").on_click(
                    cx.listener(|this, _, window, cx| this.browser_back(&BrowserBack, window, cx)),
                ),
            )
            .child(
                button("browser-forward", "→", pg.can_forward, "Forward  ⌘]").on_click(
                    cx.listener(|this, _, window, cx| {
                        this.browser_forward(&BrowserForward, window, cx)
                    }),
                ),
            )
            .child(
                button(
                    "browser-reload",
                    if loading { "✕" } else { "↻" },
                    has_page,
                    if loading { "Stop" } else { "Reload  ⌘R" },
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.browser_reload(&BrowserReload, window, cx)
                })),
            )
            .child(div().w(px(4.)))
            .child(address)
            .child(div().w(px(4.)))
            .child(
                button(
                    "browser-shield",
                    if shield_on { "🛡 On" } else { "🛡 Off" },
                    has_page,
                    "Content blocking for this site",
                )
                .when(!shield_on, |d| d.text_color(p.muted))
                .on_click(cx.listener(|this, _, _, cx| this.browser_toggle_shield(cx))),
            )
            .child(
                button("browser-external", "↗", has_page, "Open in default browser").on_click(
                    cx.listener(|this, _, _, cx| {
                        let url = this.browser.page.url.clone();
                        if super::is_web_url(&url) {
                            cx.open_url(&url);
                        }
                    }),
                ),
            )
            .child(
                button(
                    "browser-notes",
                    "→ Notes",
                    has_page,
                    "Add [title](url) to your notes (a selection on the page is quoted)  ⇧⌘N",
                )
                .on_mouse_down(MouseButton::Left, keep_selection())
                .on_click(
                    cx.listener(|this, _, window, cx| this.browser_send_to_notes(window, cx)),
                ),
            )
            .child(
                button(
                    "browser-draft",
                    if b.selected {
                        "❝ Quote → Draft"
                    } else {
                        "→ Draft"
                    },
                    has_page,
                    "Quote the passage selected on the page in your draft, with the page's link  ⇧⌘D",
                )
                .when(b.selected && has_page, |d| {
                    d.text_color(p.accent_text()).bg(p.pick())
                })
                .on_mouse_down(MouseButton::Left, keep_selection())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.quote_from(Some(crate::app::notes::QuoteFrom::Browser), window, cx)
                })),
            )
            .child(
                button("browser-close", "×", true, "Close  esc")
                    .on_click(cx.listener(|this, _, window, cx| this.close_browser(window, cx))),
            )
            .when(progress > 0.0, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .bottom_0()
                        .h(px(2.))
                        .w(relative(progress as f32))
                        .bg(p.accent),
                )
            })
    }

    /// `BLYGGER_DEMO=br-…`: the pane on `BLYGGER_DEMO_URL` (a local test
    /// page), with the page probed for blocked resources, the shield
    /// switched off and the page probed again (debug output on stdout).
    pub(crate) fn browser_demo(
        &mut self,
        scenario: &str,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A post open in the reader, then a link from it: the reader's web
        // view is cut off at the pane's edge.
        if scenario == "br-reader" {
            match n {
                0 | 1 => self.reading_demo("rd-reading", n, window, cx),
                _ => {
                    let url = std::env::var("BLYGGER_DEMO_URL")
                        .unwrap_or_else(|_| "https://blyg.example.com/".into());
                    self.open_url_in_app(&url, OpenMode::Slide, window, cx);
                    crate::app::reading::demo::snapshot_later(window, cx);
                }
            }
            return;
        }
        if n != 0 {
            return;
        }
        let url = std::env::var("BLYGGER_DEMO_URL")
            .unwrap_or_else(|_| "https://blyg.example.com/".into());
        let mode = if scenario == "br-full" {
            OpenMode::Full
        } else {
            OpenMode::Slide
        };
        self.show_view(super::super::reading::View::Reading, window, cx);
        let t0 = std::time::Instant::now();
        self.open_url_in_app(&url, mode, window, cx);
        println!("browser-open ms={}", t0.elapsed().as_millis());
        if std::env::var_os("BLYGGER_TIMING").is_some()
            && let Some(pid) = self.browser.with(|s| s.web_process_id()).flatten()
        {
            println!("browser-webcontent-pid={pid}");
        }
        if scenario == "br-teardown" {
            // Load, close, wait for the teardown, then quit (smoke test of
            // the freed WebContent process; see BLYGGER_BROWSER_TEARDOWN_MS).
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let _ = this.update_in(cx, |v, window, cx| {
                    if let Some(pid) = v.browser.with(|s| s.web_process_id()).flatten() {
                        println!("browser-webcontent-pid={pid}");
                    }
                    let t = std::time::Instant::now();
                    v.close_browser(window, cx);
                    println!("browser-closed ms={}", t.elapsed().as_millis());
                });
                cx.background_executor()
                    .timer(super::teardown_after() + Duration::from_secs(3))
                    .await;
                let _ = this.update_in(cx, |v, window, cx| {
                    println!("browser-alive-after-teardown={}", v.browser.alive());
                    let t = std::time::Instant::now();
                    v.toggle_browser(&ToggleBrowser, window, cx);
                    println!(
                        "browser-reopen-after-teardown ms={}",
                        t.elapsed().as_millis()
                    );
                    if let Some(pid) = v.browser.with(|s| s.web_process_id()).flatten() {
                        println!("browser-webcontent-pid={pid}");
                    }
                });
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let _ = this.update(cx, |_, cx| cx.quit());
            })
            .detach();
            return;
        }
        if scenario != "br-block" {
            crate::app::reading::demo::snapshot_later(window, cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let wait_loaded = async |cx: &mut AsyncWindowContext| {
                for _ in 0..100 {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    let done = this
                        .update(cx, |v, _| {
                            !v.browser.page.loading && v.browser.pending.is_none()
                        })
                        .unwrap_or(true);
                    if done {
                        break;
                    }
                }
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
            };
            wait_loaded(cx).await;
            let _ = this.update(cx, |v, _| {
                println!(
                    "browser-loaded url={} title={:?} rules={}",
                    v.browser.page.url,
                    v.browser.page.title,
                    v.browser.rules.borrow().rules
                );
                v.browser
                    .with(|s| s.probe(PROBE_JS, "browser-probe shield=on"));
            });
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let _ = this.update(cx, |v, cx| v.browser_toggle_shield(cx));
            wait_loaded(cx).await;
            let _ = this.update(cx, |v, _| {
                v.browser
                    .with(|s| s.probe(PROBE_JS, "browser-probe shield=off"));
            });
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            // Back on, so the snapshot (and the state file) show the default.
            let _ = this.update(cx, |v, cx| v.browser_toggle_shield(cx));
            wait_loaded(cx).await;
            let _ = this.update(cx, |v, _| {
                v.browser
                    .with(|s| s.probe(PROBE_JS, "browser-probe shield=on-again"));
            });
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            // Shield off here again, then the page itself goes to another
            // host (BLYGGER_DEMO_URL2) whose shield is on: the navigation
            // hook must put the lists back before that page loads.
            if let Ok(url2) = std::env::var("BLYGGER_DEMO_URL2") {
                let _ = this.update(cx, |v, cx| v.browser_toggle_shield(cx));
                wait_loaded(cx).await;
                let js = format!(
                    "(location.href = {}, 'navigating')",
                    crate::app::studio::webview::js_string(&url2)
                );
                let _ = this.update(cx, |v, _| {
                    v.browser.with(|s| s.probe(&js, "browser-navigate"));
                });
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                wait_loaded(cx).await;
                let _ = this.update(cx, |v, _| {
                    v.browser
                        .with(|s| s.probe(PROBE_JS, "browser-probe other-host"));
                });
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
            }
            let _ = this.update_in(cx, |v, window, cx| {
                // esc and ⇧⌘B while the web view is alive: no reload.
                v.close_browser(window, cx);
                let t = std::time::Instant::now();
                v.toggle_browser(&ToggleBrowser, window, cx);
                println!("browser-reopen ms={}", t.elapsed().as_millis());
                crate::app::reading::demo::snapshot_later(window, cx);
            });
        })
        .detach();
    }
}

/// What the demo page shows of its "ads": the third-party image and script
/// (blocked by a network rule) and the `.ad-slot` box (a cosmetic rule).
pub const PROBE_JS: &str = r#"JSON.stringify({
  img: (document.getElementById("ad-img") || {}).naturalWidth || 0,
  script: window.__adScript === true,
  slot: (document.querySelector(".ad-slot") ? getComputedStyle(document.querySelector(".ad-slot")).display : null),
  ipc: typeof window.ipc,
  title: document.title
})"#;
