//! Wire shapes the owner API returns that don't map 1:1 onto `model` types.
//! Field names follow `apps/blyg/src/owner-read-api.ts` and `api.ts`.

use serde::Deserialize;
use serde_json::Value;

use crate::model::{Kind, ReadingItem, RemoteRef, Status, Version};

/// `GET /api/items` element and `GET /api/items/:id` body.
#[derive(Debug, Clone, Deserialize)]
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
    #[serde(default)]
    pub permalink: Option<String>,
    /// Patch 3; absent on older servers.
    #[serde(default)]
    pub show_responses: bool,
    /// Only on `GET /api/items/:id`.
    #[serde(default)]
    pub versions: Option<Vec<Version>>,
}

impl WireItem {
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

/// `POST /api/items` and `POST /api/fork` → `201 {id, kind, status}`.
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

/// `POST /api/media` → `201 {id, url, mime}`, or `200 {…, duplicate: true}`
/// when the item already has an attachment with identical bytes (patch 8:
/// the existing row comes back and nothing new is stored).
#[derive(Debug, Clone, Deserialize)]
pub struct Media {
    pub id: String,
    pub url: String,
    pub mime: String,
    #[serde(default)]
    pub duplicate: bool,
}

/// `GET /api/reading` (patch 3) → `{items, next}`, plus `read_state: true`
/// from a server that stores read state (extension 5; each item then carries
/// its `read_version`).
#[derive(Debug, Clone, Deserialize)]
pub struct ReadingPage {
    pub items: Vec<ReadingItem>,
    #[serde(default)]
    pub next: Option<String>,
    #[serde(default, deserialize_with = "crate::model::lenient")]
    pub read_state: Option<bool>,
}

impl ReadingPage {
    /// The server advertises read-state sync (extension 5).
    pub fn read_sync(&self) -> bool {
        self.read_state == Some(true)
    }
}

/// One row's read state as sent to the server: `(sub, remote_id)` is a
/// reading row, `version` the highest version read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct ReadMark {
    pub sub: String,
    pub remote_id: String,
    pub version: u32,
}

/// At most this many entries per `POST /api/reading/read`.
pub const READ_BATCH_MAX: usize = 500;
