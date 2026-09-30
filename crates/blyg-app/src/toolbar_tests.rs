//! The toolbar (`show-buttons`): its rules without a window, then clicks,
//! tooltips and the show-buttons = false no-op in a headless GPUI window
//! against a zero-latency FakeBackend.

use std::sync::Arc;

use blyg_core::config::{MemoryTokenStore, NewNote};
use blyg_core::{Backend, ConfigStore, Item, Kind, LocalId, Status};
use gpui_kit::{Entity, Modifiers, TestAppContext, VisualTestContext};

use super::{
    ALREADY_WITHDRAWN, Button, CLOSE_SHEET, Facts, Fit, LEFT, NEED_AI, NEED_POST, NEED_POSTS,
    NOT_SCRATCH, Screen, TOO_LONG, ViewMode, buttons, fit, rule, visible,
};
use crate::app::{MainView, Sheet, TITLEBAR_H};
use crate::fake::{FakeBackend, Timing};
use crate::prefs::Prefs;

const CONNECTED: &str = "# test config\nblyg-url = https://blyg.example.com\n";

fn item(id: &str) -> Item {
    FakeBackend::with_timing(Timing::instant())
        .item(&LocalId(id.into()))
        .expect("seeded")
}

fn facts(current: Option<&Item>) -> Facts<'_> {
    Facts {
        current,
        view: ViewMode::Write,
        screen: Screen::Posts,
        reading_opened: false,
        versions_open: false,
        ai_enabled: true,
        publishing: false,
        sheet_open: false,
        new_note: NewNote::Draft,
    }
}

fn reason(action: &str, f: &Facts) -> Option<String> {
    rule(action, f).0
}

// ------------------------------------------------------------ rules

#[test]
fn publish_on_an_over_limit_fragment_says_thread() {
    let mut long = item("01J9QK3"); // a draft fragment
    long.content_md = "x".repeat(1200);
    assert!(long.over_limit());
    assert_eq!(
        reason("Publish", &facts(Some(&long))).as_deref(),
        Some(crate::keymap::hint(TOO_LONG))
    );
    // The same text as a thread publishes.
    long.kind = Kind::Thread;
    assert_eq!(reason("Publish", &facts(Some(&long))), None);
    // A scratch note's length never blocks (promotion picks the kind).
    long.kind = Kind::Fragment;
    long.status = Status::Scratch;
    assert_eq!(reason("Publish", &facts(Some(&long))), None);
    // Nothing selected, published with no edits, empty.
    assert_eq!(reason("Publish", &facts(None)).as_deref(), Some(NEED_POST));
    let public = item("01J9PX1");
    assert!(
        reason("Publish", &facts(Some(&public)))
            .unwrap()
            .starts_with("Already published as v1")
    );
    let mut empty = item("01J9QK3");
    empty.content_md = "  ".into();
    assert_eq!(
        reason("Publish", &facts(Some(&empty))).as_deref(),
        Some("Nothing to publish yet")
    );
}

#[test]
fn make_draft_only_for_scratch_notes() {
    let draft = item("01J9QK3");
    let r = reason("MakeDraft", &facts(Some(&draft))).unwrap();
    assert!(
        r.starts_with(NOT_SCRATCH) && r.ends_with("Already a draft"),
        "{r}"
    );
    let public = item("01J9PX1");
    let r = reason("MakeDraft", &facts(Some(&public))).unwrap();
    assert!(r.ends_with("Already on your blyg"), "{r}");
    let mut scratch = item("01J9QK3");
    scratch.status = Status::Scratch;
    assert_eq!(reason("MakeDraft", &facts(Some(&scratch))), None);
    scratch.content_md.clear();
    assert_eq!(
        reason("MakeDraft", &facts(Some(&scratch))).as_deref(),
        Some("Nothing to save as a draft yet")
    );
    assert_eq!(
        reason("MakeDraft", &facts(None)).as_deref(),
        Some(NEED_POST)
    );
}

