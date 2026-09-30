//! Reading list cache: one entry per post, however often it's edited.
//!
//! - Rows are keyed by `(subscription_id, remote_id)` and upserted, never
//!   appended; an edit bumps `version`/`observed_at` on the same row.
//! - Read state (`read_version`, a number only) survives edits, so an
//!   edited post reads as "edited · vN", not as a new unread item. The text of
//!   the version you read is never kept (spec §8.4); a diff is only possible
//!   when that version is pinned (`remote_pins`, see `LiveBackend`). It is
//!   set locally, and merged up (never down) from a server that syncs it
//!   (extension 5, `read_sync.rs`).
//! - RSS items whose guid changes on edit: a new remote id in the same
//!   subscription with the same resolved page URL replaces the old row and
//!   inherits its read state.
//! - `reading()` collapses cross-subscription duplicates (the same post via a
//!   blyg and its RSS feed, or two subscriptions to one origin) on the
//!   absolute page URL, else `(origin, remote_id)`, preferring the blyg row.
//! - Tombstones are hidden unless the user signalled or hoppered them, and
//!   their content is dropped on arrival unless a pin backs it (the server's
//!   `pinned_version_retained`, or a pinned version cached in `remote_pins`),
//!   in which case only that pinned content is kept (spec §13.4).

use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, params};

use super::Store;
use crate::backend::{CoreError, Result};
use crate::model::{ReadingItem, SubscriptionKind};

struct Row {
    item: ReadingItem,
    sub_kind: Option<String>,
}

fn group_key(it: &ReadingItem) -> String {
    match &it.page {
        Some(p) if !p.is_empty() => format!("page\0{p}"),
        _ => format!("id\0{}\0{}", it.origin, it.remote_id),
    }
}

fn to_json(it: &ReadingItem) -> Result<String> {
    let mut clean = it.clone();
    clean.read_version = None;
    serde_json::to_string(&clean).map_err(|e| CoreError::Storage(e.to_string()))
}

/// What may be kept of an incoming item. A tombstone keeps no content unless a
/// pin backs it: then only the pinned version's content, marked with
/// `pinned_version_retained` for attribution. "Local hoarding past withdrawal
/// is nonconforming" (spec §13.4).
fn retainable(tx: &rusqlite::Connection, it: &ReadingItem) -> Result<ReadingItem> {
    let mut it = it.clone();
    if it.state != "tombstone" {
        it.pinned_version_retained = None;
        return Ok(it);
    }
    // The server kept the pinned bytes: take them as they are.
    if it.pinned_version_retained.is_some() && !it.content_md.is_empty() {
        return Ok(it);
    }
    // Otherwise a pin we cached ourselves (that version, or the newest one).
    let pin = super::remote::pin_in(tx, &it.origin, &it.remote_id, it.pinned_version_retained)?;
    match pin {
        Some(p) => {
            it.content_md = p.content_md;
            it.content_html = p.content_html;
            it.pinned_version_retained = Some(p.version);
        }
        None => {
            it.content_md.clear();
            it.content_html.clear();
            it.pinned_version_retained = None;
        }
    }
    Ok(it)
}

/// Merge the server's read state into a stored row: `max(local, server)`.
/// Never lowers the local value. True if it rose.
fn raise_read(tx: &rusqlite::Connection, it: &ReadingItem) -> Result<bool> {
    let Some(v) = it.read_version else {
        return Ok(false);
    };
    let n = tx.execute(
        "UPDATE reading SET read_version = ?3 WHERE subscription_id = ?1 AND remote_id = ?2 \
         AND (read_version IS NULL OR read_version < ?3)",
        params![it.subscription_id, it.remote_id, v],
    )?;
    Ok(n > 0)
}

fn kind_str(k: SubscriptionKind) -> &'static str {
    match k {
        SubscriptionKind::Blyg => "blyg",
        SubscriptionKind::Rss => "rss",
    }
}

