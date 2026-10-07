//! A stock blygger-studio: no `/api/reading/imported` (nor the fork's other
//! extensions). The reading list is assembled from upstream's own routes and
//! must come out the same as the fork's rows.

mod common;

use blyg_core::*;
use common::*;
use serde_json::{Value, json};

const ORIGIN: &str = "https://them.example.com/";

fn seed(env: &Env, stock: bool) {
    let mut st = env.mock.state();
    st.stock = stock;
    st.subs = vec![json!({ "id": "S1", "kind": "blyg", "origin": ORIGIN, "title": "Them" })];
    st.hoppers = Some(vec![
        json!({ "id": "H1", "name": "Later", "slug": null, "public": false }),
    ]);
    st.reading = Some(vec![
        json!({ "subscription_id": "S1", "remote_id": "A", "subscription_title": "Them", "origin": ORIGIN,
            "kind": "fragment", "state": "current", "version": 2, "created": "2030-01-01T00:00:00Z",
            "updated": "2030-01-02T00:00:00Z", "observed_at": "2030-01-03T00:00:00Z",
            "content_md": "First *post*", "content_html": "<p>First <em>post</em></p>",
            "author": { "name": "Them", "url": "https://them.example.com/" }, "page": "f/A/",
            "thumb": 1, "hoppers": ["H1"] }),
        json!({ "subscription_id": "S1", "remote_id": "B", "subscription_title": "Them", "origin": ORIGIN,
            "kind": "thread", "state": "current", "version": 1, "created": null, "updated": null,
            "observed_at": "2030-01-02T00:00:00Z", "content_md": "A thread", "content_html": "<p>A thread</p>",
            "author": null, "page": null, "thumb": null, "hoppers": [],
            "transclusions": [{ "id": "A", "version": 2, "origin": ORIGIN }] }),
        json!({ "subscription_id": "S1", "remote_id": "C", "subscription_title": "Them", "origin": ORIGIN,
            "kind": "fragment", "state": "tombstone", "version": 3, "created": null, "updated": null,
            "observed_at": "2030-01-01T00:00:00Z", "content_md": "kept", "content_html": "<p>kept</p>",
            "author": null, "page": "f/C/", "thumb": -1, "hoppers": [], "pinned_version_retained": 1 }),
    ]);
}

/// Every reading row the store holds, raw (tombstones included), in key
/// order. `lineage_known` says where a row came from, so it's left out.
fn rows(b: &LiveBackend) -> Vec<Value> {
    let mut v: Vec<Value> = ["A", "B", "C"]
        .iter()
        .filter_map(|rid| b.reading_row("S1", rid))
        .map(|r| {
            let mut v = serde_json::to_value(r).unwrap();
            v.as_object_mut().unwrap().remove("read_version");
            v.as_object_mut().unwrap().remove("lineage_known");
            v
        })
        .collect();
    v.sort_by_key(|r| r["remote_id"].as_str().unwrap_or("").to_string());
    v
}

#[test]
fn a_stock_server_gives_the_same_reading_rows_as_the_fork() {
    let fork = Env::new();
    seed(&fork, false);
    let a = fork.manual();
    a.sync_now().unwrap();

    let stock = Env::new();
    seed(&stock, true);
    let b = stock.manual();
    b.sync_now().unwrap();

    let (ra, rb) = (rows(&a), rows(&b));
    assert_eq!(ra.len(), 3, "{ra:?}");
    assert_eq!(ra, rb);
    assert_eq!(a.server_extensions(), Some(true));
    assert_eq!(b.server_extensions(), Some(false));
}

#[test]
fn a_stock_server_is_explained_once_per_database() {
    let env = Env::new();
    seed(&env, true);
    let b = env.manual();
    b.sync_now().unwrap();
    b.sync_now().unwrap();
    let told = |env: &Env| {
        env.events()
            .iter()
            .filter(|e| matches!(e, CoreEvent::ServerLimited))
            .count()
    };
    assert_eq!(told(&env), 1);
    drop(b);
    let b = env.manual(); // relaunch, same database
    b.sync_now().unwrap();
    assert_eq!(told(&env), 1, "not again after a relaunch");

    let fork = Env::new();
    seed(&fork, false);
    fork.manual().sync_now().unwrap();
    assert_eq!(told(&fork), 0, "a server with the extensions says nothing");
}

