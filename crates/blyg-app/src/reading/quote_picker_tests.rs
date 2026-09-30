//! Headless GPUI tests of typing `![[` to open the quote picker (issue #5),
//! through real keystrokes on a zero-latency FakeBackend.

use std::sync::Arc;

use blyg_core::config::MemoryTokenStore;
use blyg_core::{Backend, ConfigStore, LocalId};
use gpui_kit::{ClipboardItem, Entity, TestAppContext, VisualTestContext};

use super::RSheet;
use crate::app::MainView;
use crate::fake::reading_seed::*;
use crate::fake::{FakeBackend, Timing};
use crate::prefs::Prefs;

const CONNECTED: &str = "# test config\nblyg-url = https://blyg.example.com\n";
const DRAFT_THREAD: &str = "01J9H4C";
const FRAGMENT: &str = "01J9QK3";

fn setup(cx: &mut TestAppContext) -> (Entity<MainView>, &mut VisualTestContext) {
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

/// Open a post with the caret at the end of the editor, on a fresh line.
fn open_at_end(view: &Entity<MainView>, id: &str, cx: &mut VisualTestContext) -> String {
    view.update_in(cx, |v, window, cx| {
        v.open(&LocalId(id.into()), window, cx);
        v.editor.update(cx, |s, cx| {
            s.focus(window, cx);
            let end = s.text().len();
            s.set_selected_range(end..end, cx);
        });
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    editor_text(view, cx)
}

fn editor_text(view: &Entity<MainView>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |v, cx| v.editor.read(cx).value().to_string())
}

fn picker_open(view: &Entity<MainView>, cx: &mut VisualTestContext) -> bool {
    view.read_with(cx, |v, _| {
        matches!(v.reading.sheet, Some(RSheet::Quote { .. }))
    })
}

#[gpui_kit::test]
fn typing_bang_brackets_opens_the_picker_and_picking_inserts(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("![");
    cx.run_until_parked();
    assert!(!picker_open(&view, cx));
    cx.simulate_input("[");
    cx.run_until_parked();
    assert!(picker_open(&view, cx), "`![[` opens the picker");
    assert_eq!(
        editor_text(&view, cx),
        before,
        "the typed `![[` is taken out"
    );
    // What's typed next filters the picker.
    cx.simulate_input("hyperlink");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(!picker_open(&view, cx));
    let text = editor_text(&view, cx);
    assert_eq!(
        text.matches("![[").count(),
        before.matches("![[").count() + 1
    );
    assert!(
        text.lines().any(|l| l == format!("![[{RUE_TRUST}]]")),
        "{text}"
    );
}

#[gpui_kit::test]
fn esc_puts_the_typed_brackets_back_without_reopening(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("![[");
    cx.run_until_parked();
    assert!(picker_open(&view, cx));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!picker_open(&view, cx), "esc closes, and doesn't reopen");
    assert_eq!(editor_text(&view, cx), format!("{before}![["));
    // Typing on is literal: the caret sits after the restored `![[`.
    cx.simulate_input("x");
    cx.run_until_parked();
    assert!(!picker_open(&view, cx));
    assert_eq!(editor_text(&view, cx), format!("{before}![[x"));
}

#[gpui_kit::test]
fn in_a_fragment_it_explains_and_leaves_the_text(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let before = open_at_end(&view, FRAGMENT, cx);
    cx.simulate_input("![[");
    cx.run_until_parked();
    assert!(!picker_open(&view, cx));
    assert_eq!(editor_text(&view, cx), format!("{before}![["));
    view.read_with(cx, |v, _| {
        assert!(v.toast.as_ref().is_some_and(|t| t.text.contains("threads")));
    });
}

#[gpui_kit::test]
fn pasting_bang_brackets_doesnt_open_the_picker(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    let before = open_at_end(&view, DRAFT_THREAD, cx);
    cx.write_to_clipboard(ClipboardItem::new_string("![[".into()));
    cx.simulate_keystrokes(&crate::keymap::keys("cmd-v"));
    cx.run_until_parked();
    assert!(!picker_open(&view, cx));
    assert_eq!(editor_text(&view, cx), format!("{before}![["));
}

#[gpui_kit::test]
fn inside_a_code_fence_it_stays_literal(cx: &mut TestAppContext) {
    let (view, cx) = setup(cx);
    open_at_end(&view, DRAFT_THREAD, cx);
    cx.simulate_input("```");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("![[");
    cx.run_until_parked();
    assert!(!picker_open(&view, cx));
    assert!(editor_text(&view, cx).ends_with("```\n![["));
}
