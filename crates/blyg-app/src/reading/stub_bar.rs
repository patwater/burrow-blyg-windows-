//! --- stub quotes --- The stub editor's quote line (studio 0.31): above a
//! stub draft, a hint names what the draft does with the post it answers
//! (the whole post, a passage, a running commentary, or a response by
//! link), with "quote whole post" and "quote a passage instead". The latter
//! shows the post's text, read-only; a selection there becomes the stub's
//! quote (`withStubQuote`), or another quote after the caret once there is
//! one (`addStubQuote`). The text logic is `blyg_render::stub_quote`.

use blyg_render::stub_quote::{
    StubQuoteForm, add_stub_quote, passage_count, stub_hint, stub_quote_form, with_stub_quote,
};
use gpui_kit::base::input::TextareaState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::MainView;

/// The passage chooser under the hint, while it's open.
pub struct Chooser {
    /// The stub draft it's for (it closes when another item opens).
    pub item: blyg_core::LocalId,
    /// The post's text, read-only, selectable. `None`: not held here.
    pub post: Option<Entity<TextareaState>>,
}

/// The post's text as the chooser shows it: its blocks as publish compares
/// them (`selection_text`), a blank line between, so any selection
/// normalizes (`normalize_selection`) to a substring publish will find.
pub fn chooser_text(content_html: &str) -> String {
    blyg_render::selection_text(content_html)
        .split('\n')
        .collect::<Vec<_>>()
        .join("\n\n")
}

impl MainView {
    /// The open draft when it's a stub thread of a blyg post: (its local
    /// id, the target's id, the target's origin).
    fn stub_target(&self) -> Option<(blyg_core::LocalId, String, String)> {
        let c = self.current.as_ref()?;
        if c.kind != blyg_core::Kind::Thread
            || !matches!(
                c.status,
                blyg_core::Status::Draft | blyg_core::Status::Scratch
            )
        {
            return None;
        }
        let s = c.stub_of.as_ref()?;
        Some((c.local_id.clone(), s.id.clone(), s.origin.clone()))
    }

    /// The held post a stub answers, by origin and id.
    fn stub_post_html(&self, origin: &str, id: &str) -> Option<String> {
        let key = blyg_core::post_key(origin, id);
        self.reading
            .rows
            .iter()
            .find(|r| blyg_core::post_key(&r.origin, &r.remote_id) == key)
            .map(|r| r.content_html.clone())
            .or_else(|| {
                self.backend
                    .reading()
                    .into_iter()
                    .find(|r| blyg_core::post_key(&r.origin, &r.remote_id) == key)
                    .map(|r| r.content_html)
            })
    }

