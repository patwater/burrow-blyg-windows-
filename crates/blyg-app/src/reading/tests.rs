//! Headless GPUI tests of the reading & management screens and the version
//! UI, through real keystrokes and actions on a zero-latency FakeBackend.

use std::sync::Arc;

use blyg_core::config::MemoryTokenStore;
use blyg_core::{Backend, ConfigStore, Kind, LocalId};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

use super::list::EditedBlock;
use super::{Load, RSheet, View};
use crate::app::MainView;
use crate::fake::reading_seed::*;
use crate::fake::{FakeBackend, Timing};
use crate::prefs::Prefs;

const CONNECTED: &str = "# test config\nblyg-url = https://blyg.example.com\n";

fn setup(cx: &mut TestAppContext) -> (Entity<MainView>, Arc<FakeBackend>, &mut VisualTestContext) {
    let prefs = Prefs::from_config(ConfigStore::in_memory(CONNECTED).config());
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::app::bind_keys(cx);
        crate::settings::init(
            ConfigStore::in_memory(CONNECTED),
            Arc::new(MemoryTokenStore::default()),
            None,
            cx,
        );
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

/// Let background tasks (zero latency) land.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn open_row(view: &Entity<MainView>, id: &str, cx: &mut VisualTestContext) {
    let key = view.read_with(cx, |v, _| {
        v.reading_rows()
            .iter()
            .find(|r| r.remote_id == id)
            .map(super::vm::key)
            .expect("row shown")
    });
    view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
    settle(cx);
}

fn open_post(view: &Entity<MainView>, id: &str, cx: &mut VisualTestContext) {
    view.update_in(cx, |v, window, cx| v.open(&LocalId(id.into()), window, cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn cmd_r_shows_reading_edited_on_top_tombstones_hidden(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert_eq!(v.reading.view, View::Reading);
        let ids: Vec<&str> = v
            .reading_rows()
            .iter()
            .map(|r| r.remote_id.as_str())
            .collect();
        // Edited since read, newest first, on top; then the rest in order.
        assert_eq!(&ids[..2], &[RUE_TRUST, LIN_GARDENS], "{ids:?}");
        assert!(
            !ids.contains(&ADA_GONE),
            "an unsignalled tombstone is hidden"
        );
        assert!(ids.contains(&RUE_KEPT), "a thumbed tombstone stays");
        assert_eq!(
            super::vm::badge(&v.reading_rows()[0]).as_deref(),
            Some("edited · v5")
        );
    });
    // Again: back to posts.
    cx.simulate_keystrokes("cmd-r");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Posts);
    // ⌘⇧M / ⌘⇧S switch too, and esc returns.
    cx.simulate_keystrokes("cmd-shift-s");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.reading.view),
        View::Subscriptions
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Posts);
}

#[gpui_kit::test]
fn opening_marks_read_and_arrows_move(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    settle(cx);
    let read = |fake: &FakeBackend, id: &str| {
        fake.reading()
            .into_iter()
            .find(|r| r.remote_id == id)
            .unwrap()
            .read_version
    };
    assert_eq!(read(&fake, RUE_TRUST), Some(5), "opened → read");
    view.read_with(cx, |v, _| {
        let o = v.reading.opened.as_ref().unwrap();
        assert_eq!(o.item.remote_id, RUE_TRUST);
        assert_eq!(
            o.item.read_version,
            Some(3),
            "the open view keeps what you read before"
        );
        // The row stays put (order is stable while the screen is open).
        assert_eq!(v.reading_rows()[0].remote_id, RUE_TRUST);
    });
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(read(&fake, LIN_GARDENS), Some(4));
    assert_eq!(read(&fake, ADA_FINISHED), None, "not opened yet");
}

