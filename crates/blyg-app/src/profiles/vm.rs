//! What the profile sheet shows, as plain data (no GPUI): the tabs, the rows
//! of each tab, the header's labels, the lineage line. The rules from
//! docs/SPEC.md § Profiles live here so they can be tested directly:
//! lists only, never a follower or subscriber count.

use blyg_core::profile::{Connection, same_origin, under_origin};
use blyg_core::{Profile, ProfileKind, ReadingItem, Subscription, SubscriptionKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Blogroll,
    Posts,
    Connections,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Blogroll => "Blogroll",
            Tab::Posts => "Posts",
            Tab::Connections => "Connections",
        }
    }

    /// The tabs a profile has: a plain feed has only its posts.
    pub fn all_for(p: &Profile) -> Vec<Tab> {
        match p.kind {
            ProfileKind::Blyg => vec![Tab::Blogroll, Tab::Posts, Tab::Connections],
            ProfileKind::Feed => vec![Tab::Posts],
        }
    }
}

/// One row of a tab.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub title: String,
    pub sub: String,
    /// `blyg` / `rss`, when we know.
    pub tag: Option<&'static str>,
    /// "Profile" opens this.
    pub profile_url: Option<String>,
    /// "Follow" subscribes to this; `following` says whether we already do.
    pub follow_url: Option<String>,
    pub following: bool,
    /// Posts: read it on the web.
    pub open_url: Option<String>,
    /// Your own blogroll: `(subscription id, in the blogroll)`.
    pub blogroll_toggle: Option<(String, bool)>,
}

impl Row {
    fn new(title: impl Into<String>, sub: impl Into<String>) -> Row {
        Row {
            title: title.into(),
            sub: sub.into(),
            tag: None,
            profile_url: None,
            follow_url: None,
            following: false,
            open_url: None,
            blogroll_toggle: None,
        }
    }
}

/// The subscription that already follows `url` (its origin, a page under
/// it, or its feed), if any.
pub fn followed<'a>(url: &str, subs: &'a [Subscription]) -> Option<&'a Subscription> {
    subs.iter().find(|s| {
        same_origin(url, &s.feed_url)
            || same_origin(url, &s.origin)
            || (s.kind == SubscriptionKind::Blyg && under_origin(url, &s.origin))
    })
}

/// The subscription that follows this profile.
pub fn followed_profile<'a>(p: &Profile, subs: &'a [Subscription]) -> Option<&'a Subscription> {
    followed(&p.origin, subs).or_else(|| p.feed_url.as_deref().and_then(|f| followed(f, subs)))
}

fn kind_tag(k: SubscriptionKind) -> &'static str {
    match k {
        SubscriptionKind::Blyg => "blyg",
        SubscriptionKind::Rss => "rss",
    }
}

/// `https://ada.example.net/blyg/` → `ada.example.net/blyg`.
pub fn short(url: &str) -> String {
    let s = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    s.to_string()
}

/// A served date (RFC 3339, or an RSS RFC 2822 date) as "3d" / "Jan 4".
pub fn when(date: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    if let Ok(t) = chrono::DateTime::parse_from_rfc2822(date) {
        return crate::vm::relative_time(&t.to_rfc3339(), now);
    }
    crate::vm::relative_time(date, now)
}

