//! `LiveBackend`: the real `Backend`, over the SQLite store, the owner API and
//! the sync worker.

use std::path::Path;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::api::Api;
use crate::api::public::{PublicClient, not_pinned};
use crate::api::wire::WireItem;
use crate::backend::*;
use crate::model::*;
use crate::store::Store;
use crate::sync::{Engine, Msg, SyncOptions, spawn};

pub struct LiveBackend {
    engine: Arc<Engine>,
    /// Unauthenticated reads of other blygs' public surface. Holds no token.
    public: PublicClient,
    tx: Mutex<Sender<Msg>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

pub const DB_FILE: &str = "blygger.db";

/// How long a fetched public changelog is trusted before refetching.
pub const CHANGELOG_TTL_MS: i64 = 2 * 60 * 1000;

impl LiveBackend {
    /// Open (or create) `<data_dir>/blygger.db` and start syncing with the
    /// blyg at `base_url`.
    pub fn open(data_dir: &Path, base_url: &str, token: &str) -> Result<LiveBackend> {
        Self::open_with(data_dir, base_url, token, SyncOptions::default())
    }

    pub fn open_with(
        data_dir: &Path,
        base_url: &str,
        token: &str,
        opts: SyncOptions,
    ) -> Result<LiveBackend> {
        let store = Store::open(&data_dir.join(DB_FILE))?;
        let scratch = crate::scratch_media::dir(data_dir);
        Ok(Self::from_parts(
            store,
            Api::new(base_url, token),
            opts,
            Some(scratch),
        ))
    }

    fn from_parts(
        store: Store,
        api: Api,
        opts: SyncOptions,
        scratch_dir: Option<std::path::PathBuf>,
    ) -> LiveBackend {
        let start = opts.start_worker;
        let mut engine = Engine::new(store, api, opts);
        engine.scratch_dir = scratch_dir;
        let engine = Arc::new(engine);
        let (tx, rx) = channel();
        let worker = start.then(|| spawn(engine.clone(), rx));
        LiveBackend {
            engine,
            public: PublicClient::new(),
            tx: Mutex::new(tx),
            worker: Mutex::new(worker),
        }
    }

    /// Pull from the server without pushing first (`sync_now` pushes, then pulls).
    pub fn pull_now(&self) -> Result<()> {
        self.engine.pull()
    }

    /// Whether the server has `GET /api/reading` (false until the first pull proves otherwise).
    pub fn reading_available(&self) -> bool {
        !self.engine.reading_unavailable()
    }

    /// False once the server answered 404 to the provenance endpoint: text
    /// generated in the app would publish without disclosure there.
    pub fn provenance_available(&self) -> bool {
        self.engine
            .store
            .meta(crate::sync::PROVENANCE_UNAVAILABLE)
            .is_none()
    }

    /// The TK provenance tracked for an item (`scopes` keyed to `keyed_to`).
    pub fn tracked_provenance(&self, id: &LocalId) -> crate::store::Tracked {
        self.engine.store.provenance(id)
    }

    /// `GET /api/hoppers` (patch 3); empty when the server doesn't have it yet.
    pub fn hoppers(&self) -> Result<Vec<Hopper>> {
        Ok(self
            .engine
            .track(self.engine.api.hoppers())?
            .unwrap_or_default())
    }

