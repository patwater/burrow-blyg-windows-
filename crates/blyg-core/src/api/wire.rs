//! Wire shapes the owner API returns that don't map 1:1 onto `model` types.
//! Field names follow upstream blygger-studio's OpenAPI contract
//! (`openapi.json`, studio 0.10), plus the extensions in docs/SERVER.md.

use serde::Deserialize;
use serde_json::Value;

use crate::model::{Author, Kind, Mention, ReadingItem, RemoteRef, ResponsesMode, Status, Version};

/// `GET /api/items` element and `GET /api/items/:id` body (the contract's
/// `Item`, plus `authored_kind` and `versions` on the detail read).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WireItem {
    pub id: String,
    /// Current kind; `"withdrawn"` for withdrawn items, so prefer `authored_kind`.
    pub kind: String,
    #[serde(default)]
    pub authored_kind: Option<String>,
    pub status: String,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub content_md: String,
    #[serde(default)]
    pub stub_of: Option<Value>,
    #[serde(default)]
    pub forked_from: Option<Value>,
    /// The item's response policy. `None` only on a hand-built placeholder.
    #[serde(default)]
    pub responses: Option<ResponsesMode>,
    /// Only on `GET /api/items/:id`.
    #[serde(default, deserialize_with = "wire_versions")]
    pub versions: Option<Vec<Version>>,
    /// Not on the wire: the client derives it (`{base}/f|t/{id}`) for
    /// published items. `None` keeps whatever the store already holds.
    #[serde(skip)]
    pub permalink: Option<String>,
    /// Not on the wire: whether the page shows responses now, resolved from
    /// `responses` and the blyg's `show_responses_default`.
    #[serde(skip)]
    pub showing: Option<bool>,
    /// The working copy's TK provenance, one entry per scope (studio 0.28+;
    /// `None` from an older server). Raw: upstream's `model` is optional.
    #[serde(default)]
    pub provenance: Option<Vec<Option<Value>>>,
    /// The item's generated-text highlight choice (studio 0.27).
    #[serde(default)]
    pub highlight: Option<ResponsesMode>,
    /// Not on the wire: whether the public page highlights generated text
    /// now, resolved from `highlight` and `highlight_generated_default`.
    #[serde(skip)]
    pub highlighting: Option<bool>,
}

impl WireItem {
    /// `provenance` as the app's shape (`None` from a server without the
    /// field). An entry without a model reads as `"unknown"`; one that
    /// doesn't parse as `null`.
    pub fn server_provenance(&self) -> Option<Vec<Option<crate::model::ScopeProvenance>>> {
        self.provenance.as_ref().map(|all| {
            all.iter()
                .map(|p| {
                    let mut p = p.clone()?;
                    let o = p.as_object_mut()?;
                    if !o.get("model").is_some_and(Value::is_string) {
                        o.insert("model".into(), Value::String("unknown".into()));
                    }
                    serde_json::from_value(p).ok()
                })
                .collect()
        })
    }

    /// Whether the page shows responses now.
    pub fn shows_responses(&self) -> bool {
        self.showing
            .unwrap_or(self.responses == Some(ResponsesMode::Show))
    }

    /// The item's own choice, when the server reports one.
    pub fn responses_mode(&self) -> Option<ResponsesMode> {
        self.responses
    }

    /// Fill the derived fields: `default` is the blyg's
    /// `show_responses_default`, `base` the API base (it carries the mount).
    pub fn resolve(&mut self, default: bool, base: &str) {
        self.showing = Some(match self.responses {
            Some(ResponsesMode::Show) => true,
            Some(ResponsesMode::Hide) => false,
            Some(ResponsesMode::Default) | None => default,
        });
        if self.version > 0 {
            let p = if self.local_kind() == Kind::Thread {
                "t"
            } else {
                "f"
            };
            self.permalink = Some(format!("{base}/{p}/{}", self.id));
        }
    }

    /// Whether the public page highlights generated text now.
    pub fn highlights(&self) -> bool {
        self.highlighting
            .unwrap_or(self.highlight == Some(ResponsesMode::Show))
    }

