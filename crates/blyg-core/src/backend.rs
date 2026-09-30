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
    #[error("not authorised — check the token")]
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
    /// Item has never reached the server (still local-only) and the op needs a server id.
    #[error("not on the server yet — still syncing")]
    NotSynced,
    #[error("storage: {0}")]
    Storage(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

pub struct PublishOutcome {
    pub version: u32,
    pub permalink: String,
    pub warning: Option<String>,
}

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
    fn set_show_responses(&self, id: &LocalId, show: bool) -> Result<()>;

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
    /// extensions (`GET /api/reading` answered 404): the reading, mentions and
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
}
