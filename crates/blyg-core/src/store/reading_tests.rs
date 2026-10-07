use std::collections::HashMap;

use super::Store;
use crate::model::*;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn ri(
    sub: &str,
    rid: &str,
    origin: &str,
    page: Option<&str>,
    version: u32,
    observed: &str,
    body: &str,
) -> ReadingItem {
    ReadingItem {
        subscription_id: sub.into(),
        remote_id: rid.into(),
        subscription_title: sub.into(),
        origin: origin.into(),
        kind: Kind::Fragment,
        state: "current".into(),
        version,
        created: None,
        updated: None,
        observed_at: observed.into(),
        content_md: body.into(),
        content_html: String::new(),
        author: None,
        page: page.map(str::to_string),
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

fn kinds(pairs: &[(&str, SubscriptionKind)]) -> HashMap<String, SubscriptionKind> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn upserts_one_row_per_post_and_keeps_read_state() {
    let s = store();
    let k = kinds(&[("s1", SubscriptionKind::Blyg)]);
    let o = "https://a.example/";
    let p = Some("https://a.example/f/A1");
    s.merge_reading(
        &[ri("s1", "A1", o, p, 1, "2026-01-01T00:00:00Z", "first")],
        true,
        &k,
    )
    .unwrap();
    assert!(s.reading()[0].is_unread());
    s.mark_read("s1", "A1").unwrap();
    let r = &s.reading()[0];
    assert_eq!(r.read_version, Some(1));
    assert!(!r.edited_since_read());

    // Edited three times: still exactly one entry, now "edited", read state intact.
    for v in 2..=4 {
        let obs = format!("2026-01-0{v}T00:00:00Z");
        let body = format!("edit {v}");
        assert!(
            s.merge_reading(&[ri("s1", "A1", o, p, v, &obs, &body)], true, &k)
                .unwrap()
        );
    }
    let all = s.reading();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].version, 4);
    assert_eq!(all[0].content_md, "edit 4");
    assert_eq!(all[0].read_version, Some(1));
    assert!(all[0].edited_since_read());
    assert!(
        !raw_reading(&s).contains("first"),
        "the text of the version read is not retained"
    );
    assert!(!all[0].is_unread());
    // an identical pull is a no-op
    let same = ri("s1", "A1", o, p, 4, "2026-01-04T00:00:00Z", "edit 4");
    assert!(!s.merge_reading(&[same], true, &k).unwrap());
}

#[test]
fn collapses_cross_subscription_duplicates_preferring_blyg() {
    let s = store();
    let k = kinds(&[
        ("blyg", SubscriptionKind::Blyg),
        ("rss", SubscriptionKind::Rss),
        ("blyg2", SubscriptionKind::Blyg),
    ]);
    let page = Some("https://a.example/f/A1");
    let items = vec![
        // RSS copy observed more recently than the blyg one
        ri(
            "rss",
            "https://a.example/f/A1",
            "https://a.example/feed.xml",
            page,
            1,
            "2026-01-03T00:00:00Z",
            "rss body",
        ),
        ri(
            "blyg",
            "A1",
            "https://a.example/",
            page,
            2,
            "2026-01-02T00:00:00Z",
            "blyg body",
        ),
        // two subscriptions to one origin, no page URL: keyed on (origin, remote_id)
        ri(
            "blyg2",
            "B7",
            "https://b.example/",
            None,
            1,
            "2026-01-01T00:00:00Z",
            "b",
        ),
        ri(
            "blyg3",
            "B7",
            "https://b.example/",
            None,
            1,
            "2026-01-01T00:00:01Z",
            "b",
        ),
        ri(
            "blyg",
            "Z9",
            "https://a.example/",
            Some("https://a.example/f/Z9"),
            1,
            "2026-01-01T00:00:00Z",
            "z",
        ),
    ];
    s.merge_reading(&items, true, &k).unwrap();
    let all = s.reading();
    assert_eq!(all.len(), 3, "{all:#?}");
    // the A1 group sits where its newest member was (first) and shows the blyg row
    assert_eq!(all[0].subscription_id, "blyg");
    assert_eq!(all[0].content_md, "blyg body");
    assert_eq!(all.iter().filter(|r| r.remote_id == "B7").count(), 1);

    // reading one copy marks the post read everywhere
    s.mark_read("rss", "https://a.example/f/A1").unwrap();
    assert!(!s.reading()[0].is_unread());
}

