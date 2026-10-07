//! @-mention autocomplete, the pure part: when typing `@` opens the popup,
//! which blygs it offers, how they rank, and the link ⏎ inserts.
//!
//! There's no identity layer (docs/SPEC.md rule 6): the origin is the name,
//! so a mention is just a plain Markdown link to the blyg,
//! `[Display Name](https://blyg.example.org/)`. A plain link notifies nobody.

use std::collections::HashMap;
use std::ops::Range;

use blyg_core::profile::{Profile, normalize_origin};
use blyg_core::{Item, ReadingItem, Subscription, SubscriptionKind};
use chrono::{DateTime, Utc};

use super::text::{in_code_fence, in_inline_code};
use crate::vm;

/// How many suggestions the popup shows.
pub const MAX_SHOWN: usize = 8;
/// A query longer than this isn't a mention any more.
const MAX_QUERY_CHARS: usize = 48;

/// One blyg the user knows.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The link target: the blyg's origin (or a feed's site), with a trailing slash.
    pub url: String,
    pub host: String,
    /// The author's name, when anything we hold says it.
    pub name: Option<String>,
    /// The site title.
    pub title: Option<String>,
    /// Higher is better: subscribed, read lately, quoted.
    pub rank: i64,
    /// Why it's offered, for the popup's second line.
    pub why: &'static str,
}

impl Candidate {
    /// What the link says: the author name, else the site title, else the host.
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .or(self.title.as_deref())
            .unwrap_or(&self.host)
    }

    /// `[label](url)`, escaped like a pasted link.
    pub fn link(&self) -> String {
        format!(
            "[{}]({})",
            vm::escape_link_text(self.label()),
            vm::link_destination(&self.url)
        )
    }
}

/// Everything held locally that says who the user knows.
#[derive(Default)]
pub struct Sources<'a> {
    pub subscriptions: &'a [Subscription],
    pub reading: &'a [ReadingItem],
    pub profiles: &'a [Profile],
    /// The user's own posts (for "quoted" and "mentioned before").
    pub items: &'a [Item],
    /// The user's own blyg, never offered.
    pub own_origin: Option<&'a str>,
}

#[derive(Default)]
struct Acc {
    url: String,
    host: String,
    name: Option<String>,
    title: Option<String>,
    rank: i64,
    why: &'static str,
    why_rank: u8,
}

impl Acc {
    fn because(&mut self, why: &'static str, strength: u8) {
        if strength > self.why_rank {
            self.why = why;
            self.why_rank = strength;
        }
    }
}

