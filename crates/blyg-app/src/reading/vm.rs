//! Pure view models for the reading and management screens. The GPUI code in
//! the sibling modules renders only what these return, so the rules (only
//! current + pinned versions, no counts, threads-only quoting, …) are tested
//! here and in `tests.rs` without pixels.

use blyg_core::{
    Item, Kind, Mention, ReadingItem, RemoteVersion, Status, Subscription, SubscriptionKind,
    Version,
};

/// (subscription id, remote id): one reading row.
pub type Key = (String, String);

pub fn key(r: &ReadingItem) -> Key {
    (r.subscription_id.clone(), r.remote_id.clone())
}

// ---------------------------------------------------------------- status bar

/// What the status bar shows besides the sync state, per screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenStatus {
    /// The current post's kind, counter, studio counts and version.
    pub post: bool,
    /// "12 to read": the reader-local unread count (private state, never
    /// social, so a number is fine).
    pub to_read: Option<String>,
}

/// Posts: the post's own status (and the unread count, as before).
/// Reading: the unread count. Mentions: nothing but the sync state (a list,
/// never a count). Subscriptions: the sync state.
pub fn screen_status(view: super::View, unread: usize) -> ScreenStatus {
    let to_read = (unread > 0).then(|| format!("{unread} to read"));
    match view {
        super::View::Posts => ScreenStatus {
            post: true,
            to_read,
        },
        super::View::Reading => ScreenStatus {
            post: false,
            to_read,
        },
        super::View::Mentions | super::View::Subscriptions => ScreenStatus {
            post: false,
            to_read: None,
        },
    }
}

// ---------------------------------------------------------------- reading list

/// A withdrawn post stays only when the user signalled or hoppered it.
pub fn visible(r: &ReadingItem) -> bool {
    r.state != "tombstone" || r.thumb.is_some() || !r.hoppers.is_empty()
}

/// The list as shown: tombstones hidden unless kept, one row per post (the
/// first of any duplicate key wins), and posts edited since you read them
/// moved to the top. Otherwise the backend's order (newest first) is kept.
pub fn order(items: Vec<ReadingItem>) -> Vec<ReadingItem> {
    let mut seen = std::collections::HashSet::new();
    let (mut edited, mut rest): (Vec<_>, Vec<_>) = items
        .into_iter()
        .filter(visible)
        .filter(|r| seen.insert(key(r)))
        .partition(|r| r.edited_since_read() && r.state != "tombstone");
    edited.append(&mut rest);
    edited
}

// ---------------------------------------------------------------- search

/// Does `r` match the reading search `query`? Case-insensitive substring
/// over the title, the author and origin names, and the post's text, all of
/// it held locally (nothing is fetched to search). An empty query matches.
pub fn matches_query(r: &ReadingItem, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let has = |s: &str| s.to_lowercase().contains(&q);
    let author = r.author.as_ref();
    has(&r.subscription_title)
        || has(&host(&r.origin))
        || author.and_then(|a| a.name.as_deref()).is_some_and(has)
        || has(&r.content_md)
        // Held without Markdown (some feeds): search its published text.
        || (r.content_md.trim().is_empty() && has(&html_text(&r.content_html)))
}

/// Indices into `rows` of the rows the reading search shows, in order.
pub fn filter(rows: &[ReadingItem], query: &str) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| matches_query(r, query))
        .map(|(i, _)| i)
        .collect()
}

/// The list's empty state when the search matches nothing.
pub fn no_match(query: &str) -> String {
    format!("No posts match “{}”", query.trim())
}

/// Visible text of an HTML fragment (tags dropped), for searching only.
fn html_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The first line with readable text, as plain text; `…` when there's none.
pub fn title(md: &str) -> String {
    blyg_core::plain_title(md).unwrap_or_else(|| "…".into())
}

/// `rue.blyg.example.com` from an origin.
pub fn host(origin: &str) -> String {
    crate::vm::url_host(origin).unwrap_or_else(|| origin.trim_end_matches('/').to_string())
}

/// The small badge after the source: `edited · v5`, `new`, `withdrawn`.
pub fn badge(r: &ReadingItem) -> Option<String> {
    if r.state == "tombstone" {
        Some("withdrawn".into())
    } else if r.edited_since_read() {
        Some(format!("edited · v{}", r.version))
    } else if r.is_unread() {
        Some("new".into())
    } else {
        None
    }
}

