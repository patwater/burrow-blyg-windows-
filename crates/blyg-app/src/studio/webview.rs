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
    // --- quote targets ---
    /// A quote's text was clicked: open the original post (`origin` from the
    /// quote box's footer, the quoted `version` when the markup has it).
    OpenQuote {
        origin: String,
        id: String,
        version: Option<u32>,
    },
    // --- reader folders ---
    /// Space in the Reader when the page was already scrolled to its end:
    /// go on to the next post to read.
    PageEnd,
    // --- selection ---
    /// The page's text selection became non-empty (`true`) or went away
    /// (`false`). A selection the page keeps through a focus change (see
    /// [`HOST_SCRIPT`]) doesn't count as going away.
    Selected(bool),
    /// The pill by the selection: "Quote in draft". (Its "Reply with this"
    /// went with studio 0.31: Reply quotes the whole post, and a passage is
    /// chosen in the stub editor.)
    QuoteSelection,
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
    /// --- notes --- Send the page's selected text on `reply` ("" when
    /// nothing is selected).
    fn selection(&mut self, reply: async_channel::Sender<String>) {
        let _ = reply.try_send(String::new());
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
    if msg == "end" {
        return Some(SurfaceEvent::PageEnd); // --- reader folders ---
    }
    // --- selection ---
    match msg {
        "sel:1" => return Some(SurfaceEvent::Selected(true)),
        "sel:0" => return Some(SurfaceEvent::Selected(false)),
        "act:quote" => return Some(SurfaceEvent::QuoteSelection),
        _ => {}
    }
    if let Some(o) = msg.strip_prefix("origin:") {
        let web = o.starts_with("https://") || o.starts_with("http://");
        return web.then(|| SurfaceEvent::OpenOrigin(o.to_string()));
    }
    // --- quote targets --- `quote:<origin>\u{1f}<id>\u{1f}<version>`
    if let Some(q) = msg.strip_prefix("quote:") {
        let mut parts = q.split('\u{1f}');
        let (origin, id, version) = (parts.next()?, parts.next()?, parts.next().unwrap_or(""));
        let web = origin.starts_with("https://") || origin.starts_with("http://");
        let id_ok =
            !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric());
        return (web && id_ok).then(|| SurfaceEvent::OpenQuote {
            origin: origin.to_string(),
            id: id.to_string(),
            version: version.parse().ok(),
        });
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

/// --- reader folders --- Space in the Reader: a page down, or `end` when
/// the page is already at its bottom.
pub const PAGE_DOWN_JS: &str = "window.__blyg && window.__blyg.page(1);";

/// --- selection --- Turn on the pill that follows a text selection
/// ("Quote in draft ⇧⌘D"). Off by default: the studio preview never shows it.
pub const SELECTION_UI_JS: &str = "window.__blyg && window.__blyg.selectionUi({});";

/// ↑/↓ with the post pane focused: scroll a little.
pub fn nudge_js(down: bool) -> String {
    format!(
        "window.__blyg && window.__blyg.nudge({});",
        if down { 1 } else { -1 }
    )
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
  // --- selection --- WebKit drops a page's selection as soon as the view
  // gives up the keyboard, and a click here hands it back to the app
  // ("focus", "line:"). So the last range is kept, and put back when a
  // collapse comes with a blur (in either order); a new press in the page
  // starts over. The pill by the selection (off unless `selectionUi`)
  // quotes it without leaving the page.
  var kept = null, lostAt = 0, blurAt = 0, pressing = false, hasSel = false;
  var ui = null, pill = null, timer = 0, pending = 0;
  function selected() {
    var s = window.getSelection();
    return s && s.rangeCount && !s.isCollapsed && String(s).trim() ? s : null;
  }
  function restore() {
    var k = kept;
    setTimeout(function () {
      var s = window.getSelection();
      if (k && s && (!s.rangeCount || s.isCollapsed)) { s.removeAllRanges(); s.addRange(k); }
    }, 0);
  }
  function schedule(ms) { clearTimeout(timer); timer = setTimeout(update, ms); }
  function update() {
    var s = selected(), on = !!s;
    if (on !== hasSel) { hasSel = on; post(on ? "sel:1" : "sel:0"); }
    if (on && ui && !pressing) showPill(s); else hidePill();
  }
  document.addEventListener("selectionchange", function () {
    if (selected()) { kept = window.getSelection().getRangeAt(0).cloneRange(); schedule(60); return; }
    if (!kept) { schedule(60); return; }
    lostAt = Date.now();
    if (lostAt - blurAt < 300) { restore(); return; }
    schedule(350); // a blur may still come
  });
  window.addEventListener("blur", function () {
    blurAt = Date.now();
    if (kept && blurAt - lostAt < 300) restore();
  });
  function inPill(t) { return !!(pill && t && pill.contains(t)); }
  document.addEventListener("mousedown", function (e) {
    if (inPill(e.target)) { e.preventDefault(); return; } // keep the selection
    if (e.button === 0) { pressing = true; kept = null; hidePill(); }
  }, true);
  document.addEventListener("mouseup", function () { pressing = false; schedule(60); }, true);
  function hidePill() { if (pill) pill.style.display = "none"; }
  function button(act, label, key, tip) {
    var b = document.createElement("button");
    b.setAttribute("data-act", act);
    b.setAttribute("data-label", label);
    if (key) b.setAttribute("data-key", key);
    b.title = tip;
    return b;
  }
  function showPill(s) {
    if (!pill) {
      var st = document.createElement("style");
      st.textContent = ".blyg-selpill{position:absolute;z-index:2147483647;display:none;gap:2px;padding:3px;" +
        "border-radius:8px;background:var(--paper,Canvas);border:1px solid var(--rule,#ccc);" +
        "box-shadow:0 4px 14px rgba(0,0,0,.18);font:12px/1 Inter,-apple-system,system-ui,sans-serif;" +
        "-webkit-user-select:none;user-select:none}" +
        ".blyg-selpill button{all:unset;cursor:pointer;padding:5px 8px;border-radius:5px;" +
        "color:var(--ink,CanvasText);white-space:nowrap}" +
        ".blyg-selpill button:hover{background:var(--wash,rgba(127,127,127,.15));color:var(--accent,#a4271b)}" +
        ".blyg-selpill button::after{content:attr(data-label)}" +
        ".blyg-selpill button[data-key]::before{content:attr(data-key);opacity:.55;margin-right:6px}";
      (document.head || document.documentElement).appendChild(st);
      pill = document.createElement("div");
      pill.className = "blyg-selpill";
      pill.setAttribute("role", "toolbar");
      document.body.appendChild(pill);
      pill.addEventListener("click", function (e) {
        var b = e.target.closest && e.target.closest("button[data-act]");
        e.stopPropagation();
        if (b) post("act:" + b.getAttribute("data-act"));
      });
    }
    // The labels are CSS content, so they never join a selection's text.
    if (!pill.firstChild) {
      pill.appendChild(button("quote", "Quote in draft", "⇧⌘D",
        "Quote this passage in your draft, with a link to the post"));
    }
    var range = s.getRangeAt(0), rects = range.getClientRects();
    var last = rects.length ? rects[rects.length - 1] : range.getBoundingClientRect();
    pill.style.display = "flex";
    var w = pill.offsetWidth, h = pill.offsetHeight;
    var top = last.bottom + 6;
    if (top + h > window.innerHeight - 4) top = (rects.length ? rects[0].top : last.top) - h - 6;
    var left = Math.min(Math.max(8, last.right - w / 2), window.innerWidth - w - 8);
    pill.style.top = (top + window.scrollY) + "px";
    pill.style.left = (left + window.scrollX) + "px";
  }
  function selectionUi(o) { ui = o || null; update(); }
  function go(m) {
    // Wait out a double-click: its first click mustn't leave the page
    // before the second one selects a word.
    clearTimeout(pending);
    pending = setTimeout(function () { if (!selected()) post(m); }, 260);
  }
  document.addEventListener("click", function (e) {
    var t = e.target;
    if (!t || !t.closest || inPill(t)) return;
    if (t.closest("a[href]") || t.closest(".blyg-yt")) return;
    // A drag or a double-click that selected text isn't a click on what's
    // under it: hand the keyboard back (the selection is kept) and stop.
    if (selected() || e.detail > 1) { clearTimeout(pending); post("focus"); return; }
    // --- quote targets --- a quote box: its footer's name opens the
    // profile, the rest opens the original post (the footer is the reader's).
    var qo = t.closest(".blyg-qorigin");
    if (qo) { go("origin:" + qo.getAttribute("data-blyg-origin")); return; }
    var q = t.closest("blockquote.blyg-transclusion[data-blyg-id]");
    var qf = q && q.querySelector(":scope > .blyg-qfoot .blyg-qorigin");
    if (q && qf) {
      go("quote:" + qf.getAttribute("data-blyg-origin") + "\u001f" + q.getAttribute("data-blyg-id") +
        "\u001f" + (q.getAttribute("data-blyg-version") || ""));
      return;
    }
    if (q && q.hasAttribute("data-blyg-origin")) { go("origin:" + q.getAttribute("data-blyg-origin")); return; }
    var b = t.closest(".item-content [data-line]");
    post(b ? "line:" + b.getAttribute("data-line") : "focus");
  }, true);
  // --- reader folders --- Space pages down; at the bottom it says "end".
  function scroller() { return document.scrollingElement || document.documentElement; }
  function page(d) {
    var el = scroller(), h = window.innerHeight;
    if (d > 0 && el.scrollTop + h >= el.scrollHeight - 4) { post("end"); return; }
    window.scrollBy({ top: d * Math.max(40, h * 0.85), behavior: "smooth" });
  }
  function nudge(d) { window.scrollBy({ top: d * 48 }); }
  document.addEventListener("DOMContentLoaded", function () { tag(); post("ready"); });
  window.__blyg = { patch: patch, scrollTo: scrollTo, page: page, nudge: nudge, selectionUi: selectionUi };
})();
"#;