    /// Fill `highlighting`: `default` is `highlight_generated_default`.
    pub fn resolve_highlight(&mut self, default: bool) {
        self.highlighting = Some(match self.highlight {
            Some(ResponsesMode::Show) => true,
            Some(ResponsesMode::Hide) => false,
            Some(ResponsesMode::Default) | None => default,
        });
    }

    pub fn local_kind(&self) -> Kind {
        let k = self.authored_kind.as_deref().unwrap_or(&self.kind);
        parse_kind(k)
    }

    pub fn local_status(&self) -> Status {
        parse_status(&self.status)
    }

    /// `stub_of` may also be `{url}` for an L0 stub, which `RemoteRef` can't hold.
    pub fn stub_ref(&self) -> Option<RemoteRef> {
        self.stub_of
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    pub fn fork_ref(&self) -> Option<RemoteRef> {
        self.forked_from
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }
}

/// The contract's `Version`: a withdraw marker is `kind: "withdrawn"`.
#[derive(Debug, Clone, Deserialize)]
struct WireVersion {
    version: u32,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    kind: String,
}

fn wire_versions<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<Version>>, D::Error> {
    let v = Option::<Vec<WireVersion>>::deserialize(d)?;
    Ok(v.map(|v| {
        v.into_iter()
            .map(|w| Version {
                version: w.version,
                published_at: w.published_at,
                note: w.note,
                pinned: w.pinned,
                endcap: w.kind == "withdrawn",
            })
            .collect()
    }))
}

/// `GET /api/mentions?direction=inbound` element. The source's author is a
/// serialized JSON string (`source_author_json`).
#[derive(Debug, Clone, Deserialize)]
pub struct WireMention {
    pub id: String,
    pub target_item_id: String,
    pub status: String,
    #[serde(default)]
    pub relation: Option<String>,
    pub source: String,
    #[serde(default)]
    pub source_origin: Option<String>,
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub source_kind: Option<String>,
    #[serde(default)]
    pub source_version: Option<u32>,
    #[serde(default)]
    pub source_author_json: Option<String>,
    #[serde(default)]
    pub first_seen: String,
    #[serde(default)]
    pub verified_at: Option<String>,
    #[serde(default)]
    pub hidden: bool,
}

impl From<WireMention> for Mention {
    fn from(w: WireMention) -> Mention {
        // Tolerant: an author we can't read is no author.
        let source_author = w
            .source_author_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<Author>(j).ok());
        Mention {
            id: w.id,
            target_item_id: w.target_item_id,
            status: w.status,
            relation: w.relation,
            source: w.source,
            source_origin: w.source_origin,
            source_id: w.source_id,
            source_kind: w.source_kind,
            source_version: w.source_version,
            source_author,
            first_seen: w.first_seen,
            verified_at: w.verified_at,
            hidden: w.hidden,
        }
    }
}

/// Collection reads: `{items, total, offset, limit}`. A server older than
/// studio 0.9 answers `{items}` alone, so a missing `total` marks it.
#[derive(Debug, Clone, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default)]
    pub total: Option<u64>,
}

pub fn parse_kind(s: &str) -> Kind {
    if s == "thread" {
        Kind::Thread
    } else {
        Kind::Fragment
    }
}

pub fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::Fragment => "fragment",
        Kind::Thread => "thread",
    }
}

pub fn parse_status(s: &str) -> Status {
    match s {
        "public" => Status::Public,
        "withdrawn" => Status::Withdrawn,
        // Local-only (never on the wire): stored in `items.status`.
        "scratch" => Status::Scratch,
        _ => Status::Draft,
    }
}

pub fn status_str(s: Status) -> &'static str {
    match s {
        Status::Draft => "draft",
        Status::Public => "public",
        Status::Withdrawn => "withdrawn",
        Status::Scratch => "scratch",
    }
}

/// `POST /api/items/:id/generate` → `{text, model, content_md}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    /// The scope's new output.
    pub text: String,
    pub model: String,
    /// The working copy with `text` spliced into its scope, as the server
    /// stored it (studio 0.27+; `None` from an older server).
    pub content_md: Option<String>,
}

