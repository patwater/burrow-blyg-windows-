//! Reading & management screens, and the version UI.
//!
//! A child module of `app`, so it can extend `MainView` directly; `app.rs`
//! keeps only small, delimited hooks (the view switcher, the body swap, the
//! sheet overlay, and event forwarding). The screens:
//!
//! - **Reading** (⌘R): one row per post; edited posts on top with
//!   "edited · vN"; the author's notes since you read it, and a text diff only
//!   when the version you read was pinned. The version pill `‹ vN ▾ ›` shows
//!   only the current and pinned versions of someone else's post.
//! - **Mentions** (⌘⇧M): a list, never a count; a dot for new.
//! - **Subscriptions** (⌘⇧S): subscribe (preview first), unsubscribe,
//!   pause/resume, blogroll.
//! - **Versions** (⌘Y) of your own post: full private history, restore, pin
//!   (type-to-confirm).
//! - **Site settings** (Blyg › Site Settings…): the blyg's title, bio, links.
//! - **Quote picker** (⌘K, threads only): inserts `![[id]]` from held items.

pub(crate) mod demo; // --- buttons --- (pub(crate): the toolbar demo snapshots too)
mod list;
mod mentions;
mod quote_picker;
mod site_settings;
mod subscriptions;
mod versions;
pub(crate) mod vm;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use blyg_core::{
    CoreError, LocalId, Mention, PinnedVersion, ReadingItem, RemoteVersion, Settings,
    SubscribePreview, Subscription, Version,
};
use gpui_kit::base::input::{InputState, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{MainView, TITLEBAR_H};
use vm::Key;

gpui_kit::actions!(
    blygger,
    [
        ShowReading,
        ShowMentions,
        ShowSubscriptions,
        ShowVersions,
        QuotePicker,
        SiteSettings,
        SubscribeTo,
    ]
);

/// Which screen the main window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Posts,
    Reading,
    Mentions,
    Subscriptions,
}

impl View {
    pub const ALL: [View; 4] = [
        View::Posts,
        View::Reading,
        View::Mentions,
        View::Subscriptions,
    ];

    pub fn label(self) -> &'static str {
        match self {
            View::Posts => "Posts",
            View::Reading => "Reading",
            View::Mentions => "Mentions",
            View::Subscriptions => "Subscriptions",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            View::Posts => "⌘L",
            View::Reading => "⌘R",
            View::Mentions => "⇧⌘M",
            View::Subscriptions => "⇧⌘S",
        }
    }
}

/// Something fetched from the server on demand.
#[derive(Debug, Clone, PartialEq)]
pub enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    /// The server lacks the read extensions (404).
    Unavailable,
    Failed(String),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(t) => Some(t),
            _ => None,
        }
    }

    fn from_result(r: blyg_core::Result<T>) -> Self {
        match r {
            Ok(t) => Load::Ready(t),
            Err(CoreError::NotFound) => Load::Unavailable,
            Err(e) => Load::Failed(e.to_string()),
        }
    }
}

pub const NOT_AVAILABLE: &str = "Not available on this server";
pub const NOT_AVAILABLE_SUB: &str = "This blyg doesn't have the owner-API read extensions \
     (GET /api/reading, /api/mentions, /api/settings). See docs/SERVER.md.";

/// A reading item open in the detail pane.
pub struct Opened {
    pub key: Key,
    /// As it was when opened: `read_version` is the version read *before*
    /// this visit, so "edited since you read it" survives marking it read.
    pub item: ReadingItem,
    pub changelog: Load<Vec<RemoteVersion>>,
    /// Current + pinned only, oldest first.
    pub shown: Load<Vec<RemoteVersion>>,
    /// The pinned version you last read, when it was pinned: diff base.
    pub diff_base: Option<PinnedVersion>,
    /// Index into `shown` of the version on screen.
    pub ix: Option<usize>,
    pub dropdown: bool,
    pub pins: HashMap<u32, Load<PinnedVersion>>,
    pub diff_vs_now: bool,
}