/// The rows of `tab`. `subs` are your subscriptions (for Following and
/// your own blogroll's toggles).
pub fn rows(
    p: &Profile,
    tab: Tab,
    subs: &[Subscription],
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Row> {
    match tab {
        Tab::Blogroll if p.own => subs
            .iter()
            .map(|s| {
                let mut r = Row::new(s.title.clone(), short(&s.origin));
                r.tag = Some(kind_tag(s.kind));
                r.profile_url = Some(s.origin.clone());
                r.blogroll_toggle = Some((s.id.clone(), s.in_blogroll));
                r
            })
            .collect(),
        Tab::Blogroll => p
            .blogroll
            .iter()
            .filter_map(|e| {
                let url = e.url()?.to_string();
                let sub_hit = followed(&url, subs)
                    .or_else(|| e.xml_url.as_deref().and_then(|x| followed(x, subs)));
                let mut r = Row::new(e.title.clone(), short(&url));
                r.tag = sub_hit.map(|s| kind_tag(s.kind));
                r.profile_url = Some(url.clone());
                r.following = sub_hit.is_some();
                r.follow_url = Some(url);
                Some(r)
            })
            .collect(),
        Tab::Posts => p
            .posts
            .iter()
            .map(|x| {
                let withdrawn = x.kind == "withdrawn";
                let title = match &x.title {
                    Some(t) => t.clone(),
                    None if withdrawn => "Withdrawn by the author".into(),
                    None => "Untitled".into(),
                };
                let mut meta: Vec<String> = vec![];
                match x.kind.as_str() {
                    "fragment" => {
                        meta.push(crate::vm::kind_label(blyg_core::Kind::Fragment).into())
                    }
                    "thread" => meta.push(crate::vm::kind_label(blyg_core::Kind::Thread).into()),
                    "rss" | "post" | "item" => {}
                    k => meta.push(k.to_string()),
                }
                if let Some(v) = x.version {
                    meta.push(if x.pinned {
                        format!("v{v} 📌")
                    } else {
                        format!("v{v}")
                    });
                }
                if let Some(d) = x
                    .date
                    .as_deref()
                    .map(|d| when(d, now))
                    .filter(|d| !d.is_empty())
                {
                    meta.push(d);
                }
                let mut r = Row::new(title, meta.join(" · "));
                r.open_url = x.url.clone();
                r
            })
            .collect(),
        Tab::Connections => p
            .connections
            .iter()
            .map(|c: &Connection| {
                let hit = followed(&c.origin, subs);
                let mut r = Row::new(short(&c.origin), c.label());
                r.tag = hit.map(|s| kind_tag(s.kind));
                r.profile_url = Some(c.origin.clone());
                r.follow_url = Some(c.origin.clone());
                r.following = hit.is_some();
                r
            })
            .collect(),
    }
}

/// What a tab says when it has no rows.
pub fn empty_note(p: &Profile, tab: Tab) -> &'static str {
    match tab {
        Tab::Blogroll if p.own => "You don't follow anyone yet.",
        Tab::Blogroll if !p.has_blogroll => "No public blogroll.",
        Tab::Blogroll => "Their blogroll is empty.",
        Tab::Posts => "No posts to show.",
        Tab::Connections => "No quotes, stubs or forks in the posts we hold.",
    }
}

/// The header: display name, the origin line, the buttons. Never a count.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub name: String,
    pub origin_line: String,
    pub initial: String,
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Follow,
    Following,
    OpenSite,
    AddToBlogroll,
    InBlogroll,
    EditSite,
    Refresh,
}

impl Button {
    pub fn label(self) -> &'static str {
        match self {
            Button::Follow => "Follow",
            Button::Following => "Following ✓",
            Button::OpenSite => "Open site",
            Button::AddToBlogroll => "Add to my blogroll",
            Button::InBlogroll => "In my blogroll ✓",
            Button::EditSite => "Edit site settings…",
            Button::Refresh => "↻",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Button::Follow | Button::Following => "pf-follow",
            Button::OpenSite => "pf-open-site",
            Button::AddToBlogroll | Button::InBlogroll => "pf-blogroll",
            Button::EditSite => "pf-edit-site",
            Button::Refresh => "pf-refresh",
        }
    }
}

pub fn header(p: &Profile, subs: &[Subscription]) -> Header {
    let host = short(&p.origin);
    let name = p
        .name
        .clone()
        .or_else(|| p.title.clone())
        .unwrap_or_else(|| host.clone());
    let kind = match p.kind {
        ProfileKind::Blyg => "blyg",
        ProfileKind::Feed => "feed",
    };
    let origin_line = if p.own {
        format!("{host} · your blyg")
    } else {
        format!("{host} · {kind}")
    };
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "·".into());
    let buttons = if p.own {
        vec![Button::EditSite, Button::OpenSite, Button::Refresh]
    } else {
        let sub = followed_profile(p, subs);
        vec![
            if sub.is_some() {
                Button::Following
            } else {
                Button::Follow
            },
            Button::OpenSite,
            if sub.is_some_and(|s| s.in_blogroll) {
                Button::InBlogroll
            } else {
                Button::AddToBlogroll
            },
            Button::Refresh,
        ]
    };
    Header {
        name,
        origin_line,
        initial,
        buttons,
    }
}

