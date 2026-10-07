//! The About window, headless: the menu action opens it (once), and "Copy
//! build info" copies the report without paths.

use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};

use super::{ShowAbout, window};

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        super::init(cx);
    });
}

#[gpui_kit::test]
fn the_menu_action_opens_the_about_window_once(cx: &mut TestAppContext) {
    setup(cx);
    assert!(cx.update(|cx| window(cx)).is_none());
    cx.update(|cx| cx.dispatch_action(&ShowAbout));
    cx.run_until_parked();
    let first = cx.update(|cx| window(cx)).expect("About window open");
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    // Asking again brings the same window forward.
    cx.update(|cx| cx.dispatch_action(&ShowAbout));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| window(cx)), Some(first));
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
}

#[test]
fn both_menus_have_about_blygger() {
    for menu in ["Burrow", "Help"] {
        let m = crate::menus(true)
            .into_iter()
            .find(|m| m.name == menu)
            .unwrap_or_else(|| panic!("no {menu} menu"));
        assert!(
            m.items.iter().any(|i| matches!(
                i,
                gpui_kit::MenuItem::Action { name, action, .. }
                    if name == "About Burrow" && action.name().ends_with("ShowAbout")
            )),
            "{menu} › About Burrow"
        );
    }
    // The standard place: first in the app menu.
    let app = crate::menus(true).into_iter().next().expect("app menu");
    assert!(matches!(
        &app.items[0],
        gpui_kit::MenuItem::Action { name, .. } if name == "About Burrow"
    ));
}

#[gpui_kit::test]
fn copy_build_info_puts_the_report_on_the_clipboard(cx: &mut TestAppContext) {
    setup(cx);
    cx.update(super::open);
    cx.run_until_parked();
    let h = cx.update(|cx| window(cx)).expect("open");
    let vcx = &mut VisualTestContext::from_window(h.into(), cx);
    vcx.run_until_parked();
    for id in [
        "about-check",
        "about-github",
        "about-release",
        "about-license",
        "about-reveal-data",
    ] {
        assert!(vcx.debug_bounds(id).is_some(), "{id} is drawn");
    }
    let b = vcx.debug_bounds("about-copy").expect("copy button");
    vcx.simulate_click(b.center(), Modifiers::none());
    vcx.run_until_parked();
    let text = vcx
        .read_from_clipboard()
        .and_then(|c| c.text())
        .expect("copied");
    assert!(text.starts_with("Burrow "), "{text}");
    assert!(text.contains(&format!("Version:      {}", env!("CARGO_PKG_VERSION"))));
    assert!(text.contains("Commit:"), "{text}");
    assert!(
        text.contains("Update:       checks are off (test build)"),
        "{text}"
    );
    assert!(text.contains("Blyg:         not connected"), "{text}");
    assert!(!text.contains('/'), "no paths: {text}");
}

/// Live: the host only (never the URL's path, never the token) and what the
/// backend found out about the server.
#[gpui_kit::test]
fn a_live_connection_shows_the_host_and_capabilities(cx: &mut TestAppContext) {
    use std::sync::Arc;

    use blyg_core::config::{MemoryTokenStore, TokenStore as _};
    use blyg_core::{Backend, ConfigStore};

    use crate::connection::{Connection, Mode, SwitchBackend};
    use crate::fake::FakeBackend;

    setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let url = "https://blyg.example.com/";
    let tokens = MemoryTokenStore::default();
    tokens.set(url, "sekrit-owner-token").unwrap();
    let fake = Arc::new(FakeBackend::new());
    fake.set_provenance_available(false);
    let backend: Arc<dyn Backend> = fake;
    cx.update(|cx| {
        crate::settings::init(
            ConfigStore::in_memory(&format!("blyg-url = {url}\n")),
            Arc::new(tokens),
            None,
            cx,
        );
        cx.set_global(Connection {
            switch: SwitchBackend::new(backend, Mode::Live),
            data_dir: dir.path().to_path_buf(),
            verify: crate::connection::fake_verifier,
        });
    });
    let r = cx.update(|cx| super::report(cx));
    assert_eq!(
        r.connection,
        super::info::ConnectionInfo::Live {
            host: "blyg.example.com".into(),
            caps: super::info::capabilities(true, true, false),
        }
    );
    let text = r.copy_text();
    assert!(
        text.contains("Blyg:              blyg.example.com\n"),
        "{text}"
    );
    assert!(text.contains("AI disclosure:     no\n"), "{text}");
    assert!(!text.contains("sekrit"), "{text}");
}