#[cfg(all(target_os = "macos", not(test)))]
pub(crate) use wry_surface::{Keyboard, keyboard_of, rect, set_appearance};

// --- browser --- The modifier keys held when a link was followed (⌘ and ⌥),
// read when WebKit asks about the navigation, so the browser pane knows a
// ⌘-click from a click even though the event reaches it a moment later.
thread_local! {
    static CLICK_MODIFIERS: std::cell::Cell<Option<(std::time::Instant, bool, bool)>> =
        const { std::cell::Cell::new(None) };
}

/// (⌘, ⌥) for the link just followed in a reader or preview page, if one was
/// followed in the last second.
pub fn take_click_modifiers() -> Option<(bool, bool)> {
    CLICK_MODIFIERS
        .take()
        .filter(|(t, ..)| t.elapsed() < std::time::Duration::from_secs(1))
        .map(|(_, cmd, alt)| (cmd, alt))
}

#[cfg(all(target_os = "macos", not(test)))]
fn note_click_modifiers() {
    // SAFETY: `+[NSEvent modifierFlags]` is a plain class getter of the
    // keys held right now; main thread (WebKit's delegate callbacks).
    let flags: usize = unsafe { objc2::msg_send![objc2::class!(NSEvent), modifierFlags] };
    const COMMAND: usize = 1 << 20;
    const OPTION: usize = 1 << 19;
    CLICK_MODIFIERS.set(Some((
        std::time::Instant::now(),
        flags & COMMAND != 0,
        flags & OPTION != 0,
    )));
}