#[test]
fn versions_need_a_published_post() {
    assert_eq!(
        reason("ShowVersions", &facts(None)).as_deref(),
        Some(NEED_POST)
    );
    let draft = item("01J9QK3");
    assert_eq!(
        reason("ShowVersions", &facts(Some(&draft))).as_deref(),
        Some("Not published yet, so there's no history")
    );
    let public = item("01J9M2A");
    assert_eq!(reason("ShowVersions", &facts(Some(&public))), None);
    // Open: active, and clicking closes it.
    let f = Facts {
        versions_open: true,
        ..facts(Some(&public))
    };
    assert_eq!(rule("ShowVersions", &f), (None, true));
    // Reading screen: the open post's version list.
    let f = Facts {
        screen: Screen::Reading,
        ..facts(None)
    };
    assert!(reason("ShowVersions", &f).is_some());
    let f = Facts {
        reading_opened: true,
        ..f
    };
    assert_eq!(reason("ShowVersions", &f), None);
}

#[test]
fn delete_and_withdraw_share_a_slot() {
    let draft = item("01J9QK3");
    let mut scratch = item("01J9QK3");
    scratch.status = Status::Scratch;
    for i in [&draft, &scratch] {
        let f = facts(Some(i));
        assert!(visible("DeleteDraft", &f) && !visible("Withdraw", &f));
        assert_eq!(reason("DeleteDraft", &f), None);
    }
    // A published post shows Withdraw, never Delete.
    let public = item("01J9PX1");
    let f = facts(Some(&public));
    assert!(!visible("DeleteDraft", &f) && visible("Withdraw", &f));
    assert_eq!(reason("Withdraw", &f), None);
    let labels: Vec<&str> = buttons(&f, "").iter().map(|b| b.label).collect();
    assert!(labels.contains(&"Withdraw") && !labels.contains(&"Delete"));
    let tip = buttons(&f, "")
        .into_iter()
        .find(|b| b.action == "Withdraw")
        .unwrap()
        .tooltip;
    assert_eq!(tip, "Withdraw…", "no key: it's permanent");
    let mut withdrawn = public.clone();
    withdrawn.status = Status::Withdrawn;
    let f = facts(Some(&withdrawn));
    assert!(visible("Withdraw", &f));
    assert_eq!(reason("Withdraw", &f).as_deref(), Some(ALREADY_WITHDRAWN));
    // Nothing selected: Delete, disabled.
    let f = facts(None);
    assert!(visible("DeleteDraft", &f) && !visible("Withdraw", &f));
    assert_eq!(reason("DeleteDraft", &f).as_deref(), Some(NEED_POST));
    let sheet = Facts {
        sheet_open: true,
        ..facts(Some(&draft))
    };
    assert_eq!(reason("DeleteDraft", &sheet).as_deref(), Some(CLOSE_SHEET));
}

#[test]
fn generate_needs_an_enabled_provider() {
    let draft = item("01J9QK3");
    let off = Facts {
        ai_enabled: false,
        ..facts(Some(&draft))
    };
    assert_eq!(reason("AiGenerate", &off).as_deref(), Some(NEED_AI));
    assert_eq!(reason("AiGenerate", &facts(Some(&draft))), None);
}

#[test]
fn screens_sheets_views_and_capture() {
    let draft = item("01J9QK3");
    let reading = Facts {
        screen: Screen::Mentions,
        ..facts(Some(&draft))
    };
    for a in ["NewDraft", "Publish", "ViewWrite", "AiGenerate"] {
        assert_eq!(
            reason(a, &reading).as_deref(),
            Some(crate::keymap::hint(NEED_POSTS)),
            "{a}"
        );
    }
    assert_eq!(
        reason("ShowCapture", &reading),
        None,
        "capture works anywhere"
    );
    let sheet = Facts {
        sheet_open: true,
        ..facts(Some(&draft))
    };
    assert_eq!(reason("Publish", &sheet).as_deref(), Some(CLOSE_SHEET));
    let f = Facts {
        view: ViewMode::Focus,
        ..facts(Some(&draft))
    };
    assert!(
        rule("ViewStudio", &f).1,
        "⌘E from the full editor is still it"
    );
    assert!(!rule("ViewWrite", &f).1);
}

