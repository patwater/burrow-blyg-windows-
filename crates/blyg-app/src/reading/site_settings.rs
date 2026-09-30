//! The blyg's own site settings (title, author, bio, links), saved to the
//! server with `save_settings`. Not the app's Settings (⌘,).

use blyg_core::Settings;
use gpui_kit::base::input::{Escape, InputEvent, InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::vm;
use super::{Load, RSheet};
use crate::app::MainView;

impl MainView {
    // --- profiles --- pub(crate): the profile sheet's "Edit site settings…".
    pub(crate) fn open_site_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.reading.sheet, Some(RSheet::Site { .. })) {
            self.close_reading_sheet(window, cx);
            return;
        }
        let input = |ph: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(ph))
        };
        let title = input("Site title", window, cx);
        let name = input("Your name (optional)", window, cx);
        let bio = input("A line about you (optional)", window, cx);
        let links = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(true));
        for i in [&title, &name, &bio] {
            let sub = cx.subscribe_in(i, window, |this, _, ev, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.save_site_settings(window, cx);
                }
            });
            self._subs.push(sub);
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.open_reading_sheet(
            RSheet::Site {
                title,
                name,
                bio,
                links,
                load: Load::Loading,
                error: None,
                busy: false,
                focus,
            },
            cx,
        );
        let backend = self.backend.clone();
        let task = cx.background_spawn(async move { backend.settings() });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                let Some(RSheet::Site {
                    title,
                    name,
                    bio,
                    links,
                    load,
                    ..
                }) = v.reading.sheet.as_mut()
                else {
                    return;
                };
                if let Ok(s) = &r {
                    let set = |e: &Entity<InputState>,
                               t: &Option<String>,
                               window: &mut Window,
                               cx: &mut App| {
                        e.update(cx, |st, cx| {
                            st.set_value(t.clone().unwrap_or_default(), window, cx)
                        })
                    };
                    set(title, &s.site_title, window, cx);
                    set(name, &s.author_name, window, cx);
                    set(bio, &s.author_bio, window, cx);
                    links.update(cx, |st, cx| {
                        st.set_value(vm::format_links(&s.author_links), window, cx)
                    });
                    title.update(cx, |st, cx| st.focus(window, cx));
                }
                *load = Load::from_result(r);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn save_site_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(RSheet::Site {
            title,
            name,
            bio,
            links,
            load,
            error,
            busy,
            ..
        }) = self.reading.sheet.as_mut()
        else {
            return;
        };
        let Load::Ready(base) = load else {
            return;
        };
        if *busy {
            return;
        }
        let text = |e: &Entity<InputState>, cx: &App| {
            Some(e.read(cx).value().trim().to_string()).filter(|s| !s.is_empty())
        };
        let parsed = vm::parse_links(&links.read(cx).value());
        let author_links = match parsed {
            Ok(l) => l,
            Err(e) => {
                *error = Some(e);
                cx.notify();
                return;
            }
        };
        let settings = Settings {
            site_title: text(title, cx),
            author_name: text(name, cx),
            author_bio: text(bio, cx),
            author_links,
            ..base.clone()
        };
        *busy = true;
        *error = None;
        cx.notify();
        let backend = self.backend.clone();
        let task = cx.background_spawn(async move { backend.save_settings(&settings) });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |v, window, cx| match r {
                Ok(()) => {
                    v.close_reading_sheet(window, cx);
                    v.show_toast(
                        "Site settings saved",
                        Some("Your blyg shows them now".into()),
                        cx,
                    );
                }
                Err(e) => {
                    if let Some(RSheet::Site { error, busy, .. }) = v.reading.sheet.as_mut() {
                        *busy = false;
                        *error = Some(format!("Couldn't save: {e}"));
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_site_sheet(&self, sheet: &RSheet, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let RSheet::Site {
            title,
            name,
            bio,
            links,
            load,
            error,
            busy,
            focus,
        } = sheet
        else {
            return div().into_any_element();
        };
        let label = |s: &'static str| {
            div()
                .mt(px(10.))
                .mb(px(4.))
                .text_size(px(10.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.muted)
                .child(s)
        };
        let frame = div()
            .track_focus(focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_reading_sheet(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.close_reading_sheet(window, cx);
            }))
            .child(self.sheet_heading("Site settings"))
            .child(
                div()
                    .text_color(p.muted)
                    .child("What your blyg shows about itself. Saved to the blyg, not this Mac."),
            );
        match load {
            Load::Unavailable => {
                return frame
                    .child(
                        div()
                            .mt(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(super::NOT_AVAILABLE),
                    )
                    .child(
                        div()
                            .mt(px(4.))
                            .text_color(p.muted)
                            .child(super::NOT_AVAILABLE_SUB),
                    )
                    .child(self.keys_row(vec![self.key_hint("esc", "close")]))
                    .into_any_element();
            }
            Load::Failed(e) => {
                return frame
                    .child(
                        div()
                            .mt(px(12.))
                            .text_color(p.over)
                            .child(format!("Couldn't load them: {e}")),
                    )
                    .child(self.keys_row(vec![self.key_hint("esc", "close")]))
                    .into_any_element();
            }
            Load::Idle | Load::Loading => {
                return frame
                    .child(div().mt(px(12.)).text_color(p.muted).child("Loading…"))
                    .into_any_element();
            }
            Load::Ready(_) => {}
        }
        frame
            .child(label("SITE TITLE"))
            .child(self.input_box(
                gpui_kit::base::input::Input::new(title).into_any_element(),
                false,
            ))
            .child(label("AUTHOR"))
            .child(self.input_box(
                gpui_kit::base::input::Input::new(name).into_any_element(),
                false,
            ))
            .child(label("BIO"))
            .child(self.input_box(
                gpui_kit::base::input::Input::new(bio).into_any_element(),
                false,
            ))
            .child(label("LINKS · one per line: Label | https://…"))
            .child(
                div()
                    .h(px(84.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(if error.is_some() { p.over } else { p.line })
                    .text_size(px(13.))
                    .child(gpui_kit::base::input::Textarea::new(links)),
            )
            .when_some(error.clone(), |d, e| {
                d.child(div().mt(px(6.)).text_color(p.over).child(e))
            })
            .when(*busy, |d| {
                d.child(div().mt(px(6.)).text_color(p.muted).child("Saving…"))
            })
            .child(
                div()
                    .mt(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .text_color(p.muted)
                    .child(self.chip("site-save", "Save").on_click(
                        cx.listener(|this, _, window, cx| this.save_site_settings(window, cx)),
                    ))
                    .child(self.key_hint("⏎", "save (in a field)"))
                    .child(self.key_hint("esc", "cancel")),
            )
            .into_any_element()
    }
}