#[test]
fn only_new_or_changed_posts_are_fetched_one_by_one() {
    let env = Env::new();
    seed(&env, true);
    let b = env.manual();
    b.sync_now().unwrap();
    let imports = |env: &Env| {
        env.mock
            .state()
            .log
            .iter()
            .filter(|l| l.starts_with("GET /api/imports/"))
            .count()
    };
    assert_eq!(imports(&env), 3);
    b.sync_now().unwrap();
    assert_eq!(imports(&env), 3, "nothing changed, nothing fetched");
    {
        let mut st = env.mock.state();
        let r = &mut st.reading.as_mut().unwrap()[1];
        // An edit: `updated` moves, `observed_at` (first sighting) doesn't.
        r["updated"] = json!("2030-02-01T00:00:00Z");
        r["content_md"] = json!("A thread, edited");
    }
    b.sync_now().unwrap();
    assert_eq!(imports(&env), 4, "only the edited one");
    assert_eq!(
        b.reading_row("S1", "B").unwrap().content_md,
        "A thread, edited"
    );
}

#[test]
fn lineage_arrives_from_the_posts_own_document() {
    let env = Env::new();
    // The subscribed blyg is the mock's public surface, under /them/.
    let origin = format!("{}/them/", env.mock.url);
    {
        let mut st = env.mock.state();
        st.stock = true;
        st.pre_lineage_imports = true;
        st.subs = vec![json!({ "id": "S1", "kind": "blyg", "origin": origin, "title": "Them" })];
        st.reading = Some(vec![
            json!({ "subscription_id": "S1", "remote_id": "R",
                "subscription_title": "Them", "origin": origin, "kind": "thread", "state": "current",
                "version": 1, "observed_at": "2030-01-01T00:00:00Z", "content_md": "A reply",
                "content_html": "<p>A reply</p>" }),
            // A newer post with no lineage: filling R mustn't drop it.
            json!({ "subscription_id": "S1", "remote_id": "N",
                "subscription_title": "Them", "origin": origin, "kind": "fragment", "state": "current",
                "version": 1, "observed_at": "2030-01-02T00:00:00Z", "content_md": "Newer",
                "content_html": "<p>Newer</p>" }),
        ]);
        st.public.insert(
            "/them/items/R.json".into(),
            json!({ "id": "R", "kind": "thread", "version": 1, "content_md": "A reply",
                "stub_of": { "origin": "https://ada.example.net/", "id": "X", "version": 2 },
                "versions": [{ "version": 1, "published_at": "2030-01-01T00:00:00Z" }] }),
        );
    }
    let b = env.open(SyncOptions {
        start_worker: false,
        lineage_fetches: 20,
        ..fast()
    });
    b.sync_now().unwrap();
    assert!(b.reading_row("S1", "N").is_some(), "the other rows stay");
    let r = b.reading_row("S1", "R").unwrap();
    assert_eq!(
        r.stub_of.as_ref().and_then(|s| s.origin.as_deref()),
        Some("https://ada.example.net/")
    );
    // Fetched once; later pulls keep it without asking again.
    b.sync_now().unwrap();
    assert!(b.reading_row("S1", "R").unwrap().stub_of.is_some());
    let docs = env
        .mock
        .state()
        .log
        .iter()
        .filter(|l| l.starts_with("GET /them/items/R.json"))
        .count();
    assert_eq!(docs, 1);
    // A studio before 0.18 is older than the vendored contract on this.
    env.mock
        .state()
        .violations
        .retain(|v| !v.contains("missing required `stub_of_json`"));
}

#[test]
fn an_edit_within_the_same_second_is_still_noticed() {
    let env = Env::new();
    seed(&env, true);
    let b = env.manual();
    b.sync_now().unwrap();
    {
        // Same timestamps (they resolve to whole seconds), new version.
        let mut st = env.mock.state();
        let r = &mut st.reading.as_mut().unwrap()[0];
        r["version"] = json!(3);
        r["content_md"] = json!("First post, v3");
        r["content_html"] = json!("<p>First post, v3</p>");
    }
    b.sync_now().unwrap();
    let r = b.reading_row("S1", "A").unwrap();
    assert_eq!((r.version, r.content_md.as_str()), (3, "First post, v3"));
}

