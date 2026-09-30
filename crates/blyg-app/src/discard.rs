//! Delete a draft, withdraw a published post (docs/SPEC.md rule 5: "No
//! deletes of published work. Withdraw instead").
//!
//! - **Delete Draft…** (⇧⌘⌫, Post menu, toolbar): drafts and scratch notes
//!   only. A sheet asks first (⏎ deletes, esc cancels); then the list moves
//!   on to the next post and a toast says so.
//! - **Withdraw…** (Post menu, and the toolbar's slot on a published post):
//!   a sheet with an optional note says, in plain words, that it's permanent
//!   and visible. It has no key on purpose.
//!
//! A child module of `app` so it can extend `MainView`; `app.rs` keeps only
//! the two `Sheet` variants (drawn in `render_sheet`) and the action hook,
//! marked `// --- delete & withdraw ---`.

use super::*;

gpui_kit::actions!(
    blygger,
    [
        /// ⇧⌘⌫: delete the current draft or scratch note (asks first).
        DeleteDraft,
        /// Post › Withdraw…: withdraw the current published post (asks first).
        Withdraw,
    ]
);

pub const DELETE_NOTE: &str = "This can't be undone.";
pub const WITHDRAW_NOTE: &str = "Withdrawn posts stay listed as withdrawn. You can't undo this.";

impl MainView {
    /// Hook: the actions, on the main window's root.
    pub(super) fn discard_actions(
        &self,
        d: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        d.on_action(cx.listener(Self::on_delete_draft))
            .on_action(cx.listener(Self::on_withdraw))
    }

    /// The post both actions work on: the current one, on the Posts screen,
    /// with no sheet up.
    fn discard_target(&mut self, cx: &mut Context<Self>) -> Option<Item> {
        if self.sheet.is_some() || self.reading.sheet.is_some() {
            return None;
        }
        if self.reading.view != reading::View::Posts {
            self.show_toast(crate::keymap::hint(toolbar::NEED_POSTS), None, cx);
            return None;
        }
        if self.current.is_none() {
            self.show_toast(toolbar::NEED_POST, None, cx);
        }
        self.current.clone()
    }

    fn on_delete_draft(&mut self, _: &DeleteDraft, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.discard_target(cx) else {
            return;
        };
        match vm::discard(&item) {
            vm::Discard::Delete => {
                let focus = cx.focus_handle();
                window.focus(&focus, cx);
                self.sheet_gen += 1;
                self.sheet = Some(Sheet::DeleteDraft {
                    title: vm::truncate_chars(&item.title(), 48),
                    noun: vm::discard_noun(&item),
                    id: item.local_id,
                    focus,
                });
                cx.notify();
            }
            // Never "delete" published work: point at Withdraw instead.
            vm::Discard::Withdraw => self.show_toast(
                "Published posts can't be deleted",
                Some("Post › Withdraw… withdraws it instead".into()),
                cx,
            ),
            vm::Discard::AlreadyWithdrawn => self.show_toast(toolbar::ALREADY_WITHDRAWN, None, cx),
        }
    }

    fn on_withdraw(&mut self, _: &Withdraw, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.discard_target(cx) else {
            return;
        };
        match vm::discard(&item) {
            vm::Discard::Withdraw => {
                let note = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("Note (optional), e.g. “no longer accurate”")
                });
                let sub = cx.subscribe_in(&note, window, |this, note, ev, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        let text = note.read(cx).value().to_string();
                        this.confirm_withdraw(text, window, cx);
                    }
                });
                self._subs.push(sub);
                note.update(cx, |s, cx| s.focus(window, cx));
                self.sheet_gen += 1;
                self.sheet = Some(Sheet::Withdraw {
                    id: item.local_id.clone(),
                    title: vm::truncate_chars(&item.title(), 48),
                    note,
                });
                cx.notify();
            }
            vm::Discard::Delete => self.show_toast(
                "Not published, so there's nothing to withdraw",
                Some(
                    crate::keymap::hint_owned(format!(
                        "⇧⌘⌫ deletes the {}",
                        vm::discard_noun(&item)
                    ))
                    .into(),
                ),
                cx,
            ),
            vm::Discard::AlreadyWithdrawn => self.show_toast(toolbar::ALREADY_WITHDRAWN, None, cx),
        }
    }

    /// The delete sheet's ⏎ (or its Delete button).
    pub(super) fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Sheet::DeleteDraft { id, noun, .. }) = self.sheet.take() else {
            return;
        };
        self.focus_after_sheet(window, cx);
        cx.notify();
        // A server draft is deleted on the blyg too (network), so off the UI thread.
        let backend = self.backend.clone();
        let task = cx.background_spawn({
            let id = id.clone();
            async move { backend.delete_draft(&id) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                match result {
                    Ok(()) => {
                        // The list moves on to the next post, previewed.
                        v.list.remove(&id);
                        if v.current.as_ref().is_some_and(|c| c.local_id == id) {
                            v.current = None;
                            v.mode = Mode::Search;
                            v.omni.update(cx, |s, cx| s.focus(window, cx));
                        }
                        v.requery(window, cx);
                        if let Some(ix) = v.list.selected_index() {
                            v.list_scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
                        }
                        let mut noun = noun.to_string();
                        noun[..1].make_ascii_uppercase();
                        v.show_toast(format!("{noun} deleted"), None, cx);
                    }
                    Err(CoreError::Offline) => v.show_toast(
                        format!("Offline. Delete the {noun} again when you're back online."),
                        None,
                        cx,
                    ),
                    Err(e) => v.show_toast(format!("Couldn't delete the {noun}: {e}"), None, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The withdraw sheet's ⏎ (or its Withdraw button).
    pub(super) fn confirm_withdraw(
        &mut self,
        note: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Sheet::Withdraw { id, .. }) = self.sheet.take() else {
            return;
        };
        self.focus_after_sheet(window, cx);
        cx.notify();
        let backend = self.backend.clone();
        let note = Some(note.trim().to_string()).filter(|n| !n.is_empty());
        let task = cx.background_spawn({
            let note = note.clone();
            async move { backend.withdraw(&id, note.as_deref()) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                match result {
                    Ok(version) => {
                        v.requery(window, cx);
                        let head = match &note {
                            Some(n) => format!("Withdrawn as v{version} · “{n}”"),
                            None => format!("Withdrawn as v{version}"),
                        };
                        v.show_toast(head, Some("It stays listed as withdrawn".into()), cx);
                    }
                    Err(CoreError::Offline) => {
                        v.show_toast("Offline. Withdraw again when you're back online.", None, cx)
                    }
                    Err(e) => v.show_toast(format!("Couldn't withdraw: {e}"), None, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `BLYGGER_DEMO=tb-delete` / `tb-withdraw` (fake mode, screenshots): the
    /// confirmation sheets, on the first draft / a published post.
    pub(super) fn discard_demo(
        &mut self,
        scenario: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match scenario {
            "tb-delete" => {
                self.open(&LocalId("01J9QK3".into()), window, cx);
                self.on_delete_draft(&DeleteDraft, window, cx);
            }
            "tb-withdraw" => {
                self.open(&LocalId("01J9PX1".into()), window, cx);
                self.on_withdraw(&Withdraw, window, cx);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "discard_tests.rs"]
mod tests;
