//! The native preview surface: a WKWebView (through `wry`) attached as a
//! child of GPUI's NSView and positioned over the preview pane.
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

/// The default factory: a real WKWebView, except in unit tests (no
/// surface at all, so no test ever needs a WebView).
pub fn default_factory() -> Factory {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        std::rc::Rc::new(|window, tx| {
            wry_surface::WrySurface::new(window, tx).map(|s| Box::new(s) as Box<dyn PreviewSurface>)
        })
    }
    #[cfg(any(not(target_os = "macos"), test))]
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
