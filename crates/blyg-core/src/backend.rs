//! The seam between UI and core. The app only ever talks to `dyn Backend`.
//!
//! Two classes of method:
//! - **Local** (no network, must return in well under a millisecond for a few
//!   thousand items): reads, `create_draft`, `save`, `set_kind`. Mutations
//!   write SQLite, enqueue an outbox op, and return. The sync worker pushes.
//! - **Remote** (blocking network call; the UI runs these on a background
//!   executor): publish, pin, withdraw, media, subscriptions, … They return the
//!   server's verdict because the user is waiting on it.

use crate::model::*;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("offline")]
    Offline,
    #[error("not authorised: sign in to your blyg again")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    /// Server said no; `message` is its `error` field, `details` its `errors`.
    #[error("{message}")]
    Rejected {
        status: u16,
        message: String,
        details: Vec<String>,
    },
    /// The server is older than blygger-studio 0.9 (its owner API predates
    /// the OpenAPI contract). Nothing is pushed to it: an old server's 404 on
    /// a new route would read as "deleted" and duplicate posts.
    #[error("your blyg's server needs updating to blygger-studio 0.9 or later")]
    ServerOutdated,
    /// Item has never reached the server (still local-only) and the op needs a server id.
    #[error("not on the server yet — still syncing")]
    NotSynced,
    #[error("storage: {0}")]
    Storage(String),
    /// 429: the blyg's API work budget is spent (studio 0.28+). Transient:
    /// nothing is dropped, and the next attempt waits `retry_after` seconds
    /// (the server's `Retry-After`).
    #[error("the blyg asked Burrow to slow down; retrying in {retry_after}s")]
    RateLimited { retry_after: u64 },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

pub struct PublishOutcome {
    pub version: u32,
    pub permalink: String,
    pub warning: Option<String>,
}

/// Someone's post fetched from its public item document because it isn't
/// held here (see [`Backend::public_item`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PublicItem {
    /// Not subscribed: `subscription_id` is empty and it's never marked read.
    pub item: ReadingItem,
    /// Its public changelog, oldest first (current row with media and lineage).
    pub versions: Vec<RemoteVersion>,
}

/// The largest media file the app reads (the Worker's upload cap is 5 MB).
pub const MEDIA_MAX_BYTES: u64 = 20 * 1024 * 1024;

pub struct MediaRef {
    /// Relative, e.g. "media/abc123.webp" — what goes in the markdown.
    pub url: String,
    pub mime: String,
}

// --- scratch notes ---

/// What `Backend::promote` turns a scratch note into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Promote {
    /// A server draft (synced, unpublished).
    Draft,
    /// Published right away, with an optional version note.
    Publish { note: Option<String> },
}

/// The result of `Backend::promote`.
pub struct Promoted {
    /// The kind the item was promoted as (see [`promotion_kind`]).
    pub kind: Kind,
    /// Set for `Promote::Publish`.
    pub published: Option<PublishOutcome>,
}

/// The kind a scratch note becomes when it's promoted: a thread when the
/// note is already a thread (the user pressed ⌘T) or its published length
/// is over [`FRAGMENT_LIMIT`] (the server's count, [`published_len`]);
/// otherwise a fragment.
pub fn promotion_kind(kind: Kind, content_md: &str) -> Kind {
    if kind == Kind::Thread || published_len(content_md) > FRAGMENT_LIMIT {
        Kind::Thread
    } else {
        Kind::Fragment
    }
}

