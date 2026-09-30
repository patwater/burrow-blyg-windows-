//! Reading list, subscriptions, signals, mentions, settings — including the
//! patch-3 endpoints being absent (404) on an older server.

mod common;

use std::time::Duration;

use blyg_core::*;
use common::*;
use serde_json::{Value, json};

fn reading_item(
    sub: &str,
    rid: &str,
    origin: &str,
    page: Option<&str>,
    version: u32,
    obs: &str,
    body: &str,
) -> Value {
    json!({
        "subscription_id": sub, "remote_id": rid, "subscription_title": "Them", "origin": origin,
        "kind": "fragment", "state": "current", "version": version, "created": null, "updated": null,
        "observed_at": obs, "content_md": body, "content_html": format!("<p>{body}</p>"),
        "author": { "name": "Them", "url": null }, "page": page, "thumb": null, "hoppers": []
    })
}

#[test]
fn patch3_404_degrades_cleanly() {
    let env = Env::new(); // reading/mentions/settings/hoppers all None → 404
    let b = env.manual();
    env.mock.state().add_item("fragment", "hello");
    b.sync_now().unwrap();
    assert_eq!(b.items().len(), 1, "items still sync");
    assert!(b.reading().is_empty());
    assert!(!b.reading_available());
    assert_eq!(b.mentions().unwrap(), vec![]);
    assert_eq!(b.hoppers().unwrap(), vec![]);
    assert!(matches!(b.settings(), Err(CoreError::NotFound)));
    assert_eq!(
        b.sync_status(),
        SyncStatus::Synced,
        "a missing feature isn't a sync error"
    );
    assert!(
        !env.events()
            .iter()
            .any(|e| matches!(e, CoreEvent::Error(_)))
    );
}

#[test]
fn patch3_404_with_the_worker_is_not_an_error_loop() {
    let env = Env::new();
    let b = env.open(SyncOptions {
        pull_interval: Duration::from_millis(100),
        ..fast()
    });
    std::thread::sleep(Duration::from_millis(550));
    let reads = env.mock.state().count("GET /api/reading");
    // one attempt per pull interval (~5), not a tight retry loop
    assert!((1..=8).contains(&reads), "{reads} reading requests");
    assert_eq!(b.sync_status(), SyncStatus::Synced);
    drop(b);
}

#[test]
fn reading_pages_with_opaque_cursor_and_absolute_pages() {
    let env = Env::new();
    {
        let mut st = env.mock.state();
        st.reading_page_size = 2;
        st.reading = Some(
            (0..5)
                .map(|i| {
                    reading_item(
                        "S1",
                        &format!("R{i}"),
                        "https://them.example/",
                        Some(&format!("f/R{i}")),
                        1,
                        &format!("2030-01-0{}T00:00:00Z", 9 - i),
                        "text",
                    )
                })
                .collect(),
        );
        st.subs = vec![
            json!({ "id": "S1", "kind": "blyg", "origin": "https://them.example/",
            "feed_url": "https://them.example/feed.xml", "title": "Them", "status": "active", "in_blogroll": true }),
        ];
    }
    let b = env.manual();
    b.sync_now().unwrap();
    let r = b.reading();
    assert_eq!(r.len(), 5, "all pages followed");
    assert_eq!(r[0].remote_id, "R0", "newest observed first");
    assert_eq!(
        r[0].page.as_deref(),
        Some("https://them.example/f/R0"),
        "page resolved against origin"
    );
    assert!(b.reading_available());
    assert!(env.events().contains(&CoreEvent::ReadingChanged));
    let st = env.mock.state();
    assert!(
        st.log
            .iter()
            .any(|l| l == "GET /api/reading?limit=500&before=c%3A2"),
        "{:?}",
        st.log
    );
    drop(st);
    assert_eq!(b.subscriptions().len(), 1);
    assert!(b.subscriptions()[0].in_blogroll);
}

#[test]
fn reading_one_entry_per_post_across_edits_and_feeds() {
    let env = Env::new();
    let page = "https://them.example/f/A1";
    {
        let mut st = env.mock.state();
        st.subs = vec![
            json!({ "id": "B", "kind": "blyg", "origin": "https://them.example/", "feed_url": "x",
                    "title": "Them", "status": "active", "in_blogroll": false }),
            json!({ "id": "F", "kind": "rss", "origin": "https://them.example/feed.xml", "feed_url": "x",
                    "title": "Them (feed)", "status": "active", "in_blogroll": false }),
        ];
        st.reading = Some(vec![
            reading_item(
                "F",
                page,
                "https://them.example/feed.xml",
                Some(page),
                0,
                "2030-01-02T00:00:00Z",
                "via rss",
            ),
            reading_item(
                "B",
                "A1",
                "https://them.example/",
                Some("f/A1"),
                1,
                "2030-01-01T00:00:00Z",
                "via blyg",
            ),
        ]);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    let r = b.reading();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].subscription_id, "B", "blyg row preferred");
    b.mark_read("B", "A1").unwrap();

    // the author edits it: the server bumps version/observed_at on the same row
    env.mock.state().reading = Some(vec![
        reading_item(
            "B",
            "A1",
            "https://them.example/",
            Some("f/A1"),
            2,
            "2030-01-03T00:00:00Z",
            "edited",
        ),
        reading_item(
            "F",
            page,
            "https://them.example/feed.xml",
            Some(page),
            0,
            "2030-01-03T00:00:01Z",
            "edited",
        ),
    ]);
    b.sync_now().unwrap();
    let r = b.reading();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].content_md, "edited");
    assert!(r[0].edited_since_read());
    assert_eq!(r[0].read_version, Some(1), "only the number is kept");
}