fn clean(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Every blyg worth offering, deduplicated by origin and ranked (without a
/// query). Built once when the popup opens, so it may do real work.
pub fn gather(src: &Sources, now: DateTime<Utc>) -> Vec<Candidate> {
    let own = src.own_origin.and_then(normalize_origin);
    let mut by_url: HashMap<String, Acc> = HashMap::new();
    let own = own.as_deref();
    macro_rules! entry {
        ($url:expr) => {
            entry_in(&mut by_url, own, $url)
        };
    }

    for s in src.subscriptions {
        if let Some(a) = entry!(&s.origin) {
            a.rank += match s.kind {
                SubscriptionKind::Blyg => 300,
                SubscriptionKind::Rss => 250,
            };
            if s.in_blogroll {
                a.rank += 50;
                a.because("in your blogroll", 3);
            } else {
                a.because("subscribed", 2);
            }
            if a.title.is_none() {
                a.title = clean(Some(&s.title));
            }
        }
    }

    // Reading: authors seen, and how recently their posts were read.
    let remote_origin: HashMap<&str, &str> = src
        .reading
        .iter()
        .map(|r| (r.remote_id.as_str(), r.origin.as_str()))
        .collect();
    for r in src.reading {
        let Some(a) = entry!(&r.origin) else { continue };
        if let Some(name) = clean(r.author.as_ref().and_then(|x| x.name.as_deref())) {
            a.name.get_or_insert(name);
        }
        if a.title.is_none() {
            a.title = clean(Some(&r.subscription_title));
        }
        a.rank += 5;
        a.because("in your reading", 1);
        if r.read_version.is_some() {
            let when = r
                .updated
                .as_deref()
                .and_then(parse_time)
                .or_else(|| parse_time(&r.observed_at));
            let days = when.map_or(30, |t| (now - t).num_days().clamp(0, 30));
            // Recently read counts most; keep the best (most recent) read.
            let bonus = 200 - days * 6;
            a.rank += 20 + bonus / 4;
        }
    }

    // Profiles already fetched: names, titles, and their blogrolls.
    for p in src.profiles {
        if let Some(a) = entry!(&p.origin) {
            if let Some(n) = clean(p.name.as_deref()) {
                a.name = Some(n);
            }
            if let Some(t) = clean(p.title.as_deref()) {
                a.title = Some(t);
            }
            a.rank += 30;
            a.because("profile seen", 1);
        }
        for b in &p.blogroll {
            let Some(url) = b.url() else { continue };
            if let Some(a) = entry!(url) {
                if a.title.is_none() {
                    a.title = clean(Some(&b.title));
                }
                a.rank += 20;
                a.because("in a blogroll you've seen", 0);
            }
        }
    }

    // What the user's own posts quote, reply to, fork, or already link to.
    for it in src.items {
        for id in blyg_core::profile::transclusion_ids(&it.content_md) {
            if let Some(o) = remote_origin.get(id.as_str())
                && let Some(a) = entry!(o)
            {
                a.rank += 150;
                a.because("you quoted them", 4);
            }
        }
        for r in [&it.stub_of, &it.forked_from].into_iter().flatten() {
            if let Some(a) = entry!(&r.origin) {
                a.rank += 150;
                a.because("you replied to them", 4);
            }
        }
    }
    let linked = linked_hosts(src.items);
    let mut out: Vec<Candidate> = by_url
        .into_values()
        .map(|mut a| {
            if let Some(n) = linked.get(&a.host) {
                a.rank += 60 * (*n).min(3) as i64;
                a.because("you linked to them", 4);
            }
            Candidate {
                url: a.url,
                host: a.host,
                name: a.name,
                title: a.title,
                rank: a.rank,
                why: if a.why.is_empty() { "known" } else { a.why },
            }
        })
        .collect();
    out.sort_by(|a, b| b.rank.cmp(&a.rank).then_with(|| a.label().cmp(b.label())));
    out
}

fn entry_in<'m>(
    map: &'m mut HashMap<String, Acc>,
    own: Option<&str>,
    url: &str,
) -> Option<&'m mut Acc> {
    let key = normalize_origin(url)?;
    if own == Some(key.as_str()) {
        return None;
    }
    let host = vm::url_host(&key).filter(|h| !h.is_empty())?;
    Some(map.entry(key.clone()).or_insert_with(|| Acc {
        url: key,
        host,
        ..Default::default()
    }))
}

/// Hosts the user's posts link to, with how many posts do.
fn linked_hosts(items: &[Item]) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::new();
    for it in items {
        let mut seen: Vec<String> = Vec::new();
        let md = &it.content_md;
        let mut i = 0;
        while let Some(rel) = md[i..].find("http") {
            let start = i + rel;
            let rest = &md[start..];
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | ']' | '"'))
                .unwrap_or(rest.len());
            if let Some(h) = vm::url_host(&rest[..end]).filter(|h| !h.is_empty())
                && !seen.contains(&h)
            {
                seen.push(h);
            }
            i = start + end.max(4);
        }
        for h in seen {
            *out.entry(h).or_default() += 1;
        }
    }
    out
}

