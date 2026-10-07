//! A tiny in-process mock of the blyg owner API (std TcpListener, one thread
//! per connection, `Connection: close`). Enough of upstream blygger-studio's
//! owner API (studio 0.10) and the docs/SERVER.md extensions to exercise
//! blyg-core; never talks to a real server.
//!
//! Every upstream `/api` request the app sends, and every response the mock
//! gives, is checked against `tests/fixtures/openapi.json`. A violation fails
//! the test when its `Mock` is dropped. Extension routes (not upstream) are
//! listed in `is_extension`.
//!
//! `set_down(true)` makes it accept and immediately drop connections, which
//! the client sees as a transport failure (= offline).

#![allow(dead_code)]

pub mod contract;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blyg_core::{Backend, CoreEvent, LiveBackend, SyncOptions};
use serde_json::{Value, json};

pub const TOKEN: &str = "test-token-not-real";
/// The studio password (`OWNER_PASSWORD`) the mock's sign-in takes.
pub const PASSWORD: &str = "test password, not real";

#[derive(Debug, Clone)]
pub struct SItem {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub version: u32,
    pub dirty: bool,
    pub created: String,
    pub updated: String,
    pub content_md: String,
    /// Contract `Version` objects (see `version_json`).
    pub versions: Vec<Value>,
    /// `"default"` | `"show"` | `"hide"`.
    pub responses: String,
    pub stub_of: Value,
    pub forked_from: Value,
}

impl SItem {
    /// Whether the page shows responses (the policy, else the site default).
    pub fn showing(&self, default: bool) -> bool {
        match self.responses.as_str() {
            "show" => true,
            "hide" => false,
            _ => default,
        }
    }
}

/// A contract `Version`.
pub fn version_json(item: &SItem, version: u32, at: &str, note: Value, withdrawn: bool) -> Value {
    json!({
        "item_id": item.id, "version": version,
        "content_md": if withdrawn { "" } else { item.content_md.as_str() },
        "content_html": "", "content_hash": format!("h{version}"),
        "published_at": at, "note": note, "pinned_at": null,
        "kind": if withdrawn { "withdrawn" } else { item.kind.as_str() },
        "pinned": false, "note_generated": false, "transclusions": [], "generated": [],
        "stub_of": item.stub_of, "stub_cite": null,
    })
}

#[derive(Default)]
pub struct State {
    pub items: BTreeMap<String, SItem>,
    pub subs: Vec<Value>,
    /// `None` = endpoint not deployed (404).
    pub reading: Option<Vec<Value>>,
    pub reading_page_size: usize,
    /// Extension 5 "deployed": `GET /api/reading` says `read_state: true`
    /// and carries `read_version`, and the read-state writes exist (else 404).
    pub read_sync: bool,
    /// Stored read state, `(sub, remote_id)` → version (max-merged).
    pub reads: BTreeMap<(String, String), u32>,
    /// Body of every `POST /api/reading/read`, in order.
    pub read_batches: Vec<Value>,
    pub mentions: Option<Vec<Value>>,
    pub settings: Option<Value>,
    pub hoppers: Option<Vec<Value>>,
    pub signals: BTreeMap<(String, String), i64>,
    pub hidden: BTreeMap<String, bool>,
    /// Body of every `PATCH /api/settings`, in order.
    pub settings_puts: Vec<Value>,
    /// `show_responses_default`, when `settings` doesn't set it.
    pub responses_default: bool,
    /// Contract violations seen so far ("METHOD /path: what").
    pub violations: Vec<String>,
    /// Path prefixes not checked: for tests that serve deliberately
    /// malformed data to prove the app tolerates it.
    pub unchecked: Vec<String>,
    /// Act like a server older than studio 0.9 (with the old owner-read
    /// extension): `GET /api/items` is `{items}` with no `total`, and the
    /// routes 0.9 added are unknown (404). Not checked against the contract.
    pub legacy_api: bool,
    /// Live studio sessions (`blyg_session` values). Clear it to end them.
    pub sessions: Vec<String>,
    /// Successful `POST …/studio/login`s.
    pub logins: usize,
    /// The bearer token is refused, as on a stock server (cookie only).
    pub no_bearer: bool,
    /// A stock blygger-studio: none of the fork's extension routes (they
    /// 404). Reading is then upstream's `/api/reading` + `/api/imports`,
    /// served from the same `reading` rows.
    pub stock: bool,
    /// A stock studio before 0.18: `/api/imports` rows without
    /// `stub_of_json`/`forked_from_json`, so lineage comes from each post's
    /// public document.
    pub pre_lineage_imports: bool,
    /// The Worker fork's reading rows carry `stub_of`/`forked_from`, and the
    /// page says so (`lineage: true`).
    pub reading_lineage: bool,
    /// The public static surface (anything outside `/api/`), path → JSON
    /// body. Unlisted paths 404, which for `v{n}.json` means "not pinned".
    /// Serve at e.g. `/blyg/items/X.json` to test a subdirectory mount.
    pub public: BTreeMap<String, Value>,
    /// The `authorization` header of every public request (None = absent).
    pub public_auth: Vec<Option<String>>,
    // --- profiles ---
    /// Public text files (OPML, RSS/Atom), path → body, served as XML.
    pub public_text: BTreeMap<String, String>,
    /// Every header of every public request, lower-cased names, in order.
    pub public_headers: Vec<Vec<(String, String)>>,
    pub media_bodies: Vec<Vec<u8>>,
    /// "METHOD /path?query" per request, in order.
    pub log: Vec<String>,
    tick: u64,
    next_id: u64,
}