#[test]
fn a_studio_since_0_18_sends_lineage_with_the_row() {
    let env = Env::new();
    let origin = format!("{}/them/", env.mock.url);
    {
        let mut st = env.mock.state();
        st.stock = true;
        st.subs = vec![json!({ "id": "S1", "kind": "blyg", "origin": origin, "title": "Them" })];
        st.reading = Some(vec![
            json!({ "subscription_id": "S1", "remote_id": "R",
                "subscription_title": "Them", "origin": origin, "kind": "thread", "state": "current",
                "version": 1, "observed_at": "2030-01-01T00:00:00Z", "content_md": "A reply",
                "content_html": "<p>A reply</p>",
                "stub_of": { "origin": "https://ada.example.net/", "id": "X", "version": 2 },
                "transclusions": [{ "id": "X", "version": 2, "origin": "https://ada.example.net/",
                    "selector": { "type": "TextQuoteSelector", "exact": "a line" } }] }),
            json!({ "subscription_id": "S1", "remote_id": "P",
                "subscription_title": "Them", "origin": origin, "kind": "fragment", "state": "current",
                "version": 1, "observed_at": "2030-01-01T00:00:00Z", "content_md": "Plain",
                "content_html": "<p>Plain</p>" }),
        ]);
    }
    let b = env.open(SyncOptions {
        start_worker: false,
        lineage_fetches: 20,
        ..fast()
    });
    b.sync_now().unwrap();
    let r = b.reading_row("S1", "R").unwrap();
    assert_eq!(r.stub_of.as_ref().and_then(|s| s.id.as_deref()), Some("X"));
    // A stub that quotes a passage of its target: one response, partial.
    let responses = b.responses("https://ada.example.net/", "X");
    assert_eq!(responses.len(), 1, "{responses:?}");
    assert_eq!(responses[0].relation, profile::Relation::Stubs);
    assert!(responses[0].partial);
    // The row said everything, the plain post included: no document fetched.
    let docs = env
        .mock
        .state()
        .log
        .iter()
        .filter(|l| l.contains("/them/items/"))
        .count();
    assert_eq!(docs, 0);
}

#[test]
fn the_forks_rows_carry_lineage_when_the_page_says_so() {
    let env = Env::new();
    let origin = format!("{}/them/", env.mock.url);
    {
        let mut st = env.mock.state();
        st.reading_lineage = true;
        st.subs = vec![json!({ "id": "S1", "kind": "blyg", "origin": origin, "title": "Them" })];
        st.reading = Some(vec![
            json!({ "subscription_id": "S1", "remote_id": "R",
                "subscription_title": "Them", "origin": origin, "kind": "thread", "state": "current",
                "version": 1, "observed_at": "2030-01-01T00:00:00Z", "content_md": "A reply",
                "content_html": "<p>A reply</p>",
                "forked_from": { "origin": "https://ada.example.net/", "id": "X", "version": 2 } }),
            json!({ "subscription_id": "S1", "remote_id": "P",
                "subscription_title": "Them", "origin": origin, "kind": "fragment", "state": "current",
                "version": 1, "observed_at": "2030-01-01T00:00:00Z", "content_md": "Plain",
                "content_html": "<p>Plain</p>" }),
        ]);
    }
    let b = env.open(SyncOptions {
        start_worker: false,
        lineage_fetches: 20,
        ..fast()
    });
    b.sync_now().unwrap();
    assert_eq!(b.server_extensions(), Some(true));
    let r = b.reading_row("S1", "R").unwrap();
    assert_eq!(r.forked_from.map(|f| f.id).as_deref(), Some("X"));
    // Both rows said everything: no public document fetched.
    let docs = env
        .mock
        .state()
        .log
        .iter()
        .filter(|l| l.contains("/them/items/"))
        .count();
    assert_eq!(docs, 0);
}
