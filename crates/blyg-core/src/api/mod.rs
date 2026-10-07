//! Blocking owner-API client (ureq + rustls). Follows upstream
//! blygger-studio's OpenAPI contract (studio 0.10, `openapi.json`), plus the
//! extensions in docs/SERVER.md. Every call carries the owner credential (a
//! bearer token, or the studio session from a password sign-in; see
//! `auth`), which is never logged, printed or included in errors (`Debug`
//! redacts it). `/api` is host-rooted: a blyg mounted at `https://host/blyg`
//! has its API at `https://host/api`.
//!
//! Error mapping: transport failures → `Offline`, 401 → `Unauthorized`,
//! 404 → `NotFound`, any other non-2xx → `Rejected{status, error, details}`
//! (`details` from the contract's `errors` and `issues`). The read endpoints
//! (`reading`, `mentions`, `settings`, `hoppers`) return `Ok(None)` on 404: a
//! server older than studio 0.9 without the owner-read extensions. The
//! read-state writes (extension 5) are only called once `GET /api/reading/imported`
//! advertised `read_state: true`.

pub mod auth;
pub mod media;
pub mod oauth;
pub mod public;
pub mod wire;

use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::backend::{CoreError, Result};
use crate::model::{
    Hopper, Kind, Mention, RemoteRef, ResponsesMode, ScopeProvenance, Settings, SubscribePreview,
    Subscription, SubscriptionKind,
};
use auth::Credential;
use wire::*;

pub struct Api {
    agent: ureq::Agent,
    /// The blyg URL, with its mount (public pages, the studio).
    base: String,
    /// Where `/api` lives: the blyg URL's origin.
    api_root: String,
    cred: Credential,
    /// With a password: the studio session cookie's value, once signed in.
    session: Mutex<Option<String>>,
    /// What items resolve against, from settings as last read or written.
    site: Mutex<Option<Site>>,
}

/// The settings an item's derived fields depend on.
#[derive(Debug, Clone, Default)]
struct Site {
    /// `show_responses_default`: what `responses: "default"` means.
    responses_default: bool,
    /// `site_url` without its trailing `/`; `""` = unset.
    site_url: String,
    /// `highlight_generated_default`: what `highlight: "default"` means.
    highlight_default: bool,
    /// `picker_typing`, when the server reports it (studio 0.29).
    picker_typing: Option<crate::model::PickerTyping>,
}

/// Collection page size (the contract's maximum outside `/api/reading`).
const PAGE: u32 = 100;

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("base", &self.base)
            .field("cred", &self.cred)
            .finish()
    }
}

/// Result of `PUT /api/items/:id/versions/:version/pin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pinned {
    pub already: bool,
}

/// A request body.
enum Body {
    None,
    Json(Value),
    Bytes { content_type: String, data: Vec<u8> },
}

