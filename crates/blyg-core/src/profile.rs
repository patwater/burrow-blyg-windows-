//! Profiles (docs/SPEC.md § Profiles): who a blyg is, who they read, what
//! they posted lately, and whom they quote, stub or fork.
//!
//! Everything comes from the other blyg's **public** files, fetched with
//! [`PublicClient`](crate::api::public::PublicClient) (no token, no cookies),
//! and only when the user opens a profile: never in the background.
//!
//! - `blyg.json` (the manifest, §6.1): name, bio, avatar, links.
//! - `blogroll.opml` (§11), only when the manifest lists it.
//! - `items/index.json` (§6.2): recent items; titles come from `feed.xml`
//!   (one more fetch) or from posts already held locally.
//! - A plain RSS/Atom feed gets a simpler card: title, link, recent items.
//!
//! Connections (the origins a blyg quotes, stubs and forks) are computed
//! from what's already held locally, plus the quote origins baked into the
//! feed's HTML. Nothing crawls item documents. Counts there are the author's
//! own data ("stubbed 2 posts"), never a social metric: there are no follower
//! or subscriber counts anywhere.
//!
//! Parsing here is pure and tolerant (tolerance rule: ignore the unknown,
//! never reject): missing manifest fields, a missing blogroll, a 404 index
//! and odd OPML all degrade to "less to show".

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{AuthorLink, Item, ReadingItem, origin_url};

/// How long a fetched profile is trusted before opening it fetches again.
pub const PROFILE_TTL_MS: i64 = 60 * 60 * 1000;

/// At most this many recent posts are kept on a profile.
pub const MAX_POSTS: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileKind {
    /// A blyg: manifest, blogroll, archive index.
    Blyg,
    /// A plain RSS/Atom feed (L0): a simpler card.
    Feed,
}

/// Someone's public profile, as the profile sheet shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub kind: ProfileKind,
    /// The blyg origin (with a trailing slash); for a feed, its site link
    /// (or the feed URL when it has none). The identity the UI shows.
    pub origin: String,
    /// The feed to subscribe to (a feed card), or the blyg's own feed.
    pub feed_url: Option<String>,
    /// `author.name` (blyg) or the feed's title.
    pub name: Option<String>,
    /// The manifest's `title` (a blyg's site title).
    pub title: Option<String>,
    pub bio: Option<String>,
    /// Absolute avatar URL (resolved against the origin).
    pub avatar: Option<String>,
    /// Author links, absolute http(s)/mailto only.
    pub links: Vec<AuthorLink>,
    /// Their public blogroll (`blogroll.opml`). Empty when they have none.
    pub blogroll: Vec<BlogrollEntry>,
    /// Whether the manifest lists a blogroll at all.
    pub has_blogroll: bool,
    /// Recent items, newest first.
    pub posts: Vec<ProfilePost>,
    /// Origins this blyg quotes, stubs and forks (computed on every read
    /// from what's held locally; see [`connections`]).
    pub connections: Vec<Connection>,
    /// Quote sources seen in the feed's HTML: `(origin, their item id)`.
    /// Folded into `connections`.
    #[serde(default)]
    pub feed_quotes: Vec<(String, String)>,
    /// This is the user's own blyg.
    #[serde(default)]
    pub own: bool,
    /// When the public files were fetched (unix ms).
    pub fetched_at: i64,
    /// Served from the cache because a refresh couldn't reach the origin.
    #[serde(default)]
    pub stale: bool,
}

/// One `<outline>` of an OPML blogroll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlogrollEntry {
    pub title: String,
    /// `xmlUrl`: the feed.
    pub xml_url: Option<String>,
    /// `htmlUrl`: the site.
    pub html_url: Option<String>,
}

impl BlogrollEntry {
    /// The address a profile or a Follow uses: the site, else the feed.
    pub fn url(&self) -> Option<&str> {
        self.html_url.as_deref().or(self.xml_url.as_deref())
    }
}

/// A recent item on a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilePost {
    /// The item id (blyg) or the feed item's link/guid.
    pub id: String,
    /// The title or first line; `None` when nothing we hold says.
    pub title: Option<String>,
    /// `fragment` / `thread` / `withdrawn` / `rss`; free text (tolerance).
    pub kind: String,
    pub version: Option<u32>,
    /// A pinned version is known (from the index or a cached changelog).
    pub pinned: bool,
    /// ISO-8601, as served.
    pub date: Option<String>,
    /// Where to read it on the web.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Relation {
    /// Quotes (transcludes) posts of that origin.
    Quotes,
    /// Wrote stubs (replies) to posts of that origin.
    Stubs,
    /// Forked pins of that origin.
    Forks,
}

/// An origin this blyg is connected to, and how often (in the author's own
/// posts that we hold).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub origin: String,
    pub relation: Relation,
    /// How many of their posts do it: the author's data, not a social count.
    pub count: u32,
}

