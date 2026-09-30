//! Read-state sync through the user's blyg (owner-API extension 5,
//! docs/SERVER.md): merge on pull, queued/coalesced `read` ops, the one-time
//! batch upload, and an unpatched server seeing no new requests at all.

mod common;

use std::time::Duration;

use blyg_core::*;
use common::*;
use serde_json::{Value, json};

fn item(sub: &str, rid: &str, page: Option<&str>, version: u32, obs: &str) -> Value {
    json!({
        "subscription_id": sub, "remote_id": rid, "subscription_title": "Them",
        "origin": "https://them.example/", "kind": "fragment", "state": "current",
        "version": version, "created": null, "updated": null, "observed_at": obs,
        "content_md": "text", "content_html": "<p>text</p>",
        "author": null, "page": page, "thumb": null, "hoppers": []
    })
}

/// Two posts from one blyg (S1: a at v2, b at v1), read sync on or off.
fn env_with(read_sync: bool) -> Env {
    let env = Env::new();
    {
        let mut st = env.mock.state();
        st.read_sync = read_sync;
        st.subs = vec![
            json!({ "id": "S1", "kind": "blyg", "origin": "https://them.example/",
            "feed_url": "x", "title": "Them", "status": "active", "in_blogroll": false }),
        ];
        st.reading = Some(vec![
            item("S1", "a", Some("f/a"), 2, "2030-01-02T00:00:00Z"),
            item("S1", "b", Some("f/b"), 1, "2030-01-01T00:00:00Z"),
        ]);
    }
    env
}

fn read_version(b: &LiveBackend, rid: &str) -> Option<u32> {
    b.reading()
        .into_iter()
        .find(|r| r.remote_id == rid)
        .and_then(|r| r.read_version)
}

fn read_requests(env: &Env) -> Vec<String> {
    env.mock
        .state()
        .log
        .iter()
        .filter(|l| l.starts_with("PUT /api/reading/") || l.starts_with("POST /api/reading/read"))
        .cloned()
        .collect()
}

fn server_read(env: &Env, sub: &str, rid: &str) -> Option<u32> {
    env.mock
        .state()
        .reads
        .get(&(sub.to_string(), rid.to_string()))
        .copied()
}

#[test]
fn pull_merges_the_max_and_never_lowers_local_state() {
    let env = env_with(true);
    env.mock.state().reads.insert(("S1".into(), "b".into()), 1);
    let b = env.manual();
    b.sync_now().unwrap();
    assert_eq!(read_version(&b, "b"), Some(1), "read on another Mac");
    assert_eq!(read_version(&b, "a"), None);
    assert!(env.events().contains(&CoreEvent::ReadingChanged));

    // Read `a` here at v2; the server (stale) says v1 afterwards.
    b.mark_read("S1", "a").unwrap();
    env.mock.state().reads.insert(("S1".into(), "a".into()), 1);
    b.pull_now().unwrap();
    assert_eq!(
        read_version(&b, "a"),
        Some(2),
        "a lower server value never wins"
    );

    // The server can't lower it either: a later pull with null keeps it.
    env.mock.state().reads.clear();
    b.pull_now().unwrap();
    assert_eq!(read_version(&b, "a"), Some(2));
    assert_eq!(read_version(&b, "b"), Some(1));
}