#[test]
fn rss_id_change_on_edit_is_the_same_post() {
    let s = store();
    let k = kinds(&[("feed", SubscriptionKind::Rss)]);
    let o = "https://c.example/feed";
    let page = Some("https://c.example/posts/hello");
    s.merge_reading(
        &[ri(
            "feed",
            "guid-1",
            o,
            page,
            0,
            "2026-01-01T00:00:00Z",
            "v1",
        )],
        true,
        &k,
    )
    .unwrap();
    s.mark_read("feed", "guid-1").unwrap();
    // The feed re-issued the post under a new guid.
    s.merge_reading(
        &[ri(
            "feed",
            "guid-2",
            o,
            page,
            1,
            "2026-01-02T00:00:00Z",
            "v2",
        )],
        true,
        &k,
    )
    .unwrap();
    let all = s.reading();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].remote_id, "guid-2");
    assert_eq!(all[0].content_md, "v2");
    assert_eq!(all[0].read_version, Some(0), "read state carried over");
    assert!(all[0].edited_since_read());
}

#[test]
fn hides_tombstones_unless_signalled_or_hoppered() {
    let s = store();
    let k = kinds(&[("s", SubscriptionKind::Blyg)]);
    let mut gone = ri(
        "s",
        "T1",
        "https://a.example/",
        None,
        2,
        "2026-01-01T00:00:00Z",
        "",
    );
    gone.state = "tombstone".into();
    let mut kept_thumb = gone.clone();
    kept_thumb.remote_id = "T2".into();
    kept_thumb.thumb = Some(1);
    let mut kept_hopper = gone.clone();
    kept_hopper.remote_id = "T3".into();
    kept_hopper.hoppers = vec!["later".into()];
    s.merge_reading(&[gone, kept_thumb, kept_hopper], true, &k)
        .unwrap();
    let ids: Vec<_> = s.reading().into_iter().map(|r| r.remote_id).collect();
    assert_eq!(ids.len(), 2);
    assert!(!ids.contains(&"T1".to_string()));
    // signalling a hidden tombstone brings it back
    s.set_thumb("s", "T1", Some(-1)).unwrap();
    assert_eq!(s.reading().len(), 3);
}

#[test]
fn prunes_only_what_the_pull_covered() {
    let s = store();
    let k = HashMap::new();
    let old = ri("s", "OLD", "o", None, 1, "2025-01-01T00:00:00Z", "old");
    let gone = ri("s", "GONE", "o", None, 1, "2026-02-01T00:00:00Z", "gone");
    let keep = ri("s", "NEW", "o", None, 1, "2026-03-01T00:00:00Z", "new");
    s.merge_reading(&[keep.clone(), gone, old], true, &k)
        .unwrap();
    // partial pull (more pages exist) reaching back to 2026-01-15: OLD is outside it
    let mid = ri("s", "MID", "o", None, 1, "2026-01-15T00:00:00Z", "mid");
    s.merge_reading(&[keep.clone(), mid], false, &k).unwrap();
    let ids: Vec<_> = s.reading().into_iter().map(|r| r.remote_id).collect();
    assert_eq!(ids, vec!["NEW", "MID", "OLD"]);
    // a complete pull drops everything not listed
    s.merge_reading(&[keep], true, &k).unwrap();
    assert_eq!(s.reading().len(), 1);
}

#[test]
fn mark_read_unknown_is_not_found() {
    assert!(matches!(
        store().mark_read("x", "y"),
        Err(crate::CoreError::NotFound)
    ));
}

