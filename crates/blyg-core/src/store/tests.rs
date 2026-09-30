use super::*;
use std::time::Instant;

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

#[test]
fn migrations_set_user_version() {
    let s = store();
    assert_eq!(s.schema_version().unwrap(), schema::latest());
}

#[test]
fn reopen_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blygger.db");
    let id = Store::open(&path)
        .unwrap()
        .create_draft(Kind::Fragment, "persist me", 0)
        .unwrap();
    let s = Store::open(&path).unwrap();
    assert_eq!(s.item(&id).unwrap().content_md, "persist me");
    assert_eq!(s.pending_count(), 1);
    let mode: String = s
        .conn()
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
}

#[test]
fn search_is_case_insensitive_substring() {
    let s = store();
    let a = s
        .create_draft(Kind::Fragment, "Walking by the lake at Montreux", 0)
        .unwrap();
    let b = s
        .create_draft(Kind::Thread, "# Notes\nthe MONTH of rain", 0)
        .unwrap();
    let _c = s.create_draft(Kind::Fragment, "nothing here", 0).unwrap();

    let ids = |q: &str| {
        s.search(q)
            .into_iter()
            .map(|i| i.local_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids("mont").len(), 2);
    assert_eq!(ids("montreux"), vec![a.clone()]);
    assert_eq!(ids("MONTR"), vec![a.clone()]);
    assert_eq!(ids("ontre"), vec![a.clone()], "mid-word substring");
    assert_eq!(ids("month of"), vec![b.clone()], "phrase with a space");
    assert!(ids("zzz").is_empty());
    // short (non-trigram) queries
    assert_eq!(ids("mo").len(), 2);
    assert_eq!(ids("Wa"), vec![a.clone()]);
    // quotes don't break the MATCH syntax
    assert!(ids("\"quoted\"").is_empty());
    // empty = everything, newest-updated first
    assert_eq!(s.search("  ").len(), 3);
    // edits are reindexed (a tick later: `updated` has millisecond precision,
    // and a tie with _c's creation would put _c first)
    std::thread::sleep(std::time::Duration::from_millis(3));
    s.save(&b, "renamed entirely", 0).unwrap();
    assert_eq!(ids("mont"), vec![a.clone()]);
    assert_eq!(ids("entire"), vec![b.clone()]);
    // order matches items(): b was just edited so it's first
    assert_eq!(s.items()[0].local_id, b);
    s.delete_item(&a).unwrap();
    assert!(ids("montreux").is_empty());
}

#[test]
fn search_unicode_case_folding() {
    let s = store();
    let a = s
        .create_draft(Kind::Fragment, "Ärger in Zürich", 0)
        .unwrap();
    assert_eq!(s.search("zürich").len(), 1);
    assert_eq!(s.search("äRGER")[0].local_id, a);
}

#[test]
fn save_coalesces_into_one_op() {
    let s = store();
    let id = s.create_draft(Kind::Fragment, "", 800).unwrap();
    for i in 0..50 {
        s.save(&id, &"x".repeat(i + 1), 800).unwrap();
    }
    let ops = s.ops().unwrap();
    assert_eq!(ops.len(), 1, "create absorbs saves");
    assert_eq!(ops[0].kind, OpKind::Create);
    // once the create has landed, 50 more saves are one save
    s.op_created(ops[0].seq, &id, "SID1", "x", Kind::Fragment)
        .unwrap();
    for i in 0..50 {
        s.save(&id, &"y".repeat(i + 1), 800).unwrap();
    }
    let ops = s.ops().unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, OpKind::Save);
    // a save while that op is in flight queues a new one
    s.set_in_flight(ops[0].seq, true).unwrap();
    s.save(&id, "z", 800).unwrap();
    assert_eq!(s.ops().unwrap().len(), 2);
}

#[test]
fn retirement_ops_are_not_local_edits() {
    let s = store();
    let id = s.create_draft(Kind::Fragment, "a", 0).unwrap();
    let seq = s.ops().unwrap()[0].seq;
    s.op_created(seq, &id, "OLD", "a", Kind::Fragment).unwrap();
    s.set_kind(&id, Kind::Thread, 0).unwrap();
    let seq = s.ops().unwrap()[0].seq;
    s.op_recreated(seq, &id, "NEW", "OLD", "a", Kind::Thread)
        .unwrap();
    // only the delete of OLD is queued: not an unpushed edit
    let ops = s.ops().unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, OpKind::DeleteRemote);
    assert_eq!(ops[0].payload, "OLD");
    assert!(!s.item(&id).unwrap().pending_sync);
    // a save must queue its own op, not coalesce into the delete
    s.save(&id, "b", 0).unwrap();
    let kinds: Vec<_> = s.ops().unwrap().into_iter().map(|o| o.kind).collect();
    assert_eq!(kinds, vec![OpKind::DeleteRemote, OpKind::Save]);
    assert!(s.item(&id).unwrap().pending_sync);
    // deleting the item locally keeps the retirement op
    s.delete_item(&id).unwrap();
    assert_eq!(s.ops().unwrap().len(), 1);
}

