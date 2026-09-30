//! Cache of other people's public changelogs (short TTL) and pinned versions
//! (forever: a pin is an irrevocable, immutable hosting promise, spec §8).
//! Keyed by `(origin, remote_id)`, the protocol's identity for an item.

use rusqlite::{Connection, OptionalExtension, params};

use super::Store;
use crate::backend::{CoreError, Result};
use crate::model::{PinnedVersion, RemoteVersion};
use crate::util::now_ms;

/// A cached pin inside an open transaction: version `v`, or the newest one.
pub(super) fn pin_in(
    c: &Connection,
    origin: &str,
    remote_id: &str,
    v: Option<u32>,
) -> Result<Option<PinnedVersion>> {
    let json: Option<String> = match v {
        Some(v) => c
            .query_row(
                "SELECT json FROM remote_pins WHERE origin = ?1 AND remote_id = ?2 AND version = ?3",
                params![origin, remote_id, v],
                |r| r.get(0),
            )
            .optional()?,
        None => c
            .query_row(
                "SELECT json FROM remote_pins WHERE origin = ?1 AND remote_id = ?2 \
                 ORDER BY version DESC LIMIT 1",
                params![origin, remote_id],
                |r| r.get(0),
            )
            .optional()?,
    };
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
}

impl Store {
    /// The cached changelog and when it was fetched (unix ms).
    pub fn cached_changelog(
        &self,
        origin: &str,
        remote_id: &str,
    ) -> Option<(Vec<RemoteVersion>, i64)> {
        let c = self.conn();
        let (json, at): (String, i64) = c
            .query_row(
                "SELECT json, fetched_at FROM remote_changelog WHERE origin = ?1 AND remote_id = ?2",
                [origin, remote_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()?;
        Some((serde_json::from_str(&json).ok()?, at))
    }

    pub fn put_changelog(&self, origin: &str, remote_id: &str, v: &[RemoteVersion]) -> Result<()> {
        let json = serde_json::to_string(v).map_err(|e| CoreError::Storage(e.to_string()))?;
        self.conn().execute(
            "INSERT INTO remote_changelog (origin, remote_id, json, fetched_at) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (origin, remote_id) DO UPDATE SET json = excluded.json, fetched_at = excluded.fetched_at",
            params![origin, remote_id, json, now_ms()],
        )?;
        Ok(())
    }

    /// Forget a cached changelog (e.g. it claimed a pin the origin 404s).
    pub fn drop_changelog(&self, origin: &str, remote_id: &str) -> Result<()> {
        self.conn().execute(
            "DELETE FROM remote_changelog WHERE origin = ?1 AND remote_id = ?2",
            [origin, remote_id],
        )?;
        Ok(())
    }

    pub fn cached_pin(&self, origin: &str, remote_id: &str, version: u32) -> Option<PinnedVersion> {
        pin_in(&self.conn(), origin, remote_id, Some(version))
            .ok()
            .flatten()
    }

    /// Cache a fetched pin. Immutable, so a second write of the same version
    /// keeps the first.
    pub fn put_pin(&self, p: &PinnedVersion) -> Result<()> {
        let json = serde_json::to_string(p).map_err(|e| CoreError::Storage(e.to_string()))?;
        self.conn().execute(
            "INSERT OR IGNORE INTO remote_pins (origin, remote_id, version, json, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![p.origin, p.id, p.version, json, now_ms()],
        )?;
        Ok(())
    }

    // --- profiles ---

    /// A cached profile and when it was fetched (unix ms).
    pub fn cached_profile(&self, key: &str) -> Option<(crate::profile::Profile, i64)> {
        let (json, at): (String, i64) = self
            .conn()
            .query_row(
                "SELECT json, fetched_at FROM profiles WHERE key = ?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()?;
        Some((serde_json::from_str(&json).ok()?, at))
    }

    /// Cache a fetched profile under each of `keys`.
    pub fn put_profile(&self, keys: &[&str], p: &crate::profile::Profile) -> Result<()> {
        let json = serde_json::to_string(p).map_err(|e| CoreError::Storage(e.to_string()))?;
        let c = self.conn();
        for k in keys {
            c.execute(
                "INSERT INTO profiles (key, json, fetched_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (key) DO UPDATE SET json = excluded.json, fetched_at = excluded.fetched_at",
                params![k, json, p.fetched_at],
            )?;
        }
        Ok(())
    }
}
