//! The `[[` / `![[` picker as a panel with its own search box: ⌘K (a
//! quote: `![[id]]` on its own line in a thread), and typing the brackets
//! when the blyg's `picker_typing` is `panel` (with `auto` or `editor` the
//! query is typed in the editor instead, `composer::assist`). It offers
//! only what's already held (your own published posts plus imported posts
//! from blyg subscriptions), searched locally (`Backend::pick_search`:
//! every word, source, subscription, sort), and never fetches by URL.
//!
//! Typing `![[` at the start of a line in a thread opens it (see
//! [`transclusion_trigger`]), and `[[` anywhere outside code opens the link
//! picker (`picker::link_trigger`); esc then puts the typed brackets back.

use gpui_kit::base::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use std::ops::Range;

use blyg_core::{Item, Pickable};

use super::vm;
use super::{RSheet, View};
use crate::app::MainView;
use crate::composer::picker::{self, Filters, PickKind};

impl MainView {
    pub(crate) fn open_quote_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.reading.sheet, Some(RSheet::Quote { .. })) {
            self.cancel_quote_picker(window, cx);
            return;
        }
        if let Some(item) = self.quote_target(cx) {
            self.open_quote_sheet(item, PickKind::Quote, None, window, cx);
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
        kind: PickKind,
        typed: Option<(usize, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match kind {
            PickKind::Quote => "Quote… (your posts and your reading)",
            PickKind::Link => "Link to… (your posts and your reading)",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let sub = cx.subscribe_in(&input, window, |this, _, ev, window, cx| match ev {
            InputEvent::PressEnter { .. } => this.pick_quote(window, cx),
            InputEvent::Change => {
                if let Some(RSheet::Quote { sel, .. }) = this.reading.sheet.as_mut() {
                    *sel = 0;
                }
                this.search_quote_sheet(cx);
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
                kind,
                rows: Vec::new(),
            },
            cx,
        );
        self.search_quote_sheet(cx);
    }

    /// Search again for the box and the filters (local and quick: the
    /// store's full-text index).
    fn search_quote_sheet(&mut self, cx: &mut Context<Self>) {
        let Some(RSheet::Quote { target, input, .. }) = self.reading.sheet.as_ref() else {
            return;
        };
        let exclude = self
            .backend
            .item(target)
            .and_then(|i| i.server_id)
            .map(|s| s.0);
        let q = Filters::get(cx).query(&input.read(cx).value(), exclude);
        let found = self.backend.pick_search(&q);
        if let Some(RSheet::Quote { rows, sel, .. }) = self.reading.sheet.as_mut() {
            *sel = (*sel).min(found.len().saturating_sub(1));
            *rows = found;
        }
    }

    /// Did the edit that just happened type the `[` of a line-leading `![[`
    /// (a quote) or of a `[[` (a link)? The brackets' range in the text.
    /// Call before `after_edit`, while `current` still holds the old text.
    pub(crate) fn typed_picker(&self, cx: &App) -> Option<(PickKind, Range<usize>)> {
        let old = &self.current.as_ref()?.content_md;
        let s = self.editor.read(cx);
        let (new, cursor) = (s.value(), s.cursor());
        if let Some(r) = transclusion_trigger(old, &new, cursor) {
            return Some((PickKind::Quote, r));
        }
        picker::link_trigger(old, &new, cursor).map(|at| (PickKind::Link, at..cursor))
    }

    /// Typed brackets open the picker. With `picker_typing` in the editor
    /// (`auto`, `editor`) the brackets stay and what's typed after them is
    /// the query (`Assist`'s popup). As a panel, the typed text comes out
    /// of the editor (the pick puts the whole directive there), and esc
    /// puts it back.
    pub(crate) fn open_picker_from_typing(
        &mut self,
        kind: PickKind,
        typed: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reading.sheet.is_some() {
            return;
        }
        let item = match kind {
            PickKind::Quote => match self.quote_target(cx) {
                Some(item) => item,
                None => return,
            },
            PickKind::Link => match self.current.clone() {
                Some(item) => item,
                None => return,
            },
        };
        if self.backend.picker_typing().in_editor() {
            let at = typed.end - kind.open().len();
            let exclude = item.server_id.map(|s| s.0);
            self.assist
                .update(cx, |a, cx| a.open_picker(kind, at, exclude, cx));
            return;
        }
        let text = self.editor.read(cx).value().to_string();
        let Some(removed) = text.get(typed.clone()).map(str::to_string) else {
            return;
        };
        let mut new_text = text.clone();
        new_text.replace_range(typed.clone(), "");
        self.splice_editor(&text, &new_text, Some(typed.start), window, cx);
        self.open_quote_sheet(item, kind, Some((typed.start, removed)), window, cx);
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
    pub(crate) fn quote_candidates(&self, _cx: &App) -> Vec<Pickable> {
        match self.reading.sheet.as_ref() {
            Some(RSheet::Quote { rows, .. }) => rows.clone(),
            _ => vec![],
        }
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
        let Some(RSheet::Quote {
            target,
            sel,
            kind,
            typed,
            ..
        }) = self.reading.sheet.as_ref()
        else {
            return;
        };
        let Some(pick) = cands.get(*sel).cloned() else {
            return;
        };
        let (target, kind) = (target.clone(), *kind);
        let typed_at = typed.as_ref().map(|(at, _)| *at);
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
        let (new_text, caret) = match kind {
            PickKind::Quote => vm::insert_transclusion(&text, cursor, &pick.id),
            // Where the typed `[[` was (it came out when the panel opened).
            PickKind::Link => {
                let at = typed_at.unwrap_or(cursor).min(text.len());
                picker::insert(&text, at, at, PickKind::Link, &pick.id)
            }
        };
        self.splice_editor(&text, &new_text, Some(caret), window, cx);
        let verb = match kind {
            PickKind::Quote => "Quoted",
            PickKind::Link => "Linked",
        };
        let from = pick
            .source_title
            .clone()
            .unwrap_or_else(|| "yours".to_string());
        self.show_toast(format!("{verb} “{}”", pick.title), Some(from.into()), cx);
        cx.notify();
    }

    pub(super) fn render_quote_sheet(&self, sheet: &RSheet, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        let RSheet::Quote {
            input, sel, kind, ..
        } = sheet
        else {
            return div().into_any_element();
        };
        let cands = self.quote_candidates(cx);
        let (sel, kind) = (*sel, *kind);
        let weak = cx.entity().downgrade();
        let set: picker::OnFilters = std::rc::Rc::new(move |f: Filters, _, cx: &mut App| {
            f.set(cx);
            let _ = weak.update(cx, |v, cx| {
                v.search_quote_sheet(cx);
                cx.notify();
            });
        });
        let weak = cx.entity().downgrade();
        let pick: picker::OnPick = std::rc::Rc::new(move |i, window, cx: &mut App| {
            let _ = weak.update(cx, |v, cx| {
                if let Some(RSheet::Quote { sel, .. }) = v.reading.sheet.as_mut() {
                    *sel = i;
                }
                v.pick_quote(window, cx);
            });
        });
        let subs = picker::blyg_subs(&self.backend.subscriptions());
        let filters = Filters::get(cx);
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
            .child(self.sheet_heading(kind.heading()))
            .child(self.input_box(
                gpui_kit::base::input::Input::new(input).into_any_element(),
                false,
            ))
            .child(div().mt(px(8.)).child(picker::filter_row(p, &filters, &subs, set)))
            .child(
                div()
                    .id("quote-list")
                    .mt(px(8.))
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .when(cands.is_empty(), |d| {
                        d.child(div().p(px(8.)).italic().text_color(p.muted).child(
                            "Nothing held matches. Links and quotes come from your published posts and your blyg subscriptions.",
                        ))
                    })
                    .children(picker::rows(p, &cands, sel, pick)),
            )
            .child(self.keys_row(vec![
                self.key_hint("↑↓", "choose"),
                self.key_hint("⏎", kind.enter_hint()),
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
