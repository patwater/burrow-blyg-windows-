//! --- notes --- The drawer's `MainView` hooks, the drawer itself and its
//! demo scenarios (`BLYGGER_DEMO=notes-…`).

use std::time::Duration;

use blyg_core::{Kind, LocalId, Promote, ReadingItem, Status, SubscriptionKind};
use gpui_kit::base::input::{
    Copy as CopyText, Enter, Escape, IndentInline, InputEditorStyle, InputEvent, MoveDown, MoveUp,
    Paste, Textarea, TextareaState,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{CONTEXT, PostRef, SLIDE_MS, TITLE, WIDTH};
use crate::app::scratch::MakeDraft;
use crate::app::{MainView, Publish, TITLEBAR_H};
use crate::composer::AssistKey;
use crate::theme::Rule;
use crate::vm; // --- themes --- dividers

gpui_kit::actions!(
    blygger,
    [
        /// ⇧⌘N: open or close the notes drawer; with text selected in a
        /// post or page on screen, quote it into the notes.
        ToggleNotes
    ]
);

/// --- onboarding --- The user's notes while the tutorial runs.
pub(crate) struct NotesPark {
    id: Option<LocalId>,
    resolved: bool,
    /// `state.json`'s `notes_note` before the tour.
    stored: Option<String>,
}

/// Where a selection is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelSource {
    /// The reading pane's web view (the open post).
    Reader,
    /// The browser pane's page.
    Browser,
}

/// What to do when there's no selection.
pub(crate) enum Fallback {
    /// Just open the drawer.
    Open,
    /// Add this post (`→ Notes` on a post).
    Post(Box<ReadingItem>),
    /// Append this block (the browser's `[title](url)`).
    Block(String),
}

/// The footer while a passage is selected in the notes (a click quotes it).
pub const QUOTE_HINT: &str = "⇧⌘D quotes the selection in your draft";

/// The script that reads the selection (the result comes back as JSON).
pub const SELECTION_JS: &str =
    "(function(){var s=window.getSelection();return s?String(s).slice(0,4000):\"\"})()";

impl MainView {
    /// Hook: the drawer's actions on the root element.
    pub(crate) fn notes_actions(&self, d: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        d.on_action(cx.listener(Self::toggle_notes))
            .on_action(cx.listener(Self::quote_to_draft))
            // --- selection --- ⌘C with a passage selected in the reading
            // pane's post while the keyboard is on the list (a click in the
            // post hands it back): copy that passage.
            .on_action(cx.listener(|this, _: &CopyText, _, cx| this.copy_reader_selection(cx)))
    }

