//! --- profiles --- Invented public profiles for `FakeBackend` (docs/SPEC.md
//! § Profiles): what the manifests, blogrolls and archive indexes of the
//! sample blygs would say. Every origin is under example.com/org/net.

use blyg_core::profile::{BlogrollEntry, ProfilePost, clean_url, normalize_origin, same_origin};
use blyg_core::*;
use chrono::{DateTime, Duration, Utc};

use super::reading_seed::{ADA, LIN, OMAR, RUE};

pub const TIDES: &str = "https://tides.example.org/";
pub const FIELDNOTES_FEED: &str = "https://fieldnotes.example.com/rss";
pub const SHORELINE: &str = "https://notes.example.org/";

fn roll(title: &str, site: &str, feed: Option<&str>) -> BlogrollEntry {
    BlogrollEntry {
        title: title.into(),
        xml_url: Some(
            feed.map(str::to_string)
                .unwrap_or(format!("{site}feed.xml")),
        ),
        html_url: Some(site.into()),
    }
}

fn post(
    origin: &str,
    id: &str,
    title: &str,
    kind: &str,
    version: u32,
    pinned: bool,
    at: DateTime<Utc>,
) -> ProfilePost {
    let p = if kind == "thread" { "t" } else { "f" };
    ProfilePost {
        id: id.into(),
        title: Some(title.into()),
        kind: kind.into(),
        version: Some(version),
        pinned,
        date: Some(at.to_rfc3339()),
        url: Some(format!("{origin}{p}/{id}/")),
    }
}

#[allow(clippy::too_many_arguments)]
fn blyg(
    origin: &str,
    name: &str,
    title: &str,
    bio: &str,
    links: &[(&str, &str)],
    blogroll: Vec<BlogrollEntry>,
    posts: Vec<ProfilePost>,
    feed_quotes: &[(&str, &str)],
) -> Profile {
    Profile {
        kind: ProfileKind::Blyg,
        origin: origin.into(),
        feed_url: Some(format!("{origin}feed.xml")),
        name: Some(name.into()),
        title: Some(title.into()),
        bio: Some(bio.into()),
        // An avatar URL would be fetched by the image loader; the sample
        // data has none, so the sheet draws the initial instead.
        avatar: None,
        links: links
            .iter()
            .map(|(l, u)| AuthorLink {
                label: (*l).into(),
                url: (*u).into(),
            })
            .collect(),
        has_blogroll: !blogroll.is_empty(),
        blogroll,
        posts,
        connections: vec![],
        feed_quotes: feed_quotes
            .iter()
            .map(|(o, i)| ((*o).into(), (*i).into()))
            .collect(),
        own: false,
        fetched_at: 0,
        stale: false,
    }
}

fn feed(
    origin: &str,
    feed_url: &str,
    name: &str,
    posts: &[(&str, &str)],
    now: DateTime<Utc>,
) -> Profile {
    Profile {
        kind: ProfileKind::Feed,
        origin: origin.into(),
        feed_url: Some(feed_url.into()),
        name: Some(name.into()),
        title: None,
        bio: None,
        avatar: None,
        links: vec![],
        blogroll: vec![],
        has_blogroll: false,
        posts: posts
            .iter()
            .enumerate()
            .map(|(i, (t, path))| ProfilePost {
                id: format!("{origin}{path}"),
                title: Some((*t).into()),
                kind: "rss".into(),
                version: None,
                pinned: false,
                date: Some((now - Duration::days(3 * i as i64 + 1)).to_rfc3339()),
                url: Some(format!("{origin}{path}")),
            })
            .collect(),
        connections: vec![],
        feed_quotes: vec![],
        own: false,
        fetched_at: 0,
        stale: false,
    }
}