#[gpui_kit::test]
fn others_posts_show_only_current_and_pinned_versions(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    open_row(&view, RUE_TRUST, cx);
    let strings = |view: &Entity<MainView>, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            super::vm::pill_strings(&v.pill_model().expect("pill"))
        })
    };
    // Rue's changelog has v1 📌, v2, v3 📌, v4, v5 (current).
    let pm = view.read_with(cx, |v, _| v.pill_model().unwrap());
    assert_eq!(pm.label, "v5 · current ▾");
    let listed: Vec<&str> = pm.entries.iter().map(|(l, _, _)| l.as_str()).collect();
    assert_eq!(listed, ["v5 · current", "📌 v3", "📌 v1"]);
    assert_eq!(
        pm.actions,
        ["Quote", "Reply", "AI reply", "Fork", "Open on web"]
    );
    // Step through every version the pill allows; unpinned never render.
    for _ in 0..4 {
        for s in strings(&view, cx) {
            assert!(
                !s.contains("v2") && !s.contains("v4"),
                "unpinned leaked: {s}"
            );
        }
        cx.simulate_keystrokes("left");
        settle(cx);
    }
    view.read_with(cx, |v, _| {
        let pm = v.pill_model().unwrap();
        assert!(pm.label.starts_with("📌 v1"), "{}", pm.label);
        assert!(!pm.can_older);
        assert_eq!(
            pm.actions,
            [
                "Quote this version",
                "Fork this pin",
                "Diff vs now",
                "Back to current"
            ]
        );
        let o = v.reading.opened.as_ref().unwrap();
        assert!(matches!(o.pins.get(&1), Some(Load::Ready(p)) if p.content_md.contains("litle")));
        assert!(
            o.pins.keys().all(|v| [1, 3].contains(v)),
            "only pins are fetched"
        );
    });
    // ⌘Y opens the dropdown here; picking the current version goes back.
    cx.simulate_keystrokes("cmd-y");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.reading.opened.as_ref().unwrap().dropdown));
    view.update_in(cx, |v, window, cx| {
        v.reading_action_for_test("Back to current", window, cx)
    });
    assert_eq!(
        view.read_with(cx, |v, _| v.pill_model().unwrap().label),
        "v5 · current ▾"
    );
}

#[gpui_kit::test]
fn diff_only_when_the_read_version_was_pinned(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    // Read at v3, which is pinned → a real diff.
    open_row(&view, RUE_TRUST, cx);
    match view.read_with(cx, |v, _| v.edited_block()) {
        Some(EditedBlock::Diff { heading, runs }) => {
            assert!(heading.contains("📌 v3"), "{heading}");
            assert!(runs.iter().any(|(op, _)| *op == crate::vm::DiffOp::Insert));
        }
        other => panic!("expected a diff, got {other:?}"),
    }
    // Read at v2, not pinned → notes only, never a diff.
    open_row(&view, LIN_GARDENS, cx);
    match view.read_with(cx, |v, _| v.edited_block()) {
        Some(EditedBlock::Notes { notes, .. }) => {
            assert_eq!(
                notes,
                ["v3 “new section on pruning”", "v4 “linked the reply”"]
            );
        }
        other => panic!("expected notes only, got {other:?}"),
    }
    // Not edited since read → nothing.
    open_row(&view, OMAR_YEAR, cx);
    assert_eq!(view.read_with(cx, |v, _| v.edited_block()), None);
}

#[gpui_kit::test]
fn pinning_needs_the_typed_word(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    open_post(&view, "01J9M2A", cx);
    cx.simulate_keystrokes("cmd-y");
    settle(cx);
    view.read_with(cx, |v, _| {
        let own = v.reading.own.as_ref().expect("history open");
        assert_eq!(own.list.ready().map(|l| l.len()), Some(3), "every version");
        assert_eq!(own.sel, Some(3));
    });
    view.update_in(cx, |v, window, cx| v.ask_pin(window, cx));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| matches!(
        v.reading.sheet,
        Some(RSheet::Pin { version: 3, .. })
    )));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input("pi");
    cx.simulate_keystrokes("enter");
    settle(cx);
    view.read_with(cx, |v, _| match &v.reading.sheet {
        Some(RSheet::Pin { error, .. }) => assert!(error.as_deref().unwrap().contains("pin")),
        _ => panic!("the sheet stays until the word is typed"),
    });
    let pinned = |fake: &FakeBackend| {
        fake.versions(&LocalId("01J9M2A".into()))
            .unwrap()
            .iter()
            .find(|v| v.version == 3)
            .unwrap()
            .pinned
    };
    assert!(!pinned(&fake));
    cx.simulate_input("n");
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(pinned(&fake), "pinned once “pin” was typed");
    assert!(view.read_with(cx, |v, _| v.reading.sheet.is_none()));
}