    fn send(&self, m: Msg) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(m);
        }
    }

    fn wake(&self) {
        self.send(Msg::Wake);
        self.engine.emit_status();
    }

    fn e(&self) -> &Engine {
        &self.engine
    }

    fn row(&self, id: &LocalId) -> Result<crate::store::SyncRow> {
        self.e().store.row(id).ok_or(CoreError::NotFound)
    }

    fn server_id(&self, id: &LocalId) -> Result<String> {
        self.row(id)?
            .item
            .server_id
            .map(|s| s.0)
            .ok_or(CoreError::NotSynced)
    }

    /// Server id for an item, pushing it first if it's still local-only.
    fn ensure_on_server(&self, id: &LocalId) -> Result<String> {
        let row = self.row(id)?;
        if row.item.conflict {
            return Err(conflict_err());
        }
        if row.item.server_id.is_none() || row.item.pending_sync {
            self.e().flush(Some(id), true)?;
        }
        self.server_id(id)
    }

    /// Best-effort refresh of one item from `GET /api/items/:id`.
    fn refresh(&self, id: &LocalId, sid: &str) -> Option<WireItem> {
        let w = self.e().api.get_item(sid).ok()?;
        self.e().store.apply_server(id, &w).ok()?;
        Some(w)
    }

    /// A cached reading row and whether it came through a blyg-kind
    /// subscription (only those have a public item document and pins).
    fn remote_row(&self, sub_id: &str, remote_id: &str) -> Result<(ReadingItem, bool)> {
        let (it, sub_kind) = self
            .e()
            .store
            .reading_row(sub_id, remote_id)
            .ok_or(CoreError::NotFound)?;
        let blyg = match sub_kind.as_deref() {
            Some(k) => k == "blyg",
            None => self
                .e()
                .store
                .subscriptions()
                .iter()
                .any(|s| s.id == sub_id && s.kind == SubscriptionKind::Blyg),
        };
        Ok((it, blyg))
    }

    /// The public changelog, from the cache while it's fresh and covers the
    /// version we hold, else fetched; a failed fetch falls back to the cache.
    fn changelog(&self, it: &ReadingItem) -> Result<Vec<RemoteVersion>> {
        let cached = self.e().store.cached_changelog(&it.origin, &it.remote_id);
        if let Some((v, at)) = &cached {
            let fresh = crate::util::now_ms() - at < CHANGELOG_TTL_MS;
            let covers = v.iter().map(|r| r.version).max() >= Some(it.version);
            if fresh && covers {
                return Ok(v.clone());
            }
        }
        match self.public.item_doc(&it.origin, &it.remote_id) {
            Ok(doc) => {
                let v = doc.versions_at(&it.origin);
                self.e()
                    .store
                    .put_changelog(&it.origin, &it.remote_id, &v)?;
                Ok(v)
            }
            Err(e) => cached.map(|(v, _)| v).ok_or(e),
        }
    }

    fn pinned_for(&self, it: &ReadingItem, blyg: bool, version: u32) -> Result<PinnedVersion> {
        if !blyg {
            return Err(not_pinned(version));
        }
        if let Some(p) = self
            .e()
            .store
            .cached_pin(&it.origin, &it.remote_id, version)
        {
            return Ok(p);
        }
        // §8.4: no request for a version the changelog doesn't mark pinned.
        let pinned = self
            .changelog(it)?
            .iter()
            .any(|v| v.version == version && v.pinned);
        if !pinned {
            return Err(not_pinned(version));
        }
        match self.public.pinned(&it.origin, &it.remote_id, version) {
            Ok(p) => {
                self.e().store.put_pin(&p)?;
                Ok(p)
            }
            Err(e) => {
                if matches!(e, CoreError::Rejected { status: 404, .. }) {
                    // The changelog we trusted was wrong or stale.
                    self.e().store.drop_changelog(&it.origin, &it.remote_id)?;
                }
                Err(e)
            }
        }
    }

    /// Before a scratch note is promoted: upload its local images and put
    /// their blyg URLs in the text, so the draft is created with them. A
    /// failed upload refuses the promotion (the note stays scratch; files
    /// already uploaded are removed again, best effort). Offline, whatever
    /// made it is swapped in and the rest go up with the queued `create`
    /// (`Engine::run_op`).
    fn upload_scratch_media(&self, id: &LocalId, content: &str) -> Result<()> {
        if !crate::scratch_media::has_refs(content) {
            return Ok(());
        }
        let (done, err) = self.e().upload_scratch_media(content);
        match err {
            None | Some(CoreError::Offline) => {
                if !done.is_empty() {
                    self.e().store.rewrite_media(id, &done)?;
                }
                Ok(())
            }
            Some(e) => {
                for (_, url) in &done {
                    let media = url.rsplit('/').next().unwrap_or_default();
                    let _ = self.delete_media(media);
                }
                Err(match e {
                    CoreError::Rejected {
                        status,
                        message,
                        details,
                    } => CoreError::Rejected {
                        status,
                        message: format!("an image couldn't be uploaded ({message})"),
                        details,
                    },
                    e => e,
                })
            }
        }
    }

    fn permalink_for(&self, kind: Kind, sid: &str) -> String {
        let p = if kind == Kind::Thread { "t" } else { "f" };
        format!("{}/{p}/{sid}", self.e().api.base_url())
    }
}

// --- profiles ---

/// What a profile URL resolved to.
enum ProfileTarget {
    Blyg(String),
    Feed { feed: String, site: Option<String> },
}

