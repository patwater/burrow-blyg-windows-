//! Versions: the `‹ vN ▾ ›` pill on someone else's post (current + pinned
//! only, spec §8.4), and your own post's full private history (⌘Y) with
//! restore and a type-to-confirm pin.

use blyg_core::{CoreError, LocalId, Version};
use gpui_kit::base::input::{Escape, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::vm::{self, Pill};
use super::{Load, OwnVersions, RSheet, View};
use crate::app::MainView;

impl MainView {
    /// The pill for the open reading item, from the shown versions only.
    pub(crate) fn pill_model(&self) -> Option<Pill> {
        let o = self.reading.opened.as_ref()?;
        let shown = o.shown.ready()?;
        vm::pill(shown, o.ix?, self.now)
    }

    pub(super) fn render_pill(
        &self,
        o: &super::Opened,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = self.palette;
        let pm = match &o.shown {
            Load::Loading => {
                return Some(div().child("versions…").into_any_element());
            }
            Load::Ready(_) => self.pill_model()?,
            _ => return None,
        };
        let pin_color: Hsla = gpui_kit::rgb(if p.dark { 0xe7c070 } else { 0x9a7414 }).into();
        let arrow = |id: &'static str, glyph: &'static str, enabled: bool, delta: isize| {
            div()
                .id(id)
                .w(px(22.))
                .h(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .text_color(p.ink)
                .when(!enabled, |d| d.opacity(0.25))
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.bg(p.sel))
                        .on_click(cx.listener(move |this, _, _, cx| this.step_version(delta, cx)))
                })
                .child(glyph)
        };
        let dropdown = o.dropdown.then(|| {
            div()
                .id("pill-dropdown")
                .occlude()
                .absolute()
                .top(px(22.))
                .left_0()
                .min_w(px(190.))
                .p(px(4.))
                .rounded(px(8.))
                .bg(p.bar)
                .border_1()
                .border_color(p.line)
                .shadow_lg()
                .children(pm.entries.iter().map(|(label, date, ix)| {
                    let ix = *ix;
                    div()
                        .id(("pill-entry", ix))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .flex()
                        .justify_between()
                        .gap(px(10.))
                        .cursor_pointer()
                        .hover(|s| s.bg(p.sel))
                        .text_color(p.ink)
                        .child(label.clone())
                        .child(div().text_color(p.muted).child(date.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| this.select_version(ix, cx)))
                }))
        });
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(2.))
                .p(px(1.))
                .rounded_full()
                .border_1()
                .border_color(p.line)
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(11.))
                .child(arrow("pill-older", "‹", pm.can_older, -1))
                .child(
                    div()
                        .id("pill-label")
                        .relative()
                        .px(px(8.))
                        .cursor_pointer()
                        .text_color(if pm.pinned { pin_color } else { p.accent })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(o) = this.reading.opened.as_mut() {
                                o.dropdown = !o.dropdown;
                            }
                            cx.notify();
                        }))
                        .child(pm.label.clone())
                        .children(dropdown.map(|d| deferred(d).with_priority(1))),
                )
                .child(arrow("pill-newer", "›", pm.can_newer, 1))
                .into_any_element(),
        )
    }

    // ------------------------------------------------------------ own posts

    /// ⌘Y: your post's history (Posts), or the version list (Reading).
    pub(super) fn toggle_versions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading.view == View::Reading {
            if let Some(o) = self.reading.opened.as_mut() {
                o.dropdown = !o.dropdown;
                cx.notify();
            }
            return;
        }
        if self.reading.view != View::Posts {
            return;
        }
        if self.reading.own.take().is_some() {
            self.focus_after_sheet(window, cx);
            cx.notify();
            return;
        }
        let Some(item) = self.current.clone() else {
            self.show_toast("Open a post first", None, cx);
            return;
        };
        if item.version == 0 || item.server_id.is_none() {
            self.show_toast("Not published yet, so there's no history", None, cx);
            return;
        }
        self.reading.own = Some(OwnVersions {
            id: item.local_id.clone(),
            list: Load::Loading,
            sel: Some(item.version),
        });
        window.focus(&self.focus, cx);
        self.load_own_versions(item.local_id, cx);
        cx.notify();
    }

    fn load_own_versions(&mut self, id: LocalId, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let id2 = id.clone();
        let task = cx.background_spawn(async move { backend.versions(&id2) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |v, cx| {
                if let Some(own) = v.reading.own.as_mut().filter(|o| o.id == id) {
                    own.list = match r {
                        Ok(list) => Load::Ready(list),
                        Err(e) => Load::Failed(e.to_string()),
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn own_selected(&self) -> Option<(LocalId, Version, u32)> {
        let own = self.reading.own.as_ref()?;
        let list = own.list.ready()?;
        let v = list.iter().find(|v| Some(v.version) == own.sel)?.clone();
        let current = self.backend.item(&own.id)?.version;
        Some((own.id.clone(), v, current))
    }

    pub(crate) fn ask_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, v, _)) = self.own_selected() else {
            return;
        };
        if v.pinned || v.endcap {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("type pin"));
        let sub = cx.subscribe_in(&input, window, |this, _, ev, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.confirm_pin(window, cx);
            }
        });
        self._subs.push(sub);
        input.update(cx, |s, cx| s.focus(window, cx));
        self.open_reading_sheet(
            RSheet::Pin {
                id,
                version: v.version,
                input,
                error: None,
                busy: false,
            },
            cx,
        );
    }

    /// ⏎ in the pin sheet: pins only when the typed word is exactly "pin".
    pub(crate) fn confirm_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(RSheet::Pin {
            id,
            version,
            input,
            error,
            busy,
        }) = self.reading.sheet.as_mut()
        else {
            return;
        };
        if *busy {
            return;
        }
        let typed = input.read(cx).value().to_string();
        if !vm::pin_confirmed(&typed) {
            *error = Some(format!("Type “{}” to confirm.", vm::PIN_WORD));
            cx.notify();
            return;
        }
        *busy = true;
        let (id, version) = (id.clone(), *version);
        let backend = self.backend.clone();
        let id2 = id.clone();
        let task = cx.background_spawn(async move { backend.pin(&id2, version) });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |v, window, cx| match r {
                Ok(()) => {
                    v.close_reading_sheet(window, cx);
                    v.show_toast(
                        format!("Pinned v{version}"),
                        Some("Served forever at its own address".into()),
                        cx,
                    );
                    v.load_own_versions(id, cx);
                }
                Err(e) => {
                    if let Some(RSheet::Pin { error, busy, .. }) = v.reading.sheet.as_mut() {
                        *busy = false;
                        *error = Some(format!("Couldn't pin: {e}"));
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn ask_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, v, current)) = self.own_selected() else {
            return;
        };
        if v.endcap || v.version == current {
            return;
        }
        let dirty = self.backend.item(&id).is_some_and(|i| i.dirty);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.open_reading_sheet(
            RSheet::Restore {
                id,
                version: v.version,
                next: current + 1,
                dirty,
                focus,
            },
            cx,
        );
    }

    fn confirm_restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(RSheet::Restore {
            id, version, next, ..
        }) = self.reading.sheet.take()
        else {
            return;
        };
        cx.notify();
        let backend = self.backend.clone();
        let id2 = id.clone();
        let task = cx.background_spawn(async move { backend.restore(&id2, version) });
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |v, window, cx| match r {
                Ok(()) => {
                    v.reading.own = None;
                    v.current = v.backend.item(&id);
                    v.load_current_into_editor(window, cx);
                    v.requery(window, cx);
                    v.open(&id, window, cx);
                    v.show_toast(
                        format!("v{version} is in the editor"),
                        Some(
                            format!("⌘⏎ publishes it as v{next} · versions never go backwards")
                                .into(),
                        ),
                        cx,
                    );
                }
                Err(CoreError::Offline) => {
                    v.focus_after_sheet(window, cx);
                    v.show_toast("Offline. Restore needs the blyg.", None, cx)
                }
                Err(e) => {
                    v.focus_after_sheet(window, cx);
                    v.show_toast(format!("Couldn't restore: {e}"), None, cx)
                }
            });
        })
        .detach();
    }

    /// Hook: the editor pane is replaced by the history while ⌘Y is open.
    pub(crate) fn render_own_versions(
        &self,
        body_font: &SharedString,
        size: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let own = self.reading.own.as_ref()?;
        let p = self.palette;
        let item = self.backend.item(&own.id)?;
        let host = crate::vm::url_host(self.base_url.as_deref().unwrap_or("")).unwrap_or_default();
        let kind = crate::vm::kind_label(item.kind);
        let pin_color: Hsla = gpui_kit::rgb(if p.dark { 0xe7c070 } else { 0x9a7414 }).into();

        let side: AnyElement = match &own.list {
            Load::Ready(list) => {
                let rows = vm::own_rows(list, item.version, self.now);
                let sel = own.sel;
                let actions = list
                    .iter()
                    .find(|v| Some(v.version) == sel)
                    .map(|v| (v.version, vm::own_actions(v, item.version)));
                div()
                    .flex()
                    .flex_col()
                    .children(rows.into_iter().map(|r| {
                        let v = r.version;
                        let on = Some(v) == sel;
                        div()
                            .id(("own-v", v as usize))
                            .flex()
                            .gap(px(8.))
                            .p(px(8.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .when(on, |d| d.bg(p.sel).border_l_2().border_color(p.accent))
                            .hover(|s| s.bg(p.sel))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(own) = this.reading.own.as_mut() {
                                    own.sel = Some(v);
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w(px(30.))
                                    .flex_none()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.muted)
                                    .child(format!("v{v}")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().italic().truncate().child(r.note))
                                    .child(
                                        div().text_size(px(11.)).text_color(p.muted).child(r.when),
                                    ),
                            )
                            .when(!r.badge.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .px(px(6.))
                                        .h(px(16.))
                                        .rounded_full()
                                        .border_1()
                                        .text_size(px(10.5))
                                        .border_color(if r.badge == "📌" {
                                            pin_color
                                        } else {
                                            p.accent
                                        })
                                        .text_color(if r.badge == "📌" {
                                            pin_color
                                        } else {
                                            p.accent
                                        })
                                        .child(r.badge),
                                )
                            })
                    }))
                    .when_some(actions, |d, (v, a)| {
                        d.child(
                            div()
                                .mt(px(12.))
                                .flex()
                                .flex_wrap()
                                .gap(px(6.))
                                // --- follow-ups --- the chips come from
                                // `OwnAction` only (no fork: forking is for
                                // other people's pins).
                                .children(a.list().into_iter().map(|act| {
                                    match act {
                                        vm::OwnAction::Pinned => div()
                                            .px(px(8.))
                                            .py(px(3.))
                                            .opacity(0.5)
                                            .text_size(px(11.5))
                                            .child(act.label(v))
                                            .into_any_element(),
                                        vm::OwnAction::Pin => self
                                            .chip("own-pin", act.label(v))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.ask_pin(window, cx)
                                            }))
                                            .into_any_element(),
                                        vm::OwnAction::Restore => self
                                            .chip("own-restore", act.label(v))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.ask_restore(window, cx)
                                            }))
                                            .into_any_element(),
                                    }
                                })),
                        )
                    })
                    .into_any_element()
            }
            Load::Failed(e) => self.muted_note(format!("Couldn't load the history: {e}")),
            _ => self.muted_note("Loading the history…"),
        };

        let sel = own.sel.unwrap_or(item.version);
        let meta = own
            .list
            .ready()
            .and_then(|l| l.iter().find(|v| v.version == sel).cloned());
        let is_current = sel == item.version;
        let doc_header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .mb(px(12.))
            .font_family("Inter")
            .text_size(px(12.))
            .text_color(p.muted)
            .child(format!("{host} · {kind}"))
            .when(is_current, |d| {
                d.child(
                    div()
                        .px(px(6.))
                        .rounded_full()
                        .border_1()
                        .border_color(p.accent)
                        .text_color(p.accent)
                        .child(format!("current · v{sel}")),
                )
            })
            .when(!is_current, |d| {
                d.child(format!(
                    "v{sel} · {}",
                    meta.as_ref()
                        .map(|m| crate::vm::relative_time(&m.published_at, self.now))
                        .unwrap_or_default()
                ))
            })
            .when(meta.as_ref().is_some_and(|m| m.pinned), |d| {
                d.child(
                    div()
                        .px(px(6.))
                        .rounded_full()
                        .border_1()
                        .border_color(pin_color)
                        .text_color(pin_color)
                        .child("📌 pinned, public forever"),
                )
            })
            .when(
                !is_current && meta.as_ref().is_some_and(|m| !m.pinned),
                |d| d.child("· private to you"),
            );
        // The current version renders as it publishes, in the reader's
        // WebView; older ones have no text until restored.
        let dirty_note = (is_current && item.dirty).then(|| {
            div()
                .mb(px(10.))
                .font_family("Inter")
                .text_size(px(11.5))
                .text_color(p.muted)
                .child("The working copy, with unpublished edits")
        });
        let body: AnyElement = if is_current && self.studio.reader.active() {
            self.reader_pane(&p)
        } else if is_current {
            div().flex_1().into_any_element()
        } else if meta.as_ref().is_some_and(|m| m.endcap) {
            div()
                .flex_1()
                .px(px(32.))
                .child(self.muted_note("The withdrawal marker: this version has no text."))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .px(px(32.))
                .child(self.muted_note(format!(
                    "Restore v{sel} to read its text: it goes into the editor, and nothing \
                     is published until you press ⌘⏎."
                )))
                .into_any_element()
        };

        Some(
            div()
                .id("own-versions")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    if this.reading.sheet.is_none() {
                        cx.stop_propagation();
                        this.reading.own = None;
                        this.focus_after_sheet(window, cx);
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .id("own-doc")
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .pt(px(22.))
                        .border_r_1()
                        .border_color(p.line)
                        .font_family(body_font.clone())
                        .text_size(px(size))
                        .line_height(relative(1.6))
                        .child(
                            div()
                                .flex_none()
                                .px(px(32.))
                                .child(doc_header)
                                .children(dirty_note),
                        )
                        .child(body),
                )
                .child(
                    div()
                        .id("own-side")
                        .w(px(290.))
                        .flex_none()
                        .overflow_y_scroll()
                        .p(px(14.))
                        .font_family("Inter")
                        .text_size(px(13.))
                        .child(
                            div()
                                .mb(px(10.))
                                .flex()
                                .justify_between()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.muted)
                                .child("VERSIONS")
                                .child("⌘Y / esc closes"),
                        )
                        .child(side),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_version_sheet(
        &self,
        sheet: &RSheet,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        match sheet {
            RSheet::Pin {
                version,
                input,
                error,
                busy,
                ..
            } => div()
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    cx.stop_propagation();
                    this.close_reading_sheet(window, cx);
                }))
                .child(self.sheet_heading(format!("Pin v{version} forever?")))
                .child(
                    div()
                        .line_height(relative(1.45))
                        .text_color(p.muted)
                        .child(
                            "This version will be served forever, at a permanent address, even \
                             if you edit or withdraw the post later. Others can quote and fork it.",
                        ),
                )
                .child(
                    div()
                        .mt(px(6.))
                        .mb(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("You can't undo this."),
                )
                .child(self.input_box(
                    gpui_kit::base::input::Input::new(input).into_any_element(),
                    error.is_some(),
                ))
                .when_some(error.clone(), |d, e| {
                    d.child(div().mt(px(6.)).text_color(p.over).child(e))
                })
                .when(*busy, |d| {
                    d.child(div().mt(px(6.)).text_color(p.muted).child("Pinning…"))
                })
                .child(self.keys_row(vec![
                    self.key_hint("pin ⏎", "type pin, then ⏎ to confirm"),
                    self.key_hint("esc", "cancel"),
                ]))
                .into_any_element(),
            RSheet::Restore {
                version,
                next,
                dirty,
                focus,
                ..
            } => div()
                .track_focus(focus)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    match ev.keystroke.key.as_str() {
                        "enter" => this.confirm_restore(window, cx),
                        "escape" => this.close_reading_sheet(window, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }))
                .child(self.sheet_heading(format!("Restore v{version}?")))
                .child(
                    div()
                        .line_height(relative(1.45))
                        .text_color(p.muted)
                        .child(format!(
                            "Its text goes back into the editor. Nothing is published until you \
                             press ⌘⏎, and then it publishes as v{next}. Versions never go backwards."
                        )),
                )
                .when(*dirty, |d| {
                    d.child(
                        div()
                            .mt(px(8.))
                            .text_color(p.over)
                            .child("Your unpublished edits in the editor will be replaced."),
                    )
                })
                .child(self.keys_row(vec![
                    self.key_hint("⏎", "restore to the editor"),
                    self.key_hint("esc", "cancel"),
                ]))
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }
}
