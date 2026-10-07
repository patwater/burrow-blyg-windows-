//! Sync: outbox flush, periodic pull, conflict detection, status tracking.
//!
//! `Engine` holds the store, the API client and the event sink. All network
//! work that touches items runs under `net` (a mutex), so the background
//! worker, `publish` and `sync_now` never interleave half-applied ops (e.g. a
//! pull that sees a server item whose create hasn't been recorded locally yet).
//! The store's own lock is never held across network I/O, so UI reads and
//! saves stay instant while a push is in flight.

mod worker;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use crate::api::wire::ChangeState;
use crate::api::{Api, is_transient, refused_provenance_key};
use crate::backend::{CoreError, Result};
use crate::model::*;
use crate::store::{Op, OpKind, Store};
use crate::util::now_ms;

pub(crate) use worker::{Msg, spawn};

/// `meta` key set when the server took provenance neither as the item's
/// `provenance` field (studio 0.28+) nor through extension 4 (a 404).
/// Cleared once a push with provenance lands.
pub const PROVENANCE_UNAVAILABLE: &str = "tk_provenance_unavailable";

/// `meta` key: the change epoch (`GET /api/changes`, studio 0.32+) the
/// stored revisions belong to.
pub const CHANGES_EPOCH: &str = "changes_epoch";

/// `meta` key prefix: per consumer (`items`, `subscriptions`, `reading`),
/// a JSON map of domain → the revision its last accepted fetch was loaded
/// under (captured before the fetch, docs/d1-polling-cache-design.md R3).
pub const CHANGES_SEEN: &str = "changes_seen:";

/// What each pulled collection depends on, by `/api/changes` domain. A
/// collection is fetched again only when one of these moved.
const ITEMS_DEPS: &[&str] = &["items", "settings"];
const SUBSCRIPTIONS_DEPS: &[&str] = &["subscriptions"];
const READING_DEPS: &[&str] = &["reading", "subscriptions", "signals", "hoppers"];

/// With `/api/changes`, everything is still read whole this often, as a
/// safety net (a trigger the server's dependency matrix misses, lineage
/// fills that wait on a later pull).
const FULL_PULL_EVERY: Duration = Duration::from_secs(10 * 60);

/// One item's autosave pushes are at least this far apart (`force` ignores
/// it): about 20 writes a minute at most from continuous typing, well under
/// studio 0.28's budgets (60 writes a minute across all tokens, 120 for the
/// owner).
pub const SAVE_GAP: Duration = Duration::from_secs(3);

/// How a push carries TK provenance (learned per session).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProvMode {
    /// `PATCH /api/items/:id {content_md, provenance}` (studio 0.28+).
    Field,
    /// The server refused the field: extension 4's `PUT …/tk-provenance`.
    Ext4,
}

/// `meta` key: "1" while the server advertises read-state sync (the last
/// `GET /api/reading/imported` said `read_state: true`, extension 5), "0" or absent
/// otherwise. Persisted so a read marked offline after a restart still queues.
pub const READ_SYNC: &str = "read_sync";

/// `meta` key set once this database's read state has been batch-uploaded
/// (the first time the server advertised read-state sync).
pub const READ_SYNC_UPLOADED: &str = "read_sync_uploaded";

/// `meta` key: "1" while the server also advertises `read_state_clear`
/// (mark unread syncs, and reads carry `read_at`).
pub const READ_CLEAR: &str = "read_clear";

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
    /// Public item documents fetched per pull to fill in lineage (0 = none).
    pub lineage_fetches: usize,
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
            lineage_fetches: 20,
        }
    }
}

enum Precheck {
    /// Carries the provenance the server reported with the item.
    Proceed(Option<Vec<Option<ScopeProvenance>>>),
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
    /// Server lacks `GET /api/reading/imported` (patch 3 not deployed).
    reading_unavailable: bool,
    /// An outbox op is on the wire (`SyncStatus::Syncing`).
    pushing: bool,
    /// The server proved to speak studio 0.9+'s API this session.
    compatible: bool,
    /// The server proved older than studio 0.9; cleared once it isn't.
    outdated: bool,
    /// Reading comes from upstream's own routes (no `/reading/imported`):
    /// `Some(true)`; `Some(false)` from the fork's; `None` not yet known.
    stock_reading: Option<bool>,
    /// When the stock reading list was last read end to end.
    stock_swept: Option<Instant>,
    /// Each stock entry's signature as last taken in (this session): a
    /// change shows even when the timestamps (whole seconds) don't move.
    stock_seen: std::collections::HashMap<(String, String), u64>,
    /// The last stock reading pull left changed rows for later
    /// (`STOCK_FETCHES`), so its revision isn't accepted yet.
    stock_deferred: bool,
    /// The server answers `GET /api/changes` (`None` before a pull).
    changes: Option<bool>,
    /// When everything was last read whole.
    full_at: Option<Instant>,
    /// A 429 is being waited out (its notice was sent).
    rate_limited: bool,
    /// A 403 is holding the outbox (its notice was sent).
    scope_blocked: bool,
    /// How provenance travels to this server, once known.
    prov_mode: Option<ProvMode>,
    /// When each item's last autosave push landed (`SAVE_GAP`).
    pushed: HashMap<LocalId, Instant>,
    /// The earliest time a save held back by `SAVE_GAP` may go.
    paced_until: Option<Instant>,
    /// --- read/unread --- The server refused a read-state write with 403
    /// (a scoped token on a Worker that keeps those writes to the owner):
    /// read state stays on this Mac for the rest of the session.
    read_denied: bool,
}