/// Largest OPML / feed read for a profile.
const PROFILE_TEXT_MAX: u64 = 4 * 1024 * 1024;

impl LiveBackend {
    /// Resolve a URL the way subscribing does: what we already know (your
    /// own blyg, your subscriptions) first, then the subscribe preview (the
    /// owner Worker's §12.1 resolution). Only when that isn't available
    /// (no read extensions, not connected) do we probe the URL ourselves:
    /// `blyg.json` at it, then the URL as a feed. Two requests, no crawl.
    fn resolve_profile(&self, url: &str) -> Result<ProfileTarget> {
        use crate::profile::{normalize_origin, parse_feed, same_origin, under_origin};
        if let Some(own) = self.base_url()
            && under_origin(url, &own)
        {
            return Ok(ProfileTarget::Blyg(normalize_origin(&own).unwrap_or(own)));
        }
        for s in self.e().store.subscriptions() {
            let feed_match = same_origin(url, &s.feed_url);
            match s.kind {
                SubscriptionKind::Blyg if feed_match || under_origin(url, &s.origin) => {
                    return Ok(ProfileTarget::Blyg(
                        normalize_origin(&s.origin).unwrap_or(s.origin),
                    ));
                }
                SubscriptionKind::Rss if feed_match || same_origin(url, &s.origin) => {
                    return Ok(ProfileTarget::Feed {
                        feed: s.feed_url,
                        site: Some(s.origin),
                    });
                }
                _ => {}
            }
        }
        let preview_err = match self.preview_subscription(url) {
            Ok(p) => {
                return Ok(match p.kind {
                    SubscriptionKind::Blyg => {
                        let o = p.origin.unwrap_or_else(|| url.to_string());
                        ProfileTarget::Blyg(normalize_origin(&o).unwrap_or(o))
                    }
                    SubscriptionKind::Rss => ProfileTarget::Feed {
                        feed: p.feed_url.unwrap_or_else(|| url.to_string()),
                        site: p.origin,
                    },
                });
            }
            Err(CoreError::Offline) => return Err(CoreError::Offline),
            Err(e) => e,
        };
        if let Some(o) = normalize_origin(url)
            && let Some(m) = origin_url(&o, "blyg.json")
            && self
                .public
                .get_json(&m)
                .is_ok_and(|v| crate::profile::is_manifest(&v))
        {
            return Ok(ProfileTarget::Blyg(o));
        }
        match self.public.get_text(url, PROFILE_TEXT_MAX) {
            Ok(t) if parse_feed(&t).is_some() => Ok(ProfileTarget::Feed {
                feed: url.to_string(),
                site: None,
            }),
            Err(CoreError::Offline) => Err(CoreError::Offline),
            _ => Err(preview_err),
        }
    }

    fn fetch_profile(&self, url: &str) -> Result<crate::profile::Profile> {
        match self.resolve_profile(url)? {
            ProfileTarget::Blyg(origin) => self.fetch_blyg_profile(&origin),
            ProfileTarget::Feed { feed, site } => self.fetch_feed_profile(&feed, site),
        }
    }