/// Byline for a row: `Rue · blyg`, `Omar's notes · RSS`.
pub fn source_line(r: &ReadingItem, subs: &[Subscription]) -> String {
    let kind = subs
        .iter()
        .find(|s| s.id == r.subscription_id)
        .map(|s| match s.kind {
            SubscriptionKind::Blyg => "blyg",
            SubscriptionKind::Rss => "RSS",
        });
    match kind {
        Some(k) => format!("{} · {k}", r.subscription_title),
        None => r.subscription_title.clone(),
    }
}

/// "Edited since you read it": the author's notes for the versions after the
/// one you read, up to now. Metadata only (spec §5.2): never text.
pub fn notes_between(changelog: &[RemoteVersion], read: u32, now: u32) -> Vec<String> {
    changelog
        .iter()
        .filter(|v| v.version > read && v.version <= now)
        .map(|v| match &v.note {
            Some(n) if !n.trim().is_empty() => format!("v{} “{}”", v.version, n.trim()),
            _ => format!("v{} (no note)", v.version),
        })
        .collect()
}

/// The header of the "edited since you read it" block.
pub fn edited_heading(read: u32, now: u32, diff_from_pin: bool) -> String {
    if diff_from_pin {
        format!("Edited since you read 📌 v{read} · diff v{read} → v{now}")
    } else {
        format!("Edited since you read v{read} · the author's notes")
    }
}

/// `https://…/f/<id>` for "Open on web": the declared page (origin-relative),
/// else the public item document.
pub fn web_url(r: &ReadingItem) -> Option<String> {
    r.page
        .as_deref()
        .and_then(|p| {
            if p.starts_with("http://") || p.starts_with("https://") {
                Some(p.to_string())
            } else {
                blyg_core::origin_url(&r.origin, p.trim_start_matches('/'))
            }
        })
        .or_else(|| blyg_core::item_doc_url(&r.origin, &r.remote_id))
}

// ---------------------------------------------------------------- version pill

/// The `‹ vN ▾ ›` control for someone else's post. Built only from the
/// shown versions (current + pinned); nothing else can reach it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pill {
    pub label: String,
    pub pinned: bool,
    pub can_older: bool,
    pub can_newer: bool,
    /// Dropdown rows, newest first: (label, date, index into `shown`).
    pub entries: Vec<(String, String, usize)>,
    pub actions: Vec<&'static str>,
}

/// Action ids under the current version. `Fork` is always shown there but
/// disabled: it says where forking lives (a pin), rather than hiding it.
pub const CURRENT_ACTIONS: [&str; 5] = ["Quote", "Reply", "AI reply", "Fork", "Open on web"];
pub const PINNED_ACTIONS: [&str; 4] = [
    "Quote this version",
    "Fork this pin",
    "Diff vs now",
    "Back to current",
];

/// What an action row needs to know to label itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActionCtx {
    /// The post comes from a blyg (transcludable), not an RSS feed.
    pub blyg: bool,
    /// The version on screen (a pin's number on a pinned view).
    pub version: u32,
    /// The current version's number.
    pub current: u32,
    /// Pinned versions the pill can step to, oldest first.
    pub pins: Vec<u32>,
}

/// One chip in the action row: what it says, what it makes (one plain
/// sentence, for the tooltip), and whether it can be clicked.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionChip {
    pub id: &'static str,
    pub label: String,
    pub tip: String,
    pub enabled: bool,
}

