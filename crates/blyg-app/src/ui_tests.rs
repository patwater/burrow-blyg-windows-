//! Headless GPUI tests of the main window's key handling: real keystrokes go
//! through GPUI's dispatch (bindings, capture actions, focus) on the test
//! platform, against a FakeBackend with zero latency.

use std::sync::Arc;

use blyg_core::config::{Change, MemoryTokenStore, TokenStore};
use blyg_core::{Backend, ConfigStore, Kind, LocalId, Status};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

use super::{MainView, Mode, Sheet};
use crate::fake::{FakeBackend, Timing};
use crate::prefs::Prefs;

const CONNECTED: &str = "# test config\nblyg-url = https://blyg.example.com\n";

fn setup(cx: &mut TestAppContext) -> (Entity<MainView>, Arc<FakeBackend>, &mut VisualTestContext) {
    setup_with(cx, CONNECTED, Arc::new(MemoryTokenStore::default()))
}

fn setup_with<'a>(
    cx: &'a mut TestAppContext,
    config: &str,
    tokens: Arc<MemoryTokenStore>,
) -> (
    Entity<MainView>,
    Arc<FakeBackend>,
    &'a mut VisualTestContext,
) {
    let config = config.to_string();
    let prefs = Prefs::from_config(ConfigStore::in_memory(&config).config());
    cx.update(|cx| {
        gpui_kit::init(cx);
        super::bind_keys(cx);
        crate::settings::init(ConfigStore::in_memory(&config), tokens, None, cx);
    });
    let fake = Arc::new(FakeBackend::with_timing(Timing::instant()).without_media_cache());
    let backend: Arc<dyn Backend> = fake.clone();
    let f2 = fake.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        MainView::new(
            backend,
            Some(f2),
            prefs,
            std::time::Instant::now(),
            window,
            cx,
        )
    });
    cx.run_until_parked();
    (view, fake, cx)
}

fn editor_text(view: &Entity<MainView>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |v, cx| v.editor.read(cx).value().to_string())
}

#[gpui_kit::test]
fn typing_filters_and_enter_opens(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    assert_eq!(view.read_with(cx, |v, _| v.list.results().len()), 6);

    cx.simulate_input("harb");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.list.results().len(), 1);
        assert_eq!(v.list.hint(), "1 found · ⏎ open");
        assert_eq!(v.mode, Mode::Search);
    });
    assert!(
        editor_text(&view, cx).starts_with("Band on"),
        "preview follows selection"
    );

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.mode), Mode::Edit);

    // Typing now goes to the editor (caret at the end) and autosaves.
    cx.simulate_input(" Nobody noticed.");
    cx.run_until_parked();
    let item = fake.item(&LocalId("01J9PX1".into())).unwrap();
    assert!(
        item.content_md.ends_with("better for it. Nobody noticed."),
        "{}",
        item.content_md
    );
    assert!(item.dirty, "editing a public post marks unpublished edits");

    // esc returns to the omnibar with the query selected.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.mode), Mode::Search);
    cx.simulate_input("lighthouse");
    cx.run_until_parked();
    assert!(editor_text(&view, cx).starts_with("A lighthouse"));
}

#[gpui_kit::test]
fn arrows_move_selection_and_preview(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    assert!(editor_text(&view, cx).starts_with("A lighthouse"));
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert!(editor_text(&view, cx).starts_with("Band on"));
    cx.simulate_keystrokes("down down");
    cx.run_until_parked();
    assert!(editor_text(&view, cx).starts_with("Notes on"));
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    assert!(editor_text(&view, cx).starts_with("On friction"));
}

#[gpui_kit::test]
fn enter_with_no_match_creates_seeded_draft(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_input("tide pools");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.list.wants_create()));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input(" are small oceans.");
    cx.run_until_parked();
    let items = fake.items();
    assert_eq!(items.len(), 7);
    assert_eq!(items[0].content_md, "tide pools are small oceans.");
    view.read_with(cx, |v, _| {
        assert_eq!(v.mode, Mode::Edit);
        assert_eq!(v.list.query(), "", "query cleared after create");
    });
}