    /// Manifest (required), then the blogroll when listed, the archive index
    /// and the feed (for titles and quote sources); each of those may fail.
    fn fetch_blyg_profile(&self, origin: &str) -> Result<crate::profile::Profile> {
        use crate::profile::*;
        let pub_ = &self.public;
        let at = |p: &str| origin_url(origin, p);
        let manifest_url =
            at("blyg.json").ok_or_else(|| CoreError::Other(format!("not a blyg: {origin}")))?;
        let m = parse_manifest(&pub_.get_json(&manifest_url)?, origin);
        let blogroll = m
            .blogroll
            .as_deref()
            .and_then(at)
            .and_then(|u| pub_.get_text(&u, PROFILE_TEXT_MAX).ok())
            .map(|t| parse_opml(&t))
            .unwrap_or_default();
        let index = at(m.items.as_deref().unwrap_or("items/index.json"))
            .and_then(|u| pub_.get_json(&u).ok())
            .map(|v| parse_index(&v))
            .unwrap_or_default();
        let feed_url = at(m.feed.as_deref().unwrap_or("feed.xml"));
        let feed = feed_url
            .as_deref()
            .and_then(|u| pub_.get_text(u, PROFILE_TEXT_MAX).ok())
            .and_then(|t| parse_feed(&t));
        let held = self.e().store.reading();
        let store = &self.e().store;
        let pinned = |id: &str| {
            held.iter()
                .filter(|r| r.remote_id == id && same_origin(&r.origin, origin))
                .any(|r| {
                    store
                        .cached_changelog(&r.origin, id)
                        .is_some_and(|(v, _)| v.iter().any(|x| x.pinned))
                })
        };
        let posts = blyg_posts(origin, &index, feed.as_ref(), &held, &pinned);
        let feed_quotes = feed
            .as_ref()
            .map(|f| {
                f.items
                    .iter()
                    .filter_map(|i| Some((i.blyg_id.clone()?, &i.quote_origins)))
                    .flat_map(|(id, os)| os.iter().map(move |o| (o.clone(), id.clone())))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Profile {
            kind: ProfileKind::Blyg,
            origin: origin.to_string(),
            feed_url,
            name: m.name,
            title: m.title,
            bio: m.bio,
            avatar: m.avatar,
            links: m.links,
            has_blogroll: m.blogroll.is_some(),
            blogroll,
            posts,
            connections: vec![],
            feed_quotes,
            own: false,
            fetched_at: crate::util::now_ms(),
            stale: false,
        })
    }

    /// A plain feed: title, link, recent items. A feed that declares
    /// `<blyg:manifest>` is a blyg (the upgrade hook) and gets the full profile.
    fn fetch_feed_profile(
        &self,
        feed: &str,
        site: Option<String>,
    ) -> Result<crate::profile::Profile> {
        use crate::profile::*;
        let text = self.public.get_text(feed, PROFILE_TEXT_MAX)?;
        let f = parse_feed(&text)
            .ok_or_else(|| CoreError::Other(format!("{feed} isn't an RSS or Atom feed")))?;
        if let Some(m) = &f.manifest
            && let Some(o) = m.strip_suffix("blyg.json").and_then(normalize_origin)
        {
            return self.fetch_blyg_profile(&o);
        }
        let origin = f
            .link
            .clone()
            .or(site)
            .and_then(|l| normalize_origin(&l))
            .unwrap_or_else(|| feed.to_string());
        Ok(Profile {
            kind: ProfileKind::Feed,
            origin,
            feed_url: Some(feed.to_string()),
            name: f.title.clone(),
            title: None,
            bio: None,
            avatar: None,
            links: vec![],
            blogroll: vec![],
            has_blogroll: false,
            posts: feed_posts(&f),
            connections: vec![],
            feed_quotes: vec![],
            own: false,
            fetched_at: crate::util::now_ms(),
            stale: false,
        })
    }

    /// Mark your own blyg and compute connections from what's held locally.
    fn finish_profile(&self, mut p: crate::profile::Profile) -> crate::profile::Profile {
        use crate::profile::*;
        p.own = p.kind == ProfileKind::Blyg
            && self.base_url().is_some_and(|b| same_origin(&b, &p.origin));
        let mut held = self.e().store.reading();
        p.connections = if p.own {
            own_connections(&p.origin, &self.e().store.items(), &held)
        } else {
            // The owner API leaves lineage out: fill it from item documents
            // already fetched (the changelog cache), never fetching more.
            for r in held
                .iter_mut()
                .filter(|r| same_origin(&r.origin, &p.origin))
            {
                if let Some((log, _)) = self.e().store.cached_changelog(&r.origin, &r.remote_id)
                    && let Some(l) = crate::model::Lineage::of_changelog(&log)
                {
                    r.fill_lineage(l);
                }
            }
            connections(&p.origin, &held, &p.feed_quotes)
        };
        p
    }
}

fn conflict_err() -> CoreError {
    CoreError::Rejected {
        status: 409,
        message: "resolve the sync conflict first".into(),
        details: vec![],
    }
}

impl Drop for LiveBackend {
    fn drop(&mut self) {
        self.send(Msg::Shutdown);
        if let Some(h) = self.worker.lock().ok().and_then(|mut w| w.take()) {
            let _ = h.join();
        }
    }
}

impl Backend for LiveBackend {
    // ---------------------------------------------------------------- local

    fn items(&self) -> Vec<Item> {
        self.e().store.items()
    }

    fn search(&self, query: &str) -> Vec<Item> {
        self.e().store.search(query)
    }

    fn item(&self, id: &LocalId) -> Option<Item> {
        self.e().store.item(id)
    }

