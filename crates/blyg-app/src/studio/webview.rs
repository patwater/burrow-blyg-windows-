//! The native preview surface: a WKWebView (through `wry`) attached as a
//! child of GPUI's NSView and positioned over the preview pane. On Windows
//! it is a WebView2 child window of GPUI's HWND instead (`wry_surface_windows`).
//!
//! The rest of the app sees only [`PreviewSurface`], so headless tests run on
//! a stub and a machine without a usable WebView falls back to a message.
//!
//! What runs in the page:
//! - `blyg-render`'s own `preview_script` (click-to-play, image fallback),
//!   under the page's CSP nonce;
//! - [`HOST_SCRIPT`], injected by the host as a WKUserScript (outside the
//!   page's CSP, like `evaluate_script`): it patches `.item-content` in place,
//!   scrolls to a block, and reports clicks on blocks over IPC.
//!
//! Content never runs script: the page's CSP only admits the nonce'd script.
//! Every navigation is refused; an http(s) link goes to the default browser.

use async_channel::Sender;
use gpui_kit::{Bounds, Pixels, Window};

/// What the page tells the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceEvent {
    /// The page finished parsing (after a full load).
    Ready,
    /// A block was clicked: put the caret on this 0-based source line.
    JumpToLine(usize),
    /// The page was clicked somewhere that isn't a link: give the keyboard
    /// back to the editor.
    Refocus,
    /// A link was followed: open it in the default browser.
    OpenUrl(String),
    /// A quote from another blyg was clicked: open that origin's profile.
    OpenOrigin(String),
}

/// A place to show the preview page. `wry` implements it for real; tests use
/// a recording stub.
pub trait PreviewSurface {
    /// Window coordinates (logical pixels, top-left origin).
    fn set_frame(&mut self, bounds: Bounds<Pixels>);
    fn set_visible(&mut self, visible: bool);
    /// Replace the whole document.
    fn load(&mut self, html: &str);
    /// Run host script in the current document.
    fn eval(&mut self, js: &str);
    /// Hand the keyboard back to GPUI's view.
    fn focus_parent(&mut self);
    /// Hand the keyboard back to GPUI's view if the page (or no view at all)
    /// has it. Called when the window becomes active again.
    fn reclaim_keyboard(&mut self) {}
    /// Follow the app's light/dark choice (the page's `prefers-color-scheme`).
    fn set_dark(&mut self, dark: bool);
    /// `BLYGGER_TIMING=1`: print what the page shows (smoke tests).
    fn probe(&mut self) {}
}

/// What a surface has been asked to do since it last caught up, coalesced:
/// the last frame, visibility and theme win, a new page drops script queued
/// for the old one, and script runs in order after the page. Windows applies
/// these from its top-level message loop rather than at once
/// (`wry_surface_windows`).
#[cfg_attr(not(all(target_os = "windows", not(test))), allow(dead_code))]
#[derive(Debug, Default, PartialEq)]
pub struct Ops {
    frame: Option<Bounds<Pixels>>,
    visible: Option<bool>,
    dark: Option<bool>,
    html: Option<String>,
    evals: Vec<String>,
    focus_parent: bool,
    probe: bool,
}

#[cfg_attr(not(all(target_os = "windows", not(test))), allow(dead_code))]
impl Ops {
    pub fn is_empty(&self) -> bool {
        *self == Ops::default()
    }

    pub fn set_frame(&mut self, bounds: Bounds<Pixels>) {
        self.frame = Some(bounds);
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = Some(visible);
    }

    pub fn set_dark(&mut self, dark: bool) {
        self.dark = Some(dark);
    }

    pub fn load(&mut self, html: &str) {
        self.html = Some(html.to_string());
        self.evals.clear();
    }

    pub fn eval(&mut self, js: &str) {
        self.evals.push(js.to_string());
    }

    pub fn focus_parent(&mut self) {
        self.focus_parent = true;
    }

    pub fn probe(&mut self) {
        self.probe = true;
    }

