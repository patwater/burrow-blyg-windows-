//! Blocking owner-API client (ureq + rustls). One method per endpoint in
//! docs/SPEC.md §API. Every call sends the bearer token; the token is never
//! logged, printed or included in errors (`Debug` redacts it).
//!
//! Error mapping: transport failures → `Offline`, 401 → `Unauthorized`,
//! 404 → `NotFound`, any other non-2xx → `Rejected{status, error, errors}`.
//! Patch-3 read endpoints (`reading`, `mentions`, `settings`, `hoppers`) return
//! `Ok(None)` on 404: the server simply doesn't have them yet. The read-state
//! writes (extension 5) are only called once `GET /api/reading` advertised
//! `read_state: true`.

pub mod public;
pub mod wire;

use std::io::Read;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::backend::{CoreError, Result};
use crate::model::{
    Hopper, Kind, Mention, RemoteRef, ScopeProvenance, Settings, SubscribePreview, Subscription,
    SubscriptionKind,
};
use wire::*;

pub struct Api {
    agent: ureq::Agent,
    base: String,
    token: String,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("base", &self.base)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Result of `POST /api/items/:id/pin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pinned {
    pub already: bool,
}

impl Api {
    pub fn new(base_url: &str, token: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30))
            .timeout_write(Duration::from_secs(60))
            .build();
        Api {
            agent,
            base: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn request(&self, method: &str, path: &str) -> ureq::Request {
        self.agent
            .request(method, &format!("{}{}", self.base, path))
            .set("authorization", &format!("Bearer {}", self.token))
            .set("accept", "application/json")
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let req = self.request(method, path);
        let res = match body {
            Some(b) => req.send_json(b),
            None => req.call(),
        };
        read_json(res)
    }

    fn call_as<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let v = self.call(method, path, body)?;
        serde_json::from_value(v)
            .map_err(|e| CoreError::Other(format!("unexpected response from {path}: {e}")))
    }

    /// 404 → `None` (endpoint not deployed yet).
    fn optional<T>(r: Result<T>) -> Result<Option<T>> {
        match r {
            Ok(v) => Ok(Some(v)),
            Err(CoreError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    // ---------- items ----------

    pub fn list_items(&self) -> Result<Vec<WireItem>> {
        #[derive(serde::Deserialize)]
        struct R {
            items: Vec<WireItem>,
        }
        Ok(self.call_as::<R>("GET", "/api/items", None)?.items)
    }

    pub fn get_item(&self, id: &str) -> Result<WireItem> {
        self.call_as("GET", &format!("/api/items/{}", enc(id)), None)
    }

    pub fn create_item(
        &self,
        content_md: &str,
        kind: Kind,
        stub_of: Option<&RemoteRef>,
    ) -> Result<String> {
        let mut body = json!({ "content_md": content_md, "kind": kind_str(kind) });
        if let Some(s) = stub_of {
            body["stub_of"] = serde_json::to_value(s).unwrap_or(Value::Null);
        }
        Ok(self
            .call_as::<Created>("POST", "/api/items", Some(body))?
            .id)
    }

    pub fn save_item(&self, id: &str, content_md: &str) -> Result<()> {
        self.call(
            "PUT",
            &format!("/api/items/{}", enc(id)),
            Some(json!({ "content_md": content_md })),
        )
        .map(|_| ())
    }

    pub fn publish(&self, id: &str, note: Option<&str>) -> Result<Published> {
        self.call_as(
            "POST",
            &format!("/api/items/{}/publish", enc(id)),
            Some(note_body(note)),
        )
    }

    pub fn withdraw(&self, id: &str, note: Option<&str>) -> Result<u32> {
        #[derive(serde::Deserialize)]
        struct R {
            version: u32,
        }
        Ok(self
            .call_as::<R>(
                "POST",
                &format!("/api/items/{}/withdraw", enc(id)),
                Some(note_body(note)),
            )?
            .version)
    }

    pub fn pin(&self, id: &str, version: u32) -> Result<Pinned> {
        #[derive(serde::Deserialize)]
        struct R {
            #[serde(default)]
            already: bool,
        }
        let r: R = self.call_as(
            "POST",
            &format!("/api/items/{}/pin", enc(id)),
            Some(json!({ "version": version })),
        )?;
        Ok(Pinned { already: r.already })
    }

    pub fn restore(&self, id: &str, version: u32) -> Result<()> {
        self.call(
            "POST",
            &format!("/api/items/{}/restore", enc(id)),
            Some(json!({ "version": version })),
        )
        .map(|_| ())
    }

    pub fn delete_item(&self, id: &str) -> Result<()> {
        self.call("DELETE", &format!("/api/items/{}", enc(id)), None)
            .map(|_| ())
    }

    pub fn fork(&self, of: &RemoteRef) -> Result<String> {
        let body = json!({ "origin": of.origin, "id": of.id, "version": of.version });
        Ok(self.call_as::<Created>("POST", "/api/fork", Some(body))?.id)
    }

    /// `GET /api/items/:id/tk-provenance` → the server's per-scope cache.
    /// `None` when the endpoint isn't there (extension 4 missing).
    pub fn get_tk_provenance(&self, id: &str) -> Result<Option<Vec<Option<ScopeProvenance>>>> {
        #[derive(serde::Deserialize)]
        struct R {
            #[serde(default)]
            scopes: Vec<Option<ScopeProvenance>>,
        }
        Self::optional(self.call_as::<R>(
            "GET",
            &format!("/api/items/{}/tk-provenance", enc(id)),
            None,
        ))
        .map(|o| o.map(|r| r.scopes))
    }

    /// `PUT /api/items/:id/tk-provenance {content_md?, scopes}`: the text and
    /// the whole position-keyed provenance array in one atomic write (the
    /// server validates first and writes nothing on a 400). Returns how many
    /// scopes are disclosed. 404 means the endpoint (or item) isn't there.
    pub fn put_tk_provenance(
        &self,
        id: &str,
        content_md: Option<&str>,
        scopes: &[Option<ScopeProvenance>],
    ) -> Result<u32> {
        let entries: Vec<Value> = scopes
            .iter()
            .enumerate()
            .map(|(i, p)| match p {
                None => Value::Null,
                Some(p) => {
                    let mut e = json!({ "index": i, "model": p.model, "sources": p.sources });
                    if let Some(at) = &p.at {
                        e["at"] = json!(at);
                    }
                    e
                }
            })
            .collect();
        let mut body = json!({ "scopes": entries });
        if let Some(c) = content_md {
            body["content_md"] = json!(c);
        }
        let v = self.call(
            "PUT",
            &format!("/api/items/{}/tk-provenance", enc(id)),
            Some(body),
        )?;
        Ok(v.get("disclosed").and_then(Value::as_u64).unwrap_or(0) as u32)
    }

    pub fn set_show_responses(&self, id: &str, show: bool) -> Result<()> {
        self.call(
            "PUT",
            &format!("/api/items/{}/responses", enc(id)),
            Some(json!({ "show": show })),
        )
        .map(|_| ())
    }

    // ---------- media ----------

    pub fn upload_media(
        &self,
        bytes: &[u8],
        mime: &str,
        item_id: Option<&str>,
        alt: Option<&str>,
    ) -> Result<Media> {
        let boundary = format!("----blygger{:x}", crate::util::now_ms() ^ 0x5eed_b1a9);
        let ext = match mime {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/svg+xml" => "svg",
            _ => "bin",
        };
        let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
        let text_field = |body: &mut Vec<u8>, name: &str, value: &str| {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
            );
        };
        if let Some(id) = item_id {
            text_field(&mut body, "item_id", id);
        }
        if let Some(a) = alt {
            text_field(&mut body, "alt", a);
        }
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload.{ext}\"\r\nContent-Type: {mime}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let res = self
            .request("POST", "/api/media")
            .set(
                "content-type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body);
        let v = read_json(res)?;
        serde_json::from_value(v)
            .map_err(|e| CoreError::Other(format!("unexpected media response: {e}")))
    }

    /// `DELETE /api/media/:id` (patch 8): removes the row and the stored
    /// file. 404 unknown; 409 when it's the site avatar.
    pub fn delete_media(&self, id: &str) -> Result<()> {
        self.call("DELETE", &format!("/api/media/{}", enc(id)), None)
            .map(|_| ())
    }

    // ---------- subscriptions ----------

    pub fn list_subscriptions(&self) -> Result<Vec<Subscription>> {
        #[derive(serde::Deserialize)]
        struct R {
            subscriptions: Vec<Subscription>,
        }
        Ok(self
            .call_as::<R>("GET", "/api/subscriptions", None)?
            .subscriptions)
    }

    pub fn preview_subscription(&self, url: &str) -> Result<SubscribePreview> {
        let v = self.call("POST", "/api/subscriptions", Some(json!({ "url": url })))?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let kind = if s("kind").as_deref() == Some("rss") {
            SubscriptionKind::Rss
        } else {
            SubscriptionKind::Blyg
        };
        Ok(SubscribePreview {
            kind,
            title: s("title").unwrap_or_default(),
            origin: s("origin"),
            feed_url: s("feedUrl"),
            site_mismatch: v.get("siteMismatch").and_then(Value::as_bool),
        })
    }

    /// `POST /api/subscriptions {url, confirm:true, title?}` → new subscription id.
    pub fn subscribe(&self, url: &str, title: Option<&str>) -> Result<String> {
        let mut body = json!({ "url": url, "confirm": true });
        if let Some(t) = title {
            body["title"] = json!(t);
        }
        Ok(self
            .call_as::<Created>("POST", "/api/subscriptions", Some(body))?
            .id)
    }

    pub fn update_subscription(
        &self,
        id: &str,
        in_blogroll: Option<bool>,
        title: Option<&str>,
    ) -> Result<()> {
        let mut body = json!({});
        if let Some(b) = in_blogroll {
            body["in_blogroll"] = json!(b);
        }
        if let Some(t) = title {
            body["title"] = json!(t);
        }
        self.call(
            "PUT",
            &format!("/api/subscriptions/{}", enc(id)),
            Some(body),
        )
        .map(|_| ())
    }

    pub fn delete_subscription(&self, id: &str) -> Result<()> {
        self.call("DELETE", &format!("/api/subscriptions/{}", enc(id)), None)
            .map(|_| ())
    }

    pub fn pause_subscription(&self, id: &str, paused: bool) -> Result<()> {
        let action = if paused { "pause" } else { "resume" };
        self.call(
            "POST",
            &format!("/api/subscriptions/{}/{action}", enc(id)),
            None,
        )
        .map(|_| ())
    }

    /// `{ok, changed}`; blyg subscriptions only (409 otherwise).
    pub fn resync_subscription(&self, id: &str) -> Result<bool> {
        let v = self.call(
            "POST",
            &format!("/api/subscriptions/{}/resync", enc(id)),
            None,
        )?;
        Ok(v.get("changed").and_then(Value::as_bool).unwrap_or(false))
    }

    // ---------- signals / mentions ----------

    pub fn set_signal(&self, sub: &str, remote_id: &str, thumb: i8) -> Result<()> {
        self.call(
            "PUT",
            &format!("/api/signals/{}/{}", enc(sub), enc(remote_id)),
            Some(json!({ "thumb": thumb })),
        )
        .map(|_| ())
    }

    pub fn clear_signal(&self, sub: &str, remote_id: &str) -> Result<()> {
        self.call(
            "DELETE",
            &format!("/api/signals/{}/{}", enc(sub), enc(remote_id)),
            None,
        )
        .map(|_| ())
    }

    pub fn set_mention_hidden(&self, id: &str, hidden: bool) -> Result<()> {
        self.call(
            "PUT",
            &format!("/api/mentions/{}/hidden", enc(id)),
            Some(json!({ "hidden": hidden })),
        )
        .map(|_| ())
    }

    // ---------- settings ----------

    /// Sends only the fields that are set (the server ignores non-strings anyway).
    pub fn put_settings(&self, s: &Settings) -> Result<()> {
        let mut body = serde_json::Map::new();
        let mut put = |k: &str, v: &Option<String>| {
            if let Some(v) = v {
                body.insert(k.to_string(), json!(v));
            }
        };
        put("site_title", &s.site_title);
        put("author_name", &s.author_name);
        put("author_bio", &s.author_bio);
        put("site_url", &s.site_url);
        put("theme", &s.theme);
        put("avatar_media_id", &s.avatar_media_id);
        body.insert(
            "author_links".into(),
            serde_json::to_value(&s.author_links).unwrap_or(json!([])),
        );
        self.call("PUT", "/api/settings", Some(Value::Object(body)))
            .map(|_| ())
    }

    // ---------- patch 3 (404 → None) ----------

    pub fn reading(&self, limit: u32, before: Option<&str>) -> Result<Option<ReadingPage>> {
        let mut path = format!("/api/reading?limit={limit}");
        if let Some(b) = before {
            path.push_str("&before=");
            path.push_str(&enc(b));
        }
        let page: Option<ReadingPage> = Self::optional(self.call_as("GET", &path, None))?;
        Ok(page.map(|mut p| {
            let read_sync = p.read_sync();
            for it in &mut p.items {
                it.page = it.page.take().and_then(|pg| absolute_page(&it.origin, &pg));
                if !read_sync {
                    // Only a server that advertises read state means it.
                    it.read_version = None;
                }
            }
            p
        }))
    }

    // ---------- extension 5: read state (404 = not deployed) ----------

    /// `PUT /api/reading/:sub/:remoteId/read {version}`. The server keeps
    /// `max(stored, version)`, so a replay is harmless.
    pub fn put_read(&self, sub: &str, remote_id: &str, version: u32) -> Result<()> {
        self.call(
            "PUT",
            &format!("/api/reading/{}/{}/read", enc(sub), enc(remote_id)),
            Some(json!({ "version": version })),
        )
        .map(|_| ())
    }

    /// `POST /api/reading/read {items: [{sub, remote_id, version}]}`, at most
    /// `READ_BATCH_MAX` entries (the caller chunks).
    pub fn put_reads(&self, items: &[ReadMark]) -> Result<()> {
        self.call("POST", "/api/reading/read", Some(json!({ "items": items })))
            .map(|_| ())
    }

    pub fn mentions(&self) -> Result<Option<Vec<Mention>>> {
        #[derive(serde::Deserialize)]
        struct R {
            mentions: Vec<Mention>,
        }
        Self::optional(self.call_as::<R>("GET", "/api/mentions", None))
            .map(|o| o.map(|r| r.mentions))
    }

    /// The server fills unset strings with upstream defaults (`""`, or
    /// `"blyg"`/`"auto"`); `""` becomes `None`.
    pub fn settings(&self) -> Result<Option<Settings>> {
        let s: Option<Settings> = Self::optional(self.call_as("GET", "/api/settings", None))?;
        Ok(s.map(|mut s| {
            for f in [
                &mut s.site_title,
                &mut s.author_name,
                &mut s.author_bio,
                &mut s.site_url,
                &mut s.theme,
                &mut s.avatar_media_id,
            ] {
                if f.as_deref() == Some("") {
                    *f = None;
                }
            }
            s
        }))
    }

    pub fn hoppers(&self) -> Result<Option<Vec<Hopper>>> {
        #[derive(serde::Deserialize)]
        struct R {
            hoppers: Vec<Hopper>,
        }
        Self::optional(self.call_as::<R>("GET", "/api/hoppers", None)).map(|o| o.map(|r| r.hoppers))
    }
}

/// Why a connection check failed, worded for the "Connect your blyg" sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// Nothing answered at that address (DNS, refused, TLS, timeout).
    Unreachable,
    /// The server said 401: the token is wrong.
    WrongToken,
    /// `GET /api/items` is 404: a blyg without the owner-API extensions.
    MissingExtensions,
    /// Anything else (a 5xx, or a page that isn't a blyg's JSON).
    Other(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Unreachable => {
                f.write_str("Couldn't reach that address. Check the URL and your connection.")
            }
            ConnectError::WrongToken => f.write_str(
                "The blyg said the token is wrong (401). Paste the owner token again.",
            ),
            ConnectError::MissingExtensions => f.write_str(
                "This server lacks the owner-API extensions (GET /api/items is 404). See docs/SERVER.md.",
            ),
            ConnectError::Other(m) => f.write_str(m),
        }
    }
}

/// Check a blyg URL + owner token before saving them: `GET /api/items` with
/// the token. Returns how many items the server holds.
pub fn verify_connection(base_url: &str, token: &str) -> std::result::Result<usize, ConnectError> {
    let api = Api::new(base_url, token);
    match api.list_items() {
        Ok(items) => Ok(items.len()),
        Err(CoreError::Offline) => Err(ConnectError::Unreachable),
        Err(CoreError::Unauthorized) => Err(ConnectError::WrongToken),
        Err(CoreError::NotFound) => Err(ConnectError::MissingExtensions),
        Err(CoreError::Rejected {
            status, message, ..
        }) => Err(ConnectError::Other(format!(
            "The server answered {status}: {message}"
        ))),
        Err(CoreError::Other(_)) => Err(ConnectError::Other(
            "That address answered, but not like a blyg (GET /api/items didn't return JSON)."
                .into(),
        )),
        Err(e) => Err(ConnectError::Other(e.to_string())),
    }
}

/// Reading items declare `page` origin-relative (e.g. `f/a1`); resolve it so
/// the UI can open it directly. Absolute URLs pass through.
pub fn absolute_page(origin: &str, page: &str) -> Option<String> {
    if page.is_empty() {
        return None;
    }
    if let Ok(u) = url::Url::parse(page) {
        return Some(u.to_string());
    }
    let base = if origin.ends_with('/') {
        origin.to_string()
    } else {
        format!("{origin}/")
    };
    match url::Url::parse(&base).and_then(|b| b.join(page)) {
        Ok(u) => Some(u.to_string()),
        Err(_) => Some(page.to_string()),
    }
}

fn note_body(note: Option<&str>) -> Value {
    match note.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => json!({ "note": n }),
        None => json!({}),
    }
}

/// Percent-encode a path segment.
fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

fn read_json(res: std::result::Result<ureq::Response, ureq::Error>) -> Result<Value> {
    match res {
        Ok(r) => {
            let mut s = String::new();
            r.into_reader()
                .take(64 * 1024 * 1024)
                .read_to_string(&mut s)
                .map_err(|_| CoreError::Offline)?;
            if s.trim().is_empty() {
                return Ok(Value::Null);
            }
            serde_json::from_str(&s)
                .map_err(|e| CoreError::Other(format!("bad JSON from server: {e}")))
        }
        Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
        Err(ureq::Error::Status(401, _)) => Err(CoreError::Unauthorized),
        Err(ureq::Error::Status(404, _)) => Err(CoreError::NotFound),
        Err(ureq::Error::Status(code, r)) => {
            let body: Value = r
                .into_string()
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(Value::Null);
            let message = body
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("server returned {code}"));
            let details = body
                .get("errors")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(detail).collect())
                .unwrap_or_default();
            Err(CoreError::Rejected {
                status: code,
                message,
                details,
            })
        }
    }
}

/// `errors` entries are strings, `{directive, reason}` (transclusions) or TK issue objects.
fn detail(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => {
            let d = o.get("directive").and_then(Value::as_str);
            let r = o
                .get("reason")
                .or_else(|| o.get("message"))
                .and_then(Value::as_str);
            match (d, r) {
                (Some(d), Some(r)) => format!("{d}: {r}"),
                (None, Some(r)) => r.to_string(),
                _ => v.to_string(),
            }
        }
        other => other.to_string(),
    }
}

/// True for failures worth retrying later (network down, server 5xx).
pub fn is_transient(e: &CoreError) -> bool {
    matches!(e, CoreError::Offline)
        || matches!(e, CoreError::Rejected { status, .. } if *status >= 500)
}