    fn create_draft(&self, kind: Kind, content_md: &str) -> Result<LocalId> {
        let id = self
            .e()
            .store
            .create_draft(kind, content_md, self.e().debounce_ms())?;
        self.wake();
        Ok(id)
    }

    fn save(&self, id: &LocalId, content_md: &str) -> Result<()> {
        self.e()
            .store
            .save(id, content_md, self.e().debounce_ms())?;
        self.wake();
        Ok(())
    }

    fn set_kind(&self, id: &LocalId, kind: Kind) -> Result<()> {
        self.e().store.set_kind(id, kind, self.e().debounce_ms())?;
        self.wake();
        Ok(())
    }

    fn tracked_tk_provenance(
        &self,
        id: &LocalId,
    ) -> Option<(String, Vec<Option<ScopeProvenance>>)> {
        let t = LiveBackend::tracked_provenance(self, id);
        Some((t.keyed_to?, t.scopes?))
    }

    fn provenance_available(&self) -> bool {
        LiveBackend::provenance_available(self)
    }

    fn save_with_provenance(
        &self,
        id: &LocalId,
        content_md: &str,
        scopes: &[Option<ScopeProvenance>],
    ) -> Result<()> {
        crate::tk::validate(content_md, scopes).map_err(|message| CoreError::Rejected {
            status: 400,
            message,
            details: vec![],
        })?;
        self.e()
            .store
            .save_inner(id, content_md, Some(scopes), self.e().debounce_ms())?;
        self.wake();
        Ok(())
    }

    fn sync_status(&self) -> SyncStatus {
        self.e().status()
    }

    /// The public origin (where `media/…` lives): learned from the
    /// server's permalinks, which carry the deployment's mount; the API base
    /// until the first one arrives (the same thing for a root mount).
    fn base_url(&self) -> Option<String> {
        let origin = self
            .e()
            .store
            .public_origin()
            .unwrap_or_else(|| self.e().api.base_url().to_string());
        Some(origin.trim_end_matches('/').to_string())
    }

    fn set_event_sink(&self, sink: Box<dyn Fn(CoreEvent) + Send + Sync>) {
        self.e().set_sink(Arc::from(sink));
    }

    fn resolve_conflict(&self, id: &LocalId, how: Resolution) -> Result<()> {
        self.e().store.resolve(id, how)?;
        self.e().emit(CoreEvent::ItemsChanged);
        self.wake();
        Ok(())
    }

    fn reading(&self) -> Vec<ReadingItem> {
        self.e().store.reading()
    }

    fn subscriptions(&self) -> Vec<Subscription> {
        self.e().store.subscriptions()
    }

    fn mark_read(&self, sub_id: &str, remote_id: &str) -> Result<()> {
        // Instant and local; on a server that syncs read state (extension 5)
        // the same write queues one `read` op per row it raised.
        let sync = self.e().read_sync_on();
        let marked = self.e().store.mark_read_rows(sub_id, remote_id, sync)?;
        if !marked.is_empty() {
            self.e().emit(CoreEvent::ReadingChanged);
            if sync {
                self.wake();
            }
        }
        Ok(())
    }

    // --- scratch notes ---

    fn create_scratch(&self, kind: Kind, content_md: &str) -> Result<LocalId> {
        let id = self.e().store.create_scratch(kind, content_md)?;
        self.e().emit(CoreEvent::ItemsChanged);
        Ok(id)
    }

    fn promote(&self, id: &LocalId, to: Promote) -> Result<Promoted> {
        let mut row = self.row(id)?;
        let kind = if row.item.status == Status::Scratch {
            // --- scratch media --- local images go up first.
            self.upload_scratch_media(id, &row.item.content_md)?;
            row = self.row(id)?;
            let kind = promotion_kind(row.item.kind, &row.item.content_md);
            self.e().store.promote_scratch(id, kind)?;
            self.e().emit(CoreEvent::ItemsChanged);
            self.wake();
            kind
        } else {
            row.item.kind
        };
        let published = match to {
            Promote::Draft => None,
            Promote::Publish { note } => Some(self.publish(id, note.as_deref())?),
        };
        Ok(Promoted { kind, published })
    }

    // --------------------------------------------------------------- remote

