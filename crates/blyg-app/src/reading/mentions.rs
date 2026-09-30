//! Mentions & responses (⇧⌘M): a list, never a count. Who, origin,
//! relation, when; a dot for new; hide a mention; and per post, whether its
//! responses show on the public page.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::Load;
use super::vm::{self, MentionRow};
use crate::app::MainView;

impl MainView {
    pub(super) fn load_mentions(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.reading.mentions, Load::Ready(_)) {
            self.reading.mentions = Load::Loading;
        }
        let backend = self.backend.clone();
        let task = cx.background_spawn(async move { backend.mentions() });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                v.reading.mentions = Load::from_result(r);
                cx.notify();
            });
        })
        .detach();
    }

    /// Mentions as listed, grouped by your post: (title, local id, show
    /// responses, rows).
    pub(crate) fn mention_groups(
        &self,
    ) -> Vec<(String, Option<blyg_core::LocalId>, bool, Vec<MentionRow>)> {
        let Some(ms) = self.reading.mentions.ready() else {
            return vec![];
        };
        let mut ms: Vec<&blyg_core::Mention> = ms
            .iter()
            .filter(|m| self.reading.show_hidden || !m.hidden)
            .collect();
        ms.sort_by(|a, b| b.first_seen.cmp(&a.first_seen));
        let rows = ms
            .into_iter()
            .map(|m| (vm::mention_row(m, &self.reading.mentions_seen, self.now), m))
            .collect();
        let items = self.backend.items();
        vm::group_mentions(rows, &items)
            .into_iter()
            .map(|(t, item, rows)| {
                (
                    t,
                    item.map(|i| i.local_id.clone()),
                    item.is_some_and(|i| i.show_responses),
                    rows,
                )
            })
            .collect()
    }

    /// Every string the Mentions screen shows (tests: no digits as counts).
    #[cfg(test)]
    pub(crate) fn mentions_screen_strings(&self) -> Vec<String> {
        let mut out = vec!["Mentions".to_string(), self.mentions_hint()];
        for (title, _, show, rows) in self.mention_groups() {
            out.push(format!("On “{title}”"));
            out.push(responses_label(show).into());
            for r in rows {
                out.extend([r.who, r.origin, r.relation, r.when]);
                out.push(if r.hidden { "Unhide" } else { "Hide" }.into());
            }
        }
        out
    }

    fn mentions_hint(&self) -> String {
        "who quoted, replied to or forked your posts · esc back".into()
    }

    fn hide_mention(&mut self, id: String, hidden: bool, cx: &mut Context<Self>) {
        if let Load::Ready(ms) = &mut self.reading.mentions
            && let Some(m) = ms.iter_mut().find(|m| m.id == id)
        {
            m.hidden = hidden;
        }
        cx.notify();
        let backend = self.backend.clone();
        let task = cx.background_spawn(async move { backend.set_mention_hidden(&id, hidden) });
        cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                let _ = this.update(cx, |v, cx| {
                    v.show_toast(format!("Couldn't change it: {e}"), None, cx);
                    v.load_mentions(cx);
                });
            }
        })
        .detach();
    }

    fn toggle_responses(&mut self, id: blyg_core::LocalId, show: bool, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let task = cx.background_spawn(async move { backend.set_show_responses(&id, show) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(()) => v.show_toast(
                        if show {
                            "Responses show on the post's page"
                        } else {
                            "Responses hidden from the post's page"
                        },
                        None,
                        cx,
                    ),
                    Err(e) => v.show_toast(format!("Couldn't change it: {e}"), None, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_mentions_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let show_hidden = self.reading.show_hidden;
        let toggle_hidden = self
            .chip(
                "show-hidden",
                if show_hidden {
                    "Hide hidden"
                } else {
                    "Show hidden"
                },
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.reading.show_hidden = !this.reading.show_hidden;
                cx.notify();
            }))
            .into_any_element();
        let available = self.reading.available && self.reading.mentions != Load::Unavailable;
        let header = self.screen_header(
            "Mentions",
            self.mentions_hint(),
            if available {
                vec![toggle_hidden]
            } else {
                vec![]
            },
        );
        let body: AnyElement = match &self.reading.mentions {
            _ if !available => self.unavailable(),
            Load::Idle | Load::Loading => self.muted_note("Loading mentions…"),
            Load::Failed(e) => self.muted_note(format!("Couldn't load mentions: {e}")),
            _ => {
                let groups = self.mention_groups();
                if groups.is_empty() {
                    self.muted_note("Nobody has quoted, replied to or forked your posts yet.")
                } else {
                    div()
                        .id("mentions-list")
                        .flex_1()
                        .overflow_y_scroll()
                        .children(groups.into_iter().enumerate().map(
                            |(gi, (title, id, show, rows))| {
                                div()
                                    .px(px(16.))
                                    .pt(px(12.))
                                    .pb(px(6.))
                                    .border_b_1()
                                    .border_color(p.line)
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(format!("On “{title}”")),
                                            )
                                            .when_some(id, |d, id| {
                                                d.child(
                                                    self.chip(
                                                        format!("responses-{gi}"),
                                                        responses_label(show),
                                                    )
                                                    .when(show, |d| d.text_color(p.accent))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.toggle_responses(id.clone(), !show, cx)
                                                    })),
                                                )
                                            }),
                                    )
                                    .children(
                                        rows.into_iter().map(|r| self.render_mention_row(r, cx)),
                                    )
                            },
                        ))
                        .into_any_element()
                }
            }
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .font_family("Inter")
            .text_size(px(13.))
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_mention_row(&self, r: MentionRow, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let (id, hidden, source) = (r.id.clone(), r.hidden, r.source.clone());
        let profile_of = r.source.clone(); // --- profiles ---
        div()
            .id(SharedString::from(format!("mention-{}", r.id)))
            .flex()
            .items_center()
            .gap(px(8.))
            .py(px(6.))
            .when(hidden, |d| d.opacity(0.5))
            .child(
                div()
                    .w(px(6.))
                    .h(px(6.))
                    .flex_none()
                    .rounded_full()
                    .when(r.new, |d| d.bg(p.accent)),
            )
            .child(div().font_weight(FontWeight::MEDIUM).child(r.who))
            .child(div().text_color(p.muted).child(r.relation))
            // --- profiles --- the origin opens its profile; ↗ opens the source.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .id(SharedString::from(format!("mention-src-{}", r.id)))
                            .min_w_0()
                            .truncate()
                            .text_color(p.muted)
                            .cursor_pointer()
                            .border_b_1()
                            .border_dashed()
                            .border_color(p.muted.opacity(0.5))
                            .hover(|s| s.text_color(p.accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_profile(profile_of.clone(), window, cx)
                            }))
                            .child(r.origin),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("mention-open-{}", r.id)))
                            .flex_none()
                            .text_color(p.muted)
                            .cursor_pointer()
                            .hover(|s| s.text_color(p.accent))
                            .on_click(move |_, _, cx| cx.open_url(&source))
                            .child("↗"),
                    ),
            )
            // --- end profiles ---
            .child(div().text_size(px(11.5)).text_color(p.muted).child(r.when))
            .child(
                self.chip(
                    format!("hide-{}", r.id),
                    if hidden { "Unhide" } else { "Hide" },
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.hide_mention(id.clone(), !hidden, cx)),
                ),
            )
            .into_any_element()
    }
}

fn responses_label(show: bool) -> &'static str {
    if show {
        "responses shown on the page"
    } else {
        "responses hidden from the page"
    }
}
