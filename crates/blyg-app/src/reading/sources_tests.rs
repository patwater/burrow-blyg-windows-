//! --- reader folders --- Headless GPUI tests of the Reader's three panes:
//! sources (smart feeds, folders), the list, the post; real keystrokes and
//! actions on a zero-latency FakeBackend.

use std::sync::Arc;

use blyg_core::config::MemoryTokenStore;
use blyg_core::{Backend, ConfigStore};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

use super::sources::{MenuCmd, MenuTarget, NewFolder};
use super::sources_vm::{Pane, Smart, Source};
use super::{RSheet, View};
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
    view.update_in(cx, |v, window, cx| {
        v.set_read_mode(super::stream_vm::ReadMode::Reader, window, cx)
    });
    settle(cx);
    (view, fake, cx)
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn shown(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |v, _| v.reading_shown_ids())
}

fn subs_of(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |v, _| {
        v.reading
            .shown_rows()
            .map(|r| r.subscription_id.clone())
            .collect()
    })
}

fn pane(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Pane {
    view.read_with(cx, |v, _| v.reading.pane)
}

fn opened(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |v, _| {
        v.reading.opened.as_ref().map(|o| o.item.remote_id.clone())
    })
}

fn select(view: &Entity<MainView>, s: Source, cx: &mut VisualTestContext) {
    view.update(cx, |v, cx| v.select_source(s, cx));
    settle(cx);
}

fn menu(view: &Entity<MainView>, cmd: MenuCmd, cx: &mut VisualTestContext) {
    view.update_in(cx, |v, window, cx| v.run_menu_cmd(cmd, window, cx));
    settle(cx);
}

#[gpui_kit::test]
fn the_reader_opens_on_all_with_the_sources_pane(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.reading.view, View::Reading);
        assert_eq!(v.reading.source, Source::Smart(Smart::All));
        assert!(v.reading.sources_open);
        assert_eq!(v.reading.pane, Pane::List);
        assert_eq!(
            v.reading.shown.len(),
            v.reading.rows.len(),
            "All = every row"
        );
        let names: Vec<String> = v.reading.folders.iter().map(|f| f.name.clone()).collect();
        assert_eq!(names, ["Friends", "Gardens"]);
    });
}

#[gpui_kit::test]
fn selecting_a_source_filters_the_list(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    select(&view, Source::Folder(FOLDER_GARDENS.into()), cx);
    let subs = subs_of(&view, cx);
    assert!(!subs.is_empty());
    assert!(subs.iter().all(|s| s == "sub-lin"), "{subs:?}");

    select(&view, Source::Folder(FOLDER_FRIENDS.into()), cx);
    let subs = subs_of(&view, cx);
    assert!(subs.contains(&"sub-rue".to_string()) && subs.contains(&"sub-ada".to_string()));
    assert!(subs.iter().all(|s| s == "sub-rue" || s == "sub-ada"));

    select(&view, Source::Sub("sub-omar".into()), cx);
    assert_eq!(shown(&view, cx), [OMAR_YEAR]);
    select(&view, Source::Smart(Smart::Thumbed), cx);
    assert_eq!(shown(&view, cx), [RUE_KEPT]);

    // The search is scoped to the source.
    select(&view, Source::Folder(FOLDER_FRIENDS.into()), cx);
    view.update_in(cx, |v, window, cx| {
        v.set_reading_query("garden", window, cx)
    });
    assert!(
        shown(&view, cx).is_empty(),
        "Lin's gardens aren't in Friends"
    );
    select(&view, Source::Smart(Smart::All), cx);
    assert_eq!(shown(&view, cx), [LIN_GARDENS, OMAR_YEAR]);
    view.update_in(cx, |v, window, cx| v.set_reading_query("", window, cx));

    // "All unread": a post read there stays listed until the source changes.
    select(&view, Source::Smart(Smart::Unread), cx);
    let unread = shown(&view, cx);
    assert!(unread.contains(&RUE_TRUST.to_string()), "edited since read");
    assert!(!unread.contains(&ADA_TIDES.to_string()), "read");
    let first = unread[0].clone();
    view.update_in(cx, |v, window, cx| v.move_reading(1, window, cx));
    settle(cx);
    assert_eq!(opened(&view, cx).as_deref(), Some(first.as_str()));
    assert!(shown(&view, cx).contains(&first), "still listed once read");
    // Picking another source that lacks it closes it (nothing else opens).
    select(&view, Source::Sub("sub-omar".into()), cx);
    assert_eq!(opened(&view, cx), None);
    select(&view, Source::Smart(Smart::Unread), cx);
    assert!(
        !shown(&view, cx).contains(&first),
        "read now: {first} in {:?}",
        shown(&view, cx)
    );

    // The stream isn't filtered by the Reader's source.
    select(&view, Source::Sub("sub-omar".into()), cx);
    view.update_in(cx, |v, window, cx| {
        v.set_read_mode(super::stream_vm::ReadMode::Stream, window, cx)
    });
    settle(cx);
    assert!(shown(&view, cx).len() > 1);
}