#[test]
fn signals_update_the_cache() {
    let env = Env::new();
    env.mock.state().reading = Some(vec![reading_item(
        "S",
        "R",
        "https://o/",
        None,
        1,
        "2030-01-01T00:00:00Z",
        "x",
    )]);
    let b = env.manual();
    b.sync_now().unwrap();
    b.signal("S", "R", Some(1)).unwrap();
    assert_eq!(b.reading()[0].thumb, Some(1));
    assert_eq!(
        env.mock.state().signals[&("S".to_string(), "R".to_string())],
        1
    );
    b.signal("S", "R", None).unwrap();
    assert_eq!(b.reading()[0].thumb, None);
    assert!(env.mock.state().signals.is_empty());
    assert!(matches!(
        b.signal("S", "R", Some(3)),
        Err(CoreError::Rejected { .. })
    ));
}

#[test]
fn subscriptions_lifecycle() {
    let env = Env::new();
    let b = env.manual();
    let p = b.preview_subscription("https://them.example/").unwrap();
    assert_eq!(p.kind, SubscriptionKind::Blyg);
    assert_eq!(p.title, "Their blyg");
    assert_eq!(p.site_mismatch, Some(false));
    assert!(
        env.mock.state().subs.is_empty(),
        "preview has no side effects"
    );
    match b.preview_subscription("https://nowhere.example/") {
        Err(CoreError::Rejected { status: 422, .. }) => {}
        other => panic!("{:?}", other.err()),
    }

    let s = b.subscribe("https://them.example/", Some("Them")).unwrap();
    assert_eq!(s.title, "Them");
    assert_eq!(b.subscriptions().len(), 1);
    b.set_subscription(&s.id, Some(true), Some("Them, renamed"))
        .unwrap();
    assert!(b.subscriptions()[0].in_blogroll);
    assert_eq!(b.subscriptions()[0].title, "Them, renamed");
    b.pause_subscription(&s.id, true).unwrap();
    assert_eq!(b.subscriptions()[0].status, "paused");
    assert_eq!(env.mock.state().subs[0]["status"], "paused");
    b.pause_subscription(&s.id, false).unwrap();
    assert_eq!(b.subscriptions()[0].status, "active");
    b.unsubscribe(&s.id).unwrap();
    assert!(b.subscriptions().is_empty());
    assert!(matches!(b.unsubscribe("gone"), Err(CoreError::NotFound)));
}

#[test]
fn mentions_and_settings_when_available() {
    let env = Env::new();
    {
        let mut st = env.mock.state();
        st.mentions = Some(vec![json!({
            "id": "m1", "target_item_id": "T", "status": "verified", "relation": "stub",
            "source": "https://else.example/t/X", "source_origin": "https://else.example/", "source_id": "X",
            "source_kind": "thread", "source_version": 1, "source_author": { "name": null, "url": null },
            "first_seen": "2030-01-01T00:00:00Z", "verified_at": null, "hidden": false
        })]);
        st.settings = Some(json!({
            "site_title": "blyg", "author_name": "Robin Example", "author_bio": "", "site_url": "",
            "theme": "auto", "avatar_media_id": "", "author_links": [{ "label": "gh", "url": "https://github.com/x" }]
        }));
        st.hoppers = Some(vec![
            json!({ "id": "h1", "name": "Later", "slug": "later", "public": false, "count": 3 }),
        ]);
    }
    let b = env.manual();
    let m = b.mentions().unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].relation.as_deref(), Some("stub"));
    b.set_mention_hidden("m1", true).unwrap();
    assert!(env.mock.state().hidden["m1"]);

    let s = b.settings().unwrap();
    assert_eq!(s.site_title.as_deref(), Some("blyg"));
    assert_eq!(s.author_name.as_deref(), Some("Robin Example"));
    assert_eq!(s.author_bio, None, "\"\" → None");
    assert_eq!(s.site_url, None);
    assert_eq!(s.theme.as_deref(), Some("auto"));
    assert_eq!(s.author_links.len(), 1);

    let mut edited = s.clone();
    edited.author_bio = Some("Writes things.".into());
    b.save_settings(&edited).unwrap();
    let put = env.mock.state().settings_puts[0].clone();
    assert_eq!(put["author_bio"], "Writes things.");
    assert!(put.get("site_url").is_none(), "unset fields aren't sent");
    assert_eq!(put["author_links"][0]["label"], "gh");

    assert_eq!(b.hoppers().unwrap()[0].count, 3);
}