impl Opened {
    /// The version on screen when it's a pin (not the current one).
    pub fn pinned_on_screen(&self) -> Option<&RemoteVersion> {
        let shown = self.shown.ready()?;
        let v = shown.get(self.ix?)?;
        (!v.current).then_some(v)
    }
}

/// Your own post's history (⌘Y).
pub struct OwnVersions {
    pub id: LocalId,
    pub list: Load<Vec<Version>>,
    pub sel: Option<u32>,
}

#[allow(clippy::large_enum_variant)] // one sheet at a time
pub enum RSheet {
    /// Type-to-confirm pin of your own version.
    Pin {
        id: LocalId,
        version: u32,
        input: Entity<InputState>,
        error: Option<String>,
        busy: bool,
    },
    Restore {
        id: LocalId,
        version: u32,
        next: u32,
        dirty: bool,
        focus: FocusHandle,
    },
    Subscribe {
        input: Entity<InputState>,
        /// The URL the preview was made for.
        previewed: Option<(String, SubscribePreview)>,
        error: Option<String>,
        busy: bool,
    },
    Site {
        title: Entity<InputState>,
        name: Entity<InputState>,
        bio: Entity<InputState>,
        links: Entity<TextareaState>,
        load: Load<Settings>,
        error: Option<String>,
        busy: bool,
        focus: FocusHandle,
    },
    Quote {
        target: LocalId,
        input: Entity<InputState>,
        sel: usize,
        /// Opened by typing `![[`: where that text was taken from, and the
        /// text itself, so esc can put it back.
        typed: Option<(usize, String)>,
    },
}

pub struct State {
    pub view: View,
    pub focus: FocusHandle,
    /// Reading rows in display order (stable while the screen is open).
    pub rows: Vec<ReadingItem>,
    pub sel: Option<Key>,
    pub opened: Option<Opened>,
    pub list_scroll: UniformListScrollHandle,
    pub subs: Vec<Subscription>,
    pub sub_sel: usize,
    /// A subscription whose "Unsubscribe" was clicked once (click again).
    pub unsub_confirm: Option<String>,
    pub mentions: Load<Vec<Mention>>,
    pub mentions_seen: HashSet<String>,
    pub show_hidden: bool,
    pub own: Option<OwnVersions>,
    pub sheet: Option<RSheet>,
    pub sheet_gen: usize,
    pub available: bool,
    /// The reading search field (made on first show; it needs a window).
    pub search: Option<Entity<InputState>>,
    /// The reading search, as typed. Filters `rows` into `shown`.
    pub query: String,
    /// Indices into `rows` that match `query`, in display order: what the
    /// list shows and ↑/↓ move through.
    pub shown: Vec<usize>,
}

impl State {
    pub fn new(backend: &dyn blyg_core::Backend, cx: &mut App) -> Self {
        Self {
            view: View::Posts,
            focus: cx.focus_handle(),
            rows: vm::order(backend.reading()),
            sel: None,
            opened: None,
            list_scroll: UniformListScrollHandle::new(),
            subs: backend.subscriptions(),
            sub_sel: 0,
            unsub_confirm: None,
            mentions: Load::Idle,
            mentions_seen: HashSet::new(),
            show_hidden: false,
            own: None,
            sheet: None,
            sheet_gen: 0,
            available: backend.read_extensions_available(),
            search: None,
            query: String::new(),
            shown: Vec::new(),
        }
        .refiltered()
    }

    fn refiltered(mut self) -> Self {
        self.refilter();
        self
    }

    /// Recompute `shown` after `rows` or `query` changed.
    pub fn refilter(&mut self) {
        self.shown = vm::filter(&self.rows, &self.query);
    }

    /// The rows the list shows (all of them without a search).
    pub fn shown_rows(&self) -> impl Iterator<Item = &ReadingItem> {
        self.shown.iter().filter_map(|&i| self.rows.get(i))
    }

    /// Position of `key` in the shown rows.
    pub fn shown_pos(&self, key: &Key) -> Option<usize> {
        self.shown_rows().position(|r| &vm::key(r) == key)
    }

    pub fn has_new_mentions(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.mentions.ready().is_some_and(|ms| {
            ms.iter()
                .any(|m| vm::is_new_mention(m, &self.mentions_seen, now))
        })
    }
}