impl Store {
    /// Upsert a pulled reading list. `complete` = the server had no further
    /// pages, so rows it didn't return are gone (e.g. unsubscribed). Otherwise
    /// only rows at least as new as the oldest returned are pruned.
    /// Returns true if anything changed.
    pub fn merge_reading(
        &self,
        items: &[ReadingItem],
        complete: bool,
        sub_kinds: &HashMap<String, SubscriptionKind>,
    ) -> Result<bool> {
        let incoming: HashSet<(String, String)> = items
            .iter()
            .map(|i| (i.subscription_id.clone(), i.remote_id.clone()))
            .collect();
        let mut changed = false;
        let mut c = self.conn();
        let tx = c.transaction()?;
        for it in items {
            let kept = retainable(&tx, it)?;
            let it = &kept;
            let json = to_json(it)?;
            let sub_kind = sub_kinds.get(&it.subscription_id).map(|k| kind_str(*k));
            let existing: Option<String> = tx
                .query_row(
                    "SELECT json FROM reading WHERE subscription_id = ?1 AND remote_id = ?2",
                    [&it.subscription_id, &it.remote_id],
                    |r| r.get(0),
                )
                .optional()?;
            let fields = params![
                it.subscription_id,
                it.remote_id,
                it.origin,
                it.observed_at,
                it.page,
                sub_kind,
                it.state,
                it.version,
                json
            ];
            if let Some(old) = existing {
                if old != json {
                    changed = true;
                }
                tx.execute(
                    "UPDATE reading SET origin = ?3, observed_at = ?4, page_url = ?5, sub_kind = ?6, state = ?7, \
                     version = ?8, json = ?9 WHERE subscription_id = ?1 AND remote_id = ?2",
                    fields,
                )?;
                changed |= raise_read(&tx, it)?;
                continue;
            }
            changed = true;
            // Same subscription, same page, different id that the server no
            // longer lists: an RSS guid that changed on edit. Same post.
            let renamed: Option<String> = match &it.page {
                Some(page) if !page.is_empty() => {
                    let mut st = tx.prepare_cached(
                        "SELECT remote_id FROM reading WHERE subscription_id = ?1 AND page_url = ?2 AND remote_id <> ?3",
                    )?;
                    let cands = st
                        .query_map([&it.subscription_id, page, &it.remote_id], |r| {
                            r.get::<_, String>(0)
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    cands
                        .into_iter()
                        .find(|rid| !incoming.contains(&(it.subscription_id.clone(), rid.clone())))
                }
                _ => None,
            };
            match renamed {
                Some(old_rid) => {
                    tx.execute(
                        "UPDATE reading SET remote_id = ?2, origin = ?3, observed_at = ?4, page_url = ?5, sub_kind = ?6, \
                         state = ?7, version = ?8, json = ?9 WHERE subscription_id = ?1 AND remote_id = ?10",
                        params![
                            it.subscription_id,
                            it.remote_id,
                            it.origin,
                            it.observed_at,
                            it.page,
                            sub_kind,
                            it.state,
                            it.version,
                            json,
                            old_rid
                        ],
                    )?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO reading (subscription_id, remote_id, origin, observed_at, page_url, sub_kind, \
                         state, version, json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        fields,
                    )?;
                }
            }
            raise_read(&tx, it)?;
        }

        // Prune rows the server no longer returns.
        let oldest = items
            .iter()
            .map(|i| i.observed_at.as_str())
            .min()
            .map(str::to_string);
        let stale: Vec<(String, String)> = {
            let mut st =
                tx.prepare("SELECT subscription_id, remote_id, observed_at FROM reading")?;
            st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .filter(|(s, rid, obs)| {
                !incoming.contains(&(s.clone(), rid.clone()))
                    && (complete || oldest.as_deref().is_some_and(|o| obs.as_str() >= o))
            })
            .map(|(s, rid, _)| (s, rid))
            .collect()
        };
        for (s, rid) in stale {
            tx.execute(
                "DELETE FROM reading WHERE subscription_id = ?1 AND remote_id = ?2",
                [&s, &rid],
            )?;
            changed = true;
        }
        tx.commit()?;
        Ok(changed)
    }

    fn reading_rows(&self) -> Vec<Row> {
        let c = self.conn();
        let Ok(mut st) = c.prepare_cached(
            "SELECT json, read_version, sub_kind FROM reading \
             ORDER BY observed_at DESC, subscription_id, remote_id",
        ) else {
            return vec![];
        };
        st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .map(|it| {
            it.filter_map(|r| r.ok())
                .filter_map(|(json, rv, sub_kind)| {
                    let mut item: ReadingItem = serde_json::from_str(&json).ok()?;
                    item.read_version = rv.map(|v| v as u32);
                    Some(Row { item, sub_kind })
                })
                .collect()
        })
        .unwrap_or_default()
    }

    /// The reading list as the UI shows it: newest first, one entry per post.
    pub fn reading(&self) -> Vec<ReadingItem> {
        let rows = self.reading_rows();
        // group key → index into `out` (position = the group's newest row)
        let mut slot: HashMap<String, usize> = HashMap::new();
        let mut out: Vec<Row> = Vec::new();
        for row in rows {
            let it = &row.item;
            if it.state == "tombstone" && it.thumb.is_none() && it.hoppers.is_empty() {
                continue;
            }
            let key = group_key(it);
            match slot.get(&key) {
                None => {
                    slot.insert(key, out.len());
                    out.push(row);
                }
                Some(&i) => {
                    if better(&row, &out[i]) {
                        out[i] = row;
                    }
                }
            }
        }
        out.into_iter().map(|r| r.item).collect()
    }

    /// Mark a reading item, and every duplicate of the same post, as read at
    /// its current version.
    pub fn mark_read(&self, sub: &str, remote_id: &str) -> Result<bool> {
        Ok(!self.mark_read_rows(sub, remote_id, false)?.is_empty())
    }

    /// `mark_read`, returning each row whose read state rose. With `queue`,
    /// the same transaction puts a `read` op per such row in the outbox
    /// (extension 5: the server keeps read state per subscription row).
    pub fn mark_read_rows(
        &self,
        sub: &str,
        remote_id: &str,
        queue: bool,
    ) -> Result<Vec<crate::api::wire::ReadMark>> {
        let rows = self.reading_rows();
        let Some(target) = rows
            .iter()
            .find(|r| r.item.subscription_id == sub && r.item.remote_id == remote_id)
        else {
            return Err(CoreError::NotFound);
        };
        let key = group_key(&target.item);
        let mut c = self.conn();
        let tx = c.transaction()?;
        let mut marked = Vec::new();
        for r in rows.iter().filter(|r| group_key(&r.item) == key) {
            let it = &r.item;
            // Never lower: a version read elsewhere may be ahead of this row.
            if it.read_version >= Some(it.version) {
                continue;
            }
            tx.execute(
                "UPDATE reading SET read_version = ?3 WHERE subscription_id = ?1 AND remote_id = ?2",
                params![it.subscription_id, it.remote_id, it.version],
            )?;
            let m = crate::api::wire::ReadMark {
                sub: it.subscription_id.clone(),
                remote_id: it.remote_id.clone(),
                version: it.version,
            };
            if queue {
                super::read_sync::queue_read(&tx, &m)?;
            }
            marked.push(m);
        }
        tx.commit()?;
        Ok(marked)
    }

    /// One cached row (not de-duplicated, tombstones included) and its
    /// subscription kind (`"blyg"` | `"rss"`, when known).
    pub fn reading_row(&self, sub: &str, remote_id: &str) -> Option<(ReadingItem, Option<String>)> {
        let c = self.conn();
        let (json, rv, sub_kind): (String, Option<i64>, Option<String>) = c
            .query_row(
                "SELECT json, read_version, sub_kind FROM reading WHERE subscription_id = ?1 AND remote_id = ?2",
                [sub, remote_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok()?;
        let mut item: ReadingItem = serde_json::from_str(&json).ok()?;
        item.read_version = rv.map(|v| v as u32);
        Some((item, sub_kind))
    }

    pub fn set_thumb(&self, sub: &str, remote_id: &str, thumb: Option<i8>) -> Result<bool> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let json: Option<String> = tx
            .query_row(
                "SELECT json FROM reading WHERE subscription_id = ?1 AND remote_id = ?2",
                [sub, remote_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(json) = json else { return Ok(false) };
        let Ok(mut it) = serde_json::from_str::<ReadingItem>(&json) else {
            return Ok(false);
        };
        it.thumb = thumb;
        tx.execute(
            "UPDATE reading SET json = ?3 WHERE subscription_id = ?1 AND remote_id = ?2",
            params![sub, remote_id, to_json(&it)?],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

/// Which of two rows for the same post to show: the blyg-kind subscription
/// beats RSS; then the higher version; then the most recently observed.
fn better(a: &Row, b: &Row) -> bool {
    let rank = |r: &Row| {
        (
            r.sub_kind.as_deref() == Some("blyg"),
            r.item.version,
            r.item.observed_at.clone(),
        )
    };
    rank(a) > rank(b)
}
