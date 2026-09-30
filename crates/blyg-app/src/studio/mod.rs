//! The full editor (SPEC § Full editor): a live preview of the working copy,
//! rendered by `blyg-render` exactly as the blyg publishes it and shown in a
//! WKWebView beside the editor.
//!
//! - Modes: ⌘1 write (list + editor), ⌘2 list + editor + preview, ⌘3 full
//!   editor (editor + preview). ⌘E toggles the preview in place (so from ⌘3
//!   it leaves the editor alone on screen). The last mode is remembered in
//!   app state (`state.json`), not the config.
//! - Rendering: ~100 ms after typing pauses, on a background thread; the page
//!   is loaded once through `page_shell` and then patched in place.
//! - Source ↔ preview: clicking a block puts the caret on its line; moving
//!   the caret scrolls the preview to the block (`line_map`).
//!
//! This module is a child of `app` (declared there), so it works on
//! `MainView` directly; `app.rs` only calls the hooks at the bottom.

// --- follow-ups --- scratch-note images in the previews.
pub mod local_media;
pub mod reader;
pub mod resolver;
pub mod sanitize;
pub mod style_cache;
pub mod webview;

#[cfg(test)]
#[path = "ui_tests.rs"]
mod ui_tests;

#[cfg(test)]
#[path = "reader_tests.rs"]
mod reader_tests;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use blyg_core::{Backend, Item, LocalId};
use blyg_render::{RenderOpts, Rendered, ShellOpts, Stats};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{MainView, Mode};
use crate::theme::Palette;
use resolver::{StoreResolver, render_kind};
use style_cache::Theme;
use webview::{Factory, FactoryGlobal, PreviewSurface, SurfaceEvent};

gpui_kit::actions!(blygger, [ViewWrite, ViewSplit, ViewStudio]);

/// Typing pause before the preview re-renders.
pub const DEBOUNCE: Duration = Duration::from_millis(100);

/// Which panes are on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// ⌘1: list + editor.
    #[default]
    Write,
    /// ⌘2: list + editor + preview.
    Split,
    /// ⌘3: editor + preview (the full editor).
    Studio,
    /// ⌘E from the full editor: the editor alone.
    Focus,
}

impl ViewMode {
    pub fn list_visible(self) -> bool {
        matches!(self, ViewMode::Write | ViewMode::Split)
    }

    pub fn preview_visible(self) -> bool {
        matches!(self, ViewMode::Split | ViewMode::Studio)
    }

    /// ⌘E: the preview on or off, leaving the list as it is.
    pub fn toggle_preview(self) -> ViewMode {
        match self {
            ViewMode::Write => ViewMode::Split,
            ViewMode::Split => ViewMode::Write,
            ViewMode::Studio => ViewMode::Focus,
            ViewMode::Focus => ViewMode::Studio,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ViewMode::Write => "write",
            ViewMode::Split => "split",
            ViewMode::Studio => "studio",
            ViewMode::Focus => "focus",
        }
    }

    pub fn parse(s: &str) -> Option<ViewMode> {
        Some(match s {
            "write" => ViewMode::Write,
            "split" => ViewMode::Split,
            "studio" => ViewMode::Studio,
            "focus" => ViewMode::Focus,
            _ => return None,
        })
    }
}

/// The remembered mode (per viewer, in `state.json`).
pub fn load_view(data_dir: Option<&std::path::Path>) -> ViewMode {
    data_dir
        .map(blyg_core::state::AppState::load)
        .and_then(|s| s.view.as_deref().and_then(ViewMode::parse))
        .unwrap_or_default()
}

fn save_view(data_dir: Option<&std::path::Path>, view: ViewMode) {
    if let Some(d) = data_dir {
        let _ = blyg_core::state::AppState::update(d, |s| s.view = Some(view.as_str().into()));
    }
}

/// The document a render is for: a full page load happens when this changes
/// (another item, fragment ⇄ thread, a new theme); otherwise it's a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PageKey {
    item: Option<LocalId>,
    kind: blyg_render::Kind,
    theme_gen: u64,
}

/// The native view and where it was last put, shared with the pane's canvas.
#[derive(Default)]
struct Slot {
    surface: Option<Box<dyn PreviewSurface>>,
    frame: Option<Bounds<Pixels>>,
    visible: bool,
    /// A sheet is open: stay hidden even though the pane is laid out.
    suppressed: bool,
}

