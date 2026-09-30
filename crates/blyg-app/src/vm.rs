//! View-model logic, free of GPUI so it can be unit-tested: the omnibar/list
//! state machine, status-bar rules, publish decisions, highlight ranges,
//! relative time, image placeholders and the conflict word diff.

use std::ops::Range;

use blyg_core::{FRAGMENT_LIMIT, Item, Kind, LocalId, Status, SyncStatus};

// ---------------------------------------------------------------- list / omnibar

/// What ⏎ in the omnibar should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnterAction {
    Open(LocalId),
    /// No matches for a non-empty query: create a draft seeded with it.
    Create(String),
    Nothing,
}

/// The NV list: current query, the backend's results for it, and the selection.
#[derive(Debug, Default)]
pub struct ListModel {
    query: String,
    results: Vec<Item>,
    selected: Option<LocalId>,
}

impl ListModel {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn trimmed_query(&self) -> &str {
        self.query.trim()
    }

    pub fn results(&self) -> &[Item] {
        &self.results
    }

    pub fn selected(&self) -> Option<&LocalId> {
        self.selected.as_ref()
    }

    pub fn selected_index(&self) -> Option<usize> {
        let sel = self.selected.as_ref()?;
        self.results.iter().position(|i| &i.local_id == sel)
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.selected_index().map(|ix| &self.results[ix])
    }

    /// A new query was typed: the selection jumps to the first match (NV).
    pub fn set_query(&mut self, query: &str, results: Vec<Item>) {
        self.query = query.to_string();
        self.selected = results.first().map(|i| i.local_id.clone());
        self.results = results;
    }

    /// Same query, fresh results (sync pulled something). Keeps the selection
    /// if it's still there, else falls back to the first row.
    pub fn refresh(&mut self, results: Vec<Item>) {
        let keep = self
            .selected
            .as_ref()
            .is_some_and(|s| results.iter().any(|i| &i.local_id == s));
        if !keep {
            self.selected = results.first().map(|i| i.local_id.clone());
        }
        self.results = results;
    }

    /// Replace one row in place (after a local edit). Doesn't re-filter: an
    /// item you're editing doesn't vanish because it stopped matching.
    pub fn update_item(&mut self, item: Item) {
        if let Some(row) = self
            .results
            .iter_mut()
            .find(|i| i.local_id == item.local_id)
        {
            *row = item;
        }
    }

    /// Drop a deleted row. If it was selected, the row after it (or, at the
    /// end, the one before) becomes the selection, so the list moves on.
    pub fn remove(&mut self, id: &LocalId) {
        let Some(ix) = self.results.iter().position(|i| &i.local_id == id) else {
            return;
        };
        self.results.remove(ix);
        if self.selected.as_ref() == Some(id) {
            self.selected = self
                .results
                .get(ix)
                .or_else(|| self.results.last())
                .map(|i| i.local_id.clone());
        }
    }

    pub fn select(&mut self, id: &LocalId) {
        if self.results.iter().any(|i| &i.local_id == id) {
            self.selected = Some(id.clone());
        }
    }

    /// ↑/↓. Clamped, no wrap. Returns the newly selected id if it changed.
    pub fn move_selection(&mut self, delta: isize) -> Option<LocalId> {
        if self.results.is_empty() {
            return None;
        }
        let last = self.results.len() as isize - 1;
        let cur = self.selected_index().map(|i| i as isize).unwrap_or(-1);
        let next = if cur < 0 {
            0
        } else {
            (cur + delta).clamp(0, last)
        };
        let id = self.results[next as usize].local_id.clone();
        if Some(&id) == self.selected.as_ref() {
            return None;
        }
        self.selected = Some(id.clone());
        Some(id)
    }

    pub fn enter(&self) -> EnterAction {
        let q = self.trimmed_query();
        if !q.is_empty() && self.results.is_empty() {
            return EnterAction::Create(q.to_string());
        }
        match &self.selected {
            Some(id) => EnterAction::Open(id.clone()),
            None => EnterAction::Nothing,
        }
    }

    /// The omnibar's right-hand hint.
    pub fn hint(&self) -> String {
        let q = self.trimmed_query();
        match (q.is_empty(), self.results.len()) {
            (true, _) => String::new(),
            (false, 0) => "⏎ new draft".into(),
            (false, n) => format!("{n} found · ⏎ open"),
        }
    }

    pub fn wants_create(&self) -> bool {
        !self.trimmed_query().is_empty() && self.results.is_empty()
    }
}

// ---------------------------------------------------------------- status bar

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Normal,
    Warn,
    Over,
}

pub fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Fragment => "◦ fragment",
        Kind::Thread => "≡ thread",
    }
}