#[test]
fn bench_search_5000() {
    let s = store();
    let words = [
        "alpine", "lake", "Montreux", "jazz", "river", "sparrow", "ledger", "quartz", "orbit",
        "fennel",
    ];
    {
        let mut c = s.conn();
        let tx = c.transaction().unwrap();
        for i in 0..5000usize {
            let body = format!(
                "Item {i} {} {} {}\n\n{}",
                words[i % 10],
                words[(i * 7) % 10],
                if i % 250 == 0 { "zebracorn" } else { "plain" },
                "lorem ipsum dolor sit amet ".repeat(20)
            );
            tx.execute(
                "INSERT INTO items (local_id, kind, status, content_md, created, updated) VALUES (?1, 'fragment', 'draft', ?2, ?3, ?3)",
                params![format!("L{i}"), body, crate::util::iso_from_ms(1_700_000_000_000 + i as i64)],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    // warm up
    assert_eq!(s.search("zebracorn").len(), 20);
    let n = 50;
    let t = Instant::now();
    for _ in 0..n {
        assert_eq!(s.search("zebracorn").len(), 20);
    }
    let per = t.elapsed() / n;
    let t = Instant::now();
    for _ in 0..n {
        assert_eq!(s.search("ebraco").len(), 20);
    }
    let per_mid = t.elapsed() / n;
    eprintln!("search over 5000 items: {per:?} (full word), {per_mid:?} (mid-word)");
    assert!(per.as_micros() < 5000, "search took {per:?}");
    assert!(per_mid.as_micros() < 5000, "search took {per_mid:?}");

    let t = Instant::now();
    let all = s.items();
    eprintln!("items() over 5000 items, cold: {:?}", t.elapsed());
    assert_eq!(all.len(), 5000);
    let t = Instant::now();
    for _ in 0..n {
        assert_eq!(s.items().len(), 5000);
    }
    eprintln!("items() over 5000 items, cached: {:?}", t.elapsed() / n);
    // the cache is invalidated by writes
    let id = s.create_draft(Kind::Fragment, "fresh", 0).unwrap();
    assert_eq!(s.items()[0].local_id, id);
    s.save(&id, "fresher", 0).unwrap();
    assert_eq!(s.items()[0].content_md, "fresher");
}

fn generated(model: &str) -> Option<ScopeProvenance> {
    Some(ScopeProvenance {
        model: model.into(),
        sources: vec![],
        at: None,
    })
}

#[test]
fn tracked_provenance_follows_its_scopes() {
    let s = store();
    let text = "[TK]one[=]generated words[/TK]";
    let id = s.create_draft(Kind::Fragment, "x", 0).unwrap();
    s.save_inner(&id, text, Some(&[generated("m")]), 0).unwrap();
    let t = s.provenance(&id);
    assert!(t.dirty && t.any());
    assert_eq!(t.keyed_to.as_deref(), Some(text));
    s.prov_pushed(&id, text, &[generated("m")]).unwrap();
    assert!(!s.provenance(&id).dirty);

    // Typing outside any scope: nothing to re-push.
    let more = format!("{text} and more");
    s.save(&id, &more, 0).unwrap();
    assert!(!s.provenance(&id).dirty);

    // A scope typed in front shifts the generated one to position 1.
    let shifted = format!("[TK]mine[=]by hand[/TK] {more}");
    s.save(&id, &shifted, 0).unwrap();
    let t = s.provenance(&id);
    assert!(t.dirty, "needs the combined push");
    assert_eq!(t.scopes.unwrap(), vec![None, generated("m")]);

    // Mid-typing malformed text keeps the last good keying.
    s.save(&id, &format!("{shifted} [TK]half"), 0).unwrap();
    assert_eq!(
        s.provenance(&id).keyed_to.as_deref(),
        Some(shifted.as_str())
    );

    // A pushed result only sticks if the text didn't move on meanwhile.
    s.prov_pushed(&id, &shifted, &[None, generated("m")])
        .unwrap();
    assert!(s.provenance(&id).dirty);
}

#[test]
fn a_server_edit_forgets_tracked_provenance() {
    let s = store();
    let id = s.create_draft(Kind::Fragment, "x", 0).unwrap();
    let text = "[TK]one[=]generated[/TK]";
    s.save_inner(&id, text, Some(&[generated("m")]), 0).unwrap();
    s.conn().execute("DELETE FROM outbox", []).unwrap();
    s.conn()
        .execute(
            "UPDATE items SET server_id = 'S1' WHERE local_id = ?1",
            [&id.0],
        )
        .unwrap();
    let w = WireItem {
        id: "S1".into(),
        kind: "fragment".into(),
        authored_kind: None,
        status: "draft".into(),
        version: 0,
        dirty: true,
        created: String::new(),
        updated: String::new(),
        content_md: "rewritten in the studio".into(),
        stub_of: None,
        forked_from: None,
        permalink: None,
        show_responses: false,
        versions: None,
    };
    s.merge_all(&[w]).unwrap();
    assert_eq!(s.item(&id).unwrap().content_md, "rewritten in the studio");
    assert_eq!(s.provenance(&id), Tracked::default());
}

#[test]
fn public_origin_comes_from_permalinks() {
    let s = store();
    assert_eq!(s.public_origin(), None);
    let id = s.create_draft(Kind::Fragment, "x", 0).unwrap();
    s.after_publish(&id, 1, "https://x.example/blyg/f/abc", "x")
        .unwrap();
    assert_eq!(
        s.public_origin().as_deref(),
        Some("https://x.example/blyg/")
    );
}

// --- reading ---
#[test]
fn a_stub_is_a_draft_thread_with_stub_of() {
    let s = store();
    let of = RemoteRef {
        origin: "https://rue.blyg.example.com/".into(),
        id: "01K2RUE0TRUST0000000000001".into(),
        version: 5,
    };
    let id = s
        .create_stub(&of, "![[01K2RUE0TRUST0000000000001]]\n\n", 0)
        .unwrap();
    let it = s.item(&id).unwrap();
    assert_eq!(it.kind, Kind::Thread);
    assert_eq!(it.status, Status::Draft);
    assert_eq!(it.stub_of, Some(of));
    let ops = s.ops().unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, OpKind::Create, "pushed like any draft");
}