impl State {
    pub fn now(&mut self) -> String {
        self.tick += 1;
        format!("2030-01-01T00:00:00.{:06}Z", self.tick)
    }

    pub fn new_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}{:0>24}", self.next_id)
    }

    /// Insert a server-side item directly (as if made in the web studio).
    pub fn add_item(&mut self, kind: &str, content: &str) -> String {
        let id = self.new_id("SV");
        let now = self.now();
        self.items.insert(
            id.clone(),
            SItem {
                id: id.clone(),
                kind: kind.into(),
                status: "draft".into(),
                version: 0,
                dirty: true,
                created: now.clone(),
                updated: now,
                content_md: content.into(),
                versions: vec![],
                responses: "default".into(),
                stub_of: Value::Null,
                forked_from: Value::Null,
            },
        );
        id
    }

    /// Edit the working copy server-side (another device).
    pub fn edit(&mut self, id: &str, content: &str) {
        let now = self.now();
        let it = self.items.get_mut(id).expect("item");
        it.content_md = content.into();
        it.dirty = true;
        if it.version == 0 {
            it.updated = now;
        }
    }

    pub fn count(&self, prefix: &str) -> usize {
        self.log.iter().filter(|l| l.starts_with(prefix)).count()
    }
}

pub struct Mock {
    pub url: String,
    state: Arc<Mutex<State>>,
    down: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
}

impl Mock {
    pub fn start() -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State {
            reading_page_size: 500,
            ..Default::default()
        }));
        let down = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (st, dn, sp) = (state.clone(), down.clone(), stop.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if sp.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                if dn.load(Ordering::SeqCst) {
                    let _ = conn.shutdown(Shutdown::Both);
                    continue;
                }
                let st = st.clone();
                std::thread::spawn(move || handle(conn, st));
            }
        });
        Mock {
            url: format!("http://{addr}"),
            state,
            down,
            stop,
            addr,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    pub fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if std::thread::panicking() {
            return;
        }
        let v = std::mem::take(&mut self.state.lock().unwrap().violations);
        assert!(
            v.is_empty(),
            "owner-API traffic broke the OpenAPI contract:\n{}",
            v.join("\n")
        );
    }
}

// ------------------------------------------------------------------ HTTP

struct Req {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn query_param(&self, key: &str) -> Option<String> {
        url::form_urlencoded::parse(self.query.as_bytes())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    }
}

fn read_req(conn: &TcpStream) -> Option<Req> {
    let mut r = BufReader::new(conn.try_clone().ok()?);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = vec![];
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    r.read_exact(&mut body).ok()?;
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    Some(Req {
        method,
        path,
        query,
        headers,
        body,
    })
}

fn handle(mut conn: TcpStream, state: Arc<Mutex<State>>) {
    let Some(req) = read_req(&conn) else { return };
    // The studio's sign-in (any mount): a form POST, answered with a 302
    // and the session cookie, like upstream's spa.ts.
    if req.method == "POST" && req.path.ends_with("/studio/login") {
        let mut st = state.lock().unwrap();
        st.log.push(format!("POST {}", req.path));
        let password = url::form_urlencoded::parse(&req.body)
            .find(|(k, _)| k == "password")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        let head = if password == PASSWORD {
            st.logins += 1;
            let s = format!("{}.sig{}", 2_000_000_000 + st.logins, st.logins);
            st.sessions.push(s.clone());
            format!(
                "HTTP/1.1 302 Found\r\nlocation: /studio\r\nset-cookie: blyg_session={s}; Path=/; Max-Age=2592000; HttpOnly; Secure; SameSite=Lax\r\n"
            )
        } else {
            "HTTP/1.1 403 Forbidden\r\ncontent-type: text/html\r\n".to_string()
        };
        let _ = write!(conn, "{head}content-length: 0\r\nconnection: close\r\n\r\n");
        let _ = conn.flush();
        return;
    }
    let (status, body) = {
        let mut st = state.lock().unwrap();
        let (status, body) = route(&req, &mut st);
        check(&req, status, &body, &mut st);
        (status, body)
    };
    // --- profiles --- a bare string is a public text file (OPML, a feed).
    let (ctype, body) = match body {
        Value::String(t) => ("application/xml", t),
        v => ("application/json", v.to_string()),
    };
    let reason = match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        _ => "Other",
    };
    let _ = write!(
        conn,
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = conn.flush();
}