    /// Do it all, in an order that never shows a stale frame or page.
    pub fn apply(self, s: &mut dyn PreviewSurface) {
        if let Some(b) = self.frame {
            s.set_frame(b);
        }
        if let Some(d) = self.dark {
            s.set_dark(d);
        }
        if let Some(h) = &self.html {
            s.load(h);
        }
        for js in &self.evals {
            s.eval(js);
        }
        if self.focus_parent {
            s.focus_parent();
        }
        if let Some(v) = self.visible {
            s.set_visible(v);
        }
        if self.probe {
            s.probe();
        }
    }
}

/// Reports the page's geometry and what it rendered, as one JSON line.
#[cfg_attr(test, allow(dead_code))]
pub const PROBE_JS: &str = r#"JSON.stringify({
  w: window.innerWidth, h: window.innerHeight,
  quotes: document.querySelectorAll("blockquote.blyg-transclusion:not(.unresolved)").length,
  unresolved: document.querySelectorAll("blockquote.blyg-transclusion.unresolved").length,
  tk: document.querySelectorAll(".blyg-tk-gen").length,
  tint: (document.querySelector(".blyg-tk-gen") ? getComputedStyle(document.querySelector(".blyg-tk-gen")).backgroundColor : null),
  yt: document.querySelectorAll("figure.blyg-yt").length,
  blocks: document.querySelectorAll(".item-content [data-line]").length,
  host: !!window.__blyg,
  edited: document.body.textContent.indexOf("Tide tables") >= 0,
  bg: getComputedStyle(document.body).backgroundColor
})"#;

/// Makes a surface for a window. Returns a sentence for the fallback message
/// when it can't.
pub type Factory = std::rc::Rc<
    dyn Fn(&mut Window, Sender<SurfaceEvent>) -> Result<Box<dyn PreviewSurface>, String>,
>;

/// Overrides the surface factory (tests install a stub).
pub struct FactoryGlobal(pub Factory);
impl gpui_kit::Global for FactoryGlobal {}

/// The default factory: a real WKWebView (WebView2 on Windows), except in
/// unit tests (no surface at all, so no test ever needs a WebView).
pub fn default_factory() -> Factory {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        std::rc::Rc::new(|window, tx| {
            wry_surface::WrySurface::new(window, tx).map(|s| Box::new(s) as Box<dyn PreviewSurface>)
        })
    }
    #[cfg(all(target_os = "windows", not(test)))]
    {
        std::rc::Rc::new(|window, tx| {
            wry_surface_windows::DeferredSurface::new(window, tx)
                .map(|s| Box::new(s) as Box<dyn PreviewSurface>)
        })
    }
    #[cfg(any(not(any(target_os = "macos", target_os = "windows")), test))]
    {
        std::rc::Rc::new(|_, _| Err("The preview isn't available here.".to_string()))
    }
}

/// Decode one IPC message from [`HOST_SCRIPT`].
pub fn parse_ipc(msg: &str) -> Option<SurfaceEvent> {
    if msg == "ready" {
        return Some(SurfaceEvent::Ready);
    }
    if msg == "focus" {
        return Some(SurfaceEvent::Refocus);
    }
    if let Some(o) = msg.strip_prefix("origin:") {
        let web = o.starts_with("https://") || o.starts_with("http://");
        return web.then(|| SurfaceEvent::OpenOrigin(o.to_string()));
    }
    msg.strip_prefix("line:")
        .and_then(|n| n.parse().ok())
        .map(SurfaceEvent::JumpToLine)
}

/// What to do with a navigation the page asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nav {
    /// Our own document load (`loadHTMLString` → `about:blank`), or a YouTube
    /// embed inside its (CSP-limited) iframe.
    Allow,
    /// A link: refuse it here, open it in the browser.
    OpenExternally(String),
    /// Anything else: refuse.
    Deny,
}