/// Label each action with the protocol primitive it creates (issue #3):
/// Reply makes a stub (`stub_of`), Quote puts `![[id]]` in a thread of
/// yours, and Fork (`forked_from`) descends from pins only.
pub fn action_chip(id: &'static str, cx: &ActionCtx) -> ActionChip {
    let on = |label: &str, tip: String| ActionChip {
        id,
        label: label.into(),
        tip,
        enabled: true,
    };
    match id {
        "Quote" if cx.blyg => on(
            "Quote into a thread",
            "Adds ![[…]] to a thread of yours (the draft open, or a new one): \
             a quote, snapshotted when you publish."
                .into(),
        ),
        "Quote" => on(
            "Quote into a thread",
            "Adds a quoted excerpt with a link to a thread of yours \
             (feed posts can't be transcluded)."
                .into(),
        ),
        "Reply" => on(
            "Reply · new stub",
            "Starts a new thread of yours that's a stub of this post (stub_of), \
             with the post quoted at the top."
                .into(),
        ),
        "AI reply" => on(
            "AI reply · new stub",
            "A reply stub with an AI-drafted answer for you to review; nothing is published."
                .into(),
        ),
        "Fork" => ActionChip {
            id,
            label: "Fork".into(),
            tip: fork_needs_pin(cx),
            enabled: false,
        },
        "Open on web" => on(
            "Open on web",
            "Opens this post's page in your browser.".into(),
        ),
        "Quote this version" => on(
            "Quote this version",
            format!(
                "Adds 📌 v{}'s text as a > quote linking to the pin, in a thread of yours \
                 (![[…]] would always show the current version).",
                cx.version
            ),
        ),
        "Fork this pin" => on(
            "Fork this pin",
            format!(
                "Makes a new thread draft of yours from 📌 v{}'s text, recorded as forked \
                 from it (forked_from).",
                cx.version
            ),
        ),
        "Diff vs now" => on(
            "Diff vs now",
            format!(
                "Shows what changed between 📌 v{} and the current v{}.",
                cx.version, cx.current
            ),
        ),
        "Back to current" => on(
            "Back to current",
            format!("Shows the current version, v{}, again.", cx.current),
        ),
        other => on(other, String::new()),
    }
}

/// Why Fork is greyed out on the current version, and where to go instead.
pub fn fork_needs_pin(cx: &ActionCtx) -> String {
    match cx.pins.last() {
        Some(pin) => format!(
            "Fork needs a pinned version: pick 📌 v{pin} in ‹ v{} ▾ › (or press ←), \
             then Fork this pin.",
            cx.current
        ),
        None => "Fork needs a pinned version, and this post has none: \
                 forks descend from pins only."
            .into(),
    }
}

/// Keep only what may be shown: the current version and pinned versions,
/// oldest first (the backend already filters; this is belt and braces).
pub fn shown(versions: &[RemoteVersion]) -> Vec<RemoteVersion> {
    let mut v: Vec<RemoteVersion> = versions.iter().filter(|v| v.openable()).cloned().collect();
    v.sort_by_key(|v| v.version);
    v
}

/// Index of the current version in `shown` (the newest when none is marked).
pub fn current_ix(shown: &[RemoteVersion]) -> Option<usize> {
    shown
        .iter()
        .position(|v| v.current)
        .or_else(|| shown.len().checked_sub(1))
}

pub fn pill(
    shown: &[RemoteVersion],
    ix: usize,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<Pill> {
    let v = shown.get(ix)?;
    let date = |v: &RemoteVersion| crate::vm::relative_time(&v.at, now);
    let pinned_view = !v.current;
    let label = if v.current {
        format!("v{} · current ▾", v.version)
    } else {
        format!("📌 v{} · {} ▾", v.version, date(v))
    };
    let entries = shown
        .iter()
        .enumerate()
        .rev()
        .map(|(i, v)| {
            let l = if v.current {
                format!("v{} · current", v.version)
            } else {
                format!("📌 v{}", v.version)
            };
            (l, date(v), i)
        })
        .collect();
    Some(Pill {
        label,
        pinned: pinned_view,
        can_older: ix > 0,
        can_newer: ix + 1 < shown.len(),
        entries,
        actions: if pinned_view {
            PINNED_ACTIONS.to_vec()
        } else {
            CURRENT_ACTIONS.to_vec()
        },
    })
}

/// Every string the pill and its dropdown can display.
#[cfg(test)]
pub fn pill_strings(p: &Pill) -> Vec<String> {
    let mut out = vec![p.label.clone()];
    for (l, d, _) in &p.entries {
        out.push(l.clone());
        out.push(d.clone());
    }
    out.extend(p.actions.iter().map(|s| s.to_string()));
    out
}

/// "Quote this version" of a pin: the pin is public forever, so quote its
/// text with a link to the pin document (`![[id]]` always means current).
pub fn quote_pinned(text: &str, host: &str, version: u32, pin_url: Option<&str>) -> String {
    let mut out: String = text
        .trim_end()
        .lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str("\n>\n> — ");
    match pin_url {
        Some(u) => out.push_str(&format!("[{host} · 📌 v{version}]({u})")),
        None => out.push_str(&format!("{host} · 📌 v{version}")),
    }
    out
}

// ---------------------------------------------------------------- own versions

/// A row of your own history (newest first in the browser).
#[derive(Debug, Clone, PartialEq)]
pub struct OwnRow {
    pub version: u32,
    pub note: String,
    pub when: String,
    /// `now`, `📌`, `withdrawn`, or empty.
    pub badge: &'static str,
}

pub fn own_rows(
    versions: &[Version],
    current: u32,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<OwnRow> {
    let mut v: Vec<OwnRow> = versions
        .iter()
        .map(|v| OwnRow {
            version: v.version,
            note: v
                .note
                .as_deref()
                .filter(|n| !n.trim().is_empty())
                .map(|n| format!("“{}”", n.trim()))
                .unwrap_or_else(|| "no note".into()),
            when: crate::vm::relative_time(&v.published_at, now),
            badge: if v.endcap {
                "withdrawn"
            } else if v.version == current {
                "now"
            } else if v.pinned {
                "📌"
            } else {
                ""
            },
        })
        .collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.version));
    v
}

