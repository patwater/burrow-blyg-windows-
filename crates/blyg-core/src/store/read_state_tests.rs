//! --- read/unread --- Read and unread on real SQLite: selection-wide
//! marks, outbox coalescing (last action wins), the unread floor, and how
//! a pull's read state settles against what's held (`read_sync.rs`).

use std::collections::HashMap;

use super::{OpKind, Store};
use crate::api::wire::{ReadMark, UnreadMark};
use crate::model::*;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

fn ri(sub: &str, rid: &str, page: Option<&str>, version: u32, observed: &str) -> ReadingItem {
    ReadingItem {
        subscription_id: sub.into(),
        remote_id: rid.into(),
        subscription_title: sub.into(),
        origin: "https://a.example/".into(),
        kind: Kind::Fragment,
        state: "current".into(),
        version,
        created: None,
        updated: None,
        observed_at: observed.into(),
        content_md: format!("{rid} v{version}"),
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

fn kinds() -> HashMap<String, SubscriptionKind> {
    [
        ("s1".to_string(), SubscriptionKind::Blyg),
        ("s2".to_string(), SubscriptionKind::Rss),
    ]
    .into_iter()
    .collect()
}

/// Three posts in s1 (A at v2, B, C), plus B again through s2's feed.
fn seeded() -> Store {
    let s = store();
    s.merge_reading(
        &[
            ri(
                "s1",
                "A",
                Some("https://a.example/f/A"),
                2,
                "2026-01-03T00:00:00Z",
            ),
            ri(
                "s1",
                "B",
                Some("https://a.example/f/B"),
                1,
                "2026-01-02T00:00:00Z",
            ),
            ri(
                "s1",
                "C",
                Some("https://a.example/f/C"),
                1,
                "2026-01-01T00:00:00Z",
            ),
            ri(
                "s2",
                "b-guid",
                Some("https://a.example/f/B"),
                1,
                "2026-01-02T00:00:00Z",
            ),
        ],
        true,
        &kinds(),
    )
    .unwrap();
    s
}

fn row(s: &Store, sub: &str, rid: &str) -> ReadingItem {
    s.reading_row(sub, rid).unwrap().0
}

/// (op, sub, remote_id) of every read/unread op in the outbox, in order.
fn read_ops(s: &Store) -> Vec<(OpKind, String, String)> {
    s.ops()
        .unwrap()
        .into_iter()
        .filter(|o| matches!(o.kind, OpKind::Read | OpKind::Unread))
        .map(|o| {
            let v: serde_json::Value = serde_json::from_str(&o.payload).unwrap();
            (
                o.kind,
                v["sub"].as_str().unwrap().to_string(),
                v["remote_id"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn server(v: &[(&str, &str, Option<u32>)]) -> HashMap<(String, String), Option<u32>> {
    v.iter()
        .map(|(s, r, rv)| ((s.to_string(), r.to_string()), *rv))
        .collect()
}

#[test]
fn a_selection_is_marked_read_in_one_go_with_its_duplicates() {
    let s = seeded();
    let marked = s
        .mark_many_read(&pairs(&[("s1", "A"), ("s1", "B")]), true)
        .unwrap()
        .unwrap();
    // B's feed copy is the same post: it goes too.
    let mut got: Vec<_> = marked
        .iter()
        .map(|m| (m.remote_id.as_str(), m.version))
        .collect();
    got.sort();
    assert_eq!(got, vec![("A", 2), ("B", 1), ("b-guid", 1)]);
    assert!(
        marked
            .iter()
            .all(|m| m.read_at.as_deref().is_some_and(|t| t.ends_with('Z')))
    );
    assert_eq!(row(&s, "s1", "A").read_version, Some(2));
    assert_eq!(row(&s, "s2", "b-guid").read_version, Some(1));
    assert_eq!(row(&s, "s1", "C").read_version, None);
    assert_eq!(read_ops(&s).len(), 3, "one read op per row");
    // Again: nothing rose, nothing more queued.
    assert!(
        s.mark_many_read(&pairs(&[("s1", "A")]), true)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert_eq!(read_ops(&s).len(), 3);
    // Nothing held: None (the backend's NotFound).
    assert!(
        s.mark_many_read(&pairs(&[("zz", "Q")]), true)
            .unwrap()
            .is_none()
    );
}

#[test]
fn unread_replaces_a_waiting_read_and_read_replaces_a_waiting_unread() {
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "C")]), true).unwrap();
    assert_eq!(read_ops(&s), vec![(OpKind::Read, "s1".into(), "C".into())]);

    let cleared = s
        .mark_many_unread(&pairs(&[("s1", "C")]), true)
        .unwrap()
        .unwrap();
    assert_eq!(
        cleared,
        vec![UnreadMark {
            sub: "s1".into(),
            remote_id: "C".into()
        }]
    );
    assert_eq!(row(&s, "s1", "C").read_version, None);
    assert_eq!(
        read_ops(&s),
        vec![(OpKind::Unread, "s1".into(), "C".into())],
        "last action wins: the read is gone"
    );

    // Unread twice: still one op.
    s.mark_many_read(&pairs(&[("s1", "C")]), true).unwrap();
    s.mark_many_unread(&pairs(&[("s1", "C")]), true).unwrap();
    s.mark_many_unread(&pairs(&[("s1", "C")]), true).unwrap();
    assert_eq!(
        read_ops(&s),
        vec![(OpKind::Unread, "s1".into(), "C".into())]
    );

    s.mark_many_read(&pairs(&[("s1", "C")]), true).unwrap();
    assert_eq!(read_ops(&s), vec![(OpKind::Read, "s1".into(), "C".into())]);
    assert_eq!(row(&s, "s1", "C").read_version, Some(1));
}

#[test]
fn an_op_on_the_wire_is_left_alone_and_the_new_one_waits_behind_it() {
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "C")]), true).unwrap();
    let seq = s.ops().unwrap()[0].seq;
    s.set_in_flight(seq, true).unwrap();
    s.mark_many_unread(&pairs(&[("s1", "C")]), true).unwrap();
    assert_eq!(
        read_ops(&s),
        vec![
            (OpKind::Read, "s1".into(), "C".into()),
            (OpKind::Unread, "s1".into(), "C".into())
        ]
    );
}