/// Every stored byte of the reading table, as text.
fn raw_reading(s: &Store) -> String {
    let c = s.conn();
    let mut st = c
        .prepare("SELECT subscription_id || remote_id || json FROM reading")
        .unwrap();
    st.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tombstone(of: &ReadingItem, version: u32) -> ReadingItem {
    let mut t = of.clone();
    t.state = "tombstone".into();
    t.version = version;
    t.observed_at = "2026-02-01T00:00:00Z".into();
    t
}

fn pin(origin: &str, id: &str, version: u32, body: &str) -> PinnedVersion {
    PinnedVersion {
        version,
        at: "2026-01-01T00:00:00Z".into(),
        note: None,
        content_md: body.into(),
        content_html: format!("<p>{body}</p>"),
        content_hash: content_hash(body),
        origin: origin.into(),
        id: id.into(),
        author: None,
        hash_mismatch: false,
    }
}

#[test]
fn withdrawal_drops_content_that_no_pin_backs() {
    let s = store();
    let k = kinds(&[("s", SubscriptionKind::Blyg)]);
    let mut live = ri(
        "s",
        "W1",
        "https://a.example/",
        None,
        1,
        "2026-01-01T00:00:00Z",
        "secret body",
    );
    live.content_html = "<p>secret body</p>".into();
    s.merge_reading(std::slice::from_ref(&live), true, &k)
        .unwrap();
    s.mark_read("s", "W1").unwrap();
    // A careless server that still sends the old bytes on the tombstone.
    let mut gone = tombstone(&live, 2);
    gone.thumb = Some(1); // signalled, so it's still listed
    s.merge_reading(&[gone], true, &k).unwrap();
    let r = &s.reading()[0];
    assert_eq!(r.state, "tombstone");
    assert_eq!(r.content_md, "");
    assert_eq!(r.content_html, "");
    assert_eq!(r.pinned_version_retained, None);
    assert!(!raw_reading(&s).contains("secret"), "{}", raw_reading(&s));
}

#[test]
fn withdrawal_keeps_only_pinned_content_with_attribution() {
    let s = store();
    let k = kinds(&[("s", SubscriptionKind::Blyg)]);
    let o = "https://a.example/blyg/";
    let live = ri("s", "P1", o, None, 3, "2026-01-01T00:00:00Z", "v3 latest");

    // 1. The server retained a pinned version: its bytes are kept.
    let mut retained = tombstone(&live, 4);
    retained.content_md = "pinned v2 text".into();
    retained.pinned_version_retained = Some(2);
    retained.hoppers = vec!["later".into()];
    s.merge_reading(&[retained], true, &k).unwrap();
    let r = &s.reading()[0];
    assert_eq!(r.content_md, "pinned v2 text");
    assert_eq!(r.pinned_version_retained, Some(2));
    assert_eq!(
        r.pin_url().as_deref(),
        Some("https://a.example/blyg/items/P1/v2.json")
    );

    // 2. The server kept nothing, but we cached a pin ourselves.
    let live2 = ri("s", "P2", o, None, 2, "2026-01-01T00:00:00Z", "v2 latest");
    s.merge_reading(std::slice::from_ref(&live2), false, &k)
        .unwrap();
    s.put_pin(&pin(o, "P2", 1, "pinned v1")).unwrap();
    let mut gone = tombstone(&live2, 3);
    gone.content_md = String::new();
    gone.thumb = Some(1);
    s.merge_reading(&[gone], false, &k).unwrap();
    let r = s
        .reading()
        .into_iter()
        .find(|r| r.remote_id == "P2")
        .unwrap();
    assert_eq!(r.content_md, "pinned v1");
    assert_eq!(r.content_html, "<p>pinned v1</p>");
    assert_eq!(r.pinned_version_retained, Some(1));
    assert!(!raw_reading(&s).contains("v2 latest"));
}

#[test]
fn a_returned_item_is_current_again() {
    let s = store();
    let k = kinds(&[("s", SubscriptionKind::Blyg)]);
    let o = "https://a.example/";
    let live = ri("s", "R1", o, None, 1, "2026-01-01T00:00:00Z", "one");
    let mut gone = tombstone(&live, 2);
    gone.pinned_version_retained = Some(1);
    s.merge_reading(&[gone], true, &k).unwrap();
    let back = ri("s", "R1", o, None, 3, "2026-03-01T00:00:00Z", "three");
    s.merge_reading(&[back], true, &k).unwrap();
    let r = &s.reading()[0];
    assert_eq!(r.state, "current");
    assert_eq!(r.content_md, "three");
    assert_eq!(r.pinned_version_retained, None);
}

#[test]
fn changelog_and_pin_cache() {
    let s = store();
    let o = "https://a/";
    assert!(s.cached_changelog(o, "X").is_none());
    let v = vec![RemoteVersion {
        version: 1,
        at: "t".into(),
        note: Some("n".into()),
        pinned: true,
        current: true,
        media: vec![],
        lineage: Default::default(),
    }];
    s.put_changelog(o, "X", &v).unwrap();
    assert_eq!(s.cached_changelog(o, "X").unwrap().0, v);
    s.drop_changelog(o, "X").unwrap();
    assert!(s.cached_changelog(o, "X").is_none());

    s.put_pin(&pin(o, "X", 1, "first")).unwrap();
    s.put_pin(&pin(o, "X", 1, "tampered")).unwrap();
    assert_eq!(
        s.cached_pin(o, "X", 1).unwrap().content_md,
        "first",
        "pins are immutable: the first copy stays"
    );
    assert!(s.cached_pin(o, "X", 2).is_none());
    assert!(
        s.cached_pin("https://b/", "X", 1).is_none(),
        "scoped to origin"
    );
}

const OLD_LIVE: &str = r#"{"subscription_id":"s","remote_id":"A","subscription_title":"s","origin":"https://a/","kind":"fragment","state":"current","version":2,"created":null,"updated":null,"observed_at":"2026-01-02T00:00:00Z","content_md":"now","content_html":"","author":null,"page":null,"thumb":null,"hoppers":[]}"#;
const OLD_GONE: &str = r#"{"subscription_id":"s","remote_id":"B","subscription_title":"s","origin":"https://a/","kind":"fragment","state":"tombstone","version":3,"created":null,"updated":null,"observed_at":"2026-01-03T00:00:00Z","content_md":"WITHDRAWNTEXT","content_html":"<p>WITHDRAWNTEXT</p>","author":null,"page":null,"thumb":1,"hoppers":[]}"#;

#[test]
fn migration_purges_old_snapshots_and_withdrawn_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        let mut c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        super::schema::migrate_to(&mut c, 1).unwrap();
        c.execute(
            "INSERT INTO reading (subscription_id, remote_id, origin, observed_at, state, version, json, \
             read_version, read_snapshot_md) VALUES ('s', 'A', 'https://a/', '2026-01-02T00:00:00Z', \
             'current', 2, ?1, 1, 'OLDSNAPSHOTTEXT')",
            [OLD_LIVE],
        )
        .unwrap();
        c.execute(
            "INSERT INTO reading (subscription_id, remote_id, origin, observed_at, state, version, json) \
             VALUES ('s', 'B', 'https://a/', '2026-01-03T00:00:00Z', 'tombstone', 3, ?1)",
            [OLD_GONE],
        )
        .unwrap();
        c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), super::schema::latest());
    let cols: Vec<String> = {
        let c = s.conn();
        let mut st = c
            .prepare("SELECT name FROM pragma_table_info('reading')")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert!(!cols.contains(&"read_snapshot_md".to_string()), "{cols:?}");
    let all = s.reading();
    let a = all.iter().find(|r| r.remote_id == "A").unwrap();
    assert_eq!(a.read_version, Some(1), "the read version number survives");
    assert!(a.edited_since_read());
    let b = all.iter().find(|r| r.remote_id == "B").unwrap();
    assert_eq!(b.content_md, "");
    assert_eq!(b.content_html, "");
    drop(s);
    // Not merely unlinked: gone from the file (secure_delete + checkpoint).
    let mut bytes = std::fs::read(&path).unwrap();
    if let Ok(wal) = std::fs::read(dir.path().join("old.db-wal")) {
        bytes.extend(wal);
    }
    let hay = String::from_utf8_lossy(&bytes);
    assert!(
        !hay.contains("OLDSNAPSHOTTEXT"),
        "snapshot bytes left on disk"
    );
    assert!(
        !hay.contains("WITHDRAWNTEXT"),
        "withdrawn bytes left on disk"
    );
}

