//! Read-state sync bookkeeping (owner-API extension 5, docs/SERVER.md).
//!
//! A `read` op in the outbox says "tell the server this row was read up to
//! `version`". Its `local_id` is a key derived from `(sub, remote_id)` (never
//! an item's id), so ops for different rows never block each other, and the
//! payload holds the `ReadMark`. There is at most one waiting op per row:
//! queueing again raises its version instead of appending (an op already on
//! the wire is left alone, and the new one waits behind it).

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};

use super::{OpKind, Store};
use crate::api::wire::ReadMark;
use crate::backend::Result;
use crate::model::LocalId;
use crate::util::now_ms;

/// Outbox key for a reading row's read op.
pub(crate) fn read_key(sub: &str, remote_id: &str) -> LocalId {
    LocalId(format!("read\u{1f}{sub}\u{1f}{remote_id}"))
}

/// Queue (or raise) the read op for one row, inside the caller's transaction.
pub(crate) fn queue_read(tx: &rusqlite::Connection, m: &ReadMark) -> rusqlite::Result<()> {
    let key = read_key(&m.sub, &m.remote_id);
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
            if old.as_ref().is_some_and(|o| o.version >= m.version) {
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

    /// Forget every waiting read op (the server stopped syncing read state).
    pub fn drop_read_ops(&self) -> Result<()> {
        self.conn()
            .execute("DELETE FROM outbox WHERE op = 'read' AND in_flight = 0", [])?;
        Ok(())
    }

    /// Every row with read state, for the one-time upload.
    pub fn read_marks(&self) -> Result<Vec<ReadMark>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT subscription_id, remote_id, read_version FROM reading \
             WHERE read_version IS NOT NULL ORDER BY subscription_id, remote_id",
        )?;
        let v = st
            .query_map([], |r| {
                Ok(ReadMark {
                    sub: r.get(0)?,
                    remote_id: r.get(1)?,
                    version: r.get::<_, i64>(2)? as u32,
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
            .read_marks()?
            .into_iter()
            .filter(|m| {
                server
                    .get(&(m.sub.clone(), m.remote_id.clone()))
                    .is_some_and(|s| Some(m.version) > *s)
            })
            .collect();
        Ok(ahead)
    }
}