/// `214 / 1000` (amber >900, red >1000) for fragments; threads have no limit.
pub fn counter(kind: Kind, chars: usize) -> (String, Level) {
    match kind {
        Kind::Fragment => {
            let level = if chars > FRAGMENT_LIMIT {
                Level::Over
            } else if chars > 900 {
                Level::Warn
            } else {
                Level::Normal
            };
            (format!("{chars} / {FRAGMENT_LIMIT}"), level)
        }
        Kind::Thread => (
            format!("{} chars · no limit", group_thousands(chars)),
            Level::Normal,
        ),
    }
}

pub fn banner(kind: Kind, chars: usize) -> Option<&'static str> {
    (kind == Kind::Fragment && chars > FRAGMENT_LIMIT)
        .then_some("Too long for a fragment · ⌘T makes it a thread")
}

pub fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The list's dot: a *published* post whose working copy has changed since.
/// A draft is unpublished by definition (the Worker reports every draft,
/// and a withdrawn post, as `dirty`), so it gets no dot.
pub fn has_unpublished_edits(item: &Item) -> bool {
    item.status == Status::Public && item.dirty
}

pub fn version_label(item: &Item) -> String {
    match item.status {
        Status::Public => {
            if item.dirty {
                format!("public v{} · unpublished edits", item.version)
            } else {
                format!("public v{}", item.version)
            }
        }
        Status::Withdrawn => format!("withdrawn · v{}", item.version),
        Status::Draft => "draft".into(),
        Status::Scratch => "scratch · only on this Mac".into(),
    }
}

/// The list row's pill: `draft`, `scratch` or `vN`.
pub fn pill(item: &Item) -> (String, bool) {
    match item.status {
        Status::Draft => ("draft".into(), false),
        Status::Scratch => ("scratch".into(), false),
        Status::Public => (format!("v{}", item.version), true),
        Status::Withdrawn => ("withdrawn".into(), false),
    }
}

/// Dot colour for the sync indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDot {
    Green,
    Amber,
    /// Accent, pulsing.
    Busy,
    Grey,
    Red,
}

pub fn sync_label(status: SyncStatus, publishing: bool) -> (String, SyncDot) {
    if publishing {
        return ("publishing…".into(), SyncDot::Busy);
    }
    match status {
        SyncStatus::Synced => ("synced".into(), SyncDot::Green),
        SyncStatus::Saving => ("saved on this Mac".into(), SyncDot::Amber),
        SyncStatus::Syncing => ("syncing…".into(), SyncDot::Busy),
        SyncStatus::Offline { pending } => (
            format!(
                "offline · {pending} change{} waiting",
                if pending == 1 { "" } else { "s" }
            ),
            SyncDot::Grey,
        ),
        SyncStatus::Error => ("sync error".into(), SyncDot::Red),
    }
}

// ---------------------------------------------------------------- publish

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishDecision {
    /// Over-limit fragment: shake the status bar, say ⌘T.
    Shake,
    AlreadyPublished(u32),
    Empty,
    Sheet {
        title: String,
        next_version: u32,
    },
}

pub fn publish_decision(item: &Item) -> PublishDecision {
    // A scratch note's kind is chosen when it's promoted (a long one becomes
    // a thread), so its length never blocks the sheet.
    if item.over_limit() && item.status != Status::Scratch {
        return PublishDecision::Shake;
    }
    if item.content_md.trim().is_empty() {
        return PublishDecision::Empty;
    }
    if item.status == Status::Public && !item.dirty {
        return PublishDecision::AlreadyPublished(item.version);
    }
    PublishDecision::Sheet {
        title: truncate_chars(&item.title(), 48),
        next_version: item.version + 1,
    }
}

/// Why ⌘D (make draft) can't promote `item`, if it can't: only a scratch
/// note with some text becomes a draft.
pub fn make_draft_blocked(item: &Item) -> Option<&'static str> {
    match item.status {
        Status::Scratch if item.content_md.trim().is_empty() => {
            Some("Nothing to save as a draft yet")
        }
        Status::Scratch => None,
        Status::Draft => Some("Already a draft"),
        _ => Some("Already on your blyg"),
    }
}

// ---------------------------------------------------------------- delete / withdraw

/// What getting rid of `item` means (docs/SPEC.md rule 5): drafts and
/// scratch notes are deleted; published work is only ever withdrawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discard {
    /// A scratch note, or a draft that was never published.
    Delete,
    /// A published post: withdrawn (permanent, visible), never deleted.
    Withdraw,
    AlreadyWithdrawn,
}

pub fn discard(item: &Item) -> Discard {
    match item.status {
        Status::Scratch => Discard::Delete,
        // The server refuses to delete anything with a version (live.rs).
        Status::Draft if item.version == 0 => Discard::Delete,
        Status::Withdrawn => Discard::AlreadyWithdrawn,
        Status::Draft | Status::Public => Discard::Withdraw,
    }
}