    /// "quote a passage instead" / "done choosing".
    pub(crate) fn toggle_stub_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((item, id, origin)) = self.stub_target() else {
            return;
        };
        if self.reading.stub.as_ref().is_some_and(|c| c.item == item) {
            self.reading.stub = None;
            cx.notify();
            return;
        }
        let post = self.stub_post_html(&origin, &id).map(|html| {
            let text = chooser_text(&html);
            let post = cx.new(|cx| {
                let mut s = TextareaState::new(window, cx).soft_wrap(true);
                s.set_value(text, window, cx);
                s.set_readonly(true, cx);
                s
            });
            // Redraw as the selection changes (the buttons follow it).
            let sub = cx.observe(&post, |_, _, cx| cx.notify());
            self._subs.push(sub);
            post
        });
        self.reading.stub = Some(Chooser { item, post });
        cx.notify();
    }

    /// The text selected in the chooser, if any.
    fn stub_chosen(&self, cx: &App) -> Option<String> {
        let c = self.reading.stub.as_ref()?;
        let post = c.post.as_ref()?.read(cx);
        let r = post.selected_range();
        let text = post.value();
        let chosen = text.get(r)?.to_string();
        (!chosen.trim().is_empty()).then_some(chosen)
    }

    /// Put the chosen passage in the stub: `add` as another quote after the
    /// caret, else as the stub's (first) quote.
    pub(crate) fn quote_stub_passage(
        &mut self,
        add: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some((_, id, _)), Some(chosen)) = (self.stub_target(), self.stub_chosen(cx)) else {
            return;
        };
        let (text, cursor) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.cursor())
        };
        let new = if add {
            add_stub_quote(&text, &id, &chosen, cursor)
        } else {
            with_stub_quote(&text, &id, Some(&chosen))
        };
        self.reading.stub = None;
        self.splice_editor(&text, &new, Some(cursor.min(new.len())), window, cx);
        self.show_toast(
            if add {
                "Added another quote"
            } else {
                "Quoting the passage"
            },
            None,
            cx,
        );
    }

    /// "quote whole post": the stub's quote back to the whole post.
    pub(crate) fn quote_stub_whole(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, id, _)) = self.stub_target() else {
            return;
        };
        let (text, cursor) = {
            let s = self.editor.read(cx);
            (s.value().to_string(), s.cursor())
        };
        let new = with_stub_quote(&text, &id, None);
        self.splice_editor(&text, &new, Some(cursor.min(new.len())), window, cx);
    }

    /// Hook (the editor pane, above the text): the stub's hint line and
    /// passage chooser. `None` unless a stub draft is open.
    pub(crate) fn render_stub_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (item, id, _) = self.stub_target()?;
        let p = self.palette.on_page();
        let text = self.editor.read(cx).value().to_string();
        let form = stub_quote_form(&text, &id);
        let passages = passage_count(&text, &id);
        let open = self.reading.stub.as_ref().filter(|c| c.item == item);
        let link = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .cursor_pointer()
                .text_color(p.accent)
                .hover(|s| s.underline())
                .child(label)
        };
        let mut row = div()
            .id("stub-hint")
            .debug_selector(|| "stub-hint".into())
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(8.))
            .gap_y(px(2.))
            .child(div().text_color(p.muted).child(stub_hint(&text, &id)));
        if passages <= 1 && form != Some(StubQuoteForm::Whole) {
            row = row
                .child(link("stub-whole", "quote whole post").on_click(
                    cx.listener(|this, _, window, cx| this.quote_stub_whole(window, cx)),
                ));
        }
        row = row.child(
            link(
                "stub-choose",
                if open.is_some() {
                    "done choosing"
                } else if form == Some(StubQuoteForm::Passage) {
                    "quote another passage"
                } else {
                    "quote a passage instead"
                },
            )
            .on_click(cx.listener(|this, _, window, cx| this.toggle_stub_chooser(window, cx))),
        );
        let chooser = open.map(|c| {
            let chosen = self.stub_chosen(cx).is_some();
            let pill = |id: &'static str, label: &'static str, strong: bool| {
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(self.theme.chip_radius))
                    .border_1()
                    .border_color(if strong { p.accent } else { p.line })
                    .text_color(if strong { p.accent } else { p.ink })
                    .cursor_pointer()
                    .child(label)
            };
            div()
                .id("stub-chooser")
                .debug_selector(|| "stub-chooser".into())
                .mt(px(6.))
                .p(px(8.))
                .rounded(px(self.theme.radius))
                .border_1()
                .border_color(p.line)
                .bg(p.panel())
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().text_color(p.muted).child(
                    "Select the passage to quote in the post below. It must be one unbroken \
                     stretch of the original.",
                ))
                .child(match &c.post {
                    Some(post) => div()
                        .h(px(170.))
                        .text_size(px(13.))
                        .child(gpui_kit::base::input::Textarea::new(post))
                        .into_any_element(),
                    None => div()
                        .italic()
                        .text_color(p.muted)
                        .child(
                            "The post isn't held on this Mac, so there's nothing to choose from.",
                        )
                        .into_any_element(),
                })
                .when(chosen, |d| {
                    d.child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .when(passages > 0, |d| {
                                d.child(pill("stub-add", "❝ add as another quote", true).on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.quote_stub_passage(true, window, cx)
                                    }),
                                ))
                            })
                            .child(
                                pill(
                                    "stub-only",
                                    if passages > 0 {
                                        "replace the first quote"
                                    } else {
                                        "❝ quote only this"
                                    },
                                    passages == 0,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.quote_stub_passage(false, window, cx)
                                    },
                                )),
                            ),
                    )
                })
        });
        Some(
            div()
                .id("stub-bar")
                .flex_none()
                .px(px(18.))
                .pt(px(8.))
                .pb(px(6.))
                .font_family(self.chrome())
                .text_size(px(11.5))
                .line_height(relative(1.4))
                .child(row)
                .children(chooser)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::chooser_text;

    #[test]
    fn the_chooser_shows_the_blocks_publish_compares() {
        let html = "<p>One  line\nwrapped.</p><ul><li>An item</li></ul><p>Two.</p>";
        assert_eq!(chooser_text(html), "One line wrapped.\n\nAn item\n\nTwo.");
        // A selection across two blocks normalizes to what publish finds.
        let sel = "wrapped.\n\nAn item";
        let hay = blyg_render::selection_text(html);
        assert!(hay.contains(&blyg_render::normalize_selection(sel)));
    }
}