pub trait Backend: Send + Sync {
    // ---------- local ----------
    /// Newest-updated first.
    fn items(&self) -> Vec<Item>;
    /// Case-insensitive match over content; same order as `items`. Empty query = `items()`.
    fn search(&self, query: &str) -> Vec<Item>;
    fn item(&self, id: &LocalId) -> Option<Item>;
    fn create_draft(&self, kind: Kind, content_md: &str) -> Result<LocalId>;
    /// Save the working copy. Debounced push to `PUT /api/items/:id`.
    fn save(&self, id: &LocalId, content_md: &str) -> Result<()>;
    /// Fragment ⇄ thread. Before first publish this is free (we recreate the
    /// draft server-side if needed); after publish the server decides.
    fn set_kind(&self, id: &LocalId, kind: Kind) -> Result<()>;
    /// Save the working copy together with who generated its TK scopes: one
    /// entry per scope, in order (`None` = written by hand). Call it when
    /// the app has just generated text; plain `save` keeps tracked
    /// provenance attached to its scopes as the text is edited. Pushed with
    /// the combined `PUT …/tk-provenance` (docs/SPEC.md § Client-recorded
    /// provenance). Local; refuses (`Rejected{400}`) an array that doesn't
    /// fit the text. Additive; the default refuses so implementors compile.
    fn save_with_provenance(
        &self,
        id: &LocalId,
        content_md: &str,
        scopes: &[Option<ScopeProvenance>],
    ) -> Result<()> {
        let _ = (id, content_md, scopes);
        Err(CoreError::Other("provenance isn't supported here".into()))
    }
    fn sync_status(&self) -> SyncStatus;
    /// The blyg's origin (`https://blyg.example.com`, no trailing slash), for
    /// resolving relative `media/…` links. `None` when not connected.
    /// Additive; the default says "unknown" so other implementors compile.
    fn base_url(&self) -> Option<String> {
        None
    }
    /// Register the single event sink (called from a background thread).
    fn set_event_sink(&self, sink: Box<dyn Fn(CoreEvent) + Send + Sync>);
    fn resolve_conflict(&self, id: &LocalId, how: Resolution) -> Result<()>;
    /// Last-known reading list from the local cache (instant).
    fn reading(&self) -> Vec<ReadingItem>;
    fn subscriptions(&self) -> Vec<Subscription>;
    /// Record that the user has seen the current version of a reading item
    /// (and of any duplicates of the same post). Local only. Additive; the
    /// default does nothing so other implementors keep compiling.
    fn mark_read(&self, sub_id: &str, remote_id: &str) -> Result<()> {
        let _ = (sub_id, remote_id);
        Ok(())
    }

    /// Mark many reading items (`(sub_id, remote_id)`, each with its
    /// duplicates) read at their current versions, or unread: a selection
    /// in the reading list, or all of it. Local and instant; on a server
    /// that syncs read state the change is queued for it (unread only when
    /// the server can clear read state; otherwise unread stays on this Mac).
    /// Returns how many rows changed. Additive; the default marks read one
    /// by one and refuses unread, so other implementors compile.
    fn set_read(&self, rows: &[(String, String)], read: bool) -> Result<usize> {
        if !read {
            return Err(CoreError::Other(
                "marking unread isn't supported here".into(),
            ));
        }
        for (s, r) in rows {
            self.mark_read(s, r)?;
        }
        Ok(rows.len())
    }

    // --- scratch notes ---
    // docs/SPEC.md § Scratch notes. Scratch items have `Status::Scratch`:
    // `save`, `set_kind`, `search` and `delete_draft` work on them locally,
    // and nothing about them is ever enqueued, pushed or pulled.

    /// Create a local-only scratch note. Local (no network, ever). Additive;
    /// the default refuses so implementors compile.
    fn create_scratch(&self, kind: Kind, content_md: &str) -> Result<LocalId> {
        let _ = (kind, content_md);
        Err(CoreError::Other(
            "scratch notes aren't supported here".into(),
        ))
    }

    /// Turn a scratch note into a server item, keeping its `LocalId` (no
    /// duplicate). Its kind becomes [`promotion_kind`] (returned in
    /// `Promoted::kind`).
    /// - `Promote::Draft` is local: the item becomes a `Draft` and a
    ///   `create` is queued in the outbox like any new draft (so it works
    ///   offline and syncs later).
    /// - `Promote::Publish` does the same, then publishes (blocking, like
    ///   `publish`). Offline, the promotion to a draft stands and stays queued;
    ///   the publish itself fails with `Offline`.
    ///
    /// On an item that isn't scratch (already promoted) `Draft` is a no-op
    /// and `Publish` is a plain `publish`. There's no demotion back to scratch.
    fn promote(&self, id: &LocalId, to: Promote) -> Result<Promoted> {
        let _ = (id, to);
        Err(CoreError::Other(
            "scratch notes aren't supported here".into(),
        ))
    }

