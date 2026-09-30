//! --- auto-update --- The main window's side of `crate::update`: the quiet
//! status-bar notice and the manual check's toast. A child module of `app`
//! (like the other hooks) so it can use `MainView`'s toast.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::MainView;
use crate::update::{self, NoticeAction};

impl MainView {
    /// A toast from the updater ("You're up to date (0.3.0)").
    pub(crate) fn update_toast(&mut self, text: String, cx: &mut Context<Self>) {
        self.show_toast(text, None, cx);
    }

    /// "Blygger X is ready · Restart to update · What's new" (or available,
    /// or can't update in place), when there's something to say.
    pub(super) fn render_update_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = update::notice(cx)?;
        let p = self.palette;
        let link = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .cursor_pointer()
                .text_color(p.accent)
                .hover(|s| s.underline())
                .child(label)
        };
        Some(
            div()
                .id("update-notice")
                .flex()
                .items_center()
                .gap(px(6.))
                .min_w_0()
                .overflow_hidden()
                .child(div().truncate().child(notice.text))
                .when_some(notice.action, |d, (label, action)| {
                    d.child("·")
                        .child(link("update-action", label).on_click(cx.listener(
                            move |_, _, _, cx| {
                                // Deferred: it may quit, or toast into this view.
                                cx.defer(move |cx| match action {
                                    NoticeAction::Restart => update::restart_to_update(cx),
                                    NoticeAction::Download => update::download_now(cx),
                                });
                            },
                        )))
                })
                .when_some(notice.link, |d, (label, url)| {
                    d.child("·")
                        .child(link("update-link", label).on_click(move |_, _, cx| {
                            cx.open_url(&url);
                        }))
                })
                .into_any_element(),
        )
    }
}