/// Check one exchange against the contract: upstream's for its own
/// operations, the fork's extensions (docs/SERVER.md) for the rest, and for
/// a reply status upstream doesn't declare (`POST /api/media` → 200
/// `duplicate`). Record what's wrong.
fn check(req: &Req, status: u16, body: &Value, s: &mut State) {
    if !req.path.starts_with("/api/") || status == 401 || s.legacy_api {
        return;
    }
    let segs: Vec<String> = req
        .path
        .trim_start_matches('/')
        .split('/')
        .map(decode)
        .collect();
    let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
    let (up, ext) = (contract::contract(), contract::extensions());
    let m = req.method.as_str();
    let what = format!("{m} {}", req.path);
    if s.unchecked.iter().any(|p| req.path.starts_with(p.as_str())) {
        return;
    }
    let req_side = if up.knows(m, &segs) { up } else { ext };
    let query: Vec<(String, String)> = url::form_urlencoded::parse(req.query.as_bytes())
        .into_owned()
        .collect();
    if let Err(e) = req_side.check_request(m, &segs, &query, req.header("content-type"), &req.body)
    {
        s.violations.push(format!("{what}: request: {e}"));
        return;
    }
    let reply_side = if up.declares(m, &segs, status) || !ext.declares(m, &segs, status) {
        req_side
    } else {
        ext
    };
    if let Err(e) = reply_side.check_response(m, &segs, status, body) {
        s.violations.push(format!("{what}: mock response: {e}"));
    }
}

/// The contract's `Item`.
fn item_json(it: &SItem) -> Value {
    json!({
        "id": it.id,
        "kind": if it.status == "withdrawn" { "withdrawn" } else { it.kind.as_str() },
        "status": it.status,
        "created": it.created,
        "updated": it.updated,
        "version": it.version,
        "content_md": it.content_md,
        "dirty": it.dirty,
        "responses": it.responses,
        "highlight": "default",
        "provenance": [],
        "stub_of": it.stub_of,
        "forked_from": it.forked_from,
        "fork_cite": null,
    })
}

/// `GET /api/items/:id`: the item plus `authored_kind`, media and versions.
fn item_detail(it: &SItem) -> Value {
    let mut v = item_json(it);
    v["authored_kind"] = json!(it.kind);
    v["media"] = json!([]);
    v["versions"] = json!(it.versions);
    v["published"] = it
        .versions
        .iter()
        .rev()
        .find(|x| x["kind"] != "withdrawn")
        .cloned()
        .unwrap_or(Value::Null);
    v
}

/// `defaults` with `over`'s fields on top.
fn overlay(mut defaults: Value, over: &Value) -> Value {
    if let (Some(d), Some(o)) = (defaults.as_object_mut(), over.as_object()) {
        for (k, v) in o {
            d.insert(k.clone(), v.clone());
        }
    }
    defaults
}

/// A contract `Subscription` from whatever a test seeded.
pub fn sub_json(v: &Value) -> Value {
    overlay(
        json!({ "id": "", "kind": "blyg", "origin": "", "feed_url": "", "title": "",
            "status": "active", "last_poll_at": null, "fail_count": 0, "last_index_sync_at": null,
            "created": "2030-01-01T00:00:00Z", "title_follows_source": true, "in_blogroll": false,
            "flags": [] }),
        v,
    )
}

/// Contract `Settings` from whatever a test seeded.
fn settings_json(v: &Value, responses_default: bool) -> Value {
    overlay(
        json!({ "site_title": "", "theme": "auto", "author_name": "", "author_bio": "",
            "author_links": [], "site_url": "", "timezone": "", "avatar_media_id": "",
            "ai_model": "", "ai_model_tk": "", "ai_model_changelog": "", "ai_model_feed": "",
            "feed_prompt": "", "auto_change_notes": false, "ai_style_prompt": "", "accept_mentions": true, "update_check": false,
            "show_responses_default": responses_default, "highlight_generated_default": false,
            "picker_typing": "auto", "update_feed_url": "",
            "update_notice_ack": false }),
        v,
    )
}

/// A contract `Mention` from whatever a test seeded.
fn mention_json(v: &Value) -> Value {
    overlay(
        json!({ "id": "", "source": "", "target": "", "target_item_id": "", "status": "pending",
            "relation": null, "source_origin": null, "source_id": null, "source_kind": null,
            "source_version": null, "source_author_json": null, "source_page": null,
            "first_seen": "2030-01-01T00:00:00Z", "last_seen": "2030-01-01T00:00:00Z",
            "verified_at": null, "attempts": 0, "error": null, "hidden": false }),
        v,
    )
}

