//! The browser pane's policies: URLs and schemes, link clicks, the address
//! field, the shield, and the notes link; plus the pane's behaviour on a
//! stub surface.

use std::collections::BTreeSet;

use super::*;

#[test]
fn only_web_pages_load_in_the_pane() {
    assert_eq!(nav_policy("https://blyg.example.com/f/abc/"), Nav::Allow);
    assert_eq!(nav_policy("http://blyg.example.com/"), Nav::Allow);
    assert_eq!(nav_policy("HTTPS://BLYG.EXAMPLE.COM/"), Nav::Allow);
    assert_eq!(nav_policy("about:blank"), Nav::Allow);
    assert_eq!(nav_policy("about:srcdoc"), Nav::Allow);
    assert_eq!(
        nav_policy("mailto:ada@example.com"),
        Nav::System("mailto:ada@example.com".into())
    );
    for bad in [
        "file:///etc/passwd",
        "FILE:///Users/",
        "javascript:alert(1)",
        "data:text/html,<script>alert(1)</script>",
        "blob:https://blyg.example.com/1234",
        "blygger://token",
        "about:config",
        "ftp://example.com/",
        "https://",
        "",
    ] {
        assert_eq!(nav_policy(bad), Nav::Deny, "{bad}");
    }
    assert!(is_web_url("https://blyg.example.com/"));
    assert!(!is_web_url("about:blank"));
    assert!(!is_web_url("mailto:ada@example.com"));
}

#[test]
fn clicks_follow_modifiers_and_open_links() {
    use LinkAction::*;
    use OpenMode::*;
    // open-links = app (the default)
    assert_eq!(link_action(OpenLinks::App, false, false), Pane(Slide));
    assert_eq!(link_action(OpenLinks::App, true, false), Pane(Full));
    assert_eq!(link_action(OpenLinks::App, false, true), DefaultBrowser);
    // open-links = browser: ⌥ is the way into the pane
    assert_eq!(
        link_action(OpenLinks::Browser, false, false),
        DefaultBrowser
    );
    assert_eq!(link_action(OpenLinks::Browser, false, true), Pane(Slide));
    assert_eq!(link_action(OpenLinks::Browser, true, false), Pane(Full));
    assert_eq!(OpenLinks::from_value(Some("browser")), OpenLinks::Browser);
    assert_eq!(OpenLinks::from_value(Some("app")), OpenLinks::App);
    assert_eq!(OpenLinks::from_value(None), OpenLinks::App);
}

#[test]
fn the_address_field_takes_web_addresses_only() {
    assert_eq!(
        parse_address(" https://blyg.example.com/a "),
        Some("https://blyg.example.com/a".into())
    );
    assert_eq!(
        parse_address("blyg.example.com/f/abc?x=1"),
        Some("https://blyg.example.com/f/abc?x=1".into())
    );
    assert_eq!(
        parse_address("localhost:8787/blyg"),
        Some("https://localhost:8787/blyg".into())
    );
    assert_eq!(
        parse_address("http://127.0.0.1:8080/"),
        Some("http://127.0.0.1:8080/".into())
    );
    for bad in [
        "",
        "tide pools",
        "javascript:alert(1)",
        "file:///etc/passwd",
        "mailto:ada@example.com",
        "data:text/html,hi",
        "notahost",
        "ftp://example.com",
    ] {
        assert_eq!(parse_address(bad), None, "{bad}");
    }
}

#[test]
fn the_shield_is_per_host_and_the_config_switch_wins() {
    let mut off = BTreeSet::new();
    off.insert("news.example.com".to_string());
    assert!(blocking_applies(true, &off, "https://blyg.example.com/"));
    assert!(!blocking_applies(
        true,
        &off,
        "https://news.example.com/story"
    ));
    assert!(!blocking_applies(true, &off, "https://NEWS.example.com/"));
    // Only that host: a subdomain has its own shield.
    assert!(blocking_applies(true, &off, "https://m.news.example.com/"));
    assert!(!blocking_applies(
        false,
        &BTreeSet::new(),
        "https://blyg.example.com/"
    ));
    assert_eq!(
        host_of("https://Blyg.Example.com:8443/x").as_deref(),
        Some("blyg.example.com")
    );
    assert_eq!(host_of("mailto:ada@example.com"), None);
}