#[gpui_kit::test]
fn restore_loads_a_version_into_the_editor(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    open_post(&view, "01J9M2A", cx);
    cx.simulate_keystrokes("cmd-y");
    settle(cx);
    view.update_in(cx, |v, window, cx| {
        v.reading.own.as_mut().unwrap().sel = Some(1);
        v.ask_restore(window, cx);
    });
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| matches!(
        v.reading.sheet,
        Some(RSheet::Restore {
            version: 1,
            next: 4,
            ..
        })
    )));
    cx.simulate_keystrokes("enter");
    settle(cx);
    let text = view.read_with(cx, |v, cx| v.editor.read(cx).value().to_string());
    assert_eq!(text, "Friction kills posts.");
    let it = fake.item(&LocalId("01J9M2A".into())).unwrap();
    assert_eq!(it.version, 3, "restoring never publishes");
    assert!(view.read_with(cx, |v, _| v.reading.own.is_none()));
}

#[gpui_kit::test]
fn mentions_are_a_list_without_counts(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-shift-m");
    settle(cx);
    let strings = view.read_with(cx, |v, _| {
        assert_eq!(v.reading.view, View::Mentions);
        v.mentions_screen_strings()
    });
    // Timestamps ("2h", "Sep 20") aside, nothing on the screen is a number.
    let times: Vec<String> = view.read_with(cx, |v, _| {
        v.mention_groups()
            .into_iter()
            .flat_map(|(_, _, _, rows)| rows.into_iter().map(|r| r.when))
            .collect()
    });
    for s in &strings {
        if times.contains(s) {
            continue;
        }
        assert!(
            !s.chars().any(|c| c.is_ascii_digit()),
            "a count-like number on the mentions screen: {s:?}"
        );
    }
    for v in View::ALL {
        assert!(!v.label().chars().any(|c| c.is_ascii_digit()));
    }
    // Who, relation; hidden ones are left out; the newest is new (a dot).
    assert!(strings.contains(&"Rue".to_string()) && strings.contains(&"replied".to_string()));
    assert!(
        !strings.contains(&"Omar".to_string()),
        "hidden mention listed"
    );
    view.read_with(cx, |v, _| {
        let groups = v.mention_groups();
        let rue = groups
            .iter()
            .flat_map(|g| g.3.iter())
            .find(|r| r.who == "Rue")
            .unwrap();
        assert!(rue.new);
        assert!(groups.iter().any(|g| g.0.starts_with("On friction")));
    });
    // Leaving the screen clears "new".
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!view.read_with(cx, |v, cx| {
        let _ = cx;
        v.reading.has_new_mentions(v.now)
    }));
}

#[gpui_kit::test]
fn quote_picker_offers_only_held_items_and_only_in_threads(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    // A fragment can't take a quote.
    open_post(&view, "01J9QK3", cx);
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.reading.sheet.is_none());
        assert!(v.toast.as_ref().is_some_and(|t| t.text.contains("threads")));
    });
    // A draft thread can.
    open_post(&view, "01J9H4C", cx);
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    let ids: Vec<String> = view.read_with(cx, |v, cx| {
        assert!(matches!(v.reading.sheet, Some(RSheet::Quote { .. })));
        v.quote_candidates(cx).into_iter().map(|q| q.id).collect()
    });
    for own in ["01J9PX1", "01J9M2A", "01J9K7T", "01J9E8B"] {
        assert!(
            ids.iter().any(|i| i.starts_with(own)),
            "own public post {own} offered"
        );
    }
    assert!(
        !ids.iter().any(|i| i.starts_with("01J9QK3")),
        "drafts aren't quotable"
    );
    for held in [RUE_TRUST, ADA_FINISHED, LIN_GARDENS, ADA_TIDES] {
        assert!(ids.contains(&held.to_string()), "{held} offered");
    }
    assert!(
        !ids.contains(&OMAR_YEAR.to_string()),
        "RSS items aren't transcludable"
    );
    assert!(!ids.contains(&RUE_KEPT.to_string()) && !ids.contains(&ADA_GONE.to_string()));
    // Filter, then ⏎ inserts ![[id]] on its own line.
    cx.simulate_input("hyperlink");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let text = view.read_with(cx, |v, cx| v.editor.read(cx).value().to_string());
    assert!(
        text.lines().any(|l| l == format!("![[{RUE_TRUST}]]")),
        "{text}"
    );
}

