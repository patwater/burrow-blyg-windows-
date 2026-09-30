//! Which backend the app runs on, and switching it at runtime.
//!
//! The UI (main window and quick capture) holds one `Arc<dyn Backend>`: a
//! `SwitchBackend` that delegates to the current backend. Connecting or
//! disconnecting a blyg swaps what's inside, so nothing else needs rewiring.
//!
//! - `BLYGGER_FAKE=1` → the seeded `FakeBackend` (in-memory token store).
//! - a `blyg-url` in the config and a token for its host → `LiveBackend`.
//! - otherwise → `Disconnected` (empty, refuses writes) until the "Connect
//!   your blyg" sheet succeeds.

use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use blyg_core::config::TokenStore;
use blyg_core::*;

type Sink = Arc<dyn Fn(CoreEvent) + Send + Sync>;

/// What the switch currently holds (for the UI's own decisions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Fake,
    Live,
    Disconnected,
}

pub struct SwitchBackend {
    inner: RwLock<(Arc<dyn Backend>, Mode)>,
    sink: Mutex<Option<Sink>>,
}

impl SwitchBackend {
    pub fn new(inner: Arc<dyn Backend>, mode: Mode) -> Arc<SwitchBackend> {
        Arc::new(SwitchBackend {
            inner: RwLock::new((inner, mode)),
            sink: Mutex::new(None),
        })
    }

    fn cur(&self) -> Arc<dyn Backend> {
        self.inner
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .0
            .clone()
    }

    pub fn mode(&self) -> Mode {
        self.inner.read().unwrap_or_else(|p| p.into_inner()).1
    }

    /// Replace the backend. The event sink moves over, and the UI hears
    /// `ItemsChanged` + the new sync status. `after` runs once the old
    /// backend is gone (on a background thread: dropping a `LiveBackend`
    /// joins its sync thread), e.g. to delete its database.
    pub fn switch(
        &self,
        next: Arc<dyn Backend>,
        mode: Mode,
        after: impl FnOnce() + Send + 'static,
    ) {
        let sink = self.sink.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(s) = &sink {
            let s = s.clone();
            next.set_event_sink(Box::new(move |e| s(e)));
        }
        let old = {
            let mut g = self.inner.write().unwrap_or_else(|p| p.into_inner());
            std::mem::replace(&mut *g, (next.clone(), mode))
        };
        if let Some(s) = sink {
            s(CoreEvent::ItemsChanged);
            s(CoreEvent::ReadingChanged);
            s(CoreEvent::SyncStatus(next.sync_status()));
        }
        std::thread::spawn(move || {
            drop(old);
            after();
        });
    }
}

/// Which blyg the local database in the data dir belongs to.
const DB_OWNER_FILE: &str = "blygger.db.blyg-url";

/// Open the live backend for `url` if the token store has a token for it.
/// A local database that belongs to a *different* blyg is moved aside first
/// (`blygger.<host>.db`), so one blyg's unpushed drafts can never be pushed
/// to another.
pub fn open_live(
    data_dir: &Path,
    url: &str,
    tokens: &dyn TokenStore,
) -> Result<Option<Arc<dyn Backend>>> {
    let Some(token) = tokens.get(url)? else {
        return Ok(None);
    };
    std::fs::create_dir_all(data_dir).map_err(|e| CoreError::Storage(e.to_string()))?;
    let owner = data_dir.join(DB_OWNER_FILE);
    let db = data_dir.join(blyg_core::live::DB_FILE);
    if let Ok(prev) = std::fs::read_to_string(&owner)
        && prev.trim() != url
        && db.exists()
    {
        let host = crate::vm::url_host(prev.trim()).unwrap_or_else(|| "previous".into());
        for suffix in ["", "-wal", "-shm"] {
            let from = data_dir.join(format!("{}{suffix}", blyg_core::live::DB_FILE));
            if from.exists() {
                let to = data_dir.join(format!("blygger.{host}.db{suffix}"));
                std::fs::rename(&from, &to).map_err(|e| CoreError::Storage(e.to_string()))?;
            }
        }
    }
    let live = LiveBackend::open(data_dir, url, &token)?;
    std::fs::write(&owner, url).map_err(|e| CoreError::Storage(e.to_string()))?;
    Ok(Some(Arc::new(live)))
}