impl Connection {
    /// "stubbed 2 posts", "forked 1 pin", "quoted 3 times".
    pub fn label(&self) -> String {
        let n = self.count;
        let s = if n == 1 { "" } else { "s" };
        match self.relation {
            Relation::Quotes => {
                if n == 1 {
                    "quoted once".into()
                } else {
                    format!("quoted {n} times")
                }
            }
            Relation::Stubs => format!("stubbed {n} post{s}"),
            Relation::Forks => format!("forked {n} pin{s}"),
        }
    }
}

// ================================================================ origins

/// `https://x.example/blyg` → `https://x.example/blyg/`: an origin with a
/// trailing slash, query and fragment stripped, scheme and host lowercased.
/// `None` unless it's an absolute http(s) URL.
pub fn normalize_origin(url: &str) -> Option<String> {
    let mut u = url::Url::parse(url.trim()).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    u.set_query(None);
    u.set_fragment(None);
    let mut s = u.to_string();
    if !s.ends_with('/') {
        s.push('/');
    }
    Some(s)
}

/// The URL as the user gave it, cleaned for use as a cache key: trimmed,
/// fragment dropped, a bare host gets `https://`.
pub fn clean_url(url: &str) -> Option<String> {
    let t = url.trim();
    if t.is_empty() {
        return None;
    }
    let t = if t.contains("://") {
        t.to_string()
    } else {
        format!("https://{t}")
    };
    let mut u = url::Url::parse(&t).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    u.set_fragment(None);
    Some(u.to_string())
}

/// `a` and `b` name the same origin (trailing slash and case tolerated).
pub fn same_origin(a: &str, b: &str) -> bool {
    match (normalize_origin(a), normalize_origin(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a.trim_end_matches('/') == b.trim_end_matches('/'),
    }
}

/// `url` is at or under `origin` (a permalink of that blyg, say).
pub fn under_origin(url: &str, origin: &str) -> bool {
    match (clean_url(url), normalize_origin(origin)) {
        (Some(u), Some(o)) => {
            let u = if u.ends_with('/') { u } else { format!("{u}/") };
            u.starts_with(&o)
        }
        _ => false,
    }
}

// ================================================================ manifest

/// What a profile takes from `blyg.json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Manifest {
    pub title: Option<String>,
    pub name: Option<String>,
    pub bio: Option<String>,
    pub avatar: Option<String>,
    pub links: Vec<AuthorLink>,
    /// Relative paths as listed (resolve against the origin).
    pub feed: Option<String>,
    pub items: Option<String>,
    pub blogroll: Option<String>,
}

fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// An absolute URL a link may open: http(s) or mailto, resolved against the origin.
fn link_url(origin: &str, href: &str) -> Option<String> {
    let h = href.trim();
    if h.is_empty() {
        return None;
    }
    if let Some(rest) = h.strip_prefix("mailto:") {
        return (!rest.is_empty()).then(|| h.to_string());
    }
    let abs = origin_url(origin, h)?;
    let u = url::Url::parse(&abs).ok()?;
    matches!(u.scheme(), "http" | "https").then_some(abs)
}

/// Parse a manifest. Anything missing is `None`; a manifest that isn't an
/// object reads as empty. `origin` resolves the avatar and relative links.
pub fn parse_manifest(v: &Value, origin: &str) -> Manifest {
    let author = v.get("author").cloned().unwrap_or(Value::Null);
    let links = author
        .get("links")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    let url = link_url(origin, l.get("url")?.as_str()?)?;
                    let label = text(l, "label").unwrap_or_else(|| {
                        crate::profile::host_of(&url).unwrap_or_else(|| url.clone())
                    });
                    Some(AuthorLink { label, url })
                })
                .collect()
        })
        .unwrap_or_default();
    Manifest {
        title: text(v, "title"),
        name: text(&author, "name"),
        bio: text(&author, "bio"),
        avatar: text(&author, "avatar").and_then(|a| link_url(origin, &a)),
        links,
        feed: text(v, "feed"),
        items: text(v, "items"),
        blogroll: text(v, "blogroll"),
    }
}

/// A manifest by the protocol's own test (§12.1: "parse-success is the
/// test"): a JSON object with a `"blyg"` key.
pub fn is_manifest(v: &Value) -> bool {
    v.get("blyg").is_some()
}

pub(crate) fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

// ================================================================ index

/// One row of `items/index.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub id: String,
    pub kind: String,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub version: Option<u32>,
    /// Not in the spec's index; read when a server adds it.
    pub pinned: bool,
}

/// Parse the archive index, newest `updated` first. Rows without an id are
/// skipped; everything else is optional.
pub fn parse_index(v: &Value) -> Vec<IndexEntry> {
    let rows = v
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| v.as_array());
    let mut out: Vec<IndexEntry> = rows
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    let id = text(r, "id")?;
                    Some(IndexEntry {
                        id,
                        kind: text(r, "kind").unwrap_or_else(|| "item".into()),
                        created: text(r, "created"),
                        updated: text(r, "updated"),
                        version: r
                            .get("version")
                            .and_then(Value::as_u64)
                            .and_then(|n| u32::try_from(n).ok()),
                        pinned: r.get("pinned").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| {
        let ka = a.updated.as_ref().or(a.created.as_ref());
        let kb = b.updated.as_ref().or(b.created.as_ref());
        kb.cmp(&ka)
    });
    out
}