/// How well `query` matches a candidate: `None` when it doesn't. Every
/// word of the query must start a word of the name, the title or the host
/// (strong), or, from three letters on, at least appear in one (weak), so
/// `@a` doesn't offer everyone on `example.com`.
fn match_score(c: &Candidate, query: &str) -> Option<u32> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Some(0);
    }
    let fields: Vec<String> = [c.name.as_deref(), c.title.as_deref(), Some(c.host.as_str())]
        .into_iter()
        .flatten()
        .map(str::to_lowercase)
        .collect();
    let starts = |w: &str| {
        fields.iter().any(|f| {
            f.split(|ch: char| !ch.is_alphanumeric())
                .any(|part| part.starts_with(w))
        })
    };
    let mut score = 0;
    for w in &words {
        if starts(w) {
            score += 3;
        } else if w.chars().count() >= 3 && fields.iter().any(|f| f.contains(w.as_str())) {
            score += 1;
        } else {
            return None;
        }
    }
    // The label itself starting with the query beats a match elsewhere.
    if c.label()
        .to_lowercase()
        .starts_with(&query.trim().to_lowercase())
    {
        score += 2;
    }
    Some(score)
}

/// The candidates matching `query`, best first, at most [`MAX_SHOWN`].
pub fn filter(cands: &[Candidate], query: &str) -> Vec<Candidate> {
    let mut hits: Vec<(u32, &Candidate)> = cands
        .iter()
        .filter_map(|c| match_score(c, query).map(|s| (s, c)))
        .collect();
    hits.sort_by(|(sa, a), (sb, b)| {
        sb.cmp(sa)
            .then_with(|| b.rank.cmp(&a.rank))
            .then_with(|| a.label().cmp(b.label()))
    });
    hits.into_iter()
        .take(MAX_SHOWN)
        .map(|(_, c)| c.clone())
        .collect()
}

/// If `old -> new` typed a single `@` at `cursor - 1` that starts a word
/// (after the start of the text, a space or an opening bracket or quote, and
/// before a space, the end or closing punctuation), outside code, the byte
/// offset of that `@`. A paste, or an `@` inside a word (an email address),
/// is `None`.
pub fn trigger(old: &str, new: &str, cursor: usize) -> Option<usize> {
    if new.len() != old.len() + 1 || cursor == 0 || cursor > new.len() {
        return None;
    }
    if !new.is_char_boundary(cursor) || !new[..cursor].ends_with('@') {
        return None;
    }
    let at = cursor - 1;
    if new[..at] != old[..at] || new[cursor..] != old[at..] {
        return None;
    }
    let before_ok = new[..at]
        .chars()
        .next_back()
        .is_none_or(|c| c.is_whitespace() || "([{\"'“‘«—–".contains(c));
    let after_ok = new[cursor..]
        .chars()
        .next()
        .is_none_or(|c| c.is_whitespace() || ")]}\"'”’».,;:!?".contains(c));
    if !before_ok || !after_ok {
        return None;
    }
    let line_start = new[..at].rfind('\n').map_or(0, |i| i + 1);
    if in_code_fence(&new[..line_start]) || in_inline_code(&new[line_start..at]) {
        return None;
    }
    Some(at)
}

/// The query typed after the `@` at `at`, while the caret is still in that
/// mention: on the same line, after the `@`, not too long, and not ending a
/// word with two spaces. `None` means the popup should close.
pub fn query(text: &str, at: usize, cursor: usize) -> Option<&str> {
    if text.as_bytes().get(at) != Some(&b'@') || cursor <= at || cursor > text.len() {
        return None;
    }
    let q = text.get(at + 1..cursor)?;
    if q.contains('\n')
        || q.starts_with(char::is_whitespace)
        || q.contains("  ")
        || q.chars().count() > MAX_QUERY_CHARS
    {
        return None;
    }
    Some(q)
}

