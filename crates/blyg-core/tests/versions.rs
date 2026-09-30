//! Versions & pins (spec §5.2, §8, §13.4; docs/SPEC.md "Versions & pins"):
//! other people's changelogs and pins from their public surface, fetched
//! unauthenticated; withdrawal retention; own-item version semantics.

mod common;

use blyg_core::*;
use common::*;
use serde_json::{Value, json};

const ID: &str = "p1";
const SUB: &str = "S";

/// A foreign blyg mounted at `/blyg/` on its own mock server.
fn foreign() -> (Mock, String) {
    let m = Mock::start();
    let origin = format!("{}/blyg/", m.url);
    (m, origin)
}

/// Public item document with a changelog of `(version, note, pinned)`.
fn item_doc(origin: &str, id: &str, current: &str, log: &[(u32, Option<&str>, bool)]) -> Value {
    let version = log.iter().map(|l| l.0).max().unwrap_or(1);
    let changelog: Vec<Value> = log
        .iter()
        .map(|(v, note, pinned)| {
            let mut e =
                json!({ "version": v, "at": format!("2030-01-0{v}T00:00:00Z"), "note": note });
            if *pinned {
                e["pinned"] = json!(true);
            }
            e
        })
        .collect();
    json!({
        "blyg": "0.2", "id": id, "kind": "fragment", "origin": origin, "page": format!("f/{id}/"),
        "author": { "name": "Them", "url": origin },
        "created": "2030-01-01T00:00:00Z", "updated": format!("2030-01-0{version}T00:00:00Z"),
        "version": version, "content_md": current, "content_html": format!("<p>{current}</p>"),
        "content_hash": content_hash(current), "media": [], "changelog": changelog,
        "some_future_field": { "ignored": true }
    })
}

fn pin_doc(origin: &str, id: &str, v: u32, body: &str, hash: Option<&str>) -> Value {
    json!({
        "blyg": "0.2", "id": id, "kind": "fragment", "version": v, "at": format!("2030-01-0{v}T00:00:00Z"),
        "note": "the good one", "pinned": true, "origin": origin, "author": { "name": "Them", "url": origin },
        "content_md": body, "content_html": format!("<p>{body}</p>"),
        "content_hash": hash.map(str::to_string).unwrap_or_else(|| content_hash(body)),
    })
}

fn reading_row(sub: &str, rid: &str, origin: &str, version: u32, body: &str) -> Value {
    json!({
        "subscription_id": sub, "remote_id": rid, "subscription_title": "Them", "origin": origin,
        "kind": "fragment", "state": "current", "version": version, "created": null,
        "updated": format!("2030-01-0{version}T00:00:00Z"), "observed_at": format!("2030-02-0{version}T00:00:00Z"),
        "content_md": body, "content_html": format!("<p>{body}</p>"),
        "author": { "name": "Them", "url": null }, "page": format!("f/{rid}/"), "thumb": null, "hoppers": []
    })
}

fn sub(id: &str, kind: &str, origin: &str) -> Value {
    json!({ "id": id, "kind": kind, "origin": origin, "feed_url": format!("{origin}feed.xml"),
            "title": "Them", "status": "active", "in_blogroll": false })
}