#[gpui_kit::test]
fn reply_makes_a_stub_and_fork_needs_a_pin(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    open_row(&view, RUE_TRUST, cx);
    view.update_in(cx, |v, window, cx| {
        v.reading_action_for_test("Reply", window, cx)
    });
    settle(cx);
    let stub = fake
        .items()
        .into_iter()
        .find(|i| i.stub_of.is_some())
        .expect("a stub");
    assert_eq!(stub.kind, Kind::Thread);
    assert_eq!(stub.stub_of.as_ref().unwrap().id, RUE_TRUST);
    assert!(stub.content_md.starts_with(&format!("![[{RUE_TRUST}]]")));
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Posts);

    // Fork is offered on pins only.
    cx.simulate_keystrokes("cmd-r");
    open_row(&view, RUE_TRUST, cx);
    assert!(!view.read_with(cx, |v, _| {
        v.pill_model().unwrap().actions.contains(&"Fork this pin")
    }));
    cx.simulate_keystrokes("left");
    settle(cx);
    view.update_in(cx, |v, window, cx| {
        v.reading_action_for_test("Fork this pin", window, cx)
    });
    settle(cx);
    let fork = fake
        .items()
        .into_iter()
        .find(|i| i.forked_from.is_some())
        .expect("a fork");
    assert_eq!(fork.forked_from.as_ref().unwrap().version, 3);
    assert_eq!(fork.content_md, "A hyperlink is a small unit of trust.");
}

#[gpui_kit::test]
fn subscribe_previews_first(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    let before = fake.subscriptions().len();
    cx.simulate_keystrokes("cmd-shift-s");
    cx.dispatch_action(super::SubscribeTo);
    cx.run_until_parked();
    cx.simulate_input("https://kit.blyg.example.com/");
    cx.simulate_keystrokes("enter");
    settle(cx);
    view.read_with(cx, |v, _| match &v.reading.sheet {
        Some(RSheet::Subscribe {
            previewed: Some((_, p)),
            ..
        }) => assert_eq!(p.title, "Kit"),
        _ => panic!("preview first"),
    });
    assert_eq!(fake.subscriptions().len(), before, "nothing subscribed yet");
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert_eq!(fake.subscriptions().len(), before + 1);
    // Pause, blogroll and unsubscribe go through the backend.
    fake.pause_subscription("sub-rue", true).unwrap();
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| {
            v.reading
                .subs
                .iter()
                .find(|s| s.id == "sub-rue")
                .unwrap()
                .status
                .clone()
        }),
        "paused"
    );
}

#[gpui_kit::test]
fn site_settings_load_and_save(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.dispatch_action(super::SiteSettings);
    settle(cx);
    view.update_in(cx, |v, window, cx| {
        let Some(RSheet::Site {
            title, links, load, ..
        }) = &v.reading.sheet
        else {
            panic!("sheet");
        };
        assert!(matches!(load, Load::Ready(_)));
        assert_eq!(title.read(cx).value().as_ref(), "Harbour notes");
        title.update(cx, |s, cx| s.set_value("Harbour notebook", window, cx));
        links.update(cx, |s, cx| {
            s.set_value(
                "Feed | https://blyg.example.com/feed.json\nHome | https://example.com",
                window,
                cx,
            )
        });
        v.save_site_settings(window, cx);
    });
    settle(cx);
    let s = fake.saved_settings();
    assert_eq!(s.site_title.as_deref(), Some("Harbour notebook"));
    assert_eq!(s.author_links.len(), 2);
    assert_eq!(
        s.author_bio.as_deref(),
        Some("Small notes from a harbour town.")
    );
    assert!(view.read_with(cx, |v, _| v.reading.sheet.is_none()));
}