/// The noun for a deletable item, for the sheet and the toast.
pub fn discard_noun(item: &Item) -> &'static str {
    if item.status == Status::Scratch {
        "scratch note"
    } else {
        "draft"
    }
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

/// An image/link URL from the markdown made absolute: `http(s)://…` as is,
/// `media/x.png` (or `/media/x.png`) joined onto the blyg's origin. `None`
/// for a relative URL when no origin is known.
pub fn absolute_url(url: &str, base: Option<&str>) -> Option<String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(url.to_string());
    }
    let base = base?.trim_end_matches('/');
    Some(format!("{base}/{}", url.trim_start_matches('/')))
}

/// The window title: `Blygger — <host>` for the connected blyg, else `Blygger`.
pub fn window_title(blyg_url: Option<&str>) -> String {
    match blyg_url.and_then(url_host).filter(|h| !h.is_empty()) {
        Some(host) => format!("Blygger — {host}"),
        None => "Blygger".to_string(),
    }
}

/// The host of an http(s) URL, without port or path.
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    Some(host.to_ascii_lowercase())
}

/// `https://blyg.example.com/f/01J…` → `blyg.example.com/f/01J9…`
pub fn short_permalink(url: &str) -> String {
    let bare = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    match bare.rsplit_once('/') {
        Some((head, id)) if id.chars().count() > 6 => {
            format!("{head}/{}…", id.chars().take(4).collect::<String>())
        }
        _ => bare.to_string(),
    }
}

// ---------------------------------------------------------------- highlights

/// Byte ranges of every case-insensitive occurrence of `query` in `text`.
/// Works char-by-char so non-ASCII case folding can't misalign byte offsets.
pub fn highlight_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let q: Vec<char> = query.trim().chars().flat_map(char::to_lowercase).collect();
    if q.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    'outer: while i < chars.len() {
        // Match q against lowercase expansion of chars starting at i.
        let mut qi = 0;
        let mut j = i;
        while qi < q.len() {
            let Some(&(_, c)) = chars.get(j) else {
                break 'outer;
            };
            for lc in c.to_lowercase() {
                if qi >= q.len() || lc != q[qi] {
                    i += 1;
                    continue 'outer;
                }
                qi += 1;
            }
            j += 1;
        }
        let start = chars[i].0;
        let end = chars.get(j).map(|c| c.0).unwrap_or(text.len());
        out.push(start..end);
        i = j.max(i + 1);
    }
    out
}

// ---------------------------------------------------------------- time

/// `now`, `2m`, `3h`, `Sep 22` (or `Sep 22 2024` in another year).
pub fn relative_time(rfc3339: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::{DateTime, Datelike, Local};
    let Ok(t) = DateTime::parse_from_rfc3339(rfc3339) else {
        return String::new();
    };
    let t = t.with_timezone(&chrono::Utc);
    let secs = (now - t).num_seconds();
    if secs < 60 {
        return "now".into();
    }
    if secs < 3600 {
        return format!("{}m", secs / 60);
    }
    if secs < 86_400 {
        return format!("{}h", secs / 3600);
    }
    let local = t.with_timezone(&Local);
    if local.year() == now.with_timezone(&Local).year() {
        local.format("%b %-d").to_string()
    } else {
        local.format("%b %-d %Y").to_string()
    }
}

// ---------------------------------------------------------------- image placeholders

pub const PLACEHOLDER: &str = "![uploading…]()";

/// Pasting a link over selected text (as WordPress does): when `sel` is a
/// non-empty selection on one line and the clipboard holds just a web or mail
/// address, the selection becomes `[text](url)`. Returns the new text and the
/// caret (after the link); `None` means an ordinary paste.
pub fn link_paste(text: &str, sel: Range<usize>, clip: &str) -> Option<(String, usize)> {
    let url = clip.trim();
    let is_link = ["https://", "http://", "mailto:"].iter().any(|p| {
        url.len() > p.len()
            && url
                .get(..p.len())
                .is_some_and(|h| h.eq_ignore_ascii_case(p))
    });
    if !is_link || url.chars().any(char::is_whitespace) || url.contains(['<', '>']) {
        return None;
    }
    let label = text.get(sel.clone())?;
    if label.trim().is_empty() || label.contains('\n') {
        return None;
    }
    // Already a link (or an address itself): leave it to a normal paste.
    let before = &text[..sel.start];
    if label.contains("](")
        || label.contains("://")
        || before.ends_with("](")
        || before.ends_with('<')
    {
        return None;
    }
    // Keep the selection's own leading/trailing spaces outside the link.
    let lead = label.len() - label.trim_start().len();
    let core = label.trim();
    let tail = &label[lead + core.len()..];
    let core = core.replace('[', "\\[").replace(']', "\\]");
    let dest = if url.contains(['(', ')']) {
        format!("<{url}>")
    } else {
        url.to_string()
    };
    let link = format!("{}[{core}]({dest})", &label[..lead]);
    let caret = sel.start + link.len();
    let mut out = String::with_capacity(text.len() + link.len());
    out.push_str(before);
    out.push_str(&link);
    out.push_str(tail);
    out.push_str(&text[sel.end..]);
    Some((out, caret + tail.len()))
}