// ================================================================ hooks

impl MainView {
    /// Hook: this module's actions on the root element.
    pub(super) fn reading_actions(
        &self,
        d: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        d.on_action(cx.listener(|this, _: &ShowReading, window, cx| {
            this.show_view(View::Reading, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ShowMentions, window, cx| {
            this.show_view(View::Mentions, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ShowSubscriptions, window, cx| {
            this.show_view(View::Subscriptions, window, cx)
        }))
        .on_action(
            cx.listener(|this, _: &ShowVersions, window, cx| this.toggle_versions(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &QuotePicker, window, cx| this.open_quote_picker(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &SiteSettings, window, cx| this.open_site_settings(window, cx)),
        )
        .on_action(cx.listener(|this, _: &SubscribeTo, window, cx| this.open_subscribe(window, cx)))
    }

    /// Hook: the reading/backend event (`CoreEvent::ReadingChanged`).
    pub(super) fn reading_changed(&mut self, cx: &mut Context<Self>) {
        let r = &mut self.reading;
        r.available = self.backend.read_extensions_available();
        r.subs = self.backend.subscriptions();
        let fresh = self.backend.reading();
        if r.view == View::Reading {
            // Keep the order stable under the cursor: update rows in place,
            // put new posts on top, drop what went away.
            let order = vm::order(fresh);
            let mut next: Vec<ReadingItem> = order
                .iter()
                .filter(|n| !r.rows.iter().any(|o| vm::key(o) == vm::key(n)))
                .cloned()
                .collect();
            for old in &r.rows {
                if let Some(n) = order.iter().find(|n| vm::key(n) == vm::key(old)) {
                    next.push(n.clone());
                }
            }
            r.rows = next;
        } else {
            r.rows = vm::order(fresh);
        }
        r.refilter();
        if let Some(o) = &mut r.opened {
            // Fresh metadata (thumb, state), but keep the read version seen
            // at open time.
            if let Some(n) = r.rows.iter().find(|n| vm::key(n) == o.key) {
                let read = o.item.read_version;
                o.item = n.clone();
                o.item.read_version = read;
            }
        }
        cx.notify();
    }

    /// Switch screens. Pressing a screen's key again returns to Posts.
    pub(super) fn show_view(&mut self, v: View, window: &mut Window, cx: &mut Context<Self>) {
        let v = if v == self.reading.view && v != View::Posts {
            View::Posts
        } else {
            v
        };
        let was = self.reading.view;
        if was == View::Mentions && v != View::Mentions {
            // Leaving Mentions: what was on screen is no longer new.
            if let Some(ms) = self.reading.mentions.ready() {
                let ids: Vec<String> = ms.iter().map(|m| m.id.clone()).collect();
                self.reading.mentions_seen.extend(ids);
            }
        }
        self.reading.view = v;
        self.reading.own = None;
        match v {
            View::Posts => {
                self.back_to_search(window, cx);
                return;
            }
            View::Reading => {
                self.reading.available = self.backend.read_extensions_available();
                self.reading.rows = vm::order(self.backend.reading());
                self.reading.refilter();
                self.ensure_reading_search(window, cx);
                if self
                    .reading
                    .sel
                    .as_ref()
                    .is_none_or(|k| self.reading.shown_pos(k).is_none())
                {
                    self.reading.sel = None;
                    self.reading.opened = None;
                }
            }
            View::Mentions => self.load_mentions(cx),
            View::Subscriptions => {
                self.reading.subs = self.backend.subscriptions();
                self.reading.unsub_confirm = None;
            }
        }
        window.focus(&self.reading.focus, cx);
        cx.notify();
    }

    /// Hook: ⌘L goes back to the posts list.
    pub(super) fn leave_reading(&mut self) {
        self.reading.view = View::Posts;
        self.reading.own = None;
    }

    /// Hook: the segmented view switcher, on the right of the title bar.
    pub(super) fn render_view_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let now = self.now;
        let tabs = View::ALL.iter().map(|&v| {
            let on = self.reading.view == v;
            let dot = match v {
                View::Reading => self.reading.rows.iter().any(|r| r.is_unread()),
                View::Mentions => self.reading.has_new_mentions(now),
                _ => false,
            };
            div()
                .id(SharedString::from(format!("view-{}", v.label())))
                .px(px(9.))
                .py(px(2.))
                .rounded_full()
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(4.))
                .when(on, |d| d.bg(p.sel).text_color(p.ink))
                .when(!on, |d| d.hover(|s| s.text_color(p.ink)))
                .child(v.label())
                .when(dot, |d| {
                    d.child(div().size(px(5.)).rounded_full().bg(p.accent))
                })
                .tooltip(move |_, cx| {
                    cx.new(|_| Tip(format!("{} · {}", v.label(), v.key())))
                        .into()
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.reading.view == v {
                        return;
                    }
                    this.show_view(v, window, cx)
                }))
        });
        div()
            .absolute()
            .top_0()
            .right(px(12.))
            .h(px(TITLEBAR_H))
            .flex()
            .items_center()
            .gap(px(2.))
            .font_family("Inter")
            .text_size(px(11.5))
            .text_color(p.muted)
            .children(tabs)
            .into_any_element()
    }

    /// Hook: the body for every screen but Posts (omnibar + list + editor);
    /// `None` = draw the Posts screen as usual.
    pub(super) fn render_reading_body(
        &self,
        body_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let inner = match self.reading.view {
            View::Posts => return None,
            View::Reading => self.render_reading_screen(body_font, cx),
            View::Mentions => self.render_mentions_screen(cx),
            View::Subscriptions => self.render_subscriptions_screen(cx),
        };
        Some(
            div()
                .id("reading-body")
                .key_context("Reading")
                .track_focus(&self.reading.focus)
                .on_key_down(cx.listener(Self::reading_key_down))
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(inner)
                .into_any_element(),
        )
    }

    fn reading_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading.sheet.is_some() {
            return;
        }
        // Typing in the search field is the field's business (its own
        // esc, ↑/↓ and ⏎ are handled in `render_reading_search`).
        if self.reading_search_focused(window, cx) {
            return;
        }
        let k = &ev.keystroke;
        // ⌘F searches the reading list. Handled here, not in the keymap
        // table: gpui-base's inputs bind ⌘F themselves, and this is the
        // reading list's own key like `/`, ↑/↓ and ←/→.
        // `secondary` is ⌘ on macOS and Ctrl on Windows; nothing else held.
        let m = &k.modifiers;
        let other = if cfg!(target_os = "macos") {
            m.control
        } else {
            m.platform
        };
        if self.reading.view == View::Reading
            && m.secondary()
            && !(other || m.alt || m.shift)
            && k.key == "f"
        {
            self.focus_reading_search(window, cx);
            cx.stop_propagation();
            return;
        }
        if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
            return;
        }
        let handled = match (self.reading.view, k.key.as_str()) {
            (View::Reading, "escape")
                if self.reading.opened.as_ref().is_some_and(|o| o.dropdown) =>
            {
                if let Some(o) = &mut self.reading.opened {
                    o.dropdown = false;
                }
                cx.notify();
                true
            }
            // esc clears a search before it leaves the screen.
            (View::Reading, "escape") if !self.reading.query.is_empty() => {
                self.set_reading_query("", window, cx);
                true
            }
            (View::Reading, "/") => {
                self.focus_reading_search(window, cx);
                true
            }
            (_, "escape") => {
                self.show_view(View::Posts, window, cx);
                true
            }
            (View::Reading, "down" | "j") => {
                self.move_reading(1, window, cx);
                true
            }
            (View::Reading, "up" | "k") => {
                self.move_reading(-1, window, cx);
                true
            }
            (View::Reading, "left") => {
                self.step_version(-1, cx);
                true
            }
            (View::Reading, "right") => {
                self.step_version(1, cx);
                true
            }
            (View::Subscriptions, "down" | "j") => {
                let n = self.reading.subs.len();
                if n > 0 {
                    self.reading.sub_sel = (self.reading.sub_sel + 1).min(n - 1);
                }
                cx.notify();
                true
            }
            (View::Subscriptions, "up" | "k") => {
                self.reading.sub_sel = self.reading.sub_sel.saturating_sub(1);
                cx.notify();
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    /// Hook: this module's sheets (drawn over everything).
    pub(super) fn render_reading_sheet(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sheet = self.reading.sheet.as_ref()?;
        let (width, content) = match sheet {
            RSheet::Pin { .. } | RSheet::Restore { .. } => {
                (460., self.render_version_sheet(sheet, cx))
            }
            RSheet::Subscribe { .. } => (500., self.render_subscribe_sheet(sheet, cx)),
            RSheet::Site { .. } => (540., self.render_site_sheet(sheet, cx)),
            RSheet::Quote { .. } => (520., self.render_quote_sheet(sheet, cx)),
        };
        Some(self.sheet_frame(width, content))
    }

    pub(super) fn close_reading_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reading.sheet = None;
        if self.reading.view == View::Posts {
            self.focus_after_sheet(window, cx);
        } else {
            window.focus(&self.reading.focus, cx);
        }
        cx.notify();
    }

    fn open_reading_sheet(&mut self, sheet: RSheet, cx: &mut Context<Self>) {
        self.reading.sheet_gen += 1;
        self.reading.sheet = Some(sheet);
        cx.notify();
    }

    // ------------------------------------------------------------ shared UI

    /// The same drop-from-the-title-bar frame the main sheets use.
    fn sheet_frame(&self, width: f32, content: AnyElement) -> AnyElement {
        let p = self.palette;
        let gen_ = self.reading.sheet_gen;
        div()
            .absolute()
            .top(px(TITLEBAR_H))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("reading-sheet")
                    .occlude()
                    .w(px(width))
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
                    .child(content)
                    .with_animation(
                        ("reading-sheet-in", gen_),
                        Animation::new(std::time::Duration::from_millis(220))
                            .with_easing(ease_out_quint()),
                        |d, t| d.mt(px(-240.0 * (1.0 - t))).opacity(t.min(1.0) * 0.4 + 0.6),
                    ),
            )
            .into_any_element()
    }

    fn kbd(&self, k: &'static str) -> Div {
        let p = self.palette;
        div()
            .px(px(6.))
            .py(px(1.))
            .min_w(px(20.))
            .flex()
            .justify_center()
            .rounded(px(5.))
            .border_1()
            .border_b_2()
            .border_color(p.line)
            .text_color(p.ink)
            .font_weight(FontWeight::MEDIUM)
            .text_size(px(11.5))
            .child(k)
    }

    fn key_hint(&self, k: &'static str, label: &'static str) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(self.kbd(k))
            .child(label)
    }

    fn keys_row(&self, items: Vec<Div>) -> Div {
        div()
            .mt(px(12.))
            .flex()
            .flex_wrap()
            .gap(px(14.))
            .text_color(self.palette.muted)
            .children(items)
    }

    fn sheet_heading(&self, s: impl Into<SharedString>) -> Div {
        div()
            .mb(px(8.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_size(px(13.))
            .child(s.into())
    }

    fn input_box(&self, el: AnyElement, error: bool) -> Div {
        let p = self.palette;
        div()
            .px(px(10.))
            .py(px(7.))
            .rounded(px(7.))
            .border_1()
            .border_color(if error { p.over } else { p.line })
            .text_size(px(13.5))
            .child(el)
    }

    /// A small bordered action chip (`Quote`, `Reply`, …).
    fn chip(&self, id: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
        let p = self.palette;
        div()
            .id(ElementId::Name(id.into()))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(6.))
            .border_1()
            .border_color(p.line)
            .cursor_pointer()
            .font_family("Inter")
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.ink)
            .hover(|s| s.border_color(p.accent))
            .child(label.into())
    }

    /// A chip that can't be clicked now: greyed, dashed, its tooltip says why.
    fn chip_disabled(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
    ) -> Stateful<Div> {
        let p = self.palette;
        div()
            .id(ElementId::Name(id.into()))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(6.))
            .border_1()
            .border_dashed()
            .border_color(p.line)
            .font_family("Inter")
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(p.muted)
            .child(label.into())
    }

