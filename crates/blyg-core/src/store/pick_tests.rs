//! The picker's search and the highlight columns, on real SQLite.

use std::collections::HashMap;

use super::Store;
use crate::api::wire::WireItem;
use crate::model::*;
use crate::pick::{PickQuery, PickSort, PickSource};

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn published(id: &str, updated: &str, md: &str) -> WireItem {
    WireItem {
        id: id.into(),
        kind: "fragment".into(),
        status: "public".into(),
        version: 1,
        created: updated.into(),
        updated: updated.into(),
        content_md: md.into(),
        ..Default::default()
    }
}

fn ri(sub: &str, rid: &str, updated: &str, body: &str) -> ReadingItem {
    ReadingItem {
        subscription_id: sub.into(),
        remote_id: rid.into(),
        subscription_title: format!("{sub} title"),
        origin: format!("https://{sub}.example/"),
        kind: Kind::Thread,
        state: "current".into(),
        version: 3,
        created: Some(updated.into()),
        updated: Some(updated.into()),
        observed_at: updated.into(),
        content_md: body.into(),
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
        lineage_known: false,
    }
}

fn kinds() -> HashMap<String, SubscriptionKind> {
    [
        ("blog".to_string(), SubscriptionKind::Blyg),
        ("other".to_string(), SubscriptionKind::Blyg),
        ("feed".to_string(), SubscriptionKind::Rss),
    ]
    .into_iter()
    .collect()
}

fn q(text: &str) -> PickQuery {
    PickQuery {
        text: text.into(),
        ..Default::default()
    }
}

fn ids(s: &Store, q: &PickQuery) -> Vec<String> {
    s.pick_search(q)
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect()
}

/// Own posts: three published, one draft.
fn seeded() -> Store {
    let s = store();
    s.merge_all(&[
        published("MINE1", "2030-01-01T00:00:00Z", "Raised beds in October"),
        published(
            "MINE2",
            "2030-01-03T00:00:00Z",
            "Seed packets and *garden* notes",
        ),
        published("MINE3", "2030-01-05T00:00:00Z", "Nothing about it"),
        WireItem {
            status: "draft".into(),
            version: 0,
            ..published(
                "DRAFT1",
                "2030-01-06T00:00:00Z",
                "garden draft, never public",
            )
        },
    ])
    .unwrap();
    s.merge_reading(
        &[
            ri("blog", "R1", "2030-01-02T00:00:00Z", "The garden in winter"),
            ri("other", "R2", "2030-01-04T00:00:00Z", "Garden beds, raised"),
            ri("feed", "R3", "2030-01-07T00:00:00Z", "An RSS garden post"),
        ],
        true,
        &kinds(),
    )
    .unwrap();
    s
}

#[test]
fn every_word_must_match_in_any_order() {
    let s = seeded();
    assert_eq!(ids(&s, &q("garden")), vec!["R2", "MINE2", "R1"]);
    assert_eq!(ids(&s, &q("beds RAISED")), vec!["R2", "MINE1"]);
    assert_eq!(ids(&s, &q("garden winter")), vec!["R1"]);
    assert!(ids(&s, &q("garden zucchini")).is_empty());
    // Drafts and RSS posts are never offered (publish wouldn't accept them).
    assert!(!ids(&s, &q("draft")).contains(&"DRAFT1".to_string()));
    assert!(ids(&s, &q("RSS")).is_empty());
}

#[test]
fn short_words_and_ids_match_too() {
    let s = seeded();
    // "th" is too short for the trigram index: checked on the text.
    assert_eq!(ids(&s, &q("garden th")), vec!["R1"]);
    // …and on the id ("MINE2" has an "in").
    assert_eq!(ids(&s, &q("garden in")), vec!["MINE2", "R1"]);
    // An id (or part of one) finds its post.
    assert_eq!(ids(&s, &q("mine3")), vec!["MINE3"]);
    assert_eq!(ids(&s, &q("r2 raised")), vec!["R2"]);
}

#[test]
fn source_sub_sort_and_exclude() {
    let s = seeded();
    let mine = PickQuery {
        source: PickSource::Mine,
        ..q("garden")
    };
    assert_eq!(ids(&s, &mine), vec!["MINE2"]);
    let imported = PickQuery {
        source: PickSource::Imported,
        ..q("garden")
    };
    assert_eq!(ids(&s, &imported), vec!["R2", "R1"]);
    let one_sub = PickQuery {
        sub: Some("blog".into()),
        ..q("garden")
    };
    assert_eq!(
        ids(&s, &one_sub),
        vec!["R1"],
        "a subscription implies imported"
    );
    let oldest = PickQuery {
        sort: PickSort::Oldest,
        ..q("")
    };
    assert_eq!(
        ids(&s, &oldest),
        vec!["MINE1", "R1", "MINE2", "R2", "MINE3"]
    );
    let excluded = PickQuery {
        exclude: Some("MINE2".into()),
        ..q("garden")
    };
    assert_eq!(ids(&s, &excluded), vec!["R2", "R1"]);
    let limited = PickQuery { limit: 2, ..q("") };
    assert_eq!(ids(&s, &limited).len(), 2);
    let rows = s.pick_search(&q("winter")).unwrap();
    assert_eq!(rows[0].source, PickSource::Imported);
    assert_eq!(rows[0].source_title.as_deref(), Some("blog title"));
    assert_eq!(rows[0].kind, Kind::Thread);
    assert_eq!(rows[0].version, 3);
    let rows = s.pick_search(&q("seed")).unwrap();
    assert_eq!(rows[0].source, PickSource::Mine);
    assert_eq!(rows[0].excerpt, "Seed packets and garden notes");
}