#[test]
fn buttons_come_from_the_keymap_with_keys_in_tooltips() {
    let draft = item("01J9QK3");
    let b = buttons(&facts(Some(&draft)), "ctrl+alt+b");
    let labels: Vec<&str> = b.iter().map(|b| b.label).collect();
    assert_eq!(
        labels,
        [
            "New",
            "Make draft",
            "Publish",
            "Write",
            "Preview",
            "Full editor",
            "Versions",
            "Generate",
            "Delete",
            "Capture"
        ]
    );
    let tip = |a: &str| b.iter().find(|b| b.action == a).unwrap().tooltip.clone();
    // The key as this platform shows it (⌘⏎ on macOS, Ctrl+Enter on Windows).
    let key = |k: &str| crate::keymap::glyphs(&crate::keymap::keys(k));
    assert_eq!(tip("Publish"), format!("Publish  {}", key("cmd-enter")));
    assert_eq!(tip("NewDraft"), format!("New draft  {}", key("cmd-n")));
    assert_eq!(
        tip("ViewStudio"),
        format!("Full editor: editor + preview  {}", key("cmd-3"))
    );
    let hotkey = crate::prefs::hotkey_glyphs;
    assert_eq!(
        tip("ShowCapture"),
        format!("Quick capture  {}", hotkey("ctrl+alt+b"))
    );
    assert_eq!(
        tip("DeleteDraft"),
        format!(
            "Delete draft or scratch note…  {}",
            key("cmd-shift-backspace")
        )
    );
    let scratchy = Facts {
        new_note: NewNote::Scratch,
        ..facts(Some(&draft))
    };
    let b = buttons(&scratchy, "cmd+shift+space");
    let tip = |a: &str| b.iter().find(|b| b.action == a).unwrap().tooltip.clone();
    assert_eq!(
        tip("NewDraft"),
        format!("New scratch note  {}", key("cmd-n"))
    );
    assert_eq!(
        tip("ShowCapture"),
        format!("Quick capture  {}", hotkey("cmd+shift+space"))
    );
}

#[test]
fn the_row_fits_or_drops_labels() {
    // Labels and title; labels only; icons with the title; icons only.
    assert_eq!(
        fit(1600., 600., 240., 300., 170.),
        Fit::Labels { title: true }
    );
    assert_eq!(
        fit(1100., 600., 240., 300., 170.),
        Fit::Labels { title: false }
    );
    assert_eq!(
        fit(900., 600., 240., 300., 170.),
        Fit::Icons { title: true }
    );
    assert_eq!(
        fit(640., 600., 240., 300., 170.),
        Fit::Icons { title: false }
    );
}

// ------------------------------------------------------------ in a window

