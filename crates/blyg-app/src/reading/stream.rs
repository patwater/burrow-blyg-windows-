//! The reading stream (issue #1), the reading screen's default mode: one
//! native timeline, newest first, drawn from each post's Markdown with GPUI
//! text (no WebView per post) in a virtualized `list`.
//!
//! - Fragments show in full; threads show their title, about four lines and
//!   "Read more". A quote (`![[id]]`) is a grey box with the quoted text when
//!   it's held here. Images and embeds are compact placeholders.
//! - j/k (↑/↓) select; ⏎ / Space / "Read more" open the post in a side pane
//!   (the sanitized WebView reader, with its version pill and actions); esc
//!   closes it. The stream keeps its place.
//! - A post counts as read after it has been at least half on screen for a
//!   second (`stream_vm::ReadTracker`), through `Backend::mark_read`; a
//!   thread opened in the pane is read too. Unread posts get a dot, never a
//!   count.
//! - The search (⌘F, `/`) filters the stream like the list.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use blyg_core::{Kind, ReadingItem};
use blyg_render::{Block, Span};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::stream_vm::{self, ReadMode, ReadTracker};
use super::vm::{self, Key};
use crate::app::MainView;
use crate::theme::Rule; // --- themes --- dividers

gpui_kit::actions!(blygger, [StreamMode, ReaderMode]);

/// How often the stream checks what's on screen.
const TICK: Duration = Duration::from_millis(250);

/// A drag over a post in the stream: the toast once it's open in the pane.
pub const SELECT_HERE: &str = "Select the passage in the post here";
pub const SELECT_HOW: &str = "Then ⇧⌘D, or the pill by the selection, quotes it";

/// A post's click as a `stream_vm::PostClick` (keyboard clicks select).
fn post_click_of(ev: &ClickEvent) -> stream_vm::PostClick {
    match ev {
        ClickEvent::Mouse(m) => {
            let (d, u) = (m.down.position, m.up.position);
            stream_vm::post_click(f32::from(u.x - d.x), f32::from(u.y - d.y), m.up.click_count)
        }
        _ => stream_vm::PostClick::Select,
    }
}

/// Parsed posts by (row, version, withdrawn).
type BlockCache = HashMap<(Key, u32, bool), Rc<Vec<Block>>>;

/// The stream's own state (inside `reading::State`).
pub struct Stream {
    pub list: ListState,
    pub tracker: ReadTracker,
    /// Parsed Markdown per post version, so a frame doesn't re-parse.
    blocks: RefCell<BlockCache>,
    timer: Option<Task<()>>,
    /// The rows (key, version) the list was last sized for.
    synced: Vec<(Key, u32)>,
}

impl Stream {
    pub fn new() -> Stream {
        Stream {
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            tracker: ReadTracker::default(),
            blocks: RefCell::default(),
            timer: None,
            synced: Vec::new(),
        }
    }

    /// Keep the list's item count (and measurements) in step with the shown
    /// rows, keeping the scroll position when posts only arrive on top.
    pub fn sync(&mut self, rows: &[ReadingItem], shown: &[usize]) {
        let now: Vec<(Key, u32)> = shown
            .iter()
            .filter_map(|&i| rows.get(i))
            .map(|r| (vm::key(r), r.version))
            .collect();
        if now == self.synced {
            return;
        }
        let old = std::mem::take(&mut self.synced);
        let same_keys = now.len() == old.len() && now.iter().zip(&old).all(|(a, b)| a.0 == b.0);
        if same_keys {
            // Edited in place: those posts are unread again, and may be taller.
            for (ix, (a, b)) in now.iter().zip(&old).enumerate() {
                if a.1 != b.1 {
                    self.tracker.forget(&a.0);
                    self.list.remeasure_items(ix..ix + 1);
                }
            }
        } else if now.len() > old.len()
            && now[now.len() - old.len()..]
                .iter()
                .zip(&old)
                .all(|(a, b)| a.0 == b.0)
        {
            // New posts on top.
            self.list.splice(0..0, now.len() - old.len());
        } else {
            self.list.reset(now.len());
        }
        self.synced = now;
    }

    fn blocks_of(&self, r: &ReadingItem) -> Rc<Vec<Block>> {
        let tomb = r.state == "tombstone";
        let key = (vm::key(r), r.version, tomb);
        if let Some(b) = self.blocks.borrow().get(&key) {
            return b.clone();
        }
        let b = Rc::new(post_blocks(r));
        self.blocks.borrow_mut().insert(key, b.clone());
        b
    }
}