impl Slot {
    fn place(&mut self, bounds: Bounds<Pixels>) {
        let Some(s) = self.surface.as_mut() else {
            return;
        };
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

    fn with(&mut self, f: impl FnOnce(&mut dyn PreviewSurface)) {
        if let Some(s) = self.surface.as_mut() {
            f(s.as_mut());
        }
    }
}

pub struct Studio {
    pub view: ViewMode,
    data_dir: Option<PathBuf>,
    slot: Rc<RefCell<Slot>>,
    events: async_channel::Sender<SurfaceEvent>,
    events_rx: Option<async_channel::Receiver<SurfaceEvent>>,
    /// Why there is no WebView (the pane shows this instead).
    failed: Option<String>,
    theme: Theme,
    /// The blyg the theme was fetched for (once per connection).
    theme_for: Option<Option<String>>,
    theme_gen: u64,
    /// What the loaded document shows.
    page: Option<PageKey>,
    /// What the newest render request was for.
    requested: Option<PageKey>,
    ready: bool,
    shown_html: Option<String>,
    /// The newest render (for patches, the status bar and caret sync).
    latest: Option<(PageKey, Rendered)>,
    render_gen: u64,
    render_task: Option<Task<()>>,
    theme_task: Option<Task<()>>,
    scrolled_block: Option<usize>,
    caret_line: usize,
    dark: Option<bool>,
    /// The Reading screen's body (its own WebView, never on screen with
    /// this one).
    pub reader: reader::Reader,
    /// Loads and patches sent (tests).
    #[cfg(test)]
    pub(crate) loads: usize,
}

impl Studio {
    /// The window became active again: if a WebView kept the keyboard (or it
    /// fell to the window itself), give it back to GPUI so typing works.
    pub fn reclaim_keyboard(&self) {
        self.slot.borrow_mut().with(|s| s.reclaim_keyboard());
        self.reader.reclaim_keyboard();
    }

    pub fn new(data_dir: Option<PathBuf>) -> Studio {
        let (tx, rx) = async_channel::unbounded();
        Studio {
            view: load_view(data_dir.as_deref()),
            data_dir,
            slot: Rc::default(),
            events: tx,
            events_rx: Some(rx),
            failed: None,
            theme: Theme::builtin(),
            theme_for: None,
            theme_gen: 0,
            page: None,
            requested: None,
            ready: false,
            shown_html: None,
            latest: None,
            render_gen: 0,
            render_task: None,
            theme_task: None,
            scrolled_block: None,
            caret_line: 0,
            dark: None,
            reader: reader::Reader::new(),
            #[cfg(test)]
            loads: 0,
        }
    }

    pub fn stats(&self) -> Option<&Stats> {
        self.latest.as_ref().map(|(_, r)| &r.stats)
    }

