//! blyg-core: everything that isn't pixels. API client, local SQLite store,
//! outbox + sync worker, and the `Backend` trait the UI talks to.

pub mod api;
pub mod backend;
pub mod config;
pub mod live;
pub mod model;
pub mod profile; // --- profiles ---
pub mod scratch_media;
pub mod state;
pub mod store;
pub mod sync;
pub mod tk;
mod util;

pub use backend::{
    Backend, CoreError, MediaRef, Promote, Promoted, PublishOutcome, Result, promotion_kind,
};
pub use config::{Config, ConfigStore};
pub use live::LiveBackend;
pub use model::*;
pub use profile::{BlogrollEntry, Profile, ProfileKind, ProfilePost, Relation}; // --- profiles ---
pub use sync::SyncOptions;