// ================================================================ XML (lenient)

/// Decode the five XML entities plus numeric references; anything else stays.
pub fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The attributes of one start tag's inside (`outline text="a" xmlUrl='b'`),
/// names lowercased. Unquoted values and junk are tolerated.
fn attrs(tag: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let b = tag.as_bytes();
    let mut i = 0;
    // Skip the element name.
    while i < b.len() && !b[i].is_ascii_whitespace() {
        i += 1;
    }
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let ns = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        let name = tag[ns..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let value = if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                let v = &tag[vs..i];
                i += 1;
                v
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                &tag[vs..i]
            };
            if !name.is_empty() {
                out.insert(name, xml_unescape(value));
            }
        } else if name.is_empty() {
            i += 1;
        }
    }
    out
}

/// Every start tag named `name` (case-insensitive), as its inside text.
fn start_tags<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let lower = xml.to_ascii_lowercase();
    let needle = format!("<{}", name.to_ascii_lowercase());
    let mut out = vec![];
    let mut from = 0;
    while let Some(i) = lower[from..].find(&needle) {
        let start = from + i + 1;
        let after = lower.as_bytes().get(start + name.len()).copied();
        from = start;
        if !matches!(after, Some(c) if c.is_ascii_whitespace() || c == b'>' || c == b'/') {
            continue;
        }
        let Some(end) = xml[start..].find('>') else {
            break;
        };
        out.push(xml[start..start + end].trim_end_matches('/'));
        from = start + end;
    }
    out
}

/// Parse an OPML blogroll: every `<outline>` with a feed or site URL (http(s)
/// only). Nested folders are flattened; junk around them is ignored.
pub fn parse_opml(xml: &str) -> Vec<BlogrollEntry> {
    let mut seen = HashSet::new();
    start_tags(xml, "outline")
        .into_iter()
        .filter_map(|t| {
            let a = attrs(t);
            let http = |k: &str| {
                a.get(k)
                    .map(|s| s.trim().to_string())
                    .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
            };
            let xml_url = http("xmlurl");
            let html_url = http("htmlurl");
            if xml_url.is_none() && html_url.is_none() {
                return None;
            }
            let title = a
                .get("title")
                .or_else(|| a.get("text"))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| html_url.as_deref().or(xml_url.as_deref()).and_then(host_of))
                .unwrap_or_default();
            let key = xml_url.clone().or_else(|| html_url.clone());
            seen.insert(key).then_some(BlogrollEntry {
                title,
                xml_url,
                html_url,
            })
        })
        .collect()
}

/// A parsed RSS 2.0 or Atom feed (the parts a profile uses).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Feed {
    pub title: Option<String>,
    pub link: Option<String>,
    /// `<blyg:manifest>`: the feed belongs to a blyg (the upgrade hook).
    pub manifest: Option<String>,
    pub items: Vec<FeedItem>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeedItem {
    pub title: Option<String>,
    pub link: Option<String>,
    pub date: Option<String>,
    /// `<blyg:id>`, `<blyg:kind>`, `<blyg:version>`.
    pub blyg_id: Option<String>,
    pub blyg_kind: Option<String>,
    pub blyg_version: Option<u32>,
    /// Origins of quotes baked into the description (`data-blyg-origin`).
    pub quote_origins: Vec<String>,
}

/// The inner text of the first `<name>…</name>` in `xml` (CDATA unwrapped,
/// entities decoded). Case-insensitive.
fn inner(xml: &str, name: &str) -> Option<String> {
    let lower = xml.to_ascii_lowercase();
    let open = format!("<{}", name.to_ascii_lowercase());
    let close = format!("</{}>", name.to_ascii_lowercase());
    let mut from = 0;
    loop {
        let i = from + lower[from..].find(&open)?;
        let after = lower.as_bytes().get(i + open.len()).copied();
        if !matches!(after, Some(c) if c.is_ascii_whitespace() || c == b'>') {
            from = i + open.len();
            continue;
        }
        let gt = i + xml[i..].find('>')?;
        if xml[..gt].ends_with('/') {
            return None;
        }
        let end = gt + 1 + lower[gt + 1..].find(&close)?;
        let raw = xml[gt + 1..end].trim();
        let raw = raw
            .strip_prefix("<![CDATA[")
            .and_then(|r| r.strip_suffix("]]>"))
            .map(str::to_string)
            .unwrap_or_else(|| xml_unescape(raw));
        return Some(raw.trim().to_string());
    }
}

/// Blocks `<name …>…</name>` (case-insensitive), as their inside.
fn blocks<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let lower = xml.to_ascii_lowercase();
    let open = format!("<{}", name.to_ascii_lowercase());
    let close = format!("</{}>", name.to_ascii_lowercase());
    let mut out = vec![];
    let mut from = 0;
    while let Some(i) = lower[from..].find(&open) {
        let i = from + i;
        let after = lower.as_bytes().get(i + open.len()).copied();
        if !matches!(after, Some(c) if c.is_ascii_whitespace() || c == b'>') {
            from = i + open.len();
            continue;
        }
        let Some(gt) = xml[i..].find('>') else { break };
        let body = i + gt + 1;
        let Some(end) = lower[body..].find(&close) else {
            break;
        };
        out.push(&xml[body..body + end]);
        from = body + end + close.len();
    }
    out
}