#[gpui_kit::test]
fn escape_in_omnibar_clears_query(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_input("zzz");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.list.results().len()), 0);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    view.read_with(cx, |v, cx| {
        assert_eq!(v.list.results().len(), 6);
        assert_eq!(v.omni.read(cx).value().as_ref(), "");
    });
}

#[gpui_kit::test]
fn cmd_t_toggles_kind_and_over_limit_publish_shakes(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("enter"); // open "A lighthouse…"
    cx.run_until_parked();
    let long = "x".repeat(1000);
    cx.simulate_input(&long);
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(
            v.sheet.is_none(),
            "no publish sheet for an over-limit fragment"
        );
        assert_eq!(v.shake_gen, 1);
        assert!(v.toast.as_ref().is_some_and(|t| t.text.contains("⌘T")));
    });
    cx.simulate_keystrokes("cmd-t");
    cx.run_until_parked();
    assert_eq!(
        fake.item(&LocalId("01J9QK3".into())).unwrap().kind,
        Kind::Thread
    );
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| matches!(
        v.sheet,
        Some(Sheet::Publish {
            next_version: 1,
            ..
        })
    )));
}

#[gpui_kit::test]
fn publish_sheet_enter_publishes_with_note(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| matches!(v.sheet, Some(Sheet::Publish { .. }))));
    cx.simulate_input("first version");
    cx.simulate_keystrokes("enter");
    // publish runs on the background executor; let it finish.
    for _ in 0..50 {
        cx.run_until_parked();
        if fake.item(&LocalId("01J9QK3".into())).unwrap().status == Status::Public {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let item = fake.item(&LocalId("01J9QK3".into())).unwrap();
    assert_eq!((item.status, item.version), (Status::Public, 1));
    assert_eq!(
        fake.versions(&item.local_id)
            .unwrap()
            .last()
            .unwrap()
            .note
            .as_deref(),
        Some("first version")
    );
    assert!(view.read_with(cx, |v, _| v.sheet.is_none()));
}

#[gpui_kit::test]
fn publish_sheet_escape_cancels(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("enter cmd-enter");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.sheet.is_some()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.sheet.is_none()));
    assert_eq!(
        fake.item(&LocalId("01J9QK3".into())).unwrap().status,
        Status::Draft
    );
}

#[gpui_kit::test]
fn conflict_sheet_keys_resolve(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    fake.trigger_conflict(&LocalId("01J9K7T".into()));
    for _ in 0..50 {
        cx.run_until_parked();
        if view.read_with(cx, |v, _| v.sheet.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(view.read_with(cx, |v, _| matches!(v.sheet, Some(Sheet::Conflict { .. }))));
    cx.simulate_keystrokes("3");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.sheet.is_none()));
    assert_eq!(fake.items().len(), 7, "keep both makes a new draft");
    assert!(!fake.item(&LocalId("01J9K7T".into())).unwrap().conflict);
}

#[gpui_kit::test]
fn preview_toggles(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-e");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.studio.view.preview_visible()
        && v.studio.stats().is_some()));
    cx.simulate_keystrokes("cmd-e");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| !v.studio.view.preview_visible()));
}

#[gpui_kit::test]
fn pasting_an_image_uploads_and_replaces_placeholder(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("enter"); // open "A lighthouse…", caret at end
    cx.run_until_parked();
    let png = gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, vec![0x89, b'P', b'N', b'G']);
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(&png));
    cx.simulate_keystrokes("cmd-v");
    cx.run_until_parked();
    let id = LocalId("01J9QK3".into());
    for _ in 0..100 {
        cx.run_until_parked();
        if fake
            .item(&id)
            .unwrap()
            .content_md
            .contains("![](https://blyg.example.com/media/")
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let body = fake.item(&id).unwrap().content_md;
    assert!(
        !body.contains("uploading"),
        "placeholder never replaced: {body:?}"
    );
    assert!(
        body.starts_with(
            "A lighthouse keeper's log is mostly weather, and that is the point.\n\n![](https://blyg.example.com/media/"
        ),
        "{body:?}"
    );
    assert!(body.trim_end().ends_with(".png)"), "{body:?}");
    assert_eq!(editor_text(&view, cx), body, "editor and store agree");
}