pub fn navigation(url: &str) -> Nav {
    let lower = url.to_ascii_lowercase();
    if lower == "about:blank" || lower.starts_with("about:srcdoc") {
        return Nav::Allow;
    }
    for host in [
        "https://www.youtube-nocookie.com/embed/",
        "https://youtube-nocookie.com/embed/",
    ] {
        if lower.starts_with(host) {
            return Nav::Allow;
        }
    }
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
    {
        return Nav::OpenExternally(url.to_string());
    }
    Nav::Deny
}

/// A JavaScript string literal (JSON rules; also safe inside `<script>`).
pub fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Patch `.item-content` to `html`, keeping unchanged blocks (and scroll).
pub fn patch_js(html: &str) -> String {
    format!("window.__blyg && window.__blyg.patch({});", js_string(html))
}

/// Bring the `index`-th `[data-line]` block into view (if it isn't).
pub fn scroll_js(index: usize) -> String {
    format!("window.__blyg && window.__blyg.scrollTo({index});")
}

/// Host-side helpers, injected at document start (main frame only).
#[cfg_attr(test, allow(dead_code))]
pub const HOST_SCRIPT: &str = r#"
(function () {
  "use strict";
  if (window.__blyg) return;
  function post(m) { try { window.ipc.postMessage(m); } catch (e) {} }
  function root() { return document.querySelector(".item-content"); }
  function key(n) { return n.nodeType === 1 ? n.outerHTML : n.textContent; }
  function tag() {
    var r = root(); if (!r) return;
    for (var i = 0; i < r.childNodes.length; i++) r.childNodes[i].__blygSrc = key(r.childNodes[i]);
  }
  // Replace only the top-level nodes whose rendered HTML changed, so a playing
  // video, loaded images and the scroll position survive typing.
  function patch(html) {
    var r = root(); if (!r) return;
    var t = document.createElement("template");
    t.innerHTML = html;
    var fresh = Array.prototype.slice.call(t.content.childNodes);
    var old = Array.prototype.slice.call(r.childNodes);
    for (var i = 0; i < fresh.length; i++) {
      var k = key(fresh[i]), o = old[i];
      if (o && o.__blygSrc === k) continue;
      fresh[i].__blygSrc = k;
      if (o) r.replaceChild(fresh[i], o); else r.appendChild(fresh[i]);
    }
    for (var j = fresh.length; j < old.length; j++) r.removeChild(old[j]);
  }
  function scrollTo(i) {
    var r = root(); if (!r) return;
    var el = r.querySelectorAll("[data-line]")[i];
    if (!el) return;
    var b = el.getBoundingClientRect();
    if (b.top >= 0 && b.bottom <= window.innerHeight) return;
    el.scrollIntoView({ block: b.height > window.innerHeight ? "start" : "center", behavior: "smooth" });
  }
  document.addEventListener("click", function (e) {
    var t = e.target;
    if (!t || !t.closest || t.closest("a[href]") || t.closest(".blyg-yt")) return;
    // A quote from another blyg: its profile.
    var q = t.closest("blockquote[data-blyg-origin]");
    if (q) { post("origin:" + q.getAttribute("data-blyg-origin")); return; }
    var b = t.closest(".item-content [data-line]");
    post(b ? "line:" + b.getAttribute("data-line") : "focus");
  }, true);
  document.addEventListener("DOMContentLoaded", function () { tag(); post("ready"); });
  window.__blyg = { patch: patch, scrollTo: scrollTo };
})();
"#;

