//! The browser pane's web view: its own WKWebView (through `wry`), kept
//! apart from the reader and preview web views.
//!
//! Security (it shows arbitrary web pages, unlike the sanitized reader):
//! - no IPC handler and no host script, so the page has no bridge to the
//!   app at all (`window.ipc` doesn't exist), and nothing the app knows (the
//!   owner token, keys) is ever handed to it;
//! - its own website data store: persistent under a fixed identifier on
//!   macOS 14+ (logins survive a relaunch, separately from the reader and
//!   preview, which use the default store), non-persistent before that;
//! - only `http(s)` and `about:blank` navigations (`super::nav_policy`);
//!   `mailto:` goes to the system, everything else (`file:`, `data:`,
//!   `javascript:`, custom schemes) is refused;
//! - downloads are cancelled and handed to the default browser; new-window
//!   requests navigate this view instead.
//!
//! Content blocking: the compiled rule lists are attached to the view's user
//! content controller, and detached for a host whose shield is off. That's
//! decided when a *main-frame* navigation is about to start, by a small
//! subclass of wry's navigation delegate (`nav_hook`), so the lists are in
//! place before the page's first request.

use async_channel::Sender;
use gpui_kit::{Bounds, Pixels, Window};

/// What the view reports to the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
pub enum BrowserEvent {
    /// A load started, committed, finished, or the title changed: read
    /// [`PageState`] again.
    Changed,
    /// A link the pane can't show (`mailto:`), or a download: the system
    /// handles it.
    OpenExternally(String),
    /// `target=_blank` / `window.open`: navigate this view.
    NewWindow(String),
    /// The WebContent process died (memory pressure, a crash).
    Crashed,
}

/// What the page is doing, read on demand.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageState {
    pub url: String,
    pub title: String,
    pub loading: bool,
    /// 0.0 ..= 1.0
    pub progress: f64,
    pub can_back: bool,
    pub can_forward: bool,
}

/// Whether blocking applies to a main-frame URL (shared with the view's
/// navigation hook).
pub type BlockingFor = std::rc::Rc<dyn Fn(&str) -> bool>;

/// A place to show web pages. `wry` implements it for real; tests use a stub.
pub trait BrowserSurface {
    fn set_frame(&mut self, bounds: Bounds<Pixels>);
    fn set_visible(&mut self, visible: bool);
    fn load_url(&mut self, url: &str);
    fn back(&mut self);
    fn forward(&mut self);
    fn reload(&mut self);
    fn stop(&mut self);
    fn state(&self) -> PageState;
    /// The compiled lists changed (or blocking was switched): re-attach for
    /// the current page (takes effect from the next load).
    fn refresh_blocking(&mut self);
    /// Hand the keyboard back to GPUI's view.
    fn focus_parent(&mut self);
    /// The page has the keyboard (a text field in it is focused, say).
    fn page_has_keyboard(&self) -> bool {
        false
    }
    /// The keyboard fell to the window itself (nobody gets typing): give it
    /// back to GPUI. Never takes it from the page.
    fn reclaim_lost_keyboard(&mut self) {}
    /// Send an edit command (`copy:`, `paste:`, …) to the page, which the
    /// app's Edit menu would otherwise send to GPUI.
    fn edit_command(&mut self, _selector: &str) {}
    fn set_dark(&mut self, _dark: bool) {}
    /// Debug builds: run `js` in the page and print its result (smoke tests).
    fn probe(&mut self, _js: &str, _label: &str) {}
    /// Debug builds: the page's WebContent process (smoke tests measure it).
    fn web_process_id(&self) -> Option<i32> {
        None
    }
    /// --- notes --- Send the page's selected text on `reply` ("" when
    /// nothing is selected).
    fn selection(&mut self, reply: Sender<String>) {
        let _ = reply.try_send(String::new());
    }
}

/// Makes the browser's surface. Returns a sentence for the pane when it can't.
pub type Factory = std::rc::Rc<
    dyn Fn(
        &mut Window,
        Sender<BrowserEvent>,
        super::rules_handle::Handle,
        BlockingFor,
    ) -> Result<Box<dyn BrowserSurface>, String>,
>;

/// Overrides the factory (tests install a stub).
pub struct FactoryGlobal(pub Factory);
impl gpui_kit::Global for FactoryGlobal {}

pub fn default_factory() -> Factory {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        std::rc::Rc::new(|window, tx, rules, blocking| {
            wry_browser::WryBrowser::new(window, tx, rules, blocking)
                .map(|s| Box::new(s) as Box<dyn BrowserSurface>)
        })
    }
    #[cfg(any(not(target_os = "macos"), test))]
    {
        std::rc::Rc::new(|_, _, _, _| Err("The browser isn't available here.".to_string()))
    }
}