/// `GET /api/changes` (studio 0.32+): `{epoch, domains: {items, reading,
/// subscriptions, hoppers, signals, settings, feed}}`. A domain's counter
/// moves whenever its data changes; a new epoch means the database was
/// restored or replaced and no stored revision holds.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChangeState {
    pub epoch: String,
    pub domains: std::collections::BTreeMap<String, u64>,
}

/// The id of a created resource (`POST /api/items` → `201 Item`,
/// `POST /api/subscriptions {confirm}` → `201 Subscription`).
#[derive(Debug, Clone, Deserialize)]
pub struct Created {
    pub id: String,
}

/// `POST /api/items/:id/publish` → `{ok, version, warning?}`.
#[derive(Debug, Clone, Deserialize)]
pub struct Published {
    pub version: u32,
    #[serde(default)]
    pub warning: Option<String>,
}

/// `POST /api/media` → `201 {id, url, mime}`. The maintainer's Worker fork
/// also answers `200 {…, duplicate: true}` when the item already has an
/// attachment with identical bytes (the existing row comes back).
#[derive(Debug, Clone, Deserialize)]
pub struct Media {
    pub id: String,
    pub url: String,
    pub mime: String,
    #[serde(default)]
    pub duplicate: bool,
}

/// `GET /api/reading/imported` (patch 3) → `{items, next}`, plus `read_state: true`
/// from a server that stores read state (extension 5; each item then carries
/// its `read_version`), and `lineage: true` from one whose rows carry their
/// `stub_of` and `forked_from` (null when none).
#[derive(Debug, Clone, Deserialize)]
pub struct ReadingPage {
    pub items: Vec<ReadingItem>,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default, deserialize_with = "crate::model::lenient")]
    pub read_state: Option<bool>,
    #[serde(default, deserialize_with = "crate::model::lenient")]
    pub lineage: Option<bool>,
    /// Read state can be cleared (mark unread, `read_at` on reads).
    #[serde(default, deserialize_with = "crate::model::lenient")]
    pub read_state_clear: Option<bool>,
}

impl ReadingPage {
    /// The server advertises read-state sync (extension 5).
    pub fn read_sync(&self) -> bool {
        self.read_state == Some(true)
    }

    /// The server can clear read state (`read_state_clear: true`).
    pub fn read_clear(&self) -> bool {
        self.read_sync() && self.read_state_clear == Some(true)
    }
}

/// One row's read state as sent to the server: `(sub, remote_id)` is a
/// reading row, `version` the highest version read. `read_at` is when it
/// was read on this Mac (ISO-8601, UTC). It is sent only to a server that
/// advertises `read_state_clear`: elsewhere a strict body would refuse it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct ReadMark {
    pub sub: String,
    pub remote_id: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_at: Option<String>,
}

/// One row to mark unread (`POST /api/reading/unread`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, Deserialize)]
pub struct UnreadMark {
    pub sub: String,
    pub remote_id: String,
}

/// `PUT /api/reading/{sub}/{remoteId}/read` → whether the read was stored
/// (false: the server holds no such row, or the read predates a later
/// "mark unread"), and the version it holds now (null: unread).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReadAck {
    #[serde(default = "stored_default")]
    pub stored: bool,
    #[serde(default)]
    pub read_version: Option<u32>,
}

/// An older extension-5 Worker answers `{ok}` only: the read was taken.
fn stored_default() -> bool {
    true
}

/// Upstream's `GET /api/reading` (a server without the fork's
/// `/reading/imported`): the entries, own and imported, newest first.
#[derive(Debug, Clone, Deserialize)]
pub struct StockReadingPage {
    pub items: Vec<StockEntry>,
    #[serde(default)]
    pub total: u64,
    /// The server stores read state (upstream's read-state routes): each
    /// imported entry's `readVersion` is meaningful.
    #[serde(default, deserialize_with = "crate::model::lenient")]
    pub read_state: Option<bool>,
    /// …and can clear it (mark unread, `read_at` on reads).
    #[serde(default, deserialize_with = "crate::model::lenient")]
    pub read_state_clear: Option<bool>,
}

impl StockReadingPage {
    /// The server advertises read-state sync.
    pub fn read_sync(&self) -> bool {
        self.read_state == Some(true)
    }