/// `data-blyg-origin="…"` values in (possibly escaped) HTML.
pub fn quote_origins(html: &str) -> Vec<String> {
    let html = if html.contains("&lt;") || html.contains("&quot;") {
        xml_unescape(html)
    } else {
        html.to_string()
    };
    let mut out = vec![];
    let mut rest = html.as_str();
    while let Some(i) = rest.find("data-blyg-origin=") {
        rest = &rest[i + "data-blyg-origin=".len()..];
        let q = rest.chars().next();
        let (Some(q @ ('"' | '\'')), true) = (q, rest.len() > 1) else {
            continue;
        };
        let body = &rest[1..];
        let Some(end) = body.find(q) else { break };
        let v = xml_unescape(&body[..end]);
        if let Some(o) = normalize_origin(&v) {
            out.push(o);
        }
        rest = &body[end..];
    }
    out
}

/// Parse RSS 2.0 or Atom, leniently. `None` when it isn't a feed at all.
pub fn parse_feed(xml: &str) -> Option<Feed> {
    let lower = xml.to_ascii_lowercase();
    let atom = lower.contains("<feed") && !lower.contains("<rss") && !lower.contains("<channel");
    if !atom && !lower.contains("<rss") && !lower.contains("<channel") {
        return None;
    }
    let (head, item_tag) = if atom {
        let first = lower.find("<entry").unwrap_or(xml.len());
        (&xml[..first], "entry")
    } else {
        let first = lower.find("<item").unwrap_or(xml.len());
        (&xml[..first], "item")
    };
    let atom_link = |x: &str| -> Option<String> {
        let tags = start_tags(x, "link");
        let pick = tags
            .iter()
            .map(|t| attrs(t))
            .find(|a| a.get("rel").is_none_or(|r| r == "alternate"))?;
        pick.get("href").cloned()
    };
    let link = if atom {
        atom_link(head)
    } else {
        inner(head, "link").filter(|l| !l.is_empty())
    };
    let items = blocks(xml, item_tag)
        .into_iter()
        .map(|b| {
            let desc = inner(b, "description")
                .or_else(|| inner(b, "content:encoded"))
                .or_else(|| inner(b, "content"))
                .or_else(|| inner(b, "summary"))
                .unwrap_or_default();
            FeedItem {
                title: inner(b, "title").filter(|t| !t.is_empty()),
                link: if atom {
                    atom_link(b)
                } else {
                    inner(b, "link").filter(|l| !l.is_empty())
                },
                date: inner(b, "pubDate")
                    .or_else(|| inner(b, "updated"))
                    .or_else(|| inner(b, "published"))
                    .or_else(|| inner(b, "dc:date")),
                blyg_id: inner(b, "blyg:id").filter(|s| !s.is_empty()),
                blyg_kind: inner(b, "blyg:kind").filter(|s| !s.is_empty()),
                blyg_version: inner(b, "blyg:version").and_then(|v| v.parse().ok()),
                quote_origins: quote_origins(&desc),
            }
        })
        .collect();
    Some(Feed {
        title: inner(head, "title").filter(|t| !t.is_empty()),
        link,
        manifest: inner(head, "blyg:manifest").filter(|s| !s.is_empty()),
        items,
    })
}

// ================================================================ assembly

/// The first non-empty line of a post, without heading marks.
pub fn first_line(md: &str) -> Option<String> {
    md.lines()
        .map(|l| l.trim_start_matches('#').trim())
        .find(|l| !l.is_empty() && !l.starts_with("![["))
        .map(|l| {
            let mut s: String = l.chars().take(120).collect();
            if l.chars().count() > 120 {
                s.push('…');
            }
            s
        })
}

