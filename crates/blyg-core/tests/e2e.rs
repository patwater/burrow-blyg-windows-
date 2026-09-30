//! End-to-end tests against a REAL local blyg Worker (`wrangler dev`).
//!
//! Every test is `#[ignore]`, so `cargo test` never needs a Worker. Run them
//! with `scripts/e2e-local.sh`, which starts the Worker(s) and sets:
//!
//! - `BLYG_E2E_URL`   the Worker under test (e.g. `http://127.0.0.1:18787`)
//! - `BLYG_E2E_TOKEN` its `BLYG_OWNER_TOKEN`
//! - `BLYG_E2E_URL_B` a second, independent Worker (the subscription source)
//! - `BLYG_E2E_CTL`   `scripts/e2e-wrangler.sh`, to stop/start instance `a`
//!   mid-test (the offline case)
//!
//! Run with `--test-threads=1`: the offline test takes the Worker down.
//! Never point these at a real blyg: they publish, withdraw and pin.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use blyg_core::api::{Api, ConnectError, verify_connection};
use blyg_core::*;
use serde_json::{Value, json};

// ---------------------------------------------------------------- harness

struct E2e {
    url: String,
    token: String,
}

fn e2e() -> E2e {
    let url = std::env::var("BLYG_E2E_URL")
        .expect("BLYG_E2E_URL not set: run scripts/e2e-local.sh")
        .trim_end_matches('/')
        .to_string();
    assert!(
        url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost"),
        "e2e tests only ever run against a local Worker, not {url}"
    );
    let token = std::env::var("BLYG_E2E_TOKEN").expect("BLYG_E2E_TOKEN not set");
    E2e { url, token }
}

fn url_b() -> String {
    let url = std::env::var("BLYG_E2E_URL_B")
        .expect("BLYG_E2E_URL_B not set: run scripts/e2e-local.sh")
        .trim_end_matches('/')
        .to_string();
    assert!(url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost"));
    url
}

fn opts(worker: bool) -> SyncOptions {
    SyncOptions {
        debounce: Duration::from_millis(150),
        pull_interval: Duration::from_secs(3600),
        backoff_initial: Duration::from_millis(200),
        backoff_max: Duration::from_millis(800),
        start_worker: worker,
        reading_pages: 4,
    }
}

fn backend_at(dir: &Path, url: &str, token: &str, worker: bool) -> LiveBackend {
    LiveBackend::open_with(dir, url, token, opts(worker)).expect("open LiveBackend")
}

fn manual(dir: &Path) -> LiveBackend {
    let e = e2e();
    backend_at(dir, &e.url, &e.token, false)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .build()
}

/// Unauthenticated GET → (status, body).
fn public_get(url: &str) -> (u16, String) {
    match agent().get(url).call() {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(c, r)) => (c, r.into_string().unwrap_or_default()),
        Err(e) => panic!("GET {url}: {e}"),
    }
}