/// Delete the local copy (database, its WAL, cached themes and images).
pub fn delete_local_data(data_dir: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let _ =
            std::fs::remove_file(data_dir.join(format!("{}{suffix}", blyg_core::live::DB_FILE)));
    }
    let _ = std::fs::remove_file(data_dir.join(DB_OWNER_FILE));
    // The retired native preview's image cache, from older versions.
    let _ = std::fs::remove_dir_all(crate::images::cache_dir(data_dir));
    crate::app::studio::style_cache::forget(data_dir);
}

// ------------------------------------------------------------ app global

/// Checks a URL + token before anything is saved (`GET /api/items`).
pub type Verifier = fn(&str, &str) -> std::result::Result<usize, blyg_core::api::ConnectError>;

/// The app's connection state (absent in headless tests, which run on a
/// `FakeBackend` directly).
pub struct Connection {
    pub switch: Arc<SwitchBackend>,
    pub data_dir: std::path::PathBuf,
    pub verify: Verifier,
}

impl gpui_kit::Global for Connection {}

pub fn mode(cx: &gpui_kit::App) -> Option<Mode> {
    cx.try_global::<Connection>().map(|c| c.switch.mode())
}

/// The connect sheet's check. Without the global (tests) it never touches
/// the network.
pub fn verifier(cx: &gpui_kit::App) -> Verifier {
    cx.try_global::<Connection>()
        .map(|c| c.verify)
        .unwrap_or(|_, _| Ok(0))
}

/// Verifier for fake mode: never touches the network.
pub fn fake_verifier(_: &str, _: &str) -> std::result::Result<usize, blyg_core::api::ConnectError> {
    Ok(6)
}

/// After a successful connect: switch to the live backend for `url`.
/// `Ok(false)` when there's nothing to switch (fake mode, tests).
pub fn go_live(url: &str, cx: &mut gpui_kit::App) -> Result<bool> {
    let Some(conn) = cx.try_global::<Connection>() else {
        return Ok(false);
    };
    // --- onboarding --- (a sample-data session still goes live on connect)
    if conn.switch.mode() == Mode::Fake && !cx.has_global::<SampleSession>() {
        return Ok(false);
    }
    let (switch, dir) = (conn.switch.clone(), conn.data_dir.clone());
    let tokens = crate::settings::get(cx).tokens.clone();
    match open_live(&dir, url, &*tokens)? {
        Some(live) => {
            switch.switch(live, Mode::Live, || {});
            Ok(true)
        }
        None => Err(CoreError::Other("the token wasn't saved".into())),
    }
}

// --- onboarding ---
impl SwitchBackend {
    /// What the switch holds right now (the tutorial parks it and puts it back).
    pub fn current(&self) -> Arc<dyn Backend> {
        self.cur()
    }
}

/// This session runs on sample data because onboarding's "just try it"
/// chose it (not `BLYGGER_FAKE`): connecting a blyg still goes live.
pub struct SampleSession;

impl gpui_kit::Global for SampleSession {}

/// Onboarding's "Skip, just try it with sample data": run this session on
/// `fake`. False without the app global (headless tests). Already fake
/// (`BLYGGER_FAKE`) → nothing to switch.
pub fn use_sample_data(fake: Arc<dyn Backend>, cx: &mut gpui_kit::App) -> bool {
    let Some(conn) = cx.try_global::<Connection>() else {
        return false;
    };
    if conn.switch.mode() != Mode::Fake {
        conn.switch.clone().switch(fake, Mode::Fake, || {});
        cx.set_global(SampleSession);
    }
    true
}
// --- end onboarding ---