/// What you can do to your own version `v`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnActions {
    pub can_pin: bool,
    pub already_pinned: bool,
    pub can_restore: bool,
}

/// One control on your own version. There's deliberately no fork: forking
/// descends from someone else's pin (`PINNED_ACTIONS`), never from your
/// own history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnAction {
    /// "Pinned ✓" (a label, not a button).
    Pinned,
    Pin,
    Restore,
}

impl OwnAction {
    pub fn label(self, v: u32) -> String {
        match self {
            OwnAction::Pinned => "Pinned ✓".into(),
            OwnAction::Pin => format!("📌 Pin v{v}…"),
            OwnAction::Restore => format!("Restore v{v}…"),
        }
    }
}

impl OwnActions {
    /// The controls shown, in order.
    pub fn list(self) -> Vec<OwnAction> {
        let mut out = Vec::new();
        if self.already_pinned {
            out.push(OwnAction::Pinned);
        }
        if self.can_pin {
            out.push(OwnAction::Pin);
        }
        if self.can_restore {
            out.push(OwnAction::Restore);
        }
        out
    }
}

pub fn own_actions(v: &Version, current: u32) -> OwnActions {
    OwnActions {
        can_pin: !v.pinned && !v.endcap,
        already_pinned: v.pinned,
        can_restore: !v.endcap && v.version != current,
    }
}

/// The typed word that confirms a pin.
pub const PIN_WORD: &str = "pin";

pub fn pin_confirmed(typed: &str) -> bool {
    typed.trim().eq_ignore_ascii_case(PIN_WORD)
}

// ---------------------------------------------------------------- mentions

/// One mention as listed: who, origin, relation, when. No counts anywhere.
#[derive(Debug, Clone, PartialEq)]
pub struct MentionRow {
    pub id: String,
    pub who: String,
    pub origin: String,
    pub relation: String,
    pub when: String,
    pub new: bool,
    pub hidden: bool,
    pub source: String,
}

pub fn relation_label(r: Option<&str>) -> &'static str {
    match r {
        Some("stub") => "replied",
        Some("transclusion") => "quoted",
        Some("fork") => "forked",
        _ => "mentioned",
    }
}

pub fn mention_row(
    m: &Mention,
    seen: &std::collections::HashSet<String>,
    now: chrono::DateTime<chrono::Utc>,
) -> MentionRow {
    let origin = m.source_origin.clone().unwrap_or_else(|| m.source.clone());
    let who = m
        .source_author
        .as_ref()
        .and_then(|a| a.name.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| host(&origin));
    MentionRow {
        id: m.id.clone(),
        who,
        origin: host(&origin),
        relation: relation_label(m.relation.as_deref()).into(),
        when: crate::vm::relative_time(&m.first_seen, now),
        new: is_new_mention(m, seen, now),
        hidden: m.hidden,
        source: m.source.clone(),
    }
}

/// New = not yet seen on the Mentions screen this session, and first seen
/// in the last week. Shown as a dot, never a number.
pub fn is_new_mention(
    m: &Mention,
    seen: &std::collections::HashSet<String>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if seen.contains(&m.id) || m.hidden {
        return false;
    }
    chrono::DateTime::parse_from_rfc3339(&m.first_seen)
        .map(|t| (now - t.with_timezone(&chrono::Utc)).num_days() < 7)
        .unwrap_or(false)
}

