//! Sync: outbox flush, periodic pull, conflict detection, status tracking.
//!
//! `Engine` holds the store, the API client and the event sink. All network
//! work that touches items runs under `net` (a mutex), so the background
//! worker, `publish` and `sync_now` never interleave half-applied ops (e.g. a
//! pull that sees a server item whose create hasn't been recorded locally yet).
//! The store's own lock is never held across network I/O, so UI reads and
//! saves stay instant while a push is in flight.

mod worker;

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::api::{Api, is_transient};
use crate::backend::{CoreError, Result};
use crate::model::*;
use crate::store::{Op, OpKind, Store};
use crate::util::now_ms;

pub(crate) use worker::{Msg, spawn};

/// `meta` key set when the server answered 404 to the provenance endpoint.
pub const PROVENANCE_UNAVAILABLE: &str = "tk_provenance_unavailable";

/// `meta` key: "1" while the server advertises read-state sync (the last
/// `GET /api/reading` said `read_state: true`, extension 5), "0" or absent
/// otherwise. Persisted so a read marked offline after a restart still queues.
pub const READ_SYNC: &str = "read_sync";

/// `meta` key set once this database's read state has been batch-uploaded
/// (the first time the server advertised read-state sync).
pub const READ_SYNC_UPLOADED: &str = "read_sync_uploaded";

/// Timings. Defaults follow the spec; tests shrink them.
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Push this long after the last save of an item.
    pub debounce: Duration,
    /// Pull `GET /api/items` + reading + subscriptions this often.
    pub pull_interval: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    /// Run the background worker thread. Tests can turn it off and drive sync
    /// with `sync_now` for determinism.
    pub start_worker: bool,
    /// Reading list pages to fetch per pull (500 items each).
    pub reading_pages: u32,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions {
            debounce: Duration::from_millis(800),
            pull_interval: Duration::from_secs(60),
            backoff_initial: Duration::from_secs(2),
            backoff_max: Duration::from_secs(120),
            start_worker: true,
            reading_pages: 4,
        }
    }
}

enum Precheck {
    Proceed,
    Stop,
}

pub(crate) type Sink = Arc<dyn Fn(CoreEvent) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Health {
    Ok,
    Offline,
    Error,
}

struct NetState {
    health: Health,
    backoff: Duration,
    retry_at: Option<Instant>,
    last_emitted: Option<SyncStatus>,
    /// Server lacks `GET /api/reading` (patch 3 not deployed).
    reading_unavailable: bool,
    /// An outbox op is on the wire (`SyncStatus::Syncing`).
    pushing: bool,
}

pub(crate) struct Engine {
    pub store: Store,
    pub api: Api,
    pub opts: SyncOptions,
    net: Mutex<()>,
    sink: RwLock<Option<Sink>>,
    state: Mutex<NetState>,
    /// `<data dir>/scratch-media` (`crate::scratch_media`); `None` = no
    /// local images here (they can't be uploaded, so the op fails).
    pub scratch_dir: Option<std::path::PathBuf>,
}

impl Engine {
    pub fn new(store: Store, api: Api, opts: SyncOptions) -> Engine {
        Engine {
            store,
            api,
            net: Mutex::new(()),
            sink: RwLock::new(None),
            state: Mutex::new(NetState {
                health: Health::Ok,
                backoff: opts.backoff_initial,
                retry_at: None,
                last_emitted: None,
                reading_unavailable: false,
                pushing: false,
            }),
            opts,
            scratch_dir: None,
        }
    }

    /// The public origin (where `media/…` lives): learned from the server's
    /// permalinks, else the API base.
    pub fn public_origin(&self) -> String {
        self.store
            .public_origin()
            .unwrap_or_else(|| self.api.base_url().to_string())
            .trim_end_matches('/')
            .to_string()
    }