    fn publish(&self, id: &LocalId, note: Option<&str>) -> Result<PublishOutcome> {
        if self.row(id)?.item.status == Status::Scratch {
            // A scratch note publishes by being promoted first.
            let note = note.map(str::to_string);
            return self
                .promote(id, Promote::Publish { note })?
                .published
                .ok_or_else(|| CoreError::Other("not published".into()));
        }
        let sid = self.ensure_on_server(id)?;
        let _net = self.e().net_lock();
        let row = self.row(id)?;
        // What the server holds right now is what gets published.
        let published = row
            .base_content
            .clone()
            .unwrap_or_else(|| row.item.content_md.clone());
        let p = self.e().track(self.e().api.publish(&sid, note))?;
        let permalink = self
            .refresh(id, &sid)
            .and_then(|w| w.permalink)
            .unwrap_or_else(|| self.permalink_for(row.item.kind, &sid));
        self.e()
            .store
            .after_publish(id, p.version, &permalink, &published)?;
        self.e().emit(CoreEvent::ItemsChanged);
        Ok(PublishOutcome {
            version: p.version,
            permalink,
            warning: p.warning,
        })
    }

    fn withdraw(&self, id: &LocalId, note: Option<&str>) -> Result<u32> {
        let sid = self.server_id(id)?;
        let _net = self.e().net_lock();
        let v = self.e().track(self.e().api.withdraw(&sid, note))?;
        if self.refresh(id, &sid).is_none() {
            self.e()
                .store
                .set_status_version(id, Status::Withdrawn, v)?;
        }
        self.e().emit(CoreEvent::ItemsChanged);
        Ok(v)
    }

    fn pin(&self, id: &LocalId, version: u32) -> Result<()> {
        let sid = self.server_id(id)?;
        let _net = self.e().net_lock();
        // Endcaps can't be pinned (§8; the server says 409 too). Refuse
        // locally, refreshing the version list first if it doesn't know `version`.
        let known = |vs: &[Version]| vs.iter().find(|v| v.version == version).cloned();
        let v = known(&self.e().store.versions(id)).or_else(|| {
            self.refresh(id, &sid);
            known(&self.e().store.versions(id))
        });
        if v.is_some_and(|v| v.endcap) {
            return Err(CoreError::Rejected {
                status: 409,
                message: "cannot pin an endcap version".into(),
                details: vec![],
            });
        }
        self.e().track(self.e().api.pin(&sid, version))?;
        self.refresh(id, &sid);
        Ok(())
    }

    fn versions(&self, id: &LocalId) -> Result<Vec<Version>> {
        let row = self.row(id)?;
        let Some(sid) = row.item.server_id else {
            return Ok(vec![]);
        };
        match self.e().track(self.e().api.get_item(&sid.0)) {
            Ok(w) => {
                let _net = self.e().net_lock();
                self.e().store.apply_server(id, &w)?;
                Ok(w.versions.unwrap_or_default())
            }
            Err(CoreError::Offline) => {
                let cached = self.e().store.versions(id);
                if cached.is_empty() {
                    Err(CoreError::Offline)
                } else {
                    Ok(cached)
                }
            }
            Err(e) => Err(e),
        }
    }

    fn restore(&self, id: &LocalId, version: u32) -> Result<()> {
        let sid = self.server_id(id)?;
        let _net = self.e().net_lock();
        self.e().track(self.e().api.restore(&sid, version))?;
        let w = self.e().track(self.e().api.get_item(&sid))?;
        self.e().store.replace_from_server(id, &w)?;
        self.e().emit(CoreEvent::ItemsChanged);
        self.e().emit_status();
        Ok(())
    }

    fn delete_draft(&self, id: &LocalId) -> Result<()> {
        let _net = self.e().net_lock();
        let row = self.row(id)?;
        if row.item.version > 0 {
            return Err(CoreError::Rejected {
                status: 409,
                message: "published items are withdrawn, not deleted".into(),
                details: vec![],
            });
        }
        if let Some(sid) = &row.item.server_id {
            match self.e().track(self.e().api.delete_item(&sid.0)) {
                Ok(()) | Err(CoreError::NotFound) => {}
                Err(e) => return Err(e),
            }
        }
        self.e().store.delete_item(id)?;
        self.e().emit(CoreEvent::ItemsChanged);
        self.e().emit_status();
        Ok(())
    }

    fn upload_media(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        item: Option<&LocalId>,
        alt: Option<&str>,
    ) -> Result<MediaRef> {
        let sid = match item {
            Some(id) => Some(self.ensure_on_server(id)?),
            None => None,
        };
        let m = self
            .e()
            .track(self.e().api.upload_media(&bytes, mime, sid.as_deref(), alt))?;
        Ok(MediaRef {
            url: m.url,
            mime: m.mime,
        })
    }