/// The note under the lists: where it came from, and that no token went.
pub fn source_note(p: &Profile, now: chrono::DateTime<chrono::Utc>) -> String {
    let host = short(&p.origin);
    let from = match p.kind {
        ProfileKind::Blyg if p.has_blogroll => {
            format!("From {host}/blyg.json, blogroll.opml and items/index.json")
        }
        ProfileKind::Blyg => format!("From {host}/blyg.json and items/index.json"),
        ProfileKind::Feed => format!("From {}", p.feed_url.as_deref().map(short).unwrap_or(host)),
    };
    let at = chrono::DateTime::from_timestamp_millis(p.fetched_at)
        .map(|t| crate::vm::relative_time(&t.to_rfc3339(), now))
        .filter(|s| !s.is_empty());
    let mut s = format!("{from}, fetched without your token. Cached for offline use.");
    if let Some(at) = at {
        s.push_str(&format!(" Fetched {at}."));
    }
    if p.stale {
        s.push_str(" Couldn't reach it just now: this is the saved copy.");
    }
    s
}

/// The lineage line under a reading item's header: `↳ stub of …` and
/// `⑂ forked from … vN 📌`, each with the origin (or page) it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lineage {
    pub id: &'static str,
    pub lead: &'static str,
    pub link: String,
    pub url: String,
}

