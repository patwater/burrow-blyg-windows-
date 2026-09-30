//! Profiles (docs/SPEC.md § Profiles): someone's manifest, blogroll and
//! archive index, fetched from their public surface without a token, only
//! when a profile is opened, cached for offline use.

mod common;

use blyg_core::profile::Relation;
use blyg_core::*;
use common::*;
use serde_json::{Value, json};

const SUB: &str = "S";

/// A foreign blyg mounted at `/blyg/` on its own mock server.
fn foreign() -> (Mock, String) {
    let m = Mock::start();
    let origin = format!("{}/blyg/", m.url);
    (m, origin)
}

fn manifest(blogroll: bool) -> Value {
    let mut m = json!({
        "blyg": "0.3", "level": 1, "title": "JD's blyg", "site": "https://jd.example.org/",
        "author": { "name": "JD", "bio": "Tinkering in public.", "avatar": "media/avatar.png",
            "links": [{ "label": "site", "url": "https://jd.example.org/" }, { "label": "junk" }] },
        "feed": "feed.xml", "items": "items/index.json", "a_future_key": [1, 2, 3]
    });
    if blogroll {
        m["blogroll"] = json!("blogroll.opml");
    }
    m
}

const OPML: &str = r#"<?xml version="1.0"?><opml version="2.0"><head><title>b</title></head><body>
<outline type="rss" text="Ada" xmlUrl="https://ada.example.net/feed.xml" htmlUrl="https://ada.example.net/"/>
<outline text="Small Tools Weekly" xmlUrl="https://tools.example.com/feed.xml"/>
<outline text="broken" xmlUrl="not a url"/>
<outline"#;

fn feed(origin: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><rss version="2.0" xmlns:blyg="https://blygger.org/ns/0.1"><channel>
<title>JD</title><link>{origin}</link><blyg:manifest>{origin}blyg.json</blyg:manifest>
<item><title>tap tap tap: is this thing on?</title><link>{origin}t/A/</link><blyg:id>A</blyg:id>
<blyg:kind>thread</blyg:kind><blyg:version>2</blyg:version>
<description>&lt;blockquote class="blyg-transclusion" data-blyg-origin="https://lin.example.org/"&gt;q&lt;/blockquote&gt;</description></item>
</channel></rss>"#
    )
}

fn index() -> Value {
    json!({ "updated": "2030-01-03T00:00:00Z", "items": [
        { "id": "A", "kind": "thread", "created": "2030-01-01T00:00:00Z", "updated": "2030-01-03T00:00:00Z", "version": 2 },
        { "id": "B", "kind": "fragment", "created": "2030-01-01T00:00:00Z", "updated": "2030-01-02T00:00:00Z", "version": 1 },
        { "id": "C", "kind": "withdrawn", "updated": "2030-01-01T00:00:00Z", "version": 3 },
        { "junk": true }
    ]})
}

fn reading_row(rid: &str, origin: &str, body: &str, extra: Value) -> Value {
    let mut v = json!({
        "subscription_id": SUB, "remote_id": rid, "subscription_title": "JD", "origin": origin,
        "kind": "thread", "state": "current", "version": 1, "created": null,
        "updated": "2030-01-02T00:00:00Z", "observed_at": "2030-02-02T00:00:00Z",
        "content_md": body, "content_html": format!("<p>{body}</p>"),
        "author": { "name": "JD", "url": null }, "page": format!("t/{rid}/"), "thumb": null, "hoppers": []
    });
    if let (Value::Object(o), Value::Object(e)) = (&mut v, extra) {
        o.extend(e);
    }
    v
}

fn serve_full(f: &Mock, origin: &str) {
    let mut st = f.state();
    st.public.insert("/blyg/blyg.json".into(), manifest(true));
    st.public.insert("/blyg/items/index.json".into(), index());
    st.public_text
        .insert("/blyg/blogroll.opml".into(), OPML.to_string());
    st.public_text.insert("/blyg/feed.xml".into(), feed(origin));
}