/// Owner env subscribed (blyg-kind) to a foreign blyg whose item `p1` is at
/// v3, with v1 pinned (and its pin file served) and v2 unpinned.
fn setup() -> (Env, Mock, String, LiveBackend) {
    let env = Env::new();
    let (f, origin) = foreign();
    {
        let mut st = f.state();
        st.public.insert(
            format!("/blyg/items/{ID}.json"),
            item_doc(
                &origin,
                ID,
                "third",
                &[
                    (1, Some("first!"), true),
                    (2, None, false),
                    (3, Some("typo"), false),
                ],
            ),
        );
        st.public.insert(
            format!("/blyg/items/{ID}/v1.json"),
            pin_doc(&origin, ID, 1, "first", None),
        );
    }
    {
        let mut st = env.mock.state();
        st.subs = vec![sub(SUB, "blyg", &origin)];
        st.reading = Some(vec![reading_row(SUB, ID, &origin, 3, "third")]);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    (env, f, origin, b)
}

fn public_gets(f: &Mock) -> Vec<String> {
    f.state().log.clone()
}

#[test]
fn changelog_comes_from_the_public_item_document_at_a_subdirectory_mount() {
    let (_env, f, _origin, b) = setup();
    let vs = b.remote_versions(SUB, ID).unwrap();
    assert_eq!(
        vs,
        vec![
            RemoteVersion {
                version: 1,
                at: "2030-01-01T00:00:00Z".into(),
                note: Some("first!".into()),
                pinned: true,
                current: false,
                media: vec![],
                lineage: Default::default(),
            },
            RemoteVersion {
                version: 2,
                at: "2030-01-02T00:00:00Z".into(),
                note: None,
                pinned: false,
                current: false,
                media: vec![],
                lineage: Default::default(),
            },
            RemoteVersion {
                version: 3,
                at: "2030-01-03T00:00:00Z".into(),
                note: Some("typo".into()),
                pinned: false,
                current: true,
                media: vec![],
                lineage: Default::default(),
            },
        ]
    );
    assert_eq!(public_gets(&f), vec![format!("GET /blyg/items/{ID}.json")]);
    // Cached (short TTL): asking again makes no request.
    b.remote_versions(SUB, ID).unwrap();
    assert_eq!(public_gets(&f).len(), 1);
}

#[test]
fn the_version_browser_shows_only_current_and_pinned() {
    let (_env, f, _origin, b) = setup();
    let shown = b.remote_shown_versions(SUB, ID).unwrap();
    assert_eq!(
        shown.iter().map(|v| v.version).collect::<Vec<_>>(),
        vec![1, 3],
        "unpinned v2 doesn't appear at all"
    );
    assert!(shown.iter().all(RemoteVersion::openable));
    // The full changelog is still there for the reading list's notes.
    assert_eq!(b.remote_versions(SUB, ID).unwrap().len(), 3);
    assert_eq!(public_gets(&f).len(), 1, "one fetch serves both");
}

#[test]
fn pinned_versions_open_and_are_cached_forever() {
    let (_env, f, origin, b) = setup();
    let p = b.remote_pinned(SUB, ID, 1).unwrap();
    assert_eq!(p.version, 1);
    assert_eq!(p.content_md, "first");
    assert_eq!(p.content_html, "<p>first</p>");
    assert_eq!(p.note.as_deref(), Some("the good one"));
    assert_eq!(p.origin, origin);
    assert_eq!(p.id, ID);
    assert_eq!(p.content_hash, content_hash("first"));
    assert!(!p.hash_mismatch);
    assert_eq!(p.url(), Some(format!("{origin}items/{ID}/v1.json")));
    assert_eq!(
        p.author.as_ref().and_then(|a| a.name.as_deref()),
        Some("Them")
    );
    let n = public_gets(&f).len();
    // Served from the cache even with the origin unreachable.
    f.set_down(true);
    assert_eq!(b.remote_pinned(SUB, ID, 1).unwrap(), p);
    assert_eq!(public_gets(&f).len(), n);
}

/// The Reading screen shows a pin by its published HTML (transclusion
/// snapshots baked in), fetched from the public document with no credentials.
#[test]
fn pinned_html_comes_from_the_public_document_without_credentials() {
    let (_env, f, origin, b) = setup();
    let html = "<p>first</p>\n<blockquote class=\"blyg-transclusion\" data-blyg-id=\"q\" \
                data-blyg-version=\"2\">\n<p>quoted</p>\n</blockquote>\n\
                <p><img src=\"media/a.png\" alt=\"\"></p>";
    let mut doc = pin_doc(&origin, ID, 1, "first\n\n![[q]]", None);
    doc["content_html"] = json!(html);
    f.state()
        .public
        .insert(format!("/blyg/items/{ID}/v1.json"), doc);
    let p = b.remote_pinned(SUB, ID, 1).unwrap();
    assert_eq!(
        p.content_html, html,
        "the HTML as the author's blyg served it"
    );

    let st = f.state();
    let at = st
        .log
        .iter()
        .position(|l| l == &format!("GET /blyg/items/{ID}/v1.json"))
        .expect("the pin document was fetched");
    let headers = &st.public_headers[at];
    let names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
    for banned in ["authorization", "cookie", "proxy-authorization"] {
        assert!(!names.contains(&banned), "sent {banned}: {headers:?}");
    }
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "accept" && v == "application/json"),
        "{headers:?}"
    );
}