/// The time allowed per pull for fetching item documents (lineage).
const LINEAGE_BUDGET: Duration = Duration::from_secs(8);

/// Meta key: the "this server is stock" notice was sent for this database.
const STOCK_TOLD: &str = "stock_told";

/// On a stock server, at most this many items are fetched one by one per
/// pull (new or changed ones); the rest follow on the next pulls.
const STOCK_FETCHES: usize = 100;
/// A stock pull normally stops at the first page with nothing new; the
/// whole list is read this often, to catch older edits and removals.
const STOCK_SWEEP: Duration = Duration::from_secs(10 * 60);

pub(crate) struct Engine {
    pub store: Store,
    pub api: Api,
    /// Other blygs' public files (lineage fills), with short timeouts.
    public: crate::api::public::PublicClient,
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
            public: crate::api::public::PublicClient::with_timeouts(
                Duration::from_secs(4),
                Duration::from_secs(6),
            ),
            net: Mutex::new(()),
            sink: RwLock::new(None),
            state: Mutex::new(NetState {
                health: Health::Ok,
                backoff: opts.backoff_initial,
                retry_at: None,
                last_emitted: None,
                reading_unavailable: false,
                pushing: false,
                compatible: false,
                outdated: false,
                stock_reading: None,
                stock_swept: None,
                stock_seen: Default::default(),
                stock_deferred: false,
                changes: None,
                full_at: None,
                rate_limited: false,
                scope_blocked: false,
                prov_mode: None,
                pushed: HashMap::new(),
                paced_until: None,
                read_denied: false,
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
                        "an image in this note ({name}) is missing from this {}",
                        if cfg!(windows) { "PC" } else { "Mac" }
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
        st.rate_limited = false;
        st.scope_blocked = false;
    }

    /// Record a failed network call. Transient failures and auth failures
    /// schedule an exponential backoff for the worker. A 429 waits exactly
    /// the server's `Retry-After` instead, and says so once.
    pub fn note_err(&self, e: &CoreError) {
        if let CoreError::RateLimited { retry_after } = e {
            let first = {
                let mut st = self.state();
                st.retry_at = Some(Instant::now() + Duration::from_secs(*retry_after));
                !std::mem::replace(&mut st.rate_limited, true)
            };
            if first {
                self.emit(CoreEvent::Error(format!(
                    "The blyg asked Burrow to slow down; retrying in {retry_after}s"
                )));
            }
            return;
        }
        if let CoreError::Rejected { status: 403, .. } = e {
            let first = !std::mem::replace(&mut self.state().scope_blocked, true);
            if first {
                // The message names the scope the credential lacks.
                self.emit(CoreEvent::Error(format!(
                    "Changes are waiting: {e}. They'll be sent once you sign in with access to it."
                )));
            }
        }
        let mut st = self.state();
        let health = match e {
            CoreError::Offline => Health::Offline,
            CoreError::Unauthorized | CoreError::ServerOutdated => Health::Error,
            CoreError::Rejected { status: 403, .. } => Health::Error,
            e if is_transient(e) => Health::Error,
            _ => return,
        };
        // Windows: say why, once, when the blyg starts refusing the token;
        // the status bar alone only says "sync error".
        let newly_refused = cfg!(target_os = "windows")
            && matches!(e, CoreError::Unauthorized)
            && st.health != Health::Error;
        st.health = health;
        st.retry_at = Some(Instant::now() + st.backoff);
        st.backoff = (st.backoff * 2).min(self.opts.backoff_max);
        drop(st);
        if newly_refused {
            self.emit(CoreEvent::Error(
                "Your blyg refused the owner token (401). If the token changed, use \
                 Disconnect… from the Menu and connect again with the new one."
                    .into(),
            ));
        }
    }

    pub fn retry_at(&self) -> Option<Instant> {
        self.state().retry_at
    }

    /// The earliest time an autosave held back by `SAVE_GAP` may go, as of
    /// the last flush.
    pub fn paced_until(&self) -> Option<Instant> {
        self.state().paced_until
    }

    /// How often the worker pulls: a quarter of `pull_interval` (15 s by
    /// default, like Studio) when the server answers `/api/changes`, since an
    /// unchanged pull is then one small read; else the full interval.
    pub fn pull_every(&self) -> Duration {
        if self.state().changes == Some(true) {
            (self.opts.pull_interval / 4).max(Duration::from_secs(1))
        } else {
            self.opts.pull_interval
        }
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
            if let Err(e) = self.check_server() {
                // Nothing is pushed; the op stays queued for a server that can take it.
                self.note_err(&e);
                break Err(e);
            }
            self.set_pushing(true);
            let ran = self.run_op(&op);
            self.set_pushing(false);
            match ran {
                Ok(()) => {
                    if op.kind == OpKind::Save {
                        self.state()
                            .pushed
                            .insert(op.local_id.clone(), Instant::now());
                    }
                    self.note_ok();
                    changed = true;
                }
                // A 403 is the credential, not the change: a token without
                // the scope this op needs (studio 0.28+). Like an ended
                // sign-in, the op waits for a credential that can send it.
                Err(e)
                    if is_transient(&e)
                        || matches!(
                            e,
                            CoreError::Unauthorized | CoreError::Rejected { status: 403, .. }
                        ) =>
                {
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

    /// Whether the server proved older than studio 0.9 (as of the last check).
    pub fn server_outdated(&self) -> bool {
        self.state().outdated
    }

    /// Make sure the server speaks studio 0.9+'s API before anything is
    /// pushed or pulled: on an older one a new route's 404 reads as "item
    /// deleted", and the push would re-create it. Checked once per session
    /// (one small read), and again while the answer is "outdated".
    fn check_server(&self) -> Result<()> {
        if self.state().compatible {
            return Ok(());
        }
        match self.api.count_items() {
            Ok(_) => {
                let mut st = self.state();
                st.compatible = true;
                st.outdated = false;
                Ok(())
            }
            Err(CoreError::ServerOutdated) => {
                let first = !std::mem::replace(&mut self.state().outdated, true);
                if first {
                    self.emit(CoreEvent::ServerOutdated);
                }
                Err(CoreError::ServerOutdated)
            }
            Err(e) => Err(e),
        }
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
        let mut st = self.state();
        st.paced_until = None;
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
            // Autosave pacing: the edits keep coalescing into this op meanwhile.
            if !force
                && op.kind == OpKind::Save
                && let Some(at) = st.pushed.get(&op.local_id).map(|t| *t + SAVE_GAP)
                && at > Instant::now()
            {
                st.paced_until = Some(st.paced_until.map_or(at, |p| p.min(at)));
                blocked.insert(op.local_id.clone());
                continue;
            }
            return Ok(Some(op));
        }
        Ok(None)
    }

    fn run_op(&self, op: &Op) -> Result<()> {
        if matches!(op.kind, OpKind::Read | OpKind::Unread) {
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
                // Studio 0.28+ takes the provenance with the text.
                if let Some(prov) = self.create_provenance(&item.local_id, &content) {
                    match self.api.create_item(
                        &content,
                        item.kind,
                        item.stub_of.as_ref(),
                        Some(&prov),
                    ) {
                        Ok(sid) => {
                            self.state().prov_mode = Some(ProvMode::Field);
                            self.store.op_created(
                                op.seq,
                                &item.local_id,
                                &sid,
                                &content,
                                item.kind,
                            )?;
                            self.provenance_landed()?;
                            return self.store.prov_pushed(&item.local_id, &content, &prov);
                        }
                        Err(e @ CoreError::Rejected { status: 400, .. }) => {
                            if refused_provenance_key(&e) {
                                self.state().prov_mode = Some(ProvMode::Ext4);
                            }
                            // Create without it; the push that follows
                            // retries (or reports) the provenance.
                        }
                        Err(e) => return Err(e),
                    }
                }
                let sid = self
                    .api
                    .create_item(&content, item.kind, item.stub_of.as_ref(), None)?;
                self.store
                    .op_created(op.seq, &item.local_id, &sid, &content, item.kind)?;
                // The provenance follows in a combined push.
                self.store.queue_prov_push(&item.local_id)
            }
            OpKind::Save => {
                let Some(sid) = &item.server_id else {
                    return self.store.op_lost_server(op.seq, &item.local_id);
                };
                let held = match self.precheck(op, &item.local_id, &sid.0)? {
                    Precheck::Proceed(held) => held,
                    Precheck::Stop => return Ok(()),
                };
                match self.push_text(
                    &item.local_id,
                    &sid.0,
                    &content,
                    row.base_content.as_deref(),
                    held,
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
                let held = match self.precheck(op, &item.local_id, &old.0)? {
                    Precheck::Proceed(held) => held,
                    Precheck::Stop => return Ok(()),
                };
                // A draft's kind can change until it is first published:
                // one `PATCH {content_md, kind}` keeps its id. Provenance the
                // server holds is keyed to `base`; adopt it so the push that
                // follows remaps it onto the new text.
                if self.store.provenance(&item.local_id).scopes.is_none()
                    && let Some(base) = &row.base_content
                    && let Ok(Some(server)) = self.server_provenance(&old.0, held)
                    && server.iter().any(Option::is_some)
                {
                    self.store.adopt_prov(&item.local_id, base, &server)?;
                }
                match self.api.save_item_kind(&old.0, &content, item.kind) {
                    Ok(()) => {
                        self.store
                            .op_kind_saved(op.seq, &item.local_id, &content, item.kind)?;
                        self.store.queue_prov_push(&item.local_id)
                    }
                    Err(CoreError::NotFound) => self.store.op_lost_server(op.seq, &item.local_id),
                    Err(e) => Err(e),
                }
            }
            OpKind::DeleteRemote | OpKind::Read | OpKind::Unread => unreachable!(),
        }
    }

    /// Push the working copy. The Worker keys TK provenance by scope
    /// position, so when the scopes changed since `base` (what the server
    /// holds), or tracked provenance changed, the text and the whole
    /// provenance array go together in one write: `PATCH {content_md,
    /// provenance}` on studio 0.28+, extension 4's `PUT …/tk-provenance` on
    /// an older server that refuses the field. Otherwise it's a plain
    /// `PATCH {content_md}`. `held` is the provenance the server reported
    /// with the item (the precheck's read; `None` before studio 0.28).
    fn push_text(
        &self,
        id: &LocalId,
        sid: &str,
        content: &str,
        base: Option<&str>,
        held: Option<Vec<Option<ScopeProvenance>>>,
    ) -> Result<()> {
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
                match self.server_provenance(sid, held) {
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
            self.with_versions(scopes)
        } else {
            vec![None; n]
        };
        self.push_provenance(id, sid, content, &scopes)
    }

    /// The provenance the server holds for an item: what its read reported
    /// (`held`, studio 0.28+), else extension 4's `GET …/tk-provenance`.
    fn server_provenance(
        &self,
        sid: &str,
        held: Option<Vec<Option<ScopeProvenance>>>,
    ) -> Result<Option<Vec<Option<ScopeProvenance>>>> {
        if held.is_some() || self.state().prov_mode == Some(ProvMode::Field) {
            return Ok(held);
        }
        match self.api.get_tk_provenance(sid) {
            // Extension routes closed to this credential: nothing readable.
            Err(CoreError::Rejected { status: 403, .. }) => Ok(None),
            r => r,
        }
    }

    /// Write `content` and `scopes` together, by whichever route the server
    /// takes. A refusal of the provenance itself (e.g. a cited source that no
    /// longer resolves) retries without the citations, then saves the text
    /// with no disclosure and says so.
    fn push_provenance(
        &self,
        id: &LocalId,
        sid: &str,
        content: &str,
        scopes: &[Option<ScopeProvenance>],
    ) -> Result<()> {
        let field = self.state().prov_mode != Some(ProvMode::Ext4);
        let first = if field {
            match self.api.save_item_provenance(sid, content, scopes) {
                Err(e) if refused_provenance_key(&e) => {
                    // Older than studio 0.28: try extension 4.
                    self.state().prov_mode = Some(ProvMode::Ext4);
                    return self.push_provenance(id, sid, content, scopes);
                }
                Ok(()) => {
                    self.state().prov_mode = Some(ProvMode::Field);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            match self.api.put_tk_provenance(sid, Some(content), scopes) {
                // No provenance on this server at all, or not for this
                // credential (a Worker answers scoped tokens 403 on its
                // extension routes): disclosure isn't recordable here.
                Err(CoreError::NotFound | CoreError::Rejected { status: 403, .. }) => {
                    self.store.set_meta(PROVENANCE_UNAVAILABLE, "1")?;
                    return self.api.save_item(sid, content);
                }
                r => r.map(|_| ()),
            }
        };
        let message = match first {
            Ok(()) => {
                self.provenance_landed()?;
                return self.store.prov_pushed(id, content, scopes);
            }
            Err(CoreError::Rejected {
                status: 400,
                message,
                ..
            }) => message,
            Err(e) => return Err(e),
        };
        let send = |s: &[Option<ScopeProvenance>]| {
            if field {
                self.api.save_item_provenance(sid, content, s)
            } else {
                self.api
                    .put_tk_provenance(sid, Some(content), s)
                    .map(|_| ())
            }
        };
        let bare: Vec<Option<ScopeProvenance>> = scopes
            .iter()
            .map(|p| {
                p.clone().map(|p| ScopeProvenance {
                    sources: vec![],
                    ..p
                })
            })
            .collect();
        match send(&bare) {
            Ok(()) => self.store.prov_pushed(id, content, &bare),
            Err(CoreError::Rejected { status: 400, .. }) => {
                self.emit(CoreEvent::Error(format!(
                    "The blyg refused the AI provenance for this post ({message}); the text was saved without it"
                )));
                let none = vec![None; scopes.len()];
                // Clear what the server held, so nothing stale stays keyed
                // to the new text; failing that, the text alone.
                match send(&none) {
                    Err(CoreError::Rejected { status: 400, .. }) => {
                        self.api.save_item(sid, content)?
                    }
                    r => r?,
                }
                // Don't retry (and re-warn) on every keystroke.
                self.store.prov_pushed(id, content, &none)
            }
            Err(e) => Err(e),
        }
    }

    /// A push with provenance landed: the server takes it after all.
    fn provenance_landed(&self) -> Result<()> {
        if self.store.meta(PROVENANCE_UNAVAILABLE).is_some() {
            self.store.delete_meta(PROVENANCE_UNAVAILABLE)?;
        }
        Ok(())
    }

    /// The provenance to send with a new item's `POST` (studio 0.28+): the
    /// tracked provenance, keyed to `content` and valid, when it discloses
    /// anything and the server isn't known to refuse the field.
    fn create_provenance(
        &self,
        id: &LocalId,
        content: &str,
    ) -> Option<Vec<Option<ScopeProvenance>>> {
        if self.state().prov_mode == Some(ProvMode::Ext4) {
            return None;
        }
        let t = self.store.provenance(id);
        let scopes = match (t.scopes, t.keyed_to) {
            (Some(s), Some(k)) if k == content => s,
            (Some(s), Some(k)) => crate::tk::remap(&k, &s, content)?,
            _ => return None,
        };
        if !scopes.iter().any(Option::is_some) || crate::tk::validate(content, &scopes).is_err() {
            return None;
        }
        Some(self.with_versions(scopes))
    }

    /// Upstream records each cited source with its version, and the app
    /// may leave that to the server (`None`): fill it in from the item held
    /// here (own posts) or the reading list, and drop a source neither
    /// knows (the disclosure itself stays).
    fn with_versions(&self, scopes: Vec<Option<ScopeProvenance>>) -> Vec<Option<ScopeProvenance>> {
        if !scopes
            .iter()
            .flatten()
            .any(|p| p.sources.iter().any(|s| s.version.is_none()))
        {
            return scopes;
        }
        let reading = self.store.reading();
        let version_of = |id: &str| -> Option<u32> {
            let own = self
                .store
                .local_id_for_server(id)
                .and_then(|l| self.store.item(&l))
                .map(|i| i.version)
                .filter(|v| *v > 0);
            own.or_else(|| {
                reading
                    .iter()
                    .find(|r| r.remote_id == id)
                    .map(|r| r.version)
                    .filter(|v| *v > 0)
            })
        };
        scopes
            .into_iter()
            .map(|p| {
                p.map(|mut p| {
                    p.sources = p
                        .sources
                        .into_iter()
                        .filter_map(|mut s| {
                            if s.version.is_none() {
                                s.version = Some(version_of(&s.id)?);
                            }
                            Some(s)
                        })
                        .collect();
                    p
                })
            })
            .collect()
    }

    /// Before overwriting the server's working copy, make sure it is still
    /// what we last synced against. Otherwise the item enters conflict (and
    /// its ops stay queued, blocked) instead of silently clobbering an edit
    /// made on another device. This closes the gap between periodic pulls.
    /// `Proceed` carries the provenance the server reported (studio 0.28+).
    fn precheck(&self, op: &Op, id: &LocalId, sid: &str) -> Result<Precheck> {
        match self.api.get_item(sid) {
            Ok(w) => match self.store.check_server(id, &w)? {
                None => Ok(Precheck::Proceed(w.server_provenance())),
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

    /// One pull. On studio 0.32+, `GET /api/changes` comes first and each
    /// collection is fetched only when a domain it depends on moved since
    /// its last accepted fetch (everything, every `FULL_PULL_EVERY`). The
    /// revisions are captured before any data is loaded and recorded only
    /// after that collection's fetch succeeded, so a failed or partial
    /// fetch is simply done again (docs/d1-polling-cache-design.md R3).
    fn pull_locked(&self) -> Result<()> {
        self.check_server()?;
        let rev = self.revisions()?;
        let full = rev.is_none()
            || self
                .state()
                .full_at
                .is_none_or(|t| t.elapsed() >= FULL_PULL_EVERY);
        let stale = |consumer: &str, deps: &[&str]| full || !self.unchanged(&rev, consumer, deps);

        if stale("items", ITEMS_DEPS) {
            let settings = full || !self.unchanged(&rev, "items", &["settings"]);
            let wires = self.api.list_items_with(settings)?;
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
            self.accept(&rev, "items", ITEMS_DEPS)?;
        }

        let mut reading_changed = false;
        if stale("subscriptions", SUBSCRIPTIONS_DEPS) {
            match self.api.list_subscriptions() {
                Ok(subs) => reading_changed |= self.store.replace_subscriptions(&subs)?,
                Err(CoreError::NotFound) => {}
                Err(e) => return Err(e),
            }
            self.accept(&rev, "subscriptions", SUBSCRIPTIONS_DEPS)?;
        }
        let mut reading_err = None;
        // Read state (extension 5) lives in a table the change counters
        // don't watch: with it on, the reading list is always read.
        // Upstream's own read state (studio's read-state routes, on a stock
        // server) advances the `reading` revision, so it needs no exception.
        let ext5_reads = self.read_sync_on() && self.state().stock_reading != Some(true);
        if stale("reading", READING_DEPS) || ext5_reads {
            self.state().stock_deferred = false;
            let fetched = self.fetch_reading()?;
            let mut whole = true;
            if let Some((items, complete)) = fetched {
                let kinds = self
                    .store
                    .subscriptions()
                    .into_iter()
                    .map(|s| (s.id, s.kind))
                    .collect();
                reading_changed |= self.store.merge_reading(&items, complete, &kinds)?;
                reading_changed |= self.fill_lineage(&items, complete, &kinds)?;
                if self.read_sync_on() {
                    match self.reconcile_reads(&items) {
                        Ok(cleared) => reading_changed |= cleared,
                        Err(e) => reading_err = Some(e),
                    }
                }
                // A stock pull that stopped early took in everything new
                // (older edits wait for the next full pull's sweep); one
                // that left changed rows for later isn't done.
                let st = self.state();
                whole = complete || (st.stock_reading == Some(true) && !st.stock_deferred);
            }
            if whole {
                self.accept(&rev, "reading", READING_DEPS)?;
            }
        }
        if reading_changed {
            self.emit(CoreEvent::ReadingChanged);
        }
        if full && rev.is_some() {
            self.state().full_at = Some(Instant::now());
        }
        reading_err.map_or(Ok(()), Err)
    }

    // ------------------------------------------------------------ revisions

    /// `GET /api/changes`, captured before anything is loaded. `None` from
    /// a server without it (older than studio 0.32), which is pulled whole
    /// as before. A new epoch (the database was restored or replaced)
    /// forgets every stored revision, so everything is read again.
    fn revisions(&self) -> Result<Option<ChangeState>> {
        let c = match self.api.changes() {
            Ok(c) => c,
            Err(e) if is_transient(&e) || matches!(e, CoreError::Unauthorized) => return Err(e),
            // Refused or unreadable: pull the old way.
            Err(_) => None,
        };
        self.state().changes = Some(c.is_some());
        let Some(c) = c else {
            return Ok(None);
        };
        if self.store.meta(CHANGES_EPOCH).as_deref() != Some(c.epoch.as_str()) {
            for consumer in ["items", "subscriptions", "reading"] {
                self.store
                    .delete_meta(&format!("{CHANGES_SEEN}{consumer}"))?;
            }
            self.store.set_meta(CHANGES_EPOCH, &c.epoch)?;
            self.state().full_at = None;
        }
        Ok(Some(c))
    }

    /// The revisions `consumer`'s last accepted fetch was loaded under.
    fn seen(&self, consumer: &str) -> BTreeMap<String, u64> {
        self.store
            .meta(&format!("{CHANGES_SEEN}{consumer}"))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Whether none of `deps` moved since `consumer` last fetched. A domain
    /// the server doesn't report counts as moved.
    fn unchanged(&self, rev: &Option<ChangeState>, consumer: &str, deps: &[&str]) -> bool {
        let Some(rev) = rev else {
            return false;
        };
        let seen = self.seen(consumer);
        deps.iter()
            .all(|d| rev.domains.get(*d).is_some_and(|n| seen.get(*d) == Some(n)))
    }

    /// `consumer`'s fetch succeeded: record the revisions captured before it.
    fn accept(&self, rev: &Option<ChangeState>, consumer: &str, deps: &[&str]) -> Result<()> {
        let Some(rev) = rev else {
            return Ok(());
        };
        let seen: BTreeMap<&str, u64> = deps
            .iter()
            .filter_map(|d| rev.domains.get(*d).map(|n| (*d, *n)))
            .collect();
        self.store.set_meta(
            &format!("{CHANGES_SEEN}{consumer}"),
            &serde_json::to_string(&seen).unwrap_or_default(),
        )
    }

    // ------------------------------------------------------------ read state

    /// The server syncs read state (extension 5, or upstream's read-state
    /// routes), as of the last reading pull.
    /// False for the rest of the session once a write was refused with 403
    /// (`read_denied`): the marks stay here, unsent, and go up from a later
    /// session that may write (each keeps its `read_at` until confirmed).
    pub fn read_sync_on(&self) -> bool {
        self.store.meta(READ_SYNC).as_deref() == Some("1") && !self.state().read_denied
    }

    /// A read-state write answered 403: stop sending them this session, and
    /// say so once. The local marks stay.
    fn read_denied(&self) -> Result<()> {
        let first = !std::mem::replace(&mut self.state().read_denied, true);
        if first {
            self.emit(CoreEvent::Error(
                "Read state stays on this Mac: this sign-in can't save it to your blyg".into(),
            ));
        }
        Ok(())
    }

    /// The server can also clear read state (`read_state_clear`): unread
    /// syncs, and reads carry `read_at`.
    pub fn read_clear_on(&self) -> bool {
        self.read_sync_on() && self.store.meta(READ_CLEAR).as_deref() == Some("1")
    }

    fn set_read_sync(&self, on: bool) -> Result<()> {
        if self.read_sync_on() == on {
            return Ok(());
        }
        self.store.set_meta(READ_SYNC, if on { "1" } else { "0" })?;
        if !on {
            // Nothing to send them to any more.
            self.store.drop_read_ops()?;
            self.set_read_clear(false)?;
        }
        Ok(())
    }

    fn set_read_clear(&self, on: bool) -> Result<()> {
        let was = self.store.meta(READ_CLEAR).as_deref() == Some("1");
        if was == on {
            return Ok(());
        }
        self.store
            .set_meta(READ_CLEAR, if on { "1" } else { "0" })?;
        if !on {
            // Unread stays on this Mac (each row's floor holds it).
            self.store.drop_unread_ops()?;
        }
        Ok(())
    }

    /// After a pull from a server that syncs read state: the first time, send
    /// everything read here in batches; afterwards, settle each row the pull
    /// reported (`Store::reconcile_read_state`): queue reads the server
    /// lacks (made while it couldn't take them, or an RSS row that inherited
    /// read state from its renamed predecessor), queue unreads it lacks (on a
    /// server that can clear read state), and take the clears made on other
    /// devices. True when a row here changed.
    fn reconcile_reads(&self, pulled: &[ReadingItem]) -> Result<bool> {
        let clear = self.read_clear_on();
        if self.store.meta(READ_SYNC_UPLOADED).is_none() {
            let marks = self.store.read_marks(clear)?;
            for chunk in marks.chunks(crate::api::wire::READ_BATCH_MAX) {
                match self.api.put_reads(chunk) {
                    Ok(()) => {}
                    Err(CoreError::NotFound) => {
                        self.set_read_sync(false)?;
                        return Ok(false);
                    }
                    Err(CoreError::Rejected { status: 403, .. }) => {
                        self.read_denied()?;
                        return Ok(false);
                    }
                    // Refused outright (a row it doesn't like): the rest
                    // still goes, and a retry wouldn't do better.
                    Err(CoreError::Rejected { .. }) => {}
                    Err(e) => return Err(e),
                }
            }
            self.store.set_meta(READ_SYNC_UPLOADED, "1")?;
            return Ok(false);
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
        let plan = self.store.reconcile_read_state(&server, clear)?;
        Ok(plan.cleared > 0)
    }

    /// Run a `read` or `unread` op: with others of its kind that wait, as
    /// one batch (`POST /api/reading/read` or `/unread`, at most 500), else
    /// one `PUT` or `DELETE`. Unread ops reach the server only when it
    /// advertises `read_state_clear`; otherwise they're dropped (unread
    /// stays on this Mac). `read_at` goes only to such a server too.
    fn run_read(&self, op: &Op) -> Result<()> {
        use crate::api::wire::{READ_BATCH_MAX, ReadMark, UnreadMark};
        let unread = op.kind == OpKind::Unread;
        if !self.read_sync_on() || (unread && !self.read_clear_on()) {
            return self.store.drop_op(op.seq);
        }
        let clear = self.read_clear_on();
        // This op first, then every other of its kind that's next in line
        // for its own row.
        let mut batch = vec![op.clone()];
        let mut seen: HashSet<LocalId> = HashSet::from([op.local_id.clone()]);
        for o in self.store.ops()? {
            if batch.len() >= READ_BATCH_MAX {
                break;
            }
            if !seen.insert(o.local_id.clone()) {
                continue;
            }
            if o.kind == op.kind && !o.in_flight && o.local_id.0.starts_with("read\u{1f}") {
                batch.push(o);
            }
        }
        for o in &batch {
            self.store.set_in_flight(o.seq, true)?;
        }
        let done = |this: &Self| -> Result<()> {
            for o in &batch {
                this.store.drop_op(o.seq)?;
            }
            Ok(())
        };
        let denied = |this: &Self| -> Result<()> {
            // Unsent this session; the rows keep their `read_at` / floor,
            // so a later session that may write sends them.
            done(this)?;
            this.read_denied()
        };
        let gone = |this: &Self| -> Result<()> {
            // The endpoint is gone (a downgraded server).
            done(this)?;
            if unread {
                this.set_read_clear(false)
            } else {
                this.set_read_sync(false)
            }
        };
        if unread {
            let marks: Vec<UnreadMark> = batch
                .iter()
                .filter_map(|o| serde_json::from_str(&o.payload).ok())
                .collect();
            let r = match marks.as_slice() {
                [] => Ok(()),
                [m] => self.api.delete_read(&m.sub, &m.remote_id),
                ms => self.api.post_unreads(ms),
            };
            return match r {
                Ok(()) => {
                    self.store.unread_acked(&marks)?;
                    done(self)
                }
                Err(CoreError::NotFound) => gone(self),
                Err(CoreError::Rejected { status: 403, .. }) => denied(self),
                Err(e) => {
                    for o in &batch[1..] {
                        self.store.set_in_flight(o.seq, false)?;
                    }
                    Err(e)
                }
            };
        }
        let marks: Vec<ReadMark> = batch
            .iter()
            .filter_map(|o| serde_json::from_str::<ReadMark>(&o.payload).ok())
            .map(|mut m| {
                if !clear {
                    m.read_at = None;
                }
                m
            })
            .collect();
        let r = match marks.as_slice() {
            [] => Ok(None),
            [m] => self
                .api
                .put_read(&m.sub, &m.remote_id, m.version, m.read_at.as_deref())
                .map(Some),
            ms => self.api.put_reads(ms).map(|()| None),
        };
        match r {
            Ok(ack) => {
                done(self)?;
                let mut changed = false;
                if let (Some(ack), [m]) = (ack, marks.as_slice()) {
                    changed = self
                        .store
                        .read_acked(m, ack.stored, ack.read_version, clear)?;
                }
                if changed {
                    self.emit(CoreEvent::ReadingChanged);
                }
                Ok(())
            }
            Err(CoreError::NotFound) => gone(self),
            Err(CoreError::Rejected { status: 403, .. }) => denied(self),
            // `read_at` refused (a strict body on a server that said it
            // could clear): stop sending it and retry without.
            Err(CoreError::Rejected {
                status: 400,
                details,
                ..
            }) if clear && details.iter().any(|d| d.contains("read_at")) => {
                for o in &batch {
                    self.store.set_in_flight(o.seq, false)?;
                }
                self.set_read_clear(false)?;
                self.run_read(op)
            }
            Err(e) => {
                // The flush retries `op` later or drops it; the rest of the
                // batch waits for its own turn.
                for o in &batch[1..] {
                    self.store.set_in_flight(o.seq, false)?;
                }
                Err(e)
            }
        }
    }

    /// `None` when the server doesn't have `/api/reading` yet. The flag is
    /// true when the last page had `next: null` (the list is complete).
    fn fetch_reading(&self) -> Result<Option<(Vec<ReadingItem>, bool)>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        for i in 0..self.opts.reading_pages.max(1) {
            match self.api.reading(500, cursor.as_deref())? {
                // No fork extension: upstream's own reading routes.
                None if i == 0 => return self.fetch_reading_stock(),
                None => {
                    self.state().reading_unavailable = true;
                    self.set_read_sync(false)?;
                    return Ok(None);
                }
                Some(page) => {
                    {
                        let mut st = self.state();
                        st.reading_unavailable = false;
                        st.stock_reading = Some(false);
                    }
                    if i == 0 {
                        self.set_read_sync(page.read_sync())?;
                        self.set_read_clear(page.read_clear())?;
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

    /// The reading list from a stock server: upstream's `GET /api/reading`
    /// says which posts there are, and each new or changed one is read whole
    /// from `GET /api/imports/{sub}/{id}`. Thumbs and hopper memberships come
    /// from their own routes; read state stays on this Mac. Unchanged rows
    /// are the stored ones, with fresh thumbs and hoppers.
    fn fetch_reading_stock(&self) -> Result<Option<(Vec<ReadingItem>, bool)>> {
        let subs: std::collections::HashMap<String, (String, String)> = self
            .store
            .subscriptions()
            .into_iter()
            .map(|s| (s.id, (s.title, s.origin)))
            .collect();
        let thumbs = Api::optional_of(self.api.signals())?.unwrap_or_default();
        let hoppers = Api::optional_of(self.api.hopper_members())?.unwrap_or_default();
        let sweep = self
            .state()
            .stock_swept
            .is_none_or(|t| t.elapsed() >= STOCK_SWEEP);
        let max_pages = u64::from(self.opts.reading_pages.max(1)) * 10;
        let mut out = Vec::new();
        let mut fetched = 0;
        let mut deferred = false;
        let mut offset = 0u64;
        let mut reached_end = false;
        for _ in 0..max_pages {
            let Some(page) = self.api.stock_reading(offset)? else {
                self.state().reading_unavailable = true;
                return Ok(None);
            };
            let first = {
                let mut st = self.state();
                st.reading_unavailable = false;
                st.stock_reading.replace(true) != Some(true)
            };
            if offset == 0 {
                // Read state, when the server keeps it (studio's read-state
                // routes): `imported.readVersion` on each entry.
                self.set_read_sync(page.read_sync())?;
                self.set_read_clear(page.read_clear())?;
            }
            if first && self.store.meta(STOCK_TOLD).is_none() {
                self.store.set_meta(STOCK_TOLD, "1")?;
                self.emit(CoreEvent::ServerLimited);
            }
            let n = page.items.len() as u64;
            let mut any_new = false;
            for imp in page.items.into_iter().filter_map(|e| e.imported) {
                let key = (imp.subscription_id.clone(), imp.remote_id.clone());
                let thumb = thumbs.get(&key).copied();
                let hop = hoppers.get(&key).cloned().unwrap_or_default();
                let stored = self.store.reading_row(&key.0, &key.1).map(|(r, _)| r);
                // The importer bumps `observed_at` (and `updated`) with each
                // version, to the second; the signature catches a change
                // within the same second.
                let sig = imp.signature();
                let seen = self.state().stock_seen.get(&key).copied();
                let changed = seen.is_some_and(|s| s != sig)
                    || stored.as_ref().is_none_or(|r| {
                        r.observed_at != imp.observed_at
                            || r.updated != imp.updated
                            || (r.state == "tombstone") != imp.withdrawn
                            || (imp.withdrawn
                                && r.pinned_version_retained != imp.pinned_version_retained)
                    });
                // What the merge takes as the server's read state.
                let server_read = imp.read_version;
                if !changed {
                    self.state().stock_seen.insert(key.clone(), sig);
                    let mut r = stored.unwrap();
                    r.thumb = thumb;
                    r.hoppers = hop;
                    r.read_version = server_read;
                    out.push(r);
                    continue;
                }
                any_new = true;
                if fetched >= STOCK_FETCHES {
                    deferred = true;
                    // keep what's held until its turn
                    out.extend(stored.map(|mut r| {
                        r.read_version = server_read;
                        r
                    }));
                    continue;
                }
                fetched += 1;
                let (title, origin) = subs
                    .get(&key.0)
                    .cloned()
                    .unwrap_or_else(|| (imp.subscription_title.clone(), String::new()));
                let raw = match self.api.imported(&key.0, &key.1) {
                    Ok(v) => v,
                    // Gone between the list and now.
                    Err(CoreError::NotFound) => continue,
                    Err(e) => return Err(e),
                };
                self.state().stock_seen.insert(key.clone(), sig);
                match crate::api::wire::stock_row(&raw, &title, &origin, thumb, &hop, server_read) {
                    Some(mut r) => {
                        r.page = r
                            .page
                            .take()
                            .and_then(|p| crate::api::absolute_page(&r.origin, &p));
                        out.push(r);
                    }
                    // Doesn't read: keep what's held rather than lose it.
                    None => out.extend(stored.map(|mut r| {
                        r.read_version = server_read;
                        r
                    })),
                }
            }
            offset += n;
            if n == 0 || offset >= page.total {
                reached_end = true;
                break;
            }
            if !sweep && !any_new {
                break; // nothing new this far down: the rest is as held
            }
        }
        let complete = reached_end && !deferred;
        self.state().stock_deferred = deferred;
        if complete {
            self.state().stock_swept = Some(Instant::now());
        }
        Ok(Some((out, complete)))
    }

    /// Reading rows from extension 3 or a studio before 0.18 carry no
    /// lineage (who a post replies to or forks; upstream issue #12), so fetch
    /// the public item document of blyg posts that haven't been looked up yet: a few per pull, newest first, within
    /// a time budget, skipping a blyg for the rest of the pass once it fails.
    /// A fetched document is cached (the changelog cache), and the merge
    /// fills lineage from it from then on. True if any row changed.
    fn fill_lineage(
        &self,
        items: &[ReadingItem],
        complete: bool,
        kinds: &std::collections::HashMap<String, SubscriptionKind>,
    ) -> Result<bool> {
        let start = Instant::now();
        let mut failed: HashSet<String> = HashSet::new();
        let mut filled = false;
        let mut fetched = 0;
        for it in items {
            if fetched >= self.opts.lineage_fetches || start.elapsed() >= LINEAGE_BUDGET {
                break;
            }
            let blyg = kinds.get(&it.subscription_id) == Some(&SubscriptionKind::Blyg);
            if !blyg
                || it.state == "tombstone"
                || it.lineage_known
                || it.stub_of.is_some()
                || it.forked_from.is_some()
                || failed.contains(&it.origin)
                || self
                    .store
                    .cached_changelog(&it.origin, &it.remote_id)
                    .is_some()
            {
                continue;
            }
            fetched += 1;
            match self.public.item_doc(&it.origin, &it.remote_id) {
                Ok(doc) => {
                    let log = doc.versions_at(&it.origin);
                    self.store.put_changelog(&it.origin, &it.remote_id, &log)?;
                    filled |= Lineage::of_changelog(&log).is_some();
                }
                Err(_) => {
                    failed.insert(it.origin.clone());
                }
            }
        }
        if !filled {
            return Ok(false);
        }
        // Merge the whole pull again, as pulled: the merge fills lineage from
        // the cache it was just given. (A subset would read as "the server
        // dropped the rest" and prune them.)
        self.store.merge_reading(items, complete, kinds)
    }

    /// Whether the server has the fork's extensions, as of the last reading
    /// pull (`None` before one).
    pub fn server_extensions(&self) -> Option<bool> {
        self.state().stock_reading.map(|stock| !stock)
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
