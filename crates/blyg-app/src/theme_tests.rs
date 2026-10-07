//! --- themes --- Headless GPUI tests: every theme renders every surface
//! (posts, the stream with its quote boxes, Settings, a toast) without
//! panicking, Settings switches themes and writes `theme`, and a user theme
//! with its own SVG ornaments renders.

use std::sync::Arc;

use blyg_core::config::MemoryTokenStore;
use blyg_core::config::theme::Registry;
use blyg_core::{Backend, ConfigStore};
use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext};

use super::MainView;
use crate::fake::{FakeBackend, Timing};
use crate::prefs::Prefs;

const CONNECTED: &str = "# test config\nblyg-url = https://blyg.example.com\n";

fn setup<'a>(
    cx: &'a mut TestAppContext,
    config: &str,
    themes: Option<Registry>,
) -> (Entity<MainView>, &'a mut VisualTestContext) {
    let config = config.to_string();
    let prefs = Prefs::from_config(ConfigStore::in_memory(&config).config());
    cx.update(|cx| {
        gpui_kit::init(cx);
        super::bind_keys(cx);
        crate::settings::init(
            ConfigStore::in_memory(&config),
            Arc::new(MemoryTokenStore::default()),
            None,
            cx,
        );
        if let Some(r) = themes {
            cx.set_global(crate::theme::Themes::new(r));
        }
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
    (view, cx)
}

fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn switch_to(view: &Entity<MainView>, id: &str, cx: &mut VisualTestContext) {
    let id = id.to_string();
    view.update_in(cx, |v, window, cx| {
        v.prefs.theme = id;
        v.apply_prefs_live(window, cx);
    });
    frame(cx);
}

#[gpui_kit::test]
fn every_theme_renders_every_surface(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx, CONNECTED, None);
    let ids = crate::theme::builtins().registry.ids();
    assert_eq!(ids.len(), 10);
    for id in &ids {
        switch_to(&view, id, cx);
        view.read_with(cx, |v, _| {
            assert_eq!(&v.theme.id, id);
            assert_eq!(v.palette, v.theme.palette);
        });
        // The stream (quote boxes, dividers between posts), then back.
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-r"));
        frame(cx);
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-r"));
        frame(cx);
        // Settings (the themes list with swatches), then a toast.
        cx.simulate_keystrokes(&crate::keymap::keys("cmd-,"));
        frame(cx);
        cx.simulate_keystrokes("escape");
        view.update(cx, |v, cx| v.show_toast("Published v3", None, cx));
        frame(cx);
    }
    // Fonts: a theme's apply until the config sets its own.
    switch_to(&view, "portolan", cx);
    view.read_with(cx, |v, _| assert_eq!(v.prefs.writing().family, "ETBembo"));
    switch_to(&view, "fortress", cx);
    view.read_with(cx, |v, _| {
        // Windows has no Menlo: the theme gets its substitute.
        let menlo = if cfg!(target_os = "windows") {
            "Consolas"
        } else {
            "Menlo"
        };
        assert_eq!(v.prefs.ui().family, menlo);
        assert!(v.palette.dark);
    });
    switch_to(&view, "system", cx);
    view.read_with(cx, |v, _| assert_eq!(v.prefs.writing().family, "Literata"));
}

#[gpui_kit::test]
fn settings_lists_themes_and_writes_the_choice(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx, CONNECTED, None);
    cx.simulate_keystrokes(&crate::keymap::keys("cmd-,"));
    cx.run_until_parked();
    for _ in 0..3 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(150));
        frame(cx);
    }
    for id in ["system", "light", "dark", "cutaway", "konkan"] {
        let sel: &'static str = Box::leak(format!("th-{id}").into_boxed_str());
        assert!(cx.debug_bounds(sel).is_some(), "{sel} isn't in Settings");
    }
    crate::app::toolbar::tests::scroll_into_view(cx, "th-aizome");
    let b = crate::app::toolbar::tests::settled_bounds(cx, "th-aizome");
    cx.simulate_click(b.center(), Modifiers::none());
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.prefs.theme, "aizome");
        assert_eq!(v.theme.id, "aizome");
    });
    let text = cx.update(|_, cx| {
        crate::settings::get(cx)
            .store
            .text()
            .unwrap_or_default()
            .to_string()
    });
    assert!(text.ends_with("theme = aizome\n"), "{text}");
}

#[gpui_kit::test]
fn a_user_theme_with_svg_ornaments_renders(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let art = dir.path().join("art.svg");
    std::fs::write(
        &art,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><path d="M1 9 L5 1 L9 9Z"/></svg>"#,
    )
    .unwrap();
    let a = art.display();
    let text = format!(
        "inherit = konkan\nname = Mine\ntitlebar.band = svg({a})\nsidebar.ground = svg({a}, #ffffff)\n\
         sidebar.top = svg({a})\neditor.frame = svg({a})\ntexture = svg({a})\n\
         divider = svg({a})\nempty.art = svg({a})\n"
    );
    let mut r = Registry::builtin();
    r.add_user("mine", &text, &dir.path().join("mine"));
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    let (view, cx) = setup(cx, &format!("{CONNECTED}theme = mine\n"), Some(r));
    view.read_with(cx, |v, _| {
        assert_eq!(v.theme.id, "mine");
        assert!(!v.theme.builtin);
        assert!(v.config_problems.is_empty(), "{:?}", v.config_problems);
    });
    frame(cx);
    cx.simulate_keystrokes(&crate::keymap::keys("cmd-r"));
    frame(cx);
}