/// A fixed identifier for the browser's own persistent website data store
/// (cookies, local storage): "blygger-browser" padded, as a UUID.
#[cfg_attr(test, allow(dead_code))]
pub const DATA_STORE_ID: [u8; 16] = *b"blygger-browser\0";

#[cfg(all(target_os = "macos", not(test)))]
mod wry_browser {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;
    use crate::app::studio::webview::{Keyboard, keyboard_of, rect};
    use objc2_web_kit::WKUserContentController;
    use wry::{
        NewWindowResponse, PageLoadEvent, WebView, WebViewBuilder, WebViewBuilderExtDarwin,
        WebViewExtMacOS,
    };

    pub struct WryBrowser {
        view: WebView,
        rules: super::super::rules_handle::Handle,
        blocking: BlockingFor,
        /// Lists are attached right now (and which generation of them).
        attached: Rc<Cell<Option<u64>>>,
    }

    fn macos_major() -> isize {
        objc2_foundation::NSProcessInfo::processInfo()
            .operatingSystemVersion()
            .majorVersion
    }

    /// Attach the rule lists (or none) to `manager`, unless that's what's
    /// attached already.
    fn apply(
        manager: &WKUserContentController,
        rules: &super::super::rules_handle::Handle,
        attached: &Cell<Option<u64>>,
        on: bool,
    ) {
        let r = rules.borrow();
        let want = on.then_some(r.generation);
        if attached.get() == want {
            return;
        }
        // SAFETY: main thread; the lists and the controller are live.
        unsafe {
            manager.removeAllContentRuleLists();
            if on {
                for l in &r.lists {
                    manager.addContentRuleList(l);
                }
            }
        }
        attached.set(want);
    }

    impl WryBrowser {
        pub fn new(
            window: &mut Window,
            tx: Sender<BrowserEvent>,
            rules: super::super::rules_handle::Handle,
            blocking: BlockingFor,
        ) -> Result<Self, String> {
            let (nav_tx, new_tx, load_tx, title_tx, dl_tx, dead_tx) = (
                tx.clone(),
                tx.clone(),
                tx.clone(),
                tx.clone(),
                tx.clone(),
                tx,
            );
            let mut b = WebViewBuilder::new()
                .with_bounds(rect(Bounds::default()))
                .with_visible(false)
                .with_focused(false)
                .with_accept_first_mouse(true)
                .with_devtools(cfg!(debug_assertions))
                .with_back_forward_navigation_gestures(true)
                .with_navigation_handler(move |url| match super::super::nav_policy(&url) {
                    super::super::Nav::Allow => true,
                    super::super::Nav::System(u) => {
                        let _ = nav_tx.try_send(BrowserEvent::OpenExternally(u));
                        false
                    }
                    super::super::Nav::Deny => false,
                })
                .with_new_window_req_handler(move |url, _| {
                    match super::super::nav_policy(&url) {
                        super::super::Nav::Allow if url != "about:blank" => {
                            let _ = new_tx.try_send(BrowserEvent::NewWindow(url));
                        }
                        super::super::Nav::System(u) => {
                            let _ = new_tx.try_send(BrowserEvent::OpenExternally(u));
                        }
                        _ => {}
                    }
                    NewWindowResponse::Deny
                })
                .with_download_started_handler(move |url, _path| {
                    if url.starts_with("https://") || url.starts_with("http://") {
                        let _ = dl_tx.try_send(BrowserEvent::OpenExternally(url));
                    }
                    false
                })
                .with_on_page_load_handler(move |_ev: PageLoadEvent, _url| {
                    let _ = load_tx.try_send(BrowserEvent::Changed);
                })
                .with_document_title_changed_handler(move |_| {
                    let _ = title_tx.try_send(BrowserEvent::Changed);
                })
                .with_on_web_content_process_terminate_handler(move || {
                    let _ = dead_tx.try_send(BrowserEvent::Crashed);
                })
                .with_url("about:blank");
            // Its own cookie jar: persistent on macOS 14+, else in memory.
            b = if macos_major() >= 14 {
                b.with_data_store_identifier(DATA_STORE_ID)
            } else {
                b.with_incognito(true)
            };
            let view = b
                .build_as_child(&*window)
                .map_err(|e| format!("The browser couldn't start ({e})."))?;
            let attached = Rc::new(Cell::new(None));
            let this = WryBrowser {
                view,
                rules,
                blocking,
                attached,
            };
            this.install_hook();
            Ok(this)
        }