    // ---------- remote (blocking) ----------
    /// Flush this item's outbox first, then publish.
    fn publish(&self, id: &LocalId, note: Option<&str>) -> Result<PublishOutcome>;
    fn withdraw(&self, id: &LocalId, note: Option<&str>) -> Result<u32>;
    /// Irrevocable. UI must confirm first.
    fn pin(&self, id: &LocalId, version: u32) -> Result<()>;
    fn versions(&self, id: &LocalId) -> Result<Vec<Version>>;
    /// Load version `v` into the working copy (server-side), then refresh local.
    fn restore(&self, id: &LocalId, version: u32) -> Result<()>;
    /// Drafts only (local-only drafts just vanish).
    fn delete_draft(&self, id: &LocalId) -> Result<()>;
    fn upload_media(
        &self,
        bytes: Vec<u8>,
        mime: &str,
        item: Option<&LocalId>,
        alt: Option<&str>,
    ) -> Result<MediaRef>;
    /// Remove an uploaded file by its `media/…` URL (or bare id), e.g. an
    /// upload whose placeholder the user deleted before it finished.
    /// `NotFound` when the server doesn't have it (or can't remove media).
    /// Additive; the default refuses so implementors compile.
    fn delete_media(&self, url: &str) -> Result<()> {
        let _ = url;
        Err(CoreError::Other(
            "removing media isn't supported here".into(),
        ))
    }
    fn fork(&self, of: &RemoteRef) -> Result<LocalId>;
    /// Set whether the item's page shows its verified responses. Returns
    /// whether it shows them now (with `Default`, the blyg's setting decides).
    fn set_responses(&self, id: &LocalId, mode: ResponsesMode) -> Result<bool>;
    /// Set whether the item's public page highlights its generated text
    /// (studio 0.27; remote, needs `owner:publish`). Returns whether it
    /// highlights now (with `Default`, `highlight_generated_default` decides).
    fn set_highlight(&self, id: &LocalId, mode: HighlightMode) -> Result<bool>;

    /// The `[[` / `![[` picker's search over what's held here (local): own
    /// published posts and posts imported from blyg subscriptions, every
    /// word matching. The default searches `items()` and `reading()` in
    /// memory; the live backend uses its full-text indexes.
    fn pick_search(&self, q: &crate::pick::PickQuery) -> Vec<crate::pick::Pickable> {
        crate::pick::search_in(&self.items(), &self.reading(), &self.subscriptions(), q)
    }

    /// Where the picker's query is typed (`picker_typing`), as the blyg's
    /// settings last said. Local; `Auto` until they've been read.
    fn picker_typing(&self) -> PickerTyping {
        PickerTyping::Auto
    }

    /// One of the blyg's own media files (`{base}/media/…`), fetched with
    /// the owner credential, since an upload no public version uses yet is
    /// private (studio 0.28). Remote. The default fetches anonymously.
    fn fetch_own_media(&self, url: &str) -> Result<(Vec<u8>, Option<String>)> {
        crate::api::public::PublicClient::new().get_bytes(url, MEDIA_MAX_BYTES)
    }