/// Attached images (the item document's `media[]`) aren't in `content_html`;
/// they come with the current changelog row, resolved against the origin.
#[test]
fn attached_images_come_with_the_current_version() {
    let (_env, f, origin, b) = setup();
    let mut doc = item_doc(
        &origin,
        ID,
        "third",
        &[(1, Some("first!"), true), (3, None, false)],
    );
    doc["media"] = json!([
        { "url": "media/a1.png", "mime": "image/png", "alt": "A heron" },
        { "url": "https://cdn.example.com/b.jpg", "mime": "image/jpeg", "alt": "" },
        { "url": "javascript:alert(1)", "mime": "image/png" },
        { "url": "media/c.pdf", "mime": "application/pdf" },
        { "nonsense": true },
    ]);
    f.state()
        .public
        .insert(format!("/blyg/items/{ID}.json"), doc);
    let vs = b.remote_versions(SUB, ID).unwrap();
    let cur = vs.iter().find(|v| v.current).unwrap();
    assert_eq!(
        cur.media,
        vec![
            RemoteMedia {
                url: format!("{origin}media/a1.png"),
                alt: Some("A heron".into())
            },
            RemoteMedia {
                url: "https://cdn.example.com/b.jpg".into(),
                alt: None
            },
        ]
    );
    assert!(vs.iter().filter(|v| !v.current).all(|v| v.media.is_empty()));
    // Cached with the changelog.
    let n = public_gets(&f).len();
    assert_eq!(b.remote_versions(SUB, ID).unwrap(), vs);
    assert_eq!(public_gets(&f).len(), n);
}

/// The owner reading API leaves lineage out, so it's read from the public
/// item document (unauthenticated) and rides on the current changelog row.
#[test]
fn lineage_comes_from_the_item_document_leniently() {
    let (_env, f, origin, b) = setup();
    let mut doc = item_doc(&origin, ID, "third", &[(1, None, true), (3, None, false)]);
    doc["stub_of"] = json!({ "origin": "https://ada.example.net/", "id": "A", "version": 1 });
    doc["forked_from"] = json!({ "origin": "https://rue.example.com/", "id": "R", "version": 3 });
    doc["transclusions"] = json!([
        { "id": "Q", "version": 2, "origin": "https://lin.example.org/" },
        "junk",
        { "version": 4 },
        { "id": "L", "version": 1 },
    ]);
    f.state()
        .public
        .insert(format!("/blyg/items/{ID}.json"), doc);
    let vs = b.remote_versions(SUB, ID).unwrap();
    let cur = vs.iter().find(|v| v.current).unwrap();
    let l = &cur.lineage;
    assert_eq!(
        l.stub_of.as_ref().and_then(StubOf::target),
        Some("https://ada.example.net/")
    );
    assert_eq!(l.stub_of.as_ref().unwrap().id.as_deref(), Some("A"));
    assert_eq!(
        l.forked_from,
        Some(RemoteRef {
            origin: "https://rue.example.com/".into(),
            id: "R".into(),
            version: 3
        })
    );
    let ids: Vec<(&str, Option<&str>)> = l
        .transclusions
        .iter()
        .map(|t| (t.id.as_str(), t.origin.as_deref()))
        .collect();
    assert_eq!(ids, [("Q", Some("https://lin.example.org/")), ("L", None)]);
    assert!(
        vs.iter()
            .filter(|v| !v.current)
            .all(|v| v.lineage.is_empty())
    );
    assert_eq!(Lineage::of_changelog(&vs), Some(l));
    // The reading row itself has none; filling from the document gives it.
    let mut row = b.reading().into_iter().find(|r| r.remote_id == ID).unwrap();
    assert!(row.lineage().is_empty());
    row.fill_lineage(l);
    assert_eq!(row.forked_from.as_ref().unwrap().version, 3);
    // Cached with the changelog: no extra request.
    let n = public_gets(&f).len();
    assert_eq!(b.remote_versions(SUB, ID).unwrap(), vs);
    assert_eq!(public_gets(&f).len(), n);
    // Fetched without credentials.
    let st = f.state();
    let at = st
        .log
        .iter()
        .position(|l| l == &format!("GET /blyg/items/{ID}.json"))
        .unwrap();
    for (k, v) in &st.public_headers[at] {
        let k = k.to_ascii_lowercase();
        assert!(
            !["authorization", "cookie", "proxy-authorization"].contains(&k.as_str()),
            "sent {k}"
        );
        assert!(!v.contains(TOKEN), "the token leaked in {k}");
    }
    assert!(st.public_auth.iter().all(Option::is_none));
}

