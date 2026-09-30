//! Headless GPUI tests of profiles, through real keystrokes and clicks on a
//! zero-latency FakeBackend.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use blyg_core::config::MemoryTokenStore;
use blyg_core::{Backend, ConfigStore};
use gpui_kit::{Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};

use super::vm::{self, Button, Tab};
use crate::app::MainView;
use crate::app::reading::View;
use crate::app::studio::webview::{FactoryGlobal, PreviewSurface};
use crate::fake::profile_seed::TIDES;
use crate::fake::reading_seed::*;
use crate::fake::{FakeBackend, ORIGIN, Timing};
use crate::prefs::Prefs;

const CONNECTED: &str = "# test config\nblyg-url = https://blyg.example.com\n";

/// Records whether the native preview is visible (no WKWebView in tests).
struct Stub(Rc<RefCell<Vec<bool>>>);

impl PreviewSurface for Stub {
    fn set_frame(&mut self, _: Bounds<Pixels>) {}
    fn set_visible(&mut self, v: bool) {
        self.0.borrow_mut().push(v);
    }
    fn load(&mut self, _: &str) {}
    fn eval(&mut self, _: &str) {}
    fn focus_parent(&mut self) {}
    fn set_dark(&mut self, _: bool) {}
}

type Setup<'a> = (
    Entity<MainView>,
    Arc<FakeBackend>,
    Rc<RefCell<Vec<bool>>>,
    &'a mut VisualTestContext,
);

fn setup(cx: &mut TestAppContext) -> Setup<'_> {
    let prefs = Prefs::from_config(ConfigStore::in_memory(CONNECTED).config());
    let visible = Rc::new(RefCell::new(Vec::new()));
    let v2 = visible.clone();
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::app::bind_keys(cx);
        crate::settings::init(
            ConfigStore::in_memory(CONNECTED),
            Arc::new(MemoryTokenStore::default()),
            None,
            cx,
        );
        cx.set_global(FactoryGlobal(Rc::new(move |_, _| {
            Ok(Box::new(Stub(v2.clone())) as Box<dyn PreviewSurface>)
        })));
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
    (view, fake, visible, cx)
}

/// Let background tasks (zero latency) land, and draw.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let b = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} isn't on screen"));
    // Near the left edge: the sheet may still be sliding in (test clocks
    // don't run animations out), so its right edge can be past the window.
    let at = gpui_kit::point(b.origin.x + gpui_kit::px(4.), b.center().y);
    cx.simulate_click(at, Modifiers::none());
    settle(cx);
}

fn open_row(view: &Entity<MainView>, id: &str, cx: &mut VisualTestContext) {
    view.update_in(cx, |v, window, cx| v.show_view(View::Reading, window, cx));
    let key = view.read_with(cx, |v, _| {
        v.reading_rows()
            .iter()
            .find(|r| r.remote_id == id)
            .map(crate::app::reading::vm::key)
            .expect("row shown")
    });
    view.update_in(cx, |v, window, cx| v.open_reading(key, window, cx));
    settle(cx);
}

/// The profile on screen: (url opened, stack depth, name).
fn top(
    view: &Entity<MainView>,
    cx: &mut VisualTestContext,
) -> Option<(String, usize, Option<String>)> {
    view.read_with(cx, |v, _| {
        v.profiles.page().map(|p| {
            (
                p.url.clone(),
                v.profiles.stack.len(),
                p.profile.as_ref().and_then(|p| p.name.clone()),
            )
        })
    })
}

fn header(view: &Entity<MainView>, cx: &mut VisualTestContext) -> vm::Header {
    view.read_with(cx, |v, _| {
        let p = v
            .profiles
            .page()
            .and_then(|p| p.profile.clone())
            .expect("loaded");
        vm::header(&p, &v.reading.subs)
    })
}

#[gpui_kit::test]
fn cmd_i_opens_the_profile_of_the_reading_items_origin(cx: &mut TestAppContext) {
    let (view, fake, _, cx) = setup(cx);
    open_row(&view, LIN_GARDENS, cx);
    assert!(
        fake.profile_fetches().is_empty(),
        "nothing fetched before an open"
    );
    cx.simulate_keystrokes("cmd-i");
    settle(cx);
    let (url, depth, name) = top(&view, cx).expect("open");
    assert_eq!((url.as_str(), depth), (LIN, 1));
    assert_eq!(name.as_deref(), Some("Lin"));
    assert!(cx.debug_bounds("profile-sheet").is_some());
    // Lists only: nothing count-like in the header, the buttons or the tabs.
    let h = header(&view, cx);
    for t in [h.name.clone(), h.origin_line.clone()]
        .into_iter()
        .chain(h.buttons.iter().map(|b| b.label().to_string()))
    {
        assert!(!t.chars().any(|c| c.is_ascii_digit()), "{t:?}");
    }
    // esc closes; ⌘I again reopens from the cache without fetching.
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(top(&view, cx).is_none());
    assert_eq!(
        view.read_with(cx, |v, _| v.reading.view),
        View::Reading,
        "esc only closed the sheet"
    );
    cx.simulate_keystrokes("cmd-i");
    settle(cx);
    assert_eq!(
        fake.profile_fetches().len(),
        1,
        "fresh: served from the cache"
    );
    // ⌘I again closes.
    cx.simulate_keystrokes("cmd-i");
    settle(cx);
    assert!(top(&view, cx).is_none());
}