#[test]
fn notes_links_are_markdown() {
    assert_eq!(
        notes_link("Tide tables", "https://blyg.example.com/f/abc/"),
        "[Tide tables](https://blyg.example.com/f/abc/)"
    );
    assert_eq!(
        notes_link(
            "  [Draft] notes\non tides ",
            "https://blyg.example.com/a (b)"
        ),
        "[\\[Draft\\] notes on tides](https://blyg.example.com/a%20%28b%29)"
    );
    assert_eq!(
        notes_link("", "https://blyg.example.com/"),
        "[https://blyg.example.com/](https://blyg.example.com/)"
    );
    assert_eq!(
        display_label("", "https://blyg.example.com/f/"),
        "blyg.example.com/f/"
    );
    assert_eq!(display_label(" Tides ", "https://x.example/"), "Tides");
}

// ------------------------------------------------------------ the pane

mod pane {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext, px};

    use super::super::surface::{BrowserSurface, FactoryGlobal, PageState};
    use crate::app::MainView;

    #[derive(Default)]
    struct Log {
        loads: Vec<String>,
        visible: bool,
        created: usize,
        dropped: usize,
        /// The page has the keyboard (a field in it is focused).
        page_keyboard: bool,
        commands: Vec<String>,
    }

    struct Stub(Rc<RefCell<Log>>);

    impl Drop for Stub {
        fn drop(&mut self) {
            self.0.borrow_mut().dropped += 1;
        }
    }

    impl BrowserSurface for Stub {
        fn set_frame(&mut self, _: Bounds<Pixels>) {}
        fn set_visible(&mut self, v: bool) {
            self.0.borrow_mut().visible = v;
        }
        fn load_url(&mut self, url: &str) {
            self.0.borrow_mut().loads.push(url.to_string());
        }
        fn back(&mut self) {}
        fn forward(&mut self) {}
        fn reload(&mut self) {}
        fn stop(&mut self) {}
        fn state(&self) -> PageState {
            PageState {
                url: self.0.borrow().loads.last().cloned().unwrap_or_default(),
                title: "Tide tables".into(),
                ..PageState::default()
            }
        }
        fn refresh_blocking(&mut self) {}
        fn focus_parent(&mut self) {}
        fn page_has_keyboard(&self) -> bool {
            self.0.borrow().page_keyboard
        }
        fn edit_command(&mut self, selector: &str) {
            self.0.borrow_mut().commands.push(selector.to_string());
        }
    }

    fn setup(
        cx: &mut TestAppContext,
    ) -> (Entity<MainView>, &mut VisualTestContext, Rc<RefCell<Log>>) {
        use std::sync::Arc;

        use blyg_core::config::MemoryTokenStore;
        use blyg_core::{Backend, ConfigStore};

        use crate::fake::{FakeBackend, Timing};
        use crate::prefs::Prefs;

        let config = "blyg-url = https://blyg.example.com\n".to_string();
        let prefs = Prefs::from_config(ConfigStore::in_memory(&config).config());
        let log = Rc::new(RefCell::new(Log::default()));
        let l = log.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::app::bind_keys(cx);
            crate::settings::init(
                ConfigStore::in_memory(&config),
                Arc::new(MemoryTokenStore::default()),
                None,
                cx,
            );
            cx.set_global(FactoryGlobal(Rc::new(move |_, _, _, _| {
                l.borrow_mut().created += 1;
                Ok(Box::new(Stub(l.clone())) as Box<dyn BrowserSurface>)
            })));
        });
        let fake = Arc::new(FakeBackend::with_timing(Timing::instant()).without_media_cache());
        let backend: Arc<dyn Backend> = fake.clone();
        let (view, cx) = cx.add_window_view(move |window, cx| {
            MainView::new(
                backend,
                Some(fake),
                prefs,
                std::time::Instant::now(),
                window,
                cx,
            )
        });
        cx.run_until_parked();
        (view, cx, log)
    }

    #[gpui_kit::test]
    fn links_open_in_the_pane_lazily_and_esc_closes_it(cx: &mut TestAppContext) {
        let (view, cx, log) = setup(cx);
        assert_eq!(
            log.borrow().created,
            0,
            "no web view until a link is opened"
        );
        view.update_in(cx, |v, window, cx| {
            v.open_link("https://blyg.example.com/f/abc/".into(), window, cx)
        });
        cx.run_until_parked();
        assert_eq!(log.borrow().created, 1);
        assert_eq!(log.borrow().loads, vec!["https://blyg.example.com/f/abc/"]);
        view.read_with(cx, |v, _| {
            assert!(v.browser.open);
            assert_eq!(v.browser.mode, super::OpenMode::Slide);
            assert!(v.browser.visible(), "placed and shown");
        });
        // The same link again: no reload. A different one: navigates.
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(
                "https://blyg.example.com/f/abc/",
                super::OpenMode::Full,
                window,
                cx,
            );
            v.open_url_in_app(
                "https://blyg.example.com/f/def/",
                super::OpenMode::Full,
                window,
                cx,
            );
        });
        assert_eq!(log.borrow().loads.len(), 2);
        assert_eq!(log.borrow().created, 1, "one web view");
        // Only web pages.
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app("file:///etc/passwd", super::OpenMode::Slide, window, cx)
        });
        assert_eq!(log.borrow().loads.len(), 2);
        // esc closes it; the web view stays (for a while) and so does the page.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        view.read_with(cx, |v, _| {
            assert!(!v.browser.open);
            assert!(v.browser.alive());
            assert_eq!(v.browser.page.url, "https://blyg.example.com/f/def/");
        });
        assert!(!log.borrow().visible, "hidden when closed");
        // ⇧⌘B brings it back without loading anything.
        cx.simulate_keystrokes("cmd-shift-b");
        cx.run_until_parked();
        view.read_with(cx, |v, _| assert!(v.browser.open));
        assert_eq!(log.borrow().loads.len(), 2);
    }

    #[gpui_kit::test]
    fn a_closed_pane_frees_its_web_view(cx: &mut TestAppContext) {
        let (view, cx, log) = setup(cx);
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(
                "https://blyg.example.com/",
                super::OpenMode::Slide,
                window,
                cx,
            )
        });
        view.update_in(cx, |v, window, cx| v.close_browser(window, cx));
        cx.executor()
            .advance_clock(super::super::TEARDOWN_AFTER + std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(log.borrow().dropped, 1);
        view.read_with(cx, |v, _| assert!(!v.browser.alive()));
        // Reopening makes a new one and loads the last page again.
        cx.simulate_keystrokes("cmd-shift-b");
        cx.run_until_parked();
        assert_eq!(log.borrow().created, 2);
        assert_eq!(
            log.borrow().loads,
            vec!["https://blyg.example.com/", "https://blyg.example.com/"]
        );
    }

    /// A recording preview (the full editor's web view), to see where the
    /// pane leaves it.
    #[derive(Default)]
    struct PreviewLog {
        frames: Vec<Bounds<Pixels>>,
        visible: bool,
    }

    struct PreviewStub(Rc<RefCell<PreviewLog>>);

    impl crate::app::studio::webview::PreviewSurface for PreviewStub {
        fn set_frame(&mut self, b: Bounds<Pixels>) {
            self.0.borrow_mut().frames.push(b);
        }
        fn set_visible(&mut self, v: bool) {
            self.0.borrow_mut().visible = v;
        }
        fn load(&mut self, _: &str) {}
        fn eval(&mut self, _: &str) {}
        fn focus_parent(&mut self) {}
        fn set_dark(&mut self, _: bool) {}
    }

    #[gpui_kit::test]
    fn other_web_views_stay_out_from_under_the_pane(cx: &mut TestAppContext) {
        let preview = Rc::new(RefCell::new(PreviewLog::default()));
        let p2 = preview.clone();
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::app::studio::webview::FactoryGlobal(Rc::new(
                move |_, _| {
                    Ok(Box::new(PreviewStub(p2.clone()))
                        as Box<dyn crate::app::studio::webview::PreviewSurface>)
                },
            )));
        });
        let (view, cx, _log) = setup(cx);
        let frame = |cx: &mut VisualTestContext| {
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
        };
        // ⌘2: the preview beside the editor.
        cx.simulate_keystrokes("cmd-2");
        frame(cx);
        let full_w = preview.borrow().frames.last().unwrap().size.width;
        assert!(preview.borrow().visible);
        let viewport = cx.update(|window, _| window.viewport_size().width);
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(
                "https://blyg.example.com/",
                super::OpenMode::Slide,
                window,
                cx,
            )
        });
        frame(cx);
        let edge = (viewport * (1. - super::super::view::SLIDE_WIDTH)).round();
        {
            let log = preview.borrow();
            if log.visible {
                let f = log.frames.last().unwrap();
                assert!(
                    f.origin.x + f.size.width <= edge + px(0.5),
                    "{f:?} past {edge:?}"
                );
                assert!(f.size.width < full_w);
            }
        }
        // ⌘-click (the whole area): nothing of the preview is left.
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(
                "https://blyg.example.com/2",
                super::OpenMode::Full,
                window,
                cx,
            )
        });
        frame(cx);
        assert!(!preview.borrow().visible, "hidden under a full pane");
        // Closed: the preview gets its whole pane back.
        view.update_in(cx, |v, window, cx| v.close_browser(window, cx));
        frame(cx);
        assert!(preview.borrow().visible);
        assert_eq!(preview.borrow().frames.last().unwrap().size.width, full_w);
    }

    /// The Edit menu's actions reach a page field that has the keyboard
    /// (they're GPUI actions, which the page would never see).
    #[gpui_kit::test]
    fn edit_commands_go_to_a_focused_page(cx: &mut TestAppContext) {
        use gpui_kit::base::input::{Copy, Paste};
        let (view, cx, log) = setup(cx);
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(
                "https://blyg.example.com/",
                super::OpenMode::Slide,
                window,
                cx,
            )
        });
        cx.run_until_parked();
        cx.dispatch_action(Paste);
        assert!(log.borrow().commands.is_empty(), "GPUI has the keyboard");
        log.borrow_mut().page_keyboard = true;
        cx.dispatch_action(Paste);
        cx.dispatch_action(Copy);
        assert_eq!(log.borrow().commands, vec!["paste:", "copy:"]);
        // Closed: never.
        view.update_in(cx, |v, window, cx| v.close_browser(window, cx));
        cx.dispatch_action(Copy);
        assert_eq!(log.borrow().commands.len(), 2);
    }

    #[gpui_kit::test]
    fn notes_link_goes_to_the_notes_drawer(cx: &mut TestAppContext) {
        let (view, cx, _log) = setup(cx);
        view.update_in(cx, |v, window, cx| {
            v.open_url_in_app(
                "https://blyg.example.com/f/abc/",
                super::OpenMode::Slide,
                window,
                cx,
            );
        });
        cx.run_until_parked();
        view.update_in(cx, |v, window, cx| {
            v.browser_refresh_state(cx);
            v.browser_send_to_notes(window, cx);
        });
        cx.run_until_parked();
        // --- notes --- into the drawer, not the clipboard.
        assert!(cx.read_from_clipboard().is_none());
        view.read_with(cx, |v, cx| {
            assert!(v.notes.open);
            assert!(
                v.notes
                    .text(cx)
                    .ends_with("[Tide tables](https://blyg.example.com/f/abc/)\n\n")
            );
        });
    }
}
