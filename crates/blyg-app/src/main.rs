//! Blygger Desktop: a Notational-Velocity-style GPUI client for a blyg.
//!
//! With a `blyg-url` in the config and its owner token in the Keychain, the
//! app runs on `LiveBackend` (local SQLite + sync with the blyg). With none,
//! it asks you to connect one. `BLYGGER_FAKE=1 cargo run -p blyg-app` runs
//! the full UI on in-memory sample data instead (see `connection.rs`).
//!
//! Settings come from one plain-text config file (see `blygger +show-config
//! --default --docs`); `blygger +action` runs a command-line action instead
//! of the app (see `cli.rs`).

mod ai;
mod app;
mod capture;
mod cli;
mod connection;
mod fake;
mod fonts;
mod images;
mod keymap;
mod platform;
mod prefs;
mod settings;
mod theme;
mod update;
mod vm;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use blyg_core::config::{ConfigFiles, MemoryTokenStore, TokenStore};
use blyg_core::{Backend, ConfigStore};
use gpui_kit::*;

use app::MainView;
use connection::{Connection, Disconnected, Mode, SwitchBackend};
use fake::FakeBackend;
use prefs::Prefs;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli::run(&args) {
        return code;
    }
    let launched = Instant::now();

    let files = ConfigFiles::discover();
    let notice = settings::migrate_on_launch(&files.primary);
    let store = ConfigStore::open(files);
    for d in settings::diagnostics(store.loaded()) {
        eprintln!("blygger: {d}");
    }
    let mut prefs = Prefs::from_config(store.config());
    // Dev/screenshot override; not persisted unless a setting is changed.
    match std::env::var("BLYGGER_THEME").as_deref() {
        Ok("light") => prefs.theme = prefs::ThemePref::Light,
        Ok("dark") => prefs.theme = prefs::ThemePref::Dark,
        _ => {}
    }
    // Fake mode never touches the real Keychain.
    let fake_mode = std::env::var_os("BLYGGER_FAKE").is_some();
    // BLYGGER_TEST_TOKEN (automation against a local `wrangler dev`): an
    // in-memory token for the configured blyg-url, and no Keychain at all.
    let tokens: Arc<dyn TokenStore> = if fake_mode {
        Arc::new(MemoryTokenStore::default())
    } else if let Ok(t) = std::env::var("BLYGGER_TEST_TOKEN") {
        let m = MemoryTokenStore::default();
        if let Some(u) = store.config().blyg_url() {
            let _ = m.set(u, t.trim());
        }
        Arc::new(m)
    } else {
        Arc::new(blyg_core::config::KeychainTokenStore)
    };

    // Which backend: fake (dev), live (a configured blyg with a token), or
    // disconnected until the Connect sheet succeeds.
    let data_dir = blyg_core::config::data_dir();
    images::init(&data_dir);
    let fake = fake_mode.then(|| Arc::new(FakeBackend::new()));
    // BLYGGER_FAKE_NO_PROVENANCE=1: act like a Worker without the provenance
    // extension (to see the publish-without-disclosure warning).
    if let Some(f) = &fake
        && std::env::var_os("BLYGGER_FAKE_NO_PROVENANCE").is_some()
    {
        f.set_provenance_available(false);
    }
    let (inner, mode): (Arc<dyn Backend>, Mode) = match (&fake, store.config().blyg_url()) {
        (Some(f), _) => (f.clone(), Mode::Fake),
        (None, Some(url)) => match connection::open_live(&data_dir, url, &*tokens) {
            Ok(Some(live)) => (live, Mode::Live),
            Ok(None) => {
                eprintln!("blygger: no owner token for {url} in the Keychain; connect again");
                (Arc::new(Disconnected), Mode::Disconnected)
            }
            Err(e) => {
                eprintln!("blygger: couldn't open the local database: {e}");
                (Arc::new(Disconnected), Mode::Disconnected)
            }
        },
        (None, None) => (Arc::new(Disconnected), Mode::Disconnected),
    };
    if std::env::var_os("BLYGGER_TIMING").is_some() {
        println!("backend={mode:?}");
    }
    let switch = SwitchBackend::new(inner, mode);
    let backend: Arc<dyn Backend> = switch.clone();
    let connection = Connection {
        switch,
        data_dir,
        verify: if fake_mode {
            connection::fake_verifier
        } else {
            blyg_core::api::verify_connection
        },
    };

    let state_dir = connection.data_dir.clone(); // --- auto-update ---

    let app = gpui_kit::application();
    // Dock click with no window open → reopen the main window.
    {
        let (b, f) = (backend.clone(), fake.clone());
        app.on_reopen(move |cx| {
            let alive =
                capture::main_window(cx).is_some_and(|h| h.update(cx, |_, _, _| ()).is_ok());
            if !alive {
                let prefs = settings::prefs(cx);
                open_main(b.clone(), f.clone(), prefs, Instant::now(), cx);
            }
        });
    }
    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        platform::set_dock_icon_unless_bundled();
        settings::init(store, tokens, notice, cx);
        ai::init(cx);
        cx.set_global(connection);
        // A remote image (a profile avatar) landed in the cache: repaint.
        {
            let (tx, rx) = async_channel::unbounded::<()>();
            images::set_on_loaded(move || {
                let _ = tx.try_send(());
            });
            cx.spawn(async move |cx| {
                while rx.recv().await.is_ok() {
                    cx.update(|cx| cx.refresh_windows());
                }
            })
            .detach();
        }
        fonts::ensure(prefs.writing().bundled, cx);
        fonts::ensure(prefs.ui().bundled, cx);
        // Status bar, hints and pills are always Inter (as in the mock).
        fonts::ensure(Some(fonts::Bundle::Inter), cx);

        app::bind_keys(cx);
        cx.on_action(|_: &app::Quit, cx| cx.quit());
        cx.on_action(|_: &app::ShowCapture, cx| capture::toggle(cx));
        cx.set_menus(menus());
        capture::init(backend.clone(), &prefs, cx);
        // --- auto-update --- (off in fake mode, tests and BLYGGER_NO_UPDATE)
        update::init(state_dir, cx);

        open_main(backend.clone(), fake.clone(), prefs.clone(), launched, cx);
        // Polite activation in automation: don't steal focus from the user.
        cx.activate(!no_activate());
    });
    ExitCode::SUCCESS
}