#[test]
fn the_reading_index_follows_edits_and_removals() {
    let s = seeded();
    s.merge_reading(
        &[
            ri("blog", "R1", "2030-02-01T00:00:00Z", "Now about tomatoes"),
            ri("other", "R2", "2030-01-04T00:00:00Z", "Garden beds, raised"),
        ],
        true,
        &kinds(),
    )
    .unwrap();
    assert_eq!(ids(&s, &q("tomatoes")), vec!["R1"]);
    assert!(!ids(&s, &q("winter")).contains(&"R1".to_string()));
    // Unsubscribed: a complete pull without its rows drops them.
    s.merge_reading(
        &[ri(
            "blog",
            "R1",
            "2030-02-01T00:00:00Z",
            "Now about tomatoes",
        )],
        true,
        &kinds(),
    )
    .unwrap();
    assert!(ids(&s, &q("raised")).iter().all(|i| i != "R2"));
    let n: i64 = s
        .conn()
        .query_row("SELECT count(*) FROM reading_fts", [], |r| r.get(0))
        .unwrap();
    let m: i64 = s
        .conn()
        .query_row("SELECT count(*) FROM reading", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, m, "one index row per reading row");
}

#[test]
fn withdrawn_posts_only_with_a_retained_pin() {
    let s = store();
    let gone = ReadingItem {
        state: "tombstone".into(),
        ..ri("blog", "GONE", "2030-01-01T00:00:00Z", "withdrawn garden")
    };
    let pinned = ReadingItem {
        state: "tombstone".into(),
        pinned_version_retained: Some(2),
        thumb: Some(1),
        ..ri("blog", "PIN", "2030-01-02T00:00:00Z", "pinned garden")
    };
    s.merge_reading(&[gone, pinned], true, &kinds()).unwrap();
    let rows = s.pick_search(&q("garden")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].id.as_str(), rows[0].version), ("PIN", 2));
}

#[test]
fn migration_indexes_reading_rows_already_held() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        let mut c = rusqlite::Connection::open(&path).unwrap();
        super::schema::migrate_to(&mut c, 9).unwrap();
        let json = serde_json::to_string(&ri(
            "blog",
            "OLD",
            "2030-01-01T00:00:00Z",
            "an heirloom tomato",
        ))
        .unwrap();
        c.execute(
            "INSERT INTO reading (subscription_id, remote_id, origin, observed_at, sub_kind, state, \
             version, json) VALUES ('blog', 'OLD', 'https://blog.example/', '2030-01-01T00:00:00Z', \
             'blyg', 'current', 3, ?1)",
            [json],
        )
        .unwrap();
        c.execute(
            "INSERT INTO items (local_id, server_id, kind, status, version, dirty, content_md, \
             created, updated) VALUES ('L1', 'S1', 'fragment', 'public', 1, 0, 'x', 'c', 'u')",
            [],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), super::schema::latest());
    assert_eq!(ids(&s, &q("heirloom")), vec!["OLD"]);
    let it = s.item(&LocalId("L1".into())).unwrap();
    assert!(!it.highlight);
    assert_eq!(it.highlight_mode, None, "not reported yet");
}

#[test]
fn highlight_round_trips_through_merge_and_set() {
    let s = store();
    let mut w = published("H1", "2030-01-01T00:00:00Z", "x");
    w.highlight = Some(ResponsesMode::Default);
    w.resolve_highlight(true);
    s.merge_all(&[w.clone()]).unwrap();
    let id = s.local_id_for_server("H1").unwrap();
    let it = s.item(&id).unwrap();
    assert!(it.highlight, "default follows the blyg's default (on)");
    assert_eq!(it.highlight_mode, Some(HighlightMode::Default));

    s.set_highlight(&id, false, Some(HighlightMode::Hide))
        .unwrap();
    let it = s.item(&id).unwrap();
    assert!(!it.highlight);
    assert_eq!(it.highlight_mode, Some(HighlightMode::Hide));

    // A pull from a server that doesn't report it keeps the stored choice.
    let mut old = published("H1", "2030-01-01T00:00:00Z", "x");
    old.resolve_highlight(false);
    s.merge_all(&[old]).unwrap();
    assert_eq!(
        s.item(&id).unwrap().highlight_mode,
        Some(HighlightMode::Hide)
    );
    // The site default changes: a `default` item follows on the next pull.
    w.resolve_highlight(false);
    s.merge_all(&[w]).unwrap();
    let it = s.item(&id).unwrap();
    assert!(!it.highlight);
    assert_eq!(it.highlight_mode, Some(HighlightMode::Default));
}