fn config_text(cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| {
        crate::settings::get(cx)
            .store
            .text()
            .unwrap_or_default()
            .to_string()
    })
}

#[gpui_kit::test]
fn first_run_asks_to_connect_a_blyg(cx: &mut TestAppContext) {
    let tokens = Arc::new(MemoryTokenStore::default());
    let (view, _, cx) = setup_with(cx, "# fresh\n", tokens.clone());
    assert!(view.read_with(cx, |v, _| matches!(v.sheet, Some(Sheet::Connect { .. }))));
    assert_eq!(view.read_with(cx, |v, _| v.title.clone()), "Blygger");

    // A bad address is refused with a message; nothing is saved.
    cx.simulate_input("blyg.example.com");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("secret-token");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    view.read_with(cx, |v, _| match &v.sheet {
        Some(Sheet::Connect { error, .. }) => assert!(error.is_some()),
        _ => panic!("sheet closed on a bad URL"),
    });
    assert_eq!(tokens.get("https://blyg.example.com").unwrap(), None);

    // Fix the address: the token goes to the token store, the URL to the config.
    view.update_in(cx, |v, window, cx| {
        if let Some(Sheet::Connect { url, .. }) = &v.sheet {
            url.update(cx, |s, cx| {
                s.set_value("https://blyg.example.com/", window, cx)
            });
        }
        v.submit_connect(window, cx);
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.sheet.is_none()));
    assert_eq!(
        tokens.get("https://blyg.example.com").unwrap().as_deref(),
        Some("secret-token")
    );
    let text = config_text(cx);
    assert_eq!(text, "# fresh\n\nblyg-url = https://blyg.example.com\n");
    assert!(!text.contains("secret"), "no secrets in the config");
    assert_eq!(
        view.read_with(cx, |v, _| v.title.clone()),
        "Blygger — blyg.example.com"
    );
}

#[gpui_kit::test]
fn escape_postpones_connecting(cx: &mut TestAppContext) {
    let (view, _, cx) = setup_with(cx, "", Arc::new(MemoryTokenStore::default()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.sheet.is_none()));
}

#[gpui_kit::test]
fn settings_write_back_only_changed_keys(cx: &mut TestAppContext) {
    let (view, _, cx) = setup_with(
        cx,
        "# my settings\nblyg-url = https://blyg.example.com\n\n# type\nfont-size = 17\n",
        Arc::new(MemoryTokenStore::default()),
    );
    assert_eq!(view.read_with(cx, |v, _| v.prefs.font_size), 17.0);
    cx.simulate_keystrokes("cmd-=");
    cx.run_until_parked();
    view.update_in(cx, |v, window, cx| {
        v.prefs.layout = crate::prefs::LayoutPref::Stacked;
        v.apply_prefs(window, cx);
    });
    assert_eq!(
        config_text(cx),
        "# my settings\nblyg-url = https://blyg.example.com\n\n# type\nfont-size = 18\nlayout = stacked\n"
    );
}

#[gpui_kit::test]
fn reload_applies_the_file_live(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    // Someone edits the file (here: through the store, as an editor would).
    cx.update(|_, cx| {
        crate::settings::write(
            &[
                ("layout", Change::Set("stacked".into())),
                ("theme", Change::Set("dark".into())),
                ("font-family-writing", Change::Set("Charter".into())),
                ("fnot-size", Change::Set("3".into())),
            ],
            cx,
        )
        .unwrap();
    });
    assert_eq!(
        view.read_with(cx, |v, _| v.prefs.layout),
        crate::prefs::LayoutPref::Side,
        "nothing changes until a reload"
    );
    cx.simulate_keystrokes("cmd-shift-,");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.prefs.layout, crate::prefs::LayoutPref::Stacked);
        assert_eq!(v.prefs.theme, crate::prefs::ThemePref::Dark);
        assert!(v.palette.dark);
        assert_eq!(v.prefs.writing().family, "Charter");
        assert_eq!(v.config_problems.len(), 1, "{:?}", v.config_problems);
        assert_eq!(v.config_problems[0].line, 6);
        assert!(!v.problems_dismissed);
        assert!(
            v.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("1 problem"))
        );
    });
}

