//! Sample reading data for `FakeBackend`: subscriptions, a reading list with
//! edited, unread and withdrawn posts, public changelogs with pins, mentions
//! and the blyg's site settings. All invented; every origin is under
//! `blyg.example.com` / `example.com`.

use std::collections::HashMap;

use blyg_core::*;
use chrono::{DateTime, Duration, Utc};

pub const RUE: &str = "https://rue.blyg.example.com/";
pub const ADA: &str = "https://ada.blyg.example.com/";
pub const LIN: &str = "https://lin.blyg.example.com/";
pub const OMAR: &str = "https://omar.example.com/";

/// Remote ids of the sample posts (26-char, base32-looking, invented).
pub const RUE_TRUST: &str = "01K2RUE0TRUST0000000000001";
pub const ADA_FINISHED: &str = "01K2ADA0FINISHED0000000001";
pub const LIN_GARDENS: &str = "01K2LIN0GARDENS00000000001";
pub const OMAR_YEAR: &str = "01K2OMAR0YEAR0000000000001";
pub const ADA_GONE: &str = "01K2ADA0GONE00000000000001";
pub const RUE_KEPT: &str = "01K2RUE0KEPT00000000000001";
pub const ADA_TIDES: &str = "01K2ADA0TIDES0000000000001";

/// Lin's thread as Lin's blyg published it: the quote of Ada's post baked in
/// as a transclusion snapshot, and an image with a relative (origin-relative)
/// URL. Its `content_md` keeps the directive, as the protocol does.
pub const LIN_GARDENS_HTML: &str = "<p>Gardens, not streams</p>
<p>A note on how I write in public: slowly, in beds that get turned over each season.</p>
<blockquote class=\"blyg-transclusion\" data-blyg-id=\"01K2ADA0TIDES0000000000001\" data-blyg-version=\"2\" data-blyg-origin=\"https://ada.blyg.example.com/\">
<p>Tide tables are a kind of promise the sea never signed.</p>
</blockquote>
<p><img src=\"media/raised-beds.jpg\" alt=\"Raised beds in October\"></p>
<h2>Pruning</h2>
<p>Cutting back is how a garden keeps its shape.</p>
";

pub struct Seed {
    pub reading: Vec<ReadingItem>,
    pub subs: Vec<Subscription>,
    /// (sub id, remote id) → public changelog, oldest first.
    pub changelogs: HashMap<(String, String), Vec<RemoteVersion>>,
    /// (sub id, remote id, version) → pinned version document.
    pub pins: HashMap<(String, String, u32), PinnedVersion>,
    pub mentions: Vec<Mention>,
    pub settings: Settings,
    /// Own posts: local id → per-version text (the fake's private history).
    pub own_texts: HashMap<(LocalId, u32), String>,
    /// Own posts: local id → versions with notes and pins.
    pub own_versions: HashMap<LocalId, Vec<Version>>,
}

fn sub(
    id: &str,
    kind: SubscriptionKind,
    origin: &str,
    title: &str,
    blogroll: bool,
) -> Subscription {
    Subscription {
        id: id.into(),
        kind,
        origin: origin.into(),
        feed_url: match kind {
            SubscriptionKind::Blyg => format!("{origin}feed.json"),
            SubscriptionKind::Rss => format!("{origin}feed.xml"),
        },
        title: title.into(),
        status: "active".into(),
        in_blogroll: blogroll,
    }
}

#[allow(clippy::too_many_arguments)]
fn post(
    sub: &Subscription,
    remote_id: &str,
    kind: Kind,
    version: u32,
    read: Option<u32>,
    observed: DateTime<Utc>,
    author: &str,
    content: &str,
) -> ReadingItem {
    ReadingItem {
        subscription_id: sub.id.clone(),
        remote_id: remote_id.into(),
        subscription_title: sub.title.clone(),
        origin: sub.origin.clone(),
        kind,
        state: "current".into(),
        version,
        created: Some(observed.to_rfc3339()),
        updated: Some(observed.to_rfc3339()),
        observed_at: observed.to_rfc3339(),
        content_md: content.into(),
        // What the author's blyg would have published (tests and the demo
        // override it where it matters).
        content_html: blyg_render::render_markdown(content),
        author: Some(Author {
            name: Some(author.into()),
            url: Some(sub.origin.clone()),
        }),
        page: Some(format!(
            "{}/{remote_id}",
            if kind == Kind::Thread { "t" } else { "f" }
        )),
        thumb: None,
        hoppers: vec![],
        pinned_version_retained: None,
        read_version: read,
        stub_of: None,
        forked_from: None,
        transclusions: vec![],
    }
}

fn rv(
    version: u32,
    at: DateTime<Utc>,
    note: Option<&str>,
    pinned: bool,
    current: bool,
) -> RemoteVersion {
    RemoteVersion {
        version,
        at: at.to_rfc3339(),
        note: note.map(str::to_string),
        pinned,
        current,
        media: vec![],
        lineage: Default::default(),
    }
}