#[gpui_kit::test]
fn follow_subscribes_and_flips_to_following(cx: &mut TestAppContext) {
    let (view, fake, _, cx) = setup(cx);
    view.update_in(cx, |v, window, cx| v.open_profile(TIDES, window, cx));
    settle(cx);
    assert_eq!(header(&view, cx).buttons[0], Button::Follow);
    click(cx, "pf-follow");
    assert!(
        fake.subscriptions().iter().any(|s| s.origin == TIDES),
        "{:?}",
        fake.subscriptions()
    );
    assert_eq!(header(&view, cx).buttons[0], Button::Following);
    assert_eq!(
        header(&view, cx).buttons[2],
        Button::AddToBlogroll,
        "blogroll is separate"
    );
    // Add to my blogroll: sets the flag on that subscription.
    click(cx, "pf-blogroll");
    assert!(
        fake.subscriptions()
            .iter()
            .any(|s| s.origin == TIDES && s.in_blogroll)
    );
    assert_eq!(header(&view, cx).buttons[2], Button::InBlogroll);
}

#[gpui_kit::test]
fn f_follows_the_selected_blogroll_entry(cx: &mut TestAppContext) {
    let (view, fake, _, cx) = setup(cx);
    view.update_in(cx, |v, window, cx| v.open_profile(LIN, window, cx));
    settle(cx);
    // Lin's blogroll: Ada, Rue, Tide Tables, Field Notes Quarterly.
    cx.simulate_keystrokes("down down f");
    settle(cx);
    assert!(fake.subscriptions().iter().any(|s| s.origin == TIDES));
    let rows = view.read_with(cx, |v, _| v.current_rows());
    assert!(rows[2].following, "the row flips to Following ✓");
    assert!(rows[0].following, "Ada was already followed");
}

#[gpui_kit::test]
fn a_blogroll_entry_opens_its_profile_and_back_returns(cx: &mut TestAppContext) {
    let (view, _, _, cx) = setup(cx);
    view.update_in(cx, |v, window, cx| v.open_profile(LIN, window, cx));
    settle(cx);
    click(cx, "pf-row-2-profile");
    let (url, depth, name) = top(&view, cx).unwrap();
    assert_eq!((url.as_str(), depth), (TIDES, 2));
    assert_eq!(name.as_deref(), Some("Tide Tables"));
    click(cx, "pf-back");
    assert_eq!(top(&view, cx).unwrap().0, LIN);
    // The same by keyboard: ↓ ⏎ opens Rue; ← goes back.
    cx.simulate_keystrokes("down enter");
    settle(cx);
    assert_eq!(top(&view, cx).unwrap().0, RUE);
    cx.simulate_keystrokes("left");
    settle(cx);
    assert_eq!(top(&view, cx).unwrap().0, LIN);
    // Connections tab: from Lin's posts we hold (and their feed).
    click(cx, "pf-tab-Connections");
    let rows = view.read_with(cx, |v, _| v.current_rows());
    let subs: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.title.clone(), r.sub.clone()))
        .collect();
    assert!(
        subs.contains(&("ada.blyg.example.com".into(), "stubbed 1 post".into())),
        "{subs:?}"
    );
    assert!(
        subs.contains(&("rue.blyg.example.com".into(), "forked 1 pin".into())),
        "{subs:?}"
    );
}

#[gpui_kit::test]
fn the_lineage_line_opens_the_stubbed_and_forked_origins(cx: &mut TestAppContext) {
    let (view, _, _, cx) = setup(cx);
    open_row(&view, LIN_GARDENS, cx);
    assert!(cx.debug_bounds("pf-lineage-stub").is_some(), "↳ stub of …");
    click(cx, "pf-lineage-stub");
    assert_eq!(top(&view, cx).unwrap().0, ADA);
    cx.simulate_keystrokes("escape");
    settle(cx);
    click(cx, "pf-lineage-fork");
    assert_eq!(top(&view, cx).unwrap().0, RUE);
    cx.simulate_keystrokes("escape");
    settle(cx);
    // The author's address in the header opens theirs.
    click(cx, "pf-origin-link");
    assert_eq!(top(&view, cx).unwrap().0, LIN);
    // A post without lineage has no lineage line.
    cx.simulate_keystrokes("escape");
    open_row(&view, RUE_TRUST, cx);
    assert!(cx.debug_bounds("pf-lineage-stub").is_none());
}