fn public_json(url: &str) -> Value {
    let (status, body) = public_get(url);
    assert_eq!(status, 200, "GET {url} → {status}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("{url}: {e}: {body}"))
}

/// Owner call made directly (as the web studio or another device would).
fn owner(method: &str, base: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let e = e2e();
    let req = agent()
        .request(method, &format!("{base}{path}"))
        .set("authorization", &format!("Bearer {}", e.token));
    let res = match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    };
    let (status, text) = match res {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(c, r)) => (c, r.into_string().unwrap_or_default()),
        Err(e) => panic!("{method} {path}: {e}"),
    };
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn server_item(sid: &str) -> Value {
    let e = e2e();
    let (s, v) = owner("GET", &e.url, &format!("/api/items/{sid}"), None);
    assert_eq!(s, 200, "{v}");
    v
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sid_of(b: &LiveBackend, id: &LocalId) -> String {
    b.item(id)
        .and_then(|i| i.server_id)
        .expect("item has a server id")
        .0
}

/// A unique marker so tests sharing one Worker never match each other's items.
fn tag(name: &str) -> String {
    format!(
        "{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// A draft pushed to the server and published as v1.
fn published(b: &LiveBackend, kind: Kind, text: &str, note: Option<&str>) -> (LocalId, String) {
    let id = b.create_draft(kind, text).unwrap();
    b.sync_now().unwrap();
    let out = b.publish(&id, note).expect("publish");
    assert_eq!(out.version, 1);
    let sid = sid_of(b, &id);
    (id, sid)
}

fn ctl(args: &[&str]) {
    let ctl = std::env::var("BLYG_E2E_CTL").expect("BLYG_E2E_CTL not set");
    let st = Command::new(ctl).args(args).status().expect("run e2e ctl");
    assert!(st.success(), "ctl {args:?} failed");
}

// ------------------------------------------------------------------ tests

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn connect_and_verify() {
    let e = e2e();
    assert!(verify_connection(&e.url, &e.token).is_ok());
    assert_eq!(
        verify_connection(&e.url, "definitely-not-the-token"),
        Err(ConnectError::WrongToken)
    );
    // Same Worker, but a path where /api/items doesn't exist: 404 → the
    // "lacks the owner-API extensions" message.
    assert_eq!(
        verify_connection(&format!("{}/not-a-blyg", e.url), &e.token),
        Err(ConnectError::MissingExtensions)
    );
    // Nothing listening.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    assert_eq!(
        verify_connection(&format!("http://127.0.0.1:{port}"), &e.token),
        Err(ConnectError::Unreachable)
    );
    assert!(
        ConnectError::MissingExtensions
            .to_string()
            .contains("docs/SERVER.md")
    );
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn draft_autosaves_to_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = backend_at(dir.path(), &e.url, &e.token, true);
    let t = tag("autosave");
    let id = b.create_draft(Kind::Fragment, &t).unwrap();
    let text = format!("{t}\n\nsecond paragraph, typed later");
    b.save(&id, &format!("{t}\n\nsecond")).unwrap();
    b.save(&id, &text).unwrap();
    wait_until("the debounced push", Duration::from_secs(10), || {
        b.item(&id)
            .is_some_and(|i| i.server_id.is_some() && !i.pending_sync)
    });
    let sid = sid_of(&b, &id);
    let v = server_item(&sid);
    assert_eq!(v["content_md"], text);
    assert_eq!(v["status"], "draft");
    wait_until("synced status", Duration::from_secs(5), || {
        b.sync_status() == SyncStatus::Synced
    });
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn publish_edit_pin_restore() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    let t = tag("publish");
    let v1_text = format!("{t} first version");
    let (id, sid) = published(&b, Kind::Fragment, &v1_text, Some("first cut"));

    // The permalink resolves, and the public item document says v1.
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Public);
    assert_eq!(it.version, 1);
    assert!(!it.dirty, "freshly published");
    let permalink = it.permalink.clone().expect("permalink");
    assert_eq!(permalink, format!("{}/f/{sid}", e.url));
    let (s, html) = public_get(&permalink);
    assert_eq!(s, 200);
    assert!(html.contains(&v1_text), "the page shows the text");
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    assert_eq!(doc["version"], 1);
    assert_eq!(doc["content_md"], v1_text);
    assert_eq!(doc["changelog"][0]["note"], "first cut");

    // Edit → publish v2.
    let v2_text = format!("{t} second version");
    b.save(&id, &v2_text).unwrap();
    assert!(b.item(&id).unwrap().dirty, "unpublished edits");
    let out = b.publish(&id, Some("second cut")).unwrap();
    assert_eq!(out.version, 2);
    assert!(!b.item(&id).unwrap().dirty);
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    assert_eq!(doc["version"], 2);
    assert_eq!(doc["content_md"], v2_text);

    // v1 isn't pinned: no pin document yet.
    let pin_url = format!("{}/items/{sid}/v1.json", e.url);
    assert_eq!(public_get(&pin_url).0, 404);
    b.pin(&id, 1).unwrap();
    let pin = public_json(&pin_url);
    assert_eq!(pin["content_md"], v1_text);
    assert_eq!(pin["pinned"], true);
    assert_eq!(pin["content_hash"], content_hash(&v1_text));
    let versions = b.versions(&id).unwrap();
    assert_eq!(versions.len(), 2);
    assert!(versions[0].pinned && !versions[1].pinned);
    // Pinning again is idempotent.
    b.pin(&id, 1).unwrap();

    // Immutable: a v3 doesn't change v1's pin.
    b.save(&id, &format!("{t} third version")).unwrap();
    b.publish(&id, None).unwrap();
    assert_eq!(public_json(&pin_url)["content_md"], v1_text);

    // Restore v1: working copy only, nothing published, version stays.
    b.restore(&id, 1).unwrap();
    let it = b.item(&id).unwrap();
    assert_eq!(it.content_md, v1_text);
    assert_eq!(it.version, 3);
    assert!(it.dirty, "restored text is an unpublished edit");
    assert!(!it.pending_sync, "the restore happened server-side");
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    assert_eq!(doc["version"], 3);
    assert_eq!(doc["content_md"], format!("{t} third version"));
    assert_eq!(server_item(&sid)["content_md"], v1_text);
    // A pull agrees (no phantom conflict or edit).
    b.sync_now().unwrap();
    let it = b.item(&id).unwrap();
    assert!(!it.conflict);
    assert_eq!(it.content_md, v1_text);
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn set_kind_before_first_publish() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());

    // Local-only draft: just changes kind; the first POST carries it.
    let t = tag("kind-local");
    let a = b.create_draft(Kind::Fragment, &t).unwrap();
    b.set_kind(&a, Kind::Thread).unwrap();
    b.sync_now().unwrap();
    let sa = sid_of(&b, &a);
    assert_eq!(server_item(&sa)["authored_kind"], "thread");

    // Server-side draft: recreated as the new kind, old draft retired.
    let t2 = tag("kind-remote");
    let c = b.create_draft(Kind::Fragment, &t2).unwrap();
    b.sync_now().unwrap();
    let old = sid_of(&b, &c);
    b.set_kind(&c, Kind::Thread).unwrap();
    b.sync_now().unwrap();
    let new = sid_of(&b, &c);
    assert_ne!(old, new, "recreated under a new id");
    assert_eq!(server_item(&new)["authored_kind"], "thread");
    assert_eq!(server_item(&new)["content_md"], t2);
    assert_eq!(
        owner("GET", &e.url, &format!("/api/items/{old}"), None).0,
        404,
        "the old draft is gone"
    );
    // Publishing it gives a thread permalink.
    let out = b.publish(&c, None).unwrap();
    assert_eq!(out.permalink, format!("{}/t/{new}", e.url));
    assert_eq!(public_get(&out.permalink).0, 200);
    // After publish the kind is fixed.
    assert!(matches!(
        b.set_kind(&c, Kind::Fragment),
        Err(CoreError::Rejected { status: 409, .. })
    ));
    // Only one local item per draft, even after pulls.
    let n = b.items().iter().filter(|i| i.content_md == t2).count();
    assert_eq!(n, 1);
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn fragment_limit_is_enforced_like_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let b = manual(dir.path());

    // 1001 plain chars: the server refuses, the client reports Rejected.
    let over = "x".repeat(1001);
    let id = b.create_draft(Kind::Fragment, &over).unwrap();
    assert!(b.item(&id).unwrap().over_limit());
    b.sync_now().unwrap();
    match b.publish(&id, None) {
        Err(CoreError::Rejected {
            status: 400,
            message,
            ..
        }) => assert!(message.contains("1000"), "{message}"),
        other => panic!("expected Rejected 400, got {:?}", other.map(|o| o.version)),
    }
    assert_eq!(b.item(&id).unwrap().version, 0, "nothing was published");

    // An emoji is 2 UTF-16 units on both sides: 998 + 🎺 = 1000 publishes…
    let at_limit = format!("{}🎺", "a".repeat(998));
    let ok = b.create_draft(Kind::Fragment, &at_limit).unwrap();
    let it = b.item(&ok).unwrap();
    assert_eq!(it.char_count(), 1000);
    assert!(!it.over_limit());
    b.sync_now().unwrap();
    assert_eq!(b.publish(&ok, None).unwrap().version, 1);

    // …and 999 + 🎺 = 1001 doesn't.
    let one_over = format!("{}🎺", "a".repeat(999));
    let bad = b.create_draft(Kind::Fragment, &one_over).unwrap();
    let it = b.item(&bad).unwrap();
    assert_eq!(it.char_count(), 1001);
    assert!(it.over_limit());
    b.sync_now().unwrap();
    assert!(matches!(
        b.publish(&bad, None),
        Err(CoreError::Rejected { status: 400, .. })
    ));

    // TK markup doesn't count, only its output (both sides strip it).
    let tk = format!(
        "{}[TK]a long instruction nobody sees[=]yz[/TK]",
        "b".repeat(998)
    );
    let tkid = b.create_draft(Kind::Fragment, &tk).unwrap();
    assert_eq!(b.item(&tkid).unwrap().char_count(), 1000);
    b.sync_now().unwrap();
    assert_eq!(b.publish(&tkid, None).unwrap().version, 1);
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn withdraw_leaves_a_tombstone() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    let t = tag("withdraw");
    let (id, sid) = published(&b, Kind::Fragment, &t, None);
    let v = b.withdraw(&id, Some("changed my mind")).unwrap();
    assert_eq!(v, 2, "the endcap is v2");
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Withdrawn);
    assert_eq!(it.version, 2);
    assert_eq!(it.kind, Kind::Fragment, "authored kind survives");
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    assert_eq!(doc["kind"], "withdrawn");
    assert_eq!(doc["content_md"], "");
    assert_eq!(doc["changelog"][1]["note"], "changed my mind");
    let versions = b.versions(&id).unwrap();
    assert!(versions[1].endcap);
    // Endcaps can't be pinned (refused locally, no request).
    assert!(matches!(
        b.pin(&id, 2),
        Err(CoreError::Rejected { status: 409, .. })
    ));
    // Published work is withdrawn, never deleted.
    assert!(matches!(
        b.delete_draft(&id),
        Err(CoreError::Rejected { status: 409, .. })
    ));
    // Withdrawing twice is refused by the server.
    assert!(matches!(
        b.withdraw(&id, None),
        Err(CoreError::Rejected { status: 409, .. })
    ));
    // A pull keeps it withdrawn.
    b.sync_now().unwrap();
    assert_eq!(b.item(&id).unwrap().status, Status::Withdrawn);
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn delete_draft_removes_it_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    let id = b.create_draft(Kind::Fragment, &tag("delete")).unwrap();
    b.sync_now().unwrap();
    let sid = sid_of(&b, &id);
    b.delete_draft(&id).unwrap();
    assert!(b.item(&id).is_none());
    assert_eq!(
        owner("GET", &e.url, &format!("/api/items/{sid}"), None).0,
        404
    );
    b.sync_now().unwrap();
    assert!(b.item(&id).is_none(), "a pull doesn't bring it back");

    // A never-synced draft just vanishes, no request.
    let local = b.create_draft(Kind::Fragment, &tag("local")).unwrap();
    b.delete_draft(&local).unwrap();
    assert!(b.item(&local).is_none());
    b.sync_now().unwrap();
    assert!(b.item(&local).is_none());
}

