//! The quote picker (⌘K): inserts `![[id]]` on its own line in a thread.
//! It offers only what's already held (your own posts plus imported posts
//! from blyg subscriptions) and never fetches anything by URL.
//!
//! Typing `![[` at the start of a line in a thread opens it too (see
//! [`transclusion_trigger`]); esc then puts the typed `![[` back.

use gpui_kit::base::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use std::ops::Range;

use blyg_core::Item;

use super::vm::{self, Quotable};
use super::{RSheet, View};
use crate::app::MainView;

impl MainView {
    pub(super) fn open_quote_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.reading.sheet, Some(RSheet::Quote { .. })) {
            self.cancel_quote_picker(window, cx);
            return;
        }
        if let Some(item) = self.quote_target(cx) {
            self.open_quote_sheet(item, None, window, cx);
        }
    }

    /// The thread a quote would go into, or `None` after showing why not.
    fn quote_target(&mut self, cx: &mut Context<Self>) -> Option<Item> {
        if self.reading.view != View::Posts || self.reading.own.is_some() {
            self.show_toast("Open a thread to quote into it", None, cx);
            return None;
        }
        let Some(item) = self.current.clone() else {
            self.show_toast("Open a thread to quote into it", None, cx);
            return None;
        };
        if !vm::can_quote_into(Some(&item)) {
            self.show_toast(
                "Quotes go in threads",
                Some(crate::keymap::hint("⌘T makes this a thread").into()),
                cx,
            );
            return None;
        }
        Some(item)
    }

    fn open_quote_sheet(
        &mut self,
        item: Item,
        typed: Option<(usize, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Quote… (your posts and your reading)")
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev, window, cx| match ev {
            InputEvent::PressEnter { .. } => this.pick_quote(window, cx),
            InputEvent::Change => {
                if let Some(RSheet::Quote { sel, .. }) = this.reading.sheet.as_mut() {
                    *sel = 0;
                }
                cx.notify();
            }
            _ => {}
        });
        self._subs.push(sub);
        input.update(cx, |s, cx| s.focus(window, cx));
        self.open_reading_sheet(
            RSheet::Quote {
                target: item.local_id,
                input,
                sel: 0,
                typed,
            },
            cx,
        );
    }

    /// Did the edit that just happened type the `[` of a line-leading `![[`?
    /// Call before `after_edit`, while `current` still holds the old text.
    pub(crate) fn typed_transclusion(&self, cx: &App) -> Option<Range<usize>> {
        let old = &self.current.as_ref()?.content_md;
        let s = self.editor.read(cx);
        transclusion_trigger(old, &s.value(), s.cursor())
    }

    /// Typing `![[` opens the picker: the typed text comes out of the editor
    /// (the pick puts a whole `![[id]]` line there), and esc puts it back.
    pub(crate) fn open_quote_picker_from_typing(
        &mut self,
        typed: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reading.sheet.is_some() {
            return;
        }
        let Some(item) = self.quote_target(cx) else {
            return;
        };
        let text = self.editor.read(cx).value().to_string();
        let Some(removed) = text.get(typed.clone()).map(str::to_string) else {
            return;
        };
        let mut new_text = text.clone();
        new_text.replace_range(typed.clone(), "");
        self.splice_editor(&text, &new_text, Some(typed.start), window, cx);
        self.open_quote_sheet(item, Some((typed.start, removed)), window, cx);
    }

    /// Esc (or ⌘K again): close the picker, putting back a typed `![[`.
    fn cancel_quote_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = match self.reading.sheet.as_mut() {
            Some(RSheet::Quote { typed, .. }) => typed.take(),
            _ => None,
        };
        self.close_reading_sheet(window, cx);
        let Some((at, removed)) = typed else {
            return;
        };
        let text = self.editor.read(cx).value().to_string();
        let at = at.min(text.len());
        if !text.is_char_boundary(at) {
            return;
        }
        let mut new_text = text.clone();
        new_text.insert_str(at, &removed);
        self.splice_editor(&text, &new_text, Some(at + removed.len()), window, cx);
    }

    /// What the picker offers right now.
    pub(crate) fn quote_candidates(&self, cx: &App) -> Vec<Quotable> {
        let Some(RSheet::Quote { target, input, .. }) = self.reading.sheet.as_ref() else {
            return vec![];
        };
        let exclude = self
            .backend
            .item(target)
            .and_then(|i| i.server_id)
            .map(|s| s.0);
        vm::quotables(
            &self.backend.items(),
            &self.backend.reading(),
            &self.backend.subscriptions(),
            exclude.as_deref(),
            &input.read(cx).value(),
        )
    }

    fn move_quote(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.quote_candidates(cx).len();
        if let Some(RSheet::Quote { sel, .. }) = self.reading.sheet.as_mut()
            && n > 0
        {
            *sel = (*sel as isize + delta).clamp(0, n as isize - 1) as usize;
            cx.notify();
        }
    }

    pub(crate) fn pick_quote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cands = self.quote_candidates(cx);
        let Some(RSheet::Quote { target, sel, .. }) = self.reading.sheet.as_ref() else {
            return;
        };
        let Some(pick) = cands.get(*sel).cloned() else {
            return;
        };
        let target = target.clone();
        self.reading.sheet = None;
        if self.current.as_ref().map(|c| &c.local_id) != Some(&target) {
            self.open(&target, window, cx);
        }
        self.mode = super::super::Mode::Edit;
        self.editor.update(cx, |s, cx| s.focus(window, cx));
        let (text, cursor) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.cursor())
        };
        let (new_text, caret) = vm::insert_transclusion(&text, cursor, &pick.id);
        self.splice_editor(&text, &new_text, Some(caret), window, cx);
        self.show_toast(
            format!("Quoted “{}”", pick.title),
            Some(pick.source.into()),
            cx,
        );
        cx.notify();
    }

    pub(super) fn render_quote_sheet(&self, sheet: &RSheet, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let RSheet::Quote { input, sel, .. } = sheet else {
            return div().into_any_element();
        };
        let cands = self.quote_candidates(cx);
        let sel = *sel;
        div()
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.cancel_quote_picker(window, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                this.move_quote(-1, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.move_quote(1, cx);
            }))
            .child(self.sheet_heading("Quote in this thread"))
            .child(self.input_box(
                gpui_kit::base::input::Input::new(input).into_any_element(),
                false,
            ))
            .child(
                div()
                    .id("quote-list")
                    .mt(px(8.))
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .when(cands.is_empty(), |d| {
                        d.child(div().p(px(8.)).italic().text_color(p.muted).child(
                            "Nothing held matches. Quotes come from your posts and your reading.",
                        ))
                    })
                    .children(cands.into_iter().enumerate().map(|(i, q)| {
                        div()
                            .id(("quote", i))
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .when(i == sel, |d| d.bg(p.sel))
                            .hover(|s| s.bg(p.sel))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(RSheet::Quote { sel, .. }) = this.reading.sheet.as_mut()
                                {
                                    *sel = i;
                                }
                                this.pick_quote(window, cx);
                            }))
                            .child(div().truncate().child(q.title))
                            .child(div().text_size(px(11.)).text_color(p.muted).child(q.source))
                    })),
            )
            .child(self.keys_row(vec![
                self.key_hint("↑↓", "choose"),
                self.key_hint("⏎", "insert ![[…]] on its own line"),
                self.key_hint("esc", "cancel"),
            ]))
            .into_any_element()
    }
}