    /// The screen header: a title, a hint, and actions on the right.
    fn screen_header(&self, title: &'static str, hint: String, actions: Vec<AnyElement>) -> Div {
        let p = self.palette;
        div()
            .h(px(super::OMNI_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(14.))
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(14.))
                    .child(title),
            )
            .child(
                div()
                    .flex_1()
                    .font_family("Inter")
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .truncate()
                    .child(hint),
            )
            .children(actions)
    }

    /// "Not available on this server", centred.
    fn unavailable(&self) -> AnyElement {
        let p = self.palette;
        div()
            .id("not-available")
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .p(px(24.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(14.))
                    .child(NOT_AVAILABLE),
            )
            .child(
                div()
                    .max_w(px(420.))
                    .text_center()
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .child(NOT_AVAILABLE_SUB),
            )
            .into_any_element()
    }

    fn muted_note(&self, s: impl Into<SharedString>) -> AnyElement {
        div()
            .p(px(16.))
            .italic()
            .text_size(px(12.5))
            .text_color(self.palette.muted)
            .child(s.into())
            .into_any_element()
    }

    /// Send a quote (`![[id]]` or a quoted pin) to a thread: the thread
    /// being written if there is one, else a new thread draft. Then edit it.
    fn quote_into_thread(&mut self, snippet: String, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .current
            .clone()
            .filter(|c| vm::can_quote_into(Some(c)) && c.status == blyg_core::Status::Draft);
        self.leave_reading();
        match target {
            Some(item) => {
                self.back_to_search(window, cx);
                self.open(&item.local_id, window, cx);
                let (text, cursor) = {
                    let s = self.editor.read(cx);
                    (s.value().to_string(), s.cursor())
                };
                let (new_text, caret) = insert_block(&text, cursor, &snippet);
                self.splice_editor(&text, &new_text, Some(caret), window, cx);
                self.show_toast(format!("Quoted in “{}”", item.title()), None, cx);
            }
            None => match self
                .backend
                .create_draft(blyg_core::Kind::Thread, &format!("{snippet}\n\n"))
            {
                Ok(id) => {
                    self.open_new_draft(&id, window, cx);
                    self.show_toast(
                        "New thread with the quote",
                        Some("Quotes go in threads".into()),
                        cx,
                    );
                }
                Err(e) => self.show_toast(format!("Couldn't start a thread: {e}"), None, cx),
            },
        }
    }

    /// Show a draft just made elsewhere (reply, fork, quote) in the editor.
    pub(super) fn open_new_draft(
        &mut self,
        id: &LocalId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_reading();
        self.set_query_text("", window, cx);
        let results = self.backend.search("");
        self.list.set_query("", results);
        self.list.select(id);
        self.current = None;
        self.open(id, window, cx);
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }
}

/// Insert a block of markdown on its own paragraph at `cursor`; returns the
/// text and the caret after the block.
fn insert_block(text: &str, cursor: usize, block: &str) -> (String, usize) {
    if block.starts_with("![[") && !block.contains('\n') {
        let id = block.trim_start_matches("![[").trim_end_matches("]]");
        return vm::insert_transclusion(text, cursor, id);
    }
    let mut c = cursor.min(text.len());
    while c > 0 && !text.is_char_boundary(c) {
        c -= 1;
    }
    let (before, after) = text.split_at(c);
    let mut out = before.to_string();
    if !before.is_empty() {
        while !out.ends_with("\n\n") {
            out.push('\n');
        }
    }
    out.push_str(block);
    let caret = out.len();
    out.push_str("\n\n");
    out.push_str(after.trim_start_matches('\n'));
    (out, caret)
}

/// Tooltip text.
pub struct Tip(pub String);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(7.))
            .py(px(3.))
            .rounded(px(5.))
            .bg(gpui_kit::black().opacity(0.85))
            .text_color(gpui_kit::white())
            .font_family("Inter")
            .text_size(px(11.))
            .child(self.0.clone())
    }
}
