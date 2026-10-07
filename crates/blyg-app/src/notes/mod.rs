//! --- notes --- The notes drawer (docs/SPEC.md § Notes drawer).
//!
//! A scratchpad that slides in over the right edge of the window while you
//! read (⇧⌘N, View › Notes): running notes, links to posts and pages, and
//! quoted selections, for writing better posts later. It never adds a layout
//! column: it's an overlay, and the web views it overlaps are cut off at its
//! edge (they draw above GPUI).
//!
//! It's backed by an ordinary scratch note (local only, in the Posts list
//! with its `scratch` pill; ⌘D makes it a draft). One rolling note titled
//! "Reading notes" by default, made a thread so `![[id]]` quotes are valid
//! in it; "New notes page" starts another and leaves the old one in Posts.
//!
//! - this file: the text the drawer appends (pure, unit-tested) and its state;
//! - `drawer`: the `MainView` hooks, the drawer itself and its demos.

mod drawer;
mod quote; // universal quoting (⇧⌘D)

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use gpui_kit::base::input::TextareaState;
use gpui_kit::*;

use blyg_core::LocalId;

pub(crate) use drawer::NotesPark;
pub use drawer::{SELECTION_JS, ToggleNotes}; // --- onboarding ---
pub(crate) use quote::QuoteFrom;
pub use quote::QuoteToDraft;

use crate::vm;

/// The drawer's width.
pub const WIDTH: f32 = 380.;

/// How long the slide in (and out) takes.
pub const SLIDE_MS: u64 = 180;

/// The rolling note's title (its first line, a heading).
pub const TITLE: &str = "Reading notes";

/// The key context inside the drawer.
pub const CONTEXT: &str = "Notes";

/// A quoted selection is cut off here (a page can't flood the note).
pub const MAX_QUOTE: usize = 4000;

// ------------------------------------------------------------ formatting

/// `[title](url)`: brackets in the title escaped, newlines folded, a
/// destination with parentheses wrapped in `<…>` (`vm::escape_link_text`,
/// `vm::link_destination`). An empty title shows the URL.
pub fn link(title: &str, url: &str) -> String {
    let t: String = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\\', "\\\\");
    let t = if t.is_empty() { url.to_string() } else { t };
    format!(
        "[{}]({})",
        vm::escape_link_text(&t),
        vm::link_destination(url.trim())
    )
}

/// Every line of `text` as a `>` quote (blank lines stay in the quote).
pub fn blockquote(text: &str) -> String {
    text.trim()
        .lines()
        .map(|l| {
            let l = l.trim_end();
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A selection, quoted, with where it came from:
///
/// ```text
/// > the selected text
/// >
/// > — [Source title](https://…)
/// ```
pub fn quote_with_source(selection: &str, title: &str, url: Option<&str>) -> String {
    let sel: String = selection.chars().take(MAX_QUOTE).collect();
    let q = blockquote(&sel);
    match url {
        Some(u) => format!("{q}\n>\n> — {}", link(title, u)),
        None if !title.trim().is_empty() => format!("{q}\n>\n> — {}", title.trim()),
        None => q,
    }
}

/// A selection in the notes (byte `range` of `text`) as a passage to quote
/// elsewhere, and the page it came from when the note says so: `>` markers
/// dropped, and, when the selection sits in a quote ending in a
/// `> — [title](url)` source line (what [`quote_with_source`] writes), that
/// title and URL. `None` for an empty selection. A note has no source URL of
/// its own, so anything else is quoted without a link.
pub fn notes_passage(
    text: &str,
    range: std::ops::Range<usize>,
) -> Option<(String, Option<(String, String)>)> {
    let (start, end) = (range.start.min(text.len()), range.end.min(text.len()));
    let selected = text.get(start..end)?;
    let passage: Vec<&str> = selected
        .lines()
        .map(|l| {
            let t = l.trim_start();
            match t.strip_prefix('>') {
                Some(rest) => rest.strip_prefix(' ').unwrap_or(rest),
                None => l,
            }
        })
        .filter(|l| source_line(l).is_none())
        .collect();
    let passage = passage.join("\n");
    if passage.trim().is_empty() {
        return None;
    }
    // The quote block around the selection's first line.
    let lines: Vec<&str> = text.split('\n').collect();
    let mut at = 0;
    let mut first = 0;
    for (i, l) in lines.iter().enumerate() {
        if at + l.len() >= start {
            first = i;
            break;
        }
        at += l.len() + 1;
    }
    let quoted = |i: usize| lines[i].trim_start().starts_with('>');
    let link = quoted(first)
        .then(|| {
            (first..lines.len())
                .take_while(|&i| quoted(i))
                .find_map(|i| source_line(lines[i].trim_start().trim_start_matches('>').trim()))
        })
        .flatten();
    Some((passage, link))
}

/// `— [title](url)` (a source line) as its title and URL.
fn source_line(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix("— [")?;
    let close = rest.rfind("](")?;
    let url = rest[close + 2..].strip_suffix(')')?;
    let url = url
        .strip_prefix('<')
        .and_then(|u| u.strip_suffix('>'))
        .unwrap_or(url);
    let mut title = String::new();
    let mut chars = rest[..close].chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            title.extend(chars.next());
        } else {
            title.push(c);
        }
    }
    Some((title, url.to_string()))
}

/// What "→ Notes" adds for a post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostRef {
    /// The post's id, when it's a blyg post (transcludable).
    pub blyg_id: Option<String>,
    /// Its title (or first line).
    pub title: String,
    /// Its first line of text (quoted under the link when there's no `![[…]]`).
    pub first_line: String,
    pub url: Option<String>,
}