fn pin(origin: &str, id: &str, version: u32, at: DateTime<Utc>, text: &str) -> PinnedVersion {
    PinnedVersion {
        version,
        at: at.to_rfc3339(),
        note: None,
        content_md: text.into(),
        content_html: blyg_render::render_markdown(text),
        content_hash: content_hash(text),
        origin: origin.into(),
        id: id.into(),
        author: None,
        hash_mismatch: false,
    }
}

pub fn seed(now: DateTime<Utc>) -> Seed {
    let h = Duration::hours;
    let d = Duration::days;
    let rue = sub("sub-rue", SubscriptionKind::Blyg, RUE, "Rue", true);
    let ada = sub("sub-ada", SubscriptionKind::Blyg, ADA, "Ada", true);
    let lin = sub("sub-lin", SubscriptionKind::Blyg, LIN, "Lin", false);
    let omar = sub(
        "sub-omar",
        SubscriptionKind::Rss,
        OMAR,
        "Omar's notes",
        false,
    );

    let trust_now = "A hyperlink is a small unit of trust, lent from one writer to another. Spend it carefully.";
    let mut reading = vec![
        post(
            &ada,
            ADA_FINISHED,
            Kind::Fragment,
            1,
            None,
            now - h(1),
            "Ada",
            "Lately I have been thinking about media that is finished: a record, a printed book, a letter. Nothing arrives to amend it.",
        ),
        post(
            &omar,
            OMAR_YEAR,
            Kind::Thread,
            1,
            Some(1),
            now - h(3),
            "Omar",
            "The consulting year in review\n\nFewer clients, longer projects, and a garden that finally got the attention it wanted.",
        ),
        // Read at v3 (pinned), now v5: a real diff is allowed.
        post(
            &rue,
            RUE_TRUST,
            Kind::Fragment,
            5,
            Some(3),
            now - h(5),
            "Rue",
            trust_now,
        ),
        // Read at v2 (not pinned), now v4: notes only, never a diff.
        post(
            &lin,
            LIN_GARDENS,
            Kind::Thread,
            4,
            Some(2),
            now - d(1),
            "Lin",
            "Gardens, not streams\n\nA note on how I write in public: slowly, in beds that get turned over each season.\n\n![[01K2ADA0TIDES0000000000001]]\n\n![Raised beds in October](media/raised-beds.jpg)\n\n## Pruning\n\nCutting back is how a garden keeps its shape.",
        ),
        post(
            &ada,
            ADA_TIDES,
            Kind::Fragment,
            2,
            Some(2),
            now - d(3),
            "Ada",
            "Tide tables are a kind of promise the sea never signed.",
        ),
    ];
    for r in reading.iter_mut() {
        match r.remote_id.as_str() {
            LIN_GARDENS => r.content_html = LIN_GARDENS_HTML.into(),
            // An RSS item that carries only text: the Markdown is rendered
            // by the app (blyg-render), not shown as published HTML.
            OMAR_YEAR => r.content_html.clear(),
            _ => {}
        }
    }
    // Withdrawn, never signalled: hidden.
    let mut gone = post(
        &ada,
        ADA_GONE,
        Kind::Fragment,
        3,
        Some(2),
        now - d(2),
        "Ada",
        "",
    );
    gone.state = "tombstone".into();
    reading.push(gone);
    // Withdrawn but thumbed up, with its pinned v2 retained: shown.
    let mut kept = post(
        &rue,
        RUE_KEPT,
        Kind::Fragment,
        4,
        Some(2),
        now - d(2) - h(2),
        "Rue",
        "Every archive is an argument about what mattered.",
    );
    kept.state = "tombstone".into();
    kept.thumb = Some(1);
    kept.pinned_version_retained = Some(2);
    reading.push(kept);

    let key = |s: &Subscription, id: &str| (s.id.clone(), id.to_string());
    let mut changelogs = HashMap::new();
    changelogs.insert(
        key(&rue, RUE_TRUST),
        vec![
            rv(1, now - d(6), Some("first"), true, false),
            rv(2, now - d(5), Some("typo"), false, false),
            rv(3, now - d(4), Some("sharper wording"), true, false),
            rv(4, now - d(1), Some("added the second clause"), false, false),
            rv(5, now - h(5), Some("spend it carefully"), false, true),
        ],
    );
    changelogs.insert(
        key(&lin, LIN_GARDENS),
        vec![
            rv(1, now - d(9), None, false, false),
            rv(2, now - d(8), Some("tidied"), false, false),
            rv(3, now - d(2), Some("new section on pruning"), false, false),
            RemoteVersion {
                // Attached on Lin's blyg: shown after the text.
                media: vec![RemoteMedia {
                    url: format!("{LIN}media/seed-packets.jpg"),
                    alt: Some("Seed packets on a windowsill".into()),
                }],
                // --- profiles --- Lin's thread stubs Ada's tide tables and
                // descends from Rue's pinned v3: the reading header's lineage
                // line. Only the item document says so (the owner reading
                // API doesn't send lineage), so it rides on the current row.
                lineage: Lineage {
                    stub_of: Some(StubOf {
                        origin: Some(ADA.into()),
                        id: Some(ADA_TIDES.into()),
                        version: Some(2),
                        url: None,
                    }),
                    forked_from: Some(RemoteRef {
                        origin: RUE.into(),
                        id: RUE_TRUST.into(),
                        version: 3,
                    }),
                    transclusions: vec![TransclusionRef {
                        id: ADA_TIDES.into(),
                        version: Some(2),
                        origin: Some(ADA.into()),
                    }],
                },
                ..rv(4, now - d(1), Some("linked the reply"), false, true)
            },
        ],
    );
    changelogs.insert(
        key(&ada, ADA_TIDES),
        vec![
            rv(1, now - d(4), None, true, false),
            rv(2, now - d(3), Some("one word"), false, true),
        ],
    );
    let mut pins = HashMap::new();
    let pk = |s: &Subscription, id: &str, v: u32| (s.id.clone(), id.to_string(), v);
    pins.insert(
        pk(&rue, RUE_TRUST, 1),
        pin(
            RUE,
            RUE_TRUST,
            1,
            now - d(6),
            "A hyperlink is a litle bit of trust.",
        ),
    );
    pins.insert(
        pk(&rue, RUE_TRUST, 3),
        pin(
            RUE,
            RUE_TRUST,
            3,
            now - d(4),
            "A hyperlink is a small unit of trust.",
        ),
    );
    pins.insert(
        pk(&ada, ADA_TIDES, 1),
        pin(
            ADA,
            ADA_TIDES,
            1,
            now - d(4),
            "Tide tables are a promise the sea never signed.",
        ),
    );
    pins.insert(
        pk(&rue, RUE_KEPT, 2),
        pin(
            RUE,
            RUE_KEPT,
            2,
            now - d(3),
            "Every archive is an argument about what mattered.",
        ),
    );

    let mention = |id: &str,
                   target: &str,
                   relation: &str,
                   origin: &str,
                   name: &str,
                   ago: Duration,
                   hidden: bool| Mention {
        id: id.into(),
        target_item_id: format!("{target}ABCDEFGHJKMNPQRS"),
        status: "verified".into(),
        relation: Some(relation.into()),
        source: format!("{origin}t/01K2M{id}"),
        source_origin: Some(origin.into()),
        source_id: Some(format!("01K2M{id}")),
        source_kind: Some("thread".into()),
        source_version: Some(1),
        source_author: Some(Author {
            name: Some(name.into()),
            url: Some(origin.into()),
        }),
        first_seen: (now - ago).to_rfc3339(),
        verified_at: Some((now - ago).to_rfc3339()),
        hidden,
    };
    let mentions = vec![
        mention("m1", "01J9M2A", "stub", RUE, "Rue", h(2), false),
        mention("m2", "01J9M2A", "transclusion", ADA, "Ada", d(3), false),
        mention("m3", "01J9K7T", "fork", LIN, "Lin", d(5), false),
        mention("m4", "01J9K7T", "transclusion", OMAR, "Omar", d(8), true),
    ];

    let settings = Settings {
        site_title: Some("Harbour notes".into()),
        author_name: Some("A. Writer".into()),
        author_bio: Some("Small notes from a harbour town.".into()),
        site_url: Some(super::ORIGIN.into()),
        theme: None,
        avatar_media_id: None,
        author_links: vec![AuthorLink {
            label: "Feed".into(),
            url: format!("{}/feed.json", super::ORIGIN),
        }],
    };

    // "On friction": v1 first, v2 pinned, v3 current.
    let friction = LocalId("01J9M2A".into());
    let texts = [
        "Friction kills posts.",
        "On friction: each step between noticing and writing is a place ideas die.",
        "On friction: every extra step between noticing something and writing it down is a chance to lose it.",
    ];
    let mut own_texts = HashMap::new();
    for (i, t) in texts.iter().enumerate() {
        own_texts.insert((friction.clone(), i as u32 + 1), t.to_string());
    }
    let mut own_versions = HashMap::new();
    own_versions.insert(
        friction,
        vec![
            Version {
                version: 1,
                published_at: (now - d(6)).to_rfc3339(),
                note: Some("first".into()),
                pinned: false,
                endcap: false,
            },
            Version {
                version: 2,
                published_at: (now - d(5)).to_rfc3339(),
                note: None,
                pinned: true,
                endcap: false,
            },
            Version {
                version: 3,
                published_at: (now - d(4)).to_rfc3339(),
                note: Some("tightened".into()),
                pinned: false,
                endcap: false,
            },
        ],
    );

    Seed {
        reading,
        subs: vec![rue, ada, lin, omar],
        changelogs,
        pins,
        mentions,
        settings,
        own_texts,
        own_versions,
    }
}