/// A post's blocks from its Markdown; an item held only as HTML (some
/// feeds) shows its text as paragraphs.
fn post_blocks(r: &ReadingItem) -> Vec<Block> {
    let kind = match r.kind {
        Kind::Thread => blyg_render::Kind::Thread,
        Kind::Fragment => blyg_render::Kind::Fragment,
    };
    if !r.content_md.trim().is_empty() {
        return blyg_render::native_blocks(&r.content_md, kind);
    }
    html_paragraphs(&r.content_html)
        .into_iter()
        .map(|t| {
            Block::Para(vec![Span {
                text: t,
                ..Default::default()
            }])
        })
        .collect()
}

/// Paragraph texts of an HTML fragment (block tags split, the rest dropped).
fn html_paragraphs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(
                    name.as_str(),
                    "p" | "br" | "div" | "li" | "h1" | "h2" | "h3" | "h4" | "blockquote" | "pre"
                ) {
                    let t = cur.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !t.is_empty() {
                        out.push(t);
                    }
                    cur.clear();
                }
            }
            c if in_tag => tag.push(c),
            c => cur.push(c),
        }
    }
    let t = cur.split_whitespace().collect::<Vec<_>>().join(" ");
    if !t.is_empty() {
        out.push(t);
    }
    out.into_iter()
        .map(|t| {
            t.replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&#39;", "'")
                .replace("&nbsp;", " ")
                .replace("&amp;", "&")
        })
        .collect()
}

/// What a clickable run opens.
#[derive(Clone)]
enum Target {
    Url(String),
    /// An `[[id]]` link: (origin, id).
    Post(String, String),
}

/// A link target as the stream opens it: absolute http(s)/mailto, or
/// resolved against the post's origin.
fn link_target(href: &str, origin: &str) -> Option<String> {
    let h = href.trim();
    let lower = h.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
    {
        return Some(h.to_string());
    }
    if lower.contains(':') && !lower.starts_with('/') {
        return None; // javascript:, data:, …
    }
    blyg_core::origin_url(origin, h.trim_start_matches('/'))
}

impl MainView {
    // ------------------------------------------------------------ mode

    /// ⌥⌘1 / ⌥⌘2 or the toggle: switch mode (and to the reading screen).
    pub(crate) fn set_read_mode(
        &mut self,
        mode: ReadMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reading.mode != mode {
            self.reading.mode = mode;
            stream_vm::save_mode(self.reading_data_dir(cx).as_deref(), mode);
            // --- reader folders --- the Reader lists its source only; the
            // post open in the stream stays listed there.
            if let Some(k) = self.reading.sel.clone() {
                self.reading.sticky.insert(k);
            }
            self.reading.refilter();
        }
        if self.reading.view != super::View::Reading {
            self.show_view(super::View::Reading, window, cx);
        }
        if mode == ReadMode::Stream {
            self.ensure_stream_timer(window, cx);
            if let Some(ix) = self
                .reading
                .sel
                .as_ref()
                .and_then(|k| self.reading.shown_pos(k))
            {
                self.reading.stream.list.scroll_to_reveal_item(ix);
            }
        } else if let Some(ix) = self
            .reading
            .sel
            .as_ref()
            .and_then(|k| self.reading.shown_pos(k))
        {
            self.reading
                .list_scroll
                .scroll_to_item(ix, ScrollStrategy::Nearest);
        }
        window.focus(&self.reading.focus, cx);
        cx.notify();
    }

    fn reading_data_dir(&self, cx: &App) -> Option<std::path::PathBuf> {
        cx.try_global::<crate::connection::Connection>()
            .map(|c| c.data_dir.clone())
    }

    pub(super) fn stream_actions(&self, d: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        d.on_action(cx.listener(|this, _: &StreamMode, window, cx| {
            this.set_read_mode(ReadMode::Stream, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ReaderMode, window, cx| {
            this.set_read_mode(ReadMode::Reader, window, cx)
        }))
    }

    /// The Stream | Reader segmented toggle in the reading header.
    pub(super) fn render_mode_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        let seg = |m: ReadMode| {
            let on = self.reading.mode == m;
            div()
                .id(SharedString::from(format!("mode-{}", m.as_str())))
                .debug_selector(move || format!("mode-{}", m.as_str()))
                .px(px(10.))
                .py(px(2.))
                .rounded(px(self.theme.corner(5.)))
                .cursor_pointer()
                .when(on, |d| d.bg(p.bg).text_color(p.ink).shadow_sm())
                .when(!on, |d| {
                    d.text_color(p.muted).hover(|s| s.text_color(p.ink))
                })
                .child(m.label())
                .tooltip(move |_, cx| {
                    cx.new(|_| super::Tip(format!("{} · {}", m.label(), m.key())))
                        .into()
                })
                .on_click(cx.listener(move |this, _, window, cx| this.set_read_mode(m, window, cx)))
        };
        div()
            .id("read-mode")
            .flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(self.theme.corner(7.)))
            .bg(p.well())
            .font_family(self.chrome())
            .text_size(px(11.5))
            .font_weight(FontWeight::MEDIUM)
            .children(ReadMode::ALL.map(seg))
            .into_any_element()
    }