// ---------------------------------------------------------------- post dates (#6)

fn dated(rid: &str, observed: &str, created: Option<&str>, updated: Option<&str>) -> ReadingItem {
    let mut it = ri("s", rid, "https://a.example/", None, 1, observed, rid);
    it.created = created.map(str::to_string);
    it.updated = updated.map(str::to_string);
    it
}

fn order_of(s: &Store) -> Vec<String> {
    s.reading().into_iter().map(|r| r.remote_id).collect()
}

#[test]
fn sorts_by_the_posts_own_date_not_the_import_time() {
    let s = store();
    // All three imported at once (a fresh subscription): the post dates
    // decide, not observed_at.
    let obs = "2030-01-10T00:00:00Z";
    let items = vec![
        dated("old", obs, Some("2029-06-01T00:00:00Z"), None),
        dated("new", obs, Some("2029-12-01T00:00:00+01:00"), None),
        dated("mid", obs, None, Some("Mon, 01 Oct 2029 08:00:00 GMT")),
    ];
    s.merge_reading(&items, true, &HashMap::new()).unwrap();
    assert_eq!(order_of(&s), ["new", "mid", "old"]);
}

#[test]
fn an_edit_by_the_author_moves_the_post_up() {
    let s = store();
    let a = dated(
        "a",
        "2030-01-01T00:00:00Z",
        Some("2029-01-01T00:00:00Z"),
        Some("2029-01-01T00:00:00Z"),
    );
    let b = dated(
        "b",
        "2030-01-01T00:00:00Z",
        Some("2029-02-01T00:00:00Z"),
        Some("2029-02-01T00:00:00Z"),
    );
    s.merge_reading(&[a.clone(), b.clone()], true, &HashMap::new())
        .unwrap();
    assert_eq!(order_of(&s), ["b", "a"]);
    let mut a2 = a.clone();
    a2.version = 2;
    a2.updated = Some("2029-03-01T00:00:00Z".into());
    s.merge_reading(&[a2, b], true, &HashMap::new()).unwrap();
    assert_eq!(order_of(&s), ["a", "b"], "edited post moves to the top");
    let top = &s.reading()[0];
    assert!(top.post_time().edited);
    assert_eq!(top.post_time().at, "2029-03-01T00:00:00.000Z");
}

