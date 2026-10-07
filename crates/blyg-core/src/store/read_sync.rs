//! Read-state sync bookkeeping (upstream's read-state routes, once the
//! fork's extension 5; docs/SERVER.md).
//!
//! A `read` op in the outbox says "tell the server this row was read up to
//! `version`"; an `unread` op says "clear this row's read state". Their
//! `local_id` is a key derived from `(sub, remote_id)` (never an item's id),
//! so ops for different rows never block each other. A row has at most one
//! *waiting* op, and the last action wins: queueing a read replaces a waiting
//! unread and vice versa, and a second read raises the waiting one's version
//! (an op already on the wire is left alone; the new one waits behind it).
//!
//! Two columns on `reading` carry the rest (schema v12):
//!
//! - `read_at`: when the row was marked read here (unix ms), until the
//!   server confirms it, then NULL. To a server that advertises
//!   `read_state_clear` it goes as `read_at`, so a read made before another
//!   device's "mark unread" can't undo it. NULL with a read version means
//!   "the server has this", which is how a later pull can tell "cleared on
//!   another device" (adopt it) from "read here, not sent yet" (send it).
//! - `read_floor`: the row was marked unread here at that version. A server
//!   read version at or below it is stale and doesn't re-mark the row read.
//!   On a server that can clear read state the floor goes once the `unread`
//!   op is acknowledged. On one that can't, unread stays on this Mac: the
//!   floor keeps the server's old read version from coming back every pull,
//!   until the post is read again (here, or at a newer version elsewhere).

use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, params};

use super::{OpKind, Store};
use crate::api::wire::{ReadMark, UnreadMark};
use crate::backend::Result;
use crate::model::LocalId;
use crate::util::{iso_from_ms, now_ms};

/// Outbox key for a reading row's read or unread op.
pub(crate) fn read_key(sub: &str, remote_id: &str) -> LocalId {
    LocalId(format!("read\u{1f}{sub}\u{1f}{remote_id}"))
}

/// `(sub, remote_id)` back from a `read_key`.
fn row_of_key(key: &str) -> Option<(String, String)> {
    let mut it = key.strip_prefix("read\u{1f}")?.splitn(2, '\u{1f}');
    Some((it.next()?.to_string(), it.next()?.to_string()))
}

/// The `read_at` a read op carries: the local mark time, ISO-8601 UTC.
/// Unknown (0: read before schema v12) becomes the epoch, which loses to
/// any "mark unread" and applies otherwise.
pub(crate) fn read_at_iso(ms: Option<i64>) -> String {
    iso_from_ms(ms.unwrap_or(0).max(0))
}

/// Queue (or raise) the read op for one row, inside the caller's
/// transaction. A waiting unread op for the row is dropped: last action wins.
pub(crate) fn queue_read(tx: &rusqlite::Connection, m: &ReadMark) -> rusqlite::Result<()> {
    let key = read_key(&m.sub, &m.remote_id);
    tx.execute(
        "DELETE FROM outbox WHERE local_id = ?1 AND op = 'unread' AND in_flight = 0",
        [&key.0],
    )?;
    let waiting: Option<(i64, String)> = tx
        .query_row(
            "SELECT seq, payload FROM outbox WHERE local_id = ?1 AND op = 'read' AND in_flight = 0 \
             ORDER BY seq DESC LIMIT 1",
            [&key.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let payload = |m: &ReadMark| serde_json::to_string(m).unwrap_or_else(|_| "{}".into());
    match waiting {
        Some((seq, old)) => {
            let old = serde_json::from_str::<ReadMark>(&old).ok();
            if old
                .as_ref()
                .is_some_and(|o| o.version >= m.version && o.read_at >= m.read_at)
            {
                return Ok(());
            }
            tx.execute(
                "UPDATE outbox SET payload = ?2 WHERE seq = ?1",
                params![seq, payload(m)],
            )?;
        }
        None => {
            tx.execute(
                "INSERT INTO outbox (local_id, op, payload, not_before) VALUES (?1, ?2, ?3, ?4)",
                params![key.0, OpKind::Read.as_str(), payload(m), now_ms()],
            )?;
        }
    }
    Ok(())
}

/// Queue the unread op for one row (at most one waits), inside the caller's
/// transaction. A waiting read op for the row is dropped: last action wins.
pub(crate) fn queue_unread(tx: &rusqlite::Connection, m: &UnreadMark) -> rusqlite::Result<()> {
    let key = read_key(&m.sub, &m.remote_id);
    drop_waiting_read(tx, &key)?;
    let waiting: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM outbox WHERE local_id = ?1 AND op = 'unread' AND in_flight = 0)",
        [&key.0],
        |r| r.get(0),
    )?;
    if !waiting {
        let payload = serde_json::to_string(m).unwrap_or_else(|_| "{}".into());
        tx.execute(
            "INSERT INTO outbox (local_id, op, payload, not_before) VALUES (?1, ?2, ?3, ?4)",
            params![key.0, OpKind::Unread.as_str(), payload, now_ms()],
        )?;
    }
    Ok(())
}

