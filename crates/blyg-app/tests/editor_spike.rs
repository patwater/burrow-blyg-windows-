//! Spike evidence: the gpui-base textarea handles a long document, soft wrap,
//! undo/redo, IME-marked text and ⌘-arrow navigation, headlessly.

use gpui_kit::base::input::{Textarea, TextareaState};
use gpui_kit::{
    AppContext, Context, Entity, EntityInputHandler, IntoElement, Render, TestAppContext, Window,
    div, prelude::*, px,
};
use std::time::Instant;

struct Host {
    input: Entity<TextareaState>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(700.))
            .h(px(600.))
            .text_size(px(19.))
            .child(Textarea::new(&self.input))
    }
}

/// gpui-base's editing keys follow each platform's conventions:
/// end of text, start of text, undo, redo.
const EDIT_KEYS: [&str; 4] = if cfg!(target_os = "macos") {
    ["cmd-down", "cmd-up", "cmd-z", "cmd-shift-z"]
} else {
    ["ctrl-end", "ctrl-home", "ctrl-z", "ctrl-y"]
};

#[gpui_kit::test]
fn long_text_editing(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let text: String = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(360);
    assert!(text.len() > 20_000);
    let (host, cx) = cx.add_window_view(|window, cx| {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(true)
                .default_value(text.clone())
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        Host { input }
    });
    let input = host.read_with(cx, |h, _| h.input.clone());
    cx.run_until_parked();

    // ⌘↓ goes to the end; typing appends.
    cx.simulate_keystrokes(EDIT_KEYS[0]);
    let t = Instant::now();
    cx.simulate_input("END");
    cx.run_until_parked();
    let per_edit = t.elapsed() / 3;
    eprintln!("spike: per-keystroke edit+layout on 20k chars: {per_edit:?}");
    assert!(input.read_with(cx, |s, _| s.value().ends_with("END")));

    // ⌘↑ goes to the start; typing prepends.
    cx.simulate_keystrokes(EDIT_KEYS[1]);
    cx.simulate_input("A");
    assert!(input.read_with(cx, |s, _| s.value().starts_with("ALorem")));

    // Undo / redo.
    cx.simulate_keystrokes(EDIT_KEYS[2]);
    cx.run_until_parked();
    assert!(input.read_with(cx, |s, _| s.value().starts_with("Lorem")));
    cx.simulate_keystrokes(EDIT_KEYS[3]);
    cx.run_until_parked();
    assert!(input.read_with(cx, |s, _| s.value().starts_with("ALorem")));

    // IME: marked (composing) text then commit; emoji is multi-byte.
    cx.update(|window, cx| {
        input.update(cx, |s, cx| {
            s.replace_and_mark_text_in_range(None, "に", None, window, cx);
            s.replace_text_in_range(None, "日本😀", window, cx);
        })
    });
    assert!(input.read_with(cx, |s, _| s.value().starts_with("A日本😀Lorem")));
}