#[test]
fn unread_without_a_clearing_server_drops_the_waiting_read_and_queues_nothing() {
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "A"), ("s1", "C")]), true)
        .unwrap();
    s.mark_many_unread(&pairs(&[("s1", "A"), ("s1", "C")]), false)
        .unwrap();
    assert!(read_ops(&s).is_empty());
    assert_eq!(row(&s, "s1", "A").read_version, None);
}

#[test]
fn the_floor_keeps_a_stale_server_read_from_re_marking_an_unread_row() {
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "A")]), false).unwrap();
    s.mark_many_unread(&pairs(&[("s1", "A")]), false).unwrap();
    // The server (which can't clear) still says read at v2, pull after pull.
    let mut from_server = ri(
        "s1",
        "A",
        Some("https://a.example/f/A"),
        2,
        "2026-01-03T00:00:00Z",
    );
    from_server.read_version = Some(2);
    for _ in 0..2 {
        s.merge_reading(std::slice::from_ref(&from_server), false, &kinds())
            .unwrap();
        assert_eq!(row(&s, "s1", "A").read_version, None, "stays unread");
        let plan = s
            .reconcile_read_state(&server(&[("s1", "A", Some(2))]), false)
            .unwrap();
        assert!(plan.reads.is_empty() && plan.unreads.is_empty());
        assert!(read_ops(&s).is_empty(), "no loop of ops either");
    }
    // Read at a newer version elsewhere: that counts.
    let mut v3 = ri(
        "s1",
        "A",
        Some("https://a.example/f/A"),
        3,
        "2026-01-04T00:00:00Z",
    );
    v3.read_version = Some(3);
    s.merge_reading(&[v3], false, &kinds()).unwrap();
    assert_eq!(row(&s, "s1", "A").read_version, Some(3));
}