    /// The server can clear read state.
    pub fn read_clear(&self) -> bool {
        self.read_sync() && self.read_state_clear == Some(true)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct StockEntry {
    /// Absent on your own posts.
    #[serde(default)]
    pub imported: Option<StockImported>,
}

/// An imported entry's identity and what tells it changed.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockImported {
    pub subscription_id: String,
    pub subscription_title: String,
    pub remote_id: String,
    pub observed_at: String,
    /// Bumped when the post changes (`observedAt` stays the first sighting).
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub withdrawn: bool,
    #[serde(default)]
    pub pinned_version_retained: Option<u32>,
    /// The entry's (sanitized) HTML: it changes with every new version, even
    /// within the second the timestamps resolve to.
    #[serde(default)]
    pub content_html: String,
    /// The highest version the owner marked read (null = unread), from a
    /// server that advertises `read_state`.
    #[serde(default)]
    pub read_version: Option<u32>,
}

impl StockImported {
    /// Everything about the entry that changes when the post does.
    pub fn signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            &self.observed_at,
            &self.updated,
            self.withdrawn,
            self.pinned_version_retained,
            &self.content_html,
        )
            .hash(&mut h);
        h.finish()
    }
}

/// Upstream's reading entries hold at most this many per request.
pub const STOCK_READING_PAGE: u32 = 50;

/// One reading row in the app's shape from upstream's `ImportedItem`
/// (`GET /api/imports/{sub}/{id}`) plus what the fork's row adds: the
/// subscription's title and origin, the thumb and the hopper ids, and the
/// server's read state (`readVersion` from `GET /api/reading`; `None` when
/// unread or the server keeps none). `None` if it doesn't read.
pub fn stock_row(
    imported: &Value,
    title: &str,
    origin: &str,
    thumb: Option<i8>,
    hoppers: &[String],
    read_version: Option<u32>,
) -> Option<ReadingItem> {
    let mut v = imported.as_object()?.clone();
    let parse = |k: &str| -> Value {
        v.get(k)
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or(Value::Null)
    };
    let author = match parse("author_json") {
        a @ Value::Object(_) => serde_json::json!({ "name": a.get("name"), "url": a.get("url") }),
        _ => Value::Null,
    };
    let transclusions = match parse("transclusions_json") {
        t @ Value::Array(_) => t,
        _ => Value::Null,
    };
    // Studio 0.18+ keeps the post's `stub_of` and `forked_from` (studio#12):
    // the keys are there, null or not, and then the row's lineage is whole.
    let lineage_known = v.contains_key("stub_of_json") || v.contains_key("forked_from_json");
    let stub_of = parse("stub_of_json");
    let forked_from = parse("forked_from_json");
    for k in [
        "author_json",
        "transclusions_json",
        "stub_of_json",
        "forked_from_json",
        "media_json",
        "content_hash",
        "l0",
    ] {
        v.remove(k);
    }
    v.insert("author".into(), author);
    v.insert("transclusions".into(), transclusions);
    v.insert("stub_of".into(), stub_of);
    v.insert("forked_from".into(), forked_from);
    v.insert("lineage_known".into(), lineage_known.into());
    v.insert("subscription_title".into(), title.into());
    v.insert("origin".into(), origin.into());
    v.insert("thumb".into(), thumb.map_or(Value::Null, Value::from));
    v.insert("hoppers".into(), hoppers.into());
    v.insert(
        "read_version".into(),
        read_version.map_or(Value::Null, Value::from),
    );
    serde_json::from_value(Value::Object(v)).ok()
}

/// At most this many entries per `POST /api/reading/read`.
pub const READ_BATCH_MAX: usize = 500;

#[cfg(test)]
mod responses_tests {
    use super::*;
    use serde_json::json;

