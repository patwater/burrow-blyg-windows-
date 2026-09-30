//! Local SQLite store (WAL, FTS5 trigram). The UI reads only from here.
//!
//! One connection behind a mutex: every method is a short transaction, and no
//! method ever holds the lock across network I/O.
//!
//! The outbox is an ordered op log (`create`, `save`, `recreate`, plus
//! `delete_remote` to retire a recreated draft, and `read` for read-state
//! sync, see `read_sync`). Edit ops carry no
//! content: the content pushed is the item's working copy *at push time*, which
//! is what makes coalescing trivial (50 keystrokes = one pending `save` whose
//! `not_before` keeps moving).

mod provenance;
pub use provenance::Tracked;
mod read_sync;
mod reading;
mod remote;
mod schema;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use crate::api::wire::{WireItem, kind_str, parse_kind, parse_status, status_str};
use crate::backend::{CoreError, Result};
use crate::model::*;
use crate::util::{new_local_id, now_iso, now_ms};

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Storage(e.to_string())
    }
}

pub struct Store {
    conn: Mutex<Connection>,
    /// `items()` snapshot, stamped with the connection's `total_changes()`
    /// when it was built: any INSERT/UPDATE/DELETE since invalidates it.
    items_cache: Mutex<Option<(u64, Arc<Vec<Item>>)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Create,
    Save,
    /// Pre-publish kind change of a draft that already exists server-side.
    Recreate,
    /// Delete a retired server draft (left behind by `Recreate`); payload = server id.
    DeleteRemote,
    /// Send a reading row's read state (extension 5); payload = a `ReadMark`,
    /// `local_id` = `read_sync::read_key`, not an item.
    Read,
}

impl OpKind {
    fn as_str(self) -> &'static str {
        match self {
            OpKind::Create => "create",
            OpKind::Save => "save",
            OpKind::Recreate => "recreate",
            OpKind::DeleteRemote => "delete_remote",
            OpKind::Read => "read",
        }
    }
    fn parse(s: &str) -> OpKind {
        match s {
            "create" => OpKind::Create,
            "recreate" => OpKind::Recreate,
            "delete_remote" => OpKind::DeleteRemote,
            "read" => OpKind::Read,
            _ => OpKind::Save,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Op {
    pub seq: i64,
    pub local_id: LocalId,
    pub kind: OpKind,
    pub not_before: i64,
    pub in_flight: bool,
    pub payload: String,
}

/// An item plus the sync bookkeeping the worker needs.
#[derive(Debug, Clone)]
pub struct SyncRow {
    pub item: Item,
    pub server_kind: Option<Kind>,
    pub base_content: Option<String>,
    pub theirs: Option<String>,
}

/// (local_id, mine, theirs)
pub type ConflictInfo = (LocalId, String, String);

#[derive(Debug, Default)]
pub struct MergeOutcome {
    pub changed: bool,
    /// (local_id, mine, theirs) for items that just entered conflict.
    pub conflicts: Vec<ConflictInfo>,
}

const ITEM_COLS: &str = "local_id, server_id, kind, status, version, dirty, content_md, created, updated, \
     permalink, stub_of, forked_from, show_responses, conflict, \
     EXISTS(SELECT 1 FROM outbox o WHERE o.local_id = items.local_id AND o.op <> 'delete_remote'), \
     server_kind, base_content, theirs_content";

fn row_to_sync(r: &Row) -> rusqlite::Result<SyncRow> {
    let json_ref = |s: Option<String>| s.and_then(|s| serde_json::from_str::<RemoteRef>(&s).ok());
    let item = Item {
        local_id: LocalId(r.get(0)?),
        server_id: r.get::<_, Option<String>>(1)?.map(ServerId),
        kind: parse_kind(&r.get::<_, String>(2)?),
        status: parse_status(&r.get::<_, String>(3)?),
        version: r.get::<_, i64>(4)? as u32,
        dirty: r.get(5)?,
        content_md: r.get(6)?,
        created: r.get(7)?,
        updated: r.get(8)?,
        permalink: r.get(9)?,
        stub_of: json_ref(r.get(10)?),
        forked_from: json_ref(r.get(11)?),
        show_responses: r.get(12)?,
        conflict: r.get(13)?,
        pending_sync: r.get(14)?,
    };
    Ok(SyncRow {
        item,
        server_kind: r.get::<_, Option<String>>(15)?.map(|k| parse_kind(&k)),
        base_content: r.get(16)?,
        theirs: r.get(17)?,
    })
}

fn row_to_item(r: &Row) -> rusqlite::Result<Item> {
    row_to_sync(r).map(|s| s.item)
}

fn get_row(tx: &Connection, id: &LocalId) -> rusqlite::Result<Option<SyncRow>> {
    tx.prepare_cached(&format!(
        "SELECT {ITEM_COLS} FROM items WHERE local_id = ?1"
    ))?
    .query_row([&id.0], row_to_sync)
    .optional()
}

fn has_ops(tx: &Connection, id: &LocalId) -> rusqlite::Result<bool> {
    tx.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM outbox WHERE local_id = ?1 AND op <> 'delete_remote')",
    )?
    .query_row([&id.0], |r| r.get(0))
}