/// `item` is a post's lineage: the reading item's own fields, filled from
/// its item document where the owner API left them out (see
/// [`blyg_core::Lineage::or`]).
pub fn lineage(item: &blyg_core::Lineage, held: &[ReadingItem]) -> Vec<Lineage> {
    let mut out = vec![];
    if let Some(s) = &item.stub_of
        && let Some(target) = s.target()
    {
        // The stubbed post's first line, when we hold it.
        let title = s.id.as_deref().and_then(|id| {
            held.iter()
                .find(|r| r.remote_id == id && same_origin(&r.origin, target))
                .and_then(|r| blyg_core::profile::first_line(&r.content_md))
        });
        let host = short(target);
        out.push(Lineage {
            id: "pf-lineage-stub",
            lead: "↳ stub of",
            link: match title {
                Some(t) => format!("{host} · “{}”", clip(&t, 48)),
                None => host,
            },
            url: target.to_string(),
        });
    }
    if let Some(f) = &item.forked_from {
        out.push(Lineage {
            id: "pf-lineage-fork",
            lead: "⑂ forked from",
            link: format!("{} v{} 📌", short(&f.origin), f.version),
            url: f.origin.clone(),
        });
    }
    out
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n - 1).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blyg_core::profile::Relation;
    use blyg_core::{BlogrollEntry, ProfilePost};

    fn sub(id: &str, origin: &str, blogroll: bool) -> Subscription {
        Subscription {
            id: id.into(),
            kind: SubscriptionKind::Blyg,
            origin: origin.into(),
            feed_url: format!("{origin}feed.xml"),
            title: id.into(),
            status: "active".into(),
            in_blogroll: blogroll,
        }
    }

    fn profile() -> Profile {
        Profile {
            kind: ProfileKind::Blyg,
            origin: "https://jd.example.org/".into(),
            feed_url: None,
            name: Some("JD".into()),
            title: None,
            bio: None,
            avatar: None,
            links: vec![],
            blogroll: vec![
                BlogrollEntry {
                    title: "Ada".into(),
                    xml_url: Some("https://ada.example.net/feed.xml".into()),
                    html_url: Some("https://ada.example.net/".into()),
                },
                BlogrollEntry {
                    title: "Tools".into(),
                    xml_url: Some("https://tools.example.com/feed.xml".into()),
                    html_url: None,
                },
            ],
            has_blogroll: true,
            posts: vec![ProfilePost {
                id: "A".into(),
                title: None,
                kind: "thread".into(),
                version: Some(3),
                pinned: true,
                date: None,
                url: None,
            }],
            connections: vec![Connection {
                origin: "https://rue.example.com/".into(),
                relation: Relation::Stubs,
                count: 2,
            }],
            feed_quotes: vec![],
            own: false,
            fetched_at: 0,
            stale: false,
        }
    }

    #[test]
    fn following_comes_from_your_subscriptions() {
        let subs = vec![sub("s1", "https://ada.example.net/", false)];
        let now = chrono::Utc::now();
        let r = rows(&profile(), Tab::Blogroll, &subs, now);
        assert!(r[0].following && !r[1].following);
        assert_eq!(r[0].tag, Some("blyg"));
        assert_eq!(
            r[1].profile_url.as_deref(),
            Some("https://tools.example.com/feed.xml")
        );
        let h = header(&profile(), &subs);
        assert_eq!(h.buttons[0], Button::Follow);
        let subs = vec![sub("s2", "https://jd.example.org/", true)];
        let h = header(&profile(), &subs);
        assert_eq!(h.buttons[0], Button::Following);
        assert_eq!(h.buttons[2], Button::InBlogroll);
        let p = rows(&profile(), Tab::Posts, &subs, now);
        assert_eq!(p[0].title, "Untitled");
        assert_eq!(p[0].sub, "≡ thread · v3 📌");
        let c = rows(&profile(), Tab::Connections, &subs, now);
        assert_eq!(c[0].sub, "stubbed 2 posts");
    }

    /// No follower, subscriber or reader counts: nothing in the header, the
    /// buttons or the tab labels carries a digit, for any follow state.
    #[test]
    fn no_digits_in_follower_like_places() {
        for subs in [vec![], vec![sub("s", "https://jd.example.org/", true)]] {
            for own in [false, true] {
                let mut p = profile();
                p.own = own;
                let h = header(&p, &subs);
                let mut texts = vec![h.name.clone(), h.origin_line.clone()];
                texts.extend(h.buttons.iter().map(|b| b.label().to_string()));
                texts.extend(Tab::all_for(&p).iter().map(|t| t.label().to_string()));
                for t in texts {
                    assert!(!t.chars().any(|c| c.is_ascii_digit()), "a count in {t:?}");
                }
            }
        }
    }

    #[test]
    fn own_blogroll_lists_every_subscription_with_a_toggle() {
        let mut p = profile();
        p.own = true;
        let subs = vec![
            sub("a", "https://ada.example.net/", true),
            sub("b", "https://rue.example.com/", false),
        ];
        let r = rows(&p, Tab::Blogroll, &subs, chrono::Utc::now());
        let toggles: Vec<_> = r.iter().map(|r| r.blogroll_toggle.clone()).collect();
        assert_eq!(
            toggles,
            [Some(("a".into(), true)), Some(("b".into(), false))]
        );
        assert_eq!(header(&p, &subs).buttons[0], Button::EditSite);
    }

    #[test]
    fn lineage_names_the_stubbed_and_forked_origins() {
        let item = ReadingItem {
            subscription_id: "s".into(),
            remote_id: "x".into(),
            subscription_title: "t".into(),
            origin: "https://jd.example.org/".into(),
            kind: blyg_core::Kind::Thread,
            state: "current".into(),
            version: 1,
            created: None,
            updated: None,
            observed_at: String::new(),
            content_md: String::new(),
            content_html: String::new(),
            author: None,
            page: None,
            thumb: None,
            hoppers: vec![],
            pinned_version_retained: None,
            read_version: None,
            stub_of: Some(blyg_core::StubOf {
                origin: Some("https://ada.example.net/".into()),
                id: Some("A".into()),
                version: Some(1),
                url: None,
            }),
            forked_from: Some(blyg_core::RemoteRef {
                origin: "https://rue.example.com/".into(),
                id: "R".into(),
                version: 3,
            }),
            transclusions: vec![],
        };
        let mut held = item.clone();
        held.origin = "https://ada.example.net/".into();
        held.remote_id = "A".into();
        held.content_md = "On first posts\n\nmore".into();
        let l = lineage(&item.lineage(), &[held]);
        assert_eq!(l[0].link, "ada.example.net · “On first posts”");
        assert_eq!(l[0].url, "https://ada.example.net/");
        assert_eq!(l[1].link, "rue.example.com v3 📌");
        let bare = ReadingItem {
            stub_of: None,
            forked_from: None,
            ..item.clone()
        };
        assert!(lineage(&bare.lineage(), &[]).is_empty());
        // The item document fills what the reading item lacks; the reading
        // item's own fields win.
        let doc = blyg_core::Lineage {
            stub_of: Some(blyg_core::StubOf {
                url: Some("https://page.example.com/post".into()),
                ..Default::default()
            }),
            forked_from: Some(blyg_core::RemoteRef {
                origin: "https://lin.example.org/".into(),
                id: "L".into(),
                version: 2,
            }),
            transclusions: vec![],
        };
        let l = lineage(&bare.lineage().or(&doc), &[]);
        assert_eq!(l[0].url, "https://page.example.com/post");
        assert_eq!(l[1].link, "lin.example.org v2 📌");
        let l = lineage(&item.lineage().or(&doc), &[]);
        assert_eq!(l[0].url, "https://ada.example.net/");
        assert_eq!(l[1].link, "rue.example.com v3 📌");
    }
}