    /// Pull everything (items, reading, subscriptions) now.
    fn sync_now(&self) -> Result<()>;
    fn preview_subscription(&self, url: &str) -> Result<SubscribePreview>;
    fn subscribe(&self, url: &str, title: Option<&str>) -> Result<Subscription>;
    fn unsubscribe(&self, sub_id: &str) -> Result<()>;
    fn set_subscription(
        &self,
        sub_id: &str,
        in_blogroll: Option<bool>,
        title: Option<&str>,
    ) -> Result<()>;
    fn pause_subscription(&self, sub_id: &str, paused: bool) -> Result<()>;
    /// Name a subscription: `Some(name)` is a name of your own (the source
    /// stops renaming it); `None` hands the name back to the source, which
    /// refreshes it while polling (studio 0.30, `title_follows_source`).
    /// Additive; the default sets a name and refuses `None`.
    fn rename_subscription(&self, sub_id: &str, title: Option<&str>) -> Result<()> {
        match title {
            Some(t) => self.set_subscription(sub_id, None, Some(t)),
            None => Err(CoreError::Other(
                "following the source's name isn't supported here".into(),
            )),
        }
    }
    /// "Check all feeds now": the server polls every subscription that
    /// isn't paused, in the background (studio 0.30), and answers with how
    /// many; a pull follows. Additive; the default refuses.
    fn poll_subscriptions(&self) -> Result<u32> {
        Err(CoreError::Other(
            "checking feeds isn't supported here".into(),
        ))
    }
    /// "Check now" for one subscription: a blyg's index is reconciled at
    /// once (true when anything changed); a feed is polled with the rest.
    /// A pull follows. Additive; the default refuses.
    fn check_subscription(&self, sub_id: &str) -> Result<bool> {
        let _ = sub_id;
        Err(CoreError::Other(
            "checking feeds isn't supported here".into(),
        ))
    }
    /// thumb: Some(1) / Some(-1) / None (clear).
    fn signal(&self, sub_id: &str, remote_id: &str, thumb: Option<i8>) -> Result<()>;
    fn mentions(&self) -> Result<Vec<Mention>>;
    fn set_mention_hidden(&self, mention_id: &str, hidden: bool) -> Result<()>;
    fn settings(&self) -> Result<Settings>;
    fn save_settings(&self, settings: &Settings) -> Result<()>;

    // ------- other people's versions & pins (remote, on demand, never polled)
    //
    // These read the public surface of the item's origin, unauthenticated: the
    // owner token never leaves the user's own blyg. Spec §8.4: only the current
    // version and pinned versions have content; the rest of the changelog is
    // metadata the UI shows without any affordance to open it.

    /// The public `changelog` of a reading item, oldest first, from
    /// `{origin}items/{id}.json` (blyg-kind subscriptions; RSS/L0 items return
    /// just their current version). Cached briefly; offline falls back to the
    /// last fetched changelog.
    fn remote_versions(&self, sub_id: &str, remote_id: &str) -> Result<Vec<RemoteVersion>> {
        let _ = (sub_id, remote_id);
        Err(CoreError::Other("unsupported".into()))
    }

    /// What the version browser shows for someone else's post: only the
    /// current and pinned versions (`model::shown_versions` over
    /// `remote_versions`). Unpinned versions don't appear at all.
    fn remote_shown_versions(&self, sub_id: &str, remote_id: &str) -> Result<Vec<RemoteVersion>> {
        self.remote_versions(sub_id, remote_id)
            .map(|v| shown_versions(&v))
    }

    /// A pinned version, from `{origin}items/{id}/v{n}.json` (cached forever).
    /// Refused locally, with no request for the version, when the changelog
    /// doesn't mark `version` pinned; that refusal and a 404 are both
    /// `Rejected{status: 404}` ("not pinned"). A `content_hash` mismatch is
    /// flagged in `hash_mismatch`, not an error.
    fn remote_pinned(&self, sub_id: &str, remote_id: &str, version: u32) -> Result<PinnedVersion> {
        let _ = (sub_id, remote_id, version);
        Err(CoreError::Other("unsupported".into()))
    }

    /// When the version the user last read (`read_version`) is pinned, that
    /// pinned version, so the UI can diff pinned → current (both sides
    /// public). `None` otherwise (unread, unpinned, not fetchable): the UI then
    /// shows only the changelog notes in between, never a diff.
    fn pinned_diff_base(&self, sub_id: &str, remote_id: &str) -> Option<PinnedVersion> {
        let _ = (sub_id, remote_id);
        None
    }