/// Odd lineage shapes never reject the item document.
#[test]
fn junk_lineage_shapes_are_tolerated() {
    let cases = [
        (
            json!("nope"),
            json!({ "origin": "https://rue.example.com/", "id": "R" }),
            json!({ "not": "a list" }),
        ),
        (json!(42), json!([1, 2]), json!("junk")),
        (
            json!({ "origin": 7 }),
            json!(null),
            json!([null, 3, { "id": 5 }]),
        ),
    ];
    for (stub, fork, tr) in cases {
        let (_env, f, origin, b) = setup();
        let mut doc = item_doc(&origin, ID, "third", &[(1, None, true), (3, None, false)]);
        doc["stub_of"] = stub;
        doc["forked_from"] = fork;
        doc["transclusions"] = tr;
        f.state()
            .public
            .insert(format!("/blyg/items/{ID}.json"), doc);
        let vs = b.remote_versions(SUB, ID).unwrap();
        assert_eq!(vs.len(), 2);
        assert!(vs.iter().all(|v| v.lineage.is_empty()), "{vs:?}");
        assert!(Lineage::of_changelog(&vs).is_none());
    }
    // A `{url}` stub (any web page) is kept.
    let (_env, f, origin, b) = setup();
    let mut doc = item_doc(&origin, ID, "third", &[(3, None, false)]);
    doc["stub_of"] = json!({ "url": "https://page.example.com/post" });
    f.state()
        .public
        .insert(format!("/blyg/items/{ID}.json"), doc);
    let vs = b.remote_versions(SUB, ID).unwrap();
    assert_eq!(
        Lineage::of_changelog(&vs)
            .and_then(|l| l.stub_of.as_ref())
            .and_then(StubOf::target),
        Some("https://page.example.com/post")
    );
}

#[test]
fn unpinned_versions_are_refused_without_a_request() {
    let (_env, f, _origin, b) = setup();
    for v in [2, 3, 9] {
        match b.remote_pinned(SUB, ID, v) {
            Err(CoreError::Rejected { status: 404, .. }) => {}
            other => panic!("v{v}: {other:?}"),
        }
    }
    let log = public_gets(&f);
    assert!(
        log.iter().all(|l| !l.contains("/v")),
        "no version file was requested: {log:?}"
    );
    assert_eq!(log.len(), 1, "one changelog fetch, then cache: {log:?}");
}

#[test]
fn a_pin_file_that_404s_means_not_pinned() {
    let (_env, f, _origin, b) = setup();
    f.state()
        .public
        .remove(&format!("/blyg/items/{ID}/v1.json"));
    match b.remote_pinned(SUB, ID, 1) {
        Err(CoreError::Rejected { status: 404, .. }) => {}
        other => panic!("{other:?}"),
    }
    assert!(
        public_gets(&f).contains(&format!("GET /blyg/items/{ID}/v1.json")),
        "the changelog said pinned, so it was asked for"
    );
    // The changelog that promised a pin is no longer trusted: refetched.
    b.remote_versions(SUB, ID).unwrap();
    let n = public_gets(&f)
        .iter()
        .filter(|l| l.ends_with(&format!("items/{ID}.json")))
        .count();
    assert_eq!(n, 2);
}

#[test]
fn a_hash_mismatch_is_flagged_not_fatal() {
    let (_env, f, origin, b) = setup();
    f.state().public.insert(
        format!("/blyg/items/{ID}/v1.json"),
        pin_doc(&origin, ID, 1, "first", Some("sha256:0000")),
    );
    let p = b.remote_pinned(SUB, ID, 1).unwrap();
    assert!(p.hash_mismatch);
    assert_eq!(p.content_md, "first", "content adopted anyway");
    assert_eq!(p.content_hash, "sha256:0000", "as served");
}

#[test]
fn the_owner_token_never_reaches_a_foreign_origin() {
    let (env, f, _origin, b) = setup();
    b.remote_versions(SUB, ID).unwrap();
    b.remote_pinned(SUB, ID, 1).unwrap();
    let _ = b.remote_pinned(SUB, ID, 2);
    let st = f.state();
    assert!(
        !st.public_auth.is_empty(),
        "the foreign origin was contacted"
    );
    assert!(
        st.public_auth.iter().all(Option::is_none),
        "authorization sent to a foreign origin: {:?}",
        st.public_auth
    );
    drop(st);
    // …while the owner's own blyg still got it on every API call.
    let own = env.mock.state();
    assert!(own.public_auth.is_empty());
    assert!(own.log.iter().all(|l| l.contains(" /api/")));
}

