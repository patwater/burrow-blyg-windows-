//! Subscriptions (⇧⌘S): subscribe with a preview first, unsubscribe,
//! pause/resume, and the blogroll flag. Following is client-local: there are
//! no follower counts anywhere.

use blyg_core::SubscriptionKind;
use gpui_kit::base::input::{Escape, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::RSheet;
use crate::app::MainView;

impl MainView {
    pub(super) fn open_subscribe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("A blyg or feed address, e.g. https://…")
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev, window, cx| match ev {
            InputEvent::PressEnter { .. } => this.subscribe_enter(window, cx),
            InputEvent::Change => {
                // Editing the address throws the old preview away.
                if let Some(RSheet::Subscribe {
                    previewed, error, ..
                }) = this.reading.sheet.as_mut()
                {
                    *previewed = None;
                    *error = None;
                    cx.notify();
                }
            }
            _ => {}
        });
        self._subs.push(sub);
        input.update(cx, |s, cx| s.focus(window, cx));
        self.open_reading_sheet(
            RSheet::Subscribe {
                input,
                previewed: None,
                error: None,
                busy: false,
            },
            cx,
        );
    }

    /// ⏎: preview first; ⏎ again on an unchanged address subscribes.
    pub(crate) fn subscribe_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(RSheet::Subscribe {
            input,
            previewed,
            error,
            busy,
        }) = self.reading.sheet.as_mut()
        else {
            return;
        };
        if *busy {
            return;
        }
        let url = input.read(cx).value().trim().to_string();
        if url.is_empty() {
            return;
        }
        *busy = true;
        *error = None;
        let confirm = previewed.as_ref().is_some_and(|(u, _)| *u == url);
        let title = previewed.as_ref().map(|(_, p)| p.title.clone());
        let backend = self.backend.clone();
        cx.notify();
        if confirm {
            let u = url.clone();
            let task = cx.background_spawn(async move { backend.subscribe(&u, title.as_deref()) });
            cx.spawn_in(window, async move |this, cx| {
                let r = task.await;
                let _ = this.update_in(cx, |v, window, cx| match r {
                    Ok(s) => {
                        v.close_reading_sheet(window, cx);
                        v.reading.subs = v.backend.subscriptions();
                        v.show_toast(format!("Subscribed to {}", s.title), None, cx);
                    }
                    Err(e) => v.subscribe_failed(e.to_string(), cx),
                });
            })
            .detach();
        } else {
            let u = url.clone();
            let task = cx.background_spawn(async move { backend.preview_subscription(&u) });
            cx.spawn_in(window, async move |this, cx| {
                let r = task.await;
                let _ = this.update_in(cx, |v, _, cx| match r {
                    Ok(p) => {
                        if let Some(RSheet::Subscribe {
                            previewed, busy, ..
                        }) = v.reading.sheet.as_mut()
                        {
                            *previewed = Some((url, p));
                            *busy = false;
                        }
                        cx.notify();
                    }
                    Err(e) => v.subscribe_failed(e.to_string(), cx),
                });
            })
            .detach();
        }
    }

    fn subscribe_failed(&mut self, msg: String, cx: &mut Context<Self>) {
        if let Some(RSheet::Subscribe { error, busy, .. }) = self.reading.sheet.as_mut() {
            *busy = false;
            *error = Some(msg);
        }
        cx.notify();
    }

    fn sub_op(&mut self, sub_id: String, op: SubOp, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let id = sub_id.clone();
        let task = cx.background_spawn(async move {
            match op {
                SubOp::Pause(p) => backend.pause_subscription(&id, p),
                SubOp::Blogroll(b) => backend.set_subscription(&id, Some(b), None),
                SubOp::Unsubscribe => backend.unsubscribe(&id),
            }
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                v.reading.subs = v.backend.subscriptions();
                v.reading.sub_sel = v
                    .reading
                    .sub_sel
                    .min(v.reading.subs.len().saturating_sub(1));
                match r {
                    Ok(()) if matches!(op, SubOp::Unsubscribe) => {
                        v.show_toast("Unsubscribed", None, cx)
                    }
                    Ok(()) => {}
                    Err(e) => {
                        v.show_toast(format!("Couldn't change the subscription: {e}"), None, cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_subscriptions_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let add = self
            .chip("subscribe", "+ Subscribe…")
            .on_click(cx.listener(|this, _, window, cx| this.open_subscribe(window, cx)))
            .into_any_element();
        let header = self.screen_header(
            "Subscriptions",
            "following is private to this blyg · esc back".into(),
            vec![add],
        );
        let rows = self.reading.subs.iter().enumerate().map(|(i, s)| {
            let paused = s.status == "paused";
            let selected = i == self.reading.sub_sel;
            let confirming = self.reading.unsub_confirm.as_deref() == Some(s.id.as_str());
            let (id1, id2, id3) = (s.id.clone(), s.id.clone(), s.id.clone());
            let blogroll = s.in_blogroll;
            div()
                .id(("sub-row", i))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(16.))
                .py(px(9.))
                .border_b_1()
                .border_color(p.line)
                .when(selected, |d| d.bg(p.sel))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.reading.sub_sel = i;
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .items_center()
                                .child(div().font_weight(FontWeight::MEDIUM).child(s.title.clone()))
                                .child(div().text_size(px(10.5)).text_color(p.muted).child(
                                    match s.kind {
                                        SubscriptionKind::Blyg => "blyg",
                                        SubscriptionKind::Rss => "RSS",
                                    },
                                ))
                                .when(paused, |d| {
                                    d.child(
                                        div()
                                            .px(px(6.))
                                            .rounded_full()
                                            .text_size(px(10.5))
                                            .bg(p.sel)
                                            .text_color(p.amber)
                                            .child("paused"),
                                    )
                                })
                                .when(s.status == "degraded", |d| {
                                    d.child(
                                        div()
                                            .text_size(px(10.5))
                                            .text_color(p.over)
                                            .child("having trouble fetching"),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(p.muted)
                                .truncate()
                                .child(super::vm::host(&s.origin)),
                        ),
                )
                .child(
                    self.chip(
                        format!("blogroll-{i}"),
                        if blogroll {
                            "☑ blogroll"
                        } else {
                            "☐ blogroll"
                        },
                    )
                    .when(blogroll, |d| d.text_color(p.accent))
                    .tooltip(|_, cx| {
                        cx.new(|_| super::Tip("List it on your blyg's blogroll".into()))
                            .into()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sub_op(id1.clone(), SubOp::Blogroll(!blogroll), cx)
                    })),
                )
                .child(
                    self.chip(
                        format!("pause-{i}"),
                        if paused { "Resume" } else { "Pause" },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sub_op(id2.clone(), SubOp::Pause(!paused), cx)
                    })),
                )
                .child(
                    self.chip(
                        format!("unsub-{i}"),
                        if confirming {
                            "Unsubscribe? click again"
                        } else {
                            "Unsubscribe"
                        },
                    )
                    .when(confirming, |d| d.border_color(p.over).text_color(p.over))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.reading.unsub_confirm.as_deref() == Some(id3.as_str()) {
                            this.reading.unsub_confirm = None;
                            this.sub_op(id3.clone(), SubOp::Unsubscribe, cx);
                        } else {
                            this.reading.unsub_confirm = Some(id3.clone());
                            cx.notify();
                        }
                    })),
                )
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .font_family("Inter")
            .text_size(px(13.))
            .child(header)
            .child(
                div()
                    .id("subs-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(self.reading.subs.is_empty(), |d| {
                        d.child(self.muted_note(
                            "No subscriptions yet. + Subscribe… adds a blyg or a feed.",
                        ))
                    })
                    .children(rows),
            )
            .into_any_element()
    }

    pub(super) fn render_subscribe_sheet(
        &self,
        sheet: &RSheet,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let RSheet::Subscribe {
            input,
            previewed,
            error,
            busy,
        } = sheet
        else {
            return div().into_any_element();
        };
        let preview = previewed.as_ref().map(|(_, pv)| {
            div()
                .id("sub-preview")
                .mt(px(10.))
                .p(px(10.))
                .rounded(px(8.))
                .border_1()
                .border_color(p.line)
                .bg(p.bar)
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(pv.title.clone()),
                        )
                        .child(div().text_color(p.muted).child(match pv.kind {
                            SubscriptionKind::Blyg => "a blyg",
                            SubscriptionKind::Rss => "an RSS/Atom feed",
                        })),
                )
                .when_some(pv.origin.clone(), |d, o| {
                    d.child(div().text_size(px(12.)).text_color(p.muted).child(o))
                })
                .when(pv.site_mismatch == Some(true), |d| {
                    d.child(
                        div()
                            .mt(px(4.))
                            .text_size(px(12.))
                            .text_color(p.amber)
                            .child(
                                "The feed says it belongs to a different site than the address.",
                            ),
                    )
                })
        });
        div()
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.close_reading_sheet(window, cx);
            }))
            .child(self.sheet_heading("Subscribe"))
            .child(
                div()
                    .mb(px(8.))
                    .text_color(p.muted)
                    .child("Paste a blyg or a feed. You'll see what it is before subscribing."),
            )
            .child(self.input_box(
                gpui_kit::base::input::Input::new(input).into_any_element(),
                error.is_some(),
            ))
            .when_some(error.clone(), |d, e| {
                d.child(div().mt(px(6.)).text_color(p.over).child(e))
            })
            .when(*busy, |d| {
                d.child(div().mt(px(6.)).text_color(p.muted).child("Checking…"))
            })
            .children(preview)
            .child(self.keys_row(if previewed.is_some() {
                vec![
                    self.key_hint("⏎", "subscribe"),
                    self.key_hint("esc", "cancel"),
                ]
            } else {
                vec![
                    self.key_hint("⏎", "preview"),
                    self.key_hint("esc", "cancel"),
                ]
            }))
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum SubOp {
    Pause(bool),
    Blogroll(bool),
    Unsubscribe,
}