    /// Upload every local image `content` refers to (unattached, like a
    /// paste into a draft), returning `(name, absolute url)` for each one
    /// that made it, plus the error that stopped the rest (if any).
    pub fn upload_scratch_media(
        &self,
        content: &str,
    ) -> (Vec<(String, String)>, Option<CoreError>) {
        let mut done = Vec::new();
        for name in crate::scratch_media::refs(content) {
            let url = format!("{}{name}", crate::scratch_media::SCHEME);
            let Some(path) = self
                .scratch_dir
                .as_deref()
                .and_then(|d| crate::scratch_media::file(d, &url))
            else {
                return (
                    done,
                    Some(CoreError::Other(format!(
                        "an image in this note ({name}) is missing from this Mac"
                    ))),
                );
            };
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => return (done, Some(CoreError::Storage(e.to_string()))),
            };
            let mime = crate::scratch_media::mime_for(&name);
            match self.track(self.api.upload_media(&bytes, mime, None, None)) {
                Ok(m) => {
                    let abs = crate::scratch_media::absolute(&m.url, &self.public_origin());
                    done.push((name, abs));
                }
                Err(e) => return (done, Some(e)),
            }
        }
        (done, None)
    }

    pub fn debounce_ms(&self) -> i64 {
        self.opts.debounce.as_millis() as i64
    }

    pub fn net_lock(&self) -> MutexGuard<'_, ()> {
        self.net.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn state(&self) -> MutexGuard<'_, NetState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ------------------------------------------------------------ events

    pub fn set_sink(&self, sink: Sink) {
        *self.sink.write().unwrap_or_else(|p| p.into_inner()) = Some(sink);
    }

    pub fn emit(&self, ev: CoreEvent) {
        let sink = self.sink.read().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(s) = sink {
            s(ev);
        }
    }

    pub fn status(&self) -> SyncStatus {
        let (health, pushing) = {
            let st = self.state();
            (st.health, st.pushing)
        };
        match health {
            Health::Offline => SyncStatus::Offline {
                pending: self.store.pending_count(),
            },
            Health::Error => SyncStatus::Error,
            Health::Ok if pushing => SyncStatus::Syncing,
            Health::Ok if self.store.earliest_due().is_some() => SyncStatus::Saving,
            Health::Ok => SyncStatus::Synced,
        }
    }

    /// Emit `SyncStatus` if it differs from the last one emitted.
    pub fn emit_status(&self) {
        let s = self.status();
        let changed = {
            let mut st = self.state();
            let c = st.last_emitted != Some(s);
            st.last_emitted = Some(s);
            c
        };
        if changed {
            self.emit(CoreEvent::SyncStatus(s));
        }
    }

    // ------------------------------------------------------------ health

    pub fn note_ok(&self) {
        let mut st = self.state();
        st.health = Health::Ok;
        st.backoff = self.opts.backoff_initial;
        st.retry_at = None;
    }

    /// Record a failed network call. Transient failures and auth failures
    /// schedule an exponential backoff for the worker.
    pub fn note_err(&self, e: &CoreError) {
        let mut st = self.state();
        let health = match e {
            CoreError::Offline => Health::Offline,
            CoreError::Unauthorized => Health::Error,
            e if is_transient(e) => Health::Error,
            _ => return,
        };
        st.health = health;
        st.retry_at = Some(Instant::now() + st.backoff);
        st.backoff = (st.backoff * 2).min(self.opts.backoff_max);
    }

    pub fn retry_at(&self) -> Option<Instant> {
        self.state().retry_at
    }

    /// Wrap a direct remote call so it feeds health/status.
    pub fn track<T>(&self, r: Result<T>) -> Result<T> {
        match &r {
            Ok(_) => self.note_ok(),
            Err(e) => self.note_err(e),
        }
        self.emit_status();
        r
    }

    // ------------------------------------------------------------ outbox

    /// Push due ops in order. `only` restricts to one item; `force` ignores
    /// the debounce. Stops at the first transient/auth failure.
    pub fn flush(&self, only: Option<&LocalId>, force: bool) -> Result<()> {
        let mut changed = false;
        let result = loop {
            let _net = self.net_lock();
            let op = match self.next_op(only, force) {
                Ok(Some(op)) => op,
                Ok(None) => break Ok(()),
                Err(e) => break Err(e),
            };
            self.set_pushing(true);
            let ran = self.run_op(&op);
            self.set_pushing(false);
            match ran {
                Ok(()) => {
                    self.note_ok();
                    changed = true;
                }
                Err(e) if is_transient(&e) || matches!(e, CoreError::Unauthorized) => {
                    let _ = self.store.set_in_flight(op.seq, false);
                    self.note_err(&e);
                    break Err(e);
                }
                Err(e) => {
                    // The server refused this op outright; retrying won't help.
                    let _ = self.store.drop_op(op.seq);
                    changed = true;
                    self.emit(CoreEvent::Error(format!("couldn't sync a change: {e}")));
                }
            }
        };
        if changed {
            self.emit(CoreEvent::ItemsChanged);
        }
        self.emit_status();
        result
    }

    /// Mark an op as on the wire; the status bar shows "syncing…" meanwhile.
    fn set_pushing(&self, on: bool) {
        self.state().pushing = on;
        if on {
            self.emit_status();
        }
    }

    fn next_op(&self, only: Option<&LocalId>, force: bool) -> Result<Option<Op>> {
        let ops = self.store.ops()?;
        let conflicted = self.store.conflicted()?;
        let now = now_ms();
        let mut blocked: HashSet<LocalId> = HashSet::new();
        for op in ops {
            if only.is_some_and(|o| *o != op.local_id) || blocked.contains(&op.local_id) {
                continue;
            }
            // Ops for one item run strictly in order: a later save waits for
            // the create that assigns its server id.
            if conflicted.contains(&op.local_id) || op.in_flight || (!force && op.not_before > now)
            {
                blocked.insert(op.local_id.clone());
                continue;
            }
            return Ok(Some(op));
        }
        Ok(None)
    }

    fn run_op(&self, op: &Op) -> Result<()> {
        if op.kind == OpKind::Read {
            return self.run_read(op);
        }
        if op.kind == OpKind::DeleteRemote {
            self.store.set_in_flight(op.seq, true)?;
            return match self.api.delete_item(&op.payload) {
                Ok(()) | Err(CoreError::NotFound) => self.store.drop_op(op.seq),
                Err(e) => Err(e),
            };
        }
        let Some(mut row) = self.store.row(&op.local_id) else {
            return self.store.drop_op(op.seq);
        };
        if crate::scratch_media::has_refs(&row.item.content_md) {
            // A scratch note promoted offline: its local images go up first,
            // and the text is sent with their blyg URLs.
            let (done, err) = self.upload_scratch_media(&row.item.content_md);
            if !done.is_empty() && self.store.rewrite_media(&op.local_id, &done)? {
                self.emit(CoreEvent::ItemsChanged);
            }
            if let Some(e) = err {
                return Err(match e {
                    CoreError::Rejected {
                        status,
                        message,
                        details,
                    } => CoreError::Rejected {
                        status,
                        message: format!("an image couldn't be uploaded: {message}"),
                        details,
                    },
                    e => e,
                });
            }
            match self.store.row(&op.local_id) {
                Some(r) => row = r,
                None => return self.store.drop_op(op.seq),
            }
        }
        let item = &row.item;
        let content = item.content_md.clone();
        self.store.set_in_flight(op.seq, true)?;
        match op.kind {
            OpKind::Create => {
                if item.server_id.is_some() {
                    return self.store.drop_op(op.seq);
                }
                let sid = self
                    .api
                    .create_item(&content, item.kind, item.stub_of.as_ref())?;
                self.store
                    .op_created(op.seq, &item.local_id, &sid, &content, item.kind)?;
                // POST can't carry provenance; a combined push follows.
                self.store.queue_prov_push(&item.local_id)
            }
            OpKind::Save => {
                let Some(sid) = &item.server_id else {
                    return self.store.op_lost_server(op.seq, &item.local_id);
                };
                match self.precheck(op, &item.local_id, &sid.0)? {
                    Precheck::Proceed => {}
                    Precheck::Stop => return Ok(()),
                }
                match self.push_text(
                    &item.local_id,
                    &sid.0,
                    &content,
                    row.base_content.as_deref(),
                ) {
                    Ok(()) => self.store.op_saved(op.seq, &item.local_id, &content),
                    Err(CoreError::NotFound) => self.store.op_lost_server(op.seq, &item.local_id),
                    Err(e) => Err(e),
                }
            }
            OpKind::Recreate => {
                let Some(old) = &item.server_id else {
                    return self.store.drop_op(op.seq);
                };
                if row.server_kind == Some(item.kind) {
                    return self.store.drop_op(op.seq);
                }
                // Retiring the old draft must not throw away an edit made elsewhere.
                match self.precheck(op, &item.local_id, &old.0)? {
                    Precheck::Proceed => {}
                    Precheck::Stop => return Ok(()),
                }
                // PUT can't change kind (api.ts only reads `kind` on POST), so
                // make a new draft of the new kind and retire the old one.
                // Provenance the old draft holds must move with the text.
                if self.store.provenance(&item.local_id).scopes.is_none()
                    && let Some(base) = &row.base_content
                    && let Ok(Some(server)) = self.api.get_tk_provenance(&old.0)
                    && server.iter().any(Option::is_some)
                {
                    self.store.adopt_prov(&item.local_id, base, &server)?;
                }
                let sid = self
                    .api
                    .create_item(&content, item.kind, item.stub_of.as_ref())?;
                self.store.op_recreated(
                    op.seq,
                    &item.local_id,
                    &sid,
                    &old.0,
                    &content,
                    item.kind,
                )?;
                self.store.queue_prov_push(&item.local_id)
            }
            OpKind::DeleteRemote | OpKind::Read => unreachable!(),
        }
    }

    /// Push the working copy. The Worker keys TK provenance by scope
    /// position, so when the scopes changed since `base` (what the server
    /// holds), or tracked provenance changed, the text and the whole
    /// provenance array go together in one `PUT …/tk-provenance`. Otherwise
    /// it's a plain `PUT /api/items/:id`.
    fn push_text(&self, id: &LocalId, sid: &str, content: &str, base: Option<&str>) -> Result<()> {
        let Some(n) = crate::tk::scope_count(content) else {
            // Malformed TK (mid-typing): positions mean nothing yet. The
            // tracked provenance stays dirty for the next well-formed push.
            return self.api.save_item(sid, content);
        };
        let t = self.store.provenance(id);
        let structure = base.is_none_or(|b| crate::tk::structure_changed(b, content));
        if !t.dirty && !structure {
            return self.api.save_item(sid, content);
        }
        let scopes = match (&t.scopes, &t.keyed_to) {
            (Some(s), Some(k)) if k == content => s.clone(),
            (Some(s), Some(k)) => crate::tk::remap(k, s, content).unwrap_or_else(|| vec![None; n]),
            _ => {
                // Not tracked here: carry the server's own provenance (keyed
                // to `base`, which the precheck just confirmed) over.
                match self.api.get_tk_provenance(sid) {
                    Ok(Some(server)) if server.iter().any(Option::is_some) => base
                        .and_then(|b| crate::tk::remap(b, &server, content))
                        .unwrap_or_else(|| vec![None; n]),
                    // Nothing recorded anywhere: nothing can shift.
                    Ok(_) | Err(CoreError::NotFound) => return self.api.save_item(sid, content),
                    Err(e) => return Err(e),
                }
            }
        };
        let scopes: Vec<Option<ScopeProvenance>> = if crate::tk::validate(content, &scopes).is_ok()
        {
            scopes
        } else {
            vec![None; n]
        };
        match self.api.put_tk_provenance(sid, Some(content), &scopes) {
            Ok(_) => self.store.prov_pushed(id, content, &scopes),
            Err(CoreError::NotFound) => {
                // No provenance extension on this server.
                self.store.set_meta(PROVENANCE_UNAVAILABLE, "1")?;
                self.api.save_item(sid, content)
            }
            Err(CoreError::Rejected {
                status: 400,
                message,
                ..
            }) => {
                // E.g. a cited source that no longer resolves: keep the
                // disclosure, drop the citations; then give up on provenance.
                let bare: Vec<Option<ScopeProvenance>> = scopes
                    .iter()
                    .map(|p| {
                        p.clone().map(|p| ScopeProvenance {
                            sources: vec![],
                            ..p
                        })
                    })
                    .collect();
                match self.api.put_tk_provenance(sid, Some(content), &bare) {
                    Ok(_) => self.store.prov_pushed(id, content, &bare),
                    Err(CoreError::Rejected { status: 400, .. }) => {
                        self.emit(CoreEvent::Error(format!(
                            "The blyg refused the AI provenance for this post ({message}); the text was saved without it"
                        )));
                        self.api.save_item(sid, content)?;
                        // Don't retry (and re-warn) on every keystroke.
                        self.store.prov_pushed(id, content, &vec![None; n])
                    }
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Before overwriting the server's working copy, make sure it is still
    /// what we last synced against. Otherwise the item enters conflict (and
    /// its ops stay queued, blocked) instead of silently clobbering an edit
    /// made on another device. This closes the gap between periodic pulls.
    fn precheck(&self, op: &Op, id: &LocalId, sid: &str) -> Result<Precheck> {
        match self.api.get_item(sid) {
            Ok(w) => match self.store.check_server(id, &w)? {
                None => Ok(Precheck::Proceed),
                Some((local_id, mine, theirs)) => {
                    self.store.set_in_flight(op.seq, false)?;
                    self.emit(CoreEvent::Conflict {
                        local_id,
                        mine,
                        theirs,
                    });
                    Ok(Precheck::Stop)
                }
            },
            Err(CoreError::NotFound) => {
                self.store.op_lost_server(op.seq, id)?;
                Ok(Precheck::Stop)
            }
            Err(e) => Err(e),
        }
    }

    // ------------------------------------------------------------ pull

    pub fn pull(&self) -> Result<()> {
        let _net = self.net_lock();
        let r = self.pull_locked();
        match &r {
            Ok(()) => self.note_ok(),
            Err(e) => self.note_err(e),
        }
        self.emit_status();
        r
    }

    fn pull_locked(&self) -> Result<()> {
        let wires = self.api.list_items()?;
        let out = self.store.merge_all(&wires)?;
        for (local_id, mine, theirs) in out.conflicts {
            self.emit(CoreEvent::Conflict {
                local_id,
                mine,
                theirs,
            });
        }
        if out.changed {
            self.emit(CoreEvent::ItemsChanged);
        }

        let mut reading_changed = false;
        match self.api.list_subscriptions() {
            Ok(subs) => reading_changed |= self.store.replace_subscriptions(&subs)?,
            Err(CoreError::NotFound) => {}
            Err(e) => return Err(e),
        }
        let mut reading_err = None;
        if let Some((items, complete)) = self.fetch_reading()? {
            let kinds = self
                .store
                .subscriptions()
                .into_iter()
                .map(|s| (s.id, s.kind))
                .collect();
            reading_changed |= self.store.merge_reading(&items, complete, &kinds)?;
            if self.read_sync_on() {
                reading_err = self.reconcile_reads(&items).err();
            }
        }
        if reading_changed {
            self.emit(CoreEvent::ReadingChanged);
        }
        reading_err.map_or(Ok(()), Err)
    }

    // ------------------------------------------------------------ read state

    /// The server syncs read state (extension 5), as of the last reading pull.
    pub fn read_sync_on(&self) -> bool {
        self.store.meta(READ_SYNC).as_deref() == Some("1")
    }

    fn set_read_sync(&self, on: bool) -> Result<()> {
        if self.read_sync_on() == on {
            return Ok(());
        }
        self.store.set_meta(READ_SYNC, if on { "1" } else { "0" })?;
        if !on {
            // Nothing to send them to any more.
            self.store.drop_read_ops()?;
        }
        Ok(())
    }

    /// After a pull from a server that syncs read state: the first time, send
    /// everything read here in batches; afterwards, queue a read op for every
    /// row held here that is ahead of what the server just reported (a read
    /// made while the server couldn't take it, or an RSS row that inherited
    /// read state from its renamed predecessor).
    fn reconcile_reads(&self, pulled: &[ReadingItem]) -> Result<()> {
        if self.store.meta(READ_SYNC_UPLOADED).is_none() {
            let marks = self.store.read_marks()?;
            for chunk in marks.chunks(crate::api::wire::READ_BATCH_MAX) {
                match self.api.put_reads(chunk) {
                    Ok(()) => {}
                    Err(CoreError::NotFound) => return self.set_read_sync(false),
                    // Refused outright (a row it doesn't like): the rest
                    // still goes, and a retry wouldn't do better.
                    Err(CoreError::Rejected { .. }) => {}
                    Err(e) => return Err(e),
                }
            }
            return self.store.set_meta(READ_SYNC_UPLOADED, "1");
        }
        let server = pulled
            .iter()
            .map(|i| {
                (
                    (i.subscription_id.clone(), i.remote_id.clone()),
                    i.read_version,
                )
            })
            .collect();
        let ahead = self.store.reads_ahead_of(&server)?;
        self.store.queue_reads(&ahead)
    }

    /// Run one `read` op: `PUT /api/reading/:sub/:remoteId/read`.
    fn run_read(&self, op: &Op) -> Result<()> {
        let Ok(m) = serde_json::from_str::<crate::api::wire::ReadMark>(&op.payload) else {
            return self.store.drop_op(op.seq);
        };
        if !self.read_sync_on() {
            return self.store.drop_op(op.seq);
        }
        self.store.set_in_flight(op.seq, true)?;
        match self.api.put_read(&m.sub, &m.remote_id, m.version) {
            Ok(()) => self.store.drop_op(op.seq),
            Err(CoreError::NotFound) => {
                // The endpoint is gone (a downgraded Worker).
                self.store.drop_op(op.seq)?;
                self.set_read_sync(false)
            }
            Err(e) => Err(e),
        }
    }

    /// `None` when the server doesn't have `/api/reading` yet. The flag is
    /// true when the last page had `next: null` (the list is complete).
    fn fetch_reading(&self) -> Result<Option<(Vec<ReadingItem>, bool)>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        for i in 0..self.opts.reading_pages.max(1) {
            match self.api.reading(500, cursor.as_deref())? {
                None => {
                    self.state().reading_unavailable = true;
                    self.set_read_sync(false)?;
                    return Ok(None);
                }
                Some(page) => {
                    self.state().reading_unavailable = false;
                    if i == 0 {
                        self.set_read_sync(page.read_sync())?;
                    }
                    all.extend(page.items);
                    match page.next {
                        // Opaque cursor: passed back verbatim as `before`.
                        Some(n) => cursor = Some(n),
                        None => return Ok(Some((all, true))),
                    }
                }
            }
        }
        Ok(Some((all, false)))
    }

    pub fn reading_unavailable(&self) -> bool {
        self.state().reading_unavailable
    }

    /// Push everything (ignoring the debounce), then pull. Bypasses backoff.
    pub fn sync_now(&self) -> Result<()> {
        {
            let mut st = self.state();
            st.retry_at = None;
        }
        self.flush(None, true)?;
        self.pull()
    }
}
