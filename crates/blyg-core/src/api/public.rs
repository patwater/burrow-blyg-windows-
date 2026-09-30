//! Unauthenticated reads of other blygs' public surface (`items/{id}.json`,
//! `items/{id}/v{n}.json`).
//!
//! This is deliberately a separate `ureq::Agent` from the owner `Api`, and it
//! holds no token at all, so there is no code path by which the owner token
//! could reach a foreign origin (spec: "never send the owner token anywhere but
//! the user's own blyg").

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::backend::{CoreError, Result};
use crate::model::{
    Author, Lineage, PinnedVersion, RemoteMedia, RemoteRef, RemoteVersion, StubOf, TransclusionRef,
    content_hash, item_doc_url, origin_url, pin_doc_url,
};

pub struct PublicClient {
    agent: ureq::Agent,
}

impl std::fmt::Debug for PublicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PublicClient")
    }
}

impl Default for PublicClient {
    fn default() -> Self {
        Self::new()
    }
}

/// The parts of a public item document (§5) this client reads. Unknown fields
/// are ignored (tolerance rule).
#[derive(Debug, Clone, Deserialize)]
pub struct ItemDoc {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default)]
    pub changelog: Vec<ChangelogEntry>,
    /// Attached images (§5.4), origin-relative as served. Entries of an
    /// unexpected shape are skipped (tolerance rule).
    #[serde(default, deserialize_with = "lenient_media")]
    pub media: Vec<MediaEntry>,
    /// Lineage (v0.3). The owner reading API doesn't send these, so the
    /// reading header reads them from here. Odd shapes read as absent.
    #[serde(default, deserialize_with = "crate::model::lenient")]
    pub stub_of: Option<StubOf>,
    #[serde(default, deserialize_with = "crate::model::lenient")]
    pub forked_from: Option<RemoteRef>,
    #[serde(default, deserialize_with = "crate::model::lenient_vec")]
    pub transclusions: Vec<TransclusionRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaEntry {
    pub url: String,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub alt: Option<String>,
}

fn lenient_media<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Vec<MediaEntry>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Array(a) => a
            .into_iter()
            .filter_map(|m| serde_json::from_value(m).ok())
            .collect(),
        _ => Vec::new(),
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangelogEntry {
    pub version: u32,
    #[serde(default)]
    pub at: String,
    #[serde(default)]
    pub note: Option<String>,
    /// `pinned?: true`; absent means not pinned.
    #[serde(default)]
    pub pinned: Option<bool>,
}

impl ItemDoc {
    /// The changelog as `RemoteVersion`s, oldest first. A doc without a
    /// changelog still yields its current version.
    pub fn versions(&self) -> Vec<RemoteVersion> {
        let mut out: Vec<RemoteVersion> = self
            .changelog
            .iter()
            .filter(|c| c.version > 0)
            .map(|c| RemoteVersion {
                version: c.version,
                at: c.at.clone(),
                note: c.note.clone().filter(|n| !n.is_empty()),
                pinned: c.pinned == Some(true),
                current: c.version == self.version,
                media: Vec::new(),
                lineage: Lineage::default(),
            })
            .collect();
        out.sort_by_key(|v| v.version);
        out.dedup_by_key(|v| v.version);
        if self.version > 0 && !out.iter().any(|v| v.current) {
            out.push(RemoteVersion {
                version: self.version,
                at: self.updated.clone().unwrap_or_default(),
                note: None,
                pinned: false,
                current: true,
                media: Vec::new(),
                lineage: Lineage::default(),
            });
        }
        out
    }

    /// `stub_of`, `forked_from` and `transclusions` as the document has them.
    pub fn lineage(&self) -> Lineage {
        Lineage {
            stub_of: self.stub_of.clone(),
            forked_from: self.forked_from.clone(),
            transclusions: self.transclusions.clone(),
        }
    }

    /// [`versions`](Self::versions), with the attached images and the
    /// lineage on the current row. Images are resolved against `origin` (the
    /// origin that was fetched, not the one the document claims); only
    /// http(s) URLs are kept.
    pub fn versions_at(&self, origin: &str) -> Vec<RemoteVersion> {
        let media: Vec<RemoteMedia> = self
            .media
            .iter()
            .filter(|m| m.mime.as_deref().is_none_or(|t| t.starts_with("image/")))
            .filter_map(|m| {
                let url = origin_url(origin, m.url.trim())?;
                (url.starts_with("https://") || url.starts_with("http://")).then(|| RemoteMedia {
                    url,
                    alt: m.alt.clone().filter(|a| !a.trim().is_empty()),
                })
            })
            .collect();
        let mut out = self.versions();
        if let Some(cur) = out.iter_mut().find(|v| v.current) {
            cur.media = media;
            cur.lineage = self.lineage();
        }
        out
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PinDoc {
    #[serde(default)]
    id: Option<String>,
    version: u32,
    #[serde(default)]
    at: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    pinned: Option<bool>,
    #[serde(default)]
    author: Option<Author>,
    #[serde(default)]
    content_md: String,
    #[serde(default)]
    content_html: String,
    #[serde(default)]
    content_hash: String,
}

impl PublicClient {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(20))
            .redirects(5)
            .build();
        PublicClient { agent }
    }