        fn install_hook(&self) {
            let wk = self.view.webview();
            // The view's navigation delegate is wry's; give that one object a
            // subclass that asks us first about main-frame navigations.
            // SAFETY: main thread; `wk` is live, and its delegate is wry's
            // WryNavigationDelegate (retained by wry for the view's lifetime).
            let delegate: *mut objc2::runtime::AnyObject =
                unsafe { objc2::msg_send![&*wk, navigationDelegate] };
            let Some(delegate) = (unsafe { delegate.as_ref() }) else {
                return;
            };
            let view_ptr = objc2::rc::Retained::as_ptr(&wk) as usize;
            // The hook needs the WebView to swap lists; it's only ever called
            // while the view is alive (WebKit calls its own delegate).
            let rules = self.rules.clone();
            let blocking = self.blocking.clone();
            let attached = self.attached.clone();
            let manager = self.view.manager();
            nav_hook::install(
                delegate,
                view_ptr,
                Box::new(move |url: &str| apply(&manager, &rules, &attached, blocking(url))),
            );
        }

        fn wk(&self) -> objc2::rc::Retained<wry::WryWebView> {
            self.view.webview()
        }
    }

    impl Drop for WryBrowser {
        fn drop(&mut self) {
            let wk = self.view.webview();
            nav_hook::uninstall(objc2::rc::Retained::as_ptr(&wk) as usize);
            if keyboard_of(&wk) != Keyboard::Elsewhere {
                let _ = self.view.focus_parent();
            }
        }
    }

    impl BrowserSurface for WryBrowser {
        fn set_frame(&mut self, bounds: Bounds<Pixels>) {
            let _ = self.view.set_bounds(rect(bounds));
        }

        fn set_visible(&mut self, visible: bool) {
            // As for the preview: never hide the first responder (typing
            // would then reach nobody).
            if !visible && keyboard_of(&self.wk()) != Keyboard::Elsewhere {
                let _ = self.view.focus_parent();
            }
            let _ = self.view.set_visible(visible);
        }

        fn load_url(&mut self, url: &str) {
            // The first page: attach before it loads (the hook covers every
            // navigation after this one too).
            let on = (self.blocking)(url);
            apply(&self.view.manager(), &self.rules, &self.attached, on);
            let _ = self.view.load_url(url);
        }

        fn back(&mut self) {
            let _ = self.view.go_back();
        }

        fn forward(&mut self) {
            let _ = self.view.go_forward();
        }

        fn reload(&mut self) {
            let url = self.state().url;
            let on = (self.blocking)(&url);
            apply(&self.view.manager(), &self.rules, &self.attached, on);
            let _ = self.view.reload();
        }

        fn stop(&mut self) {
            // SAFETY: main thread; live view.
            unsafe { self.wk().stopLoading() };
        }

        fn state(&self) -> PageState {
            let wk = self.wk();
            // SAFETY: main thread; plain WKWebView getters.
            unsafe {
                PageState {
                    url: wk
                        .URL()
                        .and_then(|u| u.absoluteString())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    title: wk.title().map(|s| s.to_string()).unwrap_or_default(),
                    loading: wk.isLoading(),
                    progress: wk.estimatedProgress(),
                    can_back: wk.canGoBack(),
                    can_forward: wk.canGoForward(),
                }
            }
        }

        fn refresh_blocking(&mut self) {
            let url = self.state().url;
            let on = (self.blocking)(&url);
            apply(&self.view.manager(), &self.rules, &self.attached, on);
        }

        fn focus_parent(&mut self) {
            let _ = self.view.focus_parent();
        }

        fn page_has_keyboard(&self) -> bool {
            keyboard_of(&self.wk()) == Keyboard::InPage
        }

        fn reclaim_lost_keyboard(&mut self) {
            if keyboard_of(&self.wk()) == Keyboard::Nowhere {
                let _ = self.view.focus_parent();
            }
        }

        fn edit_command(&mut self, selector: &str) {
            let sel = objc2::runtime::Sel::register(
                &std::ffi::CString::new(selector).unwrap_or_default(),
            );
            let wk = self.wk();
            // SAFETY: main thread; `tryToPerform:with:` walks the responder
            // chain from the web view's first responder like the Edit menu.
            unsafe {
                let win: *mut objc2::runtime::AnyObject = objc2::msg_send![&*wk, window];
                if let Some(win) = win.as_ref() {
                    let responder: *mut objc2::runtime::AnyObject =
                        objc2::msg_send![win, firstResponder];
                    if let Some(r) = responder.as_ref() {
                        let nil: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
                        let _: bool = objc2::msg_send![r, tryToPerform: sel, with: nil];
                    }
                }
            }
        }