#[gpui_kit::test]
fn config_problems_show_until_dismissed(cx: &mut TestAppContext) {
    let (view, _, cx) = setup_with(
        cx,
        "blyg-url = https://blyg.example.com\ntheme = sepia\n",
        Arc::new(MemoryTokenStore::default()),
    );
    view.read_with(cx, |v, _| {
        assert_eq!(v.config_problems.len(), 1);
        assert!(v.config_problems[0].short().contains(":2: theme:"));
    });
    view.update(cx, |v, cx| {
        v.problems_dismissed = true;
        cx.notify();
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.problems_dismissed));
}

/// The action the keymap picks for `key` where focus is right now.
fn resolves_to(key: &str, cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|window, cx| {
        let stack = window.context_stack();
        let ks = gpui_kit::Keystroke::parse(key).unwrap();
        let km = cx.key_bindings();
        let km = km.borrow();
        km.bindings_for_input(&[ks], &stack)
            .0
            .first()
            .map(|b| b.action().name().to_string())
    })
}

fn editor_focused(view: &Entity<MainView>, cx: &mut VisualTestContext) -> bool {
    use gpui_kit::Focusable as _;
    let fh = view.read_with(cx, |v, cx| v.editor.read(cx).focus_handle(cx));
    cx.update(|window, _| fh.is_focused(window))
}

#[gpui_kit::test]
fn cmd_enter_in_the_editor_publishes(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("enter"); // open "A lighthouse…" in the editor
    cx.run_until_parked();
    assert!(editor_focused(&view, cx));
    // The keystroke itself resolves to Publish, not gpui-base's Input submit.
    assert_eq!(
        resolves_to("cmd-enter", cx).as_deref(),
        Some("blygger::Publish")
    );
    let before = editor_text(&view, cx);
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| matches!(v.sheet, Some(Sheet::Publish { .. }))));
    assert_eq!(editor_text(&view, cx), before, "no newline was typed");
    // ⌘⏎ in the note field publishes, like ⏎.
    cx.simulate_input("from the editor");
    cx.simulate_keystrokes("cmd-enter");
    for _ in 0..50 {
        cx.run_until_parked();
        if fake.item(&LocalId("01J9QK3".into())).unwrap().status == Status::Public {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        fake.item(&LocalId("01J9QK3".into())).unwrap().status,
        Status::Public
    );
    // From the omnibar too.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        resolves_to("cmd-enter", cx).as_deref(),
        Some("blygger::Publish")
    );
    // ⌘E stays the preview everywhere, never Publish.
    assert_eq!(
        resolves_to("cmd-e", cx).as_deref(),
        Some("blygger::TogglePreview")
    );
}

#[gpui_kit::test]
fn cmd_e_twice_returns_to_the_editor(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("enter");
    cx.simulate_input(" More.");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-e");
    cx.run_until_parked();
    // The preview opens beside the editor, which keeps the keyboard.
    assert!(view.read_with(cx, |v, _| v.studio.view.preview_visible()));
    assert!(editor_focused(&view, cx));
    assert_eq!(
        resolves_to("cmd-e", cx).as_deref(),
        Some("blygger::TogglePreview")
    );
    cx.simulate_keystrokes("cmd-e");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(!v.studio.view.preview_visible());
        assert_eq!(v.mode, Mode::Edit);
    });
    assert!(editor_focused(&view, cx), "focus is back in the editor");
    // …with the caret where it was.
    cx.simulate_input(" Again.");
    cx.run_until_parked();
    assert!(editor_text(&view, cx).ends_with(" More. Again."));
}

