//! Pure rules for the reading stream (issue #1): the Stream | Reader
//! choice, when a post counts as read, what a thread's preview shows, and
//! where a quote box's text comes from. `stream.rs` renders what these say.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use blyg_core::{Kind, ReadingItem};
use blyg_render::Block;

use super::vm::Key;

// ---------------------------------------------------------------- mode

/// How the reading screen shows posts. Stream is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadMode {
    /// One native timeline, newest first; "Read more" opens a side pane.
    #[default]
    Stream,
    /// The list on the left, the open post on the right.
    Reader,
}

impl ReadMode {
    pub const ALL: [ReadMode; 2] = [ReadMode::Stream, ReadMode::Reader];

    pub fn label(self) -> &'static str {
        match self {
            ReadMode::Stream => "Stream",
            ReadMode::Reader => "Reader",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            ReadMode::Stream => "⌥⌘1",
            ReadMode::Reader => "⌥⌘2",
        }
    }

    /// The value kept in `state.json` (`reading_mode`).
    pub fn as_str(self) -> &'static str {
        match self {
            ReadMode::Stream => "stream",
            ReadMode::Reader => "reader",
        }
    }

    pub fn parse(s: &str) -> Option<ReadMode> {
        match s {
            "stream" => Some(ReadMode::Stream),
            "reader" => Some(ReadMode::Reader),
            _ => None,
        }
    }
}

/// The remembered mode (per viewer, `state.json`); Stream when unset.
pub fn load_mode(data_dir: Option<&std::path::Path>) -> ReadMode {
    data_dir
        .map(blyg_core::state::AppState::load)
        .and_then(|s| s.reading_mode.as_deref().and_then(ReadMode::parse))
        .unwrap_or_default()
}

pub fn save_mode(data_dir: Option<&std::path::Path>, mode: ReadMode) {
    if let Some(d) = data_dir {
        let _ =
            blyg_core::state::AppState::update(d, |s| s.reading_mode = Some(mode.as_str().into()));
    }
}

// ---------------------------------------------------------------- read tracking

/// How long a post must stay on screen to count as read (the user's call:
/// one second).
pub const READ_DWELL: Duration = Duration::from_secs(1);

/// On screen enough to count: at least half the post is visible, or (for a
/// post taller than that) it fills at least half the viewport.
pub fn seen_enough(item_top: f32, item_bottom: f32, view_top: f32, view_bottom: f32) -> bool {
    let h = item_bottom - item_top;
    let view_h = view_bottom - view_top;
    if h <= 0.0 || view_h <= 0.0 {
        return false;
    }
    let visible = (item_bottom.min(view_bottom) - item_top.max(view_top)).max(0.0);
    visible >= 0.5 * h || visible >= 0.5 * view_h
}

/// Which posts have been on screen long enough. Feed it the posts that are
/// on screen now, every tick; it answers the ones to mark read, once each.
#[derive(Debug, Default)]
pub struct ReadTracker {
    since: HashMap<Key, Instant>,
    marked: HashSet<Key>,
}

impl ReadTracker {
    /// `on_screen`: unread posts seen enough right now. A post that leaves
    /// the screen starts over when it comes back.
    pub fn observe(&mut self, on_screen: &[Key], now: Instant) -> Vec<Key> {
        self.since.retain(|k, _| on_screen.contains(k));
        let mut due = Vec::new();
        for k in on_screen {
            if self.marked.contains(k) {
                continue;
            }
            let t0 = *self.since.entry(k.clone()).or_insert(now);
            if now.duration_since(t0) >= READ_DWELL {
                self.marked.insert(k.clone());
                self.since.remove(k);
                due.push(k.clone());
            }
        }
        due
    }

    /// Forget a post (it was edited, so the new version is unread again).
    pub fn forget(&mut self, k: &Key) {
        self.marked.remove(k);
        self.since.remove(k);
    }

    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.since.len()
    }
}

// ---------------------------------------------------------------- previews

/// Lines a thread shows in the stream before "Read more".
pub const PREVIEW_LINES: usize = 4;
/// Characters per line, for estimating (the column is about 38em wide).
const LINE_CHARS: usize = 80;

fn lines_of(b: &Block) -> usize {
    match b {
        Block::Rule => 1,
        Block::Image { .. } | Block::Embed { .. } | Block::Table => 1,
        Block::Transclusion { .. } => 2,
        Block::Code(c) => c.lines().count().max(1),
        Block::List { items, .. } => items.len().max(1),
        b => b.plain().chars().count().div_ceil(LINE_CHARS).max(1),
    }
}