/// The profile at `url` (resolved like the fake subscribe preview), or a
/// 422 for anything that isn't an invented example address. `own` is the
/// user's own blyg, built from their site settings and subscriptions.
pub fn profile_for(
    url: &str,
    own_origin: &str,
    own: &Settings,
    subs: &[Subscription],
) -> Result<Profile> {
    let now = Utc::now();
    let d = Duration::days;
    let h = Duration::hours;
    let clean = clean_url(url).unwrap_or_default();
    if !clean.contains("example.") {
        return Err(CoreError::Rejected {
            status: 422,
            message: "no blyg or feed found at that address".into(),
            details: vec![],
        });
    }
    let at = |o: &str| clean.starts_with(o) || same_origin(&clean, o);
    let p = if at(LIN) {
        blyg(
            LIN,
            "Lin",
            "Lin's garden",
            "Writes slowly, in public, in beds that get turned over each season.",
            &[
                ("site", "https://lin.example.org/"),
                ("newsletter", "https://lin.example.org/letters"),
            ],
            vec![
                roll("Ada", ADA, None),
                roll("Rue", RUE, None),
                roll("Tide Tables", TIDES, None),
                roll(
                    "Field Notes Quarterly",
                    "https://fieldnotes.example.com/",
                    Some(FIELDNOTES_FEED),
                ),
            ],
            vec![
                post(
                    LIN,
                    super::reading_seed::LIN_GARDENS,
                    "Gardens, not streams",
                    "thread",
                    4,
                    false,
                    now - d(1),
                ),
                post(
                    LIN,
                    "01K2LIN0COMPOST0000000001",
                    "Compost is a kind of archive",
                    "fragment",
                    1,
                    false,
                    now - d(5),
                ),
                post(
                    LIN,
                    "01K2LIN0SEEDS000000000001",
                    "On saving seeds",
                    "thread",
                    3,
                    true,
                    now - d(12),
                ),
            ],
            &[(ADA, "01K2LIN0SEEDS000000000001")],
        )
    } else if at(ADA) {
        blyg(
            ADA,
            "Ada",
            "Beginnings",
            "Writes about beginnings: first posts, first drafts, first tools.",
            &[("site", "https://ada.example.net/")],
            vec![roll("Lin", LIN, None), roll("Tide Tables", TIDES, None)],
            vec![
                post(
                    ADA,
                    super::reading_seed::ADA_FINISHED,
                    "Lately I have been thinking about media that is finished",
                    "fragment",
                    1,
                    false,
                    now - h(1),
                ),
                post(
                    ADA,
                    super::reading_seed::ADA_TIDES,
                    "Tide tables are a kind of promise the sea never signed.",
                    "fragment",
                    2,
                    true,
                    now - d(3),
                ),
            ],
            &[(LIN, "01K2ADA0QUOTE00000000001")],
        )
    } else if at(RUE) {
        blyg(
            RUE,
            "Rue",
            "Small units of trust",
            "Archivist by day. Every archive is an argument about what mattered.",
            &[
                ("site", "https://rue.example.com/"),
                ("newsletter", "https://rue.example.com/letters"),
            ],
            vec![roll("Ada", ADA, None), roll("Tide Tables", TIDES, None)],
            vec![
                post(
                    RUE,
                    super::reading_seed::RUE_TRUST,
                    "A hyperlink is a small unit of trust",
                    "fragment",
                    5,
                    true,
                    now - h(5),
                ),
                ProfilePost {
                    id: super::reading_seed::RUE_KEPT.into(),
                    title: None,
                    kind: "withdrawn".into(),
                    version: Some(4),
                    pinned: true,
                    date: Some((now - d(2)).to_rfc3339()),
                    url: None,
                },
            ],
            &[],
        )
    } else if at(TIDES) {
        blyg(
            TIDES,
            "Tide Tables",
            "Tide Tables",
            "A shared almanac of small observations from the shore.",
            &[],
            vec![],
            vec![post(
                TIDES,
                "01K2TIDE0SPRING00000000001",
                "Spring tides, explained badly",
                "thread",
                1,
                false,
                now - d(2),
            )],
            &[],
        )
    } else if at(SHORELINE) {
        blyg(
            SHORELINE,
            "Shoreline Notes",
            "Shoreline Notes",
            "Notes from the edge of the water.",
            &[],
            vec![],
            vec![],
            &[],
        )
    } else if at(OMAR) {
        feed(
            OMAR,
            &format!("{OMAR}feed.xml"),
            "Omar's notes",
            &[
                ("The consulting year in review", "2030/review"),
                ("Soil", "2029/soil"),
            ],
            now,
        )
    } else if clean.starts_with("https://fieldnotes.example.com/") {
        feed(
            "https://fieldnotes.example.com/",
            FIELDNOTES_FEED,
            "Field Notes Quarterly",
            &[("Moss, again", "moss"), ("A census of puddles", "puddles")],
            now,
        )
    } else if same_origin(&clean, own_origin) || clean.starts_with(own_origin) {
        let origin = normalize_origin(own_origin).unwrap_or_else(|| own_origin.to_string());
        let mut p = blyg(
            &origin,
            own.author_name.as_deref().unwrap_or("You"),
            own.site_title.as_deref().unwrap_or(""),
            own.author_bio.as_deref().unwrap_or(""),
            &[],
            subs.iter()
                .filter(|s| s.in_blogroll && s.status != "paused")
                .map(|s| BlogrollEntry {
                    title: s.title.clone(),
                    xml_url: Some(s.feed_url.clone()),
                    html_url: Some(s.origin.clone()),
                })
                .collect(),
            vec![],
            &[],
        );
        p.links = own.author_links.clone();
        p.title = own.site_title.clone();
        p
    } else {
        // Any other invented address: a bare blyg with nothing to show.
        let origin = normalize_origin(&clean).unwrap_or(clean.clone());
        let host = blyg_core::profile::clean_url(&origin)
            .and_then(|u| u.split('/').nth(2).map(str::to_string))
            .unwrap_or_default();
        blyg(&origin, &host, &host, "", &[], vec![], vec![], &[])
    };
    Ok(p)
}