/// Group mentions under the post they point at: (post title, local item,
/// rows), in the order of each group's newest mention.
pub fn group_mentions<'a>(
    rows: Vec<(MentionRow, &'a Mention)>,
    items: &'a [Item],
) -> Vec<(String, Option<&'a Item>, Vec<MentionRow>)> {
    let mut out: Vec<(String, Option<&Item>, Vec<MentionRow>)> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    for (row, m) in rows {
        let at = match targets.iter().position(|t| *t == m.target_item_id) {
            Some(i) => i,
            None => {
                let item = items.iter().find(|i| {
                    i.server_id
                        .as_ref()
                        .is_some_and(|s| s.0 == m.target_item_id)
                });
                let title = item
                    .map(|i| i.title().to_string())
                    .unwrap_or_else(|| m.target_item_id.clone());
                targets.push(m.target_item_id.clone());
                out.push((title, item, Vec::new()));
                out.len() - 1
            }
        };
        out[at].2.push(row);
    }
    out
}

// ---------------------------------------------------------------- quote picker

/// Something already held that a thread can quote with `![[id]]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Quotable {
    pub id: String,
    pub title: String,
    /// `yours` or the source (`Rue · rue.blyg.example.com`).
    pub source: String,
}

/// Held items only: your own published posts and imported posts from blyg
/// subscriptions (still current). Never anything fetched by URL. `exclude`
/// is the server id of the thread being written (it can't quote itself).
pub fn quotables(
    items: &[Item],
    reading: &[ReadingItem],
    subs: &[Subscription],
    exclude: Option<&str>,
    query: &str,
) -> Vec<Quotable> {
    let q = query.trim().to_lowercase();
    let mut out = Vec::new();
    for it in items {
        let Some(sid) = &it.server_id else { continue };
        if it.status != Status::Public || Some(sid.0.as_str()) == exclude {
            continue;
        }
        out.push(Quotable {
            id: sid.0.clone(),
            title: it.title().to_string(),
            source: "yours".into(),
        });
    }
    for r in reading {
        let blyg = subs
            .iter()
            .find(|s| s.id == r.subscription_id)
            .is_some_and(|s| s.kind == SubscriptionKind::Blyg);
        if !blyg || r.state != "current" || Some(r.remote_id.as_str()) == exclude {
            continue;
        }
        if out.iter().any(|q: &Quotable| q.id == r.remote_id) {
            continue;
        }
        out.push(Quotable {
            id: r.remote_id.clone(),
            title: title(&r.content_md),
            source: format!("{} · {}", r.subscription_title, host(&r.origin)),
        });
    }
    if q.is_empty() {
        return out;
    }
    out.into_iter()
        .filter(|x| x.title.to_lowercase().contains(&q) || x.source.to_lowercase().contains(&q))
        .collect()
}

/// Can `item` take a quote? Transclusion is valid only in threads.
pub fn can_quote_into(item: Option<&Item>) -> bool {
    item.is_some_and(|i| i.kind == Kind::Thread)
}

/// Insert `![[id]]` on its own line at byte `cursor`. Returns the new text
/// and the caret after the inserted line.
pub fn insert_transclusion(text: &str, cursor: usize, id: &str) -> (String, usize) {
    let cursor = floor_boundary(text, cursor.min(text.len()));
    let (before, after) = text.split_at(cursor);
    let mut out = String::with_capacity(text.len() + id.len() + 8);
    out.push_str(before);
    if !before.is_empty() && !before.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("![[{id}]]"));
    let caret_line_end = out.len();
    if !after.starts_with('\n') {
        out.push('\n');
    }
    out.push_str(after);
    (out, caret_line_end + 1)
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// A reply stub: a thread that transcludes what it answers.
pub fn stub_body(remote_id: &str) -> String {
    format!("![[{remote_id}]]\n\n")
}

// ---------------------------------------------------------------- site settings

/// `label | url` lines ⇄ author links (one per line; blank lines skipped).
pub fn parse_links(text: &str) -> Result<Vec<blyg_core::AuthorLink>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((label, url)) = line.split_once('|') else {
            return Err(format!("line {}: write it as  Label | https://…", n + 1));
        };
        let (label, url) = (label.trim(), url.trim());
        if label.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(format!("line {}: write it as  Label | https://…", n + 1));
        }
        out.push(blyg_core::AuthorLink {
            label: label.into(),
            url: url.into(),
        });
    }
    Ok(out)
}

