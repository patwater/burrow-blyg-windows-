//! Headless GPUI tests of the reading stream (issue #1): the default mode,
//! j/k and ⏎/esc, the side pane, read-on-view, search and the toggle.

use std::sync::Arc;
use std::time::{Duration, Instant};

use blyg_core::config::MemoryTokenStore;
use blyg_core::{Backend, ConfigStore};
use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext};

use super::View;
use super::stream_vm::ReadMode;
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn read_version(fake: &FakeBackend, id: &str) -> Option<u32> {
    fake.reading()
        .into_iter()
        .find(|r| r.remote_id == id)
        .and_then(|r| r.read_version)
}

fn selected(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |v, _| v.reading.sel.as_ref().map(|k| k.1.clone()))
}

fn opened(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<String> {
    view.read_with(cx, |v, _| {
        v.reading.opened.as_ref().map(|o| o.item.remote_id.clone())
    })
}

#[gpui_kit::test]
fn the_stream_is_the_default_and_jk_select_without_opening(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.reading.view, View::Reading);
        assert_eq!(v.reading.mode, ReadMode::Stream);
        assert_eq!(v.reading.stream.list.item_count(), v.reading.shown.len());
    });
    assert!(cx.debug_bounds("mode-stream").is_some(), "the toggle");
    cx.simulate_keystrokes("j");
    settle(cx);
    assert_eq!(selected(&view, cx).as_deref(), Some(RUE_TRUST));
    assert_eq!(opened(&view, cx), None, "selecting doesn't open");
    assert_eq!(read_version(&fake, RUE_TRUST), Some(3), "nor mark read");
    // The selected post shows its actions.
    assert!(cx.debug_bounds("stream-actions").is_some());
    cx.simulate_keystrokes("j");
    settle(cx);
    assert_eq!(selected(&view, cx).as_deref(), Some(LIN_GARDENS));
    cx.simulate_keystrokes("k");
    settle(cx);
    assert_eq!(selected(&view, cx).as_deref(), Some(RUE_TRUST));
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(selected(&view, cx).as_deref(), Some(LIN_GARDENS));
}

#[gpui_kit::test]
fn enter_opens_the_side_pane_and_esc_closes_it(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    cx.simulate_keystrokes("j j");
    settle(cx);
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert_eq!(opened(&view, cx).as_deref(), Some(LIN_GARDENS));
    assert_eq!(
        read_version(&fake, LIN_GARDENS),
        Some(4),
        "a thread opened is read"
    );
    assert!(cx.debug_bounds("stream-pane-close").is_some());
    // With the pane open, j moves and the pane follows.
    cx.simulate_keystrokes("j");
    settle(cx);
    let next = selected(&view, cx);
    assert_eq!(opened(&view, cx), next);
    let top_before = view.read_with(cx, |v, _| {
        v.reading.stream.list.logical_scroll_top().item_ix
    });
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert_eq!(opened(&view, cx), None);
    view.read_with(cx, |v, _| {
        assert_eq!(v.reading.view, View::Reading, "esc closed only the pane");
        assert_eq!(
            v.reading.stream.list.logical_scroll_top().item_ix,
            top_before,
            "the stream kept its place"
        );
    });
    // Space opens too; esc twice leaves the screen.
    cx.simulate_keystrokes("space");
    settle(cx);
    assert!(opened(&view, cx).is_some());
    cx.simulate_keystrokes("escape escape");
    settle(cx);
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Posts);
}

#[gpui_kit::test]
fn read_more_opens_the_thread(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    let b = cx
        .debug_bounds("stream-read-more-1")
        .expect("a thread shows Read more");
    cx.simulate_click(b.center(), Modifiers::none());
    settle(cx);
    let o = opened(&view, cx).expect("opened");
    let kind = view.read_with(cx, |v, _| v.reading.opened.as_ref().unwrap().item.kind);
    assert_eq!(kind, blyg_core::Kind::Thread, "{o}");
    assert_eq!(selected(&view, cx), Some(o));
}