/// Insert the upload placeholder at byte `cursor`, on its own paragraph (as
/// the mock does). Returns the new text and the byte range of the placeholder.
pub fn insert_placeholder(text: &str, cursor: usize) -> (String, Range<usize>) {
    let cursor = floor_char_boundary(text, cursor.min(text.len()));
    let before = text[..cursor].trim_end();
    let after = text[cursor..].trim_start_matches([' ', '\t']);
    let lead = if before.is_empty() { "" } else { "\n\n" };
    let trail = if after.starts_with("\n\n") {
        ""
    } else if after.starts_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let mut out = String::with_capacity(text.len() + PLACEHOLDER.len() + 4);
    out.push_str(before);
    out.push_str(lead);
    let start = out.len();
    out.push_str(PLACEHOLDER);
    let end = out.len();
    out.push_str(trail);
    out.push_str(after);
    (out, start..end)
}

/// The byte range of the first placeholder, for in-place replacement.
pub fn find_placeholder(text: &str) -> Option<Range<usize>> {
    text.find(PLACEHOLDER).map(|s| s..s + PLACEHOLDER.len())
}

pub fn image_markdown(url: &str) -> String {
    format!("![]({url})")
}

pub fn replace_placeholder(text: &str, url: &str) -> Option<String> {
    let r = find_placeholder(text)?;
    Some(format!(
        "{}{}{}",
        &text[..r.start],
        image_markdown(url),
        &text[r.end..]
    ))
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub fn remove_placeholder(text: &str) -> Option<String> {
    let r = find_placeholder(text)?;
    // Take the paragraph break we added with it.
    let end = if text[r.end..].starts_with("\n\n") {
        r.end + 2
    } else {
        r.end
    };
    Some(format!("{}{}", &text[..r.start], &text[end..]))
}

/// The minimal edit turning `old` into `new`: the byte range of `old` to
/// replace and the text to put there (common prefix/suffix trimmed, on char
/// boundaries).
pub fn splice<'a>(old: &str, new: &'a str) -> (Range<usize>, &'a str) {
    let mut pre = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(pre) || !new.is_char_boundary(pre) {
        pre -= 1;
    }
    let max_suf = (old.len() - pre).min(new.len() - pre);
    let mut suf = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suf)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suf) || !new.is_char_boundary(new.len() - suf) {
        suf -= 1;
    }
    (pre..old.len() - suf, &new[pre..new.len() - suf])
}

/// Where a caret at `cursor` ends up after replacing `range` with `inserted` bytes.
pub fn shift_cursor(cursor: usize, range: &Range<usize>, inserted: usize) -> usize {
    if cursor <= range.start {
        cursor
    } else if cursor >= range.end {
        cursor + inserted - (range.end - range.start)
    } else {
        range.start + inserted
    }
}

pub fn mime_for_path(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "avif" => "image/avif",
        _ => return None,
    })
}

// ---------------------------------------------------------------- conflict diff

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffOp {
    Same,
    Insert,
    Delete,
}

/// Word-level diff (LCS over whitespace-preserving tokens). Returns, for the
/// "mine" side and the "theirs" side, token runs tagged Same/Insert/Delete
/// relative to their common ancestor-ish LCS.
pub type DiffRuns = Vec<(DiffOp, String)>;

pub fn word_diff(a: &str, b: &str) -> (DiffRuns, DiffRuns) {
    let ta = tokens(a);
    let tb = tokens(b);
    // Bound the work: huge texts get a coarse diff.
    if ta.len() * tb.len() > 4_000_000 {
        return (
            vec![(DiffOp::Same, a.to_string())],
            vec![(DiffOp::Same, b.to_string())],
        );
    }
    let (n, m) = (ta.len(), tb.len());
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[at(i, j)] = if ta[i] == tb[j] {
                dp[at(i + 1, j + 1)] + 1
            } else {
                dp[at(i + 1, j)].max(dp[at(i, j + 1)])
            };
        }
    }
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let push = |v: &mut Vec<(DiffOp, String)>, op: DiffOp, t: &str| match v.last_mut() {
        Some((last, s)) if *last == op => s.push_str(t),
        _ => v.push((op, t.to_string())),
    };
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && ta[i] == tb[j] {
            push(&mut left, DiffOp::Same, ta[i]);
            push(&mut right, DiffOp::Same, tb[j]);
            i += 1;
            j += 1;
        } else if j < m && (i == n || dp[at(i, j + 1)] >= dp[at(i + 1, j)]) {
            push(&mut right, DiffOp::Insert, tb[j]);
            j += 1;
        } else {
            push(&mut left, DiffOp::Insert, ta[i]);
            push(&mut right, DiffOp::Delete, ta[i]);
            i += 1;
        }
    }
    (coalesce(left), coalesce(right))
}