    fn get(&self, url: &str) -> Result<Value> {
        // No `authorization` header, ever: see the module docs.
        let res = self.agent.get(url).set("accept", "application/json").call();
        match res {
            Ok(r) => {
                let mut s = String::new();
                r.into_reader()
                    .take(16 * 1024 * 1024)
                    .read_to_string(&mut s)
                    .map_err(|_| CoreError::Offline)?;
                serde_json::from_str(&s)
                    .map_err(|e| CoreError::Other(format!("bad JSON from {url}: {e}")))
            }
            Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
            Err(ureq::Error::Status(404 | 410, _)) => Err(CoreError::NotFound),
            Err(ureq::Error::Status(code, _)) => Err(CoreError::Rejected {
                status: code,
                message: format!("{url} returned {code}"),
                details: vec![],
            }),
        }
    }

    /// Raw bytes of a public http(s) resource (an image in a post), at most
    /// `max` bytes, with its `content-type`. Never sends a token.
    pub fn get_bytes(&self, url: &str, max: u64) -> Result<(Vec<u8>, Option<String>)> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(CoreError::Other(format!("not an http(s) URL: {url}")));
        }
        match self.agent.get(url).call() {
            Ok(r) => {
                let mime = r
                    .header("content-type")
                    .map(|m| m.split(';').next().unwrap_or(m).trim().to_ascii_lowercase());
                let mut bytes = Vec::new();
                r.into_reader()
                    .take(max + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| CoreError::Offline)?;
                if bytes.len() as u64 > max {
                    return Err(CoreError::Other(format!(
                        "{url} is larger than {max} bytes"
                    )));
                }
                Ok((bytes, mime))
            }
            Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
            Err(ureq::Error::Status(404 | 410, _)) => Err(CoreError::NotFound),
            Err(ureq::Error::Status(code, _)) => Err(CoreError::Rejected {
                status: code,
                message: format!("{url} returned {code}"),
                details: vec![],
            }),
        }
    }

    // --- profiles ---

    /// A public JSON document (a manifest, an archive index). No token, no
    /// cookies (this agent has no cookie store). 404/410 → `NotFound`.
    pub fn get_json(&self, url: &str) -> Result<Value> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(CoreError::Other(format!("not an http(s) URL: {url}")));
        }
        self.get(url)
    }

    /// A public text resource (OPML, RSS/Atom), at most `max` bytes (longer
    /// is cut, not refused: a feed's head is what a profile needs). No
    /// token, no cookies. 404/410 → `NotFound`.
    pub fn get_text(&self, url: &str, max: u64) -> Result<String> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(CoreError::Other(format!("not an http(s) URL: {url}")));
        }
        let res = self
            .agent
            .get(url)
            .set(
                "accept",
                "application/rss+xml, application/atom+xml, text/x-opml, application/xml, text/xml;q=0.9, */*;q=0.5",
            )
            .call();
        match res {
            Ok(r) => {
                let mut bytes = Vec::new();
                r.into_reader()
                    .take(max)
                    .read_to_end(&mut bytes)
                    .map_err(|_| CoreError::Offline)?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            }
            Err(ureq::Error::Transport(_)) => Err(CoreError::Offline),
            Err(ureq::Error::Status(404 | 410, _)) => Err(CoreError::NotFound),
            Err(ureq::Error::Status(code, _)) => Err(CoreError::Rejected {
                status: code,
                message: format!("{url} returned {code}"),
                details: vec![],
            }),
        }
    }

    /// `GET {origin}items/{id}.json`.
    pub fn item_doc(&self, origin: &str, id: &str) -> Result<ItemDoc> {
        let url = item_doc_url(origin, id)
            .ok_or_else(|| CoreError::Other(format!("not a blyg origin: {origin}")))?;
        let v = self.get(&url)?;
        serde_json::from_value(v)
            .map_err(|e| CoreError::Other(format!("unexpected item document at {url}: {e}")))
    }

    /// `GET {origin}items/{id}/v{n}.json`. 404 → `Rejected{404}` (not pinned).
    /// The hash is checked over `content_md` and a mismatch is flagged.
    pub fn pinned(&self, origin: &str, id: &str, version: u32) -> Result<PinnedVersion> {
        let url = pin_doc_url(origin, id, version)
            .ok_or_else(|| CoreError::Other(format!("not a blyg origin: {origin}")))?;
        let v = match self.get(&url) {
            Err(CoreError::NotFound) => return Err(not_pinned(version)),
            r => r?,
        };
        let d: PinDoc = serde_json::from_value(v)
            .map_err(|e| CoreError::Other(format!("unexpected pin document at {url}: {e}")))?;
        if d.version != version || d.pinned == Some(false) {
            return Err(CoreError::Other(format!(
                "{url} is not pinned version {version}"
            )));
        }
        if d.id.as_deref().is_some_and(|i| i != id) {
            return Err(CoreError::Other(format!("{url} is a different item")));
        }
        let hash_mismatch = d.content_hash != content_hash(&d.content_md);
        Ok(PinnedVersion {
            version,
            at: d.at,
            note: d.note.filter(|n| !n.is_empty()),
            content_md: d.content_md,
            content_html: d.content_html,
            content_hash: d.content_hash,
            // Scope to the origin we asked, not the one the doc claims (§12.2:
            // the fetch URL is what's authenticated, by DNS).
            origin: origin.to_string(),
            id: id.to_string(),
            author: d.author,
            hash_mismatch,
        })
    }
}

/// The one error for "that version has no public content".
pub fn not_pinned(version: u32) -> CoreError {
    CoreError::Rejected {
        status: 404,
        message: format!("v{version} isn't pinned, so it has no public content"),
        details: vec![],
    }
}