#[gpui_kit::test]
fn the_caret_blinks_and_holds_while_typing(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let editor = view.read_with(cx, |v, _| v.editor.clone());
    let ticks = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let t2 = ticks.clone();
    let _sub = cx.update(|_, cx| cx.observe(&editor, move |_, _| t2.set(t2.get() + 1)));
    for _ in 0..6 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
    }
    assert!(
        ticks.get() >= 4,
        "the caret repaints on its blink ({})",
        ticks.get()
    );
}

#[gpui_kit::test]
fn connecting_verifies_before_saving(cx: &mut TestAppContext) {
    use blyg_core::api::ConnectError;
    let tokens = Arc::new(MemoryTokenStore::default());
    let (view, fake, cx) = setup_with(cx, "# fresh\n", tokens.clone());
    let switch = crate::connection::SwitchBackend::new(fake, crate::connection::Mode::Fake);
    let set_verifier = |cx: &mut VisualTestContext, v: crate::connection::Verifier| {
        cx.update(|_, cx| {
            cx.set_global(crate::connection::Connection {
                switch: switch.clone(),
                data_dir: std::env::temp_dir(),
                verify: v,
            })
        });
    };
    let attempt = |cx: &mut VisualTestContext| {
        view.update_in(cx, |v, window, cx| {
            if let Some(Sheet::Connect { url, token, .. }) = &v.sheet {
                url.update(cx, |s, cx| {
                    s.set_value("https://blyg.example.com", window, cx)
                });
                token.update(cx, |s, cx| s.set_value("a-token", window, cx));
            }
            v.submit_connect(window, cx);
        });
        cx.run_until_parked();
    };
    let error = |cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| match &v.sheet {
            Some(Sheet::Connect { error, busy, .. }) => {
                assert!(!busy);
                error.clone()
            }
            _ => None,
        })
    };
    for (verdict, words) in [
        (
            (|_, _| Err(ConnectError::WrongToken)) as crate::connection::Verifier,
            "401",
        ),
        (
            |_, _| Err(ConnectError::MissingExtensions),
            "docs/SERVER.md",
        ),
        (|_, _| Err(ConnectError::Unreachable), "Couldn't reach"),
    ] {
        set_verifier(cx, verdict);
        attempt(cx);
        let e = error(cx).expect("an error is shown");
        assert!(e.contains(words), "{e}");
        assert_eq!(tokens.get("https://blyg.example.com").unwrap(), None);
        assert_eq!(config_text(cx), "# fresh\n", "nothing written");
    }
    set_verifier(cx, |_, _| Ok(3));
    attempt(cx);
    assert!(view.read_with(cx, |v, _| v.sheet.is_none()));
    assert_eq!(
        tokens.get("https://blyg.example.com").unwrap().as_deref(),
        Some("a-token")
    );
    assert!(config_text(cx).contains("blyg-url = https://blyg.example.com"));
}

#[gpui_kit::test]
fn disconnect_forgets_token_and_url(cx: &mut TestAppContext) {
    let tokens = Arc::new(MemoryTokenStore::default());
    tokens.set("https://blyg.example.com", "tok").unwrap();
    let (view, _, cx) = setup_with(cx, CONNECTED, tokens.clone());
    cx.dispatch_action(super::Disconnect);
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| matches!(v.sheet, Some(Sheet::Disconnect { .. }))));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.sheet.is_none()));
    assert!(
        tokens.get("https://blyg.example.com").unwrap().is_some(),
        "esc cancels"
    );
    cx.dispatch_action(super::Disconnect);
    cx.run_until_parked();
    cx.simulate_keystrokes("1");
    cx.run_until_parked();
    assert_eq!(tokens.get("https://blyg.example.com").unwrap(), None);
    assert!(!config_text(cx).contains("blyg-url"));
    // …and it offers to connect again.
    assert!(view.read_with(cx, |v, _| matches!(v.sheet, Some(Sheet::Connect { .. }))));
}