    fn item(extra: Value) -> WireItem {
        let mut v = json!({ "id": "A", "kind": "fragment", "status": "public", "version": 1 });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn the_policy_resolves_against_the_site_default() {
        let mut w = item(json!({ "responses": "default" }));
        assert_eq!(w.responses_mode(), Some(ResponsesMode::Default));
        w.resolve(true, "https://blyg.example.com/b");
        assert!(w.shows_responses());
        w.resolve(false, "https://blyg.example.com/b");
        assert!(!w.shows_responses());
        let mut w = item(json!({ "responses": "show" }));
        w.resolve(false, "https://blyg.example.com/b");
        assert!(w.shows_responses());
        let mut w = item(json!({ "responses": "hide" }));
        w.resolve(true, "https://blyg.example.com/b");
        assert!(!w.shows_responses());
    }

    #[test]
    fn permalinks_carry_the_mount_and_the_kind() {
        let mut w = item(json!({ "kind": "withdrawn", "authored_kind": "thread" }));
        w.resolve(false, "https://blyg.example.com/b");
        assert_eq!(
            w.permalink.as_deref(),
            Some("https://blyg.example.com/b/t/A")
        );
        let mut d = item(json!({ "version": 0 }));
        d.resolve(false, "https://blyg.example.com");
        assert_eq!(d.permalink, None, "a draft has no page");
    }

    #[test]
    fn a_withdrawn_version_is_the_endcap() {
        let w = item(json!({ "versions": [
            { "version": 1, "published_at": "t1", "note": null, "pinned": true, "kind": "fragment" },
            { "version": 2, "published_at": "t2", "note": null, "pinned": false, "kind": "withdrawn" },
        ] }));
        let v = w.versions.unwrap();
        assert!(v[0].pinned && !v[0].endcap);
        assert!(v[1].endcap);
    }

    #[test]
    fn a_mention_author_is_parsed_from_its_json_string() {
        let w: WireMention = serde_json::from_value(json!({
            "id": "m", "target_item_id": "A", "status": "verified", "relation": "stub",
            "source": "https://other.example.com/t/x", "source_author_json": "{\"name\":\"Ana\",\"url\":null}",
            "first_seen": "t", "hidden": false,
        }))
        .unwrap();
        let m: Mention = w.into();
        assert_eq!(m.source_author.unwrap().name.as_deref(), Some("Ana"));
    }
}

#[cfg(test)]
mod stock_row_tests {
    use super::*;

    #[test]
    fn a_studio_0_18_row_carries_its_lineage() {
        let raw = serde_json::json!({
            "subscription_id": "S", "remote_id": "R", "kind": "thread", "state": "current",
            "version": 1, "created": null, "updated": null, "observed_at": "2030-01-01T00:00:00Z",
            "content_md": "Re", "content_html": "<p>Re</p>", "content_hash": null,
            "author_json": null, "media_json": null, "transclusions_json": null, "l0": false,
            "pinned_version_retained": null, "page": null,
            "stub_of_json": r#"{"origin":"https://ada.example/","id":"X","version":2}"#,
            "forked_from_json": null,
        });
        let r = stock_row(&raw, "N", "https://them.example/", None, &[], None).unwrap();
        assert!(r.lineage_known);
        assert_eq!(r.stub_of.and_then(|s| s.id).as_deref(), Some("X"));
        assert!(r.forked_from.is_none());
        // Before 0.18 the keys are absent, and the row says so.
        let mut old = raw.clone();
        let o = old.as_object_mut().unwrap();
        o.remove("stub_of_json");
        o.remove("forked_from_json");
        let r = stock_row(&old, "N", "https://them.example/", None, &[], None).unwrap();
        assert!(!r.lineage_known && r.stub_of.is_none());
    }

    #[test]
    fn a_live_imported_item_becomes_a_row() {
        let raw: Value = serde_json::from_str(r#"{"subscription_id":"S","remote_id":"R","kind":"fragment","state":"current","version":1,"created":"2026-10-03T03:56:29Z","updated":"2026-10-03T03:56:29Z","observed_at":"2026-10-03T03:56:29Z","content_md":"probe post","content_html":"<p>probe post</p>\n","content_hash":"sha256:a7","author_json":"{\"name\":\"\",\"url\":\"http://127.0.0.1:58955/\"}","media_json":"[]","transclusions_json":null,"l0":false,"pinned_version_retained":null,"page":"f/R/"}"#).unwrap();
        let r = stock_row(&raw, "N", "http://127.0.0.1:58955/", None, &[], None);
        assert!(
            r.is_some(),
            "{:?}",
            serde_json::from_value::<ReadingItem>(raw.clone()).err()
        );
    }
}