#[gpui_kit::test]
fn without_read_extensions_screens_say_not_available(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    fake.set_read_extensions(false);
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-r");
    cx.run_until_parked();
    assert!(!view.read_with(cx, |v, _| v.reading.available));
    cx.simulate_keystrokes("cmd-shift-m");
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.reading.mentions.clone()),
        Load::Unavailable
    );
    cx.dispatch_action(super::SiteSettings);
    settle(cx);
    assert!(view.read_with(cx, |v, _| matches!(
        v.reading.sheet,
        Some(RSheet::Site {
            load: Load::Unavailable,
            ..
        })
    )));
    // The window still draws every state.
    cx.run_until_parked();
}

// ---------------------------------------------------------------- search (#6)

fn query(view: &Entity<MainView>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |v, _| v.reading.query.clone())
}

fn shown(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |v, _| v.reading_shown_ids())
}

fn search_focused(view: &Entity<MainView>, cx: &mut VisualTestContext) -> bool {
    view.update_in(cx, |v, window, cx| v.reading_search_focused(window, cx))
}

fn opened(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |v, _| {
        v.reading.opened.as_ref().map(|o| o.item.remote_id.clone())
    })
}

#[gpui_kit::test]
fn cmd_f_searches_titles_authors_and_text_held_locally(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    cx.run_until_parked();
    assert!(shown(&view, cx).len() > 3);
    cx.simulate_keystrokes("cmd-f");
    cx.run_until_parked();
    assert!(search_focused(&view, cx), "⌘F focuses the search field");

    // A title (Lin's "Gardens, not streams") and body text (Omar's "a
    // garden that finally got…"), in list order, whatever the case.
    cx.simulate_input("GARDEN");
    cx.run_until_parked();
    assert_eq!(query(&view, cx), "GARDEN");
    assert_eq!(shown(&view, cx), [LIN_GARDENS, OMAR_YEAR]);
    // Nothing was opened (so nothing marked read) just by typing.
    assert_eq!(opened(&view, cx), None);

    // ↓ from the field opens the first match; ↑/↓ stay in the matches.
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(opened(&view, cx).as_deref(), Some(LIN_GARDENS));
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(opened(&view, cx).as_deref(), Some(OMAR_YEAR));
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(
        opened(&view, cx).as_deref(),
        Some(OMAR_YEAR),
        "stops at the end"
    );
    assert!(search_focused(&view, cx), "the caret stays in the field");

    // An author / subscription name.
    view.update_in(cx, |v, window, cx| {
        v.set_reading_query("omar's", window, cx)
    });
    assert_eq!(shown(&view, cx), [OMAR_YEAR]);
    // The origin's host.
    view.update_in(cx, |v, window, cx| {
        v.set_reading_query("rue.blyg.example", window, cx)
    });
    let rue = shown(&view, cx);
    assert!(rue.contains(&RUE_TRUST.to_string()), "{rue:?}");
    assert!(
        rue.iter().all(|id| id == RUE_TRUST || id == RUE_KEPT),
        "{rue:?}"
    );
    // Held rows only: the hidden tombstone never turns up.
    view.update_in(cx, |v, window, cx| v.set_reading_query("ada", window, cx));
    assert!(!shown(&view, cx).contains(&ADA_GONE.to_string()));
    view.update_in(cx, |v, window, cx| {
        v.set_reading_query("tide tables", window, cx)
    });
    assert_eq!(shown(&view, cx), [ADA_TIDES]);
}

#[gpui_kit::test]
fn slash_focuses_search_and_esc_clears_it(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    cx.run_until_parked();
    let all = shown(&view, cx);
    cx.simulate_keystrokes("/");
    cx.run_until_parked();
    assert!(search_focused(&view, cx), "/ focuses the search field");
    cx.simulate_input("trust");
    cx.run_until_parked();
    assert_eq!(shown(&view, cx), [RUE_TRUST]);
    // esc in the field clears the search (and stays on Reading).
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(query(&view, cx), "");
    assert_eq!(shown(&view, cx), all);
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Reading);
    // esc on an empty field hands the keys back to the list…
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!search_focused(&view, cx));
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Reading);
    // …where j moves again (in the field it would be text).
    cx.simulate_keystrokes("j");
    settle(cx);
    assert!(opened(&view, cx).is_some());

    // From the list, esc clears a search before it leaves the screen.
    view.update_in(cx, |v, window, cx| v.set_reading_query("trust", window, cx));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(query(&view, cx), "");
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Reading);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Posts);
}