/// The block for a post: `![[id]]` for a blyg post when the note may hold
/// quotes (a thread); otherwise a link, plus, for a blyg post, its first
/// line as a `>` quote. RSS posts get the link.
pub fn post_block(post: &PostRef, note_takes_quotes: bool) -> String {
    if let (Some(id), true) = (&post.blyg_id, note_takes_quotes) {
        return format!("![[{id}]]");
    }
    let head = match &post.url {
        Some(u) => link(&post.title, u),
        None => post.title.trim().to_string(),
    };
    let first = post.first_line.trim();
    if post.blyg_id.is_some() && !first.is_empty() && first != post.title.trim() {
        format!("{head}\n{}", blockquote(first))
    } else {
        head
    }
}

/// Append `block` as its own paragraph at the end of `text`. Returns the new
/// text and the caret: on a new line below the block, ready to type.
pub fn append(text: &str, block: &str) -> (String, usize) {
    let mut out = text.trim_end_matches([' ', '\t', '\n']).to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block.trim_end());
    out.push_str("\n\n");
    let caret = out.len();
    (out, caret)
}

/// A new page's text: the title as a heading and an empty paragraph.
pub fn page_seed(title: &str) -> String {
    format!("# {title}\n\n")
}

/// The title for a fresh notes page: "Reading notes · Sep 27".
pub fn fresh_title(today: chrono::NaiveDate) -> String {
    format!("{TITLE} · {}", today.format("%b %-d"))
}

/// The web view's answer to [`SELECTION_JS`] (a JSON string) as text.
pub fn parse_selection(json: &str) -> String {
    serde_json::from_str::<String>(json).unwrap_or_default()
}

/// Is this the drawer's kind of note (by its title)?
pub fn is_notes_title(title: &str) -> bool {
    title.trim().starts_with(TITLE)
}

// ------------------------------------------------------------ state

/// The drawer's state (a field of `MainView`).
pub struct Notes {
    /// On screen (or sliding in).
    pub open: bool,
    /// Sliding out: still drawn (and the web views still cut off).
    pub closing: bool,
    /// Times shown (a new slide-in animation each time).
    pub(crate) shown: u64,
    close_gen: u64,
    /// The scratch note it writes to, once there is one.
    pub id: Option<LocalId>,
    /// `id` has been looked up (or deliberately cleared) this session.
    resolved: bool,
    editor: Option<Entity<TextareaState>>,
    assist: Option<Entity<crate::composer::Assist>>,
    focus: Option<FocusHandle>,
    /// Where the keyboard was before the drawer took it.
    return_focus: Option<FocusHandle>,
    /// The header's ⋯ menu.
    menu: bool,
    /// A mouse press outside the drawer: it closes once the click is over,
    /// unless the click was an add action ("→ Notes"), which disarms it.
    outside_click: bool,
    /// When it last started to slide in or out (the browser pane's room
    /// follows it).
    moved: Option<std::time::Instant>,
    /// Setting the editor's text programmatically (not an edit to save).
    loading: bool,
    data_dir: Option<std::path::PathBuf>,
}

impl Notes {
    pub fn new(data_dir: Option<std::path::PathBuf>) -> Notes {
        Notes {
            open: false,
            closing: false,
            shown: 0,
            close_gen: 0,
            id: None,
            resolved: false,
            editor: None,
            assist: None,
            focus: None,
            return_focus: None,
            menu: false,
            outside_click: false,
            moved: None,
            loading: false,
            data_dir,
        }
    }

    /// Drawn this frame (open, or sliding out).
    pub fn drawn(&self) -> bool {
        self.open || self.closing
    }

    /// The drawer's left edge while it's drawn, for a window `width` wide:
    /// native views are cut off there.
    pub fn left_edge(&self, width: Pixels) -> Option<Pixels> {
        self.drawn().then(|| (width - px(WIDTH)).max(px(0.)))
    }

    /// How far the browser pane moves in from the right to make room for
    /// the drawer (its final place; `drawer::wrap_browser` animates it).
    pub fn room(&self) -> Pixels {
        if self.open { px(WIDTH) } else { px(0.) }
    }

    /// [`Self::room`] at `now`, part way through a slide.
    pub fn room_at(&self, now: std::time::Instant) -> Pixels {
        let target = f32::from(self.room());
        let Some(at) = self.moved else {
            return px(target);
        };
        let t = now.saturating_duration_since(at).as_secs_f32() * 1000. / SLIDE_MS as f32;
        if t >= 1. {
            return px(target);
        }
        let eased = 1.0 - (1.0 - t).powi(5); // ease_out_quint, as the drawer
        px(if self.open {
            eased * WIDTH
        } else {
            (1. - eased) * WIDTH
        })
    }

    /// The editor's text ("" before the drawer was first opened).
    pub fn text(&self, cx: &App) -> String {
        self.editor
            .as_ref()
            .map(|e| e.read(cx).value().to_string())
            .unwrap_or_default()
    }
}