/// If `old -> new` typed a single `[` at `cursor` that completes a `![[` at
/// the start of its line (after optional spaces/tabs), outside a fenced code
/// block, the byte range of that line prefix (indent + `![[`) in `new`.
/// Anything else, a paste included, is `None`.
pub(crate) fn transclusion_trigger(old: &str, new: &str, cursor: usize) -> Option<Range<usize>> {
    if new.len() != old.len() + 1 || cursor == 0 || cursor > new.len() {
        return None;
    }
    if !new.is_char_boundary(cursor) || !new[..cursor].ends_with('[') {
        return None;
    }
    let line_start = new[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &new[line_start..cursor];
    if prefix.trim_start_matches([' ', '\t']) != "![[" {
        return None;
    }
    // Exactly one `[` was typed at the caret.
    if new[..cursor - 1] != old[..cursor - 1] || new[cursor..] != old[cursor - 1..] {
        return None;
    }
    if in_code_fence(&new[..line_start]) {
        return None;
    }
    Some(line_start..cursor)
}

/// Whether text ending here leaves a ``` or ~~~ fence open.
fn in_code_fence(before: &str) -> bool {
    let mut open: Option<(char, usize)> = None;
    for line in before.lines() {
        let t = line.trim_start_matches(' ');
        let Some(c) = t.chars().next().filter(|c| *c == '`' || *c == '~') else {
            continue;
        };
        let run = t.chars().take_while(|x| *x == c).count();
        if run < 3 {
            continue;
        }
        match open {
            None => open = Some((c, run)),
            Some((oc, orun)) if oc == c && run >= orun && t[run..].trim().is_empty() => open = None,
            Some(_) => {}
        }
    }
    open.is_some()
}

#[cfg(test)]
mod trigger_tests {
    use super::transclusion_trigger as trig;

    fn typed(before: &str, after: &str) -> Option<std::ops::Range<usize>> {
        let old = format!("{}{after}", &before[..before.len() - 1]);
        let new = format!("{before}{after}");
        trig(&old, &new, before.len())
    }

    #[test]
    fn fires_on_a_line_leading_bracket_pair() {
        assert_eq!(typed("![[", ""), Some(0..3));
        assert_eq!(typed("One.\n\n![[", "\nTwo."), Some(6..9));
        assert_eq!(typed("One.\n  ![[", ""), Some(5..10));
    }

    #[test]
    fn ignores_mid_line_and_other_text() {
        assert_eq!(typed("see ![[", ""), None);
        assert_eq!(typed("![[x", ""), None);
        assert_eq!(typed("[[", ""), None);
        assert_eq!(typed("![", ""), None);
    }

    #[test]
    fn ignores_pastes_and_restores() {
        // Three characters at once (a paste, or esc putting `![[` back).
        assert_eq!(trig("One.\n", "One.\n![[", 8), None);
        assert_eq!(trig("", "![[abc]]", 3), None);
    }

    #[test]
    fn ignores_fenced_code() {
        assert_eq!(typed("```\n![[", "\n```"), None);
        assert_eq!(typed("~~~md\n![[", ""), None);
        assert_eq!(typed("```\ncode\n```\n![[", ""), Some(13..16));
    }
}

#[cfg(test)]
#[path = "quote_picker_tests.rs"]
mod typing_tests;
