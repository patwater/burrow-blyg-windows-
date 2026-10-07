//! The `[[` / `![[` picker's search: studio 0.29's `GET /api/search`,
//! answered locally. It offers what publish will accept as a link or quote
//! target: your own published posts and posts imported from blyg
//! subscriptions (current, or a withdrawn one whose pinned version is
//! retained). Every word of the query must appear somewhere in a post's
//! text or id, in any order, case-insensitively.
//!
//! [`search_in`] is the in-memory version (the `Backend` default); the
//! live backend answers from SQLite's FTS5 indexes and checks each hit with
//! the same [`matches`], so both agree on what matches.

use serde::{Deserialize, Serialize};

use crate::model::{Item, Kind, ReadingItem, Status, Subscription, SubscriptionKind, plain_text};

/// At most this many words of a query count (upstream's limit).
pub const MAX_WORDS: usize = 8;
/// An excerpt's length in characters (upstream's `clampText(…, 140)`).
pub const EXCERPT_CHARS: usize = 140;
/// How many rows a search returns unless the query says otherwise.
pub const DEFAULT_LIMIT: usize = 50;

/// Which posts the picker searches (`source=all|mine|imported`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PickSource {
    #[default]
    All,
    Mine,
    Imported,
}

impl PickSource {
    pub fn as_str(self) -> &'static str {
        match self {
            PickSource::All => "all",
            PickSource::Mine => "mine",
            PickSource::Imported => "imported",
        }
    }
}

/// `sort=newest|oldest`, by the post's date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PickSort {
    #[default]
    Newest,
    Oldest,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PickQuery {
    pub text: String,
    pub source: PickSource,
    /// Only posts imported through this subscription (implies imported).
    pub sub: Option<String>,
    pub sort: PickSort,
    /// The server id of the post being written (it can't name itself).
    pub exclude: Option<String>,
    /// 0 = [`DEFAULT_LIMIT`].
    pub limit: usize,
}

impl PickQuery {
    pub fn limit(&self) -> usize {
        if self.limit == 0 {
            DEFAULT_LIMIT
        } else {
            self.limit
        }
    }

    /// Whether own posts are searched (not with `imported` or a `sub`).
    pub fn wants_mine(&self) -> bool {
        self.source != PickSource::Imported && self.sub.is_none()
    }

    pub fn wants_imported(&self) -> bool {
        self.source != PickSource::Mine
    }
}

/// One row the picker offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pickable {
    /// What goes between the brackets.
    pub id: String,
    /// The post's title (its first line of its own words).
    pub title: String,
    /// Plain text, at most [`EXCERPT_CHARS`].
    pub excerpt: String,
    /// `Mine` or `Imported` (never `All`).
    pub source: PickSource,
    pub kind: Kind,
    /// The version a quote would take.
    pub version: u32,
    /// The post's date (ISO-8601), what `sort` orders by.
    pub at: String,
    pub subscription_id: Option<String>,
    /// The subscription's name, else the origin's host; `None` for yours.
    pub source_title: Option<String>,
}

/// The query's words, lowercased (at most [`MAX_WORDS`]).
pub fn words(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .take(MAX_WORDS)
        .map(str::to_lowercase)
        .collect()
}

/// Every word appears in `text` or in `id` (words are lowercased already).
pub fn matches(words: &[String], text: &str, id: &str) -> bool {
    if words.is_empty() {
        return true;
    }
    let text = text.to_lowercase();
    let id = id.to_lowercase();
    words
        .iter()
        .all(|w| text.contains(w.as_str()) || id.contains(w.as_str()))
}

