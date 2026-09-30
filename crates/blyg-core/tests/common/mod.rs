//! A tiny in-process mock of the blyg owner API (std TcpListener, one thread
//! per connection, `Connection: close`). Enough of `apps/blyg/src/*api*.ts`
//! to exercise blyg-core; never talks to a real server.
//!
//! `set_down(true)` makes it accept and immediately drop connections, which
//! the client sees as a transport failure (= offline).

#![allow(dead_code)]

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
    pub versions: Vec<Value>,
    pub show_responses: bool,
    pub forked_from: Value,
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
    pub settings_puts: Vec<Value>,
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
                show_responses: false,
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
    let (status, body) = route(&req, &mut state.lock().unwrap());
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

fn item_json(it: &SItem) -> Value {
    json!({
        "id": it.id,
        "kind": if it.status == "withdrawn" { "withdrawn" } else { it.kind.as_str() },
        "authored_kind": it.kind,
        "status": it.status,
        "version": it.version,
        "dirty": it.dirty,
        "created": it.created,
        "updated": it.updated,
        "content_md": it.content_md,
        "stub_of": null,
        "forked_from": it.forked_from,
        "permalink": if it.version > 0 {
            Value::String(format!("http://mock.test/{}/{}", if it.kind == "thread" { "t" } else { "f" }, it.id))
        } else { Value::Null },
        "show_responses": it.show_responses,
    })
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
    if req.header("authorization") != Some(&format!("Bearer {TOKEN}")) {
        return (401, json!({ "error": "unauthorized" }));
    }
    let segs: Vec<&str> = req.path.trim_start_matches('/').split('/').collect();
    let body = req.json();
    match (req.method.as_str(), segs.as_slice()) {
        ("GET", ["api", "items"]) => {
            let mut v: Vec<&SItem> = s.items.values().collect();
            v.sort_by(|a, b| b.updated.cmp(&a.updated));
            (
                200,
                json!({ "items": v.into_iter().map(item_json).collect::<Vec<_>>() }),
            )
        }
        ("POST", ["api", "items"]) => {
            let kind = if body["kind"] == "thread" {
                "thread"
            } else {
                "fragment"
            };
            let id = s.add_item(kind, body["content_md"].as_str().unwrap_or(""));
            (201, json!({ "id": id, "kind": kind, "status": "draft" }))
        }
        ("GET", ["api", "items", id]) => match s.items.get(*id) {
            None => not_found(),
            Some(it) => {
                let mut v = item_json(it);
                v["versions"] = json!(it.versions);
                (200, v)
            }
        },
        ("PUT", ["api", "items", id]) => {
            let Some(content) = body["content_md"].as_str().map(str::to_string) else {
                return (400, json!({ "error": "content_md required" }));
            };
            if !s.items.contains_key(*id) {
                return not_found();
            }
            s.edit(id, &content);
            (200, json!({ "ok": true }))
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
                            json!({ "error": "one or more transclusions do not resolve",
                                    "errors": [{ "directive": "![[BAD]]", "reason": "unknown item" }] }),
                        );
                    }
                    it.version += 1;
                    it.status = "public".into();
                    it.dirty = false;
                    it.updated = now.clone();
                    it.versions.push(json!({ "version": it.version, "published_at": now,
                        "note": body.get("note").cloned().unwrap_or(Value::Null), "pinned": false, "endcap": false }));
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
                    it.versions
                        .push(json!({ "version": it.version, "published_at": now,
                        "note": null, "pinned": false, "endcap": true }));
                    (200, json!({ "ok": true, "version": it.version }))
                }
                "pin" => {
                    let Some(v) = body["version"].as_u64() else {
                        return (400, json!({ "error": "version required" }));
                    };
                    let Some(ver) = it.versions.iter_mut().find(|x| x["version"] == v) else {
                        return (404, json!({ "error": "version not found" }));
                    };
                    if ver["endcap"] == true {
                        return (409, json!({ "error": "cannot pin an endcap version" }));
                    }
                    let already = ver["pinned"] == true;
                    ver["pinned"] = json!(true);
                    (200, json!({ "ok": true, "version": v, "already": already }))
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
        ("PUT", ["api", "items", id, "responses"]) => {
            let Some(show) = body["show"].as_bool() else {
                return (400, json!({ "error": "show must be a boolean" }));
            };
            let Some(it) = s.items.get_mut(*id) else {
                return not_found();
            };
            it.show_responses = show;
            (200, json!({ "ok": true, "show_responses": show }))
        }
        ("POST", ["api", "fork"]) => {
            let id = s.add_item("fragment", "forked content");
            let it = s.items.get_mut(&id).unwrap();
            it.forked_from = body.clone();
            (
                201,
                json!({ "id": id, "kind": "fragment", "status": "draft" }),
            )
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
        ("DELETE", ["api", "media", _]) => (200, json!({ "ok": true })),
        // ---- subscriptions
        ("GET", ["api", "subscriptions"]) => (200, json!({ "subscriptions": s.subs })),
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
                    json!({ "needsConfirm": true, "kind": "blyg", "origin": url, "title": "Their blyg", "siteMismatch": false }),
                );
            }
            let id = s.new_id("SUB");
            let title = body["title"].as_str().unwrap_or("Their blyg").to_string();
            s.subs.push(json!({ "id": id, "kind": "blyg", "origin": url, "feed_url": format!("{url}feed.xml"),
                "title": title, "status": "active", "in_blogroll": false }));
            (201, json!({ "id": id, "kind": "blyg", "origin": url }))
        }
        ("PUT", ["api", "subscriptions", id]) => {
            let Some(sub) = s.subs.iter_mut().find(|x| x["id"] == *id) else {
                return not_found();
            };
            if let Some(b) = body["in_blogroll"].as_bool() {
                sub["in_blogroll"] = json!(b);
            }
            if let Some(t) = body["title"].as_str() {
                sub["title"] = json!(t);
            }
            (200, json!({ "ok": true }))
        }
        ("DELETE", ["api", "subscriptions", id]) => {
            let before = s.subs.len();
            s.subs.retain(|x| x["id"] != *id);
            if s.subs.len() == before {
                return not_found();
            }
            (200, json!({ "ok": true }))
        }
        ("POST", ["api", "subscriptions", id, action]) => {
            let Some(sub) = s.subs.iter_mut().find(|x| x["id"] == *id) else {
                return not_found();
            };
            match *action {
                "pause" => sub["status"] = json!("paused"),
                "resume" => sub["status"] = json!("active"),
                "resync" => return (200, json!({ "ok": true, "changed": false })),
                _ => return not_found(),
            }
            (200, json!({ "ok": true }))
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
        ("PUT", ["api", "mentions", id, "hidden"]) => {
            let Some(h) = body["hidden"].as_bool() else {
                return (400, json!({ "error": "hidden must be a boolean" }));
            };
            s.hidden.insert(id.to_string(), h);
            (200, json!({ "ok": true, "hidden": h }))
        }
        ("PUT", ["api", "settings"]) => {
            s.settings_puts.push(body.clone());
            (200, json!({ "ok": true }))
        }
        // ---- patch 3 (404 until "deployed")
        ("GET", ["api", "reading"]) => {
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
                return (200, json!({ "items": page, "next": next }));
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
            (
                200,
                json!({ "items": page, "next": next, "read_state": true }),
            )
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
            Some(m) => (200, json!({ "mentions": m })),
        },
        ("GET", ["api", "settings"]) => match &s.settings {
            None => not_found(),
            Some(v) => (200, v.clone()),
        },
        ("GET", ["api", "hoppers"]) => match &s.hoppers {
            None => not_found(),
            Some(h) => (200, json!({ "hoppers": h })),
        },
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
        let b = LiveBackend::open_with(&self.data_dir(), &self.mock.url, token, opts).unwrap();
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