    // ------------------------------------------------------------ keys

    /// Stream-mode keys; `true` when handled.
    pub(super) fn stream_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match key {
            "down" | "j" => self.stream_move(1, window, cx),
            "up" | "k" => self.stream_move(-1, window, cx),
            "enter" | "space" => self.stream_open_selected(window, cx),
            "escape" if self.reading.opened.is_some() => self.close_stream_pane(window, cx),
            _ => return false,
        }
        true
    }

    /// Move the selection. With the side pane open, it follows.
    pub(crate) fn stream_move(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        self.stream_select(next, window, cx);
        if self.reading.opened.is_some()
            && let Some(key) = self.reading.sel.clone()
        {
            self.open_reading(key, window, cx);
        }
    }

    pub(crate) fn stream_select(
        &mut self,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old = self
            .reading
            .sel
            .as_ref()
            .and_then(|k| self.reading.shown_pos(k));
        let Some(key) = self.reading.shown_rows().nth(ix).map(vm::key) else {
            return;
        };
        self.reading.sel = Some(key);
        let list = &self.reading.stream.list;
        // The action row comes and goes with the selection.
        if let Some(o) = old {
            list.remeasure_items(o..o + 1);
        }
        list.remeasure_items(ix..ix + 1);
        self.reveal_stream_item(ix);
        cx.notify();
    }

    /// Scroll so the post is in view: its top at the top when it isn't
    /// wholly visible already.
    fn reveal_stream_item(&self, ix: usize) {
        let list = &self.reading.stream.list;
        let vp = list.viewport_bounds();
        let fully = list
            .bounds_for_item(ix)
            .is_some_and(|b| b.top() >= vp.top() && b.bottom() <= vp.bottom());
        if !fully {
            list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
        }
    }

    /// ⏎ / Space / "Read more": the selected post in the side pane.
    pub(crate) fn stream_open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading.sel.is_none() {
            self.stream_move(1, window, cx);
        }
        if let Some(key) = self.reading.sel.clone() {
            self.open_reading(key, window, cx);
        }
    }

    pub(crate) fn close_stream_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reading.opened = None;
        window.focus(&self.reading.focus, cx);
        cx.notify();
    }

    fn stream_click(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.reading.focus, cx);
        let Some(ix) = self.reading.shown_pos(&key) else {
            return;
        };
        if self.reading.sel.as_ref() != Some(&key) {
            self.stream_select(ix, window, cx);
        }
    }

    /// A click on a post (`stream_vm::post_click`): select it, or open it
    /// in the side pane, where its text can be selected and quoted.
    pub(crate) fn stream_post_click(
        &mut self,
        key: Key,
        click: stream_vm::PostClick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match click {
            stream_vm::PostClick::Select => self.stream_click(key, window, cx),
            stream_vm::PostClick::Open => self.stream_read_more(key, window, cx),
            stream_vm::PostClick::OpenToSelect => {
                self.stream_read_more(key, window, cx);
                self.show_toast(SELECT_HERE, Some(SELECT_HOW.into()), cx);
            }
        }
    }

    fn stream_read_more(&mut self, key: Key, window: &mut Window, cx: &mut Context<Self>) {
        self.stream_click(key.clone(), window, cx);
        self.open_reading(key, window, cx);
    }

    // ------------------------------------------------------------ read tracking

    /// Start the tick that marks on-screen posts read (once, lazily).
    pub(super) fn ensure_stream_timer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading.stream.timer.is_some() {
            return;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this.update_in(cx, |v, window, cx| {
                    v.stream_tick(Instant::now(), window.is_window_active(), cx)
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self.reading.stream.timer = Some(task);
    }

    /// One look at the screen: unread posts at least half visible for a
    /// second are marked read. Nothing counts while the window is in the
    /// background or something covers the stream.
    pub(crate) fn stream_tick(&mut self, now: Instant, active: bool, cx: &mut Context<Self>) {
        let r = &self.reading;
        let watching = active
            && r.view == super::View::Reading
            && r.mode == ReadMode::Stream
            && r.available
            && r.sheet.is_none()
            && self.sheet.is_none()
            && !self.profile_sheet_open();
        let mut on_screen = Vec::new();
        if watching {
            let list = &r.stream.list;
            let vp = list.viewport_bounds();
            let first = list.logical_scroll_top().item_ix;
            for (ix, row) in r.shown_rows().enumerate().skip(first) {
                let Some(b) = list.bounds_for_item(ix) else {
                    break;
                };
                if b.top() >= vp.bottom() {
                    break;
                }
                let unread = row.is_unread() || row.edited_since_read();
                if unread
                    && row.state != "tombstone"
                    && stream_vm::seen_enough(
                        f32::from(b.top()),
                        f32::from(b.bottom()),
                        f32::from(vp.top()),
                        f32::from(vp.bottom()),
                    )
                {
                    on_screen.push(vm::key(row));
                }
            }
        }
        let due = self.reading.stream.tracker.observe(&on_screen, now);
        for (sub, rid) in due {
            let backend = self.backend.clone();
            cx.background_spawn(async move {
                let _ = backend.mark_read(&sub, &rid);
            })
            .detach();
        }
    }

    // ------------------------------------------------------------ render

    pub(super) fn render_stream_body(
        &self,
        body_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_page();
        let held = self.reading.rows.len();
        let count = self.reading.shown.len();
        let font = body_font.clone();
        let column = div()
            .id("stream-column")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .when(held > 0, |d| d.child(self.render_reading_search(cx)))
            .when(held == 0, |d| {
                d.child(
                    self.empty_note("Nothing to read yet. Subscribe to a blyg or a feed (⇧⌘S)."),
                )
            })
            .when(held > 0 && count == 0, |d| {
                d.child(
                    div()
                        .id("reading-no-match")
                        .p(px(16.))
                        .text_size(px(13.))
                        .text_color(p.ink)
                        .child(vm::no_match(&self.reading.query)),
                )
            })
            .when(count > 0, |d| {
                d.child(
                    list(
                        self.reading.stream.list.clone(),
                        cx.processor(move |this, ix: usize, _, cx| {
                            this.render_stream_post(ix, &font, cx)
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .w_full(),
                )
            });
        let pane = self.reading.opened.as_ref().map(|_| {
            div()
                .id("stream-pane")
                .w(relative(0.54))
                .flex_none()
                .h_full()
                .relative()
                .flex()
                .rule_l(&p)
                .child(self.render_reading_detail(body_font, cx))
                .child(
                    div()
                        .id("stream-pane-close")
                        .debug_selector(|| "stream-pane-close".into())
                        .absolute()
                        .top(px(3.))
                        .right(px(10.))
                        .px(px(6.))
                        .rounded(px(self.theme.corner(5.)))
                        .cursor_pointer()
                        .font_family(self.chrome())
                        .text_size(px(11.))
                        .text_color(p.muted)
                        .hover(|s| s.text_color(p.ink).bg(p.sel))
                        .child("✕ esc")
                        .tooltip(|_, cx| cx.new(|_| super::Tip("Close · esc".into())).into())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_stream_pane(window, cx)),
                        ),
                )
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(column)
            .children(pane)
            .into_any_element()
    }

    fn render_stream_post(
        &self,
        ix: usize,
        body_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_page();
        let Some(r) = self.reading.shown_rows().nth(ix) else {
            return div().into_any_element();
        };
        let key = vm::key(r);
        let selected = self.reading.sel.as_ref() == Some(&key);
        let opened = self.reading.opened.as_ref().is_some_and(|o| o.key == key);
        let tombstone = r.state == "tombstone";
        let edited = r.edited_since_read() && !tombstone;
        let unread = (r.is_unread() || edited) && !tombstone;
        let text_px = (self.prefs.font_size * 16.0 / 19.0).round();

        let name = r
            .author
            .as_ref()
            .and_then(|a| a.name.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| r.subscription_title.clone());
        let origin = r.origin.clone();
        let meta = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .font_family(self.chrome())
            .text_size(px(11.5))
            .text_color(p.muted)
            .child(
                div()
                    .id("stream-author")
                    .cursor_pointer()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.ink)
                    .hover(|s| s.text_color(p.accent))
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Profile · ⌘I".into())).into())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_profile(origin.clone(), window, cx)
                    }))
                    .child(name),
            )
            .child(vm::host(&r.origin))
            .child("·")
            .child(vm::when_label(r, self.now))
            .child("·")
            .child(stream_vm::kind_word(r.kind))
            // --- lineage --- which kinds of relation, never how many.
            .children(self.render_lineage_glyph(r, cx))
            .when(edited, |d| {
                d.child(badge(p, format!("edited · v{}", r.version)))
            })
            .when(tombstone, |d| d.child(badge(p, "withdrawn".into())));

        let lineage = self.render_stream_lineage(r, cx);

        let blocks = self.reading.stream.blocks_of(r);
        let mut n = 0usize;
        let body: Vec<AnyElement> = if tombstone && r.pinned_version_retained.is_none() {
            vec![
                div()
                    .italic()
                    .text_color(p.muted)
                    .child("Withdrawn by the author")
                    .into_any_element(),
            ]
        } else if r.kind == Kind::Thread {
            let title = vm::post_title(r);
            let rest = stream_vm::without_title(&blocks, &title);
            let (shown, _more) = stream_vm::preview(&rest);
            let k2 = key.clone();
            let mut v = vec![
                div()
                    .text_size(px(text_px + 3.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(relative(1.3))
                    .child(title)
                    .into_any_element(),
            ];
            v.extend(self.render_blocks(&shown, r, &mut n, cx));
            // Selected, the action row has "Read more ⏎" instead.
            v.extend((!selected).then(|| {
                div()
                    .id("stream-read-more")
                    .debug_selector(move || format!("stream-read-more-{ix}"))
                    .mt(px(2.))
                    .font_family(self.chrome())
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.accent)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child("Read more →")
                    .tooltip(|_, cx| {
                        cx.new(|_| super::Tip("Open the whole thread · ⏎".into()))
                            .into()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.stream_read_more(k2.clone(), window, cx)
                    }))
                    .into_any_element()
            }));
            v
        } else {
            self.render_blocks(&blocks, r, &mut n, cx)
        };

        let k_click = key.clone();
        div()
            .id(("stream-post", ix))
            .relative()
            .w_full()
            .px(px(28.))
            .py(px(14.))
            .rule_b(&p)
            // --- themes --- the divider band along the post's foot.
            .children(
                crate::ornament::divider(&self.theme)
                    .map(|band| div().absolute().bottom_0().left_0().w_full().child(band)),
            )
            .when(selected, |d| d.bg(p.sel))
            // --- themes --- the selected post takes the theme's row shape.
            .map(|d| crate::ornament::page_row(&self.theme, d, selected))
            .when(opened && !selected, |d| d.bg(p.sel.opacity(0.5)))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                this.stream_post_click(k_click.clone(), post_click_of(ev), window, cx)
            }))
            .when(unread, |d| {
                // Unread: a small dot, never a count (the theme's
                // `marker.new` when it has one).
                d.child(
                    div()
                        .absolute()
                        .left(px(10.))
                        .top(px(15.))
                        .w(px(10.))
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .child(self.unread_marker(p.accent)),
                )
            })
            .child(
                div()
                    .max_w(px(680.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(meta)
                    .children(lineage)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .font_family(body_font.clone())
                            .text_size(px(text_px))
                            .line_height(relative(1.5))
                            .text_color(p.ink)
                            .children(body),
                    )
                    .when(selected, |d| d.child(self.render_stream_actions(r, cx))),
            )
            .into_any_element()
    }

    /// "↳ stub of …" / "⑂ forked from …" under the byline.
    fn render_stream_lineage(&self, r: &ReadingItem, cx: &mut Context<Self>) -> Option<AnyElement> {
        let parts = crate::app::profiles::vm::lineage(&r.lineage(), &self.reading.rows);
        if parts.is_empty() {
            return None;
        }
        let p = self.palette.on_page();
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap(px(12.))
                .font_family(self.chrome())
                .text_size(px(11.5))
                .text_color(p.muted)
                .children(parts.into_iter().map(|l| {
                    let url = l.url.clone();
                    // The name opens the profile; the rest, the post itself.
                    let post =
                        |el: Stateful<Div>, target: Option<(String, String, Option<u32>)>| {
                            el.when_some(target, |d, (o, i, v)| {
                                d.cursor_pointer()
                                    .hover(|s| s.text_color(p.accent))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.open_original(o.clone(), i.clone(), v, window, cx)
                                    }))
                            })
                        };
                    div()
                        .flex()
                        .gap(px(4.))
                        .child(
                            post(
                                div().id(SharedString::from(format!("{}-lead", l.id))),
                                l.target.clone(),
                            )
                            .child(l.lead),
                        )
                        .child(
                            div()
                                .id(l.id)
                                .cursor_pointer()
                                .text_color(p.ink)
                                .border_b_1()
                                .border_dashed()
                                .border_color(p.muted.opacity(0.6))
                                .hover(|s| s.text_color(p.accent))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.open_profile(url.clone(), window, cx)
                                }))
                                .child(l.name),
                        )
                        .when(!l.rest.is_empty(), |d| {
                            d.child(
                                post(
                                    div().id(SharedString::from(format!("{}-rest", l.id))),
                                    l.target.clone(),
                                )
                                .ml(px(-4.))
                                .child(l.rest),
                            )
                        })
                }))
                .into_any_element(),
        )
    }

    /// The selected post's actions, as in the reader: each names what it
    /// makes.
    fn render_stream_actions(&self, r: &ReadingItem, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette.on_page();
        let key = vm::key(r);
        let blyg = self
            .reading
            .subs
            .iter()
            .find(|s| s.id == r.subscription_id)
            .is_none_or(|s| s.kind == blyg_core::SubscriptionKind::Blyg);
        let ctx = vm::ActionCtx {
            blyg,
            version: r.version,
            current: r.version,
            pins: vec![],
        };
        let k_more = key.clone();
        let mut row: Vec<AnyElement> = vec![
            self.chip("stream-act-open", "Read more ⏎")
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.stream_read_more(k_more.clone(), window, cx)
                }))
                .into_any_element(),
        ];
        if r.state != "tombstone" {
            for id in [
                "Quote",
                "Reply",
                "AI reply",
                "Link post",
                "Open on web",
                "Notes",
            ] {
                if id == "Link post" && !blyg {
                    continue; // a feed post has no id to link
                }
                let chip = vm::action_chip(id, &ctx);
                let tip = chip.tip.clone();
                let k = key.clone();
                row.push(
                    self.chip(format!("stream-act-{id}"), chip.label)
                        .debug_selector(move || format!("stream-act-{id}")) // --- notes ---
                        .tooltip(move |_, cx| cx.new(|_| super::Tip(tip.clone())).into())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.stream_action(k.clone(), id, window, cx)
                        }))
                        .into_any_element(),
                );
            }
        }
        let thumb = r.thumb;
        let t = |id: &'static str, glyph: &'static str, want: i8| {
            let k = key.clone();
            let on = thumb == Some(want);
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_thumb_for(k.clone(), thumb, want == 1, cx)
                }))
                .into_any_element()
        };
        row.push(div().flex_1().into_any_element());
        // --- notes --- the thumbs wrap together (the row grew a chip).
        row.push(
            div()
                .flex_none()
                .flex()
                .gap(px(6.))
                .child(t("stream-thumb-up", "👍", 1))
                .child(t("stream-thumb-down", "👎", -1))
                .into_any_element(),
        );
        div()
            .id("stream-actions")
            .debug_selector(|| "stream-actions".into())
            .mt(px(4.))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .children(row)
            .into_any_element()
    }

    #[cfg(test)]
    pub(crate) fn stream_action_for_test(
        &mut self,
        key: Key,
        action: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stream_action(key, action, window, cx)
    }

    fn stream_action(
        &mut self,
        key: Key,
        action: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self
            .reading
            .rows
            .iter()
            .find(|r| vm::key(r) == key)
            .cloned()
        else {
            return;
        };
        self.item_action(item, action, window, cx);
    }

    // ------------------------------------------------------------ blocks

    fn render_blocks(
        &self,
        blocks: &[Block],
        post: &ReadingItem,
        n: &mut usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        blocks
            .iter()
            .map(|b| self.render_block(b, post, n, cx))
            .collect()
    }

    fn render_block(
        &self,
        b: &Block,
        post: &ReadingItem,
        n: &mut usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_page();
        match b {
            Block::Para(spans) => self.render_spans(spans, post, n, None, cx),
            Block::Heading(level, spans) => div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(match level {
                    1 => 21.,
                    2 => 18.5,
                    _ => 16.5,
                }))
                .line_height(relative(1.3))
                .child(self.render_spans(spans, post, n, None, cx))
                .into_any_element(),
            Block::Quote(inner) => div()
                .pl(px(12.))
                .border_l_2()
                .border_color(p.quote_rule)
                .text_color(p.muted)
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(self.render_blocks(inner, post, n, cx))
                .into_any_element(),
            Block::List { ordered, items } => div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .children(items.iter().enumerate().map(|(i, item)| {
                    let marker = match ordered {
                        Some(start) => format!("{}.", *start as usize + i),
                        None => "•".into(),
                    };
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_none()
                                .min_w(px(16.))
                                .text_color(p.muted)
                                .child(marker),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .children(self.render_blocks(item, post, n, cx)),
                        )
                }))
                .into_any_element(),
            Block::Code(code) => div()
                .px(px(10.))
                .py(px(7.))
                .rounded(px(self.theme.corner(6.)))
                .bg(p.sel)
                .font_family("Menlo")
                .text_size(px(12.5))
                .line_height(relative(1.45))
                .child(code.clone())
                .into_any_element(),
            Block::Rule => div().h(px(1.)).my(px(4.)).bg(p.line).into_any_element(),
            Block::Image { alt, .. } => placeholder(
                p,
                if alt.trim().is_empty() {
                    "▣ image".to_string()
                } else {
                    format!("▣ image · {}", alt.trim())
                },
            ),
            Block::Embed { url } => placeholder(p, format!("▶ video · {}", vm::host(url))),
            Block::Table => placeholder(p, "▦ table · open the post to see it".into()),
            Block::Transclusion { id, excerpt } => {
                self.render_quote_box(post, id, excerpt.as_deref(), n, cx)
            }
        }
    }

    /// A run of styled text; links are clickable (resolved against the
    /// post's origin, opened in the browser).
    fn render_spans(
        &self,
        spans: &[Span],
        post: &ReadingItem,
        n: &mut usize,
        size: Option<f32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_page();
        let mut text = String::new();
        let mut hl: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
        let mut links: Vec<(std::ops::Range<usize>, Target)> = Vec::new();
        for s in spans {
            let start = text.len();
            // An `[[id]]` link reads as the target's excerpt when it's held.
            let label = s.item.as_deref().map(|id| {
                let held = stream_vm::quoted(&self.reading.rows, post, id);
                let (origin, _) = stream_vm::quote_ref(post, id, held);
                (stream_vm::link_label(held), origin, id.to_string())
            });
            match &label {
                Some((l, ..)) => text.push_str(l),
                None => text.push_str(&s.text),
            }
            let r = start..text.len();
            if r.is_empty() {
                continue;
            }
            let mut st = HighlightStyle::default();
            let mut styled = false;
            if s.em {
                st.font_style = Some(FontStyle::Italic);
                styled = true;
            }
            if s.strong {
                st.font_weight = Some(FontWeight::BOLD);
                styled = true;
            }
            if s.code {
                st.background_color = Some(p.sel);
                styled = true;
            }
            if s.strike {
                st.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(p.muted),
                });
                styled = true;
            }
            let target = match label {
                Some((_, origin, id)) => Some(Target::Post(origin, id)),
                None => s
                    .link
                    .as_deref()
                    .and_then(|h| link_target(h, &post.origin))
                    .map(Target::Url),
            };
            if let Some(t) = target {
                st.color = Some(p.accent);
                st.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(p.accent.opacity(0.5)),
                    wavy: false,
                });
                styled = true;
                links.push((r.clone(), t));
            }
            if styled {
                hl.push((r, st));
            }
        }
        *n += 1;
        let styled = StyledText::new(text).with_highlights(hl);
        let el = if links.is_empty() {
            div().child(styled).into_any_element()
        } else {
            let (ranges, targets): (Vec<_>, Vec<_>) = links.into_iter().unzip();
            // Links go where the reader's do: the browser pane, or the
            // default browser (click modifiers and `open-links` decide). An
            // `[[id]]` link opens the post like a quote box does.
            let view = cx.entity().downgrade();
            InteractiveText::new(("stream-text", *n), styled)
                .on_click(ranges, move |i, window, cx| {
                    let t = targets.get(i).cloned();
                    let _ = view.update(cx, |this, cx| match t {
                        Some(Target::Url(u)) => this.open_link(u, window, cx),
                        Some(Target::Post(o, id)) => this.open_original(o, id, None, window, cx),
                        None => {}
                    });
                })
                .into_any_element()
        };
        match size {
            Some(s) => div().text_size(px(s)).child(el).into_any_element(),
            None => el,
        }
    }

    /// A `![[id]]` quote: a grey box with the quoted text when it's held
    /// here, and a footer naming where it's from.
    fn render_quote_box(
        &self,
        post: &ReadingItem,
        id: &str,
        excerpt: Option<&str>,
        n: &mut usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette.on_page();
        let held = stream_vm::quoted(&self.reading.rows, post, id);
        let (origin, version) = stream_vm::quote_ref(post, id, held);
        let cited = stream_vm::cited(post, id);
        *n += 1;
        let box_id = *n;
        let quoted: Vec<AnyElement> = match held {
            // A partial quote (§16.4): only the passage, never the whole post.
            _ if let Some(e) = excerpt => e
                .split('\n')
                .map(|para| div().child(para.to_string()).into_any_element())
                .collect(),
            Some(q) if !q.content_md.trim().is_empty() || !q.content_html.trim().is_empty() => {
                let blocks: Vec<Block> = post_blocks(q)
                    .into_iter()
                    // No quotes inside quotes: the box stays small.
                    .filter(|b| !matches!(b, Block::Transclusion { .. }))
                    .collect();
                let (shown, _) = stream_vm::preview(&blocks);
                self.render_blocks(&shown, q, n, cx)
            }
            // Not held: the citation the quoting blyg froze, when it sent one.
            _ => vec![
                match cited.and_then(|c| c.excerpt.as_deref()).map(str::trim) {
                    Some(e) if !e.is_empty() => div().child(format!("“{e}”")).into_any_element(),
                    _ => div()
                        .italic()
                        .text_color(p.muted)
                        .child("The quoted post isn't held on this Mac.")
                        .into_any_element(),
                },
            ],
        };
        let name = held
            .and_then(|q| q.author.as_ref().and_then(|a| a.name.clone()))
            .filter(|s| !s.trim().is_empty())
            .or_else(|| cited.and_then(|c| c.name()).map(str::to_string))
            .unwrap_or_else(|| vm::host(&origin));
        let prof = origin.clone();
        let (q_origin, q_id) = (origin.clone(), id.to_string());
        div()
            .id(("stream-quote", box_id))
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_size(px((self.prefs.font_size * 15.0 / 19.0).round()))
            .cursor_pointer()
            // --- quote targets --- the quoted text opens the original (at
            // the quoted version); the footer's name, the profile.
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.open_original(q_origin.clone(), q_id.clone(), version, window, cx);
            }))
            .children(quoted)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .font_family(self.chrome())
                    .text_size(px(11.))
                    .text_color(p.muted)
                    .child("quoted from")
                    .child(
                        div()
                            .id(("stream-quote-origin", box_id))
                            .text_color(p.ink)
                            .border_b_1()
                            .border_dashed()
                            .border_color(p.muted.opacity(0.6))
                            .hover(|s| s.text_color(p.accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_profile(prof.clone(), window, cx)
                            }))
                            .child(name),
                    )
                    .when_some(version, |d, v| d.child(format!("· v{v}")))
                    .child("· open original"),
            )
            // --- themes --- the frame (the classic box when unset).
            .map(|d| crate::ornament::quote_frame(&self.theme, d))
    }
}