fn raw_json(v: &Option<serde_json::Value>) -> Option<String> {
    v.as_ref().filter(|v| !v.is_null()).map(|v| v.to_string())
}

fn max_str(a: &str, b: &str) -> String {
    if a >= b { a.to_string() } else { b.to_string() }
}

impl Store {
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| CoreError::Storage(e.to_string()))?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Store> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // Purged content (withdrawn posts, dropped snapshots) is overwritten
        // on disk, not just unlinked: "local hoarding past withdrawal is
        // nonconforming" (spec §13.4).
        conn.pragma_update(None, "secure_delete", "ON")?;
        if schema::migrate(&mut conn)? {
            // Don't leave pre-migration pages sitting in the WAL.
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        }
        // Anything marked in flight belonged to a previous process that died mid-push.
        conn.execute("UPDATE outbox SET in_flight = 0", [])?;
        Ok(Store {
            items_cache: Mutex::new(None),
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn schema_version(&self) -> Result<usize> {
        let v: i64 = self
            .conn()
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        debug_assert!(v as usize <= schema::latest());
        Ok(v as usize)
    }

    // ---------------------------------------------------------------- reads

    pub fn items(&self) -> Vec<Item> {
        self.try_items().unwrap_or_default()
    }

    fn try_items(&self) -> Result<Vec<Item>> {
        Ok(self.items_snapshot()?.to_vec())
    }

    fn items_snapshot(&self) -> Result<Arc<Vec<Item>>> {
        let c = self.conn();
        let stamp = c.total_changes();
        let mut cache = self.items_cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((s, v)) = cache.as_ref()
            && *s == stamp
        {
            return Ok(v.clone());
        }
        let mut st = c.prepare_cached(&format!(
            "SELECT {ITEM_COLS} FROM items ORDER BY updated DESC, rid DESC"
        ))?;
        let v = Arc::new(
            st.query_map([], row_to_item)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
        *cache = Some((stamp, v.clone()));
        Ok(v)
    }

    /// Case-insensitive substring match over content, newest-updated first.
    /// ≥ 3 chars uses the FTS5 trigram index; shorter queries (which trigrams
    /// can't index) scan in Rust.
    pub fn search(&self, query: &str) -> Vec<Item> {
        let q = query.trim();
        if q.is_empty() {
            return self.items();
        }
        if q.chars().count() < 3 {
            let needle = q.to_lowercase();
            let all = self.items_snapshot().unwrap_or_default();
            return all
                .iter()
                .filter(|i| i.content_md.to_lowercase().contains(&needle))
                .cloned()
                .collect();
        }
        self.try_fts(q).unwrap_or_default()
    }

    fn try_fts(&self, q: &str) -> Result<Vec<Item>> {
        let phrase = format!("\"{}\"", q.replace('"', "\"\""));
        let c = self.conn();
        let mut st = c.prepare_cached(&format!(
            "SELECT {ITEM_COLS} FROM items WHERE rid IN \
             (SELECT rowid FROM items_fts WHERE items_fts MATCH ?1) ORDER BY updated DESC, rid DESC"
        ))?;
        let v = st
            .query_map([phrase], row_to_item)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn item(&self, id: &LocalId) -> Option<Item> {
        self.row(id).map(|r| r.item)
    }

    pub fn row(&self, id: &LocalId) -> Option<SyncRow> {
        get_row(&self.conn(), id).ok().flatten()
    }

    pub fn local_id_for_server(&self, sid: &str) -> Option<LocalId> {
        self.conn()
            .query_row(
                "SELECT local_id FROM items WHERE server_id = ?1",
                [sid],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .map(LocalId)
    }

    // ------------------------------------------------------- local mutations

    pub fn create_draft(&self, kind: Kind, content: &str, debounce_ms: i64) -> Result<LocalId> {
        let id = new_local_id();
        let now = now_iso();
        let mut c = self.conn();
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO items (local_id, kind, status, version, dirty, content_md, created, updated) \
             VALUES (?1, ?2, 'draft', 0, 1, ?3, ?4, ?4)",
            params![id.0, kind_str(kind), content, now],
        )?;
        enqueue(&tx, &id, OpKind::Create, now_ms() + debounce_ms)?;
        tx.commit()?;
        Ok(id)
    }

    /// A local-only scratch note: no outbox op, now or ever (until `promote_scratch`).
    pub fn create_scratch(&self, kind: Kind, content: &str) -> Result<LocalId> {
        let id = new_local_id();
        let now = now_iso();
        self.conn().execute(
            "INSERT INTO items (local_id, kind, status, version, dirty, content_md, created, updated) \
             VALUES (?1, ?2, 'scratch', 0, 1, ?3, ?4, ?4)",
            params![id.0, kind_str(kind), content, now],
        )?;
        Ok(id)
    }

    /// Scratch → draft with `kind`, keeping the local id, and queue its
    /// `create` (due now). `false` when the item isn't scratch (nothing done).
    pub fn promote_scratch(&self, id: &LocalId, kind: Kind) -> Result<bool> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let row = get_row(&tx, id)?.ok_or(CoreError::NotFound)?;
        if row.item.status != Status::Scratch {
            return Ok(false);
        }
        tx.execute(
            "UPDATE items SET status = 'draft', kind = ?2, updated = ?3 WHERE local_id = ?1",
            params![id.0, kind_str(kind), max_str(&now_iso(), &row.item.updated)],
        )?;
        enqueue(&tx, id, OpKind::Create, now_ms())?;
        tx.commit()?;
        Ok(true)
    }

    /// Swap uploaded local images into the working copy (`blyg-local:<name>`
    /// → its blyg URL, `crate::scratch_media::rewrite`). Queues nothing: it
    /// runs just before the text is pushed (promotion, or the outbox op that
    /// is about to send it). `false` when nothing changed.
    pub(crate) fn rewrite_media(
        &self,
        id: &LocalId,
        uploaded: &[(String, String)],
    ) -> Result<bool> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let row = get_row(&tx, id)?.ok_or(CoreError::NotFound)?;
        let new = crate::scratch_media::rewrite(&row.item.content_md, uploaded);
        if new == row.item.content_md {
            return Ok(false);
        }
        provenance::follow_tx(&tx, id, &new)?;
        tx.execute(
            "UPDATE items SET content_md = ?2, updated = ?3 WHERE local_id = ?1",
            params![id.0, new, max_str(&now_iso(), &row.item.updated)],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// A draft thread replying to `of` (`stub_of`); the outbox's create op
    /// sends the item's `stub_of` with `POST /api/items`.
    pub fn create_stub(&self, of: &RemoteRef, content: &str, debounce_ms: i64) -> Result<LocalId> {
        let id = new_local_id();
        let now = now_iso();
        let stub = serde_json::to_string(of).map_err(|e| CoreError::Storage(e.to_string()))?;
        let mut c = self.conn();
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO items (local_id, kind, status, version, dirty, content_md, created, updated, stub_of) \
             VALUES (?1, 'thread', 'draft', 0, 1, ?2, ?3, ?3, ?4)",
            params![id.0, content, now, stub],
        )?;
        enqueue(&tx, &id, OpKind::Create, now_ms() + debounce_ms)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn save(&self, id: &LocalId, content: &str, debounce_ms: i64) -> Result<()> {
        self.save_inner(id, content, None, debounce_ms)
    }

    /// `save`, optionally replacing the tracked TK provenance (already
    /// validated against `content`). Without it, tracked provenance follows
    /// its scopes (`tk::remap`).
    pub(crate) fn save_inner(
        &self,
        id: &LocalId,
        content: &str,
        provenance: Option<&[Option<ScopeProvenance>]>,
        debounce_ms: i64,
    ) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let row = get_row(&tx, id)?.ok_or(CoreError::NotFound)?;
        match provenance {
            Some(p) => provenance::set_tx(&tx, id, content, p)?,
            None if row.item.content_md == content => return Ok(()),
            None => provenance::follow_tx(&tx, id, content)?,
        }
        if row.item.status == Status::Scratch {
            // Local-only: the text changes here and goes nowhere.
            if row.item.content_md != content {
                tx.execute(
                    "UPDATE items SET content_md = ?2, updated = ?3 WHERE local_id = ?1",
                    params![id.0, content, max_str(&now_iso(), &row.item.updated)],
                )?;
            }
            tx.commit()?;
            return Ok(());
        }
        if row.item.content_md == content {
            // Provenance only: still needs a push.
            let due = now_ms() + debounce_ms;
            if !has_ops(&tx, id)? {
                let op = if row.item.server_id.is_some() {
                    OpKind::Save
                } else {
                    OpKind::Create
                };
                enqueue(&tx, id, op, due)?;
            }
            tx.commit()?;
            return Ok(());
        }
        tx.execute(
            "UPDATE items SET content_md = ?2, dirty = 1, updated = ?3 WHERE local_id = ?1",
            params![id.0, content, max_str(&now_iso(), &row.item.updated)],
        )?;
        let due = now_ms() + debounce_ms;
        // Coalesce into the newest not-yet-in-flight op for this item: all of
        // them push the working copy as it is when they run.
        let pending: Option<i64> = tx
            .query_row(
                "SELECT seq FROM outbox WHERE local_id = ?1 AND in_flight = 0 AND op <> 'delete_remote' ORDER BY seq DESC LIMIT 1",
                [&id.0],
                |r| r.get(0),
            )
            .optional()?;
        match pending {
            Some(seq) => {
                tx.execute(
                    "UPDATE outbox SET not_before = ?2 WHERE seq = ?1",
                    params![seq, due],
                )?;
            }
            None if row.item.server_id.is_none() && !has_ops(&tx, id)? => {
                // Never reached the server and nothing queued (e.g. lost server copy).
                enqueue(&tx, id, OpKind::Create, due)?;
            }
            None => enqueue(&tx, id, OpKind::Save, due)?,
        }
        tx.commit()?;
        Ok(())
    }

    /// Pre-publish fragment ⇄ thread.
    pub fn set_kind(&self, id: &LocalId, kind: Kind, debounce_ms: i64) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let row = get_row(&tx, id)?.ok_or(CoreError::NotFound)?;
        if row.item.kind == kind {
            return Ok(());
        }
        if row.item.status == Status::Scratch {
            tx.execute(
                "UPDATE items SET kind = ?2, updated = ?3 WHERE local_id = ?1",
                params![id.0, kind_str(kind), max_str(&now_iso(), &row.item.updated)],
            )?;
            tx.commit()?;
            return Ok(());
        }
        if row.item.version > 0 || row.item.status != Status::Draft {
            return Err(CoreError::Rejected {
                status: 409,
                message: "a published item's kind can't be changed".into(),
                details: vec![],
            });
        }
        tx.execute(
            "UPDATE items SET kind = ?2, updated = ?3 WHERE local_id = ?1",
            params![id.0, kind_str(kind), max_str(&now_iso(), &row.item.updated)],
        )?;
        let ops = ops_for(&tx, id)?;
        let pending_create = ops.iter().any(|o| o.kind == OpKind::Create && !o.in_flight);
        if !pending_create {
            // A recreate copies the current working copy, so pending saves are redundant.
            tx.execute(
                "DELETE FROM outbox WHERE local_id = ?1 AND op = 'save' AND in_flight = 0",
                [&id.0],
            )?;
            let due = now_ms() + debounce_ms;
            match ops
                .iter()
                .find(|o| o.kind == OpKind::Recreate && !o.in_flight)
            {
                Some(o) => {
                    tx.execute(
                        "UPDATE outbox SET not_before = ?2 WHERE seq = ?1",
                        params![o.seq, due],
                    )?;
                }
                None => enqueue(&tx, id, OpKind::Recreate, due)?,
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Insert an item that already exists server-side (fork, KeepBoth never uses this).
    pub fn insert_from_server(&self, w: &WireItem) -> Result<LocalId> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let id = insert_wire(&tx, w)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn delete_item(&self, id: &LocalId) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM outbox WHERE local_id = ?1 AND op <> 'delete_remote'",
            [&id.0],
        )?;
        tx.execute("DELETE FROM versions WHERE local_id = ?1", [&id.0])?;
        tx.execute("DELETE FROM items WHERE local_id = ?1", [&id.0])?;
        tx.commit()?;
        Ok(())
    }

    // ---------------------------------------------------------------- outbox

    pub fn ops(&self) -> Result<Vec<Op>> {
        let c = self.conn();
        let mut st = c.prepare_cached(
            "SELECT seq, local_id, op, not_before, in_flight, payload FROM outbox ORDER BY seq",
        )?;
        let v = st
            .query_map([], op_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn conflicted(&self) -> Result<HashSet<LocalId>> {
        let c = self.conn();
        let mut st = c.prepare_cached("SELECT local_id FROM items WHERE conflict = 1")?;
        let v = st
            .query_map([], |r| r.get::<_, String>(0).map(LocalId))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(v)
    }

    pub fn pending_count(&self) -> usize {
        self.conn()
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// Earliest `not_before` among ops whose item isn't in conflict.
    pub fn earliest_due(&self) -> Option<i64> {
        self.conn()
            .query_row(
                "SELECT MIN(o.not_before) FROM outbox o LEFT JOIN items i ON i.local_id = o.local_id \
                 WHERE COALESCE(i.conflict, 0) = 0",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
    }

    pub fn set_in_flight(&self, seq: i64, flag: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE outbox SET in_flight = ?2, attempts = attempts + ?2 WHERE seq = ?1",
            params![seq, flag as i64],
        )?;
        Ok(())
    }

    pub fn drop_op(&self, seq: i64) -> Result<()> {
        self.conn()
            .execute("DELETE FROM outbox WHERE seq = ?1", [seq])?;
        Ok(())
    }

    /// `create` or `recreate` succeeded: `sid` now holds `sent` with `kind`.
    pub fn op_created(
        &self,
        seq: i64,
        id: &LocalId,
        sid: &str,
        sent: &str,
        kind: Kind,
    ) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE items SET server_id = ?2, base_content = ?3, server_kind = ?4 WHERE local_id = ?1",
            params![id.0, sid, sent, kind_str(kind)],
        )?;
        tx.execute("DELETE FROM outbox WHERE seq = ?1", [seq])?;
        tx.commit()?;
        Ok(())
    }

    /// `recreate` succeeded: the item now lives at `sid`; queue deletion of `old`.
    pub fn op_recreated(
        &self,
        seq: i64,
        id: &LocalId,
        sid: &str,
        old: &str,
        sent: &str,
        kind: Kind,
    ) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE items SET server_id = ?2, base_content = ?3, server_kind = ?4 WHERE local_id = ?1",
            params![id.0, sid, sent, kind_str(kind)],
        )?;
        tx.execute("DELETE FROM outbox WHERE seq = ?1", [seq])?;
        tx.execute(
            "INSERT INTO outbox (local_id, op, payload, not_before) VALUES (?1, 'delete_remote', ?2, 0)",
            params![id.0, old],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn op_saved(&self, seq: i64, id: &LocalId, sent: &str) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE items SET base_content = ?2 WHERE local_id = ?1",
            params![id.0, sent],
        )?;
        tx.execute("DELETE FROM outbox WHERE seq = ?1", [seq])?;
        tx.commit()?;
        Ok(())
    }

    /// The server no longer has this item (404 on PUT): it becomes a local-only
    /// draft again and the op turns into a `create`.
    pub fn op_lost_server(&self, seq: i64, id: &LocalId) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE items SET server_id = NULL, server_kind = NULL, base_content = NULL, base_updated = NULL, \
             status = 'draft', version = 0, permalink = NULL WHERE local_id = ?1",
            [&id.0],
        )?;
        tx.execute(
            "UPDATE outbox SET op = 'create', in_flight = 0 WHERE seq = ?1",
            [seq],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Make sure a push is queued for `id` (KeepMine).
    fn ensure_push(tx: &Transaction, id: &LocalId, server_id: bool) -> rusqlite::Result<()> {
        if !has_ops(tx, id)? {
            let op = if server_id {
                OpKind::Save
            } else {
                OpKind::Create
            };
            enqueue(tx, id, op, now_ms())?;
        }
        Ok(())
    }

    // ------------------------------------------------------ server → local

    /// Merge a full `GET /api/items` listing. Applies the conflict rule and
    /// removes local copies of items the server no longer has (unless they
    /// carry unpushed edits, which will recreate them).
    pub fn merge_all(&self, wires: &[WireItem]) -> Result<MergeOutcome> {
        let mut out = MergeOutcome::default();
        let mut c = self.conn();
        let tx = c.transaction()?;
        let mut by_sid: HashMap<String, SyncRow> = HashMap::new();
        {
            let mut st = tx.prepare(&format!(
                "SELECT {ITEM_COLS} FROM items WHERE server_id IS NOT NULL"
            ))?;
            for r in st.query_map([], row_to_sync)? {
                let r = r?;
                if let Some(sid) = &r.item.server_id {
                    by_sid.insert(sid.0.clone(), r);
                }
            }
        }
        let retiring: HashSet<String> = {
            let mut st = tx.prepare("SELECT payload FROM outbox WHERE op = 'delete_remote'")?;
            st.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut seen = HashSet::new();
        for w in wires {
            if retiring.contains(&w.id) {
                continue;
            }
            seen.insert(w.id.clone());
            match by_sid.get(&w.id) {
                None => {
                    insert_wire(&tx, w)?;
                    out.changed = true;
                }
                Some(row) => {
                    let (changed, conflict) = merge_one(&tx, row, w, true)?;
                    out.changed |= changed;
                    if let Some(c) = conflict {
                        out.conflicts.push(c);
                    }
                }
            }
        }
        for (sid, row) in &by_sid {
            if !seen.contains(sid) && !row.item.pending_sync && !row.item.conflict {
                tx.execute(
                    "DELETE FROM versions WHERE local_id = ?1",
                    [&row.item.local_id.0],
                )?;
                tx.execute(
                    "DELETE FROM items WHERE local_id = ?1",
                    [&row.item.local_id.0],
                )?;
                out.changed = true;
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Merge one server copy *with* conflict detection (pre-push check).
    pub fn check_server(&self, id: &LocalId, w: &WireItem) -> Result<Option<ConflictInfo>> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let Some(row) = get_row(&tx, id)? else {
            return Ok(None);
        };
        let (_, conflict) = merge_one(&tx, &row, w, true)?;
        tx.commit()?;
        Ok(conflict)
    }

    /// Refresh one item from the server without conflict detection (after
    /// publish, withdraw, …). Local unpushed content is kept.
    pub fn apply_server(&self, id: &LocalId, w: &WireItem) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        if let Some(row) = get_row(&tx, id)? {
            merge_one(&tx, &row, w, false)?;
            if let Some(v) = &w.versions {
                put_versions(&tx, id, v)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Server copy wins outright: drop local edits and conflict (restore).
    pub fn replace_from_server(&self, id: &LocalId, w: &WireItem) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        tx.execute(
            "DELETE FROM outbox WHERE local_id = ?1 AND op <> 'delete_remote'",
            [&id.0],
        )?;
        tx.execute(
            "UPDATE items SET conflict = 0, theirs_content = NULL WHERE local_id = ?1",
            [&id.0],
        )?;
        // A restore clears the server's provenance cache too.
        provenance::forget_tx(&tx, id)?;
        if let Some(row) = get_row(&tx, id)? {
            merge_one(&tx, &row, w, false)?;
            if let Some(v) = &w.versions {
                put_versions(&tx, id, v)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn after_publish(
        &self,
        id: &LocalId,
        version: u32,
        permalink: &str,
        published: &str,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE items SET status = 'public', version = ?2, permalink = ?3, base_content = ?4, \
             dirty = (content_md <> ?4) WHERE local_id = ?1",
            params![id.0, version, permalink, published],
        )?;
        Ok(())
    }

    pub fn set_status_version(&self, id: &LocalId, status: Status, version: u32) -> Result<()> {
        self.conn().execute(
            "UPDATE items SET status = ?2, version = ?3 WHERE local_id = ?1",
            params![id.0, status_str(status), version],
        )?;
        Ok(())
    }

    pub fn set_show_responses(&self, id: &LocalId, show: bool) -> Result<()> {
        self.conn().execute(
            "UPDATE items SET show_responses = ?2 WHERE local_id = ?1",
            params![id.0, show],
        )?;
        Ok(())
    }

    // -------------------------------------------------------------- conflicts

    /// Returns the new draft's id for `KeepBoth`.
    pub fn resolve(&self, id: &LocalId, how: Resolution) -> Result<Option<LocalId>> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let row = get_row(&tx, id)?.ok_or(CoreError::NotFound)?;
        if !row.item.conflict {
            return Err(CoreError::Other("no conflict to resolve".into()));
        }
        let theirs = row.theirs.clone().unwrap_or_default();
        let mut new_id = None;
        match how {
            Resolution::KeepMine => {
                // The server's copy becomes the base, so the next pull sees no
                // divergence; the queued push then overwrites it with mine.
                tx.execute(
                    "UPDATE items SET conflict = 0, theirs_content = NULL, base_content = ?2 WHERE local_id = ?1",
                    params![id.0, theirs],
                )?;
                Self::ensure_push(&tx, id, row.item.server_id.is_some())?;
                // The server's provenance was keyed to their text.
                provenance::mark_dirty_tx(&tx, id)?;
                tx.execute(
                    "UPDATE outbox SET not_before = ?2 WHERE local_id = ?1 AND op <> 'delete_remote'",
                    params![id.0, now_ms()],
                )?;
            }
            Resolution::TakeServer | Resolution::KeepBoth => {
                if how == Resolution::KeepBoth {
                    let nid = new_local_id();
                    let now = now_iso();
                    tx.execute(
                        "INSERT INTO items (local_id, kind, status, version, dirty, content_md, created, updated) \
                         VALUES (?1, ?2, 'draft', 0, 1, ?3, ?4, ?4)",
                        params![nid.0, kind_str(row.item.kind), row.item.content_md, now],
                    )?;
                    enqueue(&tx, &nid, OpKind::Create, now_ms())?;
                    new_id = Some(nid);
                }
                tx.execute(
                    "DELETE FROM outbox WHERE local_id = ?1 AND op <> 'delete_remote'",
                    [&id.0],
                )?;
                provenance::forget_tx(&tx, id)?;
                tx.execute(
                    "UPDATE items SET conflict = 0, theirs_content = NULL, content_md = ?2, base_content = ?2, \
                     kind = COALESCE(server_kind, kind) WHERE local_id = ?1",
                    params![id.0, theirs],
                )?;
            }
        }
        tx.commit()?;
        Ok(new_id)
    }

    // --------------------------------------------------------------- versions

    pub fn put_versions(&self, id: &LocalId, v: &[Version]) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        put_versions(&tx, id, v)?;
        tx.commit()?;
        Ok(())
    }

    pub fn versions(&self, id: &LocalId) -> Vec<Version> {
        let c = self.conn();
        let Ok(mut st) = c.prepare_cached(
            "SELECT version, published_at, note, pinned, endcap FROM versions WHERE local_id = ?1 ORDER BY version",
        ) else {
            return vec![];
        };
        st.query_map([&id.0], |r| {
            Ok(Version {
                version: r.get::<_, i64>(0)? as u32,
                published_at: r.get(1)?,
                note: r.get(2)?,
                pinned: r.get(3)?,
                endcap: r.get(4)?,
            })
        })
        .and_then(|it| it.collect())
        .unwrap_or_default()
    }

    // ---------------------------------------------------------------- reading
    // See `reading.rs`.

    // ---------------------------------------------------------- subscriptions

    pub fn replace_subscriptions(&self, subs: &[Subscription]) -> Result<bool> {
        if self.subscriptions() == subs {
            return Ok(false);
        }
        let mut c = self.conn();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM subscriptions", [])?;
        {
            let mut st =
                tx.prepare("INSERT INTO subscriptions (pos, id, json) VALUES (?1, ?2, ?3)")?;
            for (i, s) in subs.iter().enumerate() {
                let json =
                    serde_json::to_string(s).map_err(|e| CoreError::Storage(e.to_string()))?;
                st.execute(params![i as i64, s.id, json])?;
            }
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn subscriptions(&self) -> Vec<Subscription> {
        let c = self.conn();
        let Ok(mut st) = c.prepare_cached("SELECT json FROM subscriptions ORDER BY pos") else {
            return vec![];
        };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|it| {
                it.filter_map(|s| s.ok())
                    .filter_map(|s| serde_json::from_str(&s).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Edit one cached subscription in place (or remove it when `f` returns false).
    pub fn edit_subscription(
        &self,
        id: &str,
        f: impl FnOnce(&mut Subscription) -> bool,
    ) -> Result<()> {
        let mut subs = self.subscriptions();
        if let Some(pos) = subs.iter().position(|s| s.id == id) {
            if !f(&mut subs[pos]) {
                subs.remove(pos);
            }
            self.replace_subscriptions(&subs)?;
        }
        Ok(())
    }

    /// The blyg's public origin (with its mount, trailing `/`), from any
    /// permalink the server gave us (`{origin}f/{id}`).
    pub fn public_origin(&self) -> Option<String> {
        let p: String = self
            .conn()
            .query_row(
                "SELECT permalink FROM items WHERE permalink IS NOT NULL AND permalink <> '' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok()?;
        let p = p.trim_end_matches('/');
        let (rest, _id) = p.rsplit_once('/')?;
        let (origin, kind) = rest.rsplit_once('/')?;
        matches!(kind, "f" | "t").then(|| format!("{origin}/"))
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }
}

fn op_from_row(r: &Row) -> rusqlite::Result<Op> {
    Ok(Op {
        seq: r.get(0)?,
        local_id: LocalId(r.get(1)?),
        kind: OpKind::parse(&r.get::<_, String>(2)?),
        not_before: r.get(3)?,
        in_flight: r.get(4)?,
        payload: r.get(5)?,
    })
}

fn ops_for(tx: &Connection, id: &LocalId) -> rusqlite::Result<Vec<Op>> {
    let mut st =
        tx.prepare_cached("SELECT seq, local_id, op, not_before, in_flight, payload FROM outbox WHERE local_id = ?1 ORDER BY seq")?;
    st.query_map([&id.0], op_from_row)?.collect()
}

fn enqueue(tx: &Connection, id: &LocalId, op: OpKind, not_before: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO outbox (local_id, op, not_before) VALUES (?1, ?2, ?3)",
        params![id.0, op.as_str(), not_before],
    )?;
    Ok(())
}

fn insert_wire(tx: &Connection, w: &WireItem) -> rusqlite::Result<LocalId> {
    let id = new_local_id();
    let kind = kind_str(w.local_kind());
    tx.execute(
        "INSERT INTO items (local_id, server_id, kind, server_kind, status, version, dirty, content_md, created, \
         updated, permalink, stub_of, forked_from, show_responses, base_updated, base_content) \
         VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?9, ?7)",
        params![
            id.0,
            w.id,
            kind,
            w.status,
            w.version,
            w.dirty,
            w.content_md,
            w.created,
            w.updated,
            w.permalink,
            raw_json(&w.stub_of),
            raw_json(&w.forked_from),
            w.show_responses,
        ],
    )?;
    if let Some(v) = &w.versions {
        put_versions(tx, &id, v)?;
    }
    Ok(id)
}

fn put_versions(tx: &Connection, id: &LocalId, v: &[Version]) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM versions WHERE local_id = ?1", [&id.0])?;
    let mut st = tx.prepare_cached(
        "INSERT INTO versions (local_id, version, published_at, note, pinned, endcap) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for x in v {
        st.execute(params![
            id.0,
            x.version,
            x.published_at,
            x.note,
            x.pinned,
            x.endcap
        ])?;
    }
    Ok(())
}

/// Merge one server copy into an existing local row.
///
/// Conflict rule: the item has unpushed local edits, and the server's content
/// differs both from what we last synced against (`base_content`) and from our
/// working copy. Content, not `updated`, is the signal: the Worker doesn't bump
/// `updated` when a published item's working copy is saved, and does bump it
/// on our own draft saves, so `updated` alone gives false negatives and
/// positives. `base_updated` is still tracked for diagnostics.
fn merge_one(
    tx: &Connection,
    row: &SyncRow,
    w: &WireItem,
    detect: bool,
) -> rusqlite::Result<(bool, Option<ConflictInfo>)> {
    let it = &row.item;
    let id = &it.local_id;
    let pending = has_ops(tx, id)?;
    let server_kind = kind_str(w.local_kind());
    let updated = max_str(&it.updated, &w.updated);

    // Metadata the server owns, applied in every case.
    tx.execute(
        "UPDATE items SET status = ?2, version = ?3, permalink = ?4, stub_of = ?5, forked_from = ?6, \
         show_responses = ?7, server_kind = ?8, base_updated = ?9, updated = ?10 WHERE local_id = ?1",
        params![
            id.0,
            w.status,
            w.version,
            w.permalink,
            raw_json(&w.stub_of),
            raw_json(&w.forked_from),
            w.show_responses,
            server_kind,
            w.updated,
            updated,
        ],
    )?;
    let meta_changed = status_str(it.status) != w.status
        || it.version != w.version
        || it.permalink != w.permalink
        || it.show_responses != w.show_responses
        || it.updated != updated;

    if it.conflict {
        if row.theirs.as_deref() != Some(w.content_md.as_str()) {
            tx.execute(
                "UPDATE items SET theirs_content = ?2 WHERE local_id = ?1",
                params![id.0, w.content_md],
            )?;
        }
        return Ok((meta_changed, None));
    }

    if !pending {
        let changed = meta_changed
            || it.content_md != w.content_md
            || it.dirty != w.dirty
            || kind_str(it.kind) != server_kind;
        if it.content_md != w.content_md {
            // Someone else's text: its provenance is whatever the server holds.
            provenance::forget_tx(tx, id)?;
        }
        tx.execute(
            "UPDATE items SET content_md = ?2, base_content = ?2, dirty = ?3, kind = ?4 WHERE local_id = ?1",
            params![id.0, w.content_md, w.dirty, server_kind],
        )?;
        return Ok((changed, None));
    }

    let server_moved = row.base_content.as_deref() != Some(w.content_md.as_str());
    if detect && server_moved && w.content_md != it.content_md {
        tx.execute(
            "UPDATE items SET conflict = 1, theirs_content = ?2 WHERE local_id = ?1",
            params![id.0, w.content_md],
        )?;
        return Ok((
            true,
            Some((id.clone(), it.content_md.clone(), w.content_md.clone())),
        ));
    }
    if w.content_md == it.content_md {
        tx.execute(
            "UPDATE items SET base_content = ?2 WHERE local_id = ?1",
            params![id.0, w.content_md],
        )?;
    }
    Ok((meta_changed, None))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod reading_tests;