/// The first [`EXCERPT_CHARS`] characters of the plain text, `…` when cut.
pub fn excerpt(md: &str) -> String {
    let t = plain_text(md);
    if t.chars().count() <= EXCERPT_CHARS {
        return t;
    }
    let cut: String = t.chars().take(EXCERPT_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

fn host(origin: &str) -> String {
    url::Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| origin.trim_end_matches('/').to_string())
}

/// A row for your own published post, if it's one the query can name.
pub fn mine(it: &Item, q: &PickQuery, words: &[String]) -> Option<Pickable> {
    let sid = it.server_id.as_ref()?;
    if it.status != Status::Public || q.exclude.as_deref() == Some(sid.0.as_str()) {
        return None;
    }
    if !matches(words, &it.content_md, &sid.0) {
        return None;
    }
    Some(Pickable {
        id: sid.0.clone(),
        title: it.title(),
        excerpt: excerpt(&it.content_md),
        source: PickSource::Mine,
        kind: it.kind,
        version: it.version,
        at: it.updated.clone(),
        subscription_id: None,
        source_title: None,
    })
}

/// A row for an imported post (`blyg` = it came from a blyg subscription).
pub fn imported(r: &ReadingItem, blyg: bool, q: &PickQuery, words: &[String]) -> Option<Pickable> {
    let version = if r.state == "current" {
        r.version
    } else {
        r.pinned_version_retained?
    };
    if !blyg || q.exclude.as_deref() == Some(r.remote_id.as_str()) {
        return None;
    }
    if q.sub.as_deref().is_some_and(|s| s != r.subscription_id) {
        return None;
    }
    if !matches(words, &r.content_md, &r.remote_id) {
        return None;
    }
    let title = r.subscription_title.trim();
    Some(Pickable {
        id: r.remote_id.clone(),
        title: crate::model::plain_title(&r.content_md).unwrap_or_else(|| "…".into()),
        excerpt: excerpt(&r.content_md),
        source: PickSource::Imported,
        kind: r.kind,
        version,
        at: r.sort_at(),
        subscription_id: Some(r.subscription_id.clone()),
        source_title: Some(if title.is_empty() {
            host(&r.origin)
        } else {
            title.to_string()
        }),
    })
}

/// Sort by date (then id) and keep the first `limit`, one row per id.
pub fn finish(mut rows: Vec<Pickable>, q: &PickQuery) -> Vec<Pickable> {
    let mut seen = std::collections::HashSet::new();
    rows.retain(|r| seen.insert(r.id.clone()));
    rows.sort_by(|a, b| {
        let o = crate::util::parse_ts_ms(&a.at)
            .cmp(&crate::util::parse_ts_ms(&b.at))
            .then_with(|| a.id.cmp(&b.id));
        match q.sort {
            PickSort::Newest => o.reverse(),
            PickSort::Oldest => o,
        }
    });
    rows.truncate(q.limit());
    rows
}

/// The search over what's in memory (the `Backend` default).
pub fn search_in(
    items: &[Item],
    reading: &[ReadingItem],
    subs: &[Subscription],
    q: &PickQuery,
) -> Vec<Pickable> {
    let w = words(&q.text);
    let mut rows = Vec::new();
    if q.wants_mine() {
        rows.extend(items.iter().filter_map(|it| mine(it, q, &w)));
    }
    if q.wants_imported() {
        let blyg = |id: &str| {
            subs.iter()
                .any(|s| s.id == id && s.kind == SubscriptionKind::Blyg)
        };
        rows.extend(
            reading
                .iter()
                .filter_map(|r| imported(r, blyg(&r.subscription_id), q, &w)),
        );
    }
    finish(rows, q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_anywhere_in_text_or_id() {
        let w = words("  Garden  BEDS ");
        assert_eq!(w, vec!["garden", "beds"]);
        assert!(matches(&w, "The beds in the garden", "x"));
        assert!(!matches(&w, "The garden", "x"));
        assert!(matches(&words("01ARZ beds"), "beds", "01arzabc"));
        assert!(matches(&words("ÉTÉ"), "un été", "x"));
        assert_eq!(words("a b c d e f g h i j").len(), MAX_WORDS);
    }

    #[test]
    fn excerpts_are_plain_and_clamped() {
        assert_eq!(
            excerpt("# Title\n\n*Some* [text](https://x.example)"),
            "Title Some text"
        );
        let long = "word ".repeat(60);
        let e = excerpt(&long);
        assert!(e.chars().count() <= EXCERPT_CHARS && e.ends_with('…'));
    }
}