    /// ⌘C for the reading pane's post (see `notes_actions`). Nothing to copy
    /// when the page has no selection.
    fn copy_reader_selection(&mut self, cx: &mut Context<Self>) {
        if !(self.studio.reader.active() && self.studio.reader.has_selection()) {
            return;
        }
        let (tx, rx) = async_channel::bounded::<String>(1);
        if !self.studio.reader.selection(tx) {
            return;
        }
        cx.spawn(async move |_, cx| {
            let text = rx.recv().await.unwrap_or_default();
            if !text.is_empty() {
                cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text)));
            }
        })
        .detach();
    }

    /// ⇧⌘N / View › Notes.
    pub(crate) fn toggle_notes(
        &mut self,
        _: &ToggleNotes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.notes.open {
            self.close_notes(window, cx);
            return;
        }
        let src = self.notes_selection_source();
        self.notes_ask_selection(src, Fallback::Open, window, cx);
    }

    /// Which web view on screen a selection would come from: the browser
    /// pane (it's on top), else the reading pane's post.
    fn notes_selection_source(&self) -> Option<SelSource> {
        if self.browser.open {
            Some(SelSource::Browser)
        } else if self.studio.reader.active() && self.reading.opened.is_some() {
            Some(SelSource::Reader)
        } else {
            None
        }
    }

    /// Read the selection in `src` (a small script in the web view); a
    /// non-empty one is quoted with its source link, otherwise `then` runs.
    pub(crate) fn notes_ask_selection(
        &mut self,
        src: Option<SelSource>,
        then: Fallback,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notes.outside_click = false; // an add action, not a click away
        let Some(src) = src else {
            return self.notes_fallback(then, window, cx);
        };
        let (title, url) = match src {
            SelSource::Browser => (
                self.browser.page.title.clone(),
                Some(self.browser.page.url.clone())
                    .filter(|u| super::super::browser::is_web_url(u)),
            ),
            SelSource::Reader => match self.reading.opened.as_ref() {
                Some(o) => (
                    post_title(&o.item),
                    crate::app::reading::vm::web_url(&o.item),
                ),
                None => return self.notes_fallback(then, window, cx),
            },
        };
        let (tx, rx) = async_channel::bounded::<String>(1);
        let asked = match src {
            SelSource::Browser => self.browser.selection(tx.clone()),
            SelSource::Reader => self.studio.reader.selection(tx.clone()),
        };
        if !asked {
            return self.notes_fallback(then, window, cx);
        }
        // A page that never answers mustn't make the key look dead.
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = tx.try_send(String::new());
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let text = rx.recv().await.unwrap_or_default();
            let _ = this.update_in(cx, |v, window, cx| {
                if text.trim().is_empty() {
                    v.notes_fallback(then, window, cx);
                } else {
                    let block = super::quote_with_source(&text, &title, url.as_deref());
                    v.notes_append(&block, window, cx);
                }
            });
        })
        .detach();
    }

    fn notes_fallback(&mut self, then: Fallback, window: &mut Window, cx: &mut Context<Self>) {
        match then {
            Fallback::Open => self.open_notes(window, cx),
            Fallback::Post(item) => self.notes_add_post_now(&item, window, cx),
            Fallback::Block(b) => self.notes_append(&b, window, cx),
        }
    }

    /// Hook: "→ Notes" on a post. From the reading pane (`from_pane`), a
    /// selection in the post's web view is quoted instead.
    pub(crate) fn notes_add_post(
        &mut self,
        item: ReadingItem,
        from_pane: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let src = (from_pane && self.studio.reader.active()).then_some(SelSource::Reader);
        self.notes_ask_selection(src, Fallback::Post(Box::new(item)), window, cx);
    }

    fn notes_add_post_now(
        &mut self,
        item: &ReadingItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blyg = self
            .reading
            .subs
            .iter()
            .find(|s| s.id == item.subscription_id)
            .is_none_or(|s| s.kind == SubscriptionKind::Blyg);
        let post = post_ref(item, blyg);
        self.notes_ensure(window, cx);
        let takes_quotes = self.notes_takes_quotes();
        let block = super::post_block(&post, takes_quotes);
        self.notes_append(&block, window, cx);
    }

    /// Hook: the browser's "→ Notes": the page's `[title](url)`, or the
    /// selection on it, quoted with that link.
    pub(crate) fn notes_add_page(
        &mut self,
        link: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notes_ask_selection(Some(SelSource::Browser), Fallback::Block(link), window, cx);
    }

    /// The note may hold `![[id]]` quotes: it's a thread (a new page is one).
    fn notes_takes_quotes(&self) -> bool {
        self.notes
            .id
            .as_ref()
            .and_then(|id| self.backend.item(id))
            .is_none_or(|i| i.kind == Kind::Thread)
    }

    /// Append `block` to the notes and open the drawer with the caret on a
    /// new line below it.
    pub(crate) fn notes_append(
        &mut self,
        block: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notes.outside_click = false;
        self.open_notes(window, cx);
        let Some(editor) = self.notes.editor.clone() else {
            return;
        };
        let old = editor.read(cx).value().to_string();
        let (new, caret) = super::append(&old, block);
        let (range, insert) = vm::splice(&old, &new);
        self.notes.loading = true;
        editor.update(cx, |s, cx| {
            s.set_selected_range(range.clone(), cx);
            s.replace(insert.to_string(), window, cx);
            let c = caret.min(s.text().len());
            s.set_selected_range(c..c, cx);
        });
        self.notes.loading = false;
        self.notes_save(window, cx);
        cx.notify();
    }

    // ------------------------------------------------------------ open / close

    /// Make the editor, its assist and the focus handle (once), and find
    /// the note.
    fn notes_ensure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notes.focus.is_none() {
            self.notes.focus = Some(cx.focus_handle());
        }
        if self.notes.editor.is_none() {
            let editor = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .soft_wrap(true)
                    .placeholder("Notes as you read…")
            });
            // --- composer --- @-mentions and spellcheck, as in quick capture.
            let assist = {
                let (e, backend) = (editor.clone(), self.backend.clone());
                let p = self.palette;
                cx.new(|cx| crate::composer::Assist::new(e, backend, p, window, cx))
            };
            self._subs.push(cx.subscribe_in(
                &editor,
                window,
                |this, _, ev: &InputEvent, window, cx| {
                    if matches!(ev, InputEvent::Change) && !this.notes.loading {
                        this.notes_save(window, cx);
                    }
                },
            ));
            self.notes.editor = Some(editor);
            self.notes.assist = Some(assist);
            self.notes_resolve();
            self.notes_load_text(window, cx);
        }
    }

    /// Find the note: the one remembered in `state.json`, else the newest
    /// scratch note titled "Reading notes…". Once per session.
    fn notes_resolve(&mut self) {
        if self.notes.resolved {
            return;
        }
        self.notes.resolved = true;
        let stored = self
            .notes
            .data_dir
            .as_deref()
            .map(blyg_core::state::AppState::load)
            .and_then(|s| s.notes_note)
            .map(LocalId);
        let usable = |id: &LocalId| {
            self.backend
                .item(id)
                .is_some_and(|i| i.status == Status::Scratch)
        };
        self.notes.id = stored.filter(usable).or_else(|| {
            self.backend
                .items()
                .into_iter()
                .find(|i| i.status == Status::Scratch && super::is_notes_title(&i.title()))
                .map(|i| i.local_id)
        });
    }

    fn notes_remember(&self) {
        if let Some(d) = &self.notes.data_dir {
            let id = self.notes.id.as_ref().map(|i| i.0.clone());
            let _ = blyg_core::state::AppState::update(d, |s| s.notes_note = id);
        }
    }

    /// Put the note's stored text in the editor (a new page's heading when
    /// there's no note yet).
    fn notes_load_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.notes.editor.clone() else {
            return;
        };
        let text = self
            .notes
            .id
            .as_ref()
            .and_then(|id| self.backend.item(id))
            .map(|i| i.content_md)
            .unwrap_or_else(|| super::page_seed(TITLE));
        if editor.read(cx).value().as_ref() != text {
            self.notes.loading = true;
            editor.update(cx, |s, cx| s.set_value(text, window, cx));
            self.notes.loading = false;
            if let Some(a) = &self.notes.assist {
                a.update(cx, |a, cx| a.reset(cx));
            }
        }
    }

    /// Show the drawer, with the keyboard in it and the caret at the end.
    pub(crate) fn open_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.notes_ensure(window, cx);
        if !self.notes.open {
            // Promoted, deleted or edited elsewhere since: follow the store.
            if let Some(id) = self.notes.id.clone()
                && self
                    .backend
                    .item(&id)
                    .is_none_or(|i| i.status != Status::Scratch)
            {
                self.notes.id = None;
            }
            self.notes_load_text(window, cx);
            let inside = self
                .notes
                .focus
                .as_ref()
                .is_some_and(|f| f.contains_focused(window, cx));
            if !inside {
                self.notes.return_focus = window.focused(cx);
            }
            if !self.notes.closing {
                self.notes.shown += 1;
            }
            self.notes.moved = Some(cx.background_executor().now());
        }
        self.notes.open = true;
        self.notes.closing = false;
        self.notes.close_gen += 1;
        if let Some(editor) = self.notes.editor.clone() {
            editor.update(cx, |s, cx| {
                s.focus(window, cx);
                let end = s.text().len();
                s.set_selected_range(end..end, cx);
            });
        }
        cx.notify();
    }

    /// esc, ⇧⌘N or a click outside: slide it back. The keyboard goes back
    /// where it was, if the drawer had it.
    pub(crate) fn close_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.notes.open {
            return;
        }
        let had_keyboard = self
            .notes
            .focus
            .as_ref()
            .is_some_and(|f| f.contains_focused(window, cx));
        self.notes.open = false;
        self.notes.closing = true;
        self.notes.moved = Some(cx.background_executor().now());
        self.notes.menu = false;
        self.notes.close_gen += 1;
        let gen_ = self.notes.close_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SLIDE_MS))
                .await;
            let _ = this.update(cx, |v, cx| {
                if v.notes.close_gen == gen_ {
                    v.notes.closing = false;
                    cx.notify();
                }
            });
        })
        .detach();
        if had_keyboard {
            match self.notes.return_focus.take() {
                Some(f) => window.focus(&f, cx),
                None if self.reading.view != crate::app::reading::View::Posts => {
                    window.focus(&self.reading.focus, cx)
                }
                None => window.focus(&self.focus, cx),
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------ onboarding

    /// --- onboarding --- The tutorial starts: the drawer goes away at once,
    /// and forgets the user's note so the tour's drawer writes to the sample
    /// data. Hand the result back to [`Self::notes_tour_restore`].
    pub(crate) fn notes_tour_park(&mut self, cx: &mut Context<Self>) -> NotesPark {
        let park = NotesPark {
            id: self.notes.id.take(),
            resolved: std::mem::replace(&mut self.notes.resolved, false),
            stored: self
                .notes
                .data_dir
                .as_deref()
                .map(blyg_core::state::AppState::load)
                .and_then(|s| s.notes_note),
        };
        self.notes_tour_hide(cx);
        park
    }

    /// --- onboarding --- The tour is over: the drawer hidden and the user's
    /// note back (in memory and in `state.json`).
    pub(crate) fn notes_tour_restore(&mut self, park: NotesPark, cx: &mut Context<Self>) {
        self.notes_tour_hide(cx);
        self.notes.id = park.id;
        self.notes.resolved = park.resolved;
        if let Some(d) = &self.notes.data_dir {
            let stored = park.stored;
            let _ = blyg_core::state::AppState::update(d, |s| s.notes_note = stored);
        }
    }

    /// Hide the drawer without sliding (the keyboard is the caller's).
    pub(crate) fn notes_tour_hide(&mut self, cx: &mut Context<Self>) {
        self.notes.open = false;
        self.notes.closing = false;
        self.notes.menu = false;
        self.notes.moved = None;
        self.notes.close_gen += 1;
        cx.notify();
    }

    // ------------------------------------------------------------ saving

    /// Autosave (every edit, like scratch notes in the editor): the first
    /// real text makes the note.
    fn notes_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.notes.text(cx);
        match self.notes.id.clone() {
            Some(id) => {
                if let Err(e) = self.backend.save(&id, &text) {
                    self.show_toast(format!("Couldn't save your notes: {e}"), None, cx);
                    return;
                }
                if let Some(fresh) = self.backend.item(&id) {
                    self.list.update_item(fresh.clone());
                    if self.current.as_ref().map(|c| &c.local_id) == Some(&id) {
                        self.current = Some(fresh);
                        if self.mode == crate::app::Mode::Search {
                            self.load_current_into_editor(window, cx);
                        }
                    }
                }
            }
            None => {
                let seed = super::page_seed(TITLE);
                if text.trim().is_empty() || text.trim() == seed.trim() {
                    return;
                }
                match self.backend.create_scratch(Kind::Thread, &text) {
                    Ok(id) => {
                        self.notes.id = Some(id);
                        self.notes_remember();
                        self.requery(window, cx);
                    }
                    Err(e) => self.show_toast(format!("Couldn't save your notes: {e}"), None, cx),
                }
            }
        }
    }

    /// ⌘D in the drawer: the notes become a draft on the blyg (the same
    /// item); the drawer starts a new page.
    fn notes_make_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.notes_save(window, cx);
        let Some(item) = self.notes.id.as_ref().and_then(|id| self.backend.item(id)) else {
            self.show_toast("Nothing to save as a draft yet", None, cx);
            return;
        };
        if let Some(msg) = vm::make_draft_blocked(&item) {
            self.show_toast(msg, None, cx);
            return;
        }
        let backend = self.backend.clone();
        let id = item.local_id.clone();
        let task = cx.background_spawn(async move { backend.promote(&id, Promote::Draft) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                match result {
                    Ok(_) => {
                        v.notes.id = None;
                        v.notes_remember();
                        v.notes_load_text(window, cx);
                        v.requery(window, cx);
                        v.show_toast(
                            format!("“{}” is now a draft on your blyg", item.title()),
                            Some("The drawer starts a new page".into()),
                            cx,
                        );
                    }
                    Err(e) => v.show_toast(format!("Couldn't make a draft: {e}"), None, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ⋯ › New notes page: a fresh scratch note; the old one stays in Posts.
    pub(super) fn notes_new_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.notes.menu = false;
        self.notes_save(window, cx);
        let title = super::fresh_title(chrono::Local::now().date_naive());
        match self
            .backend
            .create_scratch(Kind::Thread, &super::page_seed(&title))
        {
            Ok(id) => {
                let had_notes = self.notes.id.is_some();
                self.notes.id = Some(id);
                self.notes_remember();
                self.notes_load_text(window, cx);
                self.requery(window, cx);
                self.open_notes(window, cx);
                self.show_toast(
                    format!("New notes page: {title}"),
                    had_notes.then(|| "The last one is in Posts, as a scratch note".into()),
                    cx,
                );
            }
            Err(e) => self.show_toast(format!("Couldn't start a page: {e}"), None, cx),
        }
    }

    /// "Open in editor": the note in the Posts editor (the drawer closes).
    fn notes_open_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.notes.menu = false;
        self.notes_save(window, cx);
        if self.notes.id.is_none() {
            let text = self.notes.text(cx);
            let text = if text.trim().is_empty() {
                super::page_seed(TITLE)
            } else {
                text
            };
            match self.backend.create_scratch(Kind::Thread, &text) {
                Ok(id) => {
                    self.notes.id = Some(id);
                    self.notes_remember();
                }
                Err(e) => {
                    self.show_toast(format!("Couldn't save your notes: {e}"), None, cx);
                    return;
                }
            }
        }
        let Some(id) = self.notes.id.clone() else {
            return;
        };
        self.notes.return_focus = None;
        self.close_notes(window, cx);
        if self.browser.open {
            self.close_browser(window, cx);
        }
        self.open_new_draft(&id, window, cx);
    }

    /// A paste of a link over selected text makes a Markdown link (as in
    /// the editor).
    fn notes_paste_link(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.notes.editor.clone() else {
            return false;
        };
        let Some(clip) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return false;
        };
        let (text, sel) = {
            let s = editor.read(cx);
            (s.value().to_string(), s.selected_range())
        };
        let Some((new, caret)) = vm::link_paste(&text, sel, &clip) else {
            return false;
        };
        let (range, insert) = vm::splice(&text, &new);
        editor.update(cx, |s, cx| {
            s.set_selected_range(range.clone(), cx);
            s.replace(insert.to_string(), window, cx);
            let c = caret.min(s.text().len());
            s.set_selected_range(c..c, cx);
        });
        self.notes_save(window, cx);
        true
    }

    /// Give a popup key to the mention popup or the spelling menu if one is
    /// up; `true` when it took the key.
    fn notes_route_key(
        &mut self,
        key: AssistKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(assist) = self.notes.assist.clone() else {
            return false;
        };
        let took = assist.read(cx).wants_keys()
            && assist.update(cx, |a, cx| a.handle_key(key, window, cx));
        if took {
            cx.stop_propagation();
        }
        took
    }

    // ------------------------------------------------------------ render

    /// Per frame, before layout: the editor follows the palette and fonts.
    pub(crate) fn notes_frame(&mut self, cx: &mut Context<Self>) {
        let p = self.palette;
        if let Some(a) = &self.notes.assist {
            let font = self.prefs.ui().family;
            a.update(cx, |a, _| {
                a.set_palette(p);
                a.set_ui_font(font);
            });
        }
        if let Some(e) = &self.notes.editor {
            e.update(cx, |s, _| {
                s.set_editor_style(InputEditorStyle {
                    foreground: p.ink,
                    muted_foreground: p.placeholder(),
                    background: p.bg,
                    border: p.edge(),
                    selection: p.text_selection,
                    caret: p.caret(),
                    ..Default::default()
                });
                s.set_editor_paddings(Edges {
                    top: px(12.),
                    bottom: px(24.),
                    left: px(18.),
                    right: px(18.),
                });
            });
        }
    }

    /// The drawer, over the right edge of everything below the title bar.
    pub(crate) fn render_notes_drawer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let n = &self.notes;
        if !n.drawn() {
            return None;
        }
        let editor = n.editor.clone()?;
        let assist = n.assist.clone()?;
        let focus = n.focus.clone()?;
        let p = self.palette;
        let closing = n.closing;
        let body_font: SharedString = self.prefs.writing().family.into();
        let text = editor.read(cx).value().to_string();
        let title = blyg_core::plain_title(&text).unwrap_or_else(|| TITLE.to_string());
        let theme = &*self.theme;
        let chip = |id: &'static str, label: &'static str, tip: &'static str| {
            div()
                .id(id)
                .flex_none()
                .px(px(7.))
                .h(px(22.))
                .flex()
                .items_center()
                .map(|d| crate::theme_ext::chip(d, theme, false))
                .text_size(px(11.5))
                .text_color(p.ink)
                .cursor_pointer()
                .hover(|s| s.bg(p.hover()))
                .tooltip(move |_, cx| cx.new(|_| crate::app::reading::Tip(tip.into())).into())
                .child(label)
        };
        let header = div()
            .id("notes-header")
            .relative()
            .flex_none()
            .px(px(14.))
            .pt(px(10.))
            .pb(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .rule_b(&p)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(px(5.))
                            .rounded(px(theme.chip_radius.min(4.)))
                            .bg(p.pick())
                            .text_size(px(10.5))
                            .text_color(p.muted)
                            .child("scratch"),
                    )
                    .child(
                        chip("notes-menu", "⋯", "More: a new notes page")
                            .border_color(transparent_black())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notes.menu = !this.notes.menu;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        chip(
                            "notes-draft",
                            "⌘D → draft",
                            "Make these notes a draft on your blyg (the drawer starts a new page)",
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| this.notes_make_draft(window, cx)),
                        ),
                    )
                    .child(
                        chip(
                            "notes-open-editor",
                            "Open in editor",
                            "Edit these notes in the Posts editor",
                        )
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.notes_open_in_editor(window, cx)
                            }),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(p.muted)
                            .child("only on this Mac"),
                    ),
            )
            .when(n.menu, |d| {
                d.child(
                    div()
                        .id("notes-menu-pop")
                        .absolute()
                        .top(px(34.))
                        .right(px(10.))
                        .py(px(4.))
                        .min_w(px(180.))
                        .map(|d| crate::theme_ext::popover(d, theme))
                        .text_size(px(12.5))
                        .child(
                            div()
                                .id("notes-menu-new")
                                .px(px(10.))
                                .py(px(5.))
                                .cursor_pointer()
                                .hover(|s| s.bg(p.hover()))
                                .child("New notes page")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.notes_new_page(window, cx)
                                })),
                        )
                        .child(
                            div()
                                .id("notes-menu-editor")
                                .px(px(10.))
                                .py(px(5.))
                                .cursor_pointer()
                                .hover(|s| s.bg(p.hover()))
                                .child("Open in editor")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.notes_open_in_editor(window, cx)
                                })),
                        ),
                )
            });
        // --- selection --- a passage selected here: say how to quote it.
        let selected = self.notes_selected_passage(cx).is_some();
        let footer = div()
            .flex_none()
            .px(px(14.))
            .py(px(6.))
            .rule_t(&p)
            .text_size(px(11.))
            .text_color(p.muted)
            .map(|d| {
                if !selected {
                    return d.child("→ Notes on a post or page adds it · esc or ⇧⌘N closes");
                }
                d.child(
                    div()
                        .id("notes-quote-selection")
                        .debug_selector(|| "notes-quote-selection".into())
                        .cursor_pointer()
                        .text_color(p.accent_text())
                        .hover(|s| s.underline())
                        .child(QUOTE_HINT)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some((sel, source)) = this.notes_selected_passage(cx) {
                                this.quote_into_draft(sel, source, window, cx);
                            }
                        })),
                )
            });
        let w = WIDTH;
        Some(
            div()
                .id("notes-drawer")
                .debug_selector(|| "notes-drawer".into())
                .key_context(CONTEXT)
                .track_focus(&focus)
                .occlude()
                .absolute()
                .top(px(TITLEBAR_H))
                .bottom_0()
                .right_0()
                .w(px(WIDTH))
                .flex()
                .flex_col()
                .bg(p.bg)
                .text_color(p.ink)
                .rule_l(&p)
                .shadow_lg()
                .font_family(SharedString::from(self.prefs.ui().family))
                // --- composer --- the mention popup / spelling menu keys first.
                .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                    this.notes_route_key(AssistKey::Up, window, cx);
                }))
                .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                    this.notes_route_key(AssistKey::Down, window, cx);
                }))
                .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                    this.notes_route_key(AssistKey::Tab, window, cx);
                }))
                .capture_action(cx.listener(|this, a: &Enter, window, cx| {
                    if !a.secondary {
                        this.notes_route_key(AssistKey::Enter, window, cx);
                    }
                }))
                .capture_action(cx.listener(|this, _: &Paste, window, cx| {
                    if let Some(a) = &this.notes.assist {
                        a.update(cx, |a, _| a.note_paste());
                    }
                    if this.notes_paste_link(window, cx) {
                        cx.stop_propagation();
                    }
                }))
                .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                    if this.notes_route_key(AssistKey::Escape, window, cx) {
                        return;
                    }
                    cx.stop_propagation();
                    if this.notes.menu {
                        this.notes.menu = false;
                        cx.notify();
                    } else {
                        this.close_notes(window, cx);
                    }
                }))
                // esc with the keyboard on the drawer but not in its editor.
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    let k = &ev.keystroke;
                    let m = &k.modifiers;
                    if k.key == "escape" && !(m.platform || m.control || m.alt || m.shift) {
                        cx.stop_propagation();
                        this.close_notes(window, cx);
                    }
                }))
                // ⌘D and ⌘⏎ work on the notes, not the Posts selection.
                .on_action(
                    cx.listener(|this, _: &MakeDraft, window, cx| {
                        this.notes_make_draft(window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &Publish, window, cx| {
                    this.notes_open_in_editor(window, cx);
                    this.publish(window, cx);
                }))
                // A click outside closes it, once the click is over: a click
                // on an add action ("→ Notes") disarms this and keeps it open.
                .on_mouse_down_out(cx.listener(|this, _, _, _| {
                    this.notes.outside_click = this.notes.open;
                }))
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        if !this.notes.outside_click {
                            return;
                        }
                        // After this event's own handlers (the click) have run.
                        cx.spawn_in(window, async move |this, cx| {
                            let _ = this.update_in(cx, |v, window, cx| {
                                if v.notes.outside_click {
                                    v.notes.outside_click = false;
                                    v.close_notes(window, cx);
                                }
                            });
                        })
                        .detach();
                    }),
                )
                .child(header)
                .child(
                    div()
                        .id("notes-body")
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .font_family(body_font)
                        .text_size(px((self.prefs.font_size - 2.).max(12.)))
                        .line_height(relative(1.5))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(e) = this.notes.editor.clone() {
                                e.update(cx, |s, cx| s.focus(window, cx));
                            }
                        }))
                        .child(Textarea::new(&editor))
                        .child(assist),
                )
                .child(footer)
                .with_animation(
                    ElementId::NamedInteger(
                        if closing { "notes-out" } else { "notes-in" }.into(),
                        n.shown,
                    ),
                    Animation::new(Duration::from_millis(SLIDE_MS)).with_easing(ease_out_quint()),
                    move |d, t| {
                        let v = if closing { 1.0 - t } else { t };
                        d.right(px(-(1.0 - v) * w))
                    },
                )
                .into_any_element(),
        )
    }

    /// Hook: the browser pane inside a box whose right edge meets the
    /// drawer's left edge while the drawer is out, so the pane's chrome
    /// (🛡, ↗, → Notes, ×) stays uncovered. The box slides with the drawer.
    /// The box's inset follows the drawer's slide frame by frame (the
    /// drawer's own animation redraws the window meanwhile). It's computed
    /// here rather than animated with an element id, so the pane inside
    /// keeps its element state (its own slide-in never replays).
    pub(crate) fn notes_wrap_browser(&self, pane: AnyElement, cx: &App) -> AnyElement {
        let now = cx.background_executor().now();
        div()
            .debug_selector(|| "browser-room".into())
            .absolute()
            .top(px(TITLEBAR_H))
            .bottom_0()
            .left_0()
            .right(self.notes.room_at(now))
            .child(pane)
            .into_any_element()
    }

    // ------------------------------------------------------------ demos

    /// `BLYGGER_DEMO=notes-…` (snapshots): `notes-open` (the drawer over the
    /// stream, with some notes), `notes-after-add` (a blyg post and a feed
    /// post added from the stream), `notes-over-browser` (the drawer over
    /// the browser pane, its page added), `notes-posts` (over the editor).
    pub(crate) fn notes_demo(
        &mut self,
        scenario: &str,
        n: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::app::reading::View;
        use crate::app::reading::stream_vm::ReadMode;
        let typed = "Trust is a ledger nobody keeps on paper. Rue's post and Lin's garden \
                     note both circle it: write the one about benches.";
        // --- themes --- `notes-menu`: the drawer with its ⋯ menu open.
        if scenario == "notes-menu" {
            if n < 2 {
                self.notes_demo("notes-open", n, window, cx);
            }
            if n == 1 {
                self.notes.menu = true;
                cx.notify();
            }
            return;
        }
        match (scenario, n) {
            ("notes-open", 0) | ("notes-after-add", 0) => {
                self.reading.mode = ReadMode::Stream;
                self.show_view(View::Reading, window, cx);
            }
            ("notes-open", 1) => {
                self.stream_move(1, window, cx);
                self.open_notes(window, cx);
                let text = format!("{}{typed}\n", super::page_seed(TITLE));
                if let Some(e) = self.notes.editor.clone() {
                    e.update(cx, |s, cx| s.set_value(text, window, cx));
                }
                self.notes_save(window, cx);
            }
            ("notes-after-add", 1) => {
                use crate::fake::reading_seed::{LIN_GARDENS, RUE_TRUST};
                let rows = self.reading.rows.clone();
                for id in [RUE_TRUST, LIN_GARDENS] {
                    if let Some(r) = rows.iter().find(|r| r.remote_id == id) {
                        self.notes_add_post(r.clone(), false, window, cx);
                    }
                }
                // A feed post (RSS): a link.
                let rss = rows.iter().find(|r| {
                    self.reading
                        .subs
                        .iter()
                        .any(|s| s.id == r.subscription_id && s.kind == SubscriptionKind::Rss)
                });
                if let Some(r) = rss.cloned() {
                    self.notes_add_post(r, false, window, cx);
                }
            }
            ("notes-over-browser", 0) => {
                self.reading.mode = ReadMode::Reader;
                self.reading_demo("rd-reading", 0, window, cx);
            }
            ("notes-over-browser", 1) => {
                let url = std::env::var("BLYGGER_DEMO_URL")
                    .unwrap_or_else(|_| "https://blyg.example.com/".into());
                self.open_url_in_app(&url, crate::app::browser::OpenMode::Slide, window, cx);
                self.browser_send_to_notes(window, cx);
            }
            ("notes-posts", 0) => self.open(&LocalId("01J9QK3".into()), window, cx),
            ("notes-posts", 1) => {
                self.open_notes(window, cx);
                if let Some(e) = self.notes.editor.clone() {
                    e.update(cx, |s, cx| s.insert(typed.to_string(), window, cx));
                }
            }
            _ => {}
        }
        if n == 2 {
            crate::app::reading::demo::snapshot_later(window, cx);
        }
    }
}

/// A post's title for a link: a thread's heading; a fragment's first line
/// (shortened), or its blyg's name when it has no text.
fn post_title(r: &ReadingItem) -> String {
    let first = blyg_core::plain_title(&r.content_md).unwrap_or_default();
    let t = if first.chars().count() > 80 {
        let cut: String = first.chars().take(78).collect();
        format!("{}…", cut.trim_end())
    } else {
        first
    };
    if t.is_empty() {
        r.subscription_title.clone()
    } else {
        t
    }
}

/// What "→ Notes" needs from a reading post.
pub(crate) fn post_ref(r: &ReadingItem, blyg: bool) -> PostRef {
    let lines: Vec<String> = r
        .content_md
        .lines()
        .filter_map(blyg_core::plain_title)
        .collect();
    let first_line = match r.kind {
        // A thread's first line is its title: quote the next one.
        Kind::Thread => lines.get(1).cloned().unwrap_or_default(),
        Kind::Fragment => lines.first().cloned().unwrap_or_default(),
    };
    PostRef {
        blyg_id: blyg.then(|| r.remote_id.clone()),
        title: post_title(r),
        first_line,
        url: crate::app::reading::vm::web_url(r),
    }
}