impl Api {
    pub fn new(base_url: &str, cred: impl Into<Credential>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30))
            .timeout_write(Duration::from_secs(60))
            .build();
        let base = base_url.trim_end_matches('/').to_string();
        Api {
            agent,
            api_root: api_root(&base),
            base,
            cred: cred.into(),
            session: Mutex::new(None),
            site: Mutex::new(None),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// The studio session, signing in when there's none yet.
    fn session(&self, password: &str) -> Result<String> {
        if let Some(s) = self.session.lock().unwrap().clone() {
            return Ok(s);
        }
        let s = auth::login(&self.base, password)?;
        *self.session.lock().unwrap() = Some(s.clone());
        Ok(s)
    }

    /// A request with the credential on it, and the browser sign-in's
    /// access token it carries (for the 401 retry).
    fn request(&self, method: &str, path: &str) -> Result<(ureq::Request, Option<String>)> {
        let r = self
            .agent
            .request(method, &format!("{}{}", self.api_root, path))
            .set("accept", "application/json");
        Ok(match &self.cred {
            Credential::Token(t) => (r.set("authorization", &format!("Bearer {t}")), None),
            Credential::Password(p) => (
                r.set(
                    "cookie",
                    &format!("{}={}", auth::SESSION_COOKIE, self.session(p)?),
                ),
                None,
            ),
            Credential::OAuth(s) => {
                let t = s.access_token()?;
                (r.set("authorization", &format!("Bearer {t}")), Some(t))
            }
        })
    }

    fn send(&self, method: &str, path: &str, body: &Body) -> (Result<Value>, Option<String>) {
        let (req, bearer) = match self.request(method, path) {
            Ok(r) => r,
            Err(e) => return (Err(e), None),
        };
        let res = match body {
            Body::None => req.call(),
            Body::Json(v) => req.send_json(v.clone()),
            Body::Bytes { content_type, data } => {
                req.set("content-type", content_type).send_bytes(data)
            }
        };
        // A bearer without the scope this call needs (studio 0.28+): say
        // which permission is missing (it's in the challenge).
        if let Err(ureq::Error::Status(403, r)) = &res
            && let Some(needed) = r
                .header("www-authenticate")
                .and_then(oauth::insufficient_scope)
        {
            let granted = match &self.cred {
                Credential::OAuth(s) => Some(s.grant().scope),
                _ => None,
            };
            let message = oauth::missing_scope_message(&needed, granted.as_deref());
            return (
                Err(CoreError::Rejected {
                    status: 403,
                    message,
                    details: vec![],
                }),
                bearer,
            );
        }
        (read_json(res), bearer)
    }

    /// One call. With a password, a 401 means the session ended (30 days,
    /// or the server's secret changed): sign in again and retry once. With
    /// a browser sign-in, renew the access token and retry once; when the
    /// renewal is refused the grant is marked ended (sign in again).
    fn exec(&self, method: &str, path: &str, body: Body) -> Result<Value> {
        let (r, bearer) = self.send(method, path, &body);
        match (&self.cred, r) {
            (Credential::Password(_), Err(CoreError::Unauthorized)) => {
                *self.session.lock().unwrap() = None;
                self.send(method, path, &body).0
            }
            (Credential::OAuth(s), Err(CoreError::Unauthorized)) if bearer.is_some() => {
                s.after_unauthorized(bearer.as_deref())?;
                self.send(method, path, &body).0
            }
            (_, r) => r,
        }
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        self.exec(method, path, body.map_or(Body::None, Body::Json))
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
    pub fn optional_of<T>(r: Result<T>) -> Result<Option<T>> {
        Self::optional(r)
    }

    /// 404 → `None` (endpoint not deployed yet).
    fn optional<T>(r: Result<T>) -> Result<Option<T>> {
        match r {
            Ok(v) => Ok(Some(v)),
            Err(CoreError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every element of a `{items, total, offset, limit}` collection, read
    /// `PAGE` at a time. `query` is extra query parameters (`k=v&…`).
    fn collect<T: DeserializeOwned>(&self, path: &str, query: &str) -> Result<Vec<T>> {
        let mut all = Vec::new();
        loop {
            let sep = if query.is_empty() { "" } else { "&" };
            let url = format!("{path}?{query}{sep}offset={}&limit={PAGE}", all.len());
            let page: Page<T> = self.call_as("GET", &url, None)?;
            let total = page.total.ok_or(CoreError::ServerOutdated)?;
            let n = page.items.len();
            all.extend(page.items);
            if n == 0 || all.len() as u64 >= total {
                return Ok(all);
            }
        }
    }

    /// The cached site settings, reading them when unknown.
    fn site(&self) -> Site {
        if let Some(s) = self.site.lock().unwrap().clone() {
            return s;
        }
        let _ = self.settings();
        self.site.lock().unwrap().clone().unwrap_or_default()
    }

    /// Where the blyg's public pages live, without a trailing `/`: its
    /// `site_url` when set, else the API base (which carries the mount).
    /// The Worker builds its own origin the same way.
    pub fn public_base(&self) -> String {
        let s = self.site();
        if s.site_url.is_empty() {
            self.base.clone()
        } else {
            s.site_url
        }
    }

    fn resolve(&self, mut w: WireItem) -> WireItem {
        let site = self.site();
        let base = if site.site_url.is_empty() {
            &self.base
        } else {
            &site.site_url
        };
        w.resolve(site.responses_default, base);
        w.resolve_highlight(site.highlight_default);
        w
    }

    // ---------- items ----------

    /// Every item. Re-reads settings first, so `responses: "default"`
    /// resolves against the current site default.
    pub fn list_items(&self) -> Result<Vec<WireItem>> {
        self.list_items_with(true)
    }

    /// Every item; `refresh_settings: false` resolves against the settings
    /// as last read (when the blyg says they haven't changed).
    pub fn list_items_with(&self, refresh_settings: bool) -> Result<Vec<WireItem>> {
        let items: Vec<WireItem> = self.collect("/api/items", "")?;
        if refresh_settings || self.site.lock().unwrap().is_none() {
            let _ = self.settings();
        }
        Ok(items.into_iter().map(|w| self.resolve(w)).collect())
    }

    /// How many items the server holds (one small read). Also the version
    /// probe: `ServerOutdated` from a server older than studio 0.9.
    pub fn count_items(&self) -> Result<u64> {
        self.call_as::<Page<Value>>("GET", "/api/items?offset=0&limit=1", None)?
            .total
            .ok_or(CoreError::ServerOutdated)
    }

    pub fn get_item(&self, id: &str) -> Result<WireItem> {
        let w = self.call_as("GET", &format!("/api/items/{}", enc(id)), None)?;
        Ok(self.resolve(w))
    }

    /// `POST /api/items {mode: "blank", kind, content_md, stub_of?,
    /// provenance?}` → the new item's id. `provenance` (studio 0.28+) has one
    /// entry per TK scope; an older server refuses the key with a 400.
    pub fn create_item(
        &self,
        content_md: &str,
        kind: Kind,
        stub_of: Option<&RemoteRef>,
        provenance: Option<&[Option<ScopeProvenance>]>,
    ) -> Result<String> {
        let mut body = json!({ "mode": "blank", "content_md": content_md, "kind": kind_str(kind) });
        if let Some(s) = stub_of {
            body["stub_of"] = serde_json::to_value(s).unwrap_or(Value::Null);
        }
        if let Some(p) = provenance {
            body["provenance"] = provenance_body(p);
        }
        Ok(self
            .call_as::<Created>("POST", "/api/items", Some(body))?
            .id)
    }

    /// `PATCH /api/items/:id {content_md}`.
    pub fn save_item(&self, id: &str, content_md: &str) -> Result<()> {
        self.call(
            "PATCH",
            &format!("/api/items/{}", enc(id)),
            Some(json!({ "content_md": content_md })),
        )
        .map(|_| ())
    }

    /// `PATCH /api/items/:id {content_md, provenance}` (studio 0.28+): the
    /// text and its whole per-scope provenance in one atomic write. An older
    /// server refuses the unknown key with a 400 ([`refused_provenance_key`]).
    pub fn save_item_provenance(
        &self,
        id: &str,
        content_md: &str,
        provenance: &[Option<ScopeProvenance>],
    ) -> Result<()> {
        self.call(
            "PATCH",
            &format!("/api/items/{}", enc(id)),
            Some(json!({ "content_md": content_md, "provenance": provenance_body(provenance) })),
        )
        .map(|_| ())
    }

    /// `PATCH /api/items/:id {content_md, kind}`: a draft's kind can change
    /// until it is first published (409 after).
    pub fn save_item_kind(&self, id: &str, content_md: &str, kind: Kind) -> Result<()> {
        self.call(
            "PATCH",
            &format!("/api/items/{}", enc(id)),
            Some(json!({ "content_md": content_md, "kind": kind_str(kind) })),
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

    /// `PUT /api/items/:id/versions/:version/pin` (idempotent, no body).
    pub fn pin(&self, id: &str, version: u32) -> Result<Pinned> {
        #[derive(serde::Deserialize)]
        struct R {
            #[serde(default)]
            already: bool,
        }
        let r: R = self.call_as(
            "PUT",
            &format!("/api/items/{}/versions/{version}/pin", enc(id)),
            None,
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

    /// `POST /api/items/:id/generate {scope}`. The server fills that TK
    /// scope with its own model and records the provenance.
    pub fn generate(&self, id: &str, scope: u32) -> Result<Generated> {
        #[derive(serde::Deserialize)]
        struct R {
            text: String,
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            content_md: Option<String>,
        }
        let r: R = self.call_as(
            "POST",
            &format!("/api/items/{}/generate", enc(id)),
            Some(json!({ "scope": scope })),
        )?;
        Ok(Generated {
            text: r.text,
            model: r.model.unwrap_or_else(|| "unknown".into()),
            content_md: r.content_md,
        })
    }

    /// `GET /api/changes` (studio 0.32+): the blyg's change epoch and one
    /// revision counter per data domain. `None` on an older server (404).
    pub fn changes(&self) -> Result<Option<ChangeState>> {
        Self::optional(self.call_as("GET", "/api/changes", None))
    }

    /// `POST /api/items {mode: "fork", source}` → the new draft.
    pub fn fork(&self, of: &RemoteRef) -> Result<WireItem> {
        let body = json!({
            "mode": "fork",
            "source": { "origin": of.origin, "id": of.id, "version": of.version },
        });
        let w = self.call_as("POST", "/api/items", Some(body))?;
        Ok(self.resolve(w))
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

    /// `PATCH /api/items/:id {responses}`. Returns (showing now, the
    /// item's policy as the server stored it).
    pub fn set_responses(
        &self,
        id: &str,
        mode: ResponsesMode,
    ) -> Result<(bool, Option<ResponsesMode>)> {
        let w: WireItem = self.call_as(
            "PATCH",
            &format!("/api/items/{}", enc(id)),
            Some(json!({ "responses": mode.as_str() })),
        )?;
        let w = self.resolve(w);
        Ok((w.shows_responses(), w.responses_mode()))
    }

    /// `PATCH /api/items/:id {highlight}` (studio 0.27; needs
    /// `owner:publish`, as it changes the public page at once). Returns
    /// (highlighting now, the item's choice as the server stored it).
    pub fn set_highlight(
        &self,
        id: &str,
        mode: crate::model::HighlightMode,
    ) -> Result<(bool, Option<crate::model::HighlightMode>)> {
        let w: WireItem = self.call_as(
            "PATCH",
            &format!("/api/items/{}", enc(id)),
            Some(json!({ "highlight": mode.as_str() })),
        )?;
        let w = self.resolve(w);
        Ok((w.highlights(), w.highlight))
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
        let v = self.exec(
            "POST",
            "/api/media",
            Body::Bytes {
                content_type: format!("multipart/form-data; boundary={boundary}"),
                data: body,
            },
        )?;
        serde_json::from_value(v)
            .map_err(|e| CoreError::Other(format!("unexpected media response: {e}")))
    }

    /// `DELETE /api/media/:id` (extension): removes the row and the stored
    /// file. 404 unknown; 409 when it's the site avatar.
    pub fn delete_media(&self, id: &str) -> Result<()> {
        self.call("DELETE", &format!("/api/media/{}", enc(id)), None)
            .map(|_| ())
    }

    // ---------- subscriptions ----------

    pub fn list_subscriptions(&self) -> Result<Vec<Subscription>> {
        self.collect("/api/subscriptions", "")
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
            // A blyg that claims another site answers `{asserted, actual}`;
            // no key means it matched. Feeds aren't checked.
            site_mismatch: (kind == SubscriptionKind::Blyg).then(|| {
                v.get("siteMismatch")
                    .is_some_and(|m| m.as_bool().unwrap_or(m.is_object()))
            }),
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

    /// `PATCH /api/subscriptions/:id`. `title`: `None` leaves the name alone,
    /// `Some(Some(t))` sets a name of your own, `Some(None)` sends
    /// `title: null`, handing the name back to the source (studio 0.30),
    /// which refreshes it while polling.
    pub fn update_subscription(
        &self,
        id: &str,
        in_blogroll: Option<bool>,
        title: Option<Option<&str>>,
    ) -> Result<()> {
        let mut body = json!({});
        if let Some(b) = in_blogroll {
            body["in_blogroll"] = json!(b);
        }
        if let Some(t) = title {
            body["title"] = t.map_or(Value::Null, |t| json!(t));
        }
        self.call(
            "PATCH",
            &format!("/api/subscriptions/{}", enc(id)),
            Some(body),
        )
        .map(|_| ())
    }

    pub fn delete_subscription(&self, id: &str) -> Result<()> {
        self.call("DELETE", &format!("/api/subscriptions/{}", enc(id)), None)
            .map(|_| ())
    }

    /// `PATCH /api/subscriptions/:id {paused}`.
    pub fn pause_subscription(&self, id: &str, paused: bool) -> Result<()> {
        self.call(
            "PATCH",
            &format!("/api/subscriptions/{}", enc(id)),
            Some(json!({ "paused": paused })),
        )
        .map(|_| ())
    }

    /// `POST /api/subscriptions/poll` (studio 0.30): the server polls every
    /// subscription that isn't paused, in the background, and answers at
    /// once with how many.
    pub fn poll_subscriptions(&self) -> Result<u32> {
        let v = self.call("POST", "/api/subscriptions/poll", None)?;
        Ok(v.get("polling").and_then(Value::as_u64).unwrap_or(0) as u32)
    }

    /// `{ok, changed}` (`changed` counts items); blyg subscriptions only
    /// (409 otherwise). True when anything changed.
    pub fn resync_subscription(&self, id: &str) -> Result<bool> {
        let v = self.call(
            "POST",
            &format!("/api/subscriptions/{}/resync", enc(id)),
            None,
        )?;
        let changed = v.get("changed");
        Ok(changed.and_then(Value::as_u64).is_some_and(|n| n > 0)
            || changed.and_then(Value::as_bool).unwrap_or(false))
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
            "PATCH",
            &format!("/api/mentions/{}", enc(id)),
            Some(json!({ "hidden": hidden })),
        )
        .map(|_| ())
    }

    // ---------- settings ----------

    /// `PATCH /api/settings`: sends only the fields that are set.
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
        // `Some("")` clears it (the blyg then renders in UTC).
        put("timezone", &s.timezone);
        if let Some(on) = s.accept_mentions {
            body.insert("accept_mentions".into(), json!(on));
        }
        if let Some(on) = s.show_responses_default {
            body.insert("show_responses_default".into(), json!(on));
        }
        if let Some(on) = s.highlight_generated_default {
            body.insert("highlight_generated_default".into(), json!(on));
        }
        if let Some(t) = s.picker_typing {
            body.insert("picker_typing".into(), json!(t.as_str()));
        }
        body.insert(
            "author_links".into(),
            serde_json::to_value(&s.author_links).unwrap_or(json!([])),
        );
        let v = self.call("PATCH", "/api/settings", Some(Value::Object(body)))?;
        // The reply is the stored settings.
        match serde_json::from_value::<Settings>(v) {
            Ok(saved) => self.remember(Some(&saved)),
            Err(_) => *self.site.lock().unwrap() = None,
        }
        Ok(())
    }

    /// Cache what items resolve against. No settings (or no such field)
    /// means no site default (hidden) and no `site_url`.
    fn remember(&self, s: Option<&Settings>) {
        *self.site.lock().unwrap() = Some(Site {
            responses_default: s.and_then(|s| s.show_responses_default).unwrap_or(false),
            site_url: s
                .and_then(|s| s.site_url.as_deref())
                .map(|u| u.trim().trim_end_matches('/').to_string())
                .unwrap_or_default(),
            highlight_default: s
                .and_then(|s| s.highlight_generated_default)
                .unwrap_or(false),
            picker_typing: s.and_then(|s| s.picker_typing),
        });
    }

    /// `picker_typing` as last read or written (`None` until settings were
    /// read, or on a server before 0.29).
    pub fn picker_typing(&self) -> Option<crate::model::PickerTyping> {
        self.site
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.picker_typing)
    }

    // ---------- reads (404 → None) ----------

    /// `GET /api/reading/imported` (extension 3): imported rows in the app's
    /// shape, keyset-paged. Upstream's own `/api/reading` is a different,
    /// rendered resource. `content_html` comes back raw, as stored: the UI
    /// sanitizes it before display.
    pub fn reading(&self, limit: u32, before: Option<&str>) -> Result<Option<ReadingPage>> {
        let mut path = format!("/api/reading/imported?limit={limit}");
        if let Some(b) = before {
            path.push_str("&before=");
            path.push_str(&enc(b));
        }
        let page: Option<ReadingPage> = Self::optional(self.call_as("GET", &path, None))?;
        Ok(page.map(|mut p| {
            let read_sync = p.read_sync();
            let lineage = p.lineage == Some(true);
            for it in &mut p.items {
                it.page = it.page.take().and_then(|pg| absolute_page(&it.origin, &pg));
                it.lineage_known = lineage;
                if !read_sync {
                    // Only a server that advertises read state means it.
                    it.read_version = None;
                }
            }
            p
        }))
    }

    // ---------- reading on a stock server (no `/reading/imported`) ----------

    /// Upstream's `GET /api/reading`, one page (at most 50). `None` on 404.
    pub fn stock_reading(&self, offset: u64) -> Result<Option<StockReadingPage>> {
        Self::optional(self.call_as(
            "GET",
            &format!("/api/reading?offset={offset}&limit={STOCK_READING_PAGE}"),
            None,
        ))
    }

    /// `GET /api/imports/{sub}/{id}`: one imported item, raw.
    pub fn imported(&self, sub: &str, remote_id: &str) -> Result<Value> {
        self.call(
            "GET",
            &format!("/api/imports/{}/{}", enc(sub), enc(remote_id)),
            None,
        )
    }

    /// Every thumb: `GET /api/signals` → (sub, remote id) → 1 or -1.
    pub fn signals(&self) -> Result<std::collections::HashMap<(String, String), i8>> {
        #[derive(serde::Deserialize)]
        struct S {
            subscription_id: String,
            remote_id: String,
            thumb: i8,
        }
        let rows: Vec<S> = self.collect("/api/signals", "")?;
        Ok(rows
            .into_iter()
            .map(|s| ((s.subscription_id, s.remote_id), s.thumb))
            .collect())
    }

    /// Which hoppers hold each imported item: (sub, remote id) → hopper ids,
    /// from every hopper's memberships.
    pub fn hopper_members(
        &self,
    ) -> Result<std::collections::HashMap<(String, String), Vec<String>>> {
        #[derive(serde::Deserialize)]
        struct H {
            id: String,
        }
        #[derive(serde::Deserialize)]
        struct M {
            subscription_id: String,
            remote_id: String,
        }
        #[derive(serde::Deserialize)]
        struct Detail {
            #[serde(default)]
            memberships: Vec<M>,
        }
        let mut out: std::collections::HashMap<(String, String), Vec<String>> = Default::default();
        for h in self.collect::<H>("/api/hoppers", "")? {
            let d: Detail = self.call_as("GET", &format!("/api/hoppers/{}", enc(&h.id)), None)?;
            for m in d.memberships {
                out.entry((m.subscription_id, m.remote_id))
                    .or_default()
                    .push(h.id.clone());
            }
        }
        Ok(out)
    }

    // ---------- extension 5: read state (404 = not deployed) ----------

    /// `PUT /api/reading/:sub/:remoteId/read {version, read_at?}`. The server
    /// keeps `max(stored, version)`, so a replay is harmless. `read_at` only
    /// for a server that advertises `read_state_clear`: a read older than
    /// its latest "mark unread" is then ignored (`stored: false`).
    pub fn put_read(
        &self,
        sub: &str,
        remote_id: &str,
        version: u32,
        read_at: Option<&str>,
    ) -> Result<ReadAck> {
        let mut body = json!({ "version": version });
        if let Some(at) = read_at {
            body["read_at"] = json!(at);
        }
        let v = self.call(
            "PUT",
            &format!("/api/reading/{}/{}/read", enc(sub), enc(remote_id)),
            Some(body),
        )?;
        Ok(serde_json::from_value(v).unwrap_or(ReadAck {
            stored: true,
            read_version: None,
        }))
    }

    /// `DELETE /api/reading/:sub/:remoteId/read`: mark one row unread
    /// (`read_state_clear`). Idempotent; 200 for a row the server doesn't hold.
    pub fn delete_read(&self, sub: &str, remote_id: &str) -> Result<()> {
        self.call(
            "DELETE",
            &format!("/api/reading/{}/{}/read", enc(sub), enc(remote_id)),
            None,
        )
        .map(|_| ())
    }

    /// `POST /api/reading/unread {items: [{sub, remote_id}]}`, at most
    /// `READ_BATCH_MAX` entries, all or nothing.
    pub fn post_unreads(&self, items: &[UnreadMark]) -> Result<()> {
        self.call(
            "POST",
            "/api/reading/unread",
            Some(json!({ "items": items })),
        )
        .map(|_| ())
    }

    /// `POST /api/reading/read {items: [{sub, remote_id, version}]}`, at most
    /// `READ_BATCH_MAX` entries (the caller chunks).
    pub fn put_reads(&self, items: &[ReadMark]) -> Result<()> {
        self.call("POST", "/api/reading/read", Some(json!({ "items": items })))
            .map(|_| ())
    }

    /// Inbound mentions (responses to this blyg's posts).
    pub fn mentions(&self) -> Result<Option<Vec<Mention>>> {
        let m: Option<Vec<WireMention>> =
            Self::optional(self.collect("/api/mentions", "direction=inbound"))?;
        Ok(m.map(|m| m.into_iter().map(Mention::from).collect()))
    }

    /// The server fills unset strings with upstream defaults (`""`, or
    /// `"auto"`); `""` becomes `None`. Except `timezone`, where `Some("")`
    /// means "the server has the setting, and it is unset".
    pub fn settings(&self) -> Result<Option<Settings>> {
        let s: Option<Settings> = Self::optional(self.call_as("GET", "/api/settings", None))?;
        self.remember(s.as_ref());
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

    /// Every hopper, with its item count from the hopper's own preview read
    /// (`GET /api/hoppers/:id?preview=true` → `total`).
    pub fn hoppers(&self) -> Result<Option<Vec<Hopper>>> {
        #[derive(serde::Deserialize)]
        struct H {
            id: String,
            name: String,
            #[serde(default)]
            slug: Option<String>,
            #[serde(default)]
            public: bool,
        }
        #[derive(serde::Deserialize)]
        struct Detail {
            #[serde(default)]
            total: u32,
        }
        let Some(list) = Self::optional(self.collect::<H>("/api/hoppers", ""))? else {
            return Ok(None);
        };
        let mut out = Vec::with_capacity(list.len());
        for h in list {
            let count = self
                .call_as::<Detail>(
                    "GET",
                    &format!("/api/hoppers/{}?preview=true", enc(&h.id)),
                    None,
                )?
                .total;
            out.push(Hopper {
                id: h.id,
                name: h.name,
                slug: h.slug,
                public: h.public,
                count,
            });
        }
        Ok(Some(out))
    }
}

/// Why a connection check failed, worded for the "Connect your blyg" sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// Nothing answered at that address (DNS, refused, TLS, timeout).
    Unreachable,
    /// The server said 401: the token is wrong.
    WrongToken,
    /// The studio's sign-in refused the password.
    WrongPassword,
    /// No studio sign-in at `{blyg-url}/studio/login` (404).
    NoStudio,
    /// `GET /api/items` is 404: a blyg older than studio 0.9 without the
    /// owner-read extensions.
    MissingExtensions,
    /// The server answers, but it's older than blygger-studio 0.9.
    Outdated,
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
                "The blyg said the token is wrong or has expired (401). Make a new one in Studio → More → Client access and paste it.",
            ),
            ConnectError::WrongPassword => f.write_str(
                "The blyg said the password is wrong. Type the studio password again.",
            ),
            ConnectError::NoStudio => f.write_str(
                "There's no blyg studio sign-in at that address (…/studio/login is 404). Check the URL, including any path the blyg lives under.",
            ),
            ConnectError::MissingExtensions => f.write_str(
                "This server has no owner JSON API (GET /api/items is 404). It needs blygger-studio 0.9 or later. See docs/SERVER.md.",
            ),
            ConnectError::Outdated => f.write_str(
                "This blyg's server is older than blygger-studio 0.9. Update it, then connect again.",
            ),
            ConnectError::Other(m) => f.write_str(m),
        }
    }
}

/// Check a blyg URL and credential before saving them: sign in (with a
/// password), then `GET /api/items`. Returns how many items the server holds.
pub fn verify_connection(
    base_url: &str,
    cred: impl Into<Credential>,
) -> std::result::Result<usize, ConnectError> {
    let cred = cred.into();
    let password = matches!(cred, Credential::Password(_));
    if let Credential::Password(p) = &cred {
        match auth::login(base_url, p) {
            Ok(_) => {}
            Err(CoreError::Offline) => return Err(ConnectError::Unreachable),
            Err(CoreError::Unauthorized) => return Err(ConnectError::WrongPassword),
            Err(CoreError::NotFound) => return Err(ConnectError::NoStudio),
            Err(e) => return Err(ConnectError::Other(e.to_string())),
        }
    }
    let api = Api::new(base_url, cred);
    match api.count_items() {
        Ok(n) => Ok(n as usize),
        Err(CoreError::Offline) => Err(ConnectError::Unreachable),
        Err(CoreError::Unauthorized) if password => Err(ConnectError::WrongPassword),
        Err(CoreError::Unauthorized) => Err(ConnectError::WrongToken),
        Err(CoreError::NotFound) => Err(ConnectError::MissingExtensions),
        Err(CoreError::ServerOutdated) => Err(ConnectError::Outdated),
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

/// `/api` is host-rooted (upstream `app.route("/api", …)`): the origin of
/// the blyg URL, whatever its mount.
fn api_root(base: &str) -> String {
    match url::Url::parse(base) {
        Ok(u) if u.has_host() => u.origin().ascii_serialization(),
        _ => base.to_string(),
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
        Err(ureq::Error::Status(429, r)) => Err(CoreError::RateLimited {
            retry_after: retry_after(r.header("retry-after")),
        }),
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
            let mut details: Vec<String> = body
                .get("errors")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(detail).collect())
                .unwrap_or_default();
            // Validation failures: `issues: [{path, message}]`.
            if let Some(issues) = body.get("issues").and_then(Value::as_array) {
                details.extend(issues.iter().map(issue));
            }
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

/// One validation issue: `path.to.field: message`.
fn issue(v: &Value) -> String {
    let msg = v
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("invalid");
    let path: Vec<String> = v
        .get("path")
        .and_then(Value::as_array)
        .map(|p| {
            p.iter()
                .map(|s| s.as_str().map_or_else(|| s.to_string(), str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if path.is_empty() {
        msg.to_string()
    } else {
        format!("{}: {msg}", path.join("."))
    }
}

/// When to try again after a 429, in seconds: `Retry-After` as a number of
/// seconds (studio sends `60`); a minute when it's missing or unreadable (an
/// HTTP date included). Clamped to 1 s – 1 h.
pub(crate) fn retry_after(h: Option<&str>) -> u64 {
    h.and_then(|h| h.trim().parse::<u64>().ok())
        .unwrap_or(60)
        .clamp(1, 3600)
}

/// True for failures worth retrying later (network down, server 5xx, or the
/// blyg's work budget spent: a 429 never drops a queued change).
pub fn is_transient(e: &CoreError) -> bool {
    matches!(e, CoreError::Offline | CoreError::RateLimited { .. })
        || matches!(e, CoreError::Rejected { status, .. } if *status >= 500)
}

/// Upstream's `GenerationProvenance` array: `{sources: [{id, version}],
/// model?, at?}` or `null` per scope.
fn provenance_body(scopes: &[Option<ScopeProvenance>]) -> Value {
    Value::Array(
        scopes
            .iter()
            .map(|p| match p {
                None => Value::Null,
                Some(p) => {
                    let mut e = json!({ "model": p.model, "sources": p.sources });
                    if let Some(at) = &p.at {
                        e["at"] = json!(at);
                    }
                    e
                }
            })
            .collect(),
    )
}

/// A 400 that refuses the `provenance` key itself: a server older than
/// studio 0.28, whose strict item schema doesn't know it.
pub fn refused_provenance_key(e: &CoreError) -> bool {
    match e {
        CoreError::Rejected {
            status: 400,
            message,
            details,
        } => std::iter::once(message)
            .chain(details)
            .any(|m| m.contains("nrecognized") && m.contains("provenance")),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_reads_seconds_and_defaults_to_a_minute() {
        assert_eq!(retry_after(Some("60")), 60);
        assert_eq!(retry_after(Some(" 5 ")), 5);
        assert_eq!(retry_after(Some("0")), 1);
        assert_eq!(retry_after(Some("999999")), 3600);
        assert_eq!(retry_after(None), 60);
        assert_eq!(retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")), 60);
    }

    #[test]
    fn a_rate_limit_is_transient() {
        assert!(is_transient(&CoreError::RateLimited { retry_after: 60 }));
    }

    #[test]
    fn an_unknown_provenance_key_reads_as_an_older_server() {
        let strict = CoreError::Rejected {
            status: 400,
            message: "request: Unrecognized key(s) in object: 'provenance'".into(),
            details: vec![],
        };
        assert!(refused_provenance_key(&strict));
        let invalid = CoreError::Rejected {
            status: 400,
            message: "provenance must have one entry per TK scope".into(),
            details: vec![],
        };
        assert!(!refused_provenance_key(&invalid));
    }
}