/// The fork's extension routes, which a stock server doesn't have.
fn is_fork_only(method: &str, segs: &[&str]) -> bool {
    matches!(
        (method, segs),
        (_, ["api", "items", _, "tk-provenance"])
            | (_, ["api", "reading", "imported"])
            | ("PUT", ["api", "reading", _, _, "read"])
            | ("POST", ["api", "reading", "read"])
    )
}

/// Upstream's `ReadingEntry` for one (full) extension row.
fn stock_entry(r: &Value) -> Value {
    let withdrawn = r["state"] == "tombstone";
    json!({
        "key": format!("{}:{}", r["subscription_id"].as_str().unwrap_or(""), r["remote_id"].as_str().unwrap_or("")),
        "source": "imported", "kind": r["kind"], "withdrawn": withdrawn, "l0": false,
        "contentHtml": r["content_html"], "displayAt": r["observed_at"],
        "imported": {
            "subscriptionId": r["subscription_id"], "subscriptionTitle": r["subscription_title"],
            "remoteId": r["remote_id"], "kind": r["kind"], "withdrawn": withdrawn, "l0": false,
            "updated": r["updated"], "observedAt": r["observed_at"], "contentHtml": r["content_html"],
            "pinnedVersionRetained": r["pinned_version_retained"], "sourceUrl": r["page"],
        },
    })
}

/// Upstream's `ImportedItem` for one (full) extension row: the JSON columns
/// as strings, as the importer stores them. `pre_lineage`: as a studio
/// before 0.18 has it, without `stub_of_json`/`forked_from_json`.
fn imported_item(r: &Value, pre_lineage: bool) -> Value {
    let as_json = |v: &Value| {
        if v.is_null() {
            Value::Null
        } else {
            Value::String(v.to_string())
        }
    };
    let mut v = json!({
        "subscription_id": r["subscription_id"], "remote_id": r["remote_id"], "kind": r["kind"],
        "state": r["state"], "version": r["version"], "created": r["created"], "updated": r["updated"],
        "observed_at": r["observed_at"], "content_md": r["content_md"], "content_html": r["content_html"],
        "content_hash": null, "author_json": as_json(&r["author"]), "media_json": null,
        "transclusions_json": as_json(&r["transclusions"]), "l0": false,
        "pinned_version_retained": r["pinned_version_retained"], "page": r["page"],
        "stub_of_json": as_json(&r["stub_of"]), "forked_from_json": as_json(&r["forked_from"]),
    });
    if pre_lineage {
        let o = v.as_object_mut().unwrap();
        o.remove("stub_of_json");
        o.remove("forked_from_json");
    }
    v
}

/// An extension reading row from whatever a test seeded.
fn reading_row_json(v: &Value) -> Value {
    overlay(
        json!({ "subscription_id": "", "remote_id": "", "subscription_title": "", "origin": "",
            "kind": "fragment", "state": "current", "version": 1, "created": null, "updated": null,
            "observed_at": "2030-01-01T00:00:00Z", "content_md": "", "content_html": "",
            "author": null, "page": null, "thumb": null, "hoppers": [], "read_version": null,
            "transclusions": null, "pinned_version_retained": null, "stub_of": null,
            "forked_from": null }),
        v,
    )
}

/// A contract `Hopper` (the seed's `count` is the detail's `total`).
fn hopper_json(v: &Value) -> Value {
    let mut h = overlay(
        json!({ "id": "", "name": "", "slug": null, "public": false,
            "created": "2030-01-01T00:00:00Z", "slug_frozen": false, "description": null }),
        v,
    );
    h.as_object_mut().unwrap().remove("count");
    h
}