fn open_main(
    backend: Arc<dyn Backend>,
    fake: Option<Arc<FakeBackend>>,
    prefs: Prefs,
    launched: Instant,
    cx: &mut App,
) {
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(vm::window_title(settings::blyg_url(cx).as_deref()).into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(11.))),
        }),
        window_min_size: Some(size(px(640.), px(360.))),
        app_id: Some(blyg_core::config::APP_ID.into()),
        focus: !no_activate(),
        ..Default::default()
    };
    match cx.open_window(opts, |window, cx| {
        cx.new(|cx| MainView::new(backend, fake, prefs, launched, window, cx))
    }) {
        Ok(handle) => {
            capture::set_main(handle, cx);
            if no_activate() {
                let _ = handle.update(cx, |_, window, _| order_front_regardless(window));
            }
            if let Ok(scenario) = std::env::var("BLYGGER_DEMO") {
                let _ = handle.update(cx, |view, window, cx| view.run_demo(&scenario, window, cx));
            }
        }
        Err(e) => eprintln!("blygger: couldn't open the main window: {e}"),
    }
}

/// `BLYGGER_NO_ACTIVATE=1`: never take focus from the frontmost app (used by
/// automated screenshot runs so they can't swallow the user's keystrokes).
pub fn no_activate() -> bool {
    std::env::var_os("BLYGGER_NO_ACTIVATE").is_some()
}

/// Show a window without activating the app (automation only): an app that
/// never activates doesn't get its windows ordered in otherwise.
pub fn order_front_regardless(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::AppKit(h) = handle.as_raw() {
        let view = h.ns_view.as_ptr() as *mut objc2::runtime::AnyObject;
        // SAFETY: ns_view is a live NSView owned by this window; we're on the
        // main thread (GPUI's foreground executor).
        unsafe {
            let ns_window: *mut objc2::runtime::AnyObject = objc2::msg_send![view, window];
            if !ns_window.is_null() {
                let _: () = objc2::msg_send![ns_window, orderFrontRegardless];
            }
        }
    }
}