#[test]
fn rss_items_have_only_their_current_version() {
    let env = Env::new();
    let (f, origin) = foreign();
    {
        let mut st = env.mock.state();
        st.subs = vec![sub("F", "rss", &origin)];
        st.reading = Some(vec![reading_row("F", "guid-9", &origin, 2, "feed text")]);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    let vs = b.remote_versions("F", "guid-9").unwrap();
    assert_eq!(vs.len(), 1);
    assert_eq!(vs[0].version, 2);
    assert!(vs[0].current && !vs[0].pinned);
    assert!(matches!(
        b.remote_pinned("F", "guid-9", 2),
        Err(CoreError::Rejected { status: 404, .. })
    ));
    assert!(b.pinned_diff_base("F", "guid-9").is_none());
    assert!(
        public_gets(&f).is_empty(),
        "L0 has no public item documents"
    );
    assert!(matches!(
        b.remote_versions("F", "nope"),
        Err(CoreError::NotFound)
    ));
}

#[test]
fn offline_falls_back_to_the_last_changelog() {
    let (env, f, origin, b) = setup();
    b.remote_versions(SUB, ID).unwrap();
    // The author publishes v4: the cached changelog no longer covers it…
    env.mock.state().reading = Some(vec![reading_row(SUB, ID, &origin, 4, "fourth")]);
    b.sync_now().unwrap();
    // …but the origin is unreachable, so the old one answers.
    f.set_down(true);
    let vs = b.remote_versions(SUB, ID).unwrap();
    assert_eq!(vs.len(), 3);
}

#[test]
fn diff_base_only_when_the_version_read_was_pinned() {
    let env = Env::new();
    let (f, origin) = foreign();
    {
        let mut st = f.state();
        st.public.insert(
            "/blyg/items/a.json".into(),
            item_doc(
                &origin,
                "a",
                "a3",
                &[(1, None, true), (2, None, false), (3, None, false)],
            ),
        );
        st.public.insert(
            "/blyg/items/a/v1.json".into(),
            pin_doc(&origin, "a", 1, "a1", None),
        );
        st.public.insert(
            "/blyg/items/b.json".into(),
            item_doc(
                &origin,
                "b",
                "b3",
                &[(1, None, true), (2, None, false), (3, None, false)],
            ),
        );
    }
    {
        let mut st = env.mock.state();
        st.subs = vec![sub(SUB, "blyg", &origin)];
        st.reading = Some(vec![
            reading_row(SUB, "a", &origin, 1, "a1"),
            reading_row(SUB, "b", &origin, 2, "b2"),
        ]);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    assert!(b.pinned_diff_base(SUB, "a").is_none(), "unread: no base");
    b.mark_read(SUB, "a").unwrap(); // read at v1 (pinned)
    b.mark_read(SUB, "b").unwrap(); // read at v2 (not pinned)
    env.mock.state().reading = Some(vec![
        reading_row(SUB, "a", &origin, 3, "a3"),
        reading_row(SUB, "b", &origin, 3, "b3"),
    ]);
    b.sync_now().unwrap();

    let r = b.reading();
    assert!(r.iter().all(|r| r.edited_since_read()));
    let base = b.pinned_diff_base(SUB, "a").expect("pinned v1 is public");
    assert_eq!((base.version, base.content_md.as_str()), (1, "a1"));
    assert!(
        b.pinned_diff_base(SUB, "b").is_none(),
        "v2 was never public"
    );
    let log = public_gets(&f);
    assert!(
        !log.iter().any(|l| l.contains("/b/v")),
        "no request for b's unpinned history: {log:?}"
    );
}

#[test]
fn withdrawal_keeps_only_a_cached_pin() {
    let (env, _f, origin, b) = setup();
    b.remote_pinned(SUB, ID, 1).unwrap(); // cached
    let mut gone = reading_row(SUB, ID, &origin, 4, "");
    gone["state"] = json!("tombstone");
    gone["content_html"] = json!("");
    gone["thumb"] = json!(1);
    let mut gone2 = reading_row(SUB, "other", &origin, 2, "LEAKED TEXT");
    gone2["state"] = json!("tombstone");
    gone2["thumb"] = json!(1);
    env.mock.state().reading = Some(vec![gone, gone2]);
    b.sync_now().unwrap();
    let r = b.reading();
    let kept = r.iter().find(|r| r.remote_id == ID).unwrap();
    assert_eq!(kept.state, "tombstone");
    assert_eq!(
        kept.content_md, "first",
        "the pinned version, not the latest"
    );
    assert_eq!(kept.pinned_version_retained, Some(1));
    assert_eq!(kept.pin_url(), Some(format!("{origin}items/{ID}/v1.json")));
    let other = r.iter().find(|r| r.remote_id == "other").unwrap();
    assert_eq!(other.content_md, "");
    assert_eq!(other.content_html, "");
}

#[test]
fn server_retained_pins_survive_and_unmarked_tombstones_stay_hidden() {
    let env = Env::new();
    let (_f, origin) = foreign();
    let mut retained = reading_row(SUB, "r", &origin, 3, "pinned v2 body");
    retained["state"] = json!("tombstone");
    retained["pinned_version_retained"] = json!(2);
    retained["hoppers"] = json!(["keep"]);
    let mut hidden = reading_row(SUB, "h", &origin, 2, "");
    hidden["state"] = json!("tombstone");
    {
        let mut st = env.mock.state();
        st.subs = vec![sub(SUB, "blyg", &origin)];
        st.reading = Some(vec![retained, hidden]);
    }
    let b = env.manual();
    b.sync_now().unwrap();
    let r = b.reading();
    assert_eq!(r.len(), 1, "an unsignalled, unhoppered tombstone is hidden");
    assert_eq!(r[0].content_md, "pinned v2 body");
    assert_eq!(r[0].pinned_version_retained, Some(2));
}

// ---------------------------------------------------------------- own items

#[test]
fn own_versions_expose_pinned_and_endcap_and_endcaps_cannot_be_pinned() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "one").unwrap();
    b.publish(&id, Some("first")).unwrap();
    b.save(&id, "two").unwrap();
    b.publish(&id, None).unwrap();
    b.withdraw(&id, None).unwrap();
    let vs = b.versions(&id).unwrap();
    assert_eq!(
        vs.iter()
            .map(|v| (v.version, v.pinned, v.endcap))
            .collect::<Vec<_>>(),
        vec![(1, false, false), (2, false, false), (3, false, true)]
    );

    let pins_before = env.mock.state().count("POST /api/items/");
    match b.pin(&id, 3) {
        Err(CoreError::Rejected {
            status: 409,
            message,
            ..
        }) => {
            assert!(message.contains("endcap"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        env.mock.state().count("POST /api/items/"),
        pins_before,
        "refused locally, no request"
    );

    b.pin(&id, 2).unwrap();
    let vs = b.versions(&id).unwrap();
    assert!(vs[1].pinned && !vs[0].pinned && !vs[2].pinned);
}

#[test]
fn pin_refuses_an_endcap_the_local_cache_did_not_know_about() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "one").unwrap();
    b.publish(&id, None).unwrap();
    let sid = b.item(&id).unwrap().server_id.unwrap().0;
    // Withdrawn from another device; this client hasn't seen v2 yet.
    {
        let mut st = env.mock.state();
        let now = st.now();
        let it = st.items.get_mut(&sid).unwrap();
        it.version = 2;
        it.status = "withdrawn".into();
        it.versions
            .push(json!({ "version": 2, "published_at": now, "note": null,
            "pinned": false, "endcap": true }));
    }
    assert!(matches!(
        b.pin(&id, 2),
        Err(CoreError::Rejected { status: 409, .. })
    ));
    assert_eq!(
        env.mock
            .state()
            .count(&format!("POST /api/items/{sid}/pin")),
        0
    );
}

#[test]
fn restore_only_loads_the_working_copy() {
    let env = Env::new();
    let b = env.manual();
    let id = b.create_draft(Kind::Fragment, "one").unwrap();
    b.publish(&id, None).unwrap();
    b.save(&id, "two").unwrap();
    b.publish(&id, None).unwrap();
    let publishes = env.mock.state().count("POST /api/items/");
    b.restore(&id, 1).unwrap();
    let it = b.item(&id).unwrap();
    assert_eq!(it.content_md, "restored v1", "in the editor");
    assert_eq!(it.version, 2, "versions never go backwards");
    assert_eq!(it.status, Status::Public);
    assert!(it.dirty, "unpublished edits until the user publishes");
    let st = env.mock.state();
    assert_eq!(
        st.count("POST /api/items/"),
        publishes + 1,
        "only the restore call: {:?}",
        st.log
    );
    assert!(st.log.last().unwrap().starts_with("GET /api/items/"));
    let sid = it.server_id.unwrap().0;
    assert_eq!(st.items[&sid].version, 2, "nothing was published");
}