#[gpui_kit::test]
fn posts_on_screen_for_a_second_are_read(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    assert_eq!(read_version(&fake, ADA_FINISHED), None);
    let t0 = Instant::now();
    // In the background: nothing counts.
    view.update(cx, |v, cx| {
        v.stream_tick(t0, false, cx);
        v.stream_tick(t0 + Duration::from_secs(2), false, cx);
    });
    settle(cx);
    assert_eq!(read_version(&fake, ADA_FINISHED), None);
    view.update(cx, |v, cx| v.stream_tick(t0, true, cx));
    settle(cx);
    assert_eq!(read_version(&fake, ADA_FINISHED), None, "not yet");
    view.update(cx, |v, cx| {
        v.stream_tick(t0 + Duration::from_millis(1050), true, cx)
    });
    settle(cx);
    assert_eq!(read_version(&fake, ADA_FINISHED), Some(1), "read on view");
    assert_eq!(
        read_version(&fake, RUE_TRUST),
        Some(5),
        "an edited post on screen is read at its new version"
    );
    // On the Reader layout nothing is marked by being on screen.
    view.update_in(cx, |v, window, cx| {
        v.set_read_mode(ReadMode::Reader, window, cx)
    });
    settle(cx);
    let reads = |fake: &FakeBackend| -> Vec<_> {
        fake.reading()
            .into_iter()
            .map(|r| (r.remote_id, r.read_version))
            .collect()
    };
    let before = reads(&fake);
    view.update(cx, |v, cx| {
        v.stream_tick(t0 + Duration::from_secs(5), true, cx);
        v.stream_tick(t0 + Duration::from_secs(7), true, cx);
    });
    settle(cx);
    assert_eq!(reads(&fake), before);
}

#[gpui_kit::test]
fn search_filters_the_stream(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    cx.simulate_keystrokes("/");
    settle(cx);
    view.update_in(cx, |v, window, cx| {
        v.set_reading_query("garden", window, cx)
    });
    settle(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.reading.stream.list.item_count(), v.reading.shown.len());
        assert!(v.reading.shown.len() < v.reading.rows.len());
        assert!(
            v.reading
                .shown_rows()
                .all(|r| super::vm::matches_query(r, "garden"))
        );
    });
    view.update_in(cx, |v, window, cx| v.set_reading_query("", window, cx));
    settle(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.reading.stream.list.item_count(), v.reading.rows.len())
    });
}

#[gpui_kit::test]
fn the_toggle_and_its_keys_switch_modes(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("alt-cmd-2");
    settle(cx);
    view.read_with(cx, |v, _| {
        assert_eq!(v.reading.view, View::Reading, "from Posts, too");
        assert_eq!(v.reading.mode, ReadMode::Reader);
    });
    // Reader: ↓ opens as before.
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(opened(&view, cx).as_deref(), Some(RUE_TRUST));
    cx.simulate_keystrokes("alt-cmd-1");
    settle(cx);
    view.read_with(cx, |v, _| assert_eq!(v.reading.mode, ReadMode::Stream));
    assert_eq!(
        opened(&view, cx).as_deref(),
        Some(RUE_TRUST),
        "the open post stays open, in the pane"
    );
    let b = cx.debug_bounds("mode-reader").expect("toggle");
    cx.simulate_click(b.center(), Modifiers::none());
    settle(cx);
    view.read_with(cx, |v, _| assert_eq!(v.reading.mode, ReadMode::Reader));
}

#[gpui_kit::test]
fn the_selected_posts_actions_work_without_opening_it(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    cx.simulate_keystrokes("j");
    settle(cx);
    let drafts = fake.items().len();
    let key = view.read_with(cx, |v, _| v.reading.sel.clone().unwrap());
    view.update_in(cx, |v, window, cx| {
        v.stream_action_for_test(key, "Reply", window, cx)
    });
    settle(cx);
    assert_eq!(fake.items().len(), drafts + 1, "a stub draft");
    assert_eq!(view.read_with(cx, |v, _| v.reading.view), View::Posts);
}

// --- quote targets --- (issue #3)

