//! In-memory `Backend` seeded with the mock's six items, so the UI runs with no
//! server (`BLYGGER_FAKE=1`). Local calls are instant; remote calls sleep to
//! simulate latency. Saves run the real status cycle: `Saving` immediately,
//! then after an 800 ms debounce and a ~350 ms "network" hop, `Synced`.
//!
//! Test hooks (not on the trait): [`FakeBackend::set_offline`] and
//! [`FakeBackend::trigger_conflict`]. The app binds them to ⌃⌥⌘O / ⌃⌥⌘C in
//! fake mode.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use blyg_core::*;
use chrono::{Duration as ChronoDuration, Utc};

#[path = "fake_reading.rs"]
pub mod reading_seed;
// --- profiles ---
#[path = "fake_profiles.rs"]
pub mod profile_seed;

type Sink = Arc<dyn Fn(CoreEvent) + Send + Sync>;

pub const ORIGIN: &str = "https://blyg.example.com";

// --- full editor: one imported post, so a thread can quote it ---
const SAMPLE_SUB: &str = "sub-shoreline";
const SAMPLE_ORIGIN: &str = "https://notes.example.org/";
/// A quotable blyg item id (26 chars of the blyg alphabet).
pub const SAMPLE_QUOTE_ID: &str = "01j9t4r7c8m2q5v6w3x8y9z0ab";

fn sample_reading() -> ReadingItem {
    let when = (Utc::now() - ChronoDuration::days(1)).to_rfc3339();
    ReadingItem {
        subscription_id: SAMPLE_SUB.into(),
        remote_id: SAMPLE_QUOTE_ID.into(),
        subscription_title: "Shoreline Notes".into(),
        origin: SAMPLE_ORIGIN.into(),
        kind: Kind::Fragment,
        state: "current".into(),
        version: 2,
        created: Some(when.clone()),
        updated: Some(when.clone()),
        observed_at: when,
        content_md: "A tide pool is a small ocean that forgets, twice a day, that it belongs to a larger one.".into(),
        content_html: "<p>A tide pool is a small ocean that forgets, twice a day, that it belongs to a larger one.</p>\n".into(),
        author: Some(Author {
            name: Some("Shoreline Notes".into()),
            url: None,
        }),
        page: None,
        thumb: None,
        hoppers: vec![],
        pinned_version_retained: None,
        read_version: None,
        stub_of: None,
        forked_from: None,
        transclusions: vec![],
    }
}

/// The full editor's sample thread (`BLYGGER_DEMO=studio`): a quote, a
/// generated TK span, a YouTube link and a quote that can't be resolved.
pub fn studio_sample() -> String {
    format!(
        "Notes from the tide pools\n\n\
         Low tide came at six, and the rock shelf opened like a *book*.\n\n\
         ![[{SAMPLE_QUOTE_ID}]]\n\n\
         [TK]one sentence on why the pools reset[=]Twice a day the sea refills every pool, so each low tide starts a fresh page.[/TK]\n\n\
         The best short film about it is still this one:\n\n\
         https://www.youtube.com/watch?v=Qa1b2C3d4E5\n\n\
         ![[0123456789abcdefghjkmnpqrs]]\n\n\
         More after the next [low tide](https://example.com/tides)."
    )
}

#[derive(Clone)]
pub struct Timing {
    pub debounce: Duration,
    pub network: Duration,
    pub publish: Duration,
    pub upload: Duration,
}

impl Timing {
    pub fn realistic() -> Self {
        Self {
            debounce: Duration::from_millis(800),
            network: Duration::from_millis(350),
            publish: Duration::from_millis(650),
            upload: Duration::from_millis(900),
        }
    }
    #[cfg(test)]
    pub fn instant() -> Self {
        Self {
            debounce: Duration::ZERO,
            network: Duration::ZERO,
            publish: Duration::ZERO,
            upload: Duration::ZERO,
        }
    }
}

struct State {
    items: Vec<Item>,
    versions: HashMap<LocalId, Vec<Version>>,
    /// Server-side copies of items in conflict.
    theirs: HashMap<LocalId, String>,
    /// What the "server" last acknowledged per item (base for fake conflicts).
    synced: HashMap<LocalId, String>,
    status: SyncStatus,
    offline: bool,
    sink: Option<Sink>,
    next_id: u64,
    /// TK provenance per item, keyed to the text it describes (as blyg-core tracks it).
    prov: HashMap<LocalId, (String, Vec<Option<ScopeProvenance>>)>,
    /// Every `save_with_provenance`: the item and its scope count (tests).
    prov_saves: Vec<(LocalId, usize)>,
    /// Simulate a Worker without the provenance extension (404).
    prov_unavailable: bool,
    /// Reading, subscriptions, mentions, site settings (see `fake_reading.rs`).
    rd: reading_seed::Seed,
    /// The server has the owner-API read extensions (test hook).
    read_ext: bool,
    // --- follow-ups --- (scratch media)
    /// `upload_media` calls that reached the "server" (tests).
    uploads: usize,
    /// Refuse uploads with a 415 (test hook).
    fail_uploads: bool,
    // --- profiles ---
    /// Profiles as last "fetched", by the URL asked and by origin.
    profiles: HashMap<String, Profile>,
    /// Profile fetches that reached the "network" (tests).
    profile_fetches: Vec<String>,
}