    // --- AI ---
    //
    // Additive, default-implemented (so other implementors compile): what the
    // app needs to record and disclose text it generated.

    /// The TK provenance tracked locally for an item, as `(keyed_to,
    /// scopes)`: one entry per scope of the text `keyed_to` (carry it to the
    /// current text with `tk::remap`). `None` = nothing tracked here.
    fn tracked_tk_provenance(
        &self,
        id: &LocalId,
    ) -> Option<(String, Vec<Option<ScopeProvenance>>)> {
        let _ = id;
        None
    }

    /// False once the server has answered 404 to the provenance endpoint:
    /// text generated in the app would then publish without disclosure.
    /// The default (and a backend that hasn't found out yet) says true.
    fn provenance_available(&self) -> bool {
        true
    }

    // --- reading ---

    /// False once the server has shown it lacks the owner-API read
    /// extensions (`GET /api/reading/imported` answered 404): the reading, mentions and
    /// site-settings screens then say "not available on this server" instead
    /// of showing an empty list. Additive; the default says "available".
    fn read_extensions_available(&self) -> bool {
        true
    }

    /// "Reply to a reading item": a local draft thread whose `stub_of` is
    /// `of` (spec: stubs are the reply shape), pushed like any draft. Local.
    /// Additive; the default refuses so implementors compile.
    fn create_stub(&self, of: &RemoteRef, content_md: &str) -> Result<LocalId> {
        let _ = (of, content_md);
        Err(CoreError::Other("replies aren't supported here".into()))
    }

    // --- scratch media ---
    // docs/SPEC.md § Scratch notes: an image pasted into a scratch note stays
    // on this Mac until the note is promoted (`crate::scratch_media`).

    /// Keep an image for a scratch note on this Mac and return the markdown
    /// URL that refers to it (`blyg-local:<sha256>.<ext>`). Local: no
    /// network, ever. `promote` uploads it and rewrites the reference.
    /// Additive; the default refuses so implementors compile.
    fn save_scratch_media(&self, bytes: &[u8], mime: &str) -> Result<String> {
        let _ = (bytes, mime);
        Err(CoreError::Other(
            "local images aren't supported here".into(),
        ))
    }

    /// The file behind a `blyg-local:` URL (for the previews), if it's here.
    /// Additive; the default knows none.
    fn scratch_media_file(&self, url: &str) -> Option<std::path::PathBuf> {
        let _ = url;
        None
    }

    // --- profiles ---
    // docs/SPEC.md § Profiles. Someone's public profile, from their public
    // files (manifest, blogroll, archive index; or an RSS/Atom feed),
    // fetched unauthenticated and **only when the user opens it**: never in
    // the background, never polled.

    /// The profile at `url`: a blyg origin, a post permalink, or an RSS/Atom
    /// feed URL, discovered the way the subscribe preview does it. Returns
    /// the cached copy without a request while it's fresh
    /// (`profile::PROFILE_TTL_MS`) unless `refresh`; otherwise fetches
    /// (blocking). When the fetch fails and a copy is cached, that copy comes
    /// back with `stale: true`. Connections are recomputed from local data on
    /// every call. Additive; the default refuses so implementors compile.
    fn profile(&self, url: &str, refresh: bool) -> Result<crate::profile::Profile> {
        let _ = (url, refresh);
        Err(CoreError::Other("profiles aren't supported here".into()))
    }

    /// The cached profile for `url`, whatever its age. Local, instant, no
    /// network: for showing something while `profile` runs. Additive.
    fn cached_profile(&self, url: &str) -> Option<crate::profile::Profile> {
        let _ = url;
        None
    }

    // --- about ---

    /// Whether the server carries the extensions in docs/SERVER.md: `Some(false)`
    /// for a stock blygger-studio (reading comes from upstream's own routes;
    /// AI disclosure for app-generated text, read-state sync and upload
    /// removal are off), `None` until a reading pull has found out.
    fn server_extensions(&self) -> Option<bool> {
        None
    }