    fn delete_media(&self, url: &str) -> Result<()> {
        let file = url.trim_start_matches('/').trim_start_matches("media/");
        let id = file.split('.').next().unwrap_or(file);
        if id.is_empty() {
            return Err(CoreError::NotFound);
        }
        self.e().track(self.e().api.delete_media(id))
    }

    fn fork(&self, of: &RemoteRef) -> Result<LocalId> {
        let _net = self.e().net_lock();
        let sid = self.e().track(self.e().api.fork(of))?;
        let w = match self.e().api.get_item(&sid) {
            Ok(w) => w,
            // The fork exists server-side; the next pull fills in the content.
            Err(_) => WireItem {
                id: sid.clone(),
                kind: "fragment".into(),
                authored_kind: None,
                status: "draft".into(),
                version: 0,
                dirty: true,
                created: crate::util::now_iso(),
                updated: crate::util::now_iso(),
                content_md: String::new(),
                stub_of: None,
                forked_from: serde_json::to_value(of).ok(),
                permalink: None,
                show_responses: false,
                versions: None,
            },
        };
        let id = self.e().store.insert_from_server(&w)?;
        self.e().emit(CoreEvent::ItemsChanged);
        Ok(id)
    }

    fn set_show_responses(&self, id: &LocalId, show: bool) -> Result<()> {
        let sid = self.server_id(id)?;
        self.e()
            .track(self.e().api.set_show_responses(&sid, show))?;
        self.e().store.set_show_responses(id, show)?;
        self.e().emit(CoreEvent::ItemsChanged);
        Ok(())
    }

    fn sync_now(&self) -> Result<()> {
        self.e().sync_now()
    }

    fn preview_subscription(&self, url: &str) -> Result<SubscribePreview> {
        self.e().track(self.e().api.preview_subscription(url))
    }

    fn subscribe(&self, url: &str, title: Option<&str>) -> Result<Subscription> {
        let id = self.e().track(self.e().api.subscribe(url, title))?;
        let subs = self.e().api.list_subscriptions().unwrap_or_default();
        let sub = subs.iter().find(|s| s.id == id).cloned();
        let sub = match sub {
            Some(s) => {
                self.e().store.replace_subscriptions(&subs)?;
                s
            }
            None => {
                let s = Subscription {
                    id,
                    kind: SubscriptionKind::Blyg,
                    origin: url.to_string(),
                    feed_url: url.to_string(),
                    title: title.unwrap_or(url).to_string(),
                    status: "active".into(),
                    in_blogroll: false,
                };
                let mut all = self.e().store.subscriptions();
                all.push(s.clone());
                self.e().store.replace_subscriptions(&all)?;
                s
            }
        };
        self.e().emit(CoreEvent::ReadingChanged);
        self.send(Msg::Pull);
        Ok(sub)
    }

    fn unsubscribe(&self, sub_id: &str) -> Result<()> {
        self.e().track(self.e().api.delete_subscription(sub_id))?;
        self.e().store.edit_subscription(sub_id, |_| false)?;
        self.e().emit(CoreEvent::ReadingChanged);
        self.send(Msg::Pull);
        Ok(())
    }

    fn set_subscription(
        &self,
        sub_id: &str,
        in_blogroll: Option<bool>,
        title: Option<&str>,
    ) -> Result<()> {
        self.e()
            .track(self.e().api.update_subscription(sub_id, in_blogroll, title))?;
        self.e().store.edit_subscription(sub_id, |s| {
            if let Some(b) = in_blogroll {
                s.in_blogroll = b;
            }
            if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
                s.title = t.to_string();
            }
            true
        })?;
        self.e().emit(CoreEvent::ReadingChanged);
        Ok(())
    }

    fn pause_subscription(&self, sub_id: &str, paused: bool) -> Result<()> {
        self.e()
            .track(self.e().api.pause_subscription(sub_id, paused))?;
        self.e().store.edit_subscription(sub_id, |s| {
            s.status = if paused { "paused" } else { "active" }.into();
            true
        })?;
        self.e().emit(CoreEvent::ReadingChanged);
        Ok(())
    }

