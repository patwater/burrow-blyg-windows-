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