#[cfg(all(target_os = "macos", not(test)))]
mod wry_surface {
    use super::*;
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder, WebViewExtMacOS};

    pub struct WrySurface {
        view: WebView,
    }

    fn rect(b: Bounds<Pixels>) -> Rect {
        Rect {
            position: LogicalPosition::new(f64::from(b.origin.x), f64::from(b.origin.y)).into(),
            size: LogicalSize::new(
                f64::from(b.size.width).max(1.0),
                f64::from(b.size.height).max(1.0),
            )
            .into(),
        }
    }

    /// Where the window's keyboard is, as far as the WebView is concerned.
    #[derive(PartialEq, Eq)]
    enum Keyboard {
        /// The WebView (or something inside it) is first responder.
        InPage,
        /// No view has it: the window itself (or nothing) is first responder.
        Nowhere,
        /// Some other view, normally GPUI's.
        Elsewhere,
    }

    impl WrySurface {
        fn keyboard(&self) -> Keyboard {
            let wk = self.view.webview();
            // SAFETY: main thread; plain AppKit getters on live objects.
            unsafe {
                use objc2::runtime::AnyObject;
                let win: *mut AnyObject = objc2::msg_send![&*wk, window];
                if win.is_null() {
                    return Keyboard::Elsewhere;
                }
                let responder: *mut AnyObject = objc2::msg_send![win, firstResponder];
                if responder.is_null() || std::ptr::eq(responder, win) {
                    return Keyboard::Nowhere;
                }
                let is_view: bool = objc2::msg_send![
                    responder,
                    respondsToSelector: objc2::sel!(isDescendantOf:)
                ];
                if !is_view {
                    return Keyboard::Nowhere;
                }
                let mine: bool = objc2::msg_send![responder, isDescendantOf: &*wk];
                if mine {
                    Keyboard::InPage
                } else {
                    Keyboard::Elsewhere
                }
            }
        }

        pub fn new(window: &mut Window, tx: Sender<SurfaceEvent>) -> Result<Self, String> {
            let (ipc_tx, nav_tx, new_tx) = (tx.clone(), tx.clone(), tx);
            let view = WebViewBuilder::new()
                .with_bounds(rect(Bounds::default()))
                .with_visible(false)
                .with_focused(false)
                .with_accept_first_mouse(true)
                .with_initialization_script_for_main_only(HOST_SCRIPT, true)
                .with_ipc_handler(move |req| {
                    if let Some(ev) = parse_ipc(req.body()) {
                        let _ = ipc_tx.try_send(ev);
                    }
                })
                .with_navigation_handler(move |url| match navigation(&url) {
                    Nav::Allow => true,
                    Nav::OpenExternally(u) => {
                        let _ = nav_tx.try_send(SurfaceEvent::OpenUrl(u));
                        false
                    }
                    Nav::Deny => false,
                })
                .with_new_window_req_handler(move |url, _| {
                    if let Nav::OpenExternally(u) = navigation(&url) {
                        let _ = new_tx.try_send(SurfaceEvent::OpenUrl(u));
                    }
                    NewWindowResponse::Deny
                })
                .with_html("<!doctype html><html><body></body></html>")
                .build_as_child(&*window)
                .map_err(|e| format!("The preview couldn't start ({e})."))?;
            Ok(WrySurface { view })
        }
    }

    impl PreviewSurface for WrySurface {
        fn set_frame(&mut self, bounds: Bounds<Pixels>) {
            let _ = self.view.set_bounds(rect(bounds));
        }

        fn set_visible(&mut self, visible: bool) {
            // Hiding the first responder leaves the window itself as first
            // responder, and then typing reaches nobody (menu shortcuts like
            // paste still work). Hand the keyboard back first.
            if !visible && self.keyboard() != Keyboard::Elsewhere {
                let _ = self.view.focus_parent();
            }
            let _ = self.view.set_visible(visible);
        }

        fn reclaim_keyboard(&mut self) {
            if self.keyboard() != Keyboard::Elsewhere {
                let _ = self.view.focus_parent();
            }
        }

        fn load(&mut self, html: &str) {
            let _ = self.view.load_html(html);
        }

        fn eval(&mut self, js: &str) {
            let _ = self.view.evaluate_script(js);
        }

        fn focus_parent(&mut self) {
            let _ = self.view.focus_parent();
        }

        fn probe(&mut self) {
            // Who has the keyboard: it must not be the WebView.
            let focused = self.keyboard() == Keyboard::InPage;
            println!("preview-first-responder-is-webview={focused}");
            let _ = self
                .view
                .evaluate_script_with_callback(PROBE_JS, |json| println!("preview-probe {json}"));
        }

        fn set_dark(&mut self, dark: bool) {
            let wk = self.view.webview();
            let name = if dark {
                "NSAppearanceNameDarkAqua"
            } else {
                "NSAppearanceNameAqua"
            };
            // SAFETY: main thread (GPUI's foreground); `wk` is a live WKWebView,
            // and NSAppearance/appearanceNamed: is a plain AppKit class method.
            unsafe {
                use objc2::runtime::AnyObject;
                let ns_name = objc2_foundation_string(name);
                let cls = objc2::class!(NSAppearance);
                let appearance: *mut AnyObject = objc2::msg_send![cls, appearanceNamed: ns_name];
                let _: () = objc2::msg_send![&*wk, setAppearance: appearance];
            }
        }
    }

    /// An autoreleased NSString (no objc2-foundation dependency needed).
    unsafe fn objc2_foundation_string(s: &str) -> *mut objc2::runtime::AnyObject {
        let c = std::ffi::CString::new(s).unwrap_or_default();
        // SAFETY: stringWithUTF8String: copies the bytes; `c` outlives the call.
        unsafe { objc2::msg_send![objc2::class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }
}

/// The Windows surface: a WebView2 child window over GPUI's HWND.
///
/// GPUI normally draws through a topmost DirectComposition visual, which
/// would cover any child window; `main` turns that off on Windows
/// (`GPUI_DISABLE_DIRECT_COMPOSITION`) so this view shows above the app.
#[cfg(all(target_os = "windows", not(test)))]
mod wry_surface_windows {
    use super::*;
    use raw_window_handle::{
        HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
    };
    use std::cell::{Cell, RefCell};
    use std::num::NonZeroIsize;
    use std::rc::{Rc, Weak};
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::{NewWindowResponse, Rect, Theme, WebView, WebViewBuilder, WebViewExtWindows};

    // Creating a WebView2 waits for it in a nested message loop
    // (webview2-com's `wait_with_pump`), and other WebView2 and Win32 calls
    // (focus, show/hide, navigation) can send messages to GPUI's window
    // synchronously. GPUI makes its calls on the surface while it draws a
    // frame or runs an update, with the app borrowed, and a GPUI callback
    // that such a message reaches then borrows the app again and panics,
    // which aborts the process. So `DeferredSurface` touches no WebView2 or
    // Win32 API itself: it records what it is asked ([`Ops`]), and a Win32
    // thread timer, which GPUI's top-level message loop dispatches while
    // nothing is borrowed, builds the WebView and applies them.

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetTimer(
            hwnd: isize,
            id: usize,
            elapse_ms: u32,
            timer_proc: Option<unsafe extern "system" fn(isize, u32, usize, u32)>,
        ) -> usize;
        fn KillTimer(hwnd: isize, id: usize) -> i32;
    }

    type Job = Box<dyn FnOnce()>;

    thread_local! {
        static JOBS: RefCell<Vec<Job>> = const { RefCell::new(Vec::new()) };
        static TIMER: Cell<usize> = const { Cell::new(0) };
    }

    unsafe extern "system" fn run_jobs(_: isize, _: u32, id: usize, _: u32) {
        // SAFETY: a thread timer this module set; killing it is always valid.
        unsafe { KillTimer(0, id) };
        TIMER.with(|t| t.set(0));
        let jobs = JOBS.with(|j| std::mem::take(&mut *j.borrow_mut()));
        for job in jobs {
            job();
        }
    }

    /// Run `job` soon, from the top-level message loop.
    fn defer(job: Job) {
        JOBS.with(|j| j.borrow_mut().push(job));
        if TIMER.with(Cell::get) == 0 {
            // SAFETY: a plain thread timer with a static callback.
            let id = unsafe { SetTimer(0, 0, 1, Some(run_jobs)) };
            TIMER.with(|t| t.set(id));
        }
    }

    /// GPUI's window, by its HWND, for building the child WebView later.
    struct Hwnd(NonZeroIsize);

    impl HasWindowHandle for Hwnd {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.0));
            // SAFETY: GPUI's window HWND. If the window has closed since, the
            // build fails, and that is handled.
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    #[derive(Default)]
    struct Shared {
        /// `None` while it is being built, or while a flush is using it.
        view: Option<WrySurface>,
        /// Asked for and not yet applied.
        ops: Ops,
        /// It couldn't be built: drop whatever is asked.
        failed: bool,
    }

    type SharedRef = Rc<RefCell<Shared>>;

    /// Apply what is pending, outside GPUI's frame. The view is taken out
    /// while WebView2 runs, so a nested message loop inside one of its
    /// calls that reaches here again finds nothing to do; anything asked
    /// meanwhile is flushed after.
    fn flush(weak: &Weak<RefCell<Shared>>) {
        let Some(shared) = weak.upgrade() else {
            return;
        };
        let (mut view, ops) = {
            let Ok(mut s) = shared.try_borrow_mut() else {
                return;
            };
            let Some(view) = s.view.take() else {
                return;
            };
            (view, std::mem::take(&mut s.ops))
        };
        ops.apply(&mut view);
        let again = {
            let mut s = shared.borrow_mut();
            s.view = Some(view);
            !s.ops.is_empty()
        };
        if again {
            schedule(weak.clone());
        }
    }

    fn schedule(weak: Weak<RefCell<Shared>>) {
        defer(Box::new(move || flush(&weak)));
    }

    /// A WebView2 surface that GPUI can drive from inside a frame (see above).
    pub struct DeferredSurface(SharedRef);

    impl DeferredSurface {
        pub fn new(window: &mut Window, tx: Sender<SurfaceEvent>) -> Result<Self, String> {
            let hwnd = match HasWindowHandle::window_handle(&*window).map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => Hwnd(h.hwnd),
                _ => return Err("The preview couldn't find its window.".into()),
            };
            let shared = SharedRef::default();
            let weak = Rc::downgrade(&shared);
            defer(Box::new(move || {
                if weak.upgrade().is_none() {
                    return; // the pane went away first
                }
                let built = WrySurface::new(&hwnd, tx);
                let Some(shared) = weak.upgrade() else {
                    return;
                };
                match built {
                    Ok(view) => {
                        shared.borrow_mut().view = Some(view);
                        flush(&weak);
                    }
                    Err(msg) => {
                        eprintln!("blygger: {msg}");
                        let mut s = shared.borrow_mut();
                        s.failed = true;
                        s.ops = Ops::default();
                    }
                }
            }));
            Ok(DeferredSurface(shared))
        }

        /// Record a change; ask for a flush when none is due. One is due
        /// already when changes are pending, or the view is out being built
        /// or used (whoever has it flushes what is pending when done).
        fn ask(&mut self, f: impl FnOnce(&mut Ops)) {
            let mut s = self.0.borrow_mut();
            if s.failed {
                return;
            }
            let idle = s.ops.is_empty() && s.view.is_some();
            f(&mut s.ops);
            drop(s);
            if idle {
                schedule(Rc::downgrade(&self.0));
            }
        }
    }

    impl Drop for DeferredSurface {
        fn drop(&mut self) {
            // Closing a WebView2 is a WebView2 call too.
            if let Ok(mut s) = self.0.try_borrow_mut()
                && let Some(view) = s.view.take()
            {
                defer(Box::new(move || drop(view)));
            }
        }
    }

    impl PreviewSurface for DeferredSurface {
        fn set_frame(&mut self, bounds: Bounds<Pixels>) {
            self.ask(|o| o.set_frame(bounds));
        }

        fn set_visible(&mut self, visible: bool) {
            self.ask(|o| o.set_visible(visible));
        }

        fn reclaim_keyboard(&mut self) {
            // On Windows this is `focus_parent` (see `WrySurface`).
            self.ask(Ops::focus_parent);
        }

        fn load(&mut self, html: &str) {
            self.ask(|o| o.load(html));
        }

        fn eval(&mut self, js: &str) {
            self.ask(|o| o.eval(js));
        }

        fn focus_parent(&mut self) {
            self.ask(Ops::focus_parent);
        }

        fn probe(&mut self) {
            self.ask(Ops::probe);
        }

        fn set_dark(&mut self, dark: bool) {
            self.ask(|o| o.set_dark(dark));
        }
    }

    pub struct WrySurface {
        view: WebView,
        /// Set by `load`: the next top-level navigation is our own
        /// `NavigateToString`, whatever URI WebView2 reports for it.
        own_load: Rc<Cell<bool>>,
    }

    fn rect(b: Bounds<Pixels>) -> Rect {
        Rect {
            position: LogicalPosition::new(f64::from(b.origin.x), f64::from(b.origin.y)).into(),
            size: LogicalSize::new(
                f64::from(b.size.width).max(1.0),
                f64::from(b.size.height).max(1.0),
            )
            .into(),
        }
    }

    impl WrySurface {
        fn new(parent: &impl HasWindowHandle, tx: Sender<SurfaceEvent>) -> Result<Self, String> {
            let (ipc_tx, nav_tx, new_tx) = (tx.clone(), tx.clone(), tx);
            let own_load = Rc::new(Cell::new(true));
            let nav_own = own_load.clone();
            let view = WebViewBuilder::new()
                .with_bounds(rect(Bounds::default()))
                .with_visible(false)
                .with_focused(false)
                .with_initialization_script_for_main_only(HOST_SCRIPT, true)
                .with_ipc_handler(move |req| {
                    if let Some(ev) = parse_ipc(req.body()) {
                        let _ = ipc_tx.try_send(ev);
                    }
                })
                .with_navigation_handler(move |url| {
                    if nav_own.replace(false) {
                        return true;
                    }
                    match navigation(&url) {
                        Nav::Allow => true,
                        Nav::OpenExternally(u) => {
                            let _ = nav_tx.try_send(SurfaceEvent::OpenUrl(u));
                            false
                        }
                        Nav::Deny => false,
                    }
                })
                .with_new_window_req_handler(move |url, _| {
                    if let Nav::OpenExternally(u) = navigation(&url) {
                        let _ = new_tx.try_send(SurfaceEvent::OpenUrl(u));
                    }
                    NewWindowResponse::Deny
                })
                .with_html("<!doctype html><html><body></body></html>")
                .build_as_child(parent)
                .map_err(|e| {
                    format!(
                        "The preview couldn't start ({e}). It needs the Microsoft Edge \
                         WebView2 Runtime, which Windows 11 includes."
                    )
                })?;
            Ok(WrySurface { view, own_load })
        }
    }

    impl PreviewSurface for WrySurface {
        fn set_frame(&mut self, bounds: Bounds<Pixels>) {
            let _ = self.view.set_bounds(rect(bounds));
        }

        fn set_visible(&mut self, visible: bool) {
            // A hidden WebView2 can keep keyboard focus; hand it back first.
            if !visible {
                let _ = self.view.focus_parent();
            }
            let _ = self.view.set_visible(visible);
        }

        fn reclaim_keyboard(&mut self) {
            // WebView2 has no cheap "who has focus" query through wry, and
            // giving focus to GPUI's window when it already has it is harmless.
            let _ = self.view.focus_parent();
        }

        fn load(&mut self, html: &str) {
            self.own_load.set(true);
            if self.view.load_html(html).is_err() {
                self.own_load.set(false);
            }
        }

        fn eval(&mut self, js: &str) {
            let _ = self.view.evaluate_script(js);
        }

        fn focus_parent(&mut self) {
            let _ = self.view.focus_parent();
        }

        fn probe(&mut self) {
            let _ = self
                .view
                .evaluate_script_with_callback(PROBE_JS, |json| println!("preview-probe {json}"));
        }

        fn set_dark(&mut self, dark: bool) {
            let _ = self
                .view
                .set_theme(if dark { Theme::Dark } else { Theme::Light });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_messages() {
        assert_eq!(parse_ipc("ready"), Some(SurfaceEvent::Ready));
        assert_eq!(parse_ipc("focus"), Some(SurfaceEvent::Refocus));
        assert_eq!(parse_ipc("line:12"), Some(SurfaceEvent::JumpToLine(12)));
        assert_eq!(parse_ipc("line:x"), None);
        assert_eq!(
            parse_ipc("origin:https://ada.example.net/"),
            Some(SurfaceEvent::OpenOrigin("https://ada.example.net/".into()))
        );
        assert_eq!(parse_ipc("origin:javascript:alert(1)"), None);
        assert_eq!(parse_ipc("<script>"), None);
    }

    #[test]
    fn navigation_never_happens_in_the_view() {
        assert_eq!(navigation("about:blank"), Nav::Allow);
        assert_eq!(
            navigation("https://www.youtube-nocookie.com/embed/Qa1b2C3d4E5?autoplay=1"),
            Nav::Allow
        );
        assert_eq!(
            navigation("https://blyg.example.com/f/abc/"),
            Nav::OpenExternally("https://blyg.example.com/f/abc/".into())
        );
        assert_eq!(navigation("javascript:alert(1)"), Nav::Deny);
        assert_eq!(navigation("file:///etc/passwd"), Nav::Deny);
        assert_eq!(navigation("data:text/html,hi"), Nav::Deny);
    }

    /// Records the calls a surface gets, in order.
    #[derive(Default)]
    struct Calls(Vec<String>);

    impl PreviewSurface for Calls {
        fn set_frame(&mut self, b: Bounds<Pixels>) {
            self.0.push(format!("frame {}", f32::from(b.size.width)));
        }
        fn set_visible(&mut self, v: bool) {
            self.0.push(format!("visible {v}"));
        }
        fn load(&mut self, html: &str) {
            self.0.push(format!("load {html}"));
        }
        fn eval(&mut self, js: &str) {
            self.0.push(format!("eval {js}"));
        }
        fn focus_parent(&mut self) {
            self.0.push("focus".into());
        }
        fn set_dark(&mut self, d: bool) {
            self.0.push(format!("dark {d}"));
        }
    }

    #[test]
    fn deferred_ops_coalesce_and_keep_their_order() {
        use gpui_kit::{point, px, size};
        let b = |w: f32| Bounds::new(point(px(0.), px(0.)), size(px(w), px(10.)));
        let mut o = Ops::default();
        assert!(o.is_empty());
        o.set_visible(true);
        o.eval("old();");
        o.set_frame(b(100.));
        o.load("<p>a</p>");
        o.set_frame(b(200.));
        o.eval("patch();");
        o.focus_parent();
        o.set_visible(false);
        o.set_dark(true);
        assert!(!o.is_empty());
        let mut calls = Calls::default();
        o.apply(&mut calls);
        assert_eq!(
            calls.0,
            [
                "frame 200",
                "dark true",
                "load <p>a</p>",
                "eval patch();",
                "focus",
                "visible false",
            ],
            "the last frame and visibility win; a new page drops older script"
        );
        let mut calls = Calls::default();
        Ops::default().apply(&mut calls);
        assert!(calls.0.is_empty());
    }

    #[test]
    fn scripts_are_escaped() {
        let js = patch_js("<p data-line=\"0\">a \"b\" \\ </script>\u{2028}</p>");
        assert!(!js.contains("</script>"));
        assert!(js.contains("\\u003c/script>"));
        assert!(js.contains("\\\"b\\\""));
        assert!(js.contains("\\u2028"));
        assert_eq!(scroll_js(3), "window.__blyg && window.__blyg.scrollTo(3);");
    }
}