/// The owner reading API sends no lineage: the line comes from the opened
/// item's fetched item document (the current changelog row).
#[gpui_kit::test]
fn the_lineage_line_comes_from_the_item_document(cx: &mut TestAppContext) {
    let (view, fake, _, cx) = setup(cx);
    let row = fake
        .reading()
        .into_iter()
        .find(|r| r.remote_id == LIN_GARDENS)
        .unwrap();
    assert!(row.lineage().is_empty(), "the row itself has no lineage");
    open_row(&view, LIN_GARDENS, cx);
    let doc = view.read_with(cx, |v, _| {
        let o = v.reading.opened.as_ref().unwrap();
        o.changelog
            .ready()
            .and_then(|c| blyg_core::Lineage::of_changelog(c).cloned())
    });
    assert!(doc.is_some_and(|l| l.forked_from.is_some()));
    assert!(cx.debug_bounds("pf-lineage-stub").is_some(), "↳ stub of …");
    assert!(
        cx.debug_bounds("pf-lineage-fork").is_some(),
        "⑂ forked from …"
    );
}

#[gpui_kit::test]
fn own_profile_shows_the_blogroll_toggles(cx: &mut TestAppContext) {
    let (view, fake, _, cx) = setup(cx);
    // On the posts list, ⌘I is your own blyg.
    cx.simulate_keystrokes("cmd-i");
    settle(cx);
    let (url, _, name) = top(&view, cx).unwrap();
    assert_eq!(url, ORIGIN);
    assert_eq!(name.as_deref(), Some("A. Writer"));
    let own = view.read_with(cx, |v, _| {
        v.profiles.page().unwrap().profile.as_ref().unwrap().own
    });
    assert!(own);
    assert_eq!(header(&view, cx).buttons[0], Button::EditSite);
    let rows = view.read_with(cx, |v, _| v.current_rows());
    assert_eq!(rows.len(), fake.subscriptions().len(), "every subscription");
    assert!(rows.iter().all(|r| r.blogroll_toggle.is_some()));
    let (first, on) = rows[0].blogroll_toggle.clone().unwrap();
    click(cx, "pf-row-0-blogroll");
    let now = fake
        .subscriptions()
        .into_iter()
        .find(|s| s.id == first)
        .unwrap()
        .in_blogroll;
    assert_eq!(now, !on, "the toggle flipped the blogroll flag");
    // Edit site settings… opens the existing sheet.
    click(cx, "pf-edit-site");
    assert!(top(&view, cx).is_none());
    assert!(view.read_with(cx, |v, _| matches!(
        v.reading.sheet,
        Some(crate::app::reading::RSheet::Site { .. })
    )));
}

#[gpui_kit::test]
fn the_sheet_hides_the_native_preview(cx: &mut TestAppContext) {
    let (view, _, visible, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-2");
    settle(cx);
    assert_eq!(visible.borrow().last(), Some(&true), "preview showing");
    cx.simulate_keystrokes("cmd-i");
    settle(cx);
    assert!(top(&view, cx).is_some());
    assert_eq!(
        visible.borrow().last(),
        Some(&false),
        "hidden under the sheet"
    );
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert_eq!(visible.borrow().last(), Some(&true), "back when it closes");
    // ⇧⌘O's sheet too.
    cx.simulate_keystrokes("cmd-shift-o");
    settle(cx);
    assert_eq!(visible.borrow().last(), Some(&false));
}

#[gpui_kit::test]
fn open_profile_by_url(cx: &mut TestAppContext) {
    let (view, fake, _, cx) = setup(cx);
    cx.simulate_keystrokes("cmd-shift-o");
    settle(cx);
    assert!(cx.debug_bounds("pf-ask").is_some());
    cx.simulate_input("tides.example.org");
    cx.simulate_keystrokes("enter");
    settle(cx);
    let (url, _, name) = top(&view, cx).unwrap();
    assert_eq!(url, TIDES);
    assert_eq!(name.as_deref(), Some("Tide Tables"));
    assert_eq!(fake.profile_fetches(), [TIDES]);
    // A feed gets a card with only its posts.
    view.update_in(cx, |v, window, cx| {
        v.open_profile(format!("{OMAR}feed.xml"), window, cx)
    });
    settle(cx);
    let tab = view.read_with(cx, |v, _| v.profiles.page().unwrap().tab);
    assert_eq!(tab, Tab::Posts);
    // Offline: an explicit refresh keeps the cached copy, marked stale.
    fake.set_offline(true);
    view.update_in(cx, |v, _, cx| v.refresh_profile(cx));
    settle(cx);
    let stale = view.read_with(cx, |v, _| {
        v.profiles.page().unwrap().profile.as_ref().unwrap().stale
    });
    assert!(stale);
}
