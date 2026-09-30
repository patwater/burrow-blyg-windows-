//! Drawing the profile sheet (the mock's `.prof` panel) and the ⇧⌘O sheet.

use blyg_core::Profile;
use gpui_kit::base::input::{Escape, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::vm::{self, Button, Row, Tab};
use crate::app::{MainView, TITLEBAR_H};

const WIDTH: f32 = 440.;

#[derive(Clone, Copy, PartialEq)]
enum Look {
    Plain,
    Primary,
    Done,
}

impl MainView {
    fn pf_btn(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        look: Look,
    ) -> Stateful<Div> {
        let p = self.palette;
        let id: SharedString = id.into();
        let sel = id.to_string();
        div()
            .id(ElementId::Name(id))
            .debug_selector(move || sel.clone())
            .flex_none()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(7.))
            .border_1()
            .cursor_pointer()
            .font_family("Inter")
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .map(|d| match look {
                Look::Plain => d
                    .border_color(p.line)
                    .text_color(p.ink)
                    .hover(|s| s.border_color(p.accent)),
                Look::Primary => d.border_color(p.accent).text_color(p.accent),
                Look::Done => d.border_color(p.line).bg(p.sel).text_color(p.muted),
            })
            .child(label.into())
    }

    fn pf_avatar(&self, prof: &Profile, initial: &str) -> AnyElement {
        let p = self.palette;
        let circle = div()
            .size(px(52.))
            .flex_none()
            .rounded_full()
            .overflow_hidden();
        if let Some(url) = &prof.avatar
            && let crate::images::ImageState::Ready(path) = crate::images::state(url)
        {
            return circle
                .child(
                    img(ImageSource::Resource(Resource::Path(path.into())))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element();
        }
        circle
            .bg(p.accent.opacity(0.85))
            .flex()
            .items_center()
            .justify_center()
            .text_color(p.bg)
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(20.))
            .child(initial.to_string())
            .into_any_element()
    }

    pub(super) fn render_profile_sheet(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let focus = self
            .profiles
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let gen_ = self.profiles.gen_;
        let content = self.render_profile_content(cx);
        let panel = div()
            .id("profile-sheet")
            .debug_selector(|| "profile-sheet".into())
            .track_focus(&focus)
            .key_context("Profile")
            .on_key_down(cx.listener(Self::profile_key_down))
            .occlude()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(WIDTH))
            .max_w(relative(1.))
            .bg(p.bg)
            .border_l_1()
            .border_color(p.line)
            .shadow(vec![BoxShadow {
                color: p.shadow,
                offset: point(px(-18.), px(0.)),
                blur_radius: px(40.),
                spread_radius: px(-24.),
                inset: false,
            }])
            .overflow_y_scroll()
            .px(px(22.))
            .py(px(20.))
            .font_family("Inter")
            .text_size(px(13.))
            .text_color(p.ink)
            .child(content)
            .with_animation(
                ("profile-in", gen_),
                Animation::new(std::time::Duration::from_millis(220)).with_easing(ease_out_quint()),
                |d, t| d.right(px(-WIDTH * 1.05 * (1.0 - t))),
            );
        div()
            .absolute()
            .top(px(TITLEBAR_H))
            .left_0()
            .right_0()
            .bottom_0()
            .child(
                div()
                    .id("pf-scrim")
                    .absolute()
                    .size_full()
                    .bg(gpui_kit::black().opacity(if p.dark { 0.35 } else { 0.18 }))
                    .on_click(cx.listener(|this, _, window, cx| this.close_profile(window, cx))),
            )
            .child(panel)
            .into_any_element()
    }

    fn render_profile_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let Some(page) = self.profiles.page() else {
            return div().into_any_element();
        };
        let depth = self.profiles.stack.len();
        let back = (depth > 1).then(|| {
            self.pf_btn("pf-back", "‹", Look::Plain)
                .on_click(cx.listener(|this, _, window, cx| this.profile_back(window, cx)))
        });
        let close = self
            .pf_btn("pf-close", "esc", Look::Plain)
            .on_click(cx.listener(|this, _, window, cx| this.close_profile(window, cx)));
        let Some(prof) = page.profile.clone() else {
            let msg = match &page.error {
                Some(e) => format!("Couldn't open {}: {e}", vm::short(&page.url)),
                None => format!("Opening {}…", vm::short(&page.url)),
            };
            return div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .children(back)
                        .child(div().flex_1())
                        .child(close),
                )
                .child(div().text_color(p.muted).italic().child(msg))
                .into_any_element();
        };
        let subs = &self.reading.subs;
        let h = vm::header(&prof, subs);
        let serif: SharedString = self.prefs.writing().family.into();
        let follow_target = if prof.kind == blyg_core::ProfileKind::Feed {
            prof.feed_url.clone().unwrap_or(prof.origin.clone())
        } else {
            prof.origin.clone()
        };

        let head = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .children(back)
            .child(self.pf_avatar(&prof, &h.initial))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .id("pf-name")
                            .font_family(serif.clone())
                            .italic()
                            .text_size(px(21.))
                            .line_height(relative(1.1))
                            .truncate()
                            .child(h.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.muted)
                            .truncate()
                            .child(h.origin_line.clone()),
                    ),
            )
            .child(close);

        let bio = prof.bio.clone().map(|b| {
            div()
                .mt(px(12.))
                .mb(px(8.))
                .font_family(serif.clone())
                .text_size(px(15.))
                .line_height(relative(1.55))
                .child(b)
        });
        let links = (!prof.links.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(px(10.))
                .text_size(px(12.5))
                .text_color(p.muted)
                .children(prof.links.iter().enumerate().map(|(i, l)| {
                    let url = l.url.clone();
                    div()
                        .id(SharedString::from(format!("pf-link-{i}")))
                        .cursor_pointer()
                        .hover(|s| s.text_color(p.accent))
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .child(format!("{} ↗", l.label))
                }))
        });

        let buttons = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .mt(px(14.))
            .mb(px(6.))
            .children(h.buttons.iter().map(|&b| {
                let busy = match b {
                    Button::Follow => self.profiles.busy.contains(&follow_target),
                    Button::AddToBlogroll => self
                        .profiles
                        .busy
                        .contains(&format!("roll:{follow_target}")),
                    Button::Refresh => page.loading,
                    _ => false,
                };
                let look = match b {
                    Button::Follow => Look::Primary,
                    Button::Following | Button::InBlogroll => Look::Done,
                    _ => Look::Plain,
                };
                let label: SharedString = if busy && b != Button::Refresh {
                    format!("{}…", b.label()).into()
                } else {
                    b.label().into()
                };
                self.pf_btn(b.id(), label, look)
                    .when(b == Button::Refresh, |d| d.text_color(p.muted))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.profile_button(b, window, cx)),
                    )
            }));

        let tabs_list = vm::Tab::all_for(&prof);
        let tabs = div()
            .flex()
            .gap(px(16.))
            .mt(px(14.))
            .border_b_1()
            .border_color(p.line)
            .text_size(px(13.))
            .children(tabs_list.iter().map(|&t| {
                let on = t == page.tab;
                let id = format!("pf-tab-{}", t.label());
                div()
                    .id(SharedString::from(id.clone()))
                    .debug_selector(move || id.clone())
                    .py(px(6.))
                    .cursor_pointer()
                    .when(on, |d| {
                        d.text_color(p.ink).border_b_2().border_color(p.accent)
                    })
                    .when(!on, |d| d.text_color(p.muted))
                    .child(t.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_profile_tab(t, cx)))
            }));

        let rows = vm::rows(&prof, page.tab, subs, self.now);
        let list: AnyElement = if rows.is_empty() {
            div()
                .pt(px(12.))
                .text_size(px(12.5))
                .italic()
                .text_color(p.muted)
                .child(vm::empty_note(&prof, page.tab))
                .into_any_element()
        } else {
            div()
                .pt(px(4.))
                .children(
                    rows.into_iter()
                        .enumerate()
                        .map(|(i, r)| self.render_profile_row(i, r, page.sel == i, page.tab, cx)),
                )
                .into_any_element()
        };

        let note = div()
            .mt(px(14.))
            .text_size(px(12.))
            .text_color(p.muted)
            .child(if page.loading {
                format!("{} Checking for changes…", vm::source_note(&prof, self.now))
            } else {
                vm::source_note(&prof, self.now)
            });

        div()
            .flex()
            .flex_col()
            .child(head)
            .children(bio)
            .children(links)
            .child(buttons)
            .child(tabs)
            .child(list)
            .child(note)
            .when_some(page.error.clone().filter(|_| prof.stale), |d, e| {
                d.child(
                    div()
                        .mt(px(4.))
                        .text_size(px(12.))
                        .text_color(p.amber)
                        .child(e),
                )
            })
            .into_any_element()
    }

    fn render_profile_row(
        &self,
        i: usize,
        r: Row,
        selected: bool,
        tab: Tab,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let open = r.profile_url.clone().or(r.open_url.clone()).is_some();
        let body = div()
            .id(SharedString::from(format!("pf-row-{i}")))
            .flex_1()
            .min_w_0()
            .when(open, |d| d.cursor_pointer())
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(page) = this.profiles.stack.last_mut() {
                    page.sel = i;
                }
                this.activate_profile_row(i, window, cx)
            }))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .truncate()
                    .child(r.title.clone()),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(p.muted)
                    .truncate()
                    .child(r.sub.clone()),
            );
        let tag = r.tag.map(|t| {
            div()
                .flex_none()
                .px(px(4.))
                .rounded(px(4.))
                .border_1()
                .border_color(p.line)
                .text_size(px(9.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.muted)
                .child(t.to_uppercase())
        });
        let profile_btn = (tab != Tab::Posts)
            .then_some(r.profile_url.clone())
            .flatten()
            .map(|u| {
                self.pf_btn(format!("pf-row-{i}-profile"), "Profile", Look::Plain)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_profile(u.clone(), window, cx)
                    }))
            });
        let follow_btn = r.follow_url.clone().map(|u| {
            let busy = self.profiles.busy.contains(&u);
            let (label, look) = if r.following {
                ("Following ✓", Look::Done)
            } else if busy {
                ("Follow…", Look::Primary)
            } else {
                ("Follow", Look::Primary)
            };
            self.pf_btn(format!("pf-row-{i}-follow"), label, look)
                .on_click(cx.listener(move |this, _, _, cx| this.follow_url(u.clone(), cx)))
        });
        let toggle = r.blogroll_toggle.clone().map(|(id, on)| {
            self.pf_btn(
                format!("pf-row-{i}-blogroll"),
                if on {
                    "In blogroll ✓"
                } else {
                    "Add to blogroll"
                },
                if on { Look::Done } else { Look::Plain },
            )
            .on_click(
                cx.listener(move |this, _, _, cx| this.toggle_own_blogroll(id.clone(), !on, cx)),
            )
        });
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .py(px(9.))
            .px(px(6.))
            .mx(px(-6.))
            .rounded(px(6.))
            .border_b_1()
            .border_color(p.line)
            .when(selected, |d| d.bg(p.sel))
            .child(body)
            .children(tag)
            .children(profile_btn)
            .children(follow_btn)
            .children(toggle)
            .into_any_element()
    }

    // ------------------------------------------------------------ entry points

    /// An origin as clickable text (dotted underline) that opens its profile:
    /// the reading header's author address.
    pub(crate) fn profile_origin_link(
        &self,
        id: &'static str,
        origin: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let url = origin.to_string();
        div()
            .id(id)
            .debug_selector(move || id.to_string())
            .cursor_pointer()
            .text_color(p.ink)
            .border_b_1()
            .border_dashed()
            .border_color(p.muted.opacity(0.6))
            .hover(|s| s.text_color(p.accent).border_color(p.accent))
            .tooltip(|_, cx| {
                cx.new(|_| crate::app::reading::Tip("Profile · ⌘I".into()))
                    .into()
            })
            .on_click(
                cx.listener(move |this, _, window, cx| this.open_profile(url.clone(), window, cx)),
            )
            .child(vm::short(origin))
            .into_any_element()
    }

    /// The lineage line under a reading item's header: "↳ stub of …" and
    /// "⑂ forked from … vN 📌", each opening that origin's profile. Full
    /// width, so it wraps onto its own line inside the header row. The
    /// reading item's own fields win; `doc` (the opened item's fetched item
    /// document) fills the ones the owner API didn't send.
    pub(crate) fn render_lineage(
        &self,
        item: &blyg_core::ReadingItem,
        doc: Option<&blyg_core::Lineage>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = self.palette;
        let mut lineage = item.lineage();
        if let Some(doc) = doc {
            lineage = lineage.or(doc);
        }
        let parts = vm::lineage(&lineage, &self.reading.rows);
        if parts.is_empty() {
            return None;
        }
        Some(
            div()
                .id("pf-lineage")
                .w_full()
                .flex()
                .flex_wrap()
                .gap(px(14.))
                .text_size(px(12.5))
                .text_color(p.muted)
                .children(parts.into_iter().map(|l| {
                    let url = l.url.clone();
                    let id = l.id;
                    div().flex().gap(px(5.)).child(l.lead).child(
                        div()
                            .id(id)
                            .debug_selector(move || id.to_string())
                            .cursor_pointer()
                            .text_color(p.ink)
                            .border_b_1()
                            .border_dashed()
                            .border_color(p.muted.opacity(0.6))
                            .hover(|s| s.text_color(p.accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_profile(url.clone(), window, cx)
                            }))
                            .child(l.link),
                    )
                }))
                .into_any_element(),
        )
    }

    /// ⇧⌘O: paste any URL (a blyg, a permalink, a feed).
    pub(super) fn render_profile_ask(
        &mut self,
        input: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        input.update(cx, |s, _| {
            s.set_editor_style(gpui_kit::base::input::InputEditorStyle {
                foreground: p.ink,
                muted_foreground: p.muted,
                background: gpui_kit::transparent_black(),
                border: p.line,
                selection: p.text_selection,
                caret: p.accent,
                ..Default::default()
            });
        });
        let error = self.profiles.ask_error.clone();
        let kbd = |k: &'static str, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(
                    div()
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(5.))
                        .border_1()
                        .border_b_2()
                        .border_color(p.line)
                        .text_color(p.ink)
                        .text_size(px(11.5))
                        .child(k),
                )
                .child(label)
        };
        let sheet =
            div()
                .id("pf-ask")
                .debug_selector(|| "pf-ask".into())
                .occlude()
                .w(px(500.))
                .max_w(relative(0.92))
                .bg(p.bg)
                .border_1()
                .border_t_0()
                .border_color(p.line)
                .rounded_b(px(12.))
                .shadow(vec![BoxShadow {
                    color: p.shadow,
                    offset: point(px(0.), px(18.)),
                    blur_radius: px(40.),
                    spread_radius: px(-12.),
                    inset: false,
                }])
                .px(px(18.))
                .py(px(16.))
                .font_family("Inter")
                .text_size(px(13.))
                .text_color(p.ink)
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    cx.stop_propagation();
                    this.close_profile(window, cx);
                }))
                .child(
                    div()
                        .mb(px(8.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Open profile"),
                )
                .child(div().mb(px(8.)).text_color(p.muted).child(
                    "Paste a blyg, a post's address, or a feed. Fetched without your token.",
                ))
                .child(
                    div()
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(if error.is_some() { p.over } else { p.line })
                        .text_size(px(13.5))
                        .child(gpui_kit::base::input::Input::new(input)),
                )
                .when_some(error, |d, e| {
                    d.child(div().mt(px(6.)).text_color(p.over).child(e))
                })
                .child(
                    div()
                        .mt(px(12.))
                        .flex()
                        .gap(px(14.))
                        .text_color(p.muted)
                        .child(kbd("⏎", "open"))
                        .child(kbd("esc", "cancel")),
                );
        div()
            .absolute()
            .top(px(TITLEBAR_H))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(sheet)
            .into_any_element()
    }
}
