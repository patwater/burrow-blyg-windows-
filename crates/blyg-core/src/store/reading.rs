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
/// The lineage of an item's current version, from its cached public
/// document (`remote_changelog`), when that has any.
fn cached_lineage(
    tx: &rusqlite::Connection,
    origin: &str,
    remote_id: &str,
) -> Result<Option<crate::model::Lineage>> {
    let json: Option<String> = tx
        .query_row(
            "SELECT json FROM remote_changelog WHERE origin = ?1 AND remote_id = ?2",
            [origin, remote_id],
            |r| r.get(0),
        )
        .optional()?;
    let log: Vec<crate::model::RemoteVersion> = json
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    Ok(crate::model::Lineage::of_changelog(&log).cloned())
}

fn retainable(tx: &rusqlite::Connection, it: &ReadingItem) -> Result<ReadingItem> {
    let mut it = it.clone();
    // Server rows don't carry lineage (upstream issue #12): fill it from the
    // post's own public document, once fetched (`Engine::fill_lineage`).
    if it.stub_of.is_none()
        && it.forked_from.is_none()
        && let Some(l) = cached_lineage(tx, &it.origin, &it.remote_id)?
    {
        it.fill_lineage(&l);
    }
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
/// Never lowers the local value, and never raises it to a version at or
/// below the row's unread floor (marked unread here; `read_sync.rs`). A
/// read version the server holds is confirmed (`read_at` NULL), and a newer
/// one than the floor lifts the floor. True if it rose.
fn raise_read(tx: &rusqlite::Connection, it: &ReadingItem) -> Result<bool> {
    let Some(v) = it.read_version else {
        return Ok(false);
    };
    let n = tx.execute(
        "UPDATE reading SET read_version = ?3, read_at = NULL, read_floor = NULL \
         WHERE subscription_id = ?1 AND remote_id = ?2 \
         AND (read_version IS NULL OR read_version < ?3) \
         AND (read_floor IS NULL OR read_floor < ?3)",
        params![it.subscription_id, it.remote_id, v],
    )?;
    Ok(n > 0)
}

/// The duplicate groups of the held rows named in `targets`; `None` when
/// none of them is held.
fn target_groups(rows: &[Row], targets: &[(String, String)]) -> Option<HashSet<String>> {
    let want: HashSet<(&str, &str)> = targets
        .iter()
        .map(|(s, r)| (s.as_str(), r.as_str()))
        .collect();
    let groups: HashSet<String> = rows
        .iter()
        .filter(|r| want.contains(&(r.item.subscription_id.as_str(), r.item.remote_id.as_str())))
        .map(|r| group_key(&r.item))
        .collect();
    (!groups.is_empty()).then_some(groups)
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
            put_refs(&tx, it)?;
            let json = to_json(it)?;
            let sub_kind = sub_kinds.get(&it.subscription_id).map(|k| kind_str(*k));
            let existing: Option<String> = tx
                .query_row(
                    "SELECT json FROM reading WHERE subscription_id = ?1 AND remote_id = ?2",
                    [&it.subscription_id, &it.remote_id],
                    |r| r.get(0),
                )
                .optional()?;
            let sort_at = it.sort_at();
            let fields = params![
                it.subscription_id,
                it.remote_id,
                it.origin,
                it.observed_at,
                it.page,
                sub_kind,
                it.state,
                it.version,
                json,
                sort_at
            ];
            if let Some(old) = existing {
                if old != json {
                    changed = true;
                }
                tx.execute(
                    "UPDATE reading SET origin = ?3, observed_at = ?4, page_url = ?5, sub_kind = ?6, state = ?7, \
                     version = ?8, json = ?9, sort_at = ?10 WHERE subscription_id = ?1 AND remote_id = ?2",
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
                    drop_refs(&tx, &it.subscription_id, &old_rid)?;
                    tx.execute(
                        "UPDATE reading SET remote_id = ?2, origin = ?3, observed_at = ?4, page_url = ?5, sub_kind = ?6, \
                         state = ?7, version = ?8, json = ?9, sort_at = ?11 WHERE subscription_id = ?1 AND remote_id = ?10",
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
                            old_rid,
                            sort_at
                        ],
                    )?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO reading (subscription_id, remote_id, origin, observed_at, page_url, sub_kind, \
                         state, version, json, sort_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
            drop_refs(&tx, &s, &rid)?;
            changed = true;
        }
        tx.commit()?;
        Ok(changed)
    }

    fn reading_rows(&self) -> Vec<Row> {
        let c = self.conn();
        let Ok(mut st) = c.prepare_cached(
            "SELECT json, read_version, sub_kind FROM reading \
             ORDER BY sort_at DESC, subscription_id, remote_id",
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

    /// The reading list as the UI shows it: newest first by the post's own
    /// date (`sort_at`: updated, else created, else observed_at), one entry
    /// per post.
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
    /// (the server keeps read state per subscription row).
    pub fn mark_read_rows(
        &self,
        sub: &str,
        remote_id: &str,
        queue: bool,
    ) -> Result<Vec<crate::api::wire::ReadMark>> {
        let marks = self.mark_many_read(&[(sub.to_string(), remote_id.to_string())], queue)?;
        if marks.is_none() {
            return Err(CoreError::NotFound);
        }
        Ok(marks.unwrap_or_default())
    }

    /// Mark many reading items read (each with every duplicate of the same
    /// post) in one transaction, at their current versions: a selection, or
    /// a whole list. Returns each row whose read state rose (`read_at` = now,
    /// ISO), or `None` when none of `targets` is held. With `queue`, a `read`
    /// op per such row, replacing any waiting `unread` op (last action wins).
    pub fn mark_many_read(
        &self,
        targets: &[(String, String)],
        queue: bool,
    ) -> Result<Option<Vec<crate::api::wire::ReadMark>>> {
        let rows = self.reading_rows();
        let Some(groups) = target_groups(&rows, targets) else {
            return Ok(None);
        };
        let now = crate::util::now_ms();
        let mut c = self.conn();
        let tx = c.transaction()?;
        let mut marked = Vec::new();
        for r in rows.iter().filter(|r| groups.contains(&group_key(&r.item))) {
            let it = &r.item;
            // Never lower: a version read elsewhere may be ahead of this row.
            if it.read_version >= Some(it.version) {
                continue;
            }
            tx.execute(
                "UPDATE reading SET read_version = ?3, read_at = ?4, read_floor = NULL \
                 WHERE subscription_id = ?1 AND remote_id = ?2",
                params![it.subscription_id, it.remote_id, it.version, now],
            )?;
            let m = crate::api::wire::ReadMark {
                sub: it.subscription_id.clone(),
                remote_id: it.remote_id.clone(),
                version: it.version,
                read_at: Some(crate::util::iso_from_ms(now)),
            };
            if queue {
                super::read_sync::queue_read(&tx, &m)?;
            }
            marked.push(m);
        }
        tx.commit()?;
        Ok(Some(marked))
    }

    /// Mark many reading items unread (each with every duplicate of the
    /// same post) in one transaction: their read version goes, and the
    /// version they were read at becomes their floor, so the server's old
    /// read state doesn't re-mark them read (`read_sync.rs`). Any waiting
    /// `read` op for them goes (last action wins). With `queue` (a server
    /// that can clear read state), an `unread` op per row. Returns the rows
    /// that were read, or `None` when none of `targets` is held.
    pub fn mark_many_unread(
        &self,
        targets: &[(String, String)],
        queue: bool,
    ) -> Result<Option<Vec<crate::api::wire::UnreadMark>>> {
        let rows = self.reading_rows();
        let Some(groups) = target_groups(&rows, targets) else {
            return Ok(None);
        };
        let mut c = self.conn();
        let tx = c.transaction()?;
        let mut cleared = Vec::new();
        for r in rows.iter().filter(|r| groups.contains(&group_key(&r.item))) {
            let it = &r.item;
            let Some(v) = it.read_version else {
                continue;
            };
            tx.execute(
                "UPDATE reading SET read_version = NULL, read_at = NULL, \
                 read_floor = MAX(COALESCE(read_floor, 0), ?3) \
                 WHERE subscription_id = ?1 AND remote_id = ?2",
                params![it.subscription_id, it.remote_id, v],
            )?;
            let m = crate::api::wire::UnreadMark {
                sub: it.subscription_id.clone(),
                remote_id: it.remote_id.clone(),
            };
            let key = super::read_sync::read_key(&m.sub, &m.remote_id);
            super::read_sync::drop_waiting_read(&tx, &key)?;
            if queue {
                super::read_sync::queue_unread(&tx, &m)?;
            }
            cleared.push(m);
        }
        tx.commit()?;
        Ok(Some(cleared))
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

    /// Posts held here that quote, stub or fork `(origin, id)`, newest
    /// first, one per post (duplicates collapse as in [`Self::reading`], to the
    /// strongest relation);
    /// withdrawn ones only when kept. An index lookup (`reading_refs`).
    pub fn responses_to(&self, origin: &str, id: &str) -> Vec<crate::model::Response> {
        let (origin, id) = crate::model::post_key(origin, id);
        let c = self.conn();
        let Ok(mut st) = c.prepare_cached(
            "SELECT r.json, r.read_version, r.sub_kind, f.relation, f.version, f.partial \
             FROM reading_refs f JOIN reading r \
               ON r.subscription_id = f.subscription_id AND r.remote_id = f.remote_id \
             WHERE f.target_id = ?1 AND f.target_origin = ?2 \
             ORDER BY r.sort_at DESC, r.subscription_id, r.remote_id",
        ) else {
            return vec![];
        };
        let rows = st
            .query_map([&id, &origin], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, bool>(5)?,
                ))
            })
            .map(|it| it.filter_map(|r| r.ok()).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut seen = std::collections::HashMap::new();
        let mut out: Vec<crate::model::Response> = Vec::new();
        for (json, rv, _kind, rel, version, partial) in rows {
            let Ok(mut item) = serde_json::from_str::<ReadingItem>(&json) else {
                continue;
            };
            item.read_version = rv.map(|v| v as u32);
            if item.state == "tombstone" && item.thumb.is_none() && item.hoppers.is_empty() {
                continue;
            }
            let Some(relation) = relation_of(&rel) else {
                continue;
            };
            let r = crate::model::Response {
                item,
                relation,
                version: version.map(|v| v as u32),
                partial,
            };
            // Two copies of one post (say a blyg and its RSS feed) are one
            // response, with the stronger relation either copy has.
            match seen.entry(group_key(&r.item)) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(out.len());
                    out.push(r);
                }
                std::collections::hash_map::Entry::Occupied(e) => {
                    let held = &mut out[*e.get()];
                    if r.relation > held.relation {
                        held.relation = r.relation;
                        held.version = r.version;
                    }
                    held.partial |= r.partial;
                }
            }
        }
        out
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

// --- responses ---

fn relation_str(r: crate::profile::Relation) -> &'static str {
    match r {
        crate::profile::Relation::Quotes => "quotes",
        crate::profile::Relation::Stubs => "stubs",
        crate::profile::Relation::Forks => "forks",
    }
}

fn relation_of(s: &str) -> Option<crate::profile::Relation> {
    Some(match s {
        "quotes" => crate::profile::Relation::Quotes,
        "stubs" => crate::profile::Relation::Stubs,
        "forks" => crate::profile::Relation::Forks,
        _ => return None,
    })
}

fn drop_refs(tx: &rusqlite::Connection, sub: &str, rid: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM reading_refs WHERE subscription_id = ?1 AND remote_id = ?2",
        [sub, rid],
    )?;
    Ok(())
}