/// `{items, total, offset, limit}` over `all`, by the request's paging.
fn page(req: &Req, all: Vec<Value>, max: usize) -> Value {
    let offset = req
        .query_param("offset")
        .and_then(|o| o.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = req
        .query_param("limit")
        .and_then(|l| l.parse::<usize>().ok())
        .unwrap_or(max.min(50))
        .min(max);
    let total = all.len();
    let items: Vec<Value> = all.into_iter().skip(offset).take(limit).collect();
    json!({ "items": items, "total": total, "offset": offset, "limit": limit })
}

/// Percent-decode one path segment.
fn decode(seg: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", seg.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn not_found() -> (u16, Value) {
    (404, json!({ "error": "not found" }))
}

fn route(req: &Req, s: &mut State) -> (u16, Value) {
    let q = if req.query.is_empty() {
        String::new()
    } else {
        format!("?{}", req.query)
    };
    s.log.push(format!("{} {}{}", req.method, req.path, q));
    if !req.path.starts_with("/api/") {
        // Public static surface: no auth expected (record what arrived).
        s.public_auth
            .push(req.header("authorization").map(str::to_string));
        s.public_headers.push(
            req.headers
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
                .collect(),
        );
        if let ("GET", Some(t)) = (req.method.as_str(), s.public_text.get(&req.path)) {
            // A JSON string body is written raw, as XML (see `handle`).
            return (200, Value::String(t.clone()));
        }
        return match (req.method.as_str(), s.public.get(&req.path)) {
            ("GET", Some(v)) => (200, v.clone()),
            _ => not_found(),
        };
    }
    let bearer = !s.no_bearer && req.header("authorization") == Some(&format!("Bearer {TOKEN}"));
    let session = req
        .header("cookie")
        .and_then(|c| {
            c.split(';')
                .find_map(|p| p.trim().strip_prefix("blyg_session="))
        })
        .is_some_and(|v| s.sessions.iter().any(|x| x == v));
    if !bearer && !session {
        return (401, json!({ "error": "unauthorized" }));
    }
    let segs: Vec<&str> = req.path.trim_start_matches('/').split('/').collect();
    let body = req.json();
    if s.legacy_api {
        return match (req.method.as_str(), segs.as_slice()) {
            ("GET", ["api", "items"]) => {
                let all: Vec<Value> = s.items.values().map(item_json).collect();
                (200, json!({ "items": all }))
            }
            ("POST", ["api", "items"]) => {
                let id = s.add_item("fragment", body["content_md"].as_str().unwrap_or(""));
                (
                    201,
                    json!({ "id": id, "kind": "fragment", "status": "draft" }),
                )
            }
            _ => not_found(),
        };
    }
    if s.stock && is_fork_only(&req.method, &segs) {
        return not_found();
    }
    match (req.method.as_str(), segs.as_slice()) {
        // ---- upstream reading (a stock server's only reading routes)
        ("GET", ["api", "reading"]) => {
            let Some(rows) = s.reading.clone() else {
                return not_found();
            };
            let all: Vec<Value> = rows
                .iter()
                .map(|r| stock_entry(&reading_row_json(r)))
                .collect();
            let mut p = page(req, all, 50);
            p["counts"] = json!({ "all": rows.len(), "own": 0, "subscriptions": {} });
            p["selected"] = json!("all");
            (200, p)
        }
        ("GET", ["api", "imports", sub, rid]) => {
            let (sub, rid) = (decode(sub), decode(rid));
            match s.reading.as_ref().and_then(|rows| {
                rows.iter()
                    .find(|r| {
                        r["subscription_id"] == sub.as_str() && r["remote_id"] == rid.as_str()
                    })
                    .cloned()
            }) {
                Some(r) => (
                    200,
                    imported_item(&reading_row_json(&r), s.pre_lineage_imports),
                ),
                None => not_found(),
            }
        }
        ("GET", ["api", "signals"]) => {
            let mut rows: Vec<Value> = s
                .reading
                .iter()
                .flatten()
                .filter(|r| r["thumb"].is_i64())
                .map(|r| {
                    json!({ "subscription_id": r["subscription_id"], "remote_id": r["remote_id"],
                    "thumb": r["thumb"], "at": "2030-01-01T00:00:00Z" })
                })
                .collect();
            for ((sub, rid), t) in &s.signals {
                rows.retain(|r| {
                    !(r["subscription_id"] == sub.as_str() && r["remote_id"] == rid.as_str())
                });
                rows.push(
                    json!({ "subscription_id": sub, "remote_id": rid, "thumb": t,
                    "at": "2030-01-01T00:00:00Z" }),
                );
            }
            (200, page(req, rows, 100))
        }
        ("GET", ["api", "items"]) => {
            let mut v: Vec<&SItem> = s.items.values().collect();
            v.sort_by(|a, b| b.updated.cmp(&a.updated));
            let all = v.into_iter().map(item_json).collect();
            (200, page(req, all, 100))
        }
        ("POST", ["api", "items"]) => match body["mode"].as_str() {
            None | Some("blank") => {
                let kind = if body["kind"] == "thread" {
                    "thread"
                } else {
                    "fragment"
                };
                let id = s.add_item(kind, body["content_md"].as_str().unwrap_or(""));
                let it = s.items.get_mut(&id).unwrap();
                if let Some(st) = body.get("stub_of") {
                    it.stub_of = st.clone();
                }
                (201, item_json(it))
            }
            Some("fork") => {
                let id = s.add_item("fragment", "forked content");
                let it = s.items.get_mut(&id).unwrap();
                it.forked_from = body["source"].clone();
                (201, item_json(it))
            }
            Some(_) => (400, json!({ "error": "unsupported mode in this mock" })),
        },
        ("GET", ["api", "items", id]) => match s.items.get(*id) {
            None => not_found(),
            Some(it) => (200, item_detail(it)),
        },
        ("PATCH", ["api", "items", id]) => {
            let Some(it) = s.items.get(*id).cloned() else {
                return not_found();
            };
            if let Some(k) = body["kind"].as_str()
                && k != it.kind
                && it.version > 0
            {
                return (
                    409,
                    json!({ "error": "kind is fixed after the first publication" }),
                );
            }
            if let Some(content) = body["content_md"].as_str() {
                s.edit(id, content);
            }
            let it = s.items.get_mut(*id).unwrap();
            if let Some(k) = body["kind"].as_str() {
                it.kind = k.into();
            }
            if let Some(r) = body["responses"].as_str() {
                it.responses = r.into();
            }
            if let Some(st) = body.get("stub_of") {
                it.stub_of = st.clone();
            }
            (200, item_json(it))
        }
        ("DELETE", ["api", "items", id]) => match s.items.get(*id) {
            None => not_found(),
            Some(it) if it.version > 0 => (
                409,
                json!({ "error": "published items are withdrawn, not deleted" }),
            ),
            Some(_) => {
                s.items.remove(*id);
                (200, json!({ "ok": true, "outcome": "discarded" }))
            }
        },
        ("PUT", ["api", "items", id, "versions", v, "pin"]) => {
            let Some(it) = s.items.get_mut(*id) else {
                return not_found();
            };
            let Ok(v) = v.parse::<u64>() else {
                return (400, json!({ "error": "version must be an integer" }));
            };
            let Some(ver) = it.versions.iter_mut().find(|x| x["version"] == v) else {
                return (404, json!({ "error": "version not found" }));
            };
            if ver["kind"] == "withdrawn" {
                return (409, json!({ "error": "cannot pin an endcap version" }));
            }
            let already = ver["pinned"] == true;
            ver["pinned"] = json!(true);
            (200, json!({ "ok": true, "version": v, "already": already }))
        }
        ("POST", ["api", "items", id, action]) => {
            let now = s.now();
            let Some(it) = s.items.get_mut(*id) else {
                return not_found();
            };
            match *action {
                "publish" => {
                    if it.kind == "fragment" && it.content_md.chars().count() > 1000 {
                        return (400, json!({ "error": "fragment exceeds 1000 characters" }));
                    }
                    if it.content_md.contains("![[BAD") {
                        return (
                            400,
                            json!({ "error": "one or more references do not resolve",
                                    "errors": [{ "directive": "![[BAD]]", "reason": "unknown item" }] }),
                        );
                    }
                    // Protocol 0.3: an unresolvable `[[id]]` link fails publish too.
                    if it.content_md.contains("[[BAD") {
                        return (
                            400,
                            json!({ "error": "one or more references do not resolve",
                                    "errors": [{ "directive": "[[BAD]]", "reason": "unknown item" }] }),
                        );
                    }
                    it.version += 1;
                    it.status = "public".into();
                    it.dirty = false;
                    it.updated = now.clone();
                    let note = body.get("note").cloned().unwrap_or(Value::Null);
                    let v = version_json(it, it.version, &now, note, false);
                    it.versions.push(v);
                    (200, json!({ "ok": true, "version": it.version }))
                }
                "withdraw" => {
                    if it.status == "withdrawn" {
                        return (409, json!({ "error": "already withdrawn" }));
                    }
                    if it.status != "public" {
                        return (409, json!({ "error": "not published" }));
                    }
                    it.version += 1;
                    it.status = "withdrawn".into();
                    let v = version_json(it, it.version, &now, Value::Null, true);
                    it.versions.push(v);
                    (200, json!({ "ok": true, "version": it.version }))
                }
                "restore" => {
                    let Some(v) = body["version"].as_u64() else {
                        return (400, json!({ "error": "version required" }));
                    };
                    it.content_md = format!("restored v{v}");
                    it.dirty = true;
                    (
                        200,
                        json!({ "ok": true, "restored": v, "publishesAs": it.version + 1 }),
                    )
                }
                _ => not_found(),
            }
        }
        ("POST", ["api", "media"]) => {
            let ct = req.header("content-type").unwrap_or("");
            let text = String::from_utf8_lossy(&req.body);
            if !ct.starts_with("multipart/form-data; boundary=") || !text.contains("name=\"file\"")
            {
                return (400, json!({ "error": "file field required (multipart)" }));
            }
            if !text.contains("Content-Type: image/png") {
                return (415, json!({ "error": "unsupported type" }));
            }
            s.media_bodies.push(req.body.clone());
            let id = s.new_id("M");
            (
                201,
                json!({ "id": id, "url": format!("media/{id}.png"), "mime": "image/png" }),
            )
        }
        ("DELETE", ["api", "media", _]) => (200, json!({ "ok": true, "outcome": "deleted" })),
        // ---- subscriptions
        ("GET", ["api", "subscriptions"]) => {
            let all = s.subs.iter().map(sub_json).collect();
            (200, page(req, all, 100))
        }
        ("POST", ["api", "subscriptions"]) => {
            let Some(url) = body["url"].as_str().map(str::to_string) else {
                return (400, json!({ "error": "url required" }));
            };
            if url.contains("nowhere") {
                return (
                    422,
                    json!({ "error": "could not resolve this URL to a blyg or a feed", "tried": [] }),
                );
            }
            // --- profiles --- a feed URL previews as RSS.
            if body["confirm"] != true && (url.ends_with(".xml") || url.contains("/rss")) {
                return (
                    200,
                    json!({ "needsConfirm": true, "kind": "rss", "feedUrl": url, "title": "A feed" }),
                );
            }
            if body["confirm"] != true {
                return (
                    200,
                    json!({ "needsConfirm": true, "kind": "blyg", "origin": url, "title": "Their blyg" }),
                );
            }
            let id = s.new_id("SUB");
            let title = body["title"].as_str().unwrap_or("Their blyg").to_string();
            let sub = sub_json(&json!({ "id": id, "kind": "blyg", "origin": url,
                "feed_url": format!("{url}feed.xml"), "title": title }));
            s.subs.push(sub.clone());
            (201, sub)
        }
        ("PATCH", ["api", "subscriptions", id]) => {
            let Some(sub) = s.subs.iter_mut().find(|x| x["id"] == *id) else {
                return not_found();
            };
            if let Some(b) = body["in_blogroll"].as_bool() {
                sub["in_blogroll"] = json!(b);
            }
            if let Some(t) = body["title"].as_str() {
                sub["title"] = json!(t);
            }
            if let Some(p) = body["paused"].as_bool() {
                sub["status"] = json!(if p { "paused" } else { "active" });
            }
            (200, sub_json(sub))
        }
        ("DELETE", ["api", "subscriptions", id]) => {
            let before = s.subs.len();
            s.subs.retain(|x| x["id"] != *id);
            if s.subs.len() == before {
                return not_found();
            }
            (200, json!({ "ok": true }))
        }
        ("POST", ["api", "subscriptions", id, "resync"]) => {
            if !s.subs.iter().any(|x| x["id"] == *id) {
                return not_found();
            }
            (200, json!({ "ok": true, "changed": 0 }))
        }
        // ---- signals / mentions / settings
        ("PUT", ["api", "signals", sub, rid]) => {
            let t = body["thumb"].as_i64();
            if t != Some(1) && t != Some(-1) {
                return (400, json!({ "error": "thumb must be 1 or -1" }));
            }
            s.signals
                .insert((sub.to_string(), rid.to_string()), t.unwrap());
            (200, json!({ "ok": true }))
        }
        ("DELETE", ["api", "signals", sub, rid]) => {
            s.signals.remove(&(sub.to_string(), rid.to_string()));
            (200, json!({ "ok": true }))
        }
        ("PATCH", ["api", "mentions", id]) => {
            let Some(h) = body["hidden"].as_bool() else {
                return (400, json!({ "error": "hidden must be a boolean" }));
            };
            s.hidden.insert(id.to_string(), h);
            (200, json!({ "ok": true, "hidden": h }))
        }
        ("PATCH", ["api", "settings"]) => {
            s.settings_puts.push(body.clone());
            let cur = settings_json(
                &s.settings.clone().unwrap_or(json!({})),
                s.responses_default,
            );
            let next = overlay(cur, &body);
            if let Some(d) = next["show_responses_default"].as_bool() {
                s.responses_default = d;
            }
            if s.settings.is_some() {
                s.settings = Some(next.clone());
            }
            (200, next)
        }
        // ---- reads (404 until "deployed")
        // Extension 3: the app's reading rows (upstream's `/api/reading`
        // is a different, rendered resource the app doesn't use).
        ("GET", ["api", "reading", "imported"]) => {
            let Some(all) = s.reading.clone() else {
                return not_found();
            };
            let limit = req
                .query_param("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(100)
                .min(500);
            let limit = limit.min(s.reading_page_size);
            // opaque cursor: "c:<index>"
            let start = req
                .query_param("before")
                .and_then(|c| c.strip_prefix("c:").and_then(|n| n.parse::<usize>().ok()))
                .unwrap_or(0);
            let mut page: Vec<Value> = all.iter().skip(start).take(limit).cloned().collect();
            let next = if start + limit < all.len() {
                json!(format!("c:{}", start + limit))
            } else {
                Value::Null
            };
            if !s.read_sync {
                let page: Vec<Value> = page.iter().map(reading_row_json).collect();
                let mut body = json!({ "items": page, "next": next });
                if s.reading_lineage {
                    body["lineage"] = json!(true);
                }
                return (200, body);
            }
            for it in &mut page {
                if it.get("read_version").is_some() {
                    continue; // a test wants this exact value served
                }
                let key = (
                    it["subscription_id"].as_str().unwrap_or("").to_string(),
                    it["remote_id"].as_str().unwrap_or("").to_string(),
                );
                it["read_version"] = s.reads.get(&key).map_or(Value::Null, |v| json!(v));
            }
            let page: Vec<Value> = page.iter().map(reading_row_json).collect();
            let mut body = json!({ "items": page, "next": next, "read_state": true });
            if s.reading_lineage {
                body["lineage"] = json!(true);
            }
            (200, body)
        }
        // ---- extension 5 (404 until "deployed")
        ("PUT", ["api", "reading", sub, rid, "read"]) if s.read_sync => {
            let Some(v) = body["version"].as_u64() else {
                return (
                    400,
                    json!({ "error": "version must be a non-negative integer" }),
                );
            };
            let e = s
                .reads
                .entry((decode(sub), decode(rid)))
                .or_insert(v as u32);
            *e = (*e).max(v as u32);
            (
                200,
                json!({ "ok": true, "stored": true, "read_version": *e }),
            )
        }
        ("POST", ["api", "reading", "read"]) if s.read_sync => {
            let Some(items) = body["items"].as_array().cloned() else {
                return (400, json!({ "error": "items array required" }));
            };
            if items.len() > 500 {
                return (400, json!({ "error": "at most 500 items per call" }));
            }
            s.read_batches.push(body.clone());
            for it in &items {
                let (Some(sub), Some(rid), Some(v)) = (
                    it["sub"].as_str(),
                    it["remote_id"].as_str(),
                    it["version"].as_u64(),
                ) else {
                    return (400, json!({ "error": "invalid items" }));
                };
                let e = s
                    .reads
                    .entry((sub.to_string(), rid.to_string()))
                    .or_insert(v as u32);
                *e = (*e).max(v as u32);
            }
            (200, json!({ "ok": true, "received": items.len() }))
        }
        ("GET", ["api", "mentions"]) => match &s.mentions {
            None => not_found(),
            Some(m) => {
                let all: Vec<Value> = m.iter().map(mention_json).collect();
                let mut p = page(req, all, 100);
                p["direction"] = json!(req.query_param("direction").unwrap_or("inbound".into()));
                (200, p)
            }
        },
        ("GET", ["api", "settings"]) => match &s.settings {
            None => not_found(),
            Some(v) => (200, settings_json(v, s.responses_default)),
        },
        ("GET", ["api", "hoppers"]) => match &s.hoppers {
            None => not_found(),
            Some(h) => {
                let all = h.iter().map(hopper_json).collect();
                (200, page(req, all, 100))
            }
        },
        ("GET", ["api", "hoppers", id]) => {
            let Some(h) = s
                .hoppers
                .as_ref()
                .and_then(|h| h.iter().find(|x| x["id"] == *id))
            else {
                return not_found();
            };
            let members: Vec<Value> = s
                .reading
                .iter()
                .flatten()
                .filter(|r| {
                    r["hoppers"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|x| x == *id))
                })
                .map(|r| {
                    json!({ "hopper_id": id, "subscription_id": r["subscription_id"],
                    "remote_id": r["remote_id"], "added_at": "2030-01-01T00:00:00Z" })
                })
                .collect();
            let total = h["count"].as_u64().unwrap_or(members.len() as u64);
            (
                200,
                json!({ "hopper": hopper_json(h), "memberships": members, "items": [],
                    "total": total, "source_count": 0 }),
            )
        }
        _ => not_found(),
    }
}

// --------------------------------------------------------------- helpers

pub struct Env {
    pub mock: Mock,
    pub dir: tempfile::TempDir,
    pub events: Arc<Mutex<Vec<CoreEvent>>>,
}

impl Env {
    pub fn new() -> Env {
        Env {
            mock: Mock::start(),
            dir: tempfile::tempdir().unwrap(),
            events: Arc::default(),
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// Backend with no background worker: sync happens only on `sync_now`,
    /// `pull_now` and the remote methods.
    pub fn manual(&self) -> LiveBackend {
        self.open(SyncOptions {
            start_worker: false,
            ..fast()
        })
    }

    pub fn open(&self, opts: SyncOptions) -> LiveBackend {
        self.open_token(opts, TOKEN)
    }

    pub fn open_token(&self, opts: SyncOptions, token: &str) -> LiveBackend {
        self.open_cred(opts, token)
    }

    pub fn open_cred(
        &self,
        opts: SyncOptions,
        cred: impl Into<blyg_core::api::auth::Credential>,
    ) -> LiveBackend {
        let b = LiveBackend::open_with(&self.data_dir(), &self.mock.url, cred, opts).unwrap();
        let ev = self.events.clone();
        b.set_event_sink(Box::new(move |e| ev.lock().unwrap().push(e)));
        b
    }

    pub fn events(&self) -> Vec<CoreEvent> {
        self.events.lock().unwrap().clone()
    }
}

pub fn fast() -> SyncOptions {
    SyncOptions {
        debounce: Duration::from_millis(100),
        pull_interval: Duration::from_millis(300),
        backoff_initial: Duration::from_millis(50),
        backoff_max: Duration::from_millis(200),
        start_worker: true,
        reading_pages: 4,
        // Seeded rows name made-up origins: never fetch from them.
        lineage_fetches: 0,
    }
}

/// Poll `f` until it's true or `timeout` passes.
pub fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    f()
}