/// The owner holds two of JD's posts: a stub of Ada, and a fork of Rue's pin.
fn owner(env: &Env, origin: &str, subscribed: bool) -> LiveBackend {
    {
        let mut st = env.mock.state();
        if subscribed {
            st.subs = vec![json!({ "id": SUB, "kind": "blyg", "origin": origin,
                "feed_url": format!("{origin}feed.xml"), "title": "JD", "status": "active", "in_blogroll": false })];
        }
        st.reading = Some(vec![
            reading_row(
                "B",
                origin,
                "Held title\n\nbody",
                json!({ "stub_of": { "origin": "https://ada.example.net/", "id": "X", "version": 1 } }),
            ),
            reading_row(
                "D",
                origin,
                "A fork",
                json!({ "forked_from": { "origin": "https://rue.example.com/", "id": "R", "version": 3 },
                        "stub_of": { "origin": "https://ada.example.net/", "id": "Y", "version": 1 },
                        "transclusions": "junk" }),
            ),
        ]);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    b
}

fn assert_no_credentials(f: &Mock) {
    let st = f.state();
    assert!(!st.public_headers.is_empty(), "something was fetched");
    for h in &st.public_headers {
        for (k, v) in h {
            let k = k.to_ascii_lowercase();
            assert!(
                k != "authorization" && k != "cookie" && k != "proxy-authorization",
                "a public fetch sent {k}"
            );
            assert!(!v.contains(TOKEN), "the token leaked in {k}");
        }
    }
    assert!(st.public_auth.iter().all(Option::is_none));
}

#[test]
fn a_blyg_profile_comes_from_its_public_files_without_credentials() {
    let env = Env::new();
    let (f, origin) = foreign();
    serve_full(&f, &origin);
    let b = owner(&env, &origin, false);

    let p = b.profile(&origin, false).unwrap();
    assert_eq!(p.kind, ProfileKind::Blyg);
    assert_eq!(p.origin, origin);
    assert!(!p.own && !p.stale);
    assert_eq!(p.name.as_deref(), Some("JD"));
    assert_eq!(p.bio.as_deref(), Some("Tinkering in public."));
    assert_eq!(
        p.avatar.as_deref(),
        Some(format!("{origin}media/avatar.png").as_str())
    );
    assert_eq!(p.links.len(), 1, "a link without a url is dropped");
    // Blogroll: odd entries skipped.
    let roll: Vec<&str> = p.blogroll.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(roll, ["Ada", "Small Tools Weekly"]);
    assert!(p.has_blogroll);
    // Posts: newest first, titled from the feed, then from what we hold.
    let titles: Vec<Option<&str>> = p.posts.iter().map(|x| x.title.as_deref()).collect();
    assert_eq!(
        titles,
        [
            Some("tap tap tap: is this thing on?"),
            Some("Held title"),
            None
        ]
    );
    assert_eq!(p.posts[2].kind, "withdrawn");
    // Connections: from held posts (stubs, forks) and the feed (quotes).
    let conn: Vec<(&str, Relation, u32)> = p
        .connections
        .iter()
        .map(|c| (c.origin.as_str(), c.relation, c.count))
        .collect();
    assert_eq!(
        conn,
        [
            ("https://ada.example.net/", Relation::Stubs, 2),
            ("https://lin.example.org/", Relation::Quotes, 1),
            ("https://rue.example.com/", Relation::Forks, 1),
        ]
    );
    // Discovery went through the subscribe preview (the owner's own API).
    assert!(
        env.mock
            .state()
            .log
            .iter()
            .any(|l| l == "POST /api/subscriptions")
    );
    let gets = f.state().log.clone();
    assert_eq!(
        gets,
        [
            "GET /blyg/blyg.json",
            "GET /blyg/blogroll.opml",
            "GET /blyg/items/index.json",
            "GET /blyg/feed.xml"
        ]
    );
    assert_no_credentials(&f);
}

#[test]
fn a_permalink_of_a_followed_blyg_opens_its_profile_without_a_preview() {
    let env = Env::new();
    let (f, origin) = foreign();
    serve_full(&f, &origin);
    let b = owner(&env, &origin, true);
    let before = env.mock.state().log.len();
    let p = b.profile(&format!("{origin}t/A/"), false).unwrap();
    assert_eq!(p.origin, origin);
    assert!(
        !env.mock.state().log[before..]
            .iter()
            .any(|l| l.contains("subscriptions")),
        "a subscription already says what it is"
    );
    // Cached under the origin as well as the permalink.
    assert!(b.cached_profile(&origin).is_some());
}

#[test]
fn missing_pieces_degrade_to_less_to_show() {
    let env = Env::new();
    let (f, origin) = foreign();
    // A manifest with no author, no blogroll; no index (404), no feed.
    f.state()
        .public
        .insert("/blyg/blyg.json".into(), json!({ "blyg": "0.2" }));
    let b = env.manual();
    let p = b.profile(&origin, false).unwrap();
    assert_eq!(p.name, None);
    assert_eq!(p.bio, None);
    assert_eq!(p.avatar, None);
    assert!(p.links.is_empty() && p.blogroll.is_empty() && p.posts.is_empty());
    assert!(!p.has_blogroll);
    assert!(
        !f.state().log.iter().any(|l| l.contains("blogroll")),
        "no blogroll key, no blogroll fetch"
    );
    // A listed blogroll that 404s, and an index that isn't one.
    {
        let mut st = f.state();
        st.public.insert("/blyg/blyg.json".into(), manifest(true));
        st.public
            .insert("/blyg/items/index.json".into(), json!("not an index"));
    }
    let p = b.profile(&origin, true).unwrap();
    assert_eq!(p.name.as_deref(), Some("JD"));
    assert!(p.has_blogroll && p.blogroll.is_empty() && p.posts.is_empty());
    assert_no_credentials(&f);
}

#[test]
fn no_manifest_is_an_error_not_a_crash() {
    let env = Env::new();
    let (_f, origin) = foreign();
    let b = env.manual();
    assert!(b.profile(&origin, false).is_err());
    assert!(b.cached_profile(&origin).is_none());
}

#[test]
fn a_plain_feed_gets_a_simple_card() {
    let env = Env::new();
    let (f, _) = foreign();
    let feed_url = format!("{}/omar/feed.xml", f.url);
    f.state().public_text.insert(
        "/omar/feed.xml".into(),
        r#"<rss version="2.0"><channel><title>Omar's notes</title><link>https://omar.example.org/</link>
<item><title>The year in review</title><link>https://omar.example.org/2030/review</link><pubDate>Tue, 01 Jan 2030 00:00:00 GMT</pubDate></item>
<item><title>Soil</title><link>https://omar.example.org/2029/soil</link></item>
</channel></rss>"#
            .into(),
    );
    let b = env.manual();
    let p = b.profile(&feed_url, false).unwrap();
    assert_eq!(p.kind, ProfileKind::Feed);
    assert_eq!(p.name.as_deref(), Some("Omar's notes"));
    assert_eq!(p.origin, "https://omar.example.org/");
    assert_eq!(p.feed_url.as_deref(), Some(feed_url.as_str()));
    assert_eq!(p.posts.len(), 2);
    assert_eq!(p.posts[0].title.as_deref(), Some("The year in review"));
    assert!(p.blogroll.is_empty() && p.connections.is_empty());
    assert_no_credentials(&f);
}

#[test]
fn without_a_preview_the_url_itself_is_probed() {
    let env = Env::new();
    let (f, _) = foreign();
    // The owner's preview can't resolve it (422); blyg.json at the URL can.
    let origin = format!("{}/nowhere/", f.url);
    f.state()
        .public
        .insert("/nowhere/blyg.json".into(), manifest(false));
    let b = env.manual();
    let p = b.profile(&origin, false).unwrap();
    assert_eq!(p.name.as_deref(), Some("JD"));
    assert_eq!(p.origin, origin);
}

#[test]
fn the_cache_serves_offline_and_fresh_opens_dont_refetch() {
    let env = Env::new();
    let (f, origin) = foreign();
    serve_full(&f, &origin);
    let b = owner(&env, &origin, true);
    b.profile(&origin, false).unwrap();
    let n = f.state().log.len();
    // Fresh: served from the cache, no request.
    let p = b.profile(&origin, false).unwrap();
    assert!(!p.stale);
    assert_eq!(f.state().log.len(), n, "a fresh profile isn't refetched");
    // Offline: an explicit refresh falls back to the cache, marked stale.
    f.set_down(true);
    let p = b.profile(&origin, true).unwrap();
    assert!(p.stale);
    assert_eq!(p.name.as_deref(), Some("JD"));
    // Survives a restart (SQLite), and the instant read needs no network.
    drop(b);
    let b = env.manual();
    let c = b.cached_profile(&origin).unwrap();
    assert_eq!(c.name.as_deref(), Some("JD"));
    assert_eq!(c.connections.len(), 3, "connections recomputed locally");
}

/// The owner API leaves lineage out of reading rows; connections use the
/// item documents already fetched (opened items), and never fetch more.
#[test]
fn connections_use_lineage_from_item_documents_already_fetched() {
    let env = Env::new();
    let (f, origin) = foreign();
    serve_full(&f, &origin);
    {
        let mut st = env.mock.state();
        st.subs = vec![json!({ "id": SUB, "kind": "blyg", "origin": origin,
            "feed_url": format!("{origin}feed.xml"), "title": "JD", "status": "active", "in_blogroll": false })];
        st.reading = Some(vec![reading_row("E", &origin, "Bare row", json!({}))]);
    }
    f.state().public.insert(
        "/blyg/items/E.json".into(),
        json!({ "blyg": "0.3", "id": "E", "kind": "thread", "version": 1,
                "updated": "2030-01-02T00:00:00Z", "changelog": [{ "version": 1, "at": "2030-01-02T00:00:00Z" }],
                "stub_of": { "origin": "https://ada.example.net/", "id": "X", "version": 1 },
                "forked_from": { "origin": "https://rue.example.com/", "id": "R", "version": 3 },
                "transclusions": [{ "id": "Q", "version": 1, "origin": "https://kit.example.com/" }, "junk"] }),
    );
    let b = env.manual();
    b.sync_now().unwrap();
    let rels = |p: &Profile| -> Vec<(String, Relation)> {
        p.connections
            .iter()
            .map(|c| (c.origin.clone(), c.relation))
            .collect()
    };
    // Not opened yet: the row alone has no lineage, and nothing is fetched for it.
    let p = b.profile(&origin, true).unwrap();
    assert!(
        !rels(&p)
            .iter()
            .any(|(o, _)| o == "https://rue.example.com/"),
        "{:?}",
        p.connections
    );
    assert!(!f.state().log.iter().any(|l| l.contains("/items/E")));
    // Opening the item fetches its document (and caches it).
    b.remote_versions(SUB, "E").unwrap();
    let n = f.state().log.len();
    let p = b.profile(&origin, true).unwrap();
    let got = rels(&p);
    for want in [
        ("https://ada.example.net/", Relation::Stubs),
        ("https://rue.example.com/", Relation::Forks),
        ("https://kit.example.com/", Relation::Quotes),
    ] {
        assert!(got.contains(&(want.0.to_string(), want.1)), "{got:?}");
    }
    let after: Vec<String> = f.state().log[n..].to_vec();
    assert!(
        !after.iter().any(|l| l.contains("/items/E")),
        "no extra item fetch: {after:?}"
    );
    assert_no_credentials(&f);
}

#[test]
fn nothing_is_fetched_until_a_profile_is_opened() {
    let env = Env::new();
    let (f, origin) = foreign();
    serve_full(&f, &origin);
    let b = owner(&env, &origin, true);
    b.sync_now().unwrap();
    b.pull_now().unwrap();
    // The background worker too.
    let live = env.open(fast());
    std::thread::sleep(std::time::Duration::from_millis(900));
    drop(live);
    assert!(
        f.state().log.is_empty(),
        "no profile fetch without an open: {:?}",
        f.state().log
    );
    assert!(b.cached_profile(&origin).is_none());
}

#[test]
fn your_own_profile_is_marked_and_its_connections_come_from_your_posts() {
    let env = Env::new();
    // Your own blyg is the mock itself (the API base).
    {
        let mut st = env.mock.state();
        st.public.insert("/blyg.json".into(), manifest(false));
        st.reading = Some(vec![reading_row(
            "01j9t4r7c8m2q5v6w3x8y9z0ab",
            "https://ada.example.net/",
            "Ada's post",
            json!({}),
        )]);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    // A draft quoting Ada isn't public yet: not a connection.
    b.create_draft(
        Kind::Thread,
        "Quoting Ada\n\n![[01j9t4r7c8m2q5v6w3x8y9z0ab]]\n",
    )
    .unwrap();
    let own = b.base_url().unwrap();
    let p = b.profile(&own, false).unwrap();
    assert!(p.own);
    assert_eq!(p.name.as_deref(), Some("JD"));
    assert!(p.connections.is_empty(), "{:?}", p.connections);
    // (Published posts' quotes, stubs and forks: profile::own_connections tests.)
    let _ = Relation::Quotes;
    // Public files of your own blyg are fetched without the token too.
    assert!(env.mock.state().public_auth.iter().all(Option::is_none));
}