/// A post's blocks without its title line (a thread's first block of its own
/// words is its title, shown on its own). Quotes before it stay: the title
/// is never quoted text, so a stub's title comes after what it quotes.
pub fn without_title(blocks: &[Block], title: &str) -> Vec<Block> {
    let first_own = blocks.iter().position(|b| {
        !matches!(
            b,
            Block::Transclusion { .. } | Block::Quote(_) | Block::Image { .. }
        )
    });
    match first_own.map(|i| (i, &blocks[i])) {
        Some((i, b @ (Block::Para(_) | Block::Heading(..))))
            if b.plain().trim() == title.trim() =>
        {
            let mut out = blocks.to_vec();
            out.remove(i);
            out
        }
        _ => blocks.to_vec(),
    }
}

/// A thread's preview: its first blocks, about [`PREVIEW_LINES`] lines, the
/// last one cut short with "…". `true` when anything was left out.
pub fn preview(blocks: &[Block]) -> (Vec<Block>, bool) {
    let mut out = Vec::new();
    let mut budget = PREVIEW_LINES;
    for b in blocks {
        if budget == 0 {
            return (out, true);
        }
        let n = lines_of(b);
        if n <= budget {
            out.push(b.clone());
            budget -= n;
            continue;
        }
        match b {
            Block::Para(spans) => {
                out.push(Block::Para(clip_spans(spans, budget * LINE_CHARS)));
            }
            Block::Code(c) => {
                let kept: Vec<&str> = c.lines().take(budget).collect();
                out.push(Block::Code(format!("{}\n…", kept.join("\n"))));
            }
            _ if out.is_empty() => out.push(b.clone()),
            _ => {}
        }
        return (out, true);
    }
    (out, false)
}

fn clip_spans(spans: &[blyg_render::Span], max: usize) -> Vec<blyg_render::Span> {
    let mut left = max;
    let mut out = Vec::new();
    for s in spans {
        let n = s.text.chars().count();
        if n <= left {
            out.push(s.clone());
            left -= n;
            continue;
        }
        let mut cut: String = s.text.chars().take(left).collect();
        // Back up to a word boundary when there is one.
        if let Some(i) = cut
            .rfind(char::is_whitespace)
            .filter(|&i| i > cut.len() / 2)
        {
            cut.truncate(i);
        }
        out.push(blyg_render::Span {
            text: format!("{}…", cut.trim_end()),
            ..s.clone()
        });
        return out;
    }
    if let Some(last) = out.last_mut() {
        last.text.push('…');
    }
    out
}

/// The kind as the stream shows it.
pub fn kind_word(kind: Kind) -> &'static str {
    crate::vm::kind_label(kind)
}

// ---------------------------------------------------------------- quotes

/// The held post a `![[id]]` in `post` quotes: the origin its
/// `transclusions[]` names for that id (else the post's own origin), and
/// failing that any held post with that id (ids are unique).
pub fn quoted<'a>(
    rows: &'a [ReadingItem],
    post: &ReadingItem,
    id: &str,
) -> Option<&'a ReadingItem> {
    let origin = post
        .transclusions
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(id))
        .and_then(|t| t.origin.clone())
        .unwrap_or_else(|| post.origin.clone());
    let same_id = |r: &&ReadingItem| r.remote_id.eq_ignore_ascii_case(id);
    rows.iter()
        .filter(same_id)
        .find(|r| blyg_core::profile::same_origin(&r.origin, &origin))
        .or_else(|| rows.iter().find(same_id))
}

/// The citation the post's `transclusions[]` entry carries for `id`.
pub fn cited<'a>(post: &'a ReadingItem, id: &str) -> Option<&'a blyg_core::Cited> {
    post.transclusions
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(id))
        .and_then(|t| t.cited.as_ref())
}

/// The text an `[[id]]` link shows: the held target's excerpt in quotes,
/// as the published page's anchor reads, else a neutral label.
pub fn link_label(held: Option<&ReadingItem>) -> String {
    let Some(h) = held else {
        return blyg_render::LINK_LABEL.to_string();
    };
    let html = if h.content_html.trim().is_empty() {
        blyg_render::render_markdown(&h.content_md)
    } else {
        h.content_html.clone()
    };
    let e = blyg_render::excerpt_from_html(&html, 60);
    if e.is_empty() {
        blyg_render::LINK_LABEL.to_string()
    } else {
        format!("“{e}”")
    }
}

/// The origin and version a quote box names: the `transclusions[]` entry
/// when there is one, else the held post, else the quoting post's origin.
pub fn quote_ref(
    post: &ReadingItem,
    id: &str,
    held: Option<&ReadingItem>,
) -> (String, Option<u32>) {
    let t = post
        .transclusions
        .iter()
        .find(|t| t.id.eq_ignore_ascii_case(id));
    let origin = t
        .and_then(|t| t.origin.clone())
        .or_else(|| held.map(|h| h.origin.clone()))
        .unwrap_or_else(|| post.origin.clone());
    let version = t.and_then(|t| t.version).or(held.map(|h| h.version));
    (origin, version)
}