#[test]
fn junk_read_version_is_ignored_leniently() {
    let env = env_with(true);
    {
        let mut st = env.mock.state();
        let mut r = st.reading.take().unwrap();
        r[0]["read_version"] = json!("two");
        r[1]["read_version"] = json!(-3);
        st.reading = Some(r);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    assert_eq!(b.reading().len(), 2, "odd fields don't drop the rows");
    assert_eq!(read_version(&b, "a"), None);
    assert_eq!(read_version(&b, "b"), None);
}

#[test]
fn read_version_is_ignored_without_the_capability_flag() {
    let env = env_with(false);
    {
        let mut st = env.mock.state();
        let mut r = st.reading.take().unwrap();
        r[1]["read_version"] = json!(1);
        st.reading = Some(r);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    assert_eq!(read_version(&b, "b"), None, "unadvertised: not read state");
}

#[test]
fn mark_read_queues_and_flushes_a_put() {
    let env = env_with(true);
    let b = env.manual();
    b.sync_now().unwrap();
    assert!(
        read_requests(&env).len() <= 1,
        "at most the (empty) initial upload"
    );
    env.mock.state().log.clear();

    b.mark_read("S1", "a").unwrap();
    assert_eq!(read_version(&b, "a"), Some(2), "instant and local");
    assert!(read_requests(&env).is_empty(), "nothing sent inline");
    b.sync_now().unwrap();
    assert_eq!(read_requests(&env), vec!["PUT /api/reading/S1/a/read"]);
    assert_eq!(server_read(&env, "S1", "a"), Some(2));

    // Nothing left to send.
    env.mock.state().log.clear();
    b.sync_now().unwrap();
    assert!(read_requests(&env).is_empty());
}

#[test]
fn the_worker_flushes_a_read_promptly() {
    let env = env_with(true);
    let b = env.open(SyncOptions {
        pull_interval: Duration::from_secs(3600),
        ..fast()
    });
    assert!(wait_until(Duration::from_secs(5), || b.reading().len() == 2));
    b.mark_read("S1", "b").unwrap();
    assert!(wait_until(Duration::from_secs(5), || server_read(
        &env, "S1", "b"
    ) == Some(1)));
    drop(b);
}

#[test]
fn offline_mark_then_flush_and_restart() {
    let env = env_with(true);
    {
        let b = env.manual();
        b.sync_now().unwrap();
        env.mock.set_down(true);
        b.mark_read("S1", "a").unwrap();
        assert_eq!(read_version(&b, "a"), Some(2));
        assert!(b.sync_now().is_err(), "offline");
        assert!(matches!(
            b.sync_status(),
            SyncStatus::Offline { pending: 1 }
        ));
    }
    // Restart while still offline: the capability is remembered, so a new
    // read queues too.
    let b = env.manual();
    b.mark_read("S1", "b").unwrap();
    env.mock.set_down(false);
    b.sync_now().unwrap();
    assert_eq!(server_read(&env, "S1", "a"), Some(2));
    assert_eq!(server_read(&env, "S1", "b"), Some(1));
}

#[test]
fn a_transient_failure_retries_later() {
    let env = env_with(true);
    let b = env.manual();
    b.sync_now().unwrap();
    env.mock.set_down(true);
    b.mark_read("S1", "a").unwrap();
    assert!(b.sync_now().is_err());
    env.mock.set_down(false);
    env.mock.state().log.clear();
    b.sync_now().unwrap();
    assert_eq!(read_requests(&env), vec!["PUT /api/reading/S1/a/read"]);
}

#[test]
fn duplicates_are_marked_and_sent_per_row() {
    let env = Env::new();
    let page = "https://them.example/f/A1";
    {
        let mut st = env.mock.state();
        st.read_sync = true;
        st.subs = vec![
            json!({ "id": "B", "kind": "blyg", "origin": "https://them.example/", "feed_url": "x",
                    "title": "Them", "status": "active", "in_blogroll": false }),
            json!({ "id": "F", "kind": "rss", "origin": "https://them.example/feed.xml", "feed_url": "x",
                    "title": "Them (feed)", "status": "active", "in_blogroll": false }),
        ];
        let mut rss = item("F", page, Some(page), 0, "2030-01-02T00:00:00Z");
        rss["origin"] = json!("https://them.example/feed.xml");
        st.reading = Some(vec![
            rss,
            item("B", "A1", Some("f/A1"), 1, "2030-01-01T00:00:00Z"),
        ]);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    assert_eq!(b.reading().len(), 1, "one entry per post");
    b.mark_read("B", "A1").unwrap();
    b.sync_now().unwrap();
    let mut reqs = read_requests(&env);
    reqs.sort();
    assert_eq!(
        reqs,
        vec![
            "PUT /api/reading/B/A1/read".to_string(),
            format!(
                "PUT /api/reading/F/{}/read",
                url::form_urlencoded::byte_serialize(page.as_bytes()).collect::<String>()
            ),
        ]
    );
    assert_eq!(server_read(&env, "B", "A1"), Some(1));
    assert_eq!(server_read(&env, "F", page), Some(0));

    // A second Mac sees both rows read.
    let env2_dir = tempfile::tempdir().unwrap();
    let b2 = LiveBackend::open_with(
        env2_dir.path(),
        &env.mock.url,
        TOKEN,
        SyncOptions {
            start_worker: false,
            ..fast()
        },
    )
    .unwrap();
    b2.sync_now().unwrap();
    let r = b2.reading();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].read_version, Some(1));
    assert!(!r[0].is_unread());
}

#[test]
fn repeated_reads_coalesce_to_the_highest_version() {
    let env = env_with(true);
    let b = env.manual();
    b.sync_now().unwrap();
    env.mock.set_down(true);
    b.mark_read("S1", "b").unwrap(); // v1
    // The post is edited (v3) and read again, all while offline.
    env.mock.set_down(false);
    env.mock.state().reading = Some(vec![
        item("S1", "a", Some("f/a"), 2, "2030-01-02T00:00:00Z"),
        item("S1", "b", Some("f/b"), 3, "2030-01-03T00:00:00Z"),
    ]);
    b.pull_now().unwrap();
    env.mock.set_down(true);
    b.mark_read("S1", "b").unwrap(); // v3
    env.mock.set_down(false);
    env.mock.state().log.clear();
    b.sync_now().unwrap();
    assert_eq!(
        read_requests(&env),
        vec!["PUT /api/reading/S1/b/read"],
        "one op per row, not one per mark"
    );
    assert_eq!(server_read(&env, "S1", "b"), Some(3));
}

#[test]
fn initial_batch_upload_happens_once() {
    // Read on this Mac while the server couldn't sync read state.
    let env = env_with(false);
    let b = env.manual();
    b.sync_now().unwrap();
    b.mark_read("S1", "a").unwrap();
    b.mark_read("S1", "b").unwrap();
    b.sync_now().unwrap();
    assert!(read_requests(&env).is_empty());

    // The Worker is patched: the next pull uploads everything, in one batch.
    env.mock.state().read_sync = true;
    b.sync_now().unwrap();
    {
        let st = env.mock.state();
        assert_eq!(st.read_batches.len(), 1);
        let mut items = st.read_batches[0]["items"].as_array().unwrap().clone();
        items.sort_by_key(|v| v["remote_id"].as_str().unwrap().to_string());
        assert_eq!(
            items,
            vec![
                json!({ "sub": "S1", "remote_id": "a", "version": 2 }),
                json!({ "sub": "S1", "remote_id": "b", "version": 1 }),
            ]
        );
    }
    assert_eq!(server_read(&env, "S1", "a"), Some(2));

    // Never again, including after a restart.
    b.sync_now().unwrap();
    drop(b);
    let b = env.manual();
    b.sync_now().unwrap();
    assert_eq!(env.mock.state().read_batches.len(), 1);
    assert_eq!(
        env.mock.state().count("POST /api/reading/read"),
        1,
        "{:?}",
        read_requests(&env)
    );
}

#[test]
fn initial_upload_is_chunked_at_500() {
    let env = Env::new();
    {
        let mut st = env.mock.state();
        st.subs = vec![
            json!({ "id": "S1", "kind": "blyg", "origin": "https://them.example/",
            "feed_url": "x", "title": "Them", "status": "active", "in_blogroll": false }),
        ];
        st.reading = Some(
            (0..1001)
                .map(|i| item("S1", &format!("r{i:04}"), None, 1, "2030-01-01T00:00:00Z"))
                .collect(),
        );
    }
    let b = env.open(SyncOptions {
        start_worker: false,
        ..fast()
    });
    b.sync_now().unwrap();
    for r in b.reading() {
        b.mark_read(&r.subscription_id, &r.remote_id).unwrap();
    }
    env.mock.state().read_sync = true;
    b.sync_now().unwrap();
    let st = env.mock.state();
    let sizes: Vec<usize> = st
        .read_batches
        .iter()
        .map(|b| b["items"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, vec![500, 500, 1]);
    assert_eq!(st.reads.len(), 1001);
}

#[test]
fn reads_made_while_unsupported_catch_up_after_the_upload() {
    let env = env_with(true);
    let b = env.manual();
    b.sync_now().unwrap(); // capability seen, (empty) upload done
    // The server loses the capability for a while (rolled back)...
    env.mock.state().read_sync = false;
    b.sync_now().unwrap();
    b.mark_read("S1", "a").unwrap();
    b.sync_now().unwrap();
    assert!(read_requests(&env).iter().all(|l| l.starts_with("POST")));
    // ...and gets it back: the row ahead of the server is sent.
    env.mock.state().read_sync = true;
    b.sync_now().unwrap(); // pull queues it
    b.sync_now().unwrap(); // flush sends it
    assert_eq!(server_read(&env, "S1", "a"), Some(2));
}

#[test]
fn capability_absent_means_zero_requests_to_the_new_endpoints() {
    let env = env_with(false);
    let b = env.manual();
    b.sync_now().unwrap();
    b.mark_read("S1", "a").unwrap();
    b.mark_read("S1", "b").unwrap();
    assert_eq!(b.sync_status(), SyncStatus::Synced, "nothing queued");
    b.sync_now().unwrap();
    b.sync_now().unwrap();
    assert!(read_requests(&env).is_empty(), "{:?}", read_requests(&env));
    assert_eq!(read_version(&b, "a"), Some(2), "local read state as before");
    assert!(
        !env.events()
            .iter()
            .any(|e| matches!(e, CoreEvent::Error(_)))
    );
}

#[test]
fn no_reading_endpoint_means_no_read_sync() {
    let env = Env::new(); // patch 3 absent too
    let b = env.manual();
    b.sync_now().unwrap();
    assert!(matches!(b.mark_read("S1", "a"), Err(CoreError::NotFound)));
    b.sync_now().unwrap();
    assert!(read_requests(&env).is_empty());
}

#[test]
fn a_404_from_the_endpoint_turns_read_sync_off_quietly() {
    let env = env_with(true);
    let b = env.manual();
    b.sync_now().unwrap();
    // The Worker is downgraded between the pull and the flush: the reading
    // list still says read_state, but the write 404s. (Simulated by turning
    // the flag off only for writes: flip it, mark, flush.)
    b.mark_read("S1", "a").unwrap();
    env.mock.state().read_sync = false; // writes 404 now
    b.sync_now().unwrap();
    assert!(
        !env.events()
            .iter()
            .any(|e| matches!(e, CoreEvent::Error(_))),
        "a missing feature isn't an error"
    );
    assert_eq!(b.sync_status(), SyncStatus::Synced);
    // And nothing more is queued.
    env.mock.state().log.clear();
    b.mark_read("S1", "b").unwrap();
    b.sync_now().unwrap();
    assert!(read_requests(&env).is_empty());
}