        fn set_dark(&mut self, dark: bool) {
            crate::app::studio::webview::set_appearance(&self.wk(), dark);
        }

        fn web_process_id(&self) -> Option<i32> {
            if !cfg!(debug_assertions) {
                return None;
            }
            let wk = self.wk();
            // SAFETY: main thread; `_webProcessIdentifier` is WebKit SPI
            // (a pid_t getter), used only for debug measurements.
            unsafe {
                let ok: bool = objc2::msg_send![
                    &*wk,
                    respondsToSelector: objc2::sel!(_webProcessIdentifier)
                ];
                if !ok {
                    return None;
                }
                let pid: i32 = objc2::msg_send![&*wk, _webProcessIdentifier];
                (pid > 0).then_some(pid)
            }
        }

        fn probe(&mut self, js: &str, label: &str) {
            let label = label.to_string();
            let _ = self
                .view
                .evaluate_script_with_callback(js, move |json| println!("{label} {json}"));
        }

        // --- notes ---
        fn selection(&mut self, reply: Sender<String>) {
            let _ = self.view.evaluate_script_with_callback(
                crate::app::notes::SELECTION_JS,
                move |json| {
                    let _ = reply.try_send(crate::app::notes::parse_selection(&json));
                },
            );
        }
    }

    /// A per-object subclass of wry's navigation delegate (isa-swizzled, as
    /// KVO does) that calls a hook for main-frame navigations before wry's
    /// own policy runs. The subclass adds no ivars, so wry's state is
    /// untouched; the reader's and preview's delegates keep wry's class.
    mod nav_hook {
        use std::cell::RefCell;
        use std::collections::HashMap;
        use std::sync::OnceLock;

        use block2::DynBlock;
        use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
        use objc2::{msg_send, sel};
        use objc2_web_kit::{WKNavigationAction, WKNavigationActionPolicy, WKWebView};

        type Hook = Box<dyn Fn(&str)>;

        thread_local! {
            static HOOKS: RefCell<HashMap<usize, Hook>> = RefCell::new(HashMap::new());
        }

        /// (subclass, wry's class), made once.
        static CLASSES: OnceLock<(usize, usize)> = OnceLock::new();

        extern "C-unwind" fn decide(
            this: &AnyObject,
            _sel: Sel,
            webview: &WKWebView,
            action: &WKNavigationAction,
            handler: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: main thread (WebKit calls its delegate there); plain
            // getters on live objects.
            unsafe {
                let main = action.targetFrame().is_some_and(|f| f.isMainFrame());
                if main {
                    let url = action
                        .request()
                        .URL()
                        .and_then(|u| u.absoluteString())
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let key = webview as *const WKWebView as usize;
                    HOOKS.with(|h| {
                        if let Some(f) = h.borrow().get(&key) {
                            f(&url);
                        }
                    });
                }
                let Some(&(_, base)) = CLASSES.get() else {
                    return;
                };
                let base = &*(base as *const AnyClass);
                let _: () = msg_send![
                    super(this, base),
                    webView: webview,
                    decidePolicyForNavigationAction: action,
                    decisionHandler: handler
                ];
            }
        }

        pub fn install(delegate: &AnyObject, view: usize, hook: Hook) {
            let cls = delegate.class();
            let &(sub, base) = CLASSES.get_or_init(|| {
                let mut b = ClassBuilder::new(c"BlyggerBrowserNavigationDelegate", cls)
                    .expect("the browser's delegate class is registered once");
                // SAFETY: the signature matches WKNavigationDelegate's
                // webView:decidePolicyForNavigationAction:decisionHandler:.
                unsafe {
                    b.add_method(
                        sel!(webView:decidePolicyForNavigationAction:decisionHandler:),
                        decide as extern "C-unwind" fn(_, _, _, _, _),
                    );
                }
                let sub = b.register();
                (
                    sub as *const AnyClass as usize,
                    cls as *const AnyClass as usize,
                )
            });
            if cls as *const AnyClass as usize != base {
                // Not the class the subclass was made from: leave it alone.
                return;
            }
            // SAFETY: the subclass derives from the object's own class and
            // adds no ivars, so the object's layout is unchanged.
            unsafe {
                AnyObject::set_class(delegate, &*(sub as *const AnyClass));
            }
            HOOKS.with(|h| h.borrow_mut().insert(view, hook));
        }

        pub fn uninstall(view: usize) {
            HOOKS.with(|h| h.borrow_mut().remove(&view));
        }
    }
}