/// Forget the token (Keychain) and `blyg-url` (config); keep or delete the
/// local copy. Returns the host that was disconnected.
pub fn disconnect(
    delete_local: bool,
    cx: &mut gpui_kit::App,
) -> std::result::Result<String, String> {
    let url = crate::settings::blyg_url(cx).ok_or("Not connected to a blyg")?;
    let host = crate::vm::url_host(&url).unwrap_or(url.clone());
    crate::settings::get(cx)
        .tokens
        .delete(&url)
        .map_err(|e| format!("Couldn't remove the token: {e}"))?;
    crate::settings::write(&[("blyg-url", blyg_core::config::Change::Remove)], cx)?;
    if let Some(conn) = cx.try_global::<Connection>()
        && conn.switch.mode() != Mode::Fake
    {
        let dir = conn.data_dir.clone();
        conn.switch
            .switch(Arc::new(Disconnected), Mode::Disconnected, move || {
                if delete_local {
                    delete_local_data(&dir);
                }
            });
    }
    Ok(host)
}

/// Nothing connected yet: an empty list, and every write explains why.
pub struct Disconnected;

fn not_connected<T>() -> Result<T> {
    Err(CoreError::Other("Connect your blyg first".into()))
}

impl Backend for Disconnected {
    fn items(&self) -> Vec<Item> {
        vec![]
    }
    fn search(&self, _: &str) -> Vec<Item> {
        vec![]
    }
    fn item(&self, _: &LocalId) -> Option<Item> {
        None
    }
    fn create_draft(&self, _: Kind, _: &str) -> Result<LocalId> {
        not_connected()
    }
    fn save(&self, _: &LocalId, _: &str) -> Result<()> {
        not_connected()
    }
    fn set_kind(&self, _: &LocalId, _: Kind) -> Result<()> {
        not_connected()
    }
    fn sync_status(&self) -> SyncStatus {
        SyncStatus::Synced
    }
    fn set_event_sink(&self, _: Box<dyn Fn(CoreEvent) + Send + Sync>) {}
    fn resolve_conflict(&self, _: &LocalId, _: Resolution) -> Result<()> {
        not_connected()
    }
    fn reading(&self) -> Vec<ReadingItem> {
        vec![]
    }
    fn subscriptions(&self) -> Vec<Subscription> {
        vec![]
    }
    fn publish(&self, _: &LocalId, _: Option<&str>) -> Result<PublishOutcome> {
        not_connected()
    }
    fn withdraw(&self, _: &LocalId, _: Option<&str>) -> Result<u32> {
        not_connected()
    }
    fn pin(&self, _: &LocalId, _: u32) -> Result<()> {
        not_connected()
    }
    fn versions(&self, _: &LocalId) -> Result<Vec<Version>> {
        Ok(vec![])
    }
    fn restore(&self, _: &LocalId, _: u32) -> Result<()> {
        not_connected()
    }
    fn delete_draft(&self, _: &LocalId) -> Result<()> {
        not_connected()
    }
    fn upload_media(
        &self,
        _: Vec<u8>,
        _: &str,
        _: Option<&LocalId>,
        _: Option<&str>,
    ) -> Result<MediaRef> {
        not_connected()
    }
    fn fork(&self, _: &RemoteRef) -> Result<LocalId> {
        not_connected()
    }
    fn set_show_responses(&self, _: &LocalId, _: bool) -> Result<()> {
        not_connected()
    }
    fn sync_now(&self) -> Result<()> {
        Ok(())
    }
    fn preview_subscription(&self, _: &str) -> Result<SubscribePreview> {
        not_connected()
    }
    fn subscribe(&self, _: &str, _: Option<&str>) -> Result<Subscription> {
        not_connected()
    }
    fn unsubscribe(&self, _: &str) -> Result<()> {
        not_connected()
    }
    fn set_subscription(&self, _: &str, _: Option<bool>, _: Option<&str>) -> Result<()> {
        not_connected()
    }
    fn pause_subscription(&self, _: &str, _: bool) -> Result<()> {
        not_connected()
    }
    fn signal(&self, _: &str, _: &str, _: Option<i8>) -> Result<()> {
        not_connected()
    }
    fn mentions(&self) -> Result<Vec<Mention>> {
        Ok(vec![])
    }
    fn set_mention_hidden(&self, _: &str, _: bool) -> Result<()> {
        not_connected()
    }
    fn settings(&self) -> Result<Settings> {
        not_connected()
    }
    fn save_settings(&self, _: &Settings) -> Result<()> {
        not_connected()
    }
}