/// Rewrite a row's references (`reading_refs`).
fn put_refs(tx: &rusqlite::Connection, it: &ReadingItem) -> Result<()> {
    drop_refs(tx, &it.subscription_id, &it.remote_id)?;
    for r in it.references() {
        tx.execute(
            "INSERT OR IGNORE INTO reading_refs (subscription_id, remote_id, target_origin, target_id, \
             relation, version, partial) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                it.subscription_id,
                it.remote_id,
                r.origin,
                r.id,
                relation_str(r.relation),
                r.version,
                r.partial
            ],
        )?;
    }
    Ok(())
}

/// Rebuild `reading_refs` from every row (after a migration).
pub(super) fn backfill_refs(conn: &mut rusqlite::Connection) -> Result<()> {
    let tx = conn.transaction()?;
    let rows: Vec<String> = {
        let mut st = tx.prepare("SELECT json FROM reading")?;
        st.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    tx.execute("DELETE FROM reading_refs", [])?;
    for json in rows {
        if let Ok(it) = serde_json::from_str::<ReadingItem>(&json) {
            put_refs(&tx, &it)?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Fill `sort_at` for rows written before it existed (schema v5): parse
/// each row's dates as `ReadingItem::sort_at` does. A row whose JSON doesn't
/// parse falls back to `observed_at`.
pub(super) fn backfill_sort_at(conn: &mut rusqlite::Connection) -> Result<()> {
    let tx = conn.transaction()?;
    let rows: Vec<(String, String, String, String)> = {
        let mut st = tx.prepare(
            "SELECT subscription_id, remote_id, json, observed_at FROM reading WHERE sort_at = ''",
        )?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (sub, rid, json, observed) in rows {
        let at = serde_json::from_str::<ReadingItem>(&json)
            .map(|it| it.sort_at())
            .unwrap_or(observed);
        tx.execute(
            "UPDATE reading SET sort_at = ?3 WHERE subscription_id = ?1 AND remote_id = ?2",
            params![sub, rid, at],
        )?;
    }
    tx.commit()?;
    Ok(())
}