    /// The fallback message, when the WebView couldn't be created.
    pub fn failed(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    fn push(&mut self) {
        let Some((key, out)) = self.latest.clone() else {
            return;
        };
        let mut slot = self.slot.borrow_mut();
        if slot.surface.is_none() {
            return;
        }
        if self.page.as_ref() != Some(&key) {
            let base = self.base_href();
            let body = blyg_render::article_html(key.kind, &out.html, None, None);
            let page = blyg_render::page_shell_with(
                &self.theme.css,
                &body,
                &ShellOpts {
                    title: "Preview".into(),
                    base_href: base,
                    ..Default::default()
                },
            );
            slot.with(|s| s.load(&page));
            self.page = Some(key);
            self.ready = false;
            self.shown_html = Some(out.html);
            self.scrolled_block = None;
            #[cfg(test)]
            {
                self.loads += 1;
            }
        } else if self.ready && self.shown_html.as_deref() != Some(out.html.as_str()) {
            slot.with(|s| s.eval(&webview::patch_js(&out.html)));
            if std::env::var_os("BLYGGER_TIMING").is_some() {
                slot.with(|s| s.probe());
            }
            self.shown_html = Some(out.html);
        }
    }

    fn base_href(&self) -> Option<String> {
        self.theme_for
            .clone()
            .flatten()
            .map(|b| format!("{}/", b.trim_end_matches('/')))
    }

    /// The `[data-line]` block for a source line.
    pub fn block_for_line(line_map: &[(usize, usize)], line: usize) -> Option<usize> {
        line_map
            .iter()
            .take_while(|(l, _)| *l <= line)
            .last()
            .or(line_map.first())
            .map(|(_, b)| *b)
    }

    fn sync_scroll(&mut self) {
        if !self.ready {
            return;
        }
        let Some((_, out)) = &self.latest else {
            return;
        };
        let Some(block) = Self::block_for_line(&out.line_map, self.caret_line) else {
            return;
        };
        if self.scrolled_block != Some(block) {
            self.scrolled_block = Some(block);
            self.slot
                .borrow_mut()
                .with(|s| s.eval(&webview::scroll_js(block)));
        }
    }
}

/// `N thing(s)`.
fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The status bar's counts: quotes, AI spans, videos, images (zeros left out).
pub fn stats_label(s: &Stats) -> Option<String> {
    let parts: Vec<String> = [
        (s.quotes, "quote", "quotes"),
        (s.ai_spans, "AI span", "AI spans"),
        (s.videos, "video", "videos"),
        (s.images, "image", "images"),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, one, many)| count(n, one, many))
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// "⚠ N quote(s) can't be resolved" (publish would refuse them).
pub fn unresolved_warning(s: &Stats) -> Option<String> {
    let n = s.unresolved.len();
    (n > 0).then(|| {
        if n == 1 {
            "⚠ 1 quote can't be resolved".to_string()
        } else {
            format!("⚠ {n} quotes can't be resolved")
        }
    })
}

fn render_opts(base_url: Option<&str>, item: Option<&Item>) -> RenderOpts {
    RenderOpts {
        mount: base_url
            .map(|b| b.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "/blyg".into()),
        self_id: item.and_then(|i| i.server_id.as_ref()).map(|s| s.0.clone()),
        ..RenderOpts::default()
    }
}

/// Everything a render needs besides the text: the options, and a snapshot
/// of the local store as the resolver (only when the text quotes anything).
/// Local and instant, so it's taken on the UI thread.
fn prepare(backend: &dyn Backend, item: Option<&Item>, md: &str) -> (StoreResolver, RenderOpts) {
    let base = backend.base_url();
    let opts = render_opts(base.as_deref(), item);
    let resolver = if md.contains("![[") {
        StoreResolver::snapshot(backend, &opts.mount)
    } else {
        StoreResolver::empty(&opts.mount)
    };
    (resolver, opts)
}

/// Render a working copy with the local store as the resolver.
pub fn render_doc(backend: &dyn Backend, item: Option<&Item>, md: &str) -> Rendered {
    let (resolver, opts) = prepare(backend, item, md);
    let kind = render_kind(item.map_or(blyg_core::Kind::Fragment, |i| i.kind));
    let mut out = blyg_render::render_preview(md, kind, &resolver, &opts);
    with_local_images(backend, md, &mut out);
    out
}

/// Scratch-note images (`blyg-local:…`) only exist on this Mac: inline them.
fn with_local_images(backend: &dyn Backend, md: &str, out: &mut Rendered) {
    if md.contains(blyg_core::scratch_media::SCHEME) {
        out.html = local_media::inline_images(&out.html, |u| backend.scratch_media_file(u));
    }
}

// ================================================================ MainView hooks

impl MainView {
    /// Wire the preview's events into the view (called once from `new`).
    pub(super) fn studio_init(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rx) = self.studio.events_rx.take() {
            self._tasks.push(cx.spawn_in(window, async move |this, cx| {
                while let Ok(ev) = rx.recv().await {
                    if this
                        .update_in(cx, |v, window, cx| v.studio_event(ev, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        if let Some(rx) = self.studio.reader.events_rx.take() {
            self._tasks.push(cx.spawn_in(window, async move |this, cx| {
                while let Ok(ev) = rx.recv().await {
                    if this
                        .update_in(cx, |v, window, cx| v.reader_event(ev, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        self._subs
            .push(cx.observe(&self.editor, |this, editor, cx| {
                let line = editor.read(cx).cursor_position().line as usize;
                if line != this.studio.caret_line {
                    this.studio.caret_line = line;
                    this.studio.sync_scroll();
                }
            }));
        if self.studio.view.preview_visible() {
            self.studio_refresh(true, cx);
        }
    }

    fn studio_event(&mut self, ev: SurfaceEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            SurfaceEvent::Ready => {
                self.studio.ready = true;
                self.studio.push();
                self.studio.sync_scroll();
                if std::env::var_os("BLYGGER_TIMING").is_some() {
                    let frame = self.studio.slot.borrow().frame;
                    println!("preview-ready frame={frame:?}");
                    self.studio.slot.borrow_mut().with(|s| s.probe());
                }
            }
            SurfaceEvent::JumpToLine(line) => {
                self.studio.slot.borrow_mut().with(|s| s.focus_parent());
                if self.current.is_some() {
                    self.studio.caret_line = line;
                    self.studio.scrolled_block = None;
                    self.mode = Mode::Edit;
                    self.editor.update(cx, |s, cx| {
                        let last = s.value().matches('\n').count();
                        let pos = gpui_kit::base::input::Position {
                            line: line.min(last) as u32,
                            character: 0,
                        };
                        s.set_cursor_position(pos, window, cx);
                    });
                    cx.notify();
                }
            }
            SurfaceEvent::Refocus => {
                self.studio.slot.borrow_mut().with(|s| s.focus_parent());
                if self.mode == Mode::Edit {
                    self.editor.update(cx, |s, cx| s.focus(window, cx));
                }
            }
            SurfaceEvent::OpenUrl(url) => cx.open_url(&url),
            SurfaceEvent::OpenOrigin(origin) => self.open_profile(origin, window, cx),
        }
    }

    /// Switch modes (⌘1/⌘2/⌘3/⌘E) and remember the choice.
    pub(super) fn studio_set_view(&mut self, view: ViewMode, cx: &mut Context<Self>) {
        if view == self.studio.view {
            return;
        }
        let was = self.studio.view.preview_visible();
        self.studio.view = view;
        save_view(self.studio.data_dir.as_deref(), view);
        if view.preview_visible() && !was {
            self.studio_refresh(true, cx);
        }
        cx.notify();
    }

    pub(super) fn view_write(&mut self, _: &ViewWrite, _: &mut Window, cx: &mut Context<Self>) {
        self.studio_set_view(ViewMode::Write, cx);
    }

    pub(super) fn view_split(&mut self, _: &ViewSplit, _: &mut Window, cx: &mut Context<Self>) {
        self.studio_set_view(ViewMode::Split, cx);
    }

    pub(super) fn view_studio(&mut self, _: &ViewStudio, _: &mut Window, cx: &mut Context<Self>) {
        self.studio_set_view(ViewMode::Studio, cx);
    }

    /// The working copy changed (or another item was selected): re-render,
    /// after a typing pause unless `now`.
    pub(super) fn studio_refresh(&mut self, now: bool, cx: &mut Context<Self>) {
        if !self.studio.view.preview_visible() {
            // Rendered again when the preview comes back.
            self.studio.latest = None;
            self.studio.requested = None;
            self.studio.render_task = None;
            return;
        }
        self.studio_theme(cx);
        let item = self.current.clone();
        let md = if item.is_some() {
            self.editor.read(cx).value().to_string()
        } else {
            String::new()
        };
        let key = self.studio_key();
        self.studio.requested = Some(key.clone());
        let (resolver, opts) = prepare(&*self.backend, item.as_ref(), &md);
        let backend = self.backend.clone();
        self.studio.render_gen += 1;
        let gen_ = self.studio.render_gen;
        let delay = if now { None } else { Some(DEBOUNCE) };
        self.studio.render_task = Some(cx.spawn(async move |this, cx| {
            if let Some(d) = delay {
                cx.background_executor().timer(d).await;
            }
            let out = cx
                .background_spawn(async move {
                    let mut out = blyg_render::render_preview(&md, key.kind, &resolver, &opts);
                    with_local_images(&*backend, &md, &mut out);
                    out
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if v.studio.render_gen == gen_ {
                    v.studio.latest = Some((key, out));
                    v.studio.push();
                    v.studio.sync_scroll();
                    cx.notify();
                }
            });
        }));
    }

    /// Load the blyg's theme once per connection: the cache at once, then a
    /// fresh copy in the background (never in fake mode, never with a token).
    fn studio_theme(&mut self, cx: &mut Context<Self>) {
        let base = self.backend.base_url();
        if self.studio.theme_for.as_ref() == Some(&base) {
            return;
        }
        self.studio.theme_for = Some(base.clone());
        let data_dir = self.studio.data_dir.clone();
        let theme = style_cache::initial(data_dir.as_deref(), base.as_deref());
        self.studio_set_theme(theme);
        let fake = matches!(
            crate::connection::mode(cx),
            Some(crate::connection::Mode::Fake) | None
        );
        let Some(base) = base.filter(|_| !fake) else {
            return;
        };
        self.studio.theme_task = Some(cx.spawn(async move |this, cx| {
            let b = base.clone();
            let theme = cx
                .background_spawn(async move {
                    let client = blyg_core::api::public::PublicClient::new();
                    style_cache::refresh(&client, data_dir.as_deref(), &b)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                if v.studio.theme_for.as_ref() == Some(&Some(base))
                    && v.studio.theme.css != theme.css
                {
                    v.studio_set_theme(theme);
                    v.studio_refresh(true, cx);
                }
            });
        }));
    }

    fn studio_key(&self) -> PageKey {
        PageKey {
            item: self.current.as_ref().map(|i| i.local_id.clone()),
            kind: render_kind(
                self.current
                    .as_ref()
                    .map_or(blyg_core::Kind::Fragment, |i| i.kind),
            ),
            theme_gen: self.studio.theme_gen,
        }
    }

    fn studio_set_theme(&mut self, theme: Theme) {
        if theme != self.studio.theme {
            self.studio.theme = theme;
            self.studio.theme_gen += 1;
        }
    }

    /// Per frame, before layout: create the WebView the first time it's
    /// needed, and hide it whenever the pane isn't on screen or a sheet is
    /// open (a native view would sit on top of GPUI's sheets).
    pub(super) fn studio_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The Reading screen's body first: where it shows, this one doesn't.
        self.reader_frame(window, cx);
        let wanted = self.studio.view.preview_visible();
        // Another item or kind (⌘T, a sync) without an edit: render it now.
        if wanted && self.studio.requested.as_ref() != Some(&self.studio_key()) {
            self.studio_refresh(true, cx);
        }
        if wanted && self.studio.failed.is_none() && self.studio.slot.borrow().surface.is_none() {
            let factory: Factory = cx
                .try_global::<FactoryGlobal>()
                .map(|g| g.0.clone())
                .unwrap_or_else(webview::default_factory);
            match factory(window, self.studio.events.clone()) {
                Ok(surface) => {
                    self.studio.slot.borrow_mut().surface = Some(surface);
                    self.studio.page = None;
                    self.studio.push();
                }
                Err(msg) => {
                    eprintln!("blygger: {msg}");
                    self.studio.failed = Some(msg)
                }
            }
        }
        let dark = self.palette.dark;
        if self.studio.dark != Some(dark) {
            self.studio.dark = Some(dark);
            self.studio.slot.borrow_mut().with(|s| s.set_dark(dark));
        }
        let mut slot = self.studio.slot.borrow_mut();
        // Anything drawn over (or instead of) the preview pane: GPUI sheets, AI
        // overlays, the reading screens and their sheets, and your own post's
        // history (⌘Y), whose body is the reader's WebView.
        slot.suppressed = self.sheet.is_some()
            || self.ai.has_overlay()
            || self.reading.sheet.is_some()
            || self.profile_sheet_open() // --- profiles ---
            || !matches!(self.reading.view, super::reading::View::Posts)
            || self.reading.own.is_some()
            || self.studio.reader.active();
        if !wanted || slot.suppressed {
            slot.hide();
        }
    }

    /// The preview pane: the WebView is placed over this element's bounds.
    pub(super) fn studio_pane(&self, p: &Palette) -> AnyElement {
        let base = div()
            .id("preview-pane")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .border_l_1()
            .border_color(p.line)
            .bg(p.bg);
        if let Some(msg) = self.studio.failed().map(str::to_string) {
            return base
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .text_color(p.muted)
                .text_size(px(13.))
                .font_family("Inter")
                .child(msg)
                .into_any_element();
        }
        let slot = self.studio.slot.clone();
        base.child(
            canvas(
                |bounds, _, _| bounds,
                move |_, bounds, _, _| slot.borrow_mut().place(bounds),
            )
            .size_full(),
        )
        .into_any_element()
    }

    /// The status bar's preview segment (preview modes only).
    pub(super) fn studio_status(&self, p: &Palette) -> Option<AnyElement> {
        if !self.studio.view.preview_visible() || self.current.is_none() {
            return None;
        }
        let stats = self.studio.stats()?;
        let label = stats_label(stats);
        let warn = unresolved_warning(stats);
        if label.is_none() && warn.is_none() {
            return None;
        }
        Some(
            div()
                .id("studio-stats")
                .flex()
                .gap(px(10.))
                .children(label)
                .when_some(warn, |d, w| d.child(div().text_color(p.warn).child(w)))
                .into_any_element(),
        )
    }

    /// The publish sheet's warning about quotes publish would refuse.
    pub(super) fn studio_publish_warning(&self, cx: &App) -> Option<String> {
        let item = self.current.as_ref()?;
        if item.kind != blyg_core::Kind::Thread {
            return None;
        }
        let md = self.editor.read(cx).value();
        if !md.contains("![[") {
            return None;
        }
        unresolved_warning(&render_doc(&*self.backend, Some(item), &md).stats)
    }

    /// `BLYGGER_DEMO=studio`: the sample thread in the full editor.
    pub(super) fn studio_demo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let md = crate::fake::studio_sample();
        if let Ok(id) = self.backend.create_draft(blyg_core::Kind::Thread, &md) {
            let results = self.backend.search("");
            self.list.refresh(results);
            self.open(&id, window, cx);
            self.editor.update(cx, |s, cx| {
                s.set_cursor_position(
                    gpui_kit::base::input::Position {
                        line: 2,
                        character: 0,
                    },
                    window,
                    cx,
                )
            });
        }
        self.studio_set_view(ViewMode::Studio, cx);
    }

    /// Where a toast goes so the native preview doesn't cover it.
    pub(super) fn studio_toast_right(&self) -> Pixels {
        if let Some(r) = self.studio.reader.toast_right() {
            return r;
        }
        let slot = self.studio.slot.borrow();
        match (slot.visible, slot.frame) {
            (true, Some(f)) => f.size.width + px(14.),
            _ => px(14.),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Stats, Studio, ViewMode, load_view, save_view, stats_label, unresolved_warning};

    #[test]
    fn modes() {
        use ViewMode::*;
        assert!(Write.list_visible() && !Write.preview_visible());
        assert!(Split.list_visible() && Split.preview_visible());
        assert!(!Studio.list_visible() && Studio.preview_visible());
        assert!(!Focus.list_visible() && !Focus.preview_visible());
        assert_eq!(Write.toggle_preview(), Split);
        assert_eq!(Split.toggle_preview(), Write);
        assert_eq!(Studio.toggle_preview(), Focus);
        assert_eq!(Focus.toggle_preview(), Studio);
        for m in [Write, Split, Studio, Focus] {
            assert_eq!(ViewMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(ViewMode::parse("zen"), None);
    }

    #[test]
    fn the_mode_is_remembered_in_app_state() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_view(Some(dir.path())), ViewMode::Write);
        save_view(Some(dir.path()), ViewMode::Studio);
        assert_eq!(load_view(Some(dir.path())), ViewMode::Studio);
        let state = std::fs::read_to_string(dir.path().join("state.json")).unwrap();
        assert!(state.contains("\"studio\""), "{state}");
        assert_eq!(load_view(None), ViewMode::Write);
    }

    #[test]
    fn blocks_for_lines() {
        let map = [(0, 0), (2, 1), (5, 2)];
        assert_eq!(Studio::block_for_line(&map, 0), Some(0));
        assert_eq!(Studio::block_for_line(&map, 1), Some(0));
        assert_eq!(Studio::block_for_line(&map, 2), Some(1));
        assert_eq!(Studio::block_for_line(&map, 99), Some(2));
        assert_eq!(Studio::block_for_line(&[(3, 0)], 1), Some(0));
        assert_eq!(Studio::block_for_line(&[], 1), None);
    }

    #[test]
    fn status_labels() {
        let mut s = Stats::default();
        assert_eq!(stats_label(&s), None);
        assert_eq!(unresolved_warning(&s), None);
        s.quotes = 2;
        s.ai_spans = 1;
        s.images = 3;
        assert_eq!(
            stats_label(&s).as_deref(),
            Some("2 quotes · 1 AI span · 3 images")
        );
        s.unresolved.push(blyg_render::Unresolved {
            line: 4,
            directive: "![[x]]".into(),
            reason: blyg_render::UnresolvedReason::UnknownItem,
        });
        assert_eq!(
            unresolved_warning(&s).as_deref(),
            Some("⚠ 1 quote can't be resolved")
        );
        s.unresolved.push(s.unresolved[0].clone());
        assert_eq!(
            unresolved_warning(&s).as_deref(),
            Some("⚠ 2 quotes can't be resolved")
        );
    }
}