#[gpui_kit::test]
fn today_lists_posts_dated_today(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    select(&view, Source::Smart(Smart::Today), cx);
    let now = chrono::Utc::now();
    view.read_with(cx, |v, _| {
        let rows: Vec<_> = v.reading.shown_rows().cloned().collect();
        for r in &rows {
            assert!(super::sources_vm::is_today(r, now), "{}", r.remote_id);
        }
        let expect = v
            .reading
            .rows
            .iter()
            .filter(|r| super::sources_vm::is_today(r, now))
            .count();
        assert_eq!(rows.len(), expect);
    });
}

#[gpui_kit::test]
fn moving_a_subscription_between_folders(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    // "Move to folder ›" on Omar's feed lists the folders, "No folder" checked.
    view.update(cx, |v, cx| {
        v.open_source_menu(
            MenuTarget::Sub("sub-omar".into()),
            gpui_kit::point(gpui_kit::px(10.), gpui_kit::px(10.)),
            false,
            cx,
        )
    });
    let labels = |view: &Entity<MainView>, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| {
            v.menu_items()
                .into_iter()
                .map(|(l, _, c)| if c { format!("✓{l}") } else { l })
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(labels(&view, cx)[0], "Move to folder ›");
    menu(&view, MenuCmd::MoveToFolder, cx);
    assert_eq!(
        labels(&view, cx),
        ["Friends", "Gardens", "✓No folder", "", "New Folder…"]
    );
    menu(
        &view,
        MenuCmd::File("sub-omar".into(), Some(FOLDER_GARDENS.into())),
        cx,
    );
    assert_eq!(
        fake.subscription_folders()
            .get("sub-omar")
            .map(String::as_str),
        Some(FOLDER_GARDENS)
    );
    view.read_with(cx, |v, _| assert!(v.reading.src_menu.is_none(), "closed"));
    select(&view, Source::Folder(FOLDER_GARDENS.into()), cx);
    assert!(subs_of(&view, cx).contains(&"sub-omar".to_string()));

    // Dropping it back under "Subscriptions" (no folder) is the same call.
    view.update(cx, |v, cx| v.file_subscription("sub-omar", None, cx));
    settle(cx);
    assert!(!fake.subscription_folders().contains_key("sub-omar"));
    assert!(!subs_of(&view, cx).contains(&"sub-omar".to_string()));
}

#[gpui_kit::test]
fn folders_are_made_renamed_reordered_and_deleted(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    // New Folder… (Blyg menu) opens a sheet; ⏎ makes it.
    cx.dispatch_action(NewFolder);
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| matches!(
        v.reading.sheet,
        Some(RSheet::Folder { .. })
    )));
    cx.simulate_input("Long reads");
    cx.simulate_keystrokes("enter");
    settle(cx);
    let names = |fake: &FakeBackend| {
        fake.folders()
            .into_iter()
            .map(|f| f.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&fake), ["Friends", "Gardens", "Long reads"]);
    assert!(view.read_with(cx, |v, _| v.reading.sheet.is_none()));

    // A duplicate name is refused in the sheet.
    view.update_in(cx, |v, window, cx| {
        v.open_folder_sheet(None, None, window, cx)
    });
    cx.simulate_input("friends");
    cx.simulate_keystrokes("enter");
    settle(cx);
    view.read_with(cx, |v, cx| match &v.reading.sheet {
        Some(RSheet::Folder { error: Some(e), .. }) => assert!(e.contains("already"), "{e}"),
        Some(RSheet::Folder {
            error: None, input, ..
        }) => {
            panic!("no error; typed {:?}", input.read(cx).value())
        }
        _ => panic!("the sheet stays with an error"),
    });
    cx.simulate_keystrokes("escape");
    settle(cx);

    // Rename…
    view.update_in(cx, |v, window, cx| {
        v.open_folder_sheet(Some(FOLDER_GARDENS.into()), None, window, cx)
    });
    cx.simulate_input("Allotments");
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert_eq!(names(&fake), ["Friends", "Allotments", "Long reads"]);

    // Move Up / Move Down.
    menu(&view, MenuCmd::MoveUp(FOLDER_GARDENS.into()), cx);
    assert_eq!(names(&fake), ["Allotments", "Friends", "Long reads"]);
    view.update(cx, |v, cx| v.reorder_folder(FOLDER_GARDENS, 2, cx));
    settle(cx);
    assert_eq!(names(&fake), ["Friends", "Long reads", "Allotments"]);

    // Delete: its subscriptions are unfiled, and a selected folder goes.
    select(&view, Source::Folder(FOLDER_FRIENDS.into()), cx);
    menu(&view, MenuCmd::Delete(FOLDER_FRIENDS.into()), cx);
    assert_eq!(names(&fake), ["Long reads", "Allotments"]);
    let filed = fake.subscription_folders();
    assert!(!filed.contains_key("sub-rue") && !filed.contains_key("sub-ada"));
    view.read_with(cx, |v, _| {
        assert_eq!(v.reading.source, Source::Smart(Smart::All));
        let unfiled: Vec<String> = v
            .source_entries()
            .into_iter()
            .filter_map(|e| match e {
                super::sources_vm::Entry::Sub {
                    sub, filed: false, ..
                } => Some(sub.id),
                _ => None,
            })
            .collect();
        assert!(unfiled.contains(&"sub-rue".to_string()), "{unfiled:?}");
    });

    // "Move to folder › New Folder…" makes a folder and files it there.
    view.update_in(cx, |v, window, cx| {
        v.open_folder_sheet(None, Some("sub-omar".into()), window, cx)
    });
    cx.simulate_input("Work");
    cx.simulate_keystrokes("enter");
    settle(cx);
    let work = fake
        .folders()
        .into_iter()
        .find(|f| f.name == "Work")
        .unwrap();
    assert_eq!(fake.subscription_folders().get("sub-omar"), Some(&work.id));
}