#[test]
fn observed_at_is_only_the_fallback() {
    let s = store();
    let items = vec![
        // No dates at all: imported time.
        dated("nodate", "2030-01-05T00:00:00Z", None, None),
        // Junk dates: imported time too.
        dated(
            "junk",
            "2030-01-03T00:00:00Z",
            Some("yesterday"),
            Some("soon"),
        ),
        dated(
            "dated",
            "2030-01-09T00:00:00Z",
            Some("2030-01-04T00:00:00Z"),
            None,
        ),
    ];
    s.merge_reading(&items, true, &HashMap::new()).unwrap();
    assert_eq!(order_of(&s), ["nodate", "dated", "junk"]);
    let all = s.reading();
    let nodate = all.iter().find(|r| r.remote_id == "nodate").unwrap();
    assert!(nodate.post_time().observed);
    assert!(!nodate.post_time().edited);
}

#[test]
fn migration_fills_sort_at_for_existing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v4.db");
    let row = |rid: &str, observed: &str, created: &str| {
        serde_json::json!({
            "subscription_id": "s", "remote_id": rid, "subscription_title": "s",
            "origin": "https://a.example/", "kind": "fragment", "state": "current",
            "version": 1, "created": created, "updated": null, "observed_at": observed,
            "content_md": rid, "content_html": "", "author": null, "page": null,
            "thumb": null, "hoppers": []
        })
        .to_string()
    };
    {
        let mut c = rusqlite::Connection::open(&path).unwrap();
        super::schema::migrate_to(&mut c, 4).unwrap();
        for (rid, obs, created) in [
            ("x", "2030-01-02T00:00:00Z", "2029-01-01T00:00:00Z"),
            ("y", "2030-01-01T00:00:00Z", "2029-05-01T00:00:00Z"),
        ] {
            c.execute(
                "INSERT INTO reading (subscription_id, remote_id, origin, observed_at, state, version, json) \
                 VALUES ('s', ?1, 'https://a.example/', ?2, 'current', 1, ?3)",
                rusqlite::params![rid, obs, row(rid, obs, created)],
            )
            .unwrap();
        }
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), super::schema::latest());
    // By observed_at it would be x, y; by the posts' dates it's y, x.
    assert_eq!(order_of(&s), ["y", "x"]);
    let empty: i64 = s
        .conn()
        .query_row("SELECT COUNT(*) FROM reading WHERE sort_at = ''", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(empty, 0);
}

#[test]
fn read_state_survives_the_reorder() {
    let s = store();
    let a = dated(
        "a",
        "2030-01-01T00:00:00Z",
        Some("2029-01-01T00:00:00Z"),
        None,
    );
    s.merge_reading(std::slice::from_ref(&a), true, &HashMap::new())
        .unwrap();
    assert!(s.mark_read("s", "a").unwrap());
    let mut a2 = a;
    a2.version = 2;
    a2.updated = Some("2029-09-01T00:00:00Z".into());
    s.merge_reading(&[a2], true, &HashMap::new()).unwrap();
    let r = &s.reading()[0];
    assert_eq!(r.read_version, Some(1));
    assert!(r.edited_since_read());
}