/// Absorb whitespace-only `Same` runs sitting between two runs of the same
/// change, so "part 1" highlights as one span rather than two words.
fn coalesce(runs: DiffRuns) -> DiffRuns {
    // A changed region is a maximal stretch of Insert/Delete runs, allowing
    // whitespace-only Same runs *inside* it (they exist on both sides, so they
    // go to both halves). Each region renders as: what went, then what came.
    let is_gap = |ix: usize| {
        runs[ix].0 == DiffOp::Same
            && runs[ix].1.trim().is_empty()
            && ix > 0
            && runs[ix - 1].0 != DiffOp::Same
            && runs.get(ix + 1).is_some_and(|r| r.0 != DiffOp::Same)
    };
    let mut out: DiffRuns = Vec::with_capacity(runs.len());
    let mut i = 0;
    while i < runs.len() {
        if runs[i].0 == DiffOp::Same {
            push_run(&mut out, DiffOp::Same, &runs[i].1);
            i += 1;
            continue;
        }
        let (mut del, mut ins) = (String::new(), String::new());
        while i < runs.len() && (runs[i].0 != DiffOp::Same || is_gap(i)) {
            match runs[i].0 {
                DiffOp::Delete => del.push_str(&runs[i].1),
                DiffOp::Insert => ins.push_str(&runs[i].1),
                DiffOp::Same => {
                    if !del.is_empty() {
                        del.push_str(&runs[i].1);
                    }
                    ins.push_str(&runs[i].1);
                }
            }
            i += 1;
        }
        if !del.is_empty() {
            push_run(&mut out, DiffOp::Delete, &del);
        }
        if !ins.is_empty() {
            push_run(&mut out, DiffOp::Insert, &ins);
        }
    }
    out
}

fn push_run(v: &mut DiffRuns, op: DiffOp, text: &str) {
    match v.last_mut() {
        Some((prev, s)) if *prev == op => s.push_str(text),
        _ => v.push((op, text.to_string())),
    }
}

/// Word runs, whitespace runs, and each punctuation mark on its own, so
/// "Teams," and "Teams" still share the word.
fn tokens(s: &str) -> Vec<&str> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Punct,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '\'' || c == '’' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Punct
        }
    };
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<Class> = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        if let Some(p) = prev
            && (p != k || k == Class::Punct)
        {
            out.push(&s[start..i]);
            start = i;
        }
        prev = Some(k);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

// ---------------------------------------------------------------- quick capture

