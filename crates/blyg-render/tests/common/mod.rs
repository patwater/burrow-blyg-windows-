//! Shared test support: the fake store from `fixtures/fake_store.json` as a
//! [`Resolver`] implementing the reference Worker's `resolveTarget` rules
//! (the same store `gen_parity.mjs` serves to the Worker as a fake D1).

#![allow(dead_code)]

use blyg_render::{Found, ItemKind, Resolution, Resolver, UnresolvedReason};
use serde_json::Value;
use std::path::PathBuf;

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub struct FakeStore {
    pub json: Value,
}

impl FakeStore {
    pub fn load() -> Self {
        let text = std::fs::read_to_string(fixtures_dir().join("fake_store.json"))
            .expect("fake_store.json");
        FakeStore {
            json: serde_json::from_str(&text).expect("valid json"),
        }
    }

    pub fn self_id(&self) -> String {
        self.json["self_id"].as_str().unwrap().to_string()
    }

    pub fn mount(&self) -> String {
        self.json["mount"].as_str().unwrap().to_string()
    }
}

fn kind_of(v: &Value) -> ItemKind {
    if v.as_str() == Some("thread") {
        ItemKind::Thread
    } else {
        ItemKind::Fragment
    }
}

impl Resolver for FakeStore {
    fn resolve(&self, id: &str) -> Resolution {
        if let Some(item) = self.json["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == id)
        {
            if item["status"] == "draft" {
                return Resolution::Unavailable(UnresolvedReason::Draft);
            }
            if item["kind"] == "withdrawn" {
                return Resolution::Unavailable(UnresolvedReason::Withdrawn);
            }
            return Resolution::Found(Found {
                origin: None,
                id: id.to_string(),
                version: item["version"].as_u64().unwrap() as u32,
                kind: kind_of(&item["kind"]),
                content_html: item["content_html"].as_str().unwrap().to_string(),
                author: None,
                page: None,
            });
        }
        let rows: Vec<&Value> = self.json["imported"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["remote_id"] == id)
            .collect();
        let candidates: Vec<&&Value> = rows.iter().filter(|r| r["l0"] != 1).collect();
        let usable: Vec<&&&Value> = candidates
            .iter()
            .filter(|r| r["state"] == "current" || !r["pinned_version_retained"].is_null())
            .collect();
        if usable.len() > 1 {
            return Resolution::Ambiguous;
        }
        if let Some(row) = usable.first() {
            let version = if row["state"] == "tombstone" {
                row["pinned_version_retained"].as_u64().unwrap()
            } else {
                row["version"].as_u64().unwrap()
            };
            return Resolution::Found(Found {
                origin: Some(row["origin"].as_str().unwrap().to_string()),
                id: id.to_string(),
                version: version as u32,
                kind: kind_of(&row["kind"]),
                content_html: row["content_html"].as_str().unwrap().to_string(),
                author: row["sub_title"].as_str().map(str::to_string),
                page: row["page"].as_str().map(str::to_string),
            });
        }
        if !candidates.is_empty() {
            return Resolution::Unavailable(UnresolvedReason::SourceWithdrawn);
        }
        if !rows.is_empty() {
            return Resolution::RssNotQuotable;
        }
        Resolution::NotFound
    }
}

/// Collapse whitespace that sits between tags (`>\s+<` → `><`) and trim.
/// markdown-it and markdown-it.rs differ only in how many newlines they
/// emit between blocks; text content is compared byte for byte.
pub fn normalize(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'>' {
            out.push('>');
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'<' {
                i = j;
                continue;
            }
            i += 1;
            continue;
        }
        // push the whole UTF-8 char
        let ch = html[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out.trim().to_string()
}