pub struct FakeBackend {
    state: Arc<Mutex<State>>,
    generation: Arc<AtomicU64>,
    timing: Timing,
    media_dir: Option<std::path::PathBuf>,
    /// Where scratch-note images are kept (`blyg_core::scratch_media`).
    scratch_dir: std::path::PathBuf,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self::with_timing(Timing::realistic())
    }

    pub fn with_timing(timing: Timing) -> Self {
        let now = Utc::now();
        let ago = |d: ChronoDuration| (now - d).to_rfc3339();
        let seed: [(&str, Kind, Status, u32, ChronoDuration, &str); 6] = [
            (
                "01J9QK3",
                Kind::Fragment,
                Status::Draft,
                0,
                ChronoDuration::minutes(2),
                "A lighthouse keeper's log is mostly weather, and that is the point.",
            ),
            (
                "01J9PX1",
                Kind::Fragment,
                Status::Public,
                1,
                ChronoDuration::days(2),
                "Band on Harbour Road: the trombone came in half a beat late and the tune was better for it.",
            ),
            (
                "01J9M2A",
                Kind::Fragment,
                Status::Public,
                3,
                ChronoDuration::days(4),
                "On friction: every extra step between noticing something and writing it down is a chance to lose it.",
            ),
            (
                "01J9K7T",
                Kind::Thread,
                Status::Public,
                2,
                ChronoDuration::days(6),
                "Notes on The Field Guide to Small Gardens\n\nThe guide argues that a garden is a *conversation* before it is a plan. Three things stuck with me…\n\n1. Soil matters more than seeds.\n2. Watering is a habit, not a chore.\n3. Weeds are **information**.\n\n> A garden is something you do together with the weather.\n\nMore in [the guide](https://example.com/field-guide).",
            ),
            (
                "01J9H4C",
                Kind::Thread,
                Status::Draft,
                0,
                ChronoDuration::days(9),
                "Slow mornings: a reply to a letter from a friend\n\nThe letter frames it as a question of time. I think it's rhythm…",
            ),
            (
                "01J9E8B",
                Kind::Fragment,
                Status::Public,
                1,
                ChronoDuration::days(13),
                "Marginalia were comment threads before paper had a word for it.",
            ),
        ];
        let rd = reading_seed::seed(now);
        let mut items = Vec::new();
        let mut versions = HashMap::new();
        for (id, kind, status, version, age, body) in seed {
            let local_id = LocalId(id.to_string());
            let public = status == Status::Public;
            let server = format!("{id}ABCDEFGHJKMNPQRS");
            if public {
                versions.insert(
                    local_id.clone(),
                    (1..=version)
                        .map(|v| Version {
                            version: v,
                            published_at: ago(age),
                            note: None,
                            pinned: false,
                            endcap: false,
                        })
                        .collect(),
                );
            }
            items.push(Item {
                local_id,
                server_id: Some(ServerId(server.clone())),
                kind,
                status,
                version,
                dirty: false,
                content_md: body.to_string(),
                created: ago(age),
                updated: ago(age),
                permalink: public.then(|| permalink(kind, &server)),
                stub_of: None,
                forked_from: None,
                show_responses: true,
                pending_sync: false,
                conflict: false,
            });
        }
        for (id, vs) in &rd.own_versions {
            versions.insert(id.clone(), vs.clone());
        }
        let synced = items
            .iter()
            .map(|i| (i.local_id.clone(), i.content_md.clone()))
            .collect();
        Self {
            state: Arc::new(Mutex::new(State {
                items,
                versions,
                theirs: HashMap::new(),
                synced,
                status: SyncStatus::Synced,
                offline: false,
                sink: None,
                next_id: 1,
                prov: HashMap::new(),
                prov_saves: Vec::new(),
                prov_unavailable: false,
                rd,
                // BLYGGER_FAKE_NO_READ=1: a server without the read extensions.
                read_ext: std::env::var_os("BLYGGER_FAKE_NO_READ").is_none(),
                uploads: 0,
                fail_uploads: false,
                profiles: HashMap::new(),    // --- profiles ---
                profile_fetches: Vec::new(), // --- profiles ---
            })),
            generation: Arc::new(AtomicU64::new(0)),
            timing,
            media_dir: media_cache_dir(),
            scratch_dir: blyg_core::scratch_media::dir(&blyg_core::config::data_dir()),
        }
    }

    /// Tests: keep uploads in memory only (don't touch the data dir).
    #[cfg(test)]
    pub fn without_media_cache(mut self) -> Self {
        self.media_dir = None;
        static N: AtomicU64 = AtomicU64::new(0);
        self.scratch_dir = std::env::temp_dir().join(format!(
            "blygger-fake-scratch-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        self
    }

    /// Tests: how many uploads reached the "server".
    #[cfg(test)]
    pub fn upload_count(&self) -> usize {
        self.lock().uploads
    }

    /// Tests: make every upload fail (415) or succeed again.
    #[cfg(test)]
    pub fn set_fail_uploads(&self, fail: bool) {
        self.lock().fail_uploads = fail;
    }

    /// Upload a scratch note's local images and swap in their URLs, as
    /// blyg-core's promotion does. Offline leaves them for later.
    fn upload_scratch_media(&self, id: &LocalId, content: &str) -> Result<()> {
        use blyg_core::scratch_media as sm;
        let mut done: Vec<(String, String)> = Vec::new();
        for name in sm::refs(content) {
            let url = format!("{}{name}", sm::SCHEME);
            let r = sm::file(&self.scratch_dir, &url)
                .ok_or_else(|| CoreError::Other(format!("an image ({name}) is missing")))
                .and_then(|p| std::fs::read(p).map_err(|e| CoreError::Storage(e.to_string())))
                .and_then(|bytes| self.upload_media(bytes, sm::mime_for(&name), None, None));
            match r {
                Ok(m) => done.push((name, sm::absolute(&m.url, ORIGIN))),
                Err(CoreError::Offline) => break,
                Err(e) => {
                    return Err(CoreError::Other(format!(
                        "an image couldn't be uploaded ({e})"
                    )));
                }
            }
        }
        if !done.is_empty() {
            let mut st = self.lock();
            if let Some(it) = st.items.iter_mut().find(|i| &i.local_id == id) {
                it.content_md = sm::rewrite(&it.content_md, &done);
            }
        }
        Ok(())
    }

    /// Tests / fake mode: behave like a Worker that answers 404 to the
    /// provenance endpoint (or not).
    pub fn set_provenance_available(&self, available: bool) {
        self.lock().prov_unavailable = !available;
    }

    /// Tests: every `save_with_provenance` so far, as (item, scope count).
    #[cfg(test)]
    pub fn provenance_saves(&self) -> Vec<(LocalId, usize)> {
        self.lock().prov_saves.clone()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(sink: &Option<Sink>, ev: CoreEvent) {
        if let Some(s) = sink {
            s(ev);
        }
    }

    fn sorted(items: &[Item]) -> Vec<Item> {
        let mut v = items.to_vec();
        v.sort_by(|a, b| b.updated.cmp(&a.updated));
        v
    }

    fn pending(st: &State) -> usize {
        st.items.iter().filter(|i| i.pending_sync).count()
    }

    fn set_status(st: &mut State, status: SyncStatus) -> Option<(Sink, SyncStatus)> {
        if st.status == status {
            return None;
        }
        st.status = status;
        st.sink.clone().map(|s| (s, status))
    }

    /// Mark a local change and schedule the debounced "push".
    fn touched(&self) {
        let fire = {
            let mut st = self.lock();
            let status = if st.offline {
                SyncStatus::Offline {
                    pending: Self::pending(&st),
                }
            } else {
                SyncStatus::Saving
            };
            Self::set_status(&mut st, status)
        };
        if let Some((sink, s)) = fire {
            sink(CoreEvent::SyncStatus(s));
        }
        if self.lock().offline {
            return;
        }
        self.schedule_flush();
    }

    fn schedule_flush(&self) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if self.timing.debounce.is_zero() && self.timing.network.is_zero() {
            // Zero-latency (tests): flush on the caller's thread so event
            // delivery stays deterministic.
            Self::flush(&self.state);
            return;
        }
        let gen_ref = self.generation.clone();
        let state = self.state.clone();
        let timing = self.timing.clone();
        thread::spawn(move || {
            thread::sleep(timing.debounce);
            if gen_ref.load(Ordering::SeqCst) != generation {
                return; // superseded by a newer keystroke
            }
            // The push is on the wire.
            let fire = {
                let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
                if st.offline {
                    None
                } else {
                    Self::set_status(&mut st, SyncStatus::Syncing)
                }
            };
            if let Some((sink, s)) = fire {
                sink(CoreEvent::SyncStatus(s));
            }
            thread::sleep(timing.network);
            if gen_ref.load(Ordering::SeqCst) != generation {
                return;
            }
            Self::flush(&state);
        });
    }

    /// The "server" acknowledges every pending change.
    fn flush(state: &Mutex<State>) {
        let (sink, changed) = {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            if st.offline {
                return;
            }
            let mut acked = Vec::new();
            for it in st.items.iter_mut() {
                if it.pending_sync {
                    acked.push((it.local_id.clone(), it.content_md.clone()));
                }
                it.pending_sync = false;
            }
            st.synced.extend(acked);
            let changed = Self::set_status(&mut st, SyncStatus::Synced).is_some();
            (st.sink.clone(), changed)
        };
        if changed {
            Self::emit(&sink, CoreEvent::SyncStatus(SyncStatus::Synced));
            Self::emit(&sink, CoreEvent::ItemsChanged);
        }
    }

    // ------------------------------------------------------------ test hooks

    pub fn is_offline(&self) -> bool {
        self.lock().offline
    }

    /// Simulate the network dropping / coming back. Coming back flushes.
    pub fn set_offline(&self, offline: bool) {
        let fire = {
            let mut st = self.lock();
            st.offline = offline;
            let status = if offline {
                SyncStatus::Offline {
                    pending: Self::pending(&st),
                }
            } else if Self::pending(&st) > 0 {
                SyncStatus::Saving
            } else {
                SyncStatus::Synced
            };
            Self::set_status(&mut st, status)
        };
        if let Some((sink, s)) = fire {
            sink(CoreEvent::SyncStatus(s));
        }
        if !offline {
            self.schedule_flush();
        }
    }

    /// Pretend the server copy of `id` changed underneath a local edit.
    pub fn trigger_conflict(&self, id: &LocalId) {
        let (sink, mine, theirs) = {
            let mut st = self.lock();
            let base = st.synced.get(id).cloned();
            let Some(it) = st.items.iter_mut().find(|i| &i.local_id == id) else {
                return;
            };
            it.conflict = true;
            let mine = it.content_md.clone();
            // "Someone else" edited the last synced copy.
            let base = base.unwrap_or_else(|| mine.clone());
            let mut lines = base.lines();
            let first = lines.next().unwrap_or("").replacen("The ", "", 1);
            let rest: Vec<&str> = lines.collect();
            let mut theirs = format!("{first}, part 1");
            if !rest.is_empty() {
                theirs.push('\n');
                theirs.push_str(&rest.join("\n"));
            }
            st.theirs.insert(id.clone(), theirs.clone());
            (st.sink.clone(), mine, theirs)
        };
        Self::emit(&sink, CoreEvent::ItemsChanged);
        Self::emit(
            &sink,
            CoreEvent::Conflict {
                local_id: id.clone(),
                mine,
                theirs,
            },
        );
    }

    /// A new local item (a draft, or a scratch note that never syncs).
    fn insert(&self, kind: Kind, content_md: &str, status: Status) -> LocalId {
        let mut st = self.lock();
        let n = st.next_id;
        st.next_id += 1;
        let id = LocalId(format!("local-{n}-{}", Utc::now().timestamp_millis()));
        let now = Utc::now().to_rfc3339();
        st.items.push(Item {
            local_id: id.clone(),
            server_id: None,
            kind,
            status,
            version: 0,
            dirty: false,
            content_md: content_md.to_string(),
            created: now.clone(),
            updated: now,
            permalink: None,
            stub_of: None,
            forked_from: None,
            show_responses: true,
            pending_sync: status != Status::Scratch,
            conflict: false,
        });
        id
    }

    /// Pretend the server lacks (or has) the owner-API read extensions.
    #[cfg(test)]
    pub fn set_read_extensions(&self, on: bool) {
        let sink = {
            let mut st = self.lock();
            st.read_ext = on;
            st.sink.clone()
        };
        Self::emit(&sink, CoreEvent::ReadingChanged);
    }

    /// Tests: change the reading list (then it's re-read, as after a sync).
    #[cfg(test)]
    pub fn with_reading(&self, f: impl FnOnce(&mut Vec<ReadingItem>)) {
        let sink = {
            let mut st = self.lock();
            f(&mut st.rd.reading);
            st.sink.clone()
        };
        Self::emit(&sink, CoreEvent::ReadingChanged);
    }

    /// Tests: the site settings as last saved.
    #[cfg(test)]
    pub fn saved_settings(&self) -> Settings {
        self.lock().rd.settings.clone()
    }

    // --- profiles ---

    /// Tests: every URL a profile was fetched for, in order.
    #[cfg(test)]
    pub fn profile_fetches(&self) -> Vec<String> {
        self.lock().profile_fetches.clone()
    }

    /// Connections from the sample reading list (and your own posts on your
    /// own profile), the way blyg-core computes them.
    fn finish_profile(&self, mut p: Profile) -> Profile {
        use blyg_core::profile::{connections, own_connections, same_origin};
        let st = self.lock();
        p.own = same_origin(&p.origin, ORIGIN);
        p.connections = if p.own {
            own_connections(&p.origin, &st.items, &st.rd.reading)
        } else {
            // Lineage the reading rows lack comes from the item documents
            // already "fetched" (the sample changelogs), as in LiveBackend.
            let mut held = st.rd.reading.clone();
            for r in held.iter_mut() {
                if let Some(l) = st
                    .rd
                    .changelogs
                    .get(&(r.subscription_id.clone(), r.remote_id.clone()))
                    .and_then(|c| blyg_core::Lineage::of_changelog(c))
                {
                    r.fill_lineage(l);
                }
            }
            connections(&p.origin, &held, &p.feed_quotes)
        };
        p
    }

    fn read_guard(&self) -> Result<()> {
        self.remote_guard()?;
        if self.lock().read_ext {
            Ok(())
        } else {
            Err(CoreError::NotFound)
        }
    }

    fn reading_changed(&self) {
        let sink = self.lock().sink.clone();
        Self::emit(&sink, CoreEvent::ReadingChanged);
    }

    fn remote_guard(&self) -> Result<()> {
        if self.lock().offline {
            Err(CoreError::Offline)
        } else {
            Ok(())
        }
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn permalink(kind: Kind, server_id: &str) -> String {
    let k = if kind == Kind::Thread { "t" } else { "f" };
    format!("{ORIGIN}/{k}/{server_id}")
}

/// Where uploaded media is cached locally (the app resolves `media/…` here first).
pub fn media_cache_dir() -> Option<std::path::PathBuf> {
    Some(blyg_core::config::data_dir().join("media"))
}

/// What a pasted URL resolves to: an `example.` address ending in `.xml`
/// (or mentioning rss) → a feed, any other `example.` address → a blyg,
/// anything else → nothing found.
fn fake_preview(url: &str) -> Result<SubscribePreview> {
    let u = url.trim();
    if !u.contains("example.") {
        return Err(CoreError::Rejected {
            status: 422,
            message: "no blyg or feed found at that address".into(),
            details: vec![],
        });
    }
    let host = u
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(u)
        .to_string();
    let origin = format!("https://{host}/");
    if u.ends_with(".xml") || u.contains("rss") {
        return Ok(SubscribePreview {
            kind: SubscriptionKind::Rss,
            title: format!("{host} (feed)"),
            origin: Some(origin),
            feed_url: Some(u.to_string()),
            site_mismatch: None,
        });
    }
    let mut title = host.split('.').next().unwrap_or("blyg").to_string();
    if let Some(f) = title.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    Ok(SubscribePreview {
        kind: SubscriptionKind::Blyg,
        title,
        feed_url: Some(format!("{origin}feed.json")),
        origin: Some(origin),
        site_mismatch: Some(false),
    })
}

fn ext_for(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" => "heic",
        "image/avif" => "avif",
        "image/tiff" => "tiff",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}

impl Backend for FakeBackend {
    fn items(&self) -> Vec<Item> {
        Self::sorted(&self.lock().items)
    }

    fn search(&self, query: &str) -> Vec<Item> {
        let q = query.trim().to_lowercase();
        let st = self.lock();
        if q.is_empty() {
            return Self::sorted(&st.items);
        }
        let hits: Vec<Item> = st
            .items
            .iter()
            .filter(|i| i.content_md.to_lowercase().contains(&q))
            .cloned()
            .collect();
        Self::sorted(&hits)
    }

    fn item(&self, id: &LocalId) -> Option<Item> {
        self.lock()
            .items
            .iter()
            .find(|i| &i.local_id == id)
            .cloned()
    }

    fn create_draft(&self, kind: Kind, content_md: &str) -> Result<LocalId> {
        let id = self.insert(kind, content_md, Status::Draft);
        self.touched();
        Ok(id)
    }

    fn save(&self, id: &LocalId, content_md: &str) -> Result<()> {
        let scratch = {
            let mut st = self.lock();
            let it = st
                .items
                .iter_mut()
                .find(|i| &i.local_id == id)
                .ok_or(CoreError::NotFound)?;
            if it.content_md == content_md {
                return Ok(());
            }
            it.content_md = content_md.to_string();
            it.updated = Utc::now().to_rfc3339();
            let scratch = it.status == Status::Scratch;
            it.pending_sync = !scratch;
            if it.status == Status::Public {
                it.dirty = true;
            }
            // Like blyg-core: tracked provenance follows its scopes.
            if let Some((old_md, old)) = st.prov.get(id).cloned()
                && let Some(new) = blyg_core::tk::remap(&old_md, &old, content_md)
            {
                st.prov.insert(id.clone(), (content_md.to_string(), new));
            }
            scratch
        };
        if !scratch {
            self.touched();
        }
        Ok(())
    }

    fn save_with_provenance(
        &self,
        id: &LocalId,
        content_md: &str,
        scopes: &[Option<ScopeProvenance>],
    ) -> Result<()> {
        blyg_core::tk::validate(content_md, scopes).map_err(|message| CoreError::Rejected {
            status: 400,
            message,
            details: vec![],
        })?;
        let scratch = {
            let mut st = self.lock();
            let it = st
                .items
                .iter_mut()
                .find(|i| &i.local_id == id)
                .ok_or(CoreError::NotFound)?;
            it.content_md = content_md.to_string();
            it.updated = Utc::now().to_rfc3339();
            let scratch = it.status == Status::Scratch;
            it.pending_sync = !scratch;
            if it.status == Status::Public {
                it.dirty = true;
            }
            st.prov
                .insert(id.clone(), (content_md.to_string(), scopes.to_vec()));
            st.prov_saves.push((id.clone(), scopes.len()));
            scratch
        };
        if !scratch {
            self.touched();
        }
        Ok(())
    }

    fn set_kind(&self, id: &LocalId, kind: Kind) -> Result<()> {
        let scratch = {
            let mut st = self.lock();
            let it = st
                .items
                .iter_mut()
                .find(|i| &i.local_id == id)
                .ok_or(CoreError::NotFound)?;
            if it.kind == kind {
                return Ok(());
            }
            it.kind = kind;
            it.updated = Utc::now().to_rfc3339();
            let scratch = it.status == Status::Scratch;
            it.pending_sync = !scratch;
            if it.status == Status::Public {
                it.dirty = true;
                if let Some(sid) = &it.server_id {
                    it.permalink = Some(permalink(kind, &sid.0));
                }
            }
            scratch
        };
        if !scratch {
            self.touched();
        }
        Ok(())
    }

    // --- scratch notes ---

    fn create_scratch(&self, kind: Kind, content_md: &str) -> Result<LocalId> {
        let id = self.insert(kind, content_md, Status::Scratch);
        let sink = self.lock().sink.clone();
        Self::emit(&sink, CoreEvent::ItemsChanged);
        Ok(id)
    }

    fn promote(&self, id: &LocalId, to: Promote) -> Result<Promoted> {
        // --- follow-ups --- local images go up first.
        if let Some(it) = self.item(id).filter(|i| i.status == Status::Scratch) {
            self.upload_scratch_media(id, &it.content_md)?;
        }
        let (kind, promoted) = {
            let mut st = self.lock();
            let it = st
                .items
                .iter_mut()
                .find(|i| &i.local_id == id)
                .ok_or(CoreError::NotFound)?;
            if it.status == Status::Scratch {
                it.kind = promotion_kind(it.kind, &it.content_md);
                it.status = Status::Draft;
                it.pending_sync = true;
                it.updated = Utc::now().to_rfc3339();
                (it.kind, true)
            } else {
                (it.kind, false)
            }
        };
        if promoted {
            self.touched();
            let sink = self.lock().sink.clone();
            Self::emit(&sink, CoreEvent::ItemsChanged);
        }
        let published = match to {
            Promote::Draft => None,
            Promote::Publish { note } => Some(self.publish(id, note.as_deref())?),
        };
        Ok(Promoted { kind, published })
    }

    fn sync_status(&self) -> SyncStatus {
        self.lock().status
    }

    fn base_url(&self) -> Option<String> {
        Some(ORIGIN.to_string())
    }

    fn tracked_tk_provenance(
        &self,
        id: &LocalId,
    ) -> Option<(String, Vec<Option<ScopeProvenance>>)> {
        self.lock().prov.get(id).cloned()
    }

    fn provenance_available(&self) -> bool {
        !self.lock().prov_unavailable
    }

    fn set_event_sink(&self, sink: Box<dyn Fn(CoreEvent) + Send + Sync>) {
        self.lock().sink = Some(Arc::from(sink));
    }

    fn resolve_conflict(&self, id: &LocalId, how: Resolution) -> Result<()> {
        let extra = {
            let mut st = self.lock();
            let theirs = st.theirs.remove(id).unwrap_or_default();
            let it = st
                .items
                .iter_mut()
                .find(|i| &i.local_id == id)
                .ok_or(CoreError::NotFound)?;
            it.conflict = false;
            match how {
                Resolution::KeepMine => None,
                Resolution::TakeServer => {
                    it.content_md = theirs;
                    None
                }
                Resolution::KeepBoth => {
                    let mine = std::mem::replace(&mut it.content_md, theirs);
                    Some((it.kind, mine))
                }
            }
        };
        if let Some((kind, mine)) = extra {
            self.create_draft(kind, &mine)?;
        }
        let sink = self.lock().sink.clone();
        Self::emit(&sink, CoreEvent::ItemsChanged);
        Ok(())
    }

    fn reading(&self) -> Vec<ReadingItem> {
        let st = self.lock();
        if !st.read_ext {
            return Vec::new();
        }
        let mut v = st.rd.reading.clone();
        // The full editor's sample thread quotes this imported post.
        v.push(sample_reading());
        v
    }

    fn subscriptions(&self) -> Vec<Subscription> {
        let mut v = self.lock().rd.subs.clone();
        v.push(Subscription {
            id: SAMPLE_SUB.into(),
            kind: SubscriptionKind::Blyg,
            origin: SAMPLE_ORIGIN.into(),
            feed_url: format!("{SAMPLE_ORIGIN}items.json"),
            title: "Shoreline Notes".into(),
            status: "active".into(),
            in_blogroll: false,
        });
        v
    }

    fn mark_read(&self, sub_id: &str, remote_id: &str) -> Result<()> {
        let changed = {
            let mut st = self.lock();
            let mut changed = false;
            for r in st.rd.reading.iter_mut() {
                if r.subscription_id == sub_id
                    && r.remote_id == remote_id
                    && r.read_version != Some(r.version)
                {
                    r.read_version = Some(r.version);
                    changed = true;
                }
            }
            changed
        };
        if changed {
            self.reading_changed();
        }
        Ok(())
    }

    fn publish(&self, id: &LocalId, note: Option<&str>) -> Result<PublishOutcome> {
        if self.item(id).is_some_and(|i| i.status == Status::Scratch) {
            let note = note.map(str::to_string);
            return self
                .promote(id, Promote::Publish { note })?
                .published
                .ok_or_else(|| CoreError::Other("not published".into()));
        }
        thread::sleep(self.timing.publish);
        self.remote_guard()?;
        let (outcome, sink) = {
            let mut st = self.lock();
            let n = st.next_id;
            st.next_id += 1;
            let it = st
                .items
                .iter_mut()
                .find(|i| &i.local_id == id)
                .ok_or(CoreError::NotFound)?;
            if it.over_limit() {
                return Err(CoreError::Rejected {
                    status: 400,
                    message: "fragment exceeds 1000 characters".into(),
                    details: vec![],
                });
            }
            let sid = it
                .server_id
                .get_or_insert_with(|| ServerId(format!("01J9R{n:021}")))
                .0
                .clone();
            it.version += 1;
            it.status = Status::Public;
            it.dirty = false;
            it.pending_sync = false;
            it.updated = Utc::now().to_rfc3339();
            let link = permalink(it.kind, &sid);
            it.permalink = Some(link.clone());
            let version = it.version;
            st.versions.entry(id.clone()).or_default().push(Version {
                version,
                published_at: Utc::now().to_rfc3339(),
                note: note.map(str::to_string),
                pinned: false,
                endcap: false,
            });
            (
                PublishOutcome {
                    version,
                    permalink: link,
                    warning: None,
                },
                st.sink.clone(),
            )
        };
        Self::emit(&sink, CoreEvent::ItemsChanged);
        Ok(outcome)
    }

    fn withdraw(&self, id: &LocalId, note: Option<&str>) -> Result<u32> {
        thread::sleep(self.timing.publish);
        self.remote_guard()?;
        let mut st = self.lock();
        let it = st
            .items
            .iter_mut()
            .find(|i| &i.local_id == id)
            .ok_or(CoreError::NotFound)?;
        it.status = Status::Withdrawn;
        it.version += 1;
        let v = it.version;
        // The withdraw marker: an endcap version carrying the note.
        st.versions.entry(id.clone()).or_default().push(Version {
            version: v,
            published_at: Utc::now().to_rfc3339(),
            note: note.map(str::to_string),
            pinned: false,
            endcap: true,
        });
        Ok(v)
    }

    fn pin(&self, id: &LocalId, version: u32) -> Result<()> {
        thread::sleep(self.timing.network);
        self.remote_guard()?;
        let mut st = self.lock();
        let v = st
            .versions
            .get_mut(id)
            .and_then(|vs| vs.iter_mut().find(|v| v.version == version))
            .ok_or(CoreError::NotFound)?;
        v.pinned = true;
        Ok(())
    }

    fn versions(&self, id: &LocalId) -> Result<Vec<Version>> {
        thread::sleep(self.timing.network);
        self.remote_guard()?;
        Ok(self.lock().versions.get(id).cloned().unwrap_or_default())
    }

    fn restore(&self, id: &LocalId, version: u32) -> Result<()> {
        thread::sleep(self.timing.network);
        self.remote_guard()?;
        let sink = {
            let mut st = self.lock();
            let text = st
                .rd
                .own_texts
                .get(&(id.clone(), version))
                .cloned()
                .unwrap_or_else(|| format!("(the text of v{version})"));
            let it = st
                .items
                .iter_mut()
                .find(|i| &i.local_id == id)
                .ok_or(CoreError::NotFound)?;
            it.dirty = it.version != version;
            it.content_md = text;
            it.updated = Utc::now().to_rfc3339();
            st.sink.clone()
        };
        Self::emit(&sink, CoreEvent::ItemsChanged);
        Ok(())
    }

    fn delete_draft(&self, id: &LocalId) -> Result<()> {
        let mut st = self.lock();
        let before = st.items.len();
        st.items.retain(|i| {
            !(&i.local_id == id && matches!(i.status, Status::Draft | Status::Scratch))
        });
        if st.items.len() == before {
            return Err(CoreError::Rejected {
                status: 400,
                message: "only drafts can be deleted".into(),
                details: vec![],
            });
        }
        Ok(())
    }

    fn upload_media(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        _item: Option<&LocalId>,
        _alt: Option<&str>,
    ) -> Result<MediaRef> {
        thread::sleep(self.timing.upload);
        self.remote_guard()?;
        let n = {
            let mut st = self.lock();
            if st.fail_uploads {
                return Err(CoreError::Rejected {
                    status: 415,
                    message: "unsupported type".into(),
                    details: vec![],
                });
            }
            st.uploads += 1;
            st.next_id += 1;
            st.next_id
        };
        let name = format!(
            "01J9M{:07}{}.{}",
            n,
            Utc::now().timestamp_millis() % 100_000,
            ext_for(mime)
        );
        if let Some(dir) = &self.media_dir {
            std::fs::create_dir_all(dir).map_err(|e| CoreError::Storage(e.to_string()))?;
            std::fs::write(dir.join(&name), &bytes)
                .map_err(|e| CoreError::Storage(e.to_string()))?;
        }
        Ok(MediaRef {
            url: format!("media/{name}"),
            mime: mime.to_string(),
        })
    }

    fn fork(&self, of: &RemoteRef) -> Result<LocalId> {
        thread::sleep(self.timing.network);
        self.remote_guard()?;
        let text = {
            let st = self.lock();
            st.rd
                .pins
                .values()
                .find(|p| p.origin == of.origin && p.id == of.id && p.version == of.version)
                .map(|p| p.content_md.clone())
                .ok_or(CoreError::Rejected {
                    status: 404,
                    message: "not pinned".into(),
                    details: vec![],
                })?
        };
        let id = self.create_draft(Kind::Thread, &text)?;
        if let Some(it) = self.lock().items.iter_mut().find(|i| i.local_id == id) {
            it.forked_from = Some(of.clone());
        }
        Ok(id)
    }

    fn set_show_responses(&self, id: &LocalId, show: bool) -> Result<()> {
        let mut st = self.lock();
        let it = st
            .items
            .iter_mut()
            .find(|i| &i.local_id == id)
            .ok_or(CoreError::NotFound)?;
        it.show_responses = show;
        Ok(())
    }

    fn sync_now(&self) -> Result<()> {
        self.remote_guard()
    }

    fn preview_subscription(&self, url: &str) -> Result<SubscribePreview> {
        thread::sleep(self.timing.network);
        self.remote_guard()?;
        fake_preview(url)
    }

    fn subscribe(&self, url: &str, title: Option<&str>) -> Result<Subscription> {
        thread::sleep(self.timing.network);
        self.remote_guard()?;
        let p = fake_preview(url)?;
        let sub = {
            let mut st = self.lock();
            st.next_id += 1;
            let sub = Subscription {
                id: format!("sub-{}", st.next_id),
                kind: p.kind,
                origin: p.origin.clone().unwrap_or_else(|| url.to_string()),
                feed_url: p.feed_url.clone().unwrap_or_else(|| url.to_string()),
                title: title
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .unwrap_or(&p.title)
                    .to_string(),
                status: "active".into(),
                in_blogroll: false,
            };
            st.rd.subs.push(sub.clone());
            sub
        };
        self.reading_changed();
        Ok(sub)
    }

    fn unsubscribe(&self, sub_id: &str) -> Result<()> {
        self.remote_guard()?;
        {
            let mut st = self.lock();
            st.rd.subs.retain(|s| s.id != sub_id);
            st.rd.reading.retain(|r| r.subscription_id != sub_id);
        }
        self.reading_changed();
        Ok(())
    }

    fn set_subscription(
        &self,
        sub_id: &str,
        in_blogroll: Option<bool>,
        title: Option<&str>,
    ) -> Result<()> {
        self.remote_guard()?;
        {
            let mut st = self.lock();
            let s = st
                .rd
                .subs
                .iter_mut()
                .find(|s| s.id == sub_id)
                .ok_or(CoreError::NotFound)?;
            if let Some(b) = in_blogroll {
                s.in_blogroll = b;
            }
            if let Some(t) = title {
                s.title = t.to_string();
            }
        }
        self.reading_changed();
        Ok(())
    }

    fn pause_subscription(&self, sub_id: &str, paused: bool) -> Result<()> {
        self.remote_guard()?;
        {
            let mut st = self.lock();
            let s = st
                .rd
                .subs
                .iter_mut()
                .find(|s| s.id == sub_id)
                .ok_or(CoreError::NotFound)?;
            s.status = if paused { "paused" } else { "active" }.into();
        }
        self.reading_changed();
        Ok(())
    }

    fn signal(&self, sub_id: &str, remote_id: &str, thumb: Option<i8>) -> Result<()> {
        self.remote_guard()?;
        {
            let mut st = self.lock();
            for r in st
                .rd
                .reading
                .iter_mut()
                .filter(|r| r.subscription_id == sub_id && r.remote_id == remote_id)
            {
                r.thumb = thumb;
            }
        }
        self.reading_changed();
        Ok(())
    }

    fn mentions(&self) -> Result<Vec<Mention>> {
        thread::sleep(self.timing.network);
        self.read_guard()?;
        Ok(self.lock().rd.mentions.clone())
    }

    fn set_mention_hidden(&self, mention_id: &str, hidden: bool) -> Result<()> {
        self.read_guard()?;
        let mut st = self.lock();
        let m = st
            .rd
            .mentions
            .iter_mut()
            .find(|m| m.id == mention_id)
            .ok_or(CoreError::NotFound)?;
        m.hidden = hidden;
        Ok(())
    }

    fn settings(&self) -> Result<Settings> {
        thread::sleep(self.timing.network);
        self.read_guard()?;
        Ok(self.lock().rd.settings.clone())
    }

    fn save_settings(&self, settings: &Settings) -> Result<()> {
        thread::sleep(self.timing.network);
        self.remote_guard()?;
        self.lock().rd.settings = settings.clone();
        Ok(())
    }

    fn remote_versions(&self, sub_id: &str, remote_id: &str) -> Result<Vec<RemoteVersion>> {
        thread::sleep(self.timing.network);
        self.remote_guard()?;
        let st = self.lock();
        if let Some(log) = st
            .rd
            .changelogs
            .get(&(sub_id.to_string(), remote_id.to_string()))
        {
            return Ok(log.clone());
        }
        let it = st
            .rd
            .reading
            .iter()
            .find(|r| r.subscription_id == sub_id && r.remote_id == remote_id)
            .ok_or(CoreError::NotFound)?;
        Ok(vec![RemoteVersion {
            version: it.version,
            at: it.updated.clone().unwrap_or_else(|| it.observed_at.clone()),
            note: None,
            pinned: false,
            current: true,
            media: vec![],
            lineage: Default::default(),
        }])
    }

    fn remote_pinned(&self, sub_id: &str, remote_id: &str, version: u32) -> Result<PinnedVersion> {
        thread::sleep(self.timing.network);
        self.remote_guard()?;
        self.lock()
            .rd
            .pins
            .get(&(sub_id.to_string(), remote_id.to_string(), version))
            .cloned()
            .ok_or(CoreError::Rejected {
                status: 404,
                message: "not pinned".into(),
                details: vec![],
            })
    }

    fn pinned_diff_base(&self, sub_id: &str, remote_id: &str) -> Option<PinnedVersion> {
        let read = self
            .lock()
            .rd
            .reading
            .iter()
            .find(|r| r.subscription_id == sub_id && r.remote_id == remote_id)?
            .read_version?;
        self.remote_pinned(sub_id, remote_id, read).ok()
    }

    // --- reading ---

    fn read_extensions_available(&self) -> bool {
        self.lock().read_ext
    }

    // --- scratch media ---

    fn save_scratch_media(&self, bytes: &[u8], mime: &str) -> Result<String> {
        blyg_core::scratch_media::store(&self.scratch_dir, bytes, mime)
    }

    fn scratch_media_file(&self, url: &str) -> Option<std::path::PathBuf> {
        blyg_core::scratch_media::file(&self.scratch_dir, url)
    }

    fn create_stub(&self, of: &RemoteRef, content_md: &str) -> Result<LocalId> {
        let id = self.create_draft(Kind::Thread, content_md)?;
        if let Some(it) = self.lock().items.iter_mut().find(|i| i.local_id == id) {
            it.stub_of = Some(of.clone());
        }
        Ok(id)
    }

    // --- profiles ---

    fn profile(&self, url: &str, refresh: bool) -> Result<Profile> {
        let key = blyg_core::profile::clean_url(url).unwrap_or_else(|| url.to_string());
        // Bind first: a guard in an `if let`/`match` scrutinee would be held
        // across `finish_profile`, which locks again.
        let cached = self.lock().profiles.get(&key).cloned();
        if !refresh && let Some(p) = cached {
            return Ok(self.finish_profile(p));
        }
        thread::sleep(self.timing.network);
        if let Err(e) = self.remote_guard() {
            let cached = self.lock().profiles.get(&key).cloned();
            return match cached {
                Some(mut p) => {
                    p.stale = true;
                    Ok(self.finish_profile(p))
                }
                None => Err(e),
            };
        }
        let (settings, subs) = {
            let st = self.lock();
            (st.rd.settings.clone(), st.rd.subs.clone())
        };
        let mut p = profile_seed::profile_for(&key, ORIGIN, &settings, &subs)?;
        p.fetched_at = Utc::now().timestamp_millis();
        {
            let mut st = self.lock();
            st.profile_fetches.push(key.clone());
            st.profiles.insert(key, p.clone());
            st.profiles.insert(p.origin.clone(), p.clone());
        }
        Ok(self.finish_profile(p))
    }

    fn cached_profile(&self, url: &str) -> Option<Profile> {
        let key = blyg_core::profile::clean_url(url)?;
        let p = self.lock().profiles.get(&key).cloned()?;
        Some(self.finish_profile(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn fake() -> (FakeBackend, mpsc::Receiver<CoreEvent>) {
        let mut b = FakeBackend::with_timing(Timing::instant());
        b.media_dir = None; // don't write into the data dir from tests
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        b.set_event_sink(Box::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }));
        (b, rx)
    }

    fn wait_for(rx: &mpsc::Receiver<CoreEvent>, want: CoreEvent) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(e) if e == want => return true,
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
        false
    }

    #[test]
    fn seeded_newest_first_and_search() {
        let (b, _) = fake();
        let items = b.items();
        assert_eq!(items.len(), 6);
        assert!(items[0].title().starts_with("A lighthouse"));
        assert_eq!(b.search("HARB").len(), 1);
        assert_eq!(b.search("").len(), 6);
        assert!(b.search("zzz").is_empty());
    }

    #[test]
    fn save_cycles_saving_then_synced() {
        let (b, rx) = fake();
        let id = b.items()[0].local_id.clone();
        b.save(&id, "changed").unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            CoreEvent::SyncStatus(SyncStatus::Saving)
        );
        assert!(wait_for(&rx, CoreEvent::SyncStatus(SyncStatus::Synced)));
        let it = b.item(&id).unwrap();
        assert_eq!(it.content_md, "changed");
        assert!(!it.pending_sync);
        assert_eq!(b.items()[0].local_id, id, "most recently updated first");
    }

    #[test]
    fn editing_public_marks_dirty_and_publish_bumps_version() {
        let (b, _) = fake();
        let id = LocalId("01J9M2A".into());
        b.save(&id, "On friction, revised.").unwrap();
        assert!(b.item(&id).unwrap().dirty);
        let out = b.publish(&id, Some("typo")).unwrap();
        assert_eq!(out.version, 4);
        assert!(out.permalink.contains("/f/"));
        let it = b.item(&id).unwrap();
        assert!(!it.dirty);
        assert_eq!(
            b.versions(&id).unwrap().last().unwrap().note.as_deref(),
            Some("typo")
        );
    }

    #[test]
    fn offline_queues_and_blocks_remote() {
        let (b, rx) = fake();
        b.set_offline(true);
        let id = b.items()[0].local_id.clone();
        b.save(&id, "on a plane").unwrap();
        assert!(wait_for(
            &rx,
            CoreEvent::SyncStatus(SyncStatus::Offline { pending: 1 })
        ));
        assert!(matches!(b.publish(&id, None), Err(CoreError::Offline)));
        b.set_offline(false);
        assert!(wait_for(&rx, CoreEvent::SyncStatus(SyncStatus::Synced)));
    }

    #[test]
    fn over_limit_fragment_rejected() {
        let (b, _) = fake();
        let id = b.create_draft(Kind::Fragment, &"x".repeat(1001)).unwrap();
        assert!(matches!(
            b.publish(&id, None),
            Err(CoreError::Rejected { status: 400, .. })
        ));
        b.set_kind(&id, Kind::Thread).unwrap();
        assert!(b.publish(&id, None).unwrap().permalink.contains("/t/"));
    }

    #[test]
    fn conflict_resolution() {
        let (b, rx) = fake();
        let id = LocalId("01J9K7T".into());
        b.trigger_conflict(&id);
        let mut theirs = None;
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(1)) {
            if let CoreEvent::Conflict { theirs: t, .. } = e {
                theirs = Some(t);
                break;
            }
        }
        let theirs = theirs.expect("conflict event");
        assert!(theirs.starts_with("Notes on Field Guide to Small Gardens, part 1"));
        assert!(b.item(&id).unwrap().conflict);
        let before = b.items().len();
        b.resolve_conflict(&id, Resolution::KeepBoth).unwrap();
        assert_eq!(b.items().len(), before + 1);
        assert_eq!(b.item(&id).unwrap().content_md, theirs);
        assert!(!b.item(&id).unwrap().conflict);
    }

    #[test]
    fn upload_returns_relative_media_url() {
        let (b, _) = fake();
        let m = b
            .upload_media(vec![1, 2, 3], "image/png", None, None)
            .unwrap();
        assert!(m.url.starts_with("media/") && m.url.ends_with(".png"));
    }
}