pub fn capture_counter(chars: usize) -> (String, Level) {
    let (c, level) = counter(Kind::Fragment, chars);
    (format!("fragment · {c}"), level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blyg_core::LocalId;

    #[test]
    fn a_link_pasted_over_a_selection_wraps_it() {
        let t = "Read the tide tables first.";
        let sel = 9..20; // "tide tables"
        let (out, caret) = link_paste(t, sel.clone(), " https://example.org/tides\n").unwrap();
        assert_eq!(
            out,
            "Read the [tide tables](https://example.org/tides) first."
        );
        assert_eq!(
            &out[..caret],
            "Read the [tide tables](https://example.org/tides)"
        );
        // Spaces caught in the selection stay outside the link.
        let (out, _) = link_paste(t, 8..21, "https://example.org").unwrap();
        assert_eq!(out, "Read the [tide tables](https://example.org) first.");
        // Brackets are escaped; parentheses in the address use <…>.
        let (out, _) = link_paste("a [b] c", 0..7, "https://example.org/a_(b)").unwrap();
        assert_eq!(out, "[a \\[b\\] c](<https://example.org/a_(b)>)");
        assert!(link_paste("mail me", 0..7, "mailto:me@example.org").is_some());
    }

    #[test]
    fn an_ordinary_paste_otherwise() {
        let t = "Read the tide tables first.";
        assert_eq!(
            link_paste(t, 9..9, "https://example.org"),
            None,
            "no selection"
        );
        assert_eq!(link_paste(t, 9..20, "just words"), None, "not a link");
        assert_eq!(link_paste(t, 9..20, "https://a.org and more"), None);
        assert_eq!(link_paste(t, 9..20, "ftp://example.org"), None);
        assert_eq!(
            link_paste("one\ntwo", 0..7, "https://example.org"),
            None,
            "two lines"
        );
        assert_eq!(
            link_paste(t, 8..9, "https://example.org"),
            None,
            "only a space"
        );
        // Already a link, or an address selected: replace it as usual.
        let l = "[tide](https://old.example.org)";
        assert_eq!(link_paste(l, 7..30, "https://example.org"), None);
        assert_eq!(link_paste(l, 0..31, "https://example.org"), None);
    }

    fn item(id: &str, body: &str) -> Item {
        Item {
            local_id: LocalId(id.into()),
            server_id: None,
            kind: Kind::Fragment,
            status: Status::Draft,
            version: 0,
            dirty: false,
            content_md: body.into(),
            created: String::new(),
            updated: String::new(),
            permalink: None,
            stub_of: None,
            forked_from: None,
            show_responses: false,
            pending_sync: false,
            conflict: false,
        }
    }

    #[test]
    fn the_dot_is_for_published_posts_only() {
        // What a live Worker sends: every draft and withdrawn post is dirty.
        let mut it = item("a", "x");
        it.dirty = true;
        assert!(!has_unpublished_edits(&it), "a draft gets no dot");
        it.status = Status::Withdrawn;
        it.version = 2;
        assert!(!has_unpublished_edits(&it));
        it.status = Status::Public;
        assert!(has_unpublished_edits(&it));
        it.dirty = false;
        assert!(!has_unpublished_edits(&it));
    }

    fn ids(v: &[&str]) -> Vec<Item> {
        v.iter().map(|s| item(s, s)).collect()
    }

    #[test]
    fn query_selects_first_match_and_moves_clamped() {
        let mut m = ListModel::default();
        m.set_query("", ids(&["a", "b", "c"]));
        assert_eq!(m.selected(), Some(&LocalId("a".into())));
        assert_eq!(m.move_selection(1), Some(LocalId("b".into())));
        assert_eq!(m.move_selection(5), Some(LocalId("c".into())));
        assert_eq!(m.move_selection(1), None, "clamped at the end");
        assert_eq!(m.move_selection(-10), Some(LocalId("a".into())));
        assert_eq!(m.move_selection(-1), None);
    }

    #[test]
    fn enter_opens_or_creates() {
        let mut m = ListModel::default();
        m.set_query("be", ids(&["bees"]));
        assert_eq!(m.enter(), EnterAction::Open(LocalId("bees".into())));
        assert_eq!(m.hint(), "1 found · ⏎ open");
        m.set_query("  tide pools ", vec![]);
        assert_eq!(m.enter(), EnterAction::Create("tide pools".into()));
        assert_eq!(m.hint(), "⏎ new draft");
        assert!(m.wants_create());
        m.set_query("", vec![]);
        assert_eq!(m.enter(), EnterAction::Nothing);
        assert_eq!(m.hint(), "");
    }

    #[test]
    fn refresh_keeps_selection_when_present() {
        let mut m = ListModel::default();
        m.set_query("", ids(&["a", "b"]));
        m.select(&LocalId("b".into()));
        m.refresh(ids(&["new", "a", "b"]));
        assert_eq!(m.selected(), Some(&LocalId("b".into())));
        m.refresh(ids(&["x"]));
        assert_eq!(m.selected(), Some(&LocalId("x".into())));
    }

    #[test]
    fn remove_moves_the_selection_on() {
        let mut m = ListModel::default();
        m.set_query("", ids(&["a", "b", "c"]));
        m.select(&LocalId("b".into()));
        m.remove(&LocalId("b".into()));
        assert_eq!(m.selected(), Some(&LocalId("c".into())), "the next row");
        m.remove(&LocalId("c".into()));
        assert_eq!(
            m.selected(),
            Some(&LocalId("a".into())),
            "at the end: the one before"
        );
        m.remove(&LocalId("zzz".into()));
        assert_eq!(m.results().len(), 1);
        m.remove(&LocalId("a".into()));
        assert_eq!(m.selected(), None);
    }

    #[test]
    fn drafts_are_deleted_published_posts_withdrawn() {
        let mut it = item("a", "x");
        assert_eq!(discard(&it), Discard::Delete);
        assert_eq!(discard_noun(&it), "draft");
        it.status = Status::Scratch;
        assert_eq!(discard(&it), Discard::Delete);
        assert_eq!(discard_noun(&it), "scratch note");
        it.status = Status::Public;
        it.version = 2;
        assert_eq!(discard(&it), Discard::Withdraw);
        it.status = Status::Draft; // a draft with history is still published work
        assert_eq!(discard(&it), Discard::Withdraw);
        it.status = Status::Withdrawn;
        assert_eq!(discard(&it), Discard::AlreadyWithdrawn);
    }

    #[test]
    fn update_item_does_not_refilter() {
        let mut m = ListModel::default();
        m.set_query("bee", ids(&["bee"]));
        m.update_item(item("bee", "no longer matching"));
        assert_eq!(m.results().len(), 1);
        assert_eq!(m.results()[0].content_md, "no longer matching");
    }

    #[test]
    fn counter_levels_and_banner() {
        assert_eq!(
            counter(Kind::Fragment, 214),
            ("214 / 1000".into(), Level::Normal)
        );
        assert_eq!(counter(Kind::Fragment, 900).1, Level::Normal);
        assert_eq!(counter(Kind::Fragment, 901).1, Level::Warn);
        assert_eq!(counter(Kind::Fragment, 1000).1, Level::Warn);
        assert_eq!(counter(Kind::Fragment, 1001).1, Level::Over);
        assert_eq!(counter(Kind::Thread, 12345).0, "12,345 chars · no limit");
        assert_eq!(counter(Kind::Thread, 5000).1, Level::Normal);
        assert!(banner(Kind::Fragment, 1001).is_some());
        assert!(banner(Kind::Fragment, 1000).is_none());
        assert!(banner(Kind::Thread, 5000).is_none());
    }

    #[test]
    fn thousands() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }

    #[test]
    fn version_and_sync_labels() {
        let mut it = item("a", "x");
        assert_eq!(version_label(&it), "draft");
        it.status = Status::Public;
        it.version = 3;
        assert_eq!(version_label(&it), "public v3");
        it.dirty = true;
        assert_eq!(version_label(&it), "public v3 · unpublished edits");
        assert_eq!(pill(&it), ("v3".into(), true));

        assert_eq!(sync_label(SyncStatus::Synced, false).0, "synced");
        assert_eq!(sync_label(SyncStatus::Saving, false).1, SyncDot::Amber);
        assert_eq!(
            sync_label(SyncStatus::Offline { pending: 1 }, false).0,
            "offline · 1 change waiting"
        );
        assert_eq!(
            sync_label(SyncStatus::Offline { pending: 2 }, false).0,
            "offline · 2 changes waiting"
        );
        assert_eq!(sync_label(SyncStatus::Synced, true).1, SyncDot::Busy);
        assert_eq!(
            sync_label(SyncStatus::Syncing, false),
            ("syncing…".into(), SyncDot::Busy)
        );
    }

    #[test]
    fn urls_titles_and_hosts() {
        let base = Some("https://blyg.example.com/");
        assert_eq!(
            absolute_url("media/a.png", base).as_deref(),
            Some("https://blyg.example.com/media/a.png")
        );
        assert_eq!(
            absolute_url("/media/a.png", base).as_deref(),
            Some("https://blyg.example.com/media/a.png")
        );
        assert_eq!(
            absolute_url("https://cdn.example.org/x.jpg", None).as_deref(),
            Some("https://cdn.example.org/x.jpg")
        );
        assert_eq!(absolute_url("media/a.png", None), None);

        assert_eq!(
            window_title(Some("https://Blyg.Example.com:8443/sub/")),
            "Blygger — blyg.example.com"
        );
        assert_eq!(
            window_title(Some("http://127.0.0.1:8787")),
            "Blygger — 127.0.0.1"
        );
        assert_eq!(window_title(None), "Blygger");
        assert_eq!(window_title(Some("not a url")), "Blygger");
    }

    #[test]
    fn publish_rules() {
        let mut it = item("a", "A lighthouse keeper's log is mostly weather");
        assert_eq!(
            publish_decision(&it),
            PublishDecision::Sheet {
                title: "A lighthouse keeper's log is mostly weather".into(),
                next_version: 1
            }
        );
        it.content_md = "x".repeat(1001);
        assert_eq!(publish_decision(&it), PublishDecision::Shake);
        it.kind = Kind::Thread;
        assert!(matches!(
            publish_decision(&it),
            PublishDecision::Sheet { .. }
        ));
        it.status = Status::Public;
        it.version = 2;
        assert_eq!(publish_decision(&it), PublishDecision::AlreadyPublished(2));
        it.dirty = true;
        assert!(matches!(
            publish_decision(&it),
            PublishDecision::Sheet {
                next_version: 3,
                ..
            }
        ));
        it.content_md = "   ".into();
        assert_eq!(publish_decision(&it), PublishDecision::Empty);
    }

    #[test]
    fn permalink_shortening() {
        assert_eq!(
            short_permalink("https://blyg.example.com/f/01J9QK3ABCDEF"),
            "blyg.example.com/f/01J9…"
        );
    }

    #[test]
    fn highlights_case_insensitive_and_unicode_safe() {
        assert_eq!(
            highlight_ranges("Band on Harbour Road", "harb"),
            vec![8..12]
        );
        assert_eq!(highlight_ranges("aAa", "a"), vec![0..1, 1..2, 2..3]);
        assert_eq!(highlight_ranges("Café café", "CAFÉ"), vec![0..5, 6..11]);
        assert!(highlight_ranges("abc", "  ").is_empty());
        assert!(highlight_ranges("abc", "abcd").is_empty());
        // Every range must be on char boundaries.
        let t = "Ünïcödé ünï";
        for r in highlight_ranges(t, "ünï") {
            assert!(t.is_char_boundary(r.start) && t.is_char_boundary(r.end));
        }
    }

    #[test]
    fn relative_times() {
        use chrono::{Duration, Utc};
        let now = Utc::now();
        let ago = |d: Duration| (now - d).to_rfc3339();
        assert_eq!(relative_time(&ago(Duration::seconds(10)), now), "now");
        assert_eq!(relative_time(&ago(Duration::minutes(2)), now), "2m");
        assert_eq!(relative_time(&ago(Duration::hours(3)), now), "3h");
        assert!(!relative_time(&ago(Duration::days(40)), now).is_empty());
        assert_eq!(relative_time("garbage", now), "");
    }

    #[test]
    fn placeholder_insert_and_replace() {
        let (t, r) = insert_placeholder("Band on Harbour Road.", 21);
        assert_eq!(t, "Band on Harbour Road.\n\n![uploading…]()\n\n");
        assert_eq!(&t[r.clone()], PLACEHOLDER);
        let (t2, _) = insert_placeholder("one two", 3);
        assert_eq!(t2, "one\n\n![uploading…]()\n\ntwo");
        let (t3, _) = insert_placeholder("", 0);
        assert_eq!(t3, "![uploading…]()\n\n");
        let done = replace_placeholder(&t2, "media/01J9R7W.webp").unwrap();
        assert_eq!(done, "one\n\n![](media/01J9R7W.webp)\n\ntwo");
        assert!(replace_placeholder("nothing here", "x").is_none());
        // cursor mid-codepoint is clamped
        let (t4, _) = insert_placeholder("é", 1);
        assert!(t4.starts_with("![uploading"));
    }

    #[test]
    fn splice_minimal() {
        assert_eq!(splice("abc", "abXc"), (2..2, "X"));
        assert_eq!(splice("hello world", "hello"), (5..11, ""));
        assert_eq!(splice("same", "same"), (4..4, ""));
        assert_eq!(splice("aaa", "aaaa"), (3..3, "a"));
        let (r, ins) = splice("é", "è");
        assert_eq!((r, ins), (0..2, "è"));
        let old = "one\n\n![uploading…]()\n\ntwo";
        let new = replace_placeholder(old, "media/x.png").unwrap();
        let (r, ins) = splice(old, &new);
        let mut rebuilt = old.to_string();
        rebuilt.replace_range(r, ins);
        assert_eq!(rebuilt, new);
    }

    #[test]
    fn cursor_shifting() {
        assert_eq!(shift_cursor(2, &(5..7), 10), 2);
        assert_eq!(shift_cursor(9, &(5..7), 10), 17);
        assert_eq!(shift_cursor(6, &(5..7), 10), 15);
        assert_eq!(
            remove_placeholder("a\n\n![uploading…]()\n\nb").unwrap(),
            "a\n\nb"
        );
    }

    #[test]
    fn mime_types() {
        use std::path::Path;
        assert_eq!(mime_for_path(Path::new("a/b.PNG")), Some("image/png"));
        assert_eq!(mime_for_path(Path::new("x.jpeg")), Some("image/jpeg"));
        assert_eq!(mime_for_path(Path::new("x.txt")), None);
    }

    #[test]
    fn word_diff_marks_changes() {
        let (mine, theirs) = word_diff(
            "Notes on The Field Guide to Small Gardens (A. Gardener)",
            "Notes on Field Guide to Small Gardens, part 1",
        );
        let ins_mine: String = mine
            .iter()
            .filter(|(op, _)| *op == DiffOp::Insert)
            .map(|(_, s)| s.as_str())
            .collect();
        assert!(ins_mine.contains("(A."));
        assert!(ins_mine.contains("The"));
        let del_theirs: String = theirs
            .iter()
            .filter(|(op, _)| *op == DiffOp::Delete)
            .map(|(_, s)| s.as_str())
            .collect();
        assert!(del_theirs.contains("The"));
        let ins_theirs: String = theirs
            .iter()
            .filter(|(op, _)| *op == DiffOp::Insert)
            .map(|(_, s)| s.as_str())
            .collect();
        assert_eq!(ins_theirs, ", part 1", "{theirs:?}");
        // Reassembling each side's non-deleted tokens gives back the text.
        let rebuilt: String = theirs
            .iter()
            .filter(|(op, _)| *op != DiffOp::Delete)
            .map(|(_, s)| s.as_str())
            .collect();
        assert_eq!(rebuilt, "Notes on Field Guide to Small Gardens, part 1");
        let rebuilt_mine: String = mine.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(
            rebuilt_mine,
            "Notes on The Field Guide to Small Gardens (A. Gardener)"
        );
    }

    #[test]
    fn capture_counter_text() {
        assert_eq!(capture_counter(0).0, "fragment · 0 / 1000");
        assert_eq!(capture_counter(1200).1, Level::Over);
    }
}