/// A 1×1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn image_upload_is_served() {
    let dir = tempfile::tempdir().unwrap();
    let b = manual(dir.path());
    let id = b.create_draft(Kind::Fragment, &tag("image")).unwrap();
    // Uploading for a local-only item pushes it first.
    let m = b
        .upload_media(PNG.to_vec(), "image/png", Some(&id), Some("a dot"))
        .unwrap();
    assert!(
        m.url.starts_with("media/") && m.url.ends_with(".png"),
        "{}",
        m.url
    );
    assert_eq!(m.mime, "image/png");
    assert!(b.item(&id).unwrap().server_id.is_some());
    let base = b.base_url().unwrap();
    let res = agent().get(&format!("{base}/{}", m.url)).call().unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.header("content-type"), Some("image/png"));
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut res.into_reader(), &mut bytes).unwrap();
    assert_eq!(bytes, PNG);
    // Unsupported types are refused by the server with a clear message.
    match b.upload_media(b"hello".to_vec(), "text/plain", None, None) {
        Err(CoreError::Rejected { status: 415, .. }) => {}
        other => panic!("expected 415, got {:?}", other.map(|m| m.url)),
    }
}

/// A second, different 1×1 PNG (one byte of the pixel changed, CRC fixed).
fn other_png() -> Vec<u8> {
    // Build a valid PNG from scratch: 1×1 RGBA, a different colour.
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn adler(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &d in data {
            a = (a + d as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc(&c).to_be_bytes());
    }
    let raw = [0u8, 0x12, 0x34, 0x56, 0xff]; // filter byte + RGBA
    let mut z = vec![
        0x78,
        0x01,
        0x01,
        raw.len() as u8,
        0,
        !(raw.len() as u8),
        0xff,
    ];
    z.extend_from_slice(&raw);
    z.extend_from_slice(&adler(&raw).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// `<img>` tags whose src mentions `media_url` (not og:image and the like).
fn count_imgs(html: &str, media_url: &str) -> usize {
    html.split("<img")
        .skip(1)
        .filter(|tag| tag.split('>').next().unwrap_or("").contains(media_url))
        .count()
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn attachments_are_deduplicated_and_removable() {
    // Worker patch 8: POST /api/media returns the existing row (duplicate:
    // true) for identical bytes on the same item; DELETE /api/media/:id
    // removes it; the avatar can't be removed.
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    let api = Api::new(&e.url, &e.token);
    let (id, sid) = published(&b, Kind::Fragment, &tag("attach"), None);

    let first = api
        .upload_media(PNG, "image/png", Some(&sid), Some("a dot"))
        .unwrap();
    assert!(!first.duplicate);
    let again = api
        .upload_media(PNG, "image/png", Some(&sid), None)
        .unwrap();
    assert!(again.duplicate, "same bytes on the same item");
    assert_eq!(again.id, first.id);
    assert_eq!(again.url, first.url);
    let different = api
        .upload_media(&other_png(), "image/png", Some(&sid), None)
        .unwrap();
    assert!(!different.duplicate);
    assert_ne!(different.id, first.id);
    // The same bytes on another item (or none) are a separate upload.
    let loose = api.upload_media(PNG, "image/png", None, None).unwrap();
    assert!(!loose.duplicate);

    // Attachments show on the public page and in the item document, once each.
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    let urls: Vec<&str> = doc["media"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["url"].as_str())
        .collect();
    assert_eq!(urls.len(), 2, "{doc}");
    assert!(urls.contains(&first.url.as_str()));
    let (_, page) = public_get(&format!("{}/f/{sid}", e.url));
    assert_eq!(count_imgs(&page, &first.url), 1);

    // Remove one through the Backend (by its media/… URL).
    b.delete_media(&first.url).unwrap();
    assert_eq!(public_get(&format!("{}/{}", e.url, first.url)).0, 404);
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    assert_eq!(doc["media"].as_array().unwrap().len(), 1);
    assert!(matches!(
        b.delete_media(&first.url),
        Err(CoreError::NotFound)
    ));
    // Uploading it again after removal stores it anew.
    let back = api
        .upload_media(PNG, "image/png", Some(&sid), None)
        .unwrap();
    assert!(!back.duplicate);

    // The avatar is refused.
    let s = b.settings().unwrap();
    b.save_settings(&Settings {
        avatar_media_id: Some(loose.id.clone()),
        ..s.clone()
    })
    .unwrap();
    assert!(matches!(
        b.delete_media(&loose.url),
        Err(CoreError::Rejected { status: 409, .. })
    ));
    b.save_settings(&Settings {
        avatar_media_id: Some(String::new()),
        ..s
    })
    .unwrap();
    b.delete_media(&loose.url).unwrap();
    let _ = id;
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn pasted_images_render_once_inline() {
    // Public pages append every *attachment* after the text, so an image the
    // app also puts inline would show twice. The app's paste flow therefore
    // uploads without item_id and inlines it: exactly once, where the author
    // put it. It inlines the absolute URL (base_url + media/…): the Worker
    // renders a relative `media/…` page-relative, which 404s from /f/<id>/.
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    let t = tag("inline");
    let id = b.create_draft(Kind::Fragment, &t).unwrap();
    let m = b
        .upload_media(other_png(), "image/png", None, None)
        .unwrap();
    let inline = format!("{}/{}", b.base_url().unwrap(), m.url);
    b.save(&id, &format!("{t}\n\n![]({inline})\n\nafter the picture"))
        .unwrap();
    b.sync_now().unwrap();
    let out = b.publish(&id, None).unwrap();
    let (s, page) = public_get(&out.permalink);
    assert_eq!(s, 200);
    assert_eq!(count_imgs(&page, &m.url), 1, "{page}");
    // The inline src resolves against the page URL.
    let src = page
        .split("src=\"")
        .skip(1)
        .filter_map(|r| r.split('"').next())
        .find(|s| s.contains(&m.url))
        .expect("an <img> for the upload")
        .to_string();
    let page_url = url::Url::parse(&format!("{}/", out.permalink.trim_end_matches('/'))).unwrap();
    let abs = page_url.join(&src).unwrap();
    assert_eq!(
        public_get(abs.as_str()).0,
        200,
        "inline image {src} resolves from {page_url}"
    );

    // For the record, what the app avoids: a relative `media/…` breaks on
    // the page, and attached *and* inline shows the image twice.
    let rel = b
        .create_draft(Kind::Fragment, &format!("{t} relative"))
        .unwrap();
    b.save(&rel, &format!("{t} relative\n\n![]({})", m.url))
        .unwrap();
    b.sync_now().unwrap();
    let rel_out = b.publish(&rel, None).unwrap();
    let (_, rel_page) = public_get(&rel_out.permalink);
    let rel_src = rel_page
        .split("src=\"")
        .skip(1)
        .filter_map(|r| r.split('"').next())
        .find(|s| s.contains(&m.url))
        .unwrap()
        .to_string();
    let rel_base =
        url::Url::parse(&format!("{}/", rel_out.permalink.trim_end_matches('/'))).unwrap();
    assert_eq!(public_get(rel_base.join(&rel_src).unwrap().as_str()).0, 404);
    let sid = sid_of(&b, &id);
    let api = Api::new(&e.url, &e.token);
    let att = api
        .upload_media(PNG, "image/png", Some(&sid), None)
        .unwrap();
    b.save(&id, &format!("{t}\n\n![]({})", att.url)).unwrap();
    b.sync_now().unwrap();
    b.publish(&id, None).unwrap();
    let (_, page) = public_get(&out.permalink);
    assert_eq!(count_imgs(&page, &att.url), 2);
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn tk_output_with_dollar_patterns_publishes_verbatim() {
    // Worker patch 9: splicing is $-safe, so `$&`, `$1`, `$$` and `` $` ``
    // in generated text come out exactly as written.
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    let t = tag("dollar");
    let output = "It costs $& and $1, or $$ and $` $' in total.";
    let text = format!("{t}\n\n[TK]price it[=]{output}[/TK]");
    let id = b.create_draft(Kind::Fragment, &t).unwrap();
    b.save_with_provenance(&id, &text, &[prov("test-model-1", &[])])
        .unwrap();
    b.sync_now().unwrap();
    let sid = sid_of(&b, &id);
    b.publish(&id, None).unwrap();
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    let md = doc["content_md"].as_str().unwrap();
    assert!(md.contains(output), "{md}");
    let html = doc["content_html"].as_str().unwrap();
    assert!(
        html.contains("It costs $&amp; and $1, or $$ and $` $&#39; in total.")
            || html.contains("It costs $&amp; and $1, or $$ and $` $' in total."),
        "{html}"
    );
    assert_eq!(html.matches("blyg-tk-gen").count(), 1, "{html}");
    assert!(
        !html.contains("[TK]") && !html.contains("\u{e000}"),
        "{html}"
    );
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn conflict_from_a_direct_api_edit() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    let t = tag("conflict");
    let id = b.create_draft(Kind::Fragment, &t).unwrap();
    b.sync_now().unwrap();
    let sid = sid_of(&b, &id);
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let ev = events.clone();
        b.set_event_sink(Box::new(move |e| ev.lock().unwrap().push(e)));
    }
    // A local, unpushed edit…
    b.save(&id, &format!("{t} edited on this Mac")).unwrap();
    // …while the web studio edits the same draft.
    let (s, _) = owner(
        "PUT",
        &e.url,
        &format!("/api/items/{sid}"),
        Some(json!({"content_md": format!("{t} edited in the studio")})),
    );
    assert_eq!(s, 200);
    let _ = b.sync_now();
    let it = b.item(&id).unwrap();
    assert!(it.conflict, "entered conflict instead of overwriting");
    assert_eq!(it.content_md, format!("{t} edited on this Mac"));
    assert_eq!(
        server_item(&sid)["content_md"],
        format!("{t} edited in the studio"),
        "the studio edit wasn't clobbered"
    );
    let saw = events.lock().unwrap().iter().any(|e| {
        matches!(e, CoreEvent::Conflict { local_id, mine, theirs }
            if *local_id == id && mine.contains("this Mac") && theirs.contains("studio"))
    });
    assert!(saw, "a Conflict event reached the UI");
    // Publishing is blocked until it's resolved.
    assert!(matches!(
        b.publish(&id, None),
        Err(CoreError::Rejected { status: 409, .. })
    ));
    // Keep both: the item takes the server's text, mine becomes a new draft.
    b.resolve_conflict(&id, Resolution::KeepBoth).unwrap();
    b.sync_now().unwrap();
    assert_eq!(
        b.item(&id).unwrap().content_md,
        format!("{t} edited in the studio")
    );
    let mine = b
        .items()
        .into_iter()
        .find(|i| i.content_md == format!("{t} edited on this Mac"))
        .expect("my text survives as a draft");
    assert!(mine.server_id.is_some() && !mine.pending_sync);

    // Keep mine on a second round.
    b.save(&id, &format!("{t} mine again")).unwrap();
    owner(
        "PUT",
        &e.url,
        &format!("/api/items/{sid}"),
        Some(json!({"content_md": format!("{t} studio again")})),
    );
    let _ = b.sync_now();
    assert!(b.item(&id).unwrap().conflict);
    b.resolve_conflict(&id, Resolution::KeepMine).unwrap();
    b.sync_now().unwrap();
    assert_eq!(server_item(&sid)["content_md"], format!("{t} mine again"));
    assert!(!b.item(&id).unwrap().conflict);
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn read_extension_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    b.sync_now().unwrap();
    assert!(b.reading_available(), "GET /api/reading exists");
    let _ = b.reading();
    assert!(b.mentions().is_ok());
    let s = b.settings().unwrap();
    assert!(s.site_title.is_some(), "defaults come back as strings");
    // Settings round-trip (local Worker only; restored afterwards).
    let mut changed = s.clone();
    changed.author_bio = Some("Writes short things.".into());
    b.save_settings(&changed).unwrap();
    assert_eq!(
        b.settings().unwrap().author_bio.as_deref(),
        Some("Writes short things.")
    );
    b.save_settings(&Settings {
        author_bio: Some(String::new()),
        ..s.clone()
    })
    .unwrap();
    assert!(b.hoppers().is_ok());

    // Raw shapes, so a server change shows up here first.
    let (st, v) = owner("GET", &e.url, "/api/reading?limit=2", None);
    assert_eq!(st, 200);
    assert!(v["items"].is_array() && v.get("next").is_some(), "{v}");
    let (_, v) = owner("GET", &e.url, "/api/mentions", None);
    assert!(v["mentions"].is_array());
    let (_, v) = owner("GET", &e.url, "/api/settings", None);
    for k in [
        "site_title",
        "author_name",
        "author_bio",
        "site_url",
        "theme",
        "avatar_media_id",
        "author_links",
    ] {
        assert!(v.get(k).is_some(), "settings.{k} missing: {v}");
    }
    assert!(v.get("ai_style_prompt").is_none(), "no private settings");
    let (_, v) = owner("GET", &e.url, "/api/hoppers", None);
    assert!(v["hoppers"].is_array());
    let (st, _) = owner("GET", &e.url, "/api/reading?before=%%%", None);
    assert_eq!(st, 400, "a bad cursor is refused");
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn show_responses_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let b = manual(dir.path());
    let (id, sid) = published(&b, Kind::Fragment, &tag("responses"), None);
    assert!(!b.item(&id).unwrap().show_responses);
    b.set_show_responses(&id, true).unwrap();
    assert!(b.item(&id).unwrap().show_responses);
    assert_eq!(server_item(&sid)["show_responses"], true);
    b.sync_now().unwrap();
    assert!(b.item(&id).unwrap().show_responses, "a pull keeps it");
    b.set_show_responses(&id, false).unwrap();
    assert_eq!(server_item(&sid)["show_responses"], false);
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn offline_queues_then_flushes_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = backend_at(dir.path(), &e.url, &e.token, true);
    let t = tag("offline");
    let id = b.create_draft(Kind::Fragment, &t).unwrap();
    wait_until("the first push", Duration::from_secs(10), || {
        b.item(&id)
            .is_some_and(|i| i.server_id.is_some() && !i.pending_sync)
    });
    let sid = sid_of(&b, &id);

    ctl(&["stop", "a"]);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.save(&id, &format!("{t} written offline")).unwrap();
        let other = b
            .create_draft(Kind::Fragment, &format!("{t} new offline"))
            .unwrap();
        wait_until(
            "offline status",
            Duration::from_secs(10),
            || matches!(b.sync_status(), SyncStatus::Offline { pending } if pending >= 2),
        );
        // Nothing blocks: local reads and writes keep working.
        assert_eq!(
            b.item(&id).unwrap().content_md,
            format!("{t} written offline")
        );
        assert!(b.item(&other).unwrap().pending_sync);
        other
    }));
    ctl(&["start", "a", e.url.rsplit(':').next().unwrap()]);
    let other = result.unwrap_or_else(|p| std::panic::resume_unwind(p));

    wait_until("the queue to flush", Duration::from_secs(20), || {
        b.item(&id).is_some_and(|i| !i.pending_sync)
            && b.item(&other).is_some_and(|i| !i.pending_sync)
    });
    assert_eq!(
        server_item(&sid)["content_md"],
        format!("{t} written offline")
    );
    let osid = sid_of(&b, &other);
    assert_eq!(server_item(&osid)["content_md"], format!("{t} new offline"));
    wait_until("synced status", Duration::from_secs(10), || {
        b.sync_status() == SyncStatus::Synced
    });
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn owner_token_is_never_sent_to_the_public_surface() {
    // The public client used for other origins holds no token; make sure the
    // owner Api and the public surface agree about the permalink host.
    let e = e2e();
    let api = Api::new(&e.url, &e.token);
    assert_eq!(api.base_url(), e.url);
    assert!(format!("{api:?}").contains("<redacted>"));
    assert!(!format!("{api:?}").contains(&e.token));
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn publish_right_after_typing_and_republish_after_withdraw() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = backend_at(dir.path(), &e.url, &e.token, true);
    let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    {
        let ev = events.clone();
        b.set_event_sink(Box::new(move |e| ev.lock().unwrap().push(e)));
    }
    let t = tag("quick");
    // ⌘⏎ straight after the last keystroke: the debounced push hasn't run.
    let id = b.create_draft(Kind::Fragment, &t).unwrap();
    b.save(&id, &format!("{t} final words")).unwrap();
    let out = b.publish(&id, None).unwrap();
    assert_eq!(out.version, 1);
    let sid = sid_of(&b, &id);
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    assert_eq!(
        doc["content_md"],
        format!("{t} final words"),
        "published the latest text"
    );
    let it = b.item(&id).unwrap();
    assert!(!it.dirty && !it.pending_sync);

    // Withdraw, edit, publish again: v3, public again.
    b.withdraw(&id, None).unwrap();
    b.save(&id, &format!("{t} back again")).unwrap();
    let out = b.publish(&id, Some("back")).unwrap();
    assert_eq!(out.version, 3);
    let it = b.item(&id).unwrap();
    assert_eq!(it.status, Status::Public);
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    assert_eq!(doc["kind"], "fragment");
    assert_eq!(doc["content_md"], format!("{t} back again"));

    // Every sync state the UI shows was emitted along the way.
    b.sync_now().unwrap();
    let ev = events.lock().unwrap();
    let saw = |s: SyncStatus| ev.contains(&CoreEvent::SyncStatus(s));
    assert!(saw(SyncStatus::Syncing), "syncing…");
    assert!(saw(SyncStatus::Synced), "synced");
    assert!(ev.contains(&CoreEvent::ItemsChanged));
}

// ------------------------------------------------------------- provenance

fn prov(model: &str, sources: &[&str]) -> Option<ScopeProvenance> {
    Some(ScopeProvenance {
        model: model.into(),
        sources: sources
            .iter()
            .map(|s| ProvenanceSource {
                id: s.to_string(),
                version: None,
            })
            .collect(),
        at: None,
    })
}

fn server_provenance(sid: &str) -> Value {
    let e = e2e();
    let (s, v) = owner(
        "GET",
        &e.url,
        &format!("/api/items/{sid}/tk-provenance"),
        None,
    );
    assert_eq!(s, 200, "{v}");
    v["scopes"].clone()
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn client_recorded_provenance_is_disclosed() {
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    let t = tag("prov");
    let (_, src) = published(&b, Kind::Fragment, &format!("{t} a source"), None);

    // Text generated in the app, recorded with its provenance.
    let generated = format!("[TK]sum up ![[{src}]][=]A short summary.[/TK]");
    let text = format!("{t}\n\n{generated}\n\nA hand-written tail.");
    let id = b.create_draft(Kind::Fragment, &t).unwrap();
    // A wrong-length array is refused locally, before anything is queued.
    assert!(matches!(
        b.save_with_provenance(&id, &text, &[]),
        Err(CoreError::Rejected { status: 400, .. })
    ));
    b.save_with_provenance(&id, &text, &[prov("test-model-1", &[&src])])
        .unwrap();
    b.sync_now().unwrap();
    let sid = sid_of(&b, &id);
    let p = server_provenance(&sid);
    assert_eq!(p.as_array().unwrap().len(), 1, "{p}");
    assert_eq!(p[0]["model"], "test-model-1");
    assert_eq!(p[0]["sources"][0]["id"], src.as_str());
    assert_eq!(p[0]["sources"][0]["version"], 1);
    assert_eq!(server_item(&sid)["content_md"], text);

    // The position-keyed rule: a hand-written scope typed in FRONT of the
    // generated one, with a plain save. The combined push must move the
    // provenance to position 1; a plain PUT would leave it on position 0.
    let text2 = format!(
        "{t}\n\n[TK]note to self[=]typed by hand[/TK]\n\n{generated}\n\nA hand-written tail."
    );
    b.save(&id, &text2).unwrap();
    b.sync_now().unwrap();
    let p = server_provenance(&sid);
    assert_eq!(p.as_array().unwrap().len(), 2, "{p}");
    assert!(
        p[0].is_null(),
        "the hand-written scope isn't disclosed: {p}"
    );
    assert_eq!(p[1]["model"], "test-model-1");

    // Publish: the item document carries `generated`, and the HTML wraps
    // exactly the generated span in blyg-tk-gen.
    b.publish(&id, None).unwrap();
    let doc = public_json(&format!("{}/items/{sid}.json", e.url));
    let generated_doc = doc["generated"].as_array().expect("generated").clone();
    assert_eq!(generated_doc.len(), 1, "{doc}");
    assert_eq!(generated_doc[0]["model"], "test-model-1");
    assert_eq!(generated_doc[0]["sources"][0]["id"], src.as_str());
    let html = doc["content_html"].as_str().unwrap();
    assert_eq!(html.matches("blyg-tk-gen").count(), 1, "{html}");
    let at = html.find("blyg-tk-gen").unwrap();
    assert!(html[at..].contains("A short summary."), "{html}");
    assert!(html.find("typed by hand").unwrap() < at, "{html}");
    assert!(!doc["content_md"].as_str().unwrap().contains("[TK]"));

    // Removing a scope later keeps the rest aligned too.
    b.save(&id, &format!("{t}\n\n{generated}")).unwrap();
    b.sync_now().unwrap();
    let p = server_provenance(&sid);
    assert_eq!(p.as_array().unwrap().len(), 1, "{p}");
    assert_eq!(p[0]["model"], "test-model-1");
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn server_held_provenance_follows_its_scope() {
    // Provenance the app never saw (recorded by another client or the
    // Worker's own generate) still follows its scope when the app edits.
    let dir = tempfile::tempdir().unwrap();
    let e = e2e();
    let b = manual(dir.path());
    let t = tag("prov-server");
    let text = format!("{t}\n\n[TK]write a line[=]A generated line.[/TK]");
    let id = b.create_draft(Kind::Fragment, &text).unwrap();
    b.sync_now().unwrap();
    let sid = sid_of(&b, &id);
    let (s, v) = owner(
        "PUT",
        &e.url,
        &format!("/api/items/{sid}/tk-provenance"),
        Some(json!({"scopes": [{"index": 0, "model": "elsewhere-model"}]})),
    );
    assert_eq!(s, 200, "{v}");
    b.sync_now().unwrap();
    b.save(&id, &format!("[TK]opening[=]By hand.[/TK]\n\n{text}"))
        .unwrap();
    b.sync_now().unwrap();
    let p = server_provenance(&sid);
    assert_eq!(p.as_array().unwrap().len(), 2, "{p}");
    assert!(p[0].is_null(), "{p}");
    assert_eq!(p[1]["model"], "elsewhere-model");
}

// ----------------------------------------------------------- subscriptions

/// Run the Worker's cron handler (`wrangler dev --test-scheduled`).
fn trigger_scheduled(base: &str) {
    let (s, body) = public_get(&format!("{base}/__scheduled?cron=*/15+*+*+*+*"));
    assert_eq!(s, 200, "{body}");
}

fn reading_item(b: &LiveBackend, rid: &str) -> Option<ReadingItem> {
    b.reading().into_iter().find(|r| r.remote_id == rid)
}

/// Poll the subscription the way the server does on its own: the cron
/// handler first. A subscription polled less than ~30 min ago isn't due, so
/// fall back to the explicit resync the web studio offers. Returns whether
/// the cron handler alone was enough.
fn poll(subscriber: &LiveBackend, sub_id: &str, rid: &str, want_version: u32) -> bool {
    let e = e2e();
    trigger_scheduled(&e.url);
    std::thread::sleep(Duration::from_millis(300));
    subscriber.sync_now().unwrap();
    if reading_item(subscriber, rid).is_some_and(|r| r.version >= want_version) {
        return true;
    }
    let api = Api::new(&e.url, &e.token);
    api.resync_subscription(sub_id).unwrap();
    subscriber.sync_now().unwrap();
    assert!(
        reading_item(subscriber, rid).is_some_and(|r| r.version >= want_version),
        "resync didn't bring v{want_version}"
    );
    false
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn subscriptions_reading_and_pinned_versions() {
    let e = e2e();
    let src_url = url_b();
    let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let me = manual(d1.path());
    let source = backend_at(d2.path(), &src_url, &e.token, false);
    let t = tag("sub");

    let (sid, rid) = published(&source, Kind::Fragment, &format!("{t} v1"), Some("first"));

    // Subscribe (the Worker resolves the URL and backfills right away).
    let preview = me.preview_subscription(&src_url).unwrap();
    assert_eq!(preview.kind, SubscriptionKind::Blyg);
    assert_eq!(
        preview.origin.as_deref(),
        Some(format!("{src_url}/").as_str())
    );
    let sub = me.subscribe(&src_url, Some("A neighbour")).unwrap();
    assert_eq!(sub.kind, SubscriptionKind::Blyg);
    assert!(me.subscriptions().iter().any(|s| s.id == sub.id));
    me.sync_now().unwrap();
    let r = reading_item(&me, &rid).expect("the post is in the reading list");
    assert_eq!(r.subscription_title, "A neighbour");
    assert_eq!(r.version, 1);
    assert_eq!(r.content_md, format!("{t} v1"));
    assert_eq!(r.kind, Kind::Fragment);
    assert_eq!(r.state, "current");
    assert!(r.is_unread());
    assert_eq!(
        r.page.as_deref(),
        Some(format!("{src_url}/f/{rid}/").as_str()),
        "page resolved against the origin"
    );
    me.mark_read(&sub.id, &rid).unwrap();
    assert_eq!(reading_item(&me, &rid).unwrap().read_version, Some(1));

    // The author edits and republishes; the subscriber's Worker polls.
    source.save(&sid, &format!("{t} v2")).unwrap();
    source.publish(&sid, Some("second")).unwrap();
    let cron_polled = poll(&me, &sub.id, &rid, 2);
    let r = reading_item(&me, &rid).unwrap();
    assert_eq!(r.version, 2);
    assert!(r.edited_since_read(), "edited since you read it");
    assert_eq!(r.read_version, Some(1), "read state survived the edit");
    assert_eq!(
        me.reading().iter().filter(|x| x.remote_id == rid).count(),
        1,
        "one entry per post"
    );
    // v1 wasn't pinned: the changelog notes only, no diff base.
    assert!(me.pinned_diff_base(&sub.id, &rid).is_none());
    let full = me.remote_versions(&sub.id, &rid).unwrap();
    assert_eq!(
        full.iter().map(|v| v.note.clone()).collect::<Vec<_>>(),
        vec![Some("first".to_string()), Some("second".to_string())]
    );

    // Pin v1 on the source, publish v3: the version browser shows only the
    // current version and the pin; v2 doesn't appear at all.
    source.pin(&sid, 1).unwrap();
    source.save(&sid, &format!("{t} v3")).unwrap();
    source.publish(&sid, None).unwrap();
    poll(&me, &sub.id, &rid, 3);
    let shown = me.remote_shown_versions(&sub.id, &rid).unwrap();
    assert_eq!(
        shown
            .iter()
            .map(|v| (v.version, v.pinned, v.current))
            .collect::<Vec<_>>(),
        vec![(1, true, false), (3, false, true)]
    );
    let pin = me.remote_pinned(&sub.id, &rid, 1).unwrap();
    assert_eq!(pin.content_md, format!("{t} v1"));
    assert!(!pin.hash_mismatch);
    assert_eq!(pin.url().unwrap(), format!("{src_url}/items/{rid}/v1.json"));
    assert!(matches!(
        me.remote_pinned(&sub.id, &rid, 2),
        Err(CoreError::Rejected { status: 404, .. })
    ));
    // Read at v1 (pinned) → the diff base is that pin.
    assert_eq!(
        me.pinned_diff_base(&sub.id, &rid).map(|p| p.version),
        Some(1)
    );

    // Signals and withdrawal: a thumbed post stays visible as a tombstone,
    // and only the pinned version's content may be kept.
    me.signal(&sub.id, &rid, Some(1)).unwrap();
    source.withdraw(&sid, Some("gone")).unwrap();
    poll(&me, &sub.id, &rid, 4);
    let r = reading_item(&me, &rid).expect("thumbed tombstones stay listed");
    assert_eq!(r.state, "tombstone");
    assert_eq!(r.thumb, Some(1));
    match r.pinned_version_retained {
        Some(1) => assert_eq!(r.content_md, format!("{t} v1")),
        None => assert!(r.content_md.is_empty(), "no unpinned text kept"),
        other => panic!("unexpected retained version {other:?}"),
    }
    assert!(!r.content_md.contains("v3"), "withdrawn text is gone");

    me.unsubscribe(&sub.id).unwrap();
    assert!(!me.subscriptions().iter().any(|s| s.id == sub.id));
    eprintln!("subscription poll via the cron handler alone: {cron_polled}");
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn reading_items_carry_the_published_html() {
    // What the Reading screen shows is the HTML the author's blyg published:
    // the transclusion snapshot is baked in, and image URLs are as the author
    // wrote them (so a relative `media/…` needs the author's origin as base).
    let e = e2e();
    let src_url = url_b();
    let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let me = manual(d1.path());
    let source = backend_at(d2.path(), &src_url, &e.token, false);
    let t = tag("html");

    let (_, quoted) = published(&source, Kind::Fragment, &format!("{t} quoted words"), None);
    let m = source
        .upload_media(other_png(), "image/png", None, None)
        .unwrap();
    let text = format!(
        "{t} thread\n\n![[{quoted}]]\n\nA reply.\n\n![]({})\n\n![]({src_url}/{})",
        m.url, m.url
    );
    let tid = source.create_draft(Kind::Thread, &text).unwrap();
    source.sync_now().unwrap();
    // An attachment: public pages show it after the text, not in content_html.
    let att = source
        .upload_media(PNG.to_vec(), "image/png", Some(&tid), Some("a heron"))
        .unwrap();
    source.publish(&tid, None).unwrap();
    let rid = sid_of(&source, &tid);
    source.pin(&tid, 1).unwrap();

    let sub = me.subscribe(&src_url, Some("html")).unwrap();
    me.sync_now().unwrap();
    let r = reading_item(&me, &rid).expect("the thread is in the reading list");
    eprintln!("reading content_html = {}", r.content_html);
    let html = &r.content_html;
    assert!(
        html.contains(&format!(
            "<blockquote class=\"blyg-transclusion\" data-blyg-id=\"{quoted}\""
        )),
        "{html}"
    );
    assert!(html.contains("quoted words"), "{html}");
    assert!(!html.contains("![["), "{html}");
    assert!(html.contains(&format!("src=\"{}\"", m.url)), "{html}");
    assert!(
        html.contains(&format!("src=\"{src_url}/{}\"", m.url)),
        "{html}"
    );

    let pin = me.remote_pinned(&sub.id, &rid, 1).unwrap();
    eprintln!("pinned content_html = {}", pin.content_html);
    assert_eq!(&pin.content_html, html, "the pin serves the same bytes");
    assert!(!html.contains(&att.url), "attachments aren't in the HTML");
    let vs = me.remote_versions(&sub.id, &rid).unwrap();
    let cur = vs.iter().find(|v| v.current).unwrap();
    eprintln!("current media = {:?}", cur.media);
    assert_eq!(
        cur.media,
        vec![RemoteMedia {
            url: format!("{src_url}/{}", att.url),
            alt: Some("a heron".into())
        }]
    );
    me.unsubscribe(&sub.id).unwrap();
}

#[test]
#[ignore = "needs a local Worker: scripts/e2e-local.sh"]
fn read_state_syncs_between_two_macs() {
    // Extension 5: a post read on one Mac reads as read on another Mac
    // signed in to the same blyg, and the server never lowers it.
    let e = e2e();
    let api = Api::new(&e.url, &e.token);
    if !api.reading(1, None).unwrap().is_some_and(|p| p.read_sync()) {
        eprintln!("SKIP: this Worker doesn't advertise read_state (extension 5)");
        return;
    }
    let src_url = url_b();
    let (d1, d2, d3) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let mac1 = manual(d1.path());
    let mac2 = manual(d2.path());
    let source = backend_at(d3.path(), &src_url, &e.token, false);
    let t = tag("read");
    let (sid, rid) = published(&source, Kind::Fragment, &format!("{t} v1"), None);
    let sub = mac1.subscribe(&src_url, Some("Read sync")).unwrap();
    mac1.sync_now().unwrap();
    mac2.sync_now().unwrap();
    assert!(reading_item(&mac1, &rid).unwrap().is_unread());
    assert!(reading_item(&mac2, &rid).unwrap().is_unread());

    mac1.mark_read(&sub.id, &rid).unwrap();
    mac1.sync_now().unwrap(); // flushes the read op
    mac2.sync_now().unwrap();
    let r = reading_item(&mac2, &rid).unwrap();
    assert_eq!(r.read_version, Some(1), "read on the other Mac");
    assert!(!r.is_unread());

    // Edited and read at v2 on the second Mac; a stale write can't lower it.
    source.save(&sid, &format!("{t} v2")).unwrap();
    source.publish(&sid, None).unwrap();
    poll(&mac2, &sub.id, &rid, 2);
    mac2.mark_read(&sub.id, &rid).unwrap();
    mac2.sync_now().unwrap();
    api.put_read(&sub.id, &rid, 1).unwrap();
    mac1.sync_now().unwrap();
    assert_eq!(reading_item(&mac1, &rid).unwrap().read_version, Some(2));
    let page = api.reading(500, None).unwrap().unwrap();
    let row = page.items.iter().find(|i| i.remote_id == rid).unwrap();
    assert_eq!(row.read_version, Some(2), "the server kept the max");

    // A fresh install sees it read.
    let d4 = tempfile::tempdir().unwrap();
    let mac3 = manual(d4.path());
    mac3.sync_now().unwrap();
    assert_eq!(reading_item(&mac3, &rid).unwrap().read_version, Some(2));

    mac1.unsubscribe(&sub.id).unwrap();
    let page = api.reading(500, None).unwrap().unwrap();
    assert!(!page.items.iter().any(|i| i.remote_id == rid));
}