#[gpui_kit::test]
fn arrows_move_focus_between_the_panes(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    assert_eq!(pane(&view, cx), Pane::List);
    // ← to the sources; ↓ picks the next source (the first folder).
    cx.simulate_keystrokes("left");
    settle(cx);
    assert_eq!(pane(&view, cx), Pane::Sources);
    cx.simulate_keystrokes("left");
    assert_eq!(pane(&view, cx), Pane::Sources, "stops at the edge");
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.reading.source.clone()),
        Source::Folder(FOLDER_FRIENDS.into())
    );
    assert!(
        subs_of(&view, cx)
            .iter()
            .all(|s| s == "sub-rue" || s == "sub-ada")
    );
    assert_eq!(opened(&view, cx), None, "picking a source opens nothing");
    // j/k move through the posts from any pane.
    cx.simulate_keystrokes("j");
    settle(cx);
    let first = shown(&view, cx)[0].clone();
    assert_eq!(opened(&view, cx).as_deref(), Some(first.as_str()));
    assert_eq!(pane(&view, cx), Pane::Sources, "the keys stay put");
    cx.simulate_keystrokes("up");
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.reading.source.clone()),
        Source::Smart(Smart::All)
    );
    // → to the list, → to the post; ← back; esc from the post to the list.
    cx.simulate_keystrokes("right");
    settle(cx);
    assert_eq!(pane(&view, cx), Pane::List);
    cx.simulate_keystrokes("right");
    settle(cx);
    assert_eq!(pane(&view, cx), Pane::Post);
    cx.simulate_keystrokes("left");
    assert_eq!(pane(&view, cx), Pane::List);
    cx.simulate_keystrokes("right");
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert_eq!(pane(&view, cx), Pane::List);
    assert_eq!(
        view.read_with(cx, |v, _| v.reading.view),
        View::Reading,
        "esc from the post only moves the keys"
    );
    // ↓ in the list moves the posts, as before.
    let before = opened(&view, cx);
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_ne!(opened(&view, cx), before);
}