/// Forget a row's waiting read op (it was marked unread since).
pub(crate) fn drop_waiting_read(tx: &rusqlite::Connection, key: &LocalId) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM outbox WHERE local_id = ?1 AND op = 'read' AND in_flight = 0",
        [&key.0],
    )?;
    Ok(())
}

/// What a pull found for one row, against what's held here
/// (`Store::reconcile_read_state`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReadPlan {
    /// Rows read here that the server doesn't have yet: queued as reads.
    pub reads: Vec<ReadMark>,
    /// Rows marked unread here that the server still holds read: queued
    /// as unreads (only on a server that can clear read state).
    pub unreads: Vec<UnreadMark>,
    /// Rows whose read state the server cleared (another device marked
    /// them unread): unread here now.
    pub cleared: usize,
}

/// A row's read state as held: (sub, remote id, read version, read_at, floor).
type HeldRead = (String, String, Option<u32>, Option<i64>, Option<u32>);

impl Store {
    /// Queue read ops for these rows (coalescing per row).
    pub fn queue_reads(&self, marks: &[ReadMark]) -> Result<()> {
        if marks.is_empty() {
            return Ok(());
        }
        let mut c = self.conn();
        let tx = c.transaction()?;
        for m in marks {
            queue_read(&tx, m)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Queue unread ops for these rows (one per row).
    pub fn queue_unreads(&self, marks: &[UnreadMark]) -> Result<()> {
        if marks.is_empty() {
            return Ok(());
        }
        let mut c = self.conn();
        let tx = c.transaction()?;
        for m in marks {
            queue_unread(&tx, m)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Forget every waiting read and unread op (the server stopped syncing
    /// read state).
    pub fn drop_read_ops(&self) -> Result<()> {
        self.conn().execute(
            "DELETE FROM outbox WHERE op IN ('read', 'unread') AND in_flight = 0",
            [],
        )?;
        Ok(())
    }

    /// Forget every waiting unread op (the server can't clear read state:
    /// unread stays on this Mac, held by each row's floor).
    pub fn drop_unread_ops(&self) -> Result<()> {
        self.conn().execute(
            "DELETE FROM outbox WHERE op = 'unread' AND in_flight = 0",
            [],
        )?;
        Ok(())
    }

    /// Rows with a read or unread op in the outbox (waiting or on the wire).
    pub fn pending_read_rows(&self) -> Result<HashSet<(String, String)>> {
        let c = self.conn();
        let mut st = c.prepare("SELECT local_id FROM outbox WHERE op IN ('read', 'unread')")?;
        let v = st
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .filter_map(|k| row_of_key(&k))
            .collect();
        Ok(v)
    }

    /// Every row with read state, for the one-time upload. `read_at` is
    /// filled in (`with_read_at`) for a server that can clear read state.
    pub fn read_marks(&self, with_read_at: bool) -> Result<Vec<ReadMark>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT subscription_id, remote_id, read_version, read_at FROM reading \
             WHERE read_version IS NOT NULL ORDER BY subscription_id, remote_id",
        )?;
        let v = st
            .query_map([], |r| {
                let at: Option<i64> = r.get(3)?;
                Ok(ReadMark {
                    sub: r.get(0)?,
                    remote_id: r.get(1)?,
                    version: r.get::<_, i64>(2)? as u32,
                    read_at: with_read_at.then(|| read_at_iso(at)),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Rows held here whose read state is ahead of what the server just
    /// reported for them (`server`: `(sub, remote_id)` → its `read_version`).
    /// Rows the server didn't report are left out.
    pub fn reads_ahead_of(
        &self,
        server: &HashMap<(String, String), Option<u32>>,
    ) -> Result<Vec<ReadMark>> {
        let ahead = self
            .read_marks(false)?
            .into_iter()
            .filter(|m| {
                server
                    .get(&(m.sub.clone(), m.remote_id.clone()))
                    .is_some_and(|s| Some(m.version) > *s)
            })
            .collect();
        Ok(ahead)
    }

    /// After a pull from a server that syncs read state, settle each
    /// reported row against what's held here, and queue what the server
    /// still needs. `server`: `(sub, remote_id)` → its `read_version` (the
    /// merge has already raised local state to it where it was higher and
    /// not below the row's unread floor). `clear`: the server can clear
    /// read state. Rows with an op in the outbox are left to that op.
    ///
    /// - Same as here: confirmed (`read_at` → NULL).
    /// - Read here, not (or at a lower version) there: when the server
    ///   confirmed this read before (`read_at` NULL) and can clear read
    ///   state, another device cleared it: unread here too. Otherwise the
    ///   read is queued (with its `read_at` when `clear`).
    /// - Marked unread here, still read there (at or below the floor):
    ///   queued as an unread when `clear`; otherwise it stays on this Mac.
    pub fn reconcile_read_state(
        &self,
        server: &HashMap<(String, String), Option<u32>>,
        clear: bool,
    ) -> Result<ReadPlan> {
        let pending = self.pending_read_rows()?;
        let mut plan = ReadPlan::default();
        let mut c = self.conn();
        let tx = c.transaction()?;
        let rows: Vec<HeldRead> = {
            let mut st = tx.prepare(
                "SELECT subscription_id, remote_id, read_version, read_at, read_floor FROM reading",
            )?;
            st.query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, Option<i64>>(2)?.map(|v| v as u32),
                    r.get(3)?,
                    r.get::<_, Option<i64>>(4)?.map(|v| v as u32),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (sub, rid, local, at, floor) in rows {
            let key = (sub.clone(), rid.clone());
            let Some(&theirs) = server.get(&key) else {
                continue;
            };
            if pending.contains(&key) {
                continue;
            }
            match (local, theirs) {
                (l, s) if l == s => {
                    if at.is_some() {
                        tx.execute(
                            "UPDATE reading SET read_at = NULL WHERE subscription_id = ?1 AND remote_id = ?2",
                            params![sub, rid],
                        )?;
                    }
                }
                (Some(l), s) if Some(l) > s => {
                    if clear && at.is_none() {
                        tx.execute(
                            "UPDATE reading SET read_version = ?3, read_floor = NULL \
                             WHERE subscription_id = ?1 AND remote_id = ?2",
                            params![sub, rid, s],
                        )?;
                        plan.cleared += 1;
                    } else {
                        let m = ReadMark {
                            sub,
                            remote_id: rid,
                            version: l,
                            read_at: clear.then(|| read_at_iso(at)),
                        };
                        queue_read(&tx, &m)?;
                        plan.reads.push(m);
                    }
                }
                (None, Some(s)) if clear && floor.is_some_and(|f| s <= f) => {
                    let m = UnreadMark {
                        sub,
                        remote_id: rid,
                    };
                    queue_unread(&tx, &m)?;
                    plan.unreads.push(m);
                }
                _ => {}
            }
        }
        tx.commit()?;
        Ok(plan)
    }

    /// The server took a read (`PUT` answered): settle the row. `stored`
    /// with `held` = the version it holds now confirms it; on a server that
    /// can clear read state, a read it refused (older than a later "mark
    /// unread") takes the server's state instead. Only while no newer op
    /// for the row waits. True if the row's read state changed.
    pub fn read_acked(
        &self,
        m: &ReadMark,
        stored: bool,
        held: Option<u32>,
        clear: bool,
    ) -> Result<bool> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let key = read_key(&m.sub, &m.remote_id);
        let newer: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM outbox WHERE local_id = ?1 AND in_flight = 0)",
            [&key.0],
            |r| r.get(0),
        )?;
        let mut changed = false;
        if !newer {
            if stored {
                tx.execute(
                    "UPDATE reading SET read_at = NULL WHERE subscription_id = ?1 AND remote_id = ?2 \
                     AND read_version IS NOT NULL AND read_version <= ?3",
                    params![m.sub, m.remote_id, held.unwrap_or(m.version)],
                )?;
            } else if clear {
                changed = tx.execute(
                    "UPDATE reading SET read_version = ?3, read_at = NULL, read_floor = NULL \
                     WHERE subscription_id = ?1 AND remote_id = ?2 AND read_version IS ?4",
                    params![m.sub, m.remote_id, held, m.version],
                )? > 0;
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// The server cleared these rows' read state (the unread op went
    /// through): their floors go, so a later read on any device counts.
    pub fn unread_acked(&self, marks: &[UnreadMark]) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        for m in marks {
            tx.execute(
                "UPDATE reading SET read_floor = NULL WHERE subscription_id = ?1 AND remote_id = ?2 \
                 AND read_version IS NULL",
                params![m.sub, m.remote_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