#[cfg(all(target_os = "macos", not(test)))]
mod wry_surface {
    use super::*;
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder, WebViewExtMacOS, WryWebView};

    pub struct WrySurface {
        view: WebView,
    }

    pub(crate) fn rect(b: Bounds<Pixels>) -> Rect {
        Rect {
            position: LogicalPosition::new(f64::from(b.origin.x), f64::from(b.origin.y)).into(),
            size: LogicalSize::new(
                f64::from(b.size.width).max(1.0),
                f64::from(b.size.height).max(1.0),
            )
            .into(),
        }
    }

    /// Where the window's keyboard is, as far as a WebView is concerned.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum Keyboard {
        /// The WebView (or something inside it) is first responder.
        InPage,
        /// No view has it: the window itself (or nothing) is first responder.
        Nowhere,
        /// Some other view, normally GPUI's.
        Elsewhere,
    }

    /// Who has the keyboard, relative to `wk` (the preview's, the reader's
    /// or the browser pane's WKWebView).
    pub(crate) fn keyboard_of(wk: &WryWebView) -> Keyboard {
        // SAFETY: main thread; plain AppKit getters on live objects.
        unsafe {
            use objc2::runtime::AnyObject;
            let win: *mut AnyObject = objc2::msg_send![wk, window];
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
            let mine: bool = objc2::msg_send![responder, isDescendantOf: wk];
            if mine {
                Keyboard::InPage
            } else {
                Keyboard::Elsewhere
            }
        }
    }

    /// Follow the app's light/dark choice (the page's `prefers-color-scheme`).
    pub(crate) fn set_appearance(wk: &WryWebView, dark: bool) {
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
            let _: () = objc2::msg_send![wk, setAppearance: appearance];
        }
    }

    impl WrySurface {
        fn keyboard(&self) -> Keyboard {
            keyboard_of(&self.view.webview())
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
                        note_click_modifiers();
                        let _ = nav_tx.try_send(SurfaceEvent::OpenUrl(u));
                        false
                    }
                    Nav::Deny => false,
                })
                .with_new_window_req_handler(move |url, _| {
                    if let Nav::OpenExternally(u) = navigation(&url) {
                        note_click_modifiers();
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
            set_appearance(&self.view.webview(), dark);
        }

        // --- notes ---
        fn selection(&mut self, reply: async_channel::Sender<String>) {
            let _ = self.view.evaluate_script_with_callback(
                crate::app::notes::SELECTION_JS,
                move |json| {
                    let _ = reply.try_send(crate::app::notes::parse_selection(&json));
                },
            );
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
    // (webview2-com's `wait_with_pump`). GPUI asks for the surface while it
    // draws a frame, with the app borrowed, and a GPUI task or event that the
    // nested loop dispatches then borrows the app again and panics, which
    // aborts the process. So the WebView is built later, from a Win32 thread
    // timer that GPUI's top-level message loop dispatches while nothing is
    // borrowed; until then `DeferredSurface` records what it is asked to do.

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

    /// What the surface was asked to do before its WebView existed.
    #[derive(Default)]
    struct Pending {
        view: Option<WrySurface>,
        frame: Option<Bounds<Pixels>>,
        visible: bool,
        dark: Option<bool>,
        html: Option<String>,
        evals: Vec<String>,
    }

    /// A WebView2 surface built outside GPUI's frame (see above).
    pub struct DeferredSurface(Rc<RefCell<Pending>>);

    impl DeferredSurface {
        pub fn new(window: &mut Window, tx: Sender<SurfaceEvent>) -> Result<Self, String> {
            let hwnd = match HasWindowHandle::window_handle(&*window).map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => Hwnd(h.hwnd),
                _ => return Err("The preview couldn't find its window.".into()),
            };
            let pending = Rc::new(RefCell::new(Pending::default()));
            let weak: Weak<RefCell<Pending>> = Rc::downgrade(&pending);
            defer(Box::new(move || {
                if weak.upgrade().is_none() {
                    return; // the pane went away first
                }
                let built = WrySurface::new(&hwnd, tx);
                let Some(pending) = weak.upgrade() else {
                    return;
                };
                match built {
                    Ok(mut view) => {
                        let mut p = pending.borrow_mut();
                        if let Some(b) = p.frame {
                            view.set_frame(b);
                        }
                        if let Some(d) = p.dark {
                            view.set_dark(d);
                        }
                        if let Some(h) = p.html.take() {
                            view.load(&h);
                        }
                        for js in std::mem::take(&mut p.evals) {
                            view.eval(&js);
                        }
                        view.set_visible(p.visible);
                        p.view = Some(view);
                    }
                    Err(msg) => eprintln!("blygger: {msg}"),
                }
            }));
            Ok(DeferredSurface(pending))
        }
    }

    impl PreviewSurface for DeferredSurface {
        fn set_frame(&mut self, bounds: Bounds<Pixels>) {
            let mut p = self.0.borrow_mut();
            match p.view.as_mut() {
                Some(v) => v.set_frame(bounds),
                None => p.frame = Some(bounds),
            }
        }

        fn set_visible(&mut self, visible: bool) {
            let mut p = self.0.borrow_mut();
            match p.view.as_mut() {
                Some(v) => v.set_visible(visible),
                None => p.visible = visible,
            }
        }

        fn reclaim_keyboard(&mut self) {
            if let Some(v) = self.0.borrow_mut().view.as_mut() {
                v.reclaim_keyboard();
            }
        }

        fn load(&mut self, html: &str) {
            let mut p = self.0.borrow_mut();
            match p.view.as_mut() {
                Some(v) => v.load(html),
                None => {
                    // A new page makes script queued for the old one moot.
                    p.html = Some(html.to_string());
                    p.evals.clear();
                }
            }
        }

        fn eval(&mut self, js: &str) {
            let mut p = self.0.borrow_mut();
            match p.view.as_mut() {
                Some(v) => v.eval(js),
                None => p.evals.push(js.to_string()),
            }
        }

        fn focus_parent(&mut self) {
            if let Some(v) = self.0.borrow_mut().view.as_mut() {
                v.focus_parent();
            }
        }

        fn probe(&mut self) {
            if let Some(v) = self.0.borrow_mut().view.as_mut() {
                v.probe();
            }
        }

        fn set_dark(&mut self, dark: bool) {
            let mut p = self.0.borrow_mut();
            match p.view.as_mut() {
                Some(v) => v.set_dark(dark),
                None => p.dark = Some(dark),
            }
        }

        // --- notes --- (nothing is selected before the page exists)
        fn selection(&mut self, reply: async_channel::Sender<String>) {
            match self.0.borrow_mut().view.as_mut() {
                Some(v) => v.selection(reply),
                None => {
                    let _ = reply.try_send(String::new());
                }
            }
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

        // --- notes ---
        fn selection(&mut self, reply: async_channel::Sender<String>) {
            let fallback = reply.clone();
            let sent = self.view.evaluate_script_with_callback(
                crate::app::notes::SELECTION_JS,
                move |json| {
                    let _ = reply.try_send(crate::app::notes::parse_selection(&json));
                },
            );
            if sent.is_err() {
                let _ = fallback.try_send(String::new());
            }
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
        assert_eq!(parse_ipc("end"), Some(SurfaceEvent::PageEnd));
        assert_eq!(parse_ipc("line:12"), Some(SurfaceEvent::JumpToLine(12)));
        assert_eq!(parse_ipc("line:x"), None);
        assert_eq!(
            parse_ipc("origin:https://ada.example.net/"),
            Some(SurfaceEvent::OpenOrigin("https://ada.example.net/".into()))
        );
        assert_eq!(parse_ipc("origin:javascript:alert(1)"), None);
        assert_eq!(
            parse_ipc("quote:https://ada.example.net/\u{1f}01K2ADA0TIDES0000000000001\u{1f}2"),
            Some(SurfaceEvent::OpenQuote {
                origin: "https://ada.example.net/".into(),
                id: "01K2ADA0TIDES0000000000001".into(),
                version: Some(2),
            })
        );
        assert_eq!(
            parse_ipc("quote:https://ada.example.net/\u{1f}01K2ADA0TIDES0000000000001\u{1f}"),
            Some(SurfaceEvent::OpenQuote {
                origin: "https://ada.example.net/".into(),
                id: "01K2ADA0TIDES0000000000001".into(),
                version: None,
            })
        );
        assert_eq!(parse_ipc("quote:javascript:x\u{1f}01K2\u{1f}1"), None);
        assert_eq!(
            parse_ipc("quote:https://a.example/\u{1f}../../x\u{1f}1"),
            None
        );
        assert_eq!(parse_ipc("<script>"), None);
        // --- selection ---
        assert_eq!(parse_ipc("sel:1"), Some(SurfaceEvent::Selected(true)));
        assert_eq!(parse_ipc("sel:0"), Some(SurfaceEvent::Selected(false)));
        assert_eq!(parse_ipc("act:quote"), Some(SurfaceEvent::QuoteSelection));
        assert_eq!(parse_ipc("act:reply"), None, "the pill has no reply (0.31)");
        assert_eq!(parse_ipc("act:publish"), None);
    }

    /// The host script keeps a selection through a focus change, doesn't
    /// treat the click that ends a drag as a click on what's under it, and
    /// has the pill (off until the host turns it on).
    #[test]
    fn the_host_script_keeps_selections() {
        for part in [
            "selectionchange",
            "s.addRange(k)",
            "window.addEventListener(\"blur\"",
            "if (selected() || e.detail > 1) { clearTimeout(pending); post(\"focus\"); return; }",
            "post(\"act:\" + b.getAttribute(\"data-act\"))",
            "selectionUi: selectionUi",
            "content:attr(data-label)", // labels never join the selection's text
        ] {
            assert!(HOST_SCRIPT.contains(part), "{part}");
        }
        assert!(!HOST_SCRIPT.contains("Reply with this"));
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