#[gpui_kit::test]
fn search_keeps_a_matching_post_open_and_says_when_nothing_matches(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    open_row(&view, RUE_TRUST, cx);
    // Still a match: the open post and the selection stay.
    view.update_in(cx, |v, window, cx| {
        v.set_reading_query("unit of trust", window, cx)
    });
    assert_eq!(opened(&view, cx).as_deref(), Some(RUE_TRUST));
    assert!(view.read_with(cx, |v, _| v.reading.sel.is_some()));
    // No longer a match: the reader and the selection clear.
    view.update_in(cx, |v, window, cx| {
        v.set_reading_query("garden", window, cx)
    });
    assert_eq!(opened(&view, cx), None);
    assert!(view.read_with(cx, |v, _| v.reading.sel.is_none()));
    // Nothing matches: an empty list and the empty state.
    view.update_in(cx, |v, window, cx| {
        v.set_reading_query("  Zeppelin ", window, cx)
    });
    cx.run_until_parked();
    assert!(shown(&view, cx).is_empty());
    assert_eq!(
        super::vm::no_match(&query(&view, cx)),
        "No posts match “Zeppelin”"
    );
    // ↓ does nothing on an empty result.
    view.update_in(cx, |v, window, cx| v.move_reading(1, window, cx));
    assert_eq!(opened(&view, cx), None);
    // The search stays when you leave and come back (it's in the field).
    cx.simulate_keystrokes("cmd-r");
    cx.simulate_keystrokes("cmd-r");
    cx.run_until_parked();
    assert_eq!(query(&view, cx), "  Zeppelin ");
    assert!(shown(&view, cx).is_empty());
}

// ---------------------------------------------------------------- actions (#3)

fn chips(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Vec<super::vm::ActionChip> {
    view.read_with(cx, |v, _| v.reading_action_chips())
}

fn tip(row: &[super::vm::ActionChip], id: &str) -> String {
    row.iter().find(|c| c.id == id).expect(id).tip.clone()
}

#[gpui_kit::test]
fn actions_name_the_primitive_and_fork_waits_for_a_pin(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    // Rue's post has 📌 v1 and 📌 v3; the current version is v5.
    open_row(&view, RUE_TRUST, cx);
    let row = chips(&view, cx);
    let labels: Vec<&str> = row.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Quote into a thread",
            "Reply · new stub",
            "AI reply · new stub",
            "Fork",
            "Open on web"
        ]
    );
    for c in &row {
        assert!(!c.tip.is_empty(), "{} has a tooltip", c.label);
        assert_eq!(c.enabled, c.id != "Fork", "{}", c.label);
    }
    assert!(tip(&row, "Reply").contains("stub_of"));
    assert!(tip(&row, "Quote").contains("![[…]]"));
    let fork = tip(&row, "Fork");
    assert!(
        fork.contains("📌 v3") && fork.contains("‹ v5 ▾ ›"),
        "points to the newest pin: {fork}"
    );
    assert!(!fork.contains("v2") && !fork.contains("v4"), "{fork}");
    // The greyed Fork makes nothing.
    view.update_in(cx, |v, window, cx| {
        v.reading_action_for_test("Fork", window, cx)
    });
    settle(cx);
    assert!(fake.items().iter().all(|i| i.forked_from.is_none()));

    // On a pin, Fork this pin is live and says what it makes.
    cx.simulate_keystrokes("left");
    settle(cx);
    let row = chips(&view, cx);
    assert!(row.iter().all(|c| c.enabled));
    let f = tip(&row, "Fork this pin");
    assert!(f.contains("📌 v3") && f.contains("forked_from"), "{f}");

    // A post with no pins: Fork says so.
    open_row(&view, ADA_FINISHED, cx);
    assert!(tip(&chips(&view, cx), "Fork").contains("has none"));
    // A feed post can't be transcluded: Quote says what it does instead.
    open_row(&view, OMAR_YEAR, cx);
    assert!(tip(&chips(&view, cx), "Quote").contains("feed posts"));
}