pub fn format_links(links: &[blyg_core::AuthorLink]) -> String {
    links
        .iter()
        .map(|l| format!("{} | {}", l.label, l.url))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rv(version: u32, pinned: bool, current: bool) -> RemoteVersion {
        RemoteVersion {
            version,
            at: "2026-09-20T10:00:00Z".into(),
            note: None,
            pinned,
            current,
            media: vec![],
            lineage: Default::default(),
        }
    }

    #[test]
    fn pill_never_contains_unpinned_versions() {
        let log = vec![
            rv(1, true, false),
            rv(2, false, false),
            rv(3, true, false),
            rv(4, false, false),
            rv(5, false, true),
        ];
        let s = shown(&log);
        assert_eq!(s.iter().map(|v| v.version).collect::<Vec<_>>(), [1, 3, 5]);
        let now = chrono::Utc::now();
        for ix in 0..s.len() {
            let p = pill(&s, ix, now).unwrap();
            for text in pill_strings(&p) {
                assert!(
                    !text.contains("v2") && !text.contains("v4"),
                    "unpinned version leaked: {text}"
                );
            }
        }
        let cur = pill(&s, 2, now).unwrap();
        assert_eq!(cur.label, "v5 · current ▾");
        assert_eq!(cur.actions, CURRENT_ACTIONS);
        assert!(cur.can_older && !cur.can_newer);
        let pinned = pill(&s, 1, now).unwrap();
        assert!(pinned.label.starts_with("📌 v3"));
        assert_eq!(pinned.actions, PINNED_ACTIONS);
    }

    #[test]
    fn transclusion_goes_on_its_own_line() {
        assert_eq!(insert_transclusion("", 0, "X").0, "![[X]]\n");
        let (t, c) = insert_transclusion("Hello world", 5, "X");
        assert_eq!(t, "Hello\n![[X]]\n world");
        assert_eq!(&t[c..], " world");
        assert_eq!(insert_transclusion("a\n", 2, "X").0, "a\n![[X]]\n");
    }

    #[test]
    fn links_round_trip() {
        let l =
            parse_links("Feed | https://blyg.example.com/feed.json\n\nHome | https://example.com")
                .unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!(parse_links(&format_links(&l)).unwrap(), l);
        assert!(parse_links("nope").is_err());
    }

    #[test]
    fn pin_word() {
        assert!(pin_confirmed(" pin "));
        assert!(pin_confirmed("PIN"));
        assert!(!pin_confirmed("pi"));
        assert!(!pin_confirmed(""));
    }

    /// --- follow-ups --- Fork descends from pins only, and other people's
    /// unpinned versions never reach any control (pill, actions, AI reply).
    #[test]
    fn fork_and_ai_reply_only_where_allowed_on_others_posts() {
        let log = vec![
            rv(1, false, false),
            rv(2, true, false),
            rv(3, false, false),
            rv(4, false, false),
            rv(5, false, true),
        ];
        let s = shown(&log);
        assert!(s.iter().all(|v| v.pinned || v.current), "{s:?}");
        let now = chrono::Utc::now();
        for ix in 0..s.len() {
            let pm = pill(&s, ix, now).unwrap();
            let v = &s[ix];
            assert_eq!(
                pm.actions.contains(&"Fork this pin"),
                !v.current && v.pinned
            );
            // "AI reply" answers the current version only.
            assert_eq!(pm.actions.contains(&"AI reply"), v.current);
            for (label, _, i) in &pm.entries {
                assert!(s[*i].pinned || s[*i].current, "{label}");
                assert!(!label.contains("v1") && !label.contains("v3") && !label.contains("v4"));
            }
        }
        assert!(!CURRENT_ACTIONS.contains(&"Fork this pin"));
        assert!(!PINNED_ACTIONS.contains(&"AI reply"));
    }

    /// Your own history: every version can be restored or pinned, but no
    /// version, pinned or not, offers Fork.
    #[test]
    fn own_versions_never_offer_fork() {
        for (pinned, endcap, version) in [
            (false, false, 1),
            (true, false, 2),
            (false, false, 3),
            (false, true, 4),
            (false, false, 5),
        ] {
            let v = Version {
                version,
                published_at: "2026-09-20T10:00:00Z".into(),
                note: None,
                pinned,
                endcap,
            };
            let a = own_actions(&v, 5);
            for act in a.list() {
                assert!(!act.label(version).to_lowercase().contains("fork"));
            }
            if !pinned && !endcap && version != 5 {
                assert_eq!(a.list(), [OwnAction::Pin, OwnAction::Restore]);
            }
        }
    }

    /// The reading search: title, author, origin and text, any case; an
    /// item held only as HTML is searched by its visible text, not its tags.
    #[test]
    fn search_matches_title_author_origin_and_text() {
        let r = ReadingItem {
            subscription_id: "s".into(),
            remote_id: "01K2KIT0KITES0000000000001".into(),
            subscription_title: "Kit's field notes".into(),
            origin: "https://kit.blyg.example.com/".into(),
            kind: Kind::Thread,
            state: "current".into(),
            version: 1,
            created: None,
            updated: None,
            observed_at: "2026-09-20T10:00:00Z".into(),
            content_md: String::new(),
            content_html: "<h1>On kites</h1><p class=\"lede\">Wind &amp; string.</p>".into(),
            author: Some(blyg_core::Author {
                name: Some("Kit Moreno".into()),
                url: None,
            }),
            page: None,
            thumb: None,
            hoppers: vec![],
            pinned_version_retained: None,
            read_version: None,
            stub_of: None,
            forked_from: None,
            transclusions: vec![],
        };
        for q in [
            "",
            "  ",
            "KITES",
            "wind & string",
            "moreno",
            "field notes",
            "kit.blyg",
        ] {
            assert!(matches_query(&r, q), "{q:?}");
        }
        for q in ["lede", "class", "h1", "zeppelin"] {
            assert!(!matches_query(&r, q), "{q:?} is markup or absent");
        }
        let mut md = r.clone();
        md.content_md = "Paper and bamboo.".into();
        assert!(matches_query(&md, "bamboo"));
        assert_eq!(filter(&[r.clone(), md, r], "bamboo"), [1]);
        assert_eq!(no_match(" kites  "), "No posts match “kites”");
    }

    /// The quote picker offers someone else's post as the current version
    /// only (`![[id]]`, snapshotted at publish): no version choice at all.
    #[test]
    fn quote_picker_offers_only_current_posts() {
        let sub = Subscription {
            id: "s".into(),
            kind: SubscriptionKind::Blyg,
            origin: "https://rue.blyg.example.com/".into(),
            feed_url: "https://rue.blyg.example.com/".into(),
            title: "Rue".into(),
            status: "active".into(),
            in_blogroll: false,
        };
        let mut r = ReadingItem {
            subscription_id: "s".into(),
            remote_id: "01K2RUE0TRUST0000000000001".into(),
            subscription_title: "Rue".into(),
            origin: "https://rue.blyg.example.com/".into(),
            kind: Kind::Fragment,
            state: "current".into(),
            version: 5,
            created: None,
            updated: None,
            observed_at: "2026-09-20T10:00:00Z".into(),
            content_md: "Current words.".into(),
            content_html: String::new(),
            author: None,
            page: None,
            thumb: None,
            hoppers: vec![],
            pinned_version_retained: None,
            read_version: None,
            stub_of: None,
            forked_from: None,
            transclusions: vec![],
        };
        let q = quotables(
            &[],
            std::slice::from_ref(&r),
            std::slice::from_ref(&sub),
            None,
            "",
        );
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].title, "Current words.");
        assert!(!q[0].id.contains('@') && !q[0].id.contains("/v"));
        r.state = "tombstone".into();
        assert!(quotables(&[], &[r], &[sub], None, "").is_empty());
    }

    #[test]
    fn status_bar_per_screen() {
        use super::super::View;
        let s = screen_status(View::Reading, 12);
        assert!(!s.post, "no post counter on Reading");
        assert_eq!(s.to_read.as_deref(), Some("12 to read"));
        assert_eq!(screen_status(View::Reading, 0).to_read, None);
        // Mentions: a list, never a count.
        let m = screen_status(View::Mentions, 12);
        assert_eq!((m.post, m.to_read), (false, None));
        let s = screen_status(View::Subscriptions, 12);
        assert_eq!((s.post, s.to_read), (false, None));
        assert!(screen_status(View::Posts, 3).post);
    }
}