/// Every method delegates to the current backend.
impl Backend for SwitchBackend {
    fn items(&self) -> Vec<Item> {
        self.cur().items()
    }
    fn search(&self, q: &str) -> Vec<Item> {
        self.cur().search(q)
    }
    fn item(&self, id: &LocalId) -> Option<Item> {
        self.cur().item(id)
    }
    fn create_draft(&self, kind: Kind, content_md: &str) -> Result<LocalId> {
        self.cur().create_draft(kind, content_md)
    }
    fn save(&self, id: &LocalId, content_md: &str) -> Result<()> {
        self.cur().save(id, content_md)
    }
    fn set_kind(&self, id: &LocalId, kind: Kind) -> Result<()> {
        self.cur().set_kind(id, kind)
    }
    fn save_with_provenance(
        &self,
        id: &LocalId,
        content_md: &str,
        scopes: &[Option<ScopeProvenance>],
    ) -> Result<()> {
        self.cur().save_with_provenance(id, content_md, scopes)
    }
    // --- AI ---
    fn tracked_tk_provenance(
        &self,
        id: &LocalId,
    ) -> Option<(String, Vec<Option<ScopeProvenance>>)> {
        self.cur().tracked_tk_provenance(id)
    }
    fn provenance_available(&self) -> bool {
        self.cur().provenance_available()
    }
    fn sync_status(&self) -> SyncStatus {
        self.cur().sync_status()
    }
    // --- scratch notes ---
    fn create_scratch(&self, kind: Kind, content_md: &str) -> Result<LocalId> {
        self.cur().create_scratch(kind, content_md)
    }
    fn promote(&self, id: &LocalId, to: blyg_core::Promote) -> Result<blyg_core::Promoted> {
        self.cur().promote(id, to)
    }
    fn base_url(&self) -> Option<String> {
        self.cur().base_url()
    }
    fn set_event_sink(&self, sink: Box<dyn Fn(CoreEvent) + Send + Sync>) {
        let sink: Sink = Arc::from(sink);
        *self.sink.lock().unwrap_or_else(|p| p.into_inner()) = Some(sink.clone());
        self.cur().set_event_sink(Box::new(move |e| sink(e)));
    }
    fn resolve_conflict(&self, id: &LocalId, how: Resolution) -> Result<()> {
        self.cur().resolve_conflict(id, how)
    }
    fn reading(&self) -> Vec<ReadingItem> {
        self.cur().reading()
    }
    fn subscriptions(&self) -> Vec<Subscription> {
        self.cur().subscriptions()
    }
    fn mark_read(&self, sub_id: &str, remote_id: &str) -> Result<()> {
        self.cur().mark_read(sub_id, remote_id)
    }
    fn publish(&self, id: &LocalId, note: Option<&str>) -> Result<PublishOutcome> {
        self.cur().publish(id, note)
    }
    fn withdraw(&self, id: &LocalId, note: Option<&str>) -> Result<u32> {
        self.cur().withdraw(id, note)
    }
    fn pin(&self, id: &LocalId, version: u32) -> Result<()> {
        self.cur().pin(id, version)
    }
    fn versions(&self, id: &LocalId) -> Result<Vec<Version>> {
        self.cur().versions(id)
    }
    fn restore(&self, id: &LocalId, version: u32) -> Result<()> {
        self.cur().restore(id, version)
    }
    fn delete_draft(&self, id: &LocalId) -> Result<()> {
        self.cur().delete_draft(id)
    }
    fn upload_media(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        item: Option<&LocalId>,
        alt: Option<&str>,
    ) -> Result<MediaRef> {
        self.cur().upload_media(bytes, mime, item, alt)
    }
    fn delete_media(&self, url: &str) -> Result<()> {
        self.cur().delete_media(url)
    }
    fn fork(&self, of: &RemoteRef) -> Result<LocalId> {
        self.cur().fork(of)
    }
    fn set_show_responses(&self, id: &LocalId, show: bool) -> Result<()> {
        self.cur().set_show_responses(id, show)
    }
    fn sync_now(&self) -> Result<()> {
        self.cur().sync_now()
    }
    fn preview_subscription(&self, url: &str) -> Result<SubscribePreview> {
        self.cur().preview_subscription(url)
    }
    fn subscribe(&self, url: &str, title: Option<&str>) -> Result<Subscription> {
        self.cur().subscribe(url, title)
    }
    fn unsubscribe(&self, sub_id: &str) -> Result<()> {
        self.cur().unsubscribe(sub_id)
    }
    fn set_subscription(
        &self,
        sub_id: &str,
        in_blogroll: Option<bool>,
        title: Option<&str>,
    ) -> Result<()> {
        self.cur().set_subscription(sub_id, in_blogroll, title)
    }
    fn pause_subscription(&self, sub_id: &str, paused: bool) -> Result<()> {
        self.cur().pause_subscription(sub_id, paused)
    }
    fn signal(&self, sub_id: &str, remote_id: &str, thumb: Option<i8>) -> Result<()> {
        self.cur().signal(sub_id, remote_id, thumb)
    }
    fn mentions(&self) -> Result<Vec<Mention>> {
        self.cur().mentions()
    }
    fn set_mention_hidden(&self, mention_id: &str, hidden: bool) -> Result<()> {
        self.cur().set_mention_hidden(mention_id, hidden)
    }
    fn settings(&self) -> Result<Settings> {
        self.cur().settings()
    }
    fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.cur().save_settings(settings)
    }
    fn remote_versions(&self, sub_id: &str, remote_id: &str) -> Result<Vec<RemoteVersion>> {
        self.cur().remote_versions(sub_id, remote_id)
    }
    fn remote_shown_versions(&self, sub_id: &str, remote_id: &str) -> Result<Vec<RemoteVersion>> {
        self.cur().remote_shown_versions(sub_id, remote_id)
    }
    fn remote_pinned(&self, sub_id: &str, remote_id: &str, version: u32) -> Result<PinnedVersion> {
        self.cur().remote_pinned(sub_id, remote_id, version)
    }
    fn pinned_diff_base(&self, sub_id: &str, remote_id: &str) -> Option<PinnedVersion> {
        self.cur().pinned_diff_base(sub_id, remote_id)
    }
    // --- reading ---
    fn read_extensions_available(&self) -> bool {
        self.cur().read_extensions_available()
    }
    fn create_stub(&self, of: &RemoteRef, content_md: &str) -> Result<LocalId> {
        self.cur().create_stub(of, content_md)
    }
    // --- profiles ---
    fn profile(&self, url: &str, refresh: bool) -> Result<blyg_core::Profile> {
        self.cur().profile(url, refresh)
    }
    fn cached_profile(&self, url: &str) -> Option<blyg_core::Profile> {
        self.cur().cached_profile(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_moves_the_sink_and_announces_itself() {
        let sw = SwitchBackend::new(Arc::new(Disconnected), Mode::Disconnected);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        sw.set_event_sink(Box::new(move |e| s2.lock().unwrap().push(e)));
        assert!(sw.create_draft(Kind::Fragment, "x").is_err());
        let fake = Arc::new(crate::fake::FakeBackend::with_timing(
            crate::fake::Timing::instant(),
        ));
        let (tx, rx) = std::sync::mpsc::channel();
        sw.switch(fake.clone(), Mode::Fake, move || tx.send(()).unwrap());
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("`after` ran once the old backend was dropped");
        assert_eq!(sw.mode(), Mode::Fake);
        assert!(!sw.items().is_empty());
        let ev = seen.lock().unwrap().clone();
        assert!(ev.contains(&CoreEvent::ItemsChanged), "{ev:?}");
        // Events from the new backend reach the same sink.
        let id = sw.create_draft(Kind::Fragment, "hello").unwrap();
        assert!(sw.item(&id).is_some());
    }
}