fn badge(p: crate::theme::Palette, s: String) -> Div {
    let (fg, bg) = p.quiet_badge();
    div()
        .px(px(6.))
        .rounded_full()
        .text_size(px(10.5))
        .text_color(fg)
        .bg(bg)
        .child(s)
}

fn placeholder(p: crate::theme::Palette, label: String) -> AnyElement {
    div()
        .flex()
        .child(
            div()
                .px(px(8.))
                .py(px(3.))
                .rounded(px(5.))
                .border_1()
                .border_color(p.line)
                .font_family("Inter")
                .text_size(px(11.5))
                .text_color(p.muted)
                .child(label),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{html_paragraphs, link_target};

    #[test]
    fn html_only_items_become_paragraphs() {
        let p = html_paragraphs("<h1>On kites</h1><p class=\"x\">Wind &amp; string.</p>tail");
        assert_eq!(p, ["On kites", "Wind & string.", "tail"]);
    }

    #[test]
    fn links_resolve_against_the_origin_and_refuse_scripts() {
        let o = "https://ada.blyg.example.com/";
        assert_eq!(
            link_target("t/01ABC", o).as_deref(),
            Some("https://ada.blyg.example.com/t/01ABC")
        );
        assert_eq!(
            link_target("/f/x", o).as_deref(),
            Some("https://ada.blyg.example.com/f/x")
        );
        assert_eq!(
            link_target("https://b.example/", o).as_deref(),
            Some("https://b.example/")
        );
        assert_eq!(link_target("javascript:alert(1)", o), None);
        assert_eq!(
            link_target("mailto:a@b.example", o).as_deref(),
            Some("mailto:a@b.example")
        );
    }
}
