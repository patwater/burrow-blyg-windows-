//! The reading list (⌘R): one row per post, and the post on the right with
//! its version pill, "edited since you read it", thumbs and actions.

use blyg_core::{RemoteRef, SubscriptionKind};
use gpui_kit::base::input::{Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::vm::{self, Key};
use super::{Load, Opened};
use crate::app::MainView;
use crate::theme::Rule;
use crate::vm::DiffOp; // --- themes --- dividers

/// What the detail pane shows under the post (for tests and render alike).
#[derive(Debug, Clone, PartialEq)]
pub enum EditedBlock {
    /// Read at a pinned version: a real diff pinned → now.
    Diff {
        heading: String,
        runs: Vec<(DiffOp, String)>,
    },
    /// Read at an unpinned version: the author's notes only.
    Notes { heading: String, notes: Vec<String> },
}

impl MainView {
    /// Every reading row, in display order (a search doesn't change these).
    #[cfg(test)]
    pub(crate) fn reading_rows(&self) -> &[blyg_core::ReadingItem] {
        &self.reading.rows
    }

    /// Remote ids of the rows the list shows now (after any search).
    #[cfg(test)]
    pub(crate) fn reading_shown_ids(&self) -> Vec<String> {
        self.reading
            .shown_rows()
            .map(|r| r.remote_id.clone())
            .collect()
    }

    /// The "edited since you read it" block for the open post, if any.
    pub(crate) fn edited_block(&self) -> Option<EditedBlock> {
        let o = self.reading.opened.as_ref()?;
        let read = o.item.read_version?;
        if read >= o.item.version || o.item.state == "tombstone" {
            return None;
        }
        // A diff only from a pinned read version: both sides are public.
        if let Some(base) = o.diff_base.as_ref().filter(|b| b.version == read) {
            let (_, runs) = crate::vm::word_diff(&base.content_md, &o.item.content_md);
            return Some(EditedBlock::Diff {
                heading: vm::edited_heading(read, o.item.version, true),
                runs,
            });
        }
        let notes = o
            .changelog
            .ready()
            .map(|log| vm::notes_between(log, read, o.item.version))
            .unwrap_or_default();
        Some(EditedBlock::Notes {
            heading: vm::edited_heading(read, o.item.version, false),
            notes,
        })
    }

    // ------------------------------------------------------------ search

    /// Make the search field once there's a window (the first ⌘R).
    pub(super) fn ensure_reading_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading.search.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::keymap::hint("Search your reading…  (⌘F or /)"))
        });
        let sub = cx.subscribe_in(&input, window, |this, input, ev, window, cx| match ev {
            InputEvent::Change => {
                let q = input.read(cx).value().to_string();
                this.apply_reading_query(q, cx);
            }
            // ⏎: to the list, opening the first match if nothing is open.
            InputEvent::PressEnter { .. } => {
                if this.reading.sel.is_none() {
                    this.move_reading(1, window, cx);
                }
                window.focus(&this.reading.focus, cx);
                cx.notify();
            }
            _ => {}
        });
        self._subs.push(sub);
        self.reading.search = Some(input);
    }

    pub(super) fn focus_reading_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_reading_search(window, cx);
        if let Some(s) = &self.reading.search {
            s.update(cx, |s, cx| {
                s.focus(window, cx);
                s.select_all(window, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn reading_search_focused(&self, window: &Window, cx: &App) -> bool {
        self.reading
            .search
            .as_ref()
            .is_some_and(|s| s.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Set the search text (the field and the filter together).
    pub(crate) fn set_reading_query(
        &mut self,
        q: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = &self.reading.search {
            s.update(cx, |s, cx| s.set_value(q.to_string(), window, cx));
        }
        self.apply_reading_query(q.to_string(), cx);
    }

    /// Filter the list. The open post stays open while it still matches;
    /// otherwise the selection and the reader clear (nothing is opened, so
    /// nothing is marked read, just by typing). ↓ or ⏎ opens the first match.
    fn apply_reading_query(&mut self, q: String, cx: &mut Context<Self>) {
        if self.reading.query == q {
            return;
        }
        self.reading.query = q;
        self.reading.refilter();
        let keep = self
            .reading
            .sel
            .as_ref()
            .and_then(|k| self.reading.shown_pos(k));
        match keep {
            Some(ix) => {
                self.reading.stream.list.scroll_to_reveal_item(ix);
                self.reading
                    .list_scroll
                    .scroll_to_item(ix, ScrollStrategy::Nearest)
            }
            None => {
                self.reading.sel = None;
                self.reading.opened = None;
                self.reading
                    .list_scroll
                    .scroll_to_item(0, ScrollStrategy::Top);
            }
        }
        cx.notify();
    }

    /// The search field above the reading list.
    pub(super) fn render_reading_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        let Some(search) = self.reading.search.as_ref() else {
            return div().into_any_element();
        };
        div()
            .id("reading-search")
            .flex_none()
            .h(px(34.))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(14.))
            .rule_b(&p)
            .child(div().text_size(px(14.)).text_color(p.muted).child("⌕"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(self.prefs.ui().family)
                    .text_size(px(12.5))
                    .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                        cx.stop_propagation();
                        this.move_reading(-1, window, cx);
                    }))
                    .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                        cx.stop_propagation();
                        this.move_reading(1, window, cx);
                    }))
                    // esc clears the search; on an empty one it hands the
                    // keys back to the list.
                    .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                        cx.stop_propagation();
                        if this.reading.query.is_empty() {
                            window.focus(&this.reading.focus, cx);
                            cx.notify();
                        } else {
                            this.set_reading_query("", window, cx);
                        }
                    }))
                    .child(Input::new(search)),
            )
            .into_any_element()
    }

    pub(super) fn move_reading(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // --- stream --- the stream selects; the pane follows when open.
        if self.reading.mode == super::stream_vm::ReadMode::Stream {
            self.stream_move(delta, window, cx);
            return;
        }
        // ↑/↓ move through what the list shows (the search's matches).
        let n = self.reading.shown.len();
        if n == 0 {
            return;
        }
        let cur = self
            .reading
            .sel
            .as_ref()
            .and_then(|k| self.reading.shown_pos(k));
        let next = match cur {
            None => 0,
            Some(i) => (i as isize + delta).clamp(0, n as isize - 1) as usize,
        };
        if Some(next) == cur {
            return;
        }
        let Some(key) = self.reading.shown_rows().nth(next).map(vm::key) else {
            return;
        };
        self.reading
            .list_scroll
            .scroll_to_item(next, ScrollStrategy::Nearest);
        self.open_reading(key, window, cx);
    }

    /// Open a post: show it, fetch its versions (and the pinned diff base
    /// while the old read version still stands), then mark it read.
    pub(crate) fn open_reading(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self
            .reading
            .rows
            .iter()
            .find(|r| vm::key(r) == key)
            .cloned()
        else {
            return;
        };
        // --- quote targets --- opened from a quote: show its version.
        let want = self.reading.want_version.take();
        self.reading.sel = Some(key.clone());
        // --- reader folders --- stays listed in its source once read.
        self.reading.sticky.insert(key.clone());
        let tombstone = item.state == "tombstone";
        let responses = self.backend.responses(&item.origin, &item.remote_id);
        if self.is_own_origin(&item.origin) && matches!(self.reading.mentions, Load::Idle) {
            self.load_mentions(cx);
        }
        self.reading.opened = Some(Opened {
            responses,
            key: key.clone(),
            item,
            changelog: if tombstone { Load::Idle } else { Load::Loading },
            shown: if tombstone { Load::Idle } else { Load::Loading },
            diff_base: None,
            ix: None,
            dropdown: false,
            pins: Default::default(),
            diff_vs_now: false,
        });
        cx.notify();
        let backend = self.backend.clone();
        let (sub, rid) = key.clone();
        let task = cx.background_spawn(async move {
            let fetched = (!tombstone).then(|| {
                let base = backend.pinned_diff_base(&sub, &rid);
                let log = backend.remote_versions(&sub, &rid);
                let shown = backend.remote_shown_versions(&sub, &rid);
                (base, log, shown)
            });
            let _ = backend.mark_read(&sub, &rid);
            fetched
        });
        cx.spawn_in(window, async move |this, cx| {
            let fetched = task.await;
            let _ = this.update_in(cx, |v, _, cx| {
                let Some(o) = v.reading.opened.as_mut().filter(|o| o.key == key) else {
                    return;
                };
                let mut pick = None;
                if let Some((base, log, shown)) = fetched {
                    o.diff_base = base;
                    o.changelog = Load::from_result(log);
                    o.shown = Load::from_result(shown.map(|s| vm::shown(&s)));
                    o.ix = o.shown.ready().and_then(|s| vm::current_ix(s));
                    pick = want
                        .and_then(|w| o.shown.ready()?.iter().position(|s| s.version == w))
                        .filter(|&i| Some(i) != o.ix);
                }
                if let Some(i) = pick {
                    v.select_version(i, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ‹ / › on the version pill: step through current + pinned only.
    pub(super) fn step_version(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(o) = self.reading.opened.as_ref() else {
            return;
        };
        let (Some(shown), Some(ix)) = (o.shown.ready(), o.ix) else {
            return;
        };
        let next = ix as isize + delta;
        if next < 0 || next as usize >= shown.len() {
            return;
        }
        self.select_version(next as usize, cx);
    }

    pub(crate) fn select_version(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(o) = self.reading.opened.as_mut() else {
            return;
        };
        let Some(v) = o.shown.ready().and_then(|s| s.get(ix)).cloned() else {
            return;
        };
        o.ix = Some(ix);
        o.dropdown = false;
        o.diff_vs_now = false;
        if !v.current && !o.pins.contains_key(&v.version) {
            // Pinned: fetch its public document (cached forever by core).
            o.pins.insert(v.version, Load::Loading);
            let (sub, rid) = o.key.clone();
            let key = o.key.clone();
            let backend = self.backend.clone();
            let version = v.version;
            // --- quote targets --- a post fetched without a subscription
            // reads its pins straight from its origin.
            let origin = super::original::is_external(&key).then(|| o.item.origin.clone());
            let task = cx.background_spawn(async move {
                match origin {
                    Some(origin) => backend.public_pinned(&origin, &rid, version),
                    None => backend.remote_pinned(&sub, &rid, version),
                }
            });
            cx.spawn(async move |this, cx| {
                let r = task.await;
                let _ = this.update(cx, |v, cx| {
                    if let Some(o) = v.reading.opened.as_mut().filter(|o| o.key == key) {
                        o.pins.insert(version, Load::from_result(r));
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn toggle_thumb(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some(o) = self.reading.opened.as_ref() else {
            return;
        };
        self.toggle_thumb_for(o.key.clone(), o.item.thumb, up, cx);
    }

    /// 👍 / 👎 on a post whose thumb is `current` (again clears it).
    pub(super) fn toggle_thumb_for(
        &mut self,
        key: Key,
        current: Option<i8>,
        up: bool,
        cx: &mut Context<Self>,
    ) {
        let want = if up { 1 } else { -1 };
        let thumb = if current == Some(want) {
            None
        } else {
            Some(want)
        };
        let (sub, rid) = key;
        let backend = self.backend.clone();
        let task = cx.background_spawn(async move { backend.signal(&sub, &rid, thumb) });
        cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                let _ = this.update(cx, |v, cx| {
                    v.show_toast(format!("Couldn't send the signal: {e}"), None, cx)
                });
            }
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn reading_action_for_test(
        &mut self,
        action: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reading_action(action, window, cx)
    }

    pub(crate) fn reading_action(
        &mut self,
        action: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(o) = self.reading.opened.as_ref() else {
            return;
        };
        let item = o.item.clone();
        let pinned = o.pinned_on_screen().cloned();
        match action {
            // The current version's actions work on any post (the stream's
            // selected one too); "AI reply" never on a pinned view. Reply
            // quotes the whole post; a passage is chosen in the stub editor
            // (studio 0.31).
            "Quote" | "Reply" | "Link post" | "Open on web" => {
                self.item_action(item, action, window, cx)
            }
            "AI reply" if pinned.is_none() => self.item_action(item, action, window, cx),
            "Notes" => self.notes_add_post(item, true, window, cx), // --- notes ---
            "Quote this version" => {
                let Some(v) = pinned else { return };
                let text = o
                    .pins
                    .get(&v.version)
                    .and_then(|l| l.ready())
                    .map(|p| p.content_md.clone());
                let Some(text) = text else {
                    self.show_toast("The pinned version is still loading", None, cx);
                    return;
                };
                let url = blyg_core::pin_doc_url(&item.origin, &item.remote_id, v.version);
                let snippet =
                    vm::quote_pinned(&text, &vm::host(&item.origin), v.version, url.as_deref());
                self.quote_into_thread(snippet, window, cx);
            }
            "Fork this pin" => {
                // Forking descends from pins only.
                let Some(v) = pinned else { return };
                let of = RemoteRef {
                    origin: item.origin.clone(),
                    id: item.remote_id.clone(),
                    version: v.version,
                };
                let backend = self.backend.clone();
                let task = cx.background_spawn(async move { backend.fork(&of) });
                cx.spawn_in(window, async move |this, cx| {
                    let r = task.await;
                    let _ = this.update_in(cx, |this, window, cx| match r {
                        Ok(id) => {
                            this.open_new_draft(&id, window, cx);
                            this.show_toast(format!("Forked 📌 v{}", v.version), None, cx);
                        }
                        Err(e) => this.show_toast(format!("Couldn't fork: {e}"), None, cx),
                    });
                })
                .detach();
            }
            "Subscribe" => self.follow_url(item.origin.clone(), cx),
            "Diff vs now" => {
                if let Some(o) = self.reading.opened.as_mut() {
                    o.diff_vs_now = !o.diff_vs_now;
                }
                cx.notify();
            }
            "Back to current" => {
                let ix = o.shown.ready().and_then(|s| vm::current_ix(s));
                if let Some(ix) = ix {
                    self.select_version(ix, cx);
                }
            }
            _ => {}
        }
    }

    /// The current version's actions on any post (the open one, or the
    /// stream's selected one): Quote, Reply, AI reply, Open on web.
    pub(super) fn item_action(
        &mut self,
        item: blyg_core::ReadingItem,
        action: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blyg = self
            .reading
            .subs
            .iter()
            .find(|s| s.id == item.subscription_id)
            .is_none_or(|s| s.kind == SubscriptionKind::Blyg);
        match action {
            "Quote" => {
                let snippet = if blyg {
                    format!("![[{}]]", item.remote_id)
                } else {
                    // RSS items aren't transcludable: quote with a link.
                    let text = vm::title(&item.content_md);
                    let link = vm::web_url(&item).unwrap_or_default();
                    format!("> {text}\n>\n> — [{}]({link})", vm::host(&item.origin))
                };
                self.quote_into_thread(snippet, window, cx);
            }
            "Reply" => self.start_stub(&item, window, cx),
            // A plain `[[id]]` link in a new fragment: no stub_of, no mention.
            "Link post" if blyg => match self.backend.create_draft(
                blyg_core::Kind::Fragment,
                &vm::link_post_body(&item.remote_id),
            ) {
                Ok(id) => {
                    self.open_new_draft(&id, window, cx);
                    self.show_toast(
                        format!("New fragment linking {}", vm::host(&item.origin)),
                        None,
                        cx,
                    );
                }
                Err(e) => self.show_toast(format!("Couldn't start a post: {e}"), None, cx),
            },
            "Link post" => self.show_toast("Feed posts can't be linked with [[…]]", None, cx),
            // --- follow-ups --- a stub with a generated reply, for review.
            "AI reply" => self.ai_reply_to(item, window, cx),
            "Notes" => self.notes_add_post(item, false, window, cx), // --- notes ---
            "Open on web" => match vm::web_url(&item) {
                Some(u) => {
                    cx.open_url(&u);
                    self.show_toast("Opening in your browser…", None, cx);
                }
                None => self.show_toast("No web address for this post", None, cx),
            },
            _ => {}
        }
    }

    /// Reply: a stub of `item` quoting the whole post (studio 0.31, the
    /// server's own `![[id]]\n\n` prefill), the caret below the quote. The
    /// stub editor's hint line then offers a passage instead
    /// (`MainView::render_stub_bar`).
    fn start_stub(
        &mut self,
        item: &blyg_core::ReadingItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let body = vm::stub_body(&item.remote_id);
        let of = RemoteRef {
            origin: item.origin.clone(),
            id: item.remote_id.clone(),
            version: item.version,
        };
        match self.backend.create_stub(&of, &body) {
            Ok(id) => {
                self.open_new_draft(&id, window, cx);
                self.show_toast(
                    format!("Reply to {} · a stub thread", vm::host(&item.origin)),
                    Some("Quoting the whole post · choose a passage above the editor".into()),
                    cx,
                );
            }
            Err(e) => self.show_toast(format!("Couldn't start a reply: {e}"), None, cx),
        }
    }

    /// The action row under the open post: each chip names what it
    /// creates (a stub, a quote in a thread, a fork of a pin), with a
    /// one-sentence tooltip; Fork shows greyed on the current version.
    pub(crate) fn reading_action_chips(&self) -> Vec<vm::ActionChip> {
        let Some(o) = self.reading.opened.as_ref() else {
            return vec![];
        };
        let item = &o.item;
        // --- quote targets --- not followed: Subscribe, Reply, Open on web.
        if super::original::is_external(&o.key) {
            let ctx = vm::ActionCtx {
                blyg: true,
                version: item.version,
                current: item.version,
                pins: vec![],
            };
            return ["Subscribe", "Reply", "Open on web"]
                .into_iter()
                .map(|id| vm::action_chip(id, &ctx))
                .collect();
        }
        let ids = match self.pill_model() {
            Some(pm) => pm.actions,
            None if item.state == "tombstone" => vec![],
            None => vm::CURRENT_ACTIONS.to_vec(),
        };
        let shown = o.shown.ready().map(Vec::as_slice).unwrap_or_default();
        let ctx = vm::ActionCtx {
            blyg: self
                .reading
                .subs
                .iter()
                .find(|s| s.id == item.subscription_id)
                .is_none_or(|s| s.kind == SubscriptionKind::Blyg),
            version: o
                .pinned_on_screen()
                .map(|v| v.version)
                .unwrap_or(item.version),
            current: item.version,
            pins: shown
                .iter()
                .filter(|v| !v.current && v.pinned)
                .map(|v| v.version)
                .collect(),
        };
        ids.into_iter()
            // A feed post has no id to link.
            .filter(|id| *id != "Link post" || ctx.blyg)
            .map(|id| vm::action_chip(id, &ctx))
            .collect()
    }

    // ------------------------------------------------------------ render

    pub(super) fn render_reading_screen(
        &self,
        body_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_page();
        let unread = self.reading.rows.iter().filter(|r| r.is_unread()).count();
        let stream = self.reading.mode == super::stream_vm::ReadMode::Stream;
        let keys = if stream {
            "j/k move · ⏎ read more · / search · esc back"
        } else {
            // --- reader folders ---
            "←→ panes · j/k posts · space next unread · r/u read/unread · / search"
        };
        let picked = self.reading.picked.set.len();
        let hint = if !self.reading.available {
            String::new()
        } else if picked > 0 {
            // --- read/unread ---
            format!("{picked} picked · r marks read · u unread · esc lets go")
        } else if unread > 0 {
            // Reader-local, private state: allowed (never social).
            format!("{unread} to read · {keys}")
        } else {
            keys.into()
        };
        // --- stream --- the Stream | Reader toggle (⌥⌘1 / ⌥⌘2).
        let mut actions = vec![self.render_mode_toggle(cx)];
        if !stream {
            // --- reader folders --- the sources pane's button (⌥⌘S).
            actions.insert(0, self.render_sources_button(cx));
        }
        let header = self.screen_header("Reading", hint, actions);
        if !self.reading.available {
            return div()
                .flex_1()
                .flex()
                .flex_col()
                .child(header)
                .child(self.unavailable())
                .into_any_element();
        }
        if stream {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(header)
                .child(self.render_stream_body(body_font, cx))
                .into_any_element();
        }
        let held = self.reading.rows.len();
        let count = self.reading.shown.len();
        // --- reader folders --- a source with nothing in it (no search).
        let empty_source = held > 0 && count == 0 && self.reading.query.trim().is_empty();
        let list = div()
            .id("reader-list-pane")
            .w(relative(if self.reading.sources_open { 0.3 } else { 0.38 }))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .rule_r(&p)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.set_pane(super::sources_vm::Pane::List, cx)),
            )
            .when(held > 0, |d| d.child(self.render_source_title()))
            .when(held > 0, |d| d.child(self.render_reading_search(cx)))
            .when(held == 0, |d| {
                d.child(self.empty_note(crate::keymap::hint(
                    "Nothing to read yet. Subscribe to a blyg or a feed (⇧⌘S).",
                )))
            })
            .when(empty_source, |d| {
                d.child(self.empty_note(super::sources_vm::empty_label(&self.reading.source)))
            })
            .when(held > 0 && count == 0 && !empty_source, |d| {
                d.child(
                    div()
                        .id("reading-no-match")
                        .p(px(16.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(p.ink)
                                .child(vm::no_match(&self.reading.query)),
                        )
                        .child(
                            div()
                                .font_family(self.chrome())
                                .text_size(px(11.5))
                                .text_color(p.muted)
                                .child(crate::keymap::hint(
                                    "Searches titles, authors and text of the posts held \
                                     on this Mac · esc clears",
                                )),
                        ),
                )
            })
            .when(count > 0, |d| {
                d.child(
                    uniform_list(
                        "reading-rows",
                        count,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|ix| this.render_reading_row(ix, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.reading.list_scroll)
                    .flex_1()
                    .min_h_0()
                    .w_full(),
                )
            });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            // --- reader folders --- sources | list | post.
            .child(self.render_three_panes(
                list.into_any_element(),
                self.render_reading_detail(body_font, cx),
                cx,
            ))
            .into_any_element()
    }

    fn render_reading_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        // `ix` is a position in the shown (searched) rows.
        let Some(r) = self
            .reading
            .shown
            .get(ix)
            .and_then(|&i| self.reading.rows.get(i))
        else {
            return div().into_any_element();
        };
        let key = vm::key(r);
        let selected = self.reading.sel.as_ref() == Some(&key);
        // --- reader folders --- the selection is stronger in the pane with
        // the keys: the theme's `row.selected` (the plain look: an accent
        // wash), else the selected-row ground.
        let focused = self.reading.pane == super::sources_vm::Pane::List
            && self.reading.mode == super::stream_vm::ReadMode::Reader;
        let shaped = self.theme.has(blyg_core::config::theme::Slot::RowSelected);
        let sel_bg = if focused && !shaped {
            p.accent.opacity(0.14)
        } else {
            p.sel
        };
        let badge = vm::badge(r);
        let edited = r.edited_since_read() && r.state != "tombstone";
        let title = if r.state == "tombstone" && r.pinned_version_retained.is_none() {
            "(withdrawn)".to_string()
        } else {
            vm::post_title(r)
        };
        // The search's matches in the title, as the posts list shows them.
        let highlights: Vec<_> = crate::vm::highlight_ranges(&title, &self.reading.query)
            .into_iter()
            .map(|r| {
                (
                    r,
                    HighlightStyle {
                        color: Some(p.accent),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let when = vm::when_label(r, self.now);
        // --- read/unread --- picked rows (⌘-click, ⇧-click, ⌘A).
        let picked = self.reading.picked.contains(&key);
        let menu_key = key.clone();
        div()
            .id(("reading-row", ix))
            .px(px(14.))
            .py(px(7.))
            .relative()
            .rule_b(&p)
            .cursor_pointer()
            .when(selected, |d| d.bg(sel_bg))
            .when(picked, |d| d.bg(p.accent.opacity(0.22)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_post_menu(menu_key.clone(), e.position, cx)
                }),
            )
            // --- themes --- the theme's selected-row shape (lit cell,
            // outline, lantern…) on the page's selected ground.
            .map(|d| crate::ornament::page_row(&self.theme, d, selected && focused && shaped))
            .when(r.is_unread() || edited, |d| {
                // Unread / edited: an accent bar on the left, as in the mock.
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(3.))
                        .bg(p.accent),
                )
            })
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                window.focus(&this.reading.focus, cx);
                this.set_pane(super::sources_vm::Pane::List, cx);
                // --- read/unread --- ⌘ / ⇧ pick instead of opening.
                if this.pick_click(&key, e.modifiers(), cx) {
                    return;
                }
                this.open_reading(key.clone(), window, cx)
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .font_family(self.chrome())
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child(if r.version > 1 {
                        format!(
                            "{} · v{}",
                            vm::source_line(r, &self.reading.subs),
                            r.version
                        )
                    } else {
                        vm::source_line(r, &self.reading.subs)
                    })
                    .when_some(badge, |d, b| {
                        // --- themes --- badges that read on the row and
                        // on the selected ground alike.
                        let (fg, bg): (Hsla, Hsla) = if edited {
                            (p.edited, p.edited_bg)
                        } else if b == "new" {
                            p.badge()
                        } else {
                            p.quiet_badge()
                        };
                        d.child(
                            div()
                                .px(px(6.))
                                .rounded_full()
                                .text_size(px(10.5))
                                .text_color(fg)
                                .bg(bg)
                                .child(b),
                        )
                    })
                    // --- lineage --- which kinds, never how many.
                    .children(self.render_lineage_glyph(r, cx))
                    .child(div().flex_1())
                    .child(when),
            )
            .child(
                div()
                    .mt(px(2.))
                    .text_size(px(self.prefs.list_size()))
                    .line_height(relative(1.35))
                    .truncate()
                    .child(StyledText::new(title).with_highlights(highlights)),
            )
            .into_any_element()
    }

    pub(super) fn render_reading_detail(
        &self,
        body_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_page();
        let Some(o) = self.reading.opened.as_ref() else {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(p.muted)
                .italic()
                .text_size(px(13.))
                .child(if self.reading.shown.is_empty() {
                    ""
                } else {
                    "Pick a post · ↓ starts at the top"
                })
                .into_any_element();
        };
        let item = &o.item;
        let kind = crate::vm::kind_label(item.kind);
        let tombstone = item.state == "tombstone";
        let pinned_on_screen = o.pinned_on_screen().cloned();

        // The text on screen: current, or the pinned version selected.
        let (text, banner): (Option<String>, Option<String>) = match &pinned_on_screen {
            Some(v) => match o.pins.get(&v.version) {
                Some(Load::Ready(pin)) => (
                    Some(pin.content_md.clone()),
                    Some(format!(
                        "📌 Pinned v{} · {} · public forever{}",
                        v.version,
                        crate::vm::relative_time(&v.at, self.now),
                        if pin.hash_mismatch {
                            " · content hash doesn't match"
                        } else {
                            ""
                        }
                    )),
                ),
                Some(Load::Failed(e)) => {
                    (None, Some(format!("Couldn't load 📌 v{}: {e}", v.version)))
                }
                _ => (None, Some(format!("Loading 📌 v{}…", v.version))),
            },
            None if tombstone => match item.pinned_version_retained {
                Some(v) => (
                    Some(item.content_md.clone()),
                    Some(format!("Withdrawn by the author · kept: their 📌 v{v}")),
                ),
                None => (None, Some("Withdrawn by the author".into())),
            },
            // --- quote targets --- fetched on demand, not from a subscription.
            None if super::original::is_external(&o.key) => (
                Some(item.content_md.clone()),
                Some(format!(
                    "From {}'s public files · you don't follow this blyg",
                    vm::host(&item.origin)
                )),
            ),
            None => (Some(item.content_md.clone()), None),
        };

        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .mb(px(12.))
            .font_family(self.chrome())
            .text_size(px(12.))
            .text_color(p.muted)
            // --- profiles --- the author's address opens their profile
            .child(self.profile_origin_link("pf-origin-link", &item.origin, cx))
            .child(format!("· {kind}"))
            // --- end profiles ---
            .children(self.render_pill(o, cx))
            .children(self.render_lineage_glyph(item, cx)) // --- lineage ---
            .child(div().flex_1())
            .when(!super::original::is_external(&o.key), |d| {
                d.child(self.render_thumbs(item.thumb, cx))
            })
            // --- profiles --- "↳ stub of …" / "⑂ forked from …" on its own line
            .children(
                self.render_lineage(
                    item,
                    o.changelog
                        .ready()
                        .and_then(|c| blyg_core::Lineage::of_changelog(c)),
                    cx,
                ),
            );

        // Native, above the body: the banner, the retained-pin link, the
        // "edited since you read it" notes or diff, and "Diff vs now".
        let diffing = o.diff_vs_now && pinned_on_screen.is_some();
        let edited = pinned_on_screen
            .is_none()
            .then(|| self.edited_block())
            .flatten()
            .map(|b| match b {
                EditedBlock::Diff { heading, runs } => self.render_diff(heading, &runs, body_font),
                EditedBlock::Notes { heading, notes } => div()
                    .id("edited-notes")
                    .mt(px(4.))
                    .pl(px(10.))
                    .border_l_2()
                    .border_color(p.edited_rule)
                    .font_family(self.chrome())
                    .text_size(px(12.5))
                    .text_color(p.muted)
                    .child(div().text_color(p.ink).child(heading))
                    .child(if notes.is_empty() {
                        "Loading the author's notes…".to_string()
                    } else {
                        notes.join(" · ")
                    })
                    .into_any_element(),
            });
        let notes = div()
            .id("reading-notes")
            .px(px(32.))
            .flex_none()
            .overflow_y_scroll()
            .when(diffing, |d| d.flex_1().min_h_0())
            // Room for the body: a long diff scrolls here instead.
            .when(!diffing, |d| d.max_h(relative(0.45)))
            .when_some(banner, |d, b| {
                d.child(
                    div()
                        .mb(px(10.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(self.theme.corner(7.)))
                        .font_family(self.chrome())
                        .text_size(px(12.5))
                        .bg(p.notice_bg)
                        .text_color(p.notice)
                        .child(b),
                )
            })
            .when_some(tombstone.then(|| item.pin_url()).flatten(), |d, url| {
                d.child(
                    div()
                        .id("retained-pin")
                        .mb(px(10.))
                        .font_family(self.chrome())
                        .text_size(px(11.5))
                        .text_color(p.muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(p.accent))
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .child("the pinned version, attributed ↗"),
                )
            })
            .children(edited)
            .when(diffing, |d| {
                let now_text = item.content_md.clone();
                let base = text.clone().unwrap_or_default();
                let (_, runs) = crate::vm::word_diff(&base, &now_text);
                d.child(self.render_diff(
                    format!(
                        "📌 v{} → now (v{})",
                        pinned_on_screen.as_ref().map(|v| v.version).unwrap_or(0),
                        item.version
                    ),
                    &runs,
                    body_font,
                ))
            });

        // The body: the post as its blyg published it, in the WebView.
        let body = if self.studio.reader.active() {
            self.reader_pane(&p)
        } else {
            div()
                .flex_1()
                .when(diffing, |d| d.flex_none())
                .into_any_element()
        };

        let actions = self.reading_action_chips();
        let actions_row = div()
            .flex_none()
            .px(px(32.))
            .pt(px(10.))
            .pb(px(16.))
            .rule_t(&p)
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .children(actions.into_iter().map(|chip| {
                let a = chip.id;
                let tip = chip.tip;
                let id = format!("act-{a}");
                let el = if chip.enabled {
                    self.chip(id, chip.label)
                        .when(a == "Diff vs now" && o.diff_vs_now, |d| {
                            d.border_color(p.accent).text_color(p.accent)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.reading_action(a, window, cx)
                        }))
                } else {
                    // Greyed, not hidden: it says where the action lives.
                    self.chip_disabled(id, chip.label)
                };
                el.when(!tip.is_empty(), |d| {
                    d.tooltip(move |_, cx| cx.new(|_| super::Tip(tip.clone())).into())
                })
                .into_any_element()
            }));

        div()
            .id("reading-detail")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .pt(px(22.))
            .child(div().px(px(32.)).flex_none().child(header))
            .child(notes)
            .child(body)
            // --- responses --- a list, never a count.
            .children(self.render_responses(&item.origin, &item.remote_id, &o.responses, cx))
            .child(actions_row)
            .into_any_element()
    }

    /// A word diff with a heading (ins green, del struck red).
    fn render_diff(
        &self,
        heading: String,
        runs: &[(DiffOp, String)],
        body_font: &SharedString,
    ) -> AnyElement {
        let p = self.palette.on_page();
        let text: String = runs.iter().map(|(_, s)| s.as_str()).collect();
        let mut hl = Vec::new();
        let mut at = 0;
        for (op, s) in runs {
            let r = at..at + s.len();
            at += s.len();
            match op {
                DiffOp::Insert => hl.push((
                    r,
                    HighlightStyle {
                        background_color: Some(p.ins_bg),
                        ..Default::default()
                    },
                )),
                DiffOp::Delete => hl.push((
                    r,
                    HighlightStyle {
                        background_color: Some(p.del_bg),
                        strikethrough: Some(StrikethroughStyle {
                            thickness: px(1.),
                            color: Some(p.over),
                        }),
                        ..Default::default()
                    },
                )),
                DiffOp::Same => {}
            }
        }
        div()
            .id("edited-diff")
            .mt(px(14.))
            .pl(px(10.))
            .border_l_2()
            .border_color(p.edited_rule)
            .child(
                div()
                    .mb(px(4.))
                    .font_family(self.chrome())
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .child(heading),
            )
            .child(
                div()
                    .font_family(body_font.clone())
                    .text_size(px(14.))
                    .line_height(relative(1.5))
                    .child(StyledText::new(text).with_highlights(hl)),
            )
            .into_any_element()
    }

    fn render_thumbs(&self, thumb: Option<i8>, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        let t = |id: &'static str, glyph: &'static str, on: bool, up: bool| {
            div()
                .id(id)
                .px(px(6.))
                .py(px(1.))
                .rounded_full()
                .border_1()
                .cursor_pointer()
                .border_color(if on { p.accent } else { p.line })
                .when(!on, |d| d.opacity(0.6))
                .hover(|s| s.opacity(1.0))
                .child(glyph)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_thumb(up, cx)))
        };
        div()
            .flex()
            .gap(px(4.))
            .child(t("thumb-up", "👍", thumb == Some(1), true))
            .child(t("thumb-down", "👎", thumb == Some(-1), false))
            .into_any_element()
    }
}