fn menus() -> Vec<Menu> {
    use gpui_kit::base::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
    vec![
        Menu {
            name: "Blygger".into(),
            items: vec![
                MenuItem::action("Settings…", app::OpenSettings),
                MenuItem::action("Check for Updates…", update::CheckForUpdates), // --- auto-update ---
                MenuItem::action("Open Config File", app::OpenConfigFile),
                MenuItem::action("Reload Config", app::ReloadConfig),
                MenuItem::action("Quick Capture", app::ShowCapture),
                MenuItem::separator(),
                MenuItem::action("Disconnect…", app::Disconnect),
                MenuItem::separator(),
                MenuItem::action("Quit Blygger", app::Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", Cut),
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
                MenuItem::action("Select All", SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "Post".into(),
            items: vec![
                MenuItem::action("New Draft", app::NewDraft),
                MenuItem::action("Find…", app::FocusSearch),
                MenuItem::separator(),
                MenuItem::action("Publish…", app::Publish),
                MenuItem::action("Make Draft", app::scratch::MakeDraft),
                MenuItem::action("Fragment ⇄ Thread", app::ToggleKind),
                MenuItem::action("Open on the Web", app::OpenPermalink),
                MenuItem::separator(),
                MenuItem::action("Generate (TK)", ai::AiGenerate),
                MenuItem::action("Shorten to Fit 1000", ai::AiShorten),
                // --- reading & versions ---
                MenuItem::separator(),
                MenuItem::action("Versions…", app::reading::ShowVersions),
                MenuItem::action("Quote…", app::reading::QuotePicker),
                // --- delete & withdraw ---
                MenuItem::separator(),
                MenuItem::action("Delete Draft…", app::discard::DeleteDraft),
                MenuItem::action("Withdraw…", app::discard::Withdraw),
            ],
            disabled: false,
        },
        // --- reading & versions ---
        Menu {
            name: "Blyg".into(),
            items: vec![
                MenuItem::action("Reading", app::reading::ShowReading),
                MenuItem::action("Mentions", app::reading::ShowMentions),
                MenuItem::action("Subscriptions", app::reading::ShowSubscriptions),
                MenuItem::separator(),
                MenuItem::action("Subscribe…", app::reading::SubscribeTo),
                MenuItem::action("Site Settings…", app::reading::SiteSettings),
                // --- profiles ---
                MenuItem::separator(),
                MenuItem::action("Profile", app::profiles::ShowProfile),
                MenuItem::action("Open Profile…", app::profiles::OpenProfile),
                MenuItem::action("My Profile", app::profiles::MyProfile),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                // --- full editor ---
                MenuItem::action("Write", app::studio::ViewWrite),
                MenuItem::action("List + Editor + Preview", app::studio::ViewSplit),
                MenuItem::action("Full Editor", app::studio::ViewStudio),
                MenuItem::action("Preview", app::TogglePreview),
                MenuItem::separator(),
                MenuItem::action("Bigger", app::FontBigger),
                MenuItem::action("Smaller", app::FontSmaller),
                MenuItem::action("Actual Size", app::FontReset),
            ],
            disabled: false,
        },
        // --- onboarding ---
        Menu {
            name: "Help".into(),
            items: vec![MenuItem::action(
                "Blygger Tutorial",
                app::onboarding::ShowTutorial,
            )],
            disabled: false,
        },
    ]
}

// --- buttons ---
#[cfg(test)]
mod menu_tests {
    use gpui_kit::MenuItem;

    /// Menus, shortcuts and toolbar buttons all come from `keymap::table()`:
    /// every menu item here is a table row with the same menu path and
    /// action, and every row with a menu is in a menu.
    #[test]
    fn menus_match_the_keymap_table() {
        let table = crate::keymap::table();
        let mut shown = Vec::new();
        for menu in super::menus() {
            if menu.name == "Edit" {
                continue; // the standard text-editing items (gpui-base's actions)
            }
            for item in menu.items {
                if let MenuItem::Action { name, action, .. } = item {
                    let path = format!("{} › {}", menu.name, name);
                    let action = action.name().rsplit("::").next().unwrap_or_default();
                    assert!(
                        table
                            .iter()
                            .any(|k| k.menu == Some(path.as_str()) && k.action == action),
                        "menu item {path} ({action}) isn't in keymap::table()"
                    );
                    shown.push(path);
                }
            }
        }
        for k in table.iter().filter_map(|k| k.menu) {
            assert!(
                shown.iter().any(|s| s == k),
                "{k} is in the table but no menu"
            );
        }
    }
}