fn opened_key(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<(String, String)> {
    view.read_with(cx, |v, _| v.reading.opened.as_ref().map(|o| o.key.clone()))
}

fn on_screen_version(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<u32> {
    view.read_with(cx, |v, _| {
        let o = v.reading.opened.as_ref()?;
        Some(o.shown.ready()?.get(o.ix?)?.version)
    })
}

#[gpui_kit::test]
fn a_held_original_opens_at_once_at_the_quoted_pin(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    view.update_in(cx, |v, window, cx| {
        v.open_original(ADA.into(), ADA_TIDES.into(), Some(1), window, cx)
    });
    settle(cx);
    assert_eq!(
        opened_key(&view, cx),
        Some(("sub-ada".into(), ADA_TIDES.into()))
    );
    assert_eq!(on_screen_version(&view, cx), Some(1), "the quoted pin");
    let pin_loaded = view.read_with(cx, |v, _| {
        let o = v.reading.opened.as_ref().unwrap();
        o.pins.get(&1).and_then(|l| l.ready()).is_some()
    });
    assert!(pin_loaded);
    assert!(fake.public_fetches().is_empty(), "held: nothing fetched");
    // An unpinned version can't be shown: the current one instead.
    view.update_in(cx, |v, window, cx| {
        v.open_original(RUE.into(), RUE_TRUST.into(), Some(4), window, cx)
    });
    settle(cx);
    assert_eq!(on_screen_version(&view, cx), Some(5));
}

#[gpui_kit::test]
fn an_original_nobody_follows_is_fetched_with_a_subscribe_action(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    let reads = |fake: &FakeBackend| -> Vec<_> {
        fake.reading()
            .into_iter()
            .map(|r| (r.remote_id, r.read_version))
            .collect()
    };
    let before = reads(&fake);
    view.update_in(cx, |v, window, cx| {
        v.open_original(KIT.into(), KIT_TIDES.into(), Some(1), window, cx)
    });
    settle(cx);
    assert_eq!(fake.public_fetches().len(), 1);
    let key = opened_key(&view, cx).expect("opened");
    assert!(super::original::is_external(&key));
    assert_eq!(key.1, KIT_TIDES);
    assert_eq!(on_screen_version(&view, cx), Some(1), "Kit's pinned v1");
    let chips: Vec<&str> = view.read_with(cx, |v, _| {
        v.reading_action_chips().into_iter().map(|c| c.id).collect()
    });
    assert_eq!(chips, ["Subscribe", "Reply", "Open on web"]);
    assert_eq!(reads(&fake), before, "nothing marked read");
    let subs = fake.subscriptions().len();
    view.update_in(cx, |v, window, cx| {
        v.reading_action_for_test("Subscribe", window, cx)
    });
    settle(cx);
    assert_eq!(fake.subscriptions().len(), subs + 1, "followed");
    // Asking again while it's open doesn't fetch again.
    view.update_in(cx, |v, window, cx| {
        v.open_original(KIT.into(), KIT_TIDES.into(), None, window, cx)
    });
    settle(cx);
    assert_eq!(fake.public_fetches().len(), 1);
}

#[gpui_kit::test]
fn the_lineage_lines_name_opens_the_profile_and_the_rest_the_post(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("alt-cmd-2");
    settle(cx);
    let key = view.read_with(cx, |v, _| {
        v.reading
            .rows
            .iter()
            .find(|r| r.remote_id == LIN_GARDENS)
            .map(super::vm::key)
            .unwrap()
    });
    view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
    settle(cx);
    let b = cx
        .debug_bounds("pf-lineage-stub-post")
        .expect("the stub line's title");
    cx.simulate_click(b.center(), Modifiers::none());
    settle(cx);
    assert_eq!(
        opened(&view, cx).as_deref(),
        Some(ADA_TIDES),
        "the stubbed post"
    );
    assert!(!view.read_with(cx, |v, _| v.profile_sheet_open()));
}

// --- responses --- (issue #7)

#[gpui_kit::test]
fn responses_seen_in_the_network_are_listed_in_the_pane(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    // The stream marks the post, without a number.
    assert!(view.read_with(cx, |v, _| v.has_responses(LIN, LIN_BENCH)));
    assert!(!view.read_with(cx, |v, _| v.has_responses(ADA, ADA_FINISHED)));
    let key = view.read_with(cx, |v, _| {
        v.reading
            .rows
            .iter()
            .find(|r| r.remote_id == LIN_BENCH)
            .map(super::vm::key)
            .unwrap()
    });
    view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
    settle(cx);
    let responses: Vec<(String, blyg_core::profile::Relation)> = view.read_with(cx, |v, _| {
        v.reading
            .opened
            .as_ref()
            .unwrap()
            .responses
            .iter()
            .map(|r| (r.item.remote_id.clone(), r.relation))
            .collect()
    });
    use blyg_core::profile::Relation;
    // The reply stubs and quotes the bench post: one act, so one response.
    assert_eq!(responses, [(ADA_REPLY.to_string(), Relation::Stubs)]);
    assert!(cx.debug_bounds("responses").is_some(), "the list");
    // A post nobody responded to has no list at all.
    let key = view.read_with(cx, |v, _| {
        v.reading
            .rows
            .iter()
            .find(|r| r.remote_id == ADA_FINISHED)
            .map(super::vm::key)
            .unwrap()
    });
    view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
    settle(cx);
    assert!(cx.debug_bounds("responses").is_none());
}

#[gpui_kit::test]
fn your_own_posts_list_their_verified_mentions(cx: &mut TestAppContext) {
    let (view, _, cx) = setup(cx);
    view.update_in(cx, |v, window, cx| {
        v.open(&blyg_core::LocalId("01J9M2A".into()), window, cx);
        v.toggle_versions(window, cx);
    });
    settle(cx);
    assert!(
        view.read_with(cx, |v, _| v.reading.mentions.ready().is_some()),
        "mentions fetched"
    );
    assert!(
        cx.debug_bounds("responses").is_some(),
        "listed under the post"
    );
}

// --- lineage --- (⌘J)

/// (centre id, ups, downs, ring open, previewed action)
type LineageNow = (
    String,
    Vec<String>,
    Vec<String>,
    bool,
    Option<super::lineage_vm::Act>,
);

fn lineage(view: &Entity<MainView>, cx: &mut VisualTestContext) -> Option<LineageNow> {
    view.read_with(cx, |v, _| match &v.reading.sheet {
        Some(super::RSheet::Lineage(s)) => Some((
            s.model.centre.id.clone(),
            s.model.ups.iter().map(|n| n.id.to_uppercase()).collect(),
            s.model.downs.iter().map(|n| n.id.to_uppercase()).collect(),
            s.ring,
            s.act,
        )),
        _ => None,
    })
}

#[gpui_kit::test]
fn lineage_walks_one_step_at_a_time_and_rings_the_actions(cx: &mut TestAppContext) {
    use super::lineage_vm::Act;
    let (view, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-r");
    settle(cx);
    let ix = view.read_with(cx, |v, _| {
        v.reading
            .shown_rows()
            .position(|r| r.remote_id == ADA_REPLY)
            .unwrap()
    });
    view.update_in(cx, |v, window, cx| v.stream_select(ix, window, cx));
    settle(cx);
    // The reply's glyph is drawn: it draws on the bench post.
    assert!(cx.debug_bounds("lineage-glyph").is_some());

    cx.simulate_keystrokes("cmd-j");
    settle(cx);
    let (centre, ups, _, ring, _) = lineage(&view, cx).expect("⌘J opens it");
    assert_eq!(centre, ADA_REPLY);
    assert!(!ring);
    // The reply stubs and quotes the bench post: one neighbour, not two.
    assert_eq!(
        ups.iter()
            .filter(|u| *u == &LIN_BENCH.to_uppercase())
            .count(),
        1
    );

    // Up to it, ⏎ makes it the centre; the reply is below it now.
    let at = ups
        .iter()
        .position(|u| u == &LIN_BENCH.to_uppercase())
        .unwrap();
    cx.simulate_keystrokes("up");
    for _ in 0..ups.len() {
        cx.simulate_keystrokes("left");
    }
    for _ in 0..at {
        cx.simulate_keystrokes("right");
    }
    cx.simulate_keystrokes("enter");
    settle(cx);
    let (centre, _, downs, _, _) = lineage(&view, cx).unwrap();
    assert_eq!(centre.to_uppercase(), LIN_BENCH.to_uppercase());
    assert!(downs.contains(&ADA_REPLY.to_uppercase()));
    // ⌫ walks back.
    cx.simulate_keystrokes("backspace");
    settle(cx);
    assert_eq!(lineage(&view, cx).unwrap().0, ADA_REPLY);

    // Space on the centre opens the ring; a letter previews; esc backs out
    // one layer at a time.
    cx.simulate_keystrokes("space");
    settle(cx);
    assert!(lineage(&view, cx).unwrap().3, "the ring is open");
    cx.simulate_keystrokes("q");
    assert_eq!(lineage(&view, cx).unwrap().4, Some(Act::Quote));
    cx.simulate_keystrokes("escape");
    assert_eq!(lineage(&view, cx).unwrap().4, None);
    cx.simulate_keystrokes("escape");
    assert!(!lineage(&view, cx).unwrap().3, "the ring closed");
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(lineage(&view, cx).is_none(), "esc closes the sheet");

    // Link post from the ring makes a draft that links the reply.
    cx.simulate_keystrokes("cmd-j");
    settle(cx);
    cx.simulate_keystrokes("space l enter");
    settle(cx);
    assert!(lineage(&view, cx).is_none());
    let draft = view.read_with(cx, |v, cx| v.editor.read(cx).value().to_string());
    assert!(draft.contains(&format!("[[{ADA_REPLY}]]")), "{draft}");
}