/// Recent posts of a blyg: the index (newest first), titled from the feed
/// or from posts held locally, pins from the index or `pinned` (a lookup of
/// cached changelogs). Falls back to the feed alone when the index is gone.
pub fn blyg_posts(
    origin: &str,
    index: &[IndexEntry],
    feed: Option<&Feed>,
    held: &[ReadingItem],
    pinned: &dyn Fn(&str) -> bool,
) -> Vec<ProfilePost> {
    let feed_title = |id: &str| {
        feed.and_then(|f| {
            f.items
                .iter()
                .find(|i| i.blyg_id.as_deref() == Some(id))
                .and_then(|i| i.title.clone())
        })
    };
    let held_title = |id: &str| {
        held.iter()
            .find(|r| r.remote_id == id && same_origin(&r.origin, origin))
            .and_then(|r| first_line(&r.content_md))
    };
    if index.is_empty() {
        return feed
            .map(|f| {
                let mut seen = HashSet::new();
                f.items
                    .iter()
                    .filter(|i| seen.insert(i.blyg_id.clone().or_else(|| i.link.clone())))
                    .take(MAX_POSTS)
                    .map(|i| ProfilePost {
                        id: i
                            .blyg_id
                            .clone()
                            .or_else(|| i.link.clone())
                            .unwrap_or_default(),
                        title: i.title.clone(),
                        kind: i.blyg_kind.clone().unwrap_or_else(|| "post".into()),
                        version: i.blyg_version,
                        pinned: i.blyg_id.as_deref().is_some_and(pinned),
                        date: i.date.clone(),
                        url: i.link.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    index
        .iter()
        .take(MAX_POSTS)
        .map(|e| {
            let withdrawn = e.kind == "withdrawn";
            ProfilePost {
                id: e.id.clone(),
                title: if withdrawn {
                    None
                } else {
                    feed_title(&e.id).or_else(|| held_title(&e.id))
                },
                kind: e.kind.clone(),
                version: e.version,
                pinned: e.pinned || pinned(&e.id),
                date: e.updated.clone().or_else(|| e.created.clone()),
                url: page_url(origin, &e.kind, &e.id),
            }
        })
        .collect()
}

/// `{origin}f/{id}/` or `t/{id}/` (the ref's page paths; readers may fall
/// back to them, §5). `None` for a withdrawn item or an unknown kind.
fn page_url(origin: &str, kind: &str, id: &str) -> Option<String> {
    let p = match kind {
        "fragment" => "f",
        "thread" => "t",
        _ => return None,
    };
    origin_url(origin, &format!("{p}/{id}/"))
}

/// Recent items of a plain feed.
pub fn feed_posts(feed: &Feed) -> Vec<ProfilePost> {
    feed.items
        .iter()
        .take(MAX_POSTS)
        .map(|i| ProfilePost {
            id: i.link.clone().unwrap_or_default(),
            title: i.title.clone(),
            kind: "rss".into(),
            version: None,
            pinned: false,
            date: i.date.clone(),
            url: i.link.clone(),
        })
        .collect()
}

/// Who `origin` quotes, stubs and forks, from the posts of theirs held
/// locally (`held`: reading items) plus quote sources seen in their feed
/// (`feed_quotes`). Each post counts once per (origin, relation); the blyg's
/// own origin is left out. Most-connected first, then by origin.
pub fn connections(
    origin: &str,
    held: &[ReadingItem],
    feed_quotes: &[(String, String)],
) -> Vec<Connection> {
    let mut seen: HashSet<(String, Relation, String)> = HashSet::new();
    let mut add = |o: &str, rel: Relation, item: &str| {
        let Some(o) = normalize_origin(o) else { return };
        if same_origin(&o, origin) {
            return;
        }
        seen.insert((o, rel, item.to_string()));
    };
    for r in held.iter().filter(|r| same_origin(&r.origin, origin)) {
        if let Some(t) = r.stub_of.as_ref().and_then(|s| s.origin.as_deref()) {
            add(t, Relation::Stubs, &r.remote_id);
        }
        if let Some(f) = &r.forked_from {
            add(&f.origin, Relation::Forks, &r.remote_id);
        }
        for t in &r.transclusions {
            if let Some(o) = &t.origin {
                add(o, Relation::Quotes, &r.remote_id);
            }
        }
        for o in quote_origins(&r.content_html) {
            add(&o, Relation::Quotes, &r.remote_id);
        }
    }
    for (o, id) in feed_quotes {
        add(o, Relation::Quotes, id);
    }
    tally(seen)
}

/// Your own blyg's connections, from your own posts: `stub_of`,
/// `forked_from`, and each `![[id]]` whose source is a held reading item
/// from another origin.
pub fn own_connections(origin: &str, items: &[Item], held: &[ReadingItem]) -> Vec<Connection> {
    let mut seen: HashSet<(String, Relation, String)> = HashSet::new();
    for it in items
        .iter()
        .filter(|i| i.status == crate::model::Status::Public)
    {
        let key = it.local_id.0.clone();
        let mut add = |o: &str, rel: Relation| {
            if let Some(o) = normalize_origin(o).filter(|o| !same_origin(o, origin)) {
                seen.insert((o, rel, key.clone()));
            }
        };
        if let Some(s) = &it.stub_of {
            add(&s.origin, Relation::Stubs);
        }
        if let Some(f) = &it.forked_from {
            add(&f.origin, Relation::Forks);
        }
        for id in transclusion_ids(&it.content_md) {
            if let Some(r) = held.iter().find(|r| r.remote_id == id) {
                add(&r.origin, Relation::Quotes);
            }
        }
    }
    tally(seen)
}

/// Ids of `![[id]]` lines (the transclusion grammar, §10.1).
pub fn transclusion_ids(md: &str) -> Vec<String> {
    md.lines()
        .filter_map(|l| {
            let t = l.trim();
            let id = t.strip_prefix("![[")?.strip_suffix("]]")?;
            (id.len() == 26 && id.chars().all(|c| c.is_ascii_alphanumeric()))
                .then(|| id.to_string())
        })
        .collect()
}

fn tally(seen: HashSet<(String, Relation, String)>) -> Vec<Connection> {
    let mut counts: BTreeMap<(String, Relation), u32> = BTreeMap::new();
    for (o, r, _) in seen {
        *counts.entry((o, r)).or_default() += 1;
    }
    let mut out: Vec<Connection> = counts
        .into_iter()
        .map(|((origin, relation), count)| Connection {
            origin,
            relation,
            count,
        })
        .collect();
    out.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.origin.cmp(&b.origin))
            .then_with(|| a.relation.cmp(&b.relation))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const O: &str = "https://jd.example.org/";

    #[test]
    fn manifest_reads_what_is_there_and_resolves_urls() {
        let m = parse_manifest(
            &json!({
                "blyg": "0.2", "title": "JD's blyg",
                "author": { "name": "JD", "bio": "Tinkering in public.", "avatar": "media/a.png",
                    "links": [
                        { "label": "site", "url": "https://jd.example.org/about" },
                        { "url": "/rel" },
                        { "label": "bad", "url": "javascript:alert(1)" },
                        { "label": "mail", "url": "mailto:jd@example.org" },
                        "junk", { "label": "no url" }
                    ] },
                "blogroll": "blogroll.opml", "feed": "feed.xml", "items": "items/index.json",
                "future": { "x": 1 }
            }),
            "https://jd.example.org/blyg/",
        );
        assert_eq!(m.name.as_deref(), Some("JD"));
        assert_eq!(m.title.as_deref(), Some("JD's blyg"));
        assert_eq!(
            m.avatar.as_deref(),
            Some("https://jd.example.org/blyg/media/a.png")
        );
        let urls: Vec<&str> = m.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://jd.example.org/about",
                "https://jd.example.org/rel",
                "mailto:jd@example.org"
            ]
        );
        assert_eq!(m.links[1].label, "jd.example.org", "a missing label → host");
        assert_eq!(m.blogroll.as_deref(), Some("blogroll.opml"));
        // Nothing at all: all None, no panic.
        let e = parse_manifest(&json!({ "blyg": "0.9", "author": "nope" }), O);
        assert_eq!(e, Manifest::default());
        assert_eq!(parse_manifest(&json!([1, 2]), O), Manifest::default());
    }

    #[test]
    fn opml_takes_outlines_and_tolerates_junk() {
        let xml = r#"<?xml version="1.0"?>
<opml version="2.0"><head><title>Blogroll</title></head><body>
  <outline type="rss" text="Ada &amp; co" xmlUrl="https://ada.example.net/feed.xml" htmlUrl="https://ada.example.net/"/>
  <outline text='Rue' xmlUrl='https://rue.example.com/feed.xml'>
  <outline text="Folder">
     <OUTLINE TEXT="Nested" XMLURL="https://nested.example.com/rss"/>
  </outline>
  <outline text="no urls"/>
  <outline text="evil" xmlUrl="javascript:alert(1)"/>
  <outline xmlUrl=https://bare.example.org/feed.xml />
  <outline text="dupe" xmlUrl="https://ada.example.net/feed.xml"/>
  <outlines text="not an outline" xmlUrl="https://no.example.com/"/>
  <outline text="unterminated
</body></opml> trailing junk <<<"#;
        let r = parse_opml(xml);
        let titles: Vec<&str> = r.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["Ada & co", "Rue", "Nested", "bare.example.org"]);
        assert_eq!(r[0].html_url.as_deref(), Some("https://ada.example.net/"));
        assert_eq!(r[0].url(), Some("https://ada.example.net/"));
        assert_eq!(r[1].url(), Some("https://rue.example.com/feed.xml"));
        assert!(parse_opml("").is_empty());
        assert!(parse_opml("<html>not opml</html>").is_empty());
    }

    #[test]
    fn index_sorts_newest_first_and_skips_bad_rows() {
        let v = json!({ "updated": "x", "items": [
            { "id": "a", "kind": "fragment", "created": "2030-01-01", "updated": "2030-01-02", "version": 1 },
            { "kind": "thread" },
            { "id": "b", "kind": "thread", "updated": "2030-03-01", "version": 3, "pinned": true },
            { "id": "c", "version": "two" },
            7
        ]});
        let ix = parse_index(&v);
        let ids: Vec<&str> = ix.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["b", "a", "c"]);
        assert!(ix[0].pinned);
        assert_eq!(ix[2].version, None);
        assert!(parse_index(&json!({ "items": "nope" })).is_empty());
        assert!(parse_index(&json!(null)).is_empty());
    }

    #[test]
    fn rss_and_atom_feeds() {
        let rss = r#"<?xml version="1.0"?><rss version="2.0" xmlns:blyg="https://blygger.org/ns/0.1"><channel>
<title>Omar &amp; notes</title><link>https://omar.example.org/</link>
<blyg:manifest>https://omar.example.org/blyg.json</blyg:manifest>
<item><title>First</title><link>https://omar.example.org/t/A/</link><pubDate>Mon, 01 Jan 2030 00:00:00 GMT</pubDate>
<blyg:id>A</blyg:id><blyg:kind>thread</blyg:kind><blyg:version>2</blyg:version>
<description><![CDATA[<blockquote class="blyg-transclusion" data-blyg-origin="https://ada.example.net/">x</blockquote>]]></description></item>
<item><title></title><link>https://omar.example.org/f/B/</link>
<description>&lt;blockquote data-blyg-origin=&quot;https://rue.example.com/&quot;&gt;</description></item>
</channel></rss>"#;
        let f = parse_feed(rss).unwrap();
        assert_eq!(f.title.as_deref(), Some("Omar & notes"));
        assert_eq!(f.link.as_deref(), Some("https://omar.example.org/"));
        assert!(f.manifest.is_some());
        assert_eq!(f.items.len(), 2);
        assert_eq!(f.items[0].blyg_id.as_deref(), Some("A"));
        assert_eq!(f.items[0].blyg_version, Some(2));
        assert_eq!(f.items[0].quote_origins, ["https://ada.example.net/"]);
        assert_eq!(f.items[1].title, None);
        assert_eq!(f.items[1].quote_origins, ["https://rue.example.com/"]);

        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title type="text">Field notes</title>
<link rel="self" href="https://fieldnotes.example.com/atom.xml"/><link href="https://fieldnotes.example.com/"/>
<entry><title>Moss</title><link rel="alternate" href="https://fieldnotes.example.com/moss"/><updated>2030-02-02T00:00:00Z</updated></entry>
</feed>"#;
        let a = parse_feed(atom).unwrap();
        assert_eq!(a.title.as_deref(), Some("Field notes"));
        assert_eq!(a.link.as_deref(), Some("https://fieldnotes.example.com/"));
        let posts = feed_posts(&a);
        assert_eq!(posts[0].title.as_deref(), Some("Moss"));
        assert_eq!(posts[0].date.as_deref(), Some("2030-02-02T00:00:00Z"));
        assert!(parse_feed("<html><body>hi</body></html>").is_none());
        assert!(parse_feed("").is_none());
    }

    fn held(id: &str, origin: &str) -> ReadingItem {
        serde_json::from_value(json!({
            "subscription_id": "s", "remote_id": id, "subscription_title": "t", "origin": origin,
            "kind": "thread", "state": "current", "version": 1, "created": null, "updated": null,
            "observed_at": "2030-01-01", "content_md": "Hello\n\nworld", "content_html": "",
            "author": null, "page": null, "thumb": null, "hoppers": []
        }))
        .unwrap()
    }

    #[test]
    fn reading_item_lineage_is_parsed_leniently() {
        let base = json!({
            "subscription_id": "s", "remote_id": "x", "subscription_title": "t", "origin": O,
            "kind": "thread", "state": "current", "version": 1, "created": null, "updated": null,
            "observed_at": "2030-01-01", "content_md": "", "content_html": "",
            "author": null, "page": null, "thumb": null, "hoppers": []
        });
        let mut v = base.clone();
        v["stub_of"] = json!({ "origin": "https://ada.example.net/", "id": "A", "version": 1 });
        v["forked_from"] = json!({ "origin": "https://rue.example.com/", "id": "R", "version": 3 });
        v["transclusions"] = json!([{ "id": "Q", "version": 2, "origin": "https://lin.example.org/" }, "junk", { "id": "L" }]);
        let r: ReadingItem = serde_json::from_value(v).unwrap();
        assert_eq!(
            r.stub_of.as_ref().unwrap().target(),
            Some("https://ada.example.net/")
        );
        assert_eq!(r.forked_from.as_ref().unwrap().version, 3);
        assert_eq!(r.transclusions.len(), 2);
        // Odd shapes read as absent, never an error.
        let mut v = base.clone();
        v["stub_of"] = json!({ "url": "https://page.example.com/post" });
        v["forked_from"] = json!("nope");
        v["transclusions"] = json!({ "not": "a list" });
        let r: ReadingItem = serde_json::from_value(v).unwrap();
        assert_eq!(
            r.stub_of.unwrap().target(),
            Some("https://page.example.com/post")
        );
        assert!(r.forked_from.is_none() && r.transclusions.is_empty());
        // Round-trips through the store's JSON.
        let r: ReadingItem = serde_json::from_value(base).unwrap();
        let back: ReadingItem = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn connections_count_posts_per_origin_and_relation() {
        let mut a = held("A", O);
        a.stub_of = Some(crate::model::StubOf {
            origin: Some("https://ada.example.net".into()),
            ..Default::default()
        });
        a.content_html = r#"<blockquote data-blyg-origin="https://lin.example.org/">q</blockquote>
<blockquote data-blyg-origin="https://lin.example.org/">q again</blockquote>
<blockquote data-blyg-origin="https://jd.example.org/">own quote</blockquote>"#
            .into();
        let mut b = held("B", "https://JD.example.org");
        b.stub_of = Some(crate::model::StubOf {
            origin: Some("https://ada.example.net/".into()),
            ..Default::default()
        });
        b.forked_from = Some(crate::model::RemoteRef {
            origin: "https://rue.example.com/".into(),
            id: "R".into(),
            version: 3,
        });
        // Someone else's post: not theirs, not counted.
        let mut c = held("C", "https://other.example.com/");
        c.stub_of = a.stub_of.clone();
        let feed_quotes = vec![
            ("https://lin.example.org/".to_string(), "A".to_string()),
            ("https://lin.example.org/".to_string(), "Z".to_string()),
        ];
        let got = connections(O, &[a, b, c], &feed_quotes);
        let labels: Vec<(String, String)> =
            got.iter().map(|c| (c.origin.clone(), c.label())).collect();
        assert_eq!(
            labels,
            [
                ("https://ada.example.net/".into(), "stubbed 2 posts".into()),
                ("https://lin.example.org/".into(), "quoted 2 times".into()),
                ("https://rue.example.com/".into(), "forked 1 pin".into()),
            ]
        );
    }

    #[test]
    fn own_connections_come_from_published_posts() {
        use crate::model::{Kind, LocalId, RemoteRef, Status};
        let item = |id: &str, status: Status, md: &str| Item {
            local_id: LocalId(id.into()),
            server_id: None,
            kind: Kind::Thread,
            status,
            version: 1,
            dirty: false,
            content_md: md.into(),
            created: String::new(),
            updated: String::new(),
            permalink: None,
            stub_of: None,
            forked_from: None,
            show_responses: false,
            pending_sync: false,
            conflict: false,
        };
        let q = "01j9t4r7c8m2q5v6w3x8y9z0ab";
        let mut a = item("a", Status::Public, &format!("Quote\n\n![[{q}]]\n"));
        a.stub_of = Some(RemoteRef {
            origin: "https://ada.example.net/".into(),
            id: q.into(),
            version: 1,
        });
        let mut b = item("b", Status::Public, "fork");
        b.forked_from = Some(RemoteRef {
            origin: "https://rue.example.com/".into(),
            id: "R".into(),
            version: 3,
        });
        let mut c = item("c", Status::Draft, &format!("![[{q}]]"));
        c.stub_of = a.stub_of.clone();
        let mut ada = held(q, "https://ada.example.net/");
        ada.kind = crate::model::Kind::Fragment;
        let got = own_connections("https://blyg.example.com/", &[a, b, c], &[ada]);
        let labels: Vec<String> = got
            .iter()
            .map(|c| format!("{} {}", c.origin, c.label()))
            .collect();
        assert_eq!(
            labels,
            [
                "https://ada.example.net/ quoted once",
                "https://ada.example.net/ stubbed 1 post",
                "https://rue.example.com/ forked 1 pin"
            ]
        );
    }

    #[test]
    fn posts_are_titled_from_the_feed_or_held_items() {
        let ix = parse_index(&json!({ "items": [
            { "id": "A", "kind": "thread", "updated": "2030-01-03", "version": 2 },
            { "id": "B", "kind": "fragment", "updated": "2030-01-02", "version": 1 },
            { "id": "C", "kind": "withdrawn", "updated": "2030-01-01", "version": 4 },
            { "id": "D", "kind": "fragment", "updated": "2029-01-01", "version": 1 }
        ]}));
        let feed = Feed {
            items: vec![FeedItem {
                title: Some("Title from the feed".into()),
                blyg_id: Some("A".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let posts = blyg_posts(O, &ix, Some(&feed), &[held("B", O)], &|id| id == "B");
        assert_eq!(posts[0].title.as_deref(), Some("Title from the feed"));
        assert_eq!(posts[0].url.as_deref(), Some("https://jd.example.org/t/A/"));
        assert_eq!(posts[1].title.as_deref(), Some("Hello"));
        assert!(posts[1].pinned);
        assert_eq!(posts[2].title, None, "withdrawn: no title");
        assert_eq!(posts[2].url, None);
        assert_eq!(posts[3].title, None, "nothing held says");
        // No index: the feed alone.
        let f = blyg_posts(O, &[], Some(&feed), &[], &|_| false);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn origins_normalize() {
        assert_eq!(
            normalize_origin("HTTPS://Ada.Example.net/blyg?x=1#y").as_deref(),
            Some("https://ada.example.net/blyg/")
        );
        assert!(same_origin(
            "https://ada.example.net",
            "https://ada.example.net/"
        ));
        assert!(under_origin(
            "https://ada.example.net/blyg/t/01J/",
            "https://ada.example.net/blyg"
        ));
        assert!(!under_origin(
            "https://ada.example.net/other",
            "https://ada.example.net/blyg/"
        ));
        assert_eq!(
            clean_url("ada.example.net").as_deref(),
            Some("https://ada.example.net/")
        );
        assert_eq!(clean_url("ftp://x.example.com"), None);
        assert_eq!(normalize_origin("not a url"), None);
    }

    #[test]
    fn transclusion_ids_follow_the_grammar() {
        let md = "hi\n![[01j9t4r7c8m2q5v6w3x8y9z0ab]]\n  ![[01J9T4R7C8M2Q5V6W3X8Y9Z0AB]]  \ninline ![[01j9t4r7c8m2q5v6w3x8y9z0ab]]\n![[short]]";
        assert_eq!(transclusion_ids(md).len(), 2);
    }
}