fn setup<'a>(
    cx: &'a mut TestAppContext,
    config: &str,
) -> (
    Entity<MainView>,
    Arc<FakeBackend>,
    &'a mut VisualTestContext,
) {
    let config = config.to_string();
    let prefs = Prefs::from_config(ConfigStore::in_memory(&config).config());
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::app::bind_keys(cx);
        crate::settings::init(
            ConfigStore::in_memory(&config),
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

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let b = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(b.center(), Modifiers::none());
    cx.run_until_parked();
}

fn button(view: &Entity<MainView>, action: &str, cx: &mut VisualTestContext) -> Button {
    view.read_with(cx, |v, cx| {
        v.toolbar_buttons(cx)
            .into_iter()
            .find(|b| b.action == action)
            .unwrap()
    })
}

#[gpui_kit::test]
fn clicking_publish_opens_the_publish_sheet(cx: &mut TestAppContext) {
    let (view, _fake, cx) = setup(cx, CONNECTED);
    // The first post (a draft fragment) is selected on launch.
    assert!(button(&view, "Publish", cx).enabled());
    assert!(cx.debug_bounds("toolbar").is_some());
    click(cx, "tb-Publish");
    view.read_with(cx, |v, _| {
        assert!(
            matches!(
                v.sheet,
                Some(Sheet::Publish {
                    next_version: 1,
                    ..
                })
            ),
            "the publish sheet is open"
        );
    });
    // With the sheet up, the post buttons wait for it.
    assert_eq!(
        button(&view, "MakeDraft", cx).reason.as_deref(),
        Some(CLOSE_SHEET)
    );
}

#[gpui_kit::test]
fn clicking_views_and_new_does_what_the_keys_do(cx: &mut TestAppContext) {
    let (view, _fake, cx) = setup(cx, CONNECTED);
    assert!(button(&view, "ViewWrite", cx).active);
    click(cx, "tb-ViewStudio");
    assert_eq!(view.read_with(cx, |v, _| v.studio.view), ViewMode::Studio);
    assert!(button(&view, "ViewStudio", cx).active);
    click(cx, "tb-ViewWrite");
    assert_eq!(view.read_with(cx, |v, _| v.studio.view), ViewMode::Write);
    // A disabled button does nothing: Versions on a never-published draft.
    let b = button(&view, "ShowVersions", cx);
    assert_eq!(
        b.reason.as_deref(),
        Some("Not published yet, so there's no history")
    );
    click(cx, "tb-ShowVersions");
    assert!(view.read_with(cx, |v, _| v.reading.own.is_none() && v.toast.is_none()));
}

#[gpui_kit::test]
fn disabled_reasons_in_the_window(cx: &mut TestAppContext) {
    let (view, fake, cx) = setup(cx, CONNECTED);
    // No AI enabled in this config.
    assert_eq!(
        button(&view, "AiGenerate", cx).reason.as_deref(),
        Some(NEED_AI)
    );
    // The selected draft grows past 1000: Publish says ⌘T.
    let id = LocalId("01J9QK3".into());
    fake.save(&id, &"word ".repeat(260)).unwrap();
    view.update_in(cx, |v, window, cx| v.refresh_from_backend(window, cx));
    cx.run_until_parked();
    assert_eq!(
        button(&view, "Publish", cx).reason.as_deref(),
        Some(crate::keymap::hint(TOO_LONG))
    );
    assert!(
        button(&view, "MakeDraft", cx)
            .reason
            .unwrap()
            .starts_with(NOT_SCRATCH)
    );
}

#[gpui_kit::test]
fn show_buttons_false_draws_no_toolbar(cx: &mut TestAppContext) {
    let (view, _fake, cx) = setup(cx, &format!("{CONNECTED}show-buttons = false\n"));
    assert!(cx.debug_bounds("toolbar").is_none());
    assert!(cx.debug_bounds("tb-Publish").is_none());
    assert!(view.read_with(cx, |v, _| !v.prefs.show_buttons));
    let fitted = cx.update(|window, cx| view.read(cx).toolbar_fit(window, cx).is_none());
    assert!(fitted, "no toolbar layout at all");
}

#[gpui_kit::test]
fn the_toolbar_stays_in_the_title_bar(cx: &mut TestAppContext) {
    let (_view, _fake, cx) = setup(cx, CONNECTED);
    let bar = cx.debug_bounds("toolbar").expect("toolbar");
    assert!(f32::from(bar.origin.y) >= 0. && f32::from(bar.size.height) <= TITLEBAR_H);
    // Every button sits inside it, clear of the traffic lights.
    let b = cx.debug_bounds("tb-ShowCapture").expect("capture button");
    assert!(f32::from(b.origin.x) >= LEFT);
    assert!(f32::from(b.origin.y + b.size.height) <= TITLEBAR_H);
}

#[gpui_kit::test]
fn settings_toggle_writes_show_buttons_and_reload_reads_it(cx: &mut TestAppContext) {
    let (view, _fake, cx) = setup(cx, CONNECTED);
    cx.simulate_keystrokes(&crate::keymap::keys("cmd-,"));
    cx.run_until_parked();
    // Let the sheet finish dropping in, so the chip is where it'll be
    // clicked: step the animation until the chip stops moving. (A fixed
    // three steps was sometimes short, and always on Windows.)
    let mut last = None;
    for _ in 0..40 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(150));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let now = cx.debug_bounds("show-buttons-off");
        if now.is_some() && now == last {
            break;
        }
        last = now;
    }
    click(cx, "show-buttons-off");
    assert!(view.read_with(cx, |v, _| !v.prefs.show_buttons));
    let text = cx.update(|_, cx| {
        crate::settings::get(cx)
            .store
            .text()
            .unwrap_or_default()
            .to_string()
    });
    assert!(text.contains("show-buttons = false"), "{text}");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds("toolbar").is_none());

    // Edited in the file, then ⌘⇧, : picked up live.
    cx.update(|_, cx| {
        crate::settings::write(
            &[(
                "show-buttons",
                blyg_core::config::Change::Set("true".into()),
            )],
            cx,
        )
        .unwrap();
    });
    cx.simulate_keystrokes(&crate::keymap::keys("cmd-shift-,"));
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.prefs.show_buttons));
    assert!(cx.debug_bounds("toolbar").is_some());
}