    /// The connected server is older than blygger-studio 0.9: nothing syncs
    /// until it's updated (`CoreEvent::ServerOutdated` says when first seen).
    fn server_outdated(&self) -> bool {
        false
    }

    /// Whether read state syncs with the server (the last `GET /api/reading/imported`
    /// advertised `read_state: true`, extension 5; meta key `read_sync`).
    /// For the About window. Additive; the default says no.
    fn read_state_sync(&self) -> bool {
        false
    }

    /// Every profile fetched so far, one per origin, whatever its age.
    /// Local, no network (the composer's @-mentions read it). Additive.
    fn cached_profiles(&self) -> Vec<crate::profile::Profile> {
        Vec::new()
    }

    // --- responses --- (issue #7)

    /// Posts in the local reading list, and your own published posts, that
    /// quote, stub or fork the post `(origin, id)`: "seen in your network".
    /// One per post, the strongest relation winning (fork > stub > quote).
    /// Local, instant; a list, never a count. Additive; the default knows none.
    fn responses(&self, origin: &str, id: &str) -> Vec<Response> {
        let _ = (origin, id);
        Vec::new()
    }

    // --- reader folders ---
    // The Reader's folders of subscriptions: local only (this Mac's store),
    // never sent to the blyg and unrelated to the server's hoppers. One
    // folder per subscription. All local and instant. Additive; the
    // defaults know no folders and refuse changes, so implementors compile.

    /// Every folder, in display order.
    fn folders(&self) -> Vec<Folder> {
        Vec::new()
    }

    /// Subscription id → folder id, for each filed subscription.
    fn subscription_folders(&self) -> std::collections::HashMap<String, String> {
        Default::default()
    }

    /// A new folder at the end. `Rejected{400}` for an empty name,
    /// `Rejected{409}` for a name another folder has (any case).
    fn create_folder(&self, name: &str) -> Result<Folder> {
        let _ = name;
        Err(CoreError::Other("folders aren't supported here".into()))
    }

    fn rename_folder(&self, id: &str, name: &str) -> Result<()> {
        let _ = (id, name);
        Err(CoreError::Other("folders aren't supported here".into()))
    }

    /// Its subscriptions go back to unfiled.
    fn delete_folder(&self, id: &str) -> Result<()> {
        let _ = id;
        Err(CoreError::Other("folders aren't supported here".into()))
    }

    /// Move a folder to `index` in the display order (clamped).
    fn move_folder(&self, id: &str, index: usize) -> Result<()> {
        let _ = (id, index);
        Err(CoreError::Other("folders aren't supported here".into()))
    }

    /// File a subscription in a folder, or unfile it (`None`).
    fn set_subscription_folder(&self, sub_id: &str, folder: Option<&str>) -> Result<()> {
        let _ = (sub_id, folder);
        Err(CoreError::Other("folders aren't supported here".into()))
    }

    // --- end reader folders ---

    // --- quote targets ---
    // Opening a quote's (or a stub's, or a fork's) original that isn't held
    // here: its public files, fetched unauthenticated with `PublicClient`
    // (never the owner token) and only when the user opens it.

    /// `{origin}items/{id}.json` as a post to show. Blocking. Additive; the
    /// default refuses so implementors compile.
    fn public_item(&self, origin: &str, id: &str) -> Result<PublicItem> {
        let _ = (origin, id);
        Err(CoreError::Other("not supported here".into()))
    }

    /// A pinned version of a post that isn't held, `{origin}items/{id}/v{n}.json`
    /// (cached forever). Refused without a request when its changelog doesn't
    /// mark `version` pinned (§8.4), as `remote_pinned` does. Additive.
    fn public_pinned(&self, origin: &str, id: &str, version: u32) -> Result<PinnedVersion> {
        let _ = (origin, id, version);
        Err(CoreError::Other("not supported here".into()))
    }
}