// ---------------------------------------------------------------- clicks

/// How far the pointer may move between press and release for a click
/// still to be a click (logical pixels).
pub const DRAG_SLOP: f32 = 6.0;

/// What a mouse click on a post in the stream does. The stream is drawn
/// natively and its text can't be selected, so the gestures that mean "I
/// want this text" open the post in the side pane, where it can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostClick {
    /// A plain click: select the post.
    Select,
    /// A double-click: open it in the side pane (as ⏎ does).
    Open,
    /// A press-and-drag, the way text is selected: open it in the side
    /// pane and say that's where to select.
    OpenToSelect,
}

/// A click that moved (`dx`, `dy`) between press and release, the
/// `clicks`-th in a row.
pub fn post_click(dx: f32, dy: f32, clicks: usize) -> PostClick {
    if dx.hypot(dy) > DRAG_SLOP {
        PostClick::OpenToSelect
    } else if clicks >= 2 {
        PostClick::Open
    } else {
        PostClick::Select
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blyg_render::{Span, native_blocks};

    fn k(s: &str) -> Key {
        ("sub".into(), s.into())
    }

    #[test]
    fn a_drag_or_a_double_click_opens_the_post_to_select_in() {
        assert_eq!(post_click(0., 0., 1), PostClick::Select);
        assert_eq!(post_click(3., -4., 1), PostClick::Select, "a shaky click");
        assert_eq!(post_click(1., 0., 2), PostClick::Open);
        assert_eq!(post_click(40., 2., 1), PostClick::OpenToSelect);
        assert_eq!(
            post_click(0., 12., 2),
            PostClick::OpenToSelect,
            "a double-click-drag selects words"
        );
    }

    #[test]
    fn modes_round_trip_and_default_to_stream() {
        assert_eq!(ReadMode::default(), ReadMode::Stream);
        for m in ReadMode::ALL {
            assert_eq!(ReadMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(ReadMode::parse("gallery"), None);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_mode(Some(dir.path())), ReadMode::Stream);
        save_mode(Some(dir.path()), ReadMode::Reader);
        assert_eq!(load_mode(Some(dir.path())), ReadMode::Reader);
        assert_eq!(load_mode(None), ReadMode::Stream);
    }

    #[test]
    fn half_visible_counts() {
        // 100px post, viewport 0..500.
        assert!(seen_enough(0.0, 100.0, 0.0, 500.0));
        assert!(seen_enough(450.0, 550.0, 0.0, 500.0), "exactly half");
        assert!(!seen_enough(460.0, 560.0, 0.0, 500.0), "40% visible");
        assert!(!seen_enough(600.0, 700.0, 0.0, 500.0), "below");
        // A post taller than the viewport: filling half the screen counts.
        assert!(seen_enough(-1000.0, 2000.0, 0.0, 500.0));
        assert!(!seen_enough(300.0, 3000.0, 0.0, 500.0), "only 200px shown");
        assert!(!seen_enough(0.0, 0.0, 0.0, 500.0));
    }

    #[test]
    fn a_post_is_read_after_one_second_on_screen() {
        let mut t = ReadTracker::default();
        let t0 = Instant::now();
        assert!(t.observe(&[k("a"), k("b")], t0).is_empty());
        assert!(
            t.observe(&[k("a"), k("b")], t0 + Duration::from_millis(900))
                .is_empty()
        );
        // b scrolled away just before the second: it starts over.
        let due = t.observe(&[k("a")], t0 + Duration::from_millis(1000));
        assert_eq!(due, vec![k("a")]);
        assert_eq!(t.pending(), 0);
        let t1 = t0 + Duration::from_millis(1100);
        assert!(t.observe(&[k("a"), k("b")], t1).is_empty(), "a only once");
        assert!(
            t.observe(&[k("a"), k("b")], t1 + Duration::from_millis(999))
                .is_empty()
        );
        assert_eq!(
            t.observe(&[k("b")], t1 + Duration::from_millis(1000)),
            vec![k("b")]
        );
        t.forget(&k("a"));
        assert!(t.observe(&[k("a")], t1).is_empty());
        assert_eq!(
            t.observe(&[k("a")], t1 + READ_DWELL),
            vec![k("a")],
            "an edited post can be read again"
        );
    }

    #[test]
    fn thread_previews_are_about_four_lines() {
        let long = "word ".repeat(200);
        let md = format!("Title\n\nShort one.\n\n{long}\n\nNever shown.");
        let blocks = native_blocks(&md, blyg_render::Kind::Thread);
        let body = without_title(&blocks, "Title");
        assert_eq!(body[0].plain(), "Short one.");
        let (p, more) = preview(&body);
        assert!(more);
        assert_eq!(p.len(), 2);
        let clipped = p[1].plain();
        assert!(clipped.ends_with('…'));
        assert!(clipped.chars().count() <= 3 * 80 + 1, "{}", clipped.len());
        assert!(!p.iter().any(|b| b.plain().contains("Never")));
        // A short thread shows whole, with nothing more to read.
        let short = native_blocks("Title\n\nJust this.", blyg_render::Kind::Thread);
        let (p, more) = preview(&without_title(&short, "Title"));
        assert!(!more);
        assert_eq!(p.len(), 1);
    }

    /// A stub's title is its own first line, after what it quotes: that line
    /// isn't shown twice, and the quote stays.
    #[test]
    fn a_stubs_title_comes_after_its_quote() {
        let md = "![[0a1b2c3d4e5f6g7h8j9k0m1n2p]]\n\n> theirs\n\nMine.\n\nMore.";
        let title = blyg_core::plain_title(md).unwrap();
        assert_eq!(title, "Mine.");
        let blocks = native_blocks(md, blyg_render::Kind::Thread);
        let body = without_title(&blocks, &title);
        assert_eq!(body.len(), blocks.len() - 1);
        assert!(matches!(body[0], Block::Transclusion { .. }));
        assert!(!body.iter().any(|b| b.plain() == "Mine."));
        assert_eq!(body.last().unwrap().plain(), "More.");
    }

    #[test]
    fn clipping_keeps_styles() {
        let spans = vec![
            Span {
                text: "aaaa ".into(),
                ..Default::default()
            },
            Span {
                text: "bbbb cccc dddd".into(),
                em: true,
                ..Default::default()
            },
        ];
        let c = clip_spans(&spans, 12);
        assert_eq!(c.len(), 2);
        assert!(c[1].em);
        assert_eq!(c[1].text, "bbbb…");
    }

    fn item(sub: &str, origin: &str, id: &str) -> ReadingItem {
        let mut r = crate::fake::reading_seed::seed(chrono::Utc::now())
            .reading
            .remove(0);
        r.subscription_id = sub.into();
        r.origin = origin.into();
        r.remote_id = id.into();
        r
    }

    #[test]
    fn quotes_resolve_from_held_posts() {
        let a = item("sa", "https://a.example/", "01AAAAAAAAAAAAAAAAAAAAAAAA");
        let b = item("sb", "https://b.example/", "01AAAAAAAAAAAAAAAAAAAAAAAA");
        let rows = vec![a.clone(), b.clone()];
        let mut post = item("sa", "https://a.example/", "01POSTPOSTPOSTPOSTPOSTPOST");
        // No transclusions[]: the post's own origin first.
        assert_eq!(
            quoted(&rows, &post, "01aaaaaaaaaaaaaaaaaaaaaaaa").map(|r| &r.subscription_id),
            Some(&a.subscription_id)
        );
        post.transclusions = vec![blyg_core::TransclusionRef {
            id: "01AAAAAAAAAAAAAAAAAAAAAAAA".into(),
            version: Some(3),
            origin: Some("https://b.example".into()),
            cited: None,
            selector: None,
        }];
        let held = quoted(&rows, &post, "01AAAAAAAAAAAAAAAAAAAAAAAA");
        assert_eq!(held.map(|r| r.subscription_id.as_str()), Some("sb"));
        assert_eq!(
            quote_ref(&post, "01AAAAAAAAAAAAAAAAAAAAAAAA", held),
            ("https://b.example".to_string(), Some(3))
        );
        assert!(quoted(&rows, &post, "01ZZZZZZZZZZZZZZZZZZZZZZZZ").is_none());
    }

    #[test]
    fn links_are_labelled_by_the_held_target() {
        assert_eq!(link_label(None), blyg_render::LINK_LABEL);
        let mut held = item("sub", "https://b.example", "01AAAAAAAAAAAAAAAAAAAAAAAA");
        held.content_md = "Tides keep *time*.".into();
        held.content_html = String::new();
        assert_eq!(link_label(Some(&held)), "“Tides keep time.”");
    }

    #[test]
    fn citations_ride_on_the_transclusion() {
        let mut post = item("sub", "https://a.example", "P");
        let json = r#"[{"id":"01AAAAAAAAAAAAAAAAAAAAAAAA","version":3,"origin":"https://b.example/","cited":{"source":"B Blyg","author":"  ","excerpt":"what they said","url":7}}]"#;
        post.transclusions = serde_json::from_str(json).unwrap();
        let c = cited(&post, "01aaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        assert_eq!(c.excerpt.as_deref(), Some("what they said"));
        assert_eq!(c.url, None, "a field of the wrong type reads as absent");
        assert_eq!(c.name(), Some("B Blyg"));
    }
}