    fn signal(&self, sub_id: &str, remote_id: &str, thumb: Option<i8>) -> Result<()> {
        let r = match thumb {
            Some(t @ (1 | -1)) => self.e().api.set_signal(sub_id, remote_id, t),
            Some(_) => {
                return Err(CoreError::Rejected {
                    status: 400,
                    message: "thumb must be 1 or -1".into(),
                    details: vec![],
                });
            }
            None => self.e().api.clear_signal(sub_id, remote_id),
        };
        self.e().track(r)?;
        if self.e().store.set_thumb(sub_id, remote_id, thumb)? {
            self.e().emit(CoreEvent::ReadingChanged);
        }
        Ok(())
    }

    fn mentions(&self) -> Result<Vec<Mention>> {
        Ok(self.e().track(self.e().api.mentions())?.unwrap_or_default())
    }

    fn set_mention_hidden(&self, mention_id: &str, hidden: bool) -> Result<()> {
        self.e()
            .track(self.e().api.set_mention_hidden(mention_id, hidden))
    }

    /// `NotFound` means the server doesn't expose `GET /api/settings` yet.
    fn settings(&self) -> Result<Settings> {
        self.e()
            .track(self.e().api.settings())?
            .ok_or(CoreError::NotFound)
    }

    fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.e().track(self.e().api.put_settings(settings))
    }

    fn remote_versions(&self, sub_id: &str, remote_id: &str) -> Result<Vec<RemoteVersion>> {
        let (it, blyg) = self.remote_row(sub_id, remote_id)?;
        if !blyg {
            // RSS/L0: no public changelog, just what the feed shows now.
            return Ok(vec![RemoteVersion {
                version: it.version,
                at: it.updated.clone().unwrap_or_else(|| it.observed_at.clone()),
                note: None,
                pinned: false,
                current: true,
                media: vec![],
                lineage: Default::default(),
            }]);
        }
        self.changelog(&it)
    }

    fn remote_pinned(&self, sub_id: &str, remote_id: &str, version: u32) -> Result<PinnedVersion> {
        let (it, blyg) = self.remote_row(sub_id, remote_id)?;
        self.pinned_for(&it, blyg, version)
    }

    fn pinned_diff_base(&self, sub_id: &str, remote_id: &str) -> Option<PinnedVersion> {
        let (it, blyg) = self.remote_row(sub_id, remote_id).ok()?;
        let read = it.read_version?;
        self.pinned_for(&it, blyg, read).ok()
    }

    // --- reading ---

    fn read_extensions_available(&self) -> bool {
        self.reading_available()
    }

    fn create_stub(&self, of: &RemoteRef, content_md: &str) -> Result<LocalId> {
        let id = self
            .e()
            .store
            .create_stub(of, content_md, self.e().debounce_ms())?;
        self.e().emit(CoreEvent::ItemsChanged);
        self.wake();
        Ok(id)
    }

    // --- scratch media ---

    fn save_scratch_media(&self, bytes: &[u8], mime: &str) -> Result<String> {
        let dir = self
            .e()
            .scratch_dir
            .as_deref()
            .ok_or_else(|| CoreError::Other("no place for local images".into()))?;
        crate::scratch_media::store(dir, bytes, mime)
    }

    fn scratch_media_file(&self, url: &str) -> Option<std::path::PathBuf> {
        crate::scratch_media::file(self.e().scratch_dir.as_deref()?, url)
    }

    // --- profiles ---

    fn profile(&self, url: &str, refresh: bool) -> Result<crate::profile::Profile> {
        use crate::profile::{PROFILE_TTL_MS, clean_url};
        let key =
            clean_url(url).ok_or_else(|| CoreError::Other(format!("not a web address: {url}")))?;
        let store = &self.e().store;
        if !refresh
            && let Some((p, at)) = store.cached_profile(&key)
            && crate::util::now_ms() - at < PROFILE_TTL_MS
        {
            return Ok(self.finish_profile(p));
        }
        match self.fetch_profile(&key) {
            Ok(p) => {
                store.put_profile(&[&key, &p.origin], &p)?;
                Ok(self.finish_profile(p))
            }
            Err(e) => match store.cached_profile(&key) {
                Some((mut p, _)) => {
                    p.stale = true;
                    Ok(self.finish_profile(p))
                }
                None => Err(e),
            },
        }
    }

    fn cached_profile(&self, url: &str) -> Option<crate::profile::Profile> {
        let key = crate::profile::clean_url(url)?;
        let (p, _) = self.e().store.cached_profile(&key)?;
        Some(self.finish_profile(p))
    }
}