#[test]
fn a_clearing_server_gets_the_local_unread_and_then_the_floor_goes() {
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "B")]), false).unwrap();
    s.mark_many_unread(&pairs(&[("s1", "B")]), false).unwrap(); // made while it couldn't clear
    let plan = s
        .reconcile_read_state(
            &server(&[("s1", "B", Some(1)), ("s2", "b-guid", Some(1))]),
            true,
        )
        .unwrap();
    let mut rows: Vec<_> = plan.unreads.iter().map(|m| m.remote_id.clone()).collect();
    rows.sort();
    assert_eq!(rows, vec!["B", "b-guid"]);
    assert_eq!(read_ops(&s).len(), 2);
    s.unread_acked(&plan.unreads).unwrap();
    // Read again on another device at the same version: it shows read.
    let mut again = ri(
        "s1",
        "B",
        Some("https://a.example/f/B"),
        1,
        "2026-01-02T00:00:00Z",
    );
    again.read_version = Some(1);
    s.merge_reading(&[again], false, &kinds()).unwrap();
    assert_eq!(row(&s, "s1", "B").read_version, Some(1));
}

#[test]
fn a_clear_from_another_device_is_taken_once_this_read_was_confirmed() {
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "A"), ("s1", "C")]), false)
        .unwrap();
    // The server reports both read: confirmed (read_at goes).
    let plan = s
        .reconcile_read_state(&server(&[("s1", "A", Some(2)), ("s1", "C", Some(1))]), true)
        .unwrap();
    assert_eq!(plan, Default::default());
    // Another device marks A unread; C is read here again after... nothing.
    let plan = s
        .reconcile_read_state(&server(&[("s1", "A", None), ("s1", "C", Some(1))]), true)
        .unwrap();
    assert_eq!(plan.cleared, 1);
    assert!(
        plan.reads.is_empty(),
        "no read pushed back over their unread"
    );
    assert_eq!(row(&s, "s1", "A").read_version, None);
    assert_eq!(row(&s, "s1", "C").read_version, Some(1));
}

#[test]
fn a_read_the_server_lacks_is_queued_with_when_it_happened() {
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "C")]), false).unwrap();
    let plan = s
        .reconcile_read_state(&server(&[("s1", "C", None)]), true)
        .unwrap();
    assert_eq!(plan.cleared, 0);
    assert_eq!(plan.reads.len(), 1);
    let at = plan.reads[0]
        .read_at
        .clone()
        .expect("read_at for a clearing server");
    assert!(at.starts_with("20") && at.ends_with('Z'), "{at}");
    // Without read_state_clear: no read_at (a strict body would refuse it).
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "C")]), false).unwrap();
    let plan = s
        .reconcile_read_state(&server(&[("s1", "C", None)]), false)
        .unwrap();
    assert_eq!(plan.reads[0].read_at, None);
}

#[test]
fn rows_with_an_op_waiting_are_left_to_it() {
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "C")]), true).unwrap();
    s.mark_many_unread(&pairs(&[("s1", "C")]), true).unwrap();
    let plan = s
        .reconcile_read_state(&server(&[("s1", "C", Some(1))]), true)
        .unwrap();
    assert_eq!(plan, Default::default());
    assert_eq!(row(&s, "s1", "C").read_version, None);
}

#[test]
fn a_refused_stale_read_takes_the_servers_state() {
    let s = seeded();
    let m = s
        .mark_many_read(&pairs(&[("s1", "C")]), false)
        .unwrap()
        .unwrap()
        .remove(0);
    // Stored: confirmed, stays read.
    assert!(!s.read_acked(&m, true, Some(1), true).unwrap());
    assert_eq!(row(&s, "s1", "C").read_version, Some(1));
    // Refused (older than a clear elsewhere): unread here too.
    let m2 = ReadMark { ..m.clone() };
    assert!(s.read_acked(&m2, false, None, true).unwrap());
    assert_eq!(row(&s, "s1", "C").read_version, None);
}

#[test]
fn reads_from_before_the_migration_have_an_epoch_read_at() {
    let s = seeded();
    s.mark_many_read(&pairs(&[("s1", "C")]), false).unwrap();
    s.conn()
        .execute("UPDATE reading SET read_at = 0 WHERE remote_id = 'C'", [])
        .unwrap();
    let marks = s.read_marks(true).unwrap();
    assert_eq!(
        marks[0].read_at.as_deref(),
        Some("1970-01-01T00:00:00.000Z")
    );
}