/// Replace the typed `@query` (`at..cursor`) with the link. Returns the new
/// text and the caret (just after the link).
pub fn insert(text: &str, at: usize, cursor: usize, c: &Candidate) -> (String, usize) {
    let range: Range<usize> = at.min(text.len())..cursor.min(text.len());
    let link = c.link();
    let mut out = String::with_capacity(text.len() + link.len());
    out.push_str(&text[..range.start]);
    out.push_str(&link);
    out.push_str(&text[range.end..]);
    (out, range.start + link.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use blyg_core::profile::{BlogrollEntry, ProfileKind};
    use blyg_core::{Author, Kind, LocalId, RemoteRef, Status};

    fn typed(before: &str, after: &str) -> Option<usize> {
        let old = format!("{}{after}", &before[..before.len() - 1]);
        let new = format!("{before}{after}");
        trigger(&old, &new, before.len())
    }

    #[test]
    fn trigger_fires_at_the_start_of_a_word() {
        assert_eq!(typed("@", ""), Some(0));
        assert_eq!(typed("Thanks @", ""), Some(7));
        assert_eq!(typed("One.\n@", "\nTwo."), Some(5));
        assert_eq!(typed("(@", ")"), Some(1));
        assert_eq!(typed("“@", ""), Some(3));
        assert_eq!(typed("Hi @", " there"), Some(3));
    }

    #[test]
    fn trigger_ignores_emails_mid_word_and_pastes() {
        assert_eq!(typed("me@", ""), None);
        assert_eq!(typed("Hi @", "ada"), None, "an @ glued to a word");
        assert_eq!(trigger("Hi ", "Hi @a", 5), None, "two characters at once");
        assert_eq!(trigger("", "@", 0), None);
    }

    #[test]
    fn trigger_ignores_code() {
        assert_eq!(typed("```\n@", "\n```"), None);
        assert_eq!(typed("see `x @", "`"), None);
        assert_eq!(typed("```\ncode\n```\n@", ""), Some(13));
        assert_eq!(typed("`a` and @", ""), Some(8));
    }

    #[test]
    fn query_follows_the_caret() {
        let t = "Thanks @ada lo";
        assert_eq!(query(t, 7, 7), None, "caret before the @");
        assert_eq!(query(t, 7, 8), Some(""));
        assert_eq!(query(t, 7, 11), Some("ada"));
        assert_eq!(query(t, 7, 14), Some("ada lo"));
        assert_eq!(query("@ada\nx", 0, 6), None, "a new line ends it");
        assert_eq!(query("@ada  x", 0, 7), None, "two spaces end it");
        assert_eq!(query("@ x", 0, 3), None, "a space right after @");
        assert_eq!(query("x", 0, 1), None, "the @ is gone");
    }

    fn cand(name: Option<&str>, title: Option<&str>, url: &str, rank: i64) -> Candidate {
        Candidate {
            url: url.into(),
            host: vm::url_host(url).unwrap(),
            name: name.map(Into::into),
            title: title.map(Into::into),
            rank,
            why: "subscribed",
        }
    }

    #[test]
    fn link_uses_name_then_title_then_host_and_escapes() {
        let c = cand(
            Some("Ada [A.]"),
            Some("Tides"),
            "https://ada.blyg.example.org/",
            0,
        );
        assert_eq!(c.link(), "[Ada \\[A.\\]](https://ada.blyg.example.org/)");
        let c = cand(None, Some("Tides"), "https://tides.example.org/", 0);
        assert_eq!(c.link(), "[Tides](https://tides.example.org/)");
        let c = cand(None, None, "https://x.example.net/a(b)/", 0);
        assert_eq!(c.link(), "[x.example.net](<https://x.example.net/a(b)/>)");
    }

    #[test]
    fn insert_replaces_the_typed_query_only() {
        let c = cand(Some("Ada"), None, "https://ada.blyg.example.org/", 0);
        let (t, caret) = insert("Thanks @ad, see you", 7, 10, &c);
        assert_eq!(t, "Thanks [Ada](https://ada.blyg.example.org/), see you");
        assert_eq!(&t[..caret], "Thanks [Ada](https://ada.blyg.example.org/)");
    }

    #[test]
    fn filter_matches_name_title_and_host_prefixes() {
        let cs = vec![
            cand(
                Some("Ada"),
                Some("Tide notes"),
                "https://ada.blyg.example.org/",
                100,
            ),
            cand(
                Some("Rue"),
                Some("Small trusts"),
                "https://rue.blyg.example.org/",
                300,
            ),
            cand(
                None,
                Some("Field notes"),
                "https://fieldnotes.example.com/",
                50,
            ),
        ];
        let labels = |q: &str| {
            filter(&cs, q)
                .iter()
                .map(|c| c.label().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(""), ["Rue", "Ada", "Field notes"], "rank order");
        assert_eq!(labels("ad"), ["Ada"]);
        assert_eq!(labels("TIDE"), ["Ada"], "by title, any case");
        assert_eq!(labels("fieldn"), ["Field notes"], "by host");
        assert_eq!(labels("notes"), ["Ada", "Field notes"], "rank breaks ties");
        assert_eq!(labels("field no"), ["Field notes"], "several words");
        assert!(labels("zebra").is_empty());
        assert_eq!(
            labels("u"),
            Vec::<String>::new(),
            "Rue: no substring match under 3 letters"
        );
        assert_eq!(
            labels("otes"),
            ["Ada", "Field notes"],
            "a substring from 3 on"
        );
        // A label-prefix match outranks a better-ranked match elsewhere.
        assert_eq!(labels("small")[0], "Rue");
    }

    fn sub(origin: &str, title: &str, kind: SubscriptionKind, blogroll: bool) -> Subscription {
        Subscription {
            id: format!("sub-{title}"),
            kind,
            origin: origin.into(),
            feed_url: format!("{origin}feed.json"),
            title: title.into(),
            status: "active".into(),
            in_blogroll: blogroll,
            title_follows_source: true,
        }
    }

    fn reading(origin: &str, id: &str, author: &str, read: bool, at: &str) -> ReadingItem {
        ReadingItem {
            subscription_id: "s".into(),
            remote_id: id.into(),
            subscription_title: String::new(),
            origin: origin.into(),
            kind: Kind::Fragment,
            state: "current".into(),
            version: 1,
            created: None,
            updated: Some(at.into()),
            observed_at: at.into(),
            content_md: String::new(),
            content_html: String::new(),
            author: Some(Author {
                name: Some(author.into()),
                url: None,
            }),
            page: None,
            thumb: None,
            hoppers: vec![],
            pinned_version_retained: None,
            read_version: read.then_some(1),
            stub_of: None,
            forked_from: None,
            transclusions: vec![],
            lineage_known: false,
        }
    }

    fn own(md: &str) -> Item {
        Item {
            local_id: LocalId("l".into()),
            server_id: None,
            kind: Kind::Thread,
            status: Status::Draft,
            version: 0,
            dirty: false,
            content_md: md.into(),
            created: String::new(),
            updated: String::new(),
            permalink: None,
            stub_of: None,
            forked_from: None,
            show_responses: false,
            responses_mode: None,
            pending_sync: false,
            conflict: false,
            highlight: false,
            highlight_mode: None,
        }
    }

    fn profile(origin: &str, name: &str, roll: Vec<BlogrollEntry>) -> Profile {
        Profile {
            kind: ProfileKind::Blyg,
            origin: origin.into(),
            feed_url: None,
            name: Some(name.into()),
            title: Some(format!("{name}'s blyg")),
            bio: None,
            avatar: None,
            links: vec![],
            has_blogroll: !roll.is_empty(),
            blogroll: roll,
            posts: vec![],
            connections: vec![],
            feed_quotes: vec![],
            own: false,
            fetched_at: 0,
            stale: false,
        }
    }

    const ADA: &str = "https://ada.blyg.example.org/";
    const RUE: &str = "https://rue.blyg.example.org/";
    const LIN: &str = "https://lin.blyg.example.org/";
    const OMAR: &str = "https://omar.example.com/";
    const ME: &str = "https://blyg.example.com/";
    const LIN_POST: &str = "01K2LIN0POST0000000000001X";

    #[test]
    fn gather_merges_sources_by_origin_and_skips_your_own() {
        let now = Utc::now();
        let t = |d: i64| (now - chrono::Duration::days(d)).to_rfc3339();
        let subs = vec![
            sub(ADA, "Tide notes", SubscriptionKind::Blyg, true),
            sub(OMAR, "Omar's notes", SubscriptionKind::Rss, false),
            sub(ME, "Me", SubscriptionKind::Blyg, false),
        ];
        let rd = vec![
            reading("https://ada.blyg.example.org", "a1", "Ada", true, &t(1)),
            reading(LIN, "l1", "Lin", false, &t(2)),
        ];
        let profiles = vec![profile(
            RUE,
            "Rue",
            vec![BlogrollEntry {
                title: "Field notes".into(),
                xml_url: Some("https://fieldnotes.example.com/rss".into()),
                html_url: Some("https://fieldnotes.example.com/".into()),
            }],
        )];
        let src = Sources {
            subscriptions: &subs,
            reading: &rd,
            profiles: &profiles,
            items: &[],
            own_origin: Some(ME),
        };
        let cs = gather(&src, now);
        let urls: Vec<&str> = cs.iter().map(|c| c.url.as_str()).collect();
        assert!(!urls.contains(&ME), "never your own blyg: {urls:?}");
        assert_eq!(
            cs.iter().filter(|c| c.url == ADA).count(),
            1,
            "one entry per origin"
        );
        let ada = cs.iter().find(|c| c.url == ADA).unwrap();
        assert_eq!(ada.name.as_deref(), Some("Ada"), "author from reading");
        assert_eq!(ada.title.as_deref(), Some("Tide notes"));
        assert_eq!(ada.why, "in your blogroll");
        let fieldnotes = cs.iter().find(|c| c.host == "fieldnotes.example.com");
        assert_eq!(
            fieldnotes.unwrap().label(),
            "Field notes",
            "from a blogroll"
        );
        assert_eq!(cs[0].url, ADA, "subscribed, blogrolled and read: first");
        assert!(urls.contains(&LIN) && urls.contains(&RUE) && urls.contains(&OMAR));
    }

    #[test]
    fn quoting_and_recent_reading_rank_higher() {
        let now = Utc::now();
        let t = |d: i64| (now - chrono::Duration::days(d)).to_rfc3339();
        // Two equal subscriptions; reading and quoting decide.
        let subs = vec![
            sub(ADA, "Ada", SubscriptionKind::Blyg, false),
            sub(RUE, "Rue", SubscriptionKind::Blyg, false),
            sub(LIN, "Lin", SubscriptionKind::Blyg, false),
        ];
        let rd = vec![
            reading(ADA, "a1", "Ada", true, &t(20)),
            reading(RUE, "r1", "Rue", true, &t(0)),
            reading(LIN, LIN_POST, "Lin", false, &t(0)),
        ];
        let base = Sources {
            subscriptions: &subs,
            reading: &rd,
            ..Default::default()
        };
        let order = |cs: Vec<Candidate>| cs.into_iter().map(|c| c.url).collect::<Vec<_>>();
        assert_eq!(
            order(gather(&base, now)),
            [RUE, ADA, LIN],
            "read today > read weeks ago > unread"
        );
        let items = vec![own(&format!("Replying to this:\n\n![[{LIN_POST}]]\n"))];
        let quoted = Sources {
            items: &items,
            ..base
        };
        let cs = gather(&quoted, now);
        assert_eq!(cs[0].url, LIN, "quoting puts Lin first");
        assert_eq!(cs[0].why, "you quoted them");

        let mut reply = own("A reply.");
        reply.stub_of = Some(RemoteRef {
            origin: ADA.into(),
            id: "a1".into(),
            version: 1,
        });
        let linked = own("As [Rue](https://rue.blyg.example.org/f/x) said.");
        let items = vec![reply, linked];
        let src = Sources {
            subscriptions: &subs,
            reading: &[],
            items: &items,
            ..Default::default()
        };
        let cs = gather(&src, now);
        assert_eq!(order(cs.clone())[2], LIN);
        assert_eq!(
            cs.iter().find(|c| c.url == RUE).unwrap().why,
            "you linked to them"
        );
    }
}