// ---------------------------------------------------------------- responses (#7)

fn responder(sub: &str, rid: &str, origin: &str, sort: &str) -> ReadingItem {
    let mut it = ri(sub, rid, origin, None, 1, sort, rid);
    it.created = Some(sort.into());
    it.kind = Kind::Thread;
    it
}

#[test]
fn responses_are_found_by_origin_and_id() {
    let s = store();
    let target = ri(
        "sa",
        "T1",
        "https://ada.example/",
        None,
        2,
        "2030-01-01T00:00:00Z",
        "t",
    );
    let mut quote = responder("sb", "Q1", "https://bo.example/", "2030-01-03T00:00:00Z");
    quote.transclusions = vec![TransclusionRef {
        id: "t1".into(), // another spelling of the id
        version: Some(2),
        origin: Some("https://ADA.example".into()),
        cited: None,
        selector: None,
    }];
    let mut stub = responder("sc", "S1", "https://cy.example/", "2030-01-04T00:00:00Z");
    stub.stub_of = Some(StubOf {
        origin: Some("https://ada.example/".into()),
        id: Some("T1".into()),
        version: Some(1),
        url: None,
    });
    let mut fork = responder("sc", "F1", "https://cy.example/", "2030-01-02T00:00:00Z");
    fork.forked_from = Some(RemoteRef {
        origin: "https://ada.example/".into(),
        id: "T1".into(),
        version: 1,
    });
    // Same id on another origin: not a response.
    let mut other = responder("sd", "O1", "https://dee.example/", "2030-01-05T00:00:00Z");
    other.stub_of = Some(StubOf {
        origin: Some("https://elsewhere.example/".into()),
        id: Some("T1".into()),
        version: None,
        url: None,
    });
    // A quote without an origin is from its own blyg.
    let mut own = responder("sa", "A2", "https://ada.example/", "2030-01-06T00:00:00Z");
    own.transclusions = vec![TransclusionRef {
        id: "T1".into(),
        version: None,
        origin: None,
        cited: None,
        selector: None,
    }];
    s.merge_reading(
        &[target, quote, stub.clone(), fork, other, own],
        true,
        &HashMap::new(),
    )
    .unwrap();
    let r = s.responses_to("https://ada.example", "T1");
    let got: Vec<(&str, crate::profile::Relation, Option<u32>)> = r
        .iter()
        .map(|x| (x.item.remote_id.as_str(), x.relation, x.version))
        .collect();
    use crate::profile::Relation::*;
    assert_eq!(
        got,
        [
            ("A2", Quotes, None),
            ("S1", Stubs, Some(1)),
            ("Q1", Quotes, Some(2)),
            ("F1", Forks, Some(1)),
        ]
    );
    // An edit that drops the stub drops the response; a pruned row too.
    let mut edited = stub;
    edited.stub_of = None;
    s.merge_reading(&[edited], false, &HashMap::new()).unwrap();
    assert!(
        !s.responses_to("https://ada.example/", "t1")
            .iter()
            .any(|x| x.item.remote_id == "S1")
    );
    s.merge_reading(&[], true, &HashMap::new()).unwrap();
    assert!(s.responses_to("https://ada.example/", "T1").is_empty());
}

#[test]
fn migration_indexes_existing_rows_references() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v5.db");
    let json = serde_json::json!({
        "subscription_id": "s", "remote_id": "R", "subscription_title": "s",
        "origin": "https://b.example/", "kind": "thread", "state": "current",
        "version": 1, "created": "2030-01-01T00:00:00Z", "updated": null,
        "observed_at": "2030-01-01T00:00:00Z", "content_md": "r", "content_html": "",
        "author": null, "page": null, "thumb": null, "hoppers": [],
        "stub_of": { "origin": "https://a.example/", "id": "X", "version": 1 }
    })
    .to_string();
    {
        let mut c = rusqlite::Connection::open(&path).unwrap();
        super::schema::migrate_to(&mut c, 5).unwrap();
        c.execute(
            "INSERT INTO reading (subscription_id, remote_id, origin, observed_at, state, version, json) \
             VALUES ('s', 'R', 'https://b.example/', '2030-01-01T00:00:00Z', 'current', 1, ?1)",
            [json],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    let r = s.responses_to("https://a.example/", "x");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].relation, crate::profile::Relation::Stubs);
}