#[gpui_kit::test]
fn right_from_the_sources_opens_the_first_post(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    select(&view, Source::Sub("sub-omar".into()), cx);
    view.update(cx, |v, cx| v.set_pane(Pane::Sources, cx));
    cx.simulate_keystrokes("right");
    settle(cx);
    assert_eq!(pane(&view, cx), Pane::List);
    assert_eq!(opened(&view, cx).as_deref(), Some(OMAR_YEAR));
}

#[gpui_kit::test]
fn space_goes_on_to_the_next_unread_post(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    // Headless there's no page to scroll: Space goes straight on.
    cx.simulate_keystrokes("space");
    settle(cx);
    let first = opened(&view, cx).expect("the first post to read");
    let row = |fake: &FakeBackend, id: &str| {
        fake.reading()
            .into_iter()
            .find(|r| r.remote_id == id)
            .unwrap()
    };
    assert!(row(&fake, &first).read_version.is_some(), "opened → read");
    cx.simulate_keystrokes("space");
    settle(cx);
    let second = opened(&view, cx).unwrap();
    assert_ne!(first, second);
    view.read_with(cx, |v, _| {
        let pos = |id: &str| {
            v.reading
                .shown_rows()
                .position(|r| r.remote_id == id)
                .unwrap()
        };
        assert!(pos(&second) > pos(&first), "forward, skipping read posts");
        // Everything between was already read.
        for r in v
            .reading
            .shown_rows()
            .skip(pos(&first) + 1)
            .take(pos(&second) - pos(&first) - 1)
        {
            assert!(!super::sources_vm::needs_reading(r), "{}", r.remote_id);
        }
    });
    // The page's "end" (Space at the bottom of the page) does the same.
    view.update_in(cx, |v, window, cx| v.open_next_unread(window, cx));
    settle(cx);
    assert_ne!(opened(&view, cx).as_deref(), Some(second.as_str()));
}

#[gpui_kit::test]
fn brackets_step_versions_and_the_pane_hides(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    let key = view.read_with(cx, |v, _| {
        v.reading_rows()
            .iter()
            .find(|r| r.remote_id == RUE_TRUST)
            .map(super::vm::key)
            .unwrap()
    });
    view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
    settle(cx);
    let label = |view: &Entity<MainView>, cx: &mut VisualTestContext| {
        view.read_with(cx, |v, _| v.pill_model().unwrap().label)
    };
    assert_eq!(label(&view, cx), "v5 · current ▾");
    cx.simulate_keystrokes("[");
    settle(cx);
    assert!(
        label(&view, cx).starts_with("📌 v3"),
        "{}",
        label(&view, cx)
    );
    cx.simulate_keystrokes("]");
    settle(cx);
    assert_eq!(label(&view, cx), "v5 · current ▾");

    // ⌥⌘S hides the sources pane; ← then stays in the list.
    cx.simulate_keystrokes(&crate::keymap::keys("alt-cmd-s"));
    settle(cx);
    assert!(!view.read_with(cx, |v, _| v.reading.sources_open));
    cx.simulate_keystrokes("left");
    assert_eq!(pane(&view, cx), Pane::List);
    cx.simulate_keystrokes(&crate::keymap::keys("alt-cmd-s"));
    settle(cx);
    assert!(view.read_with(cx, |v, _| v.reading.sources_open));
}

#[gpui_kit::test]
fn the_subscriptions_screen_chooses_a_folder(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-s"));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |v, _| v.reading.view),
        View::Subscriptions
    );
    view.update(cx, |v, cx| {
        v.open_source_menu(
            MenuTarget::Sub("sub-rue".into()),
            gpui_kit::point(gpui_kit::px(10.), gpui_kit::px(10.)),
            true,
            cx,
        )
    });
    let checked = view.read_with(cx, |v, _| {
        v.menu_items()
            .into_iter()
            .filter(|(_, _, c)| *c)
            .map(|(l, _, _)| l)
            .collect::<Vec<_>>()
    });
    assert_eq!(checked, ["Friends"]);
    // esc closes the menu before it leaves the screen.
    cx.simulate_keystrokes("escape");
    settle(cx);
    view.read_with(cx, |v, _| {
        assert!(v.reading.src_menu.is_none());
        assert_eq!(v.reading.view, View::Subscriptions);
    });
    menu(&view, MenuCmd::File("sub-rue".into(), None), cx);
    assert!(!fake.subscription_folders().contains_key("sub-rue"));
}
