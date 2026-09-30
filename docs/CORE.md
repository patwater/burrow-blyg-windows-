# blyg-core notes

How `crates/blyg-core` works, for whoever touches it next. The spec is
`docs/SPEC.md`; this file covers only the decisions the spec leaves open.

## Layout

| Module | Role |
|---|---|
| `api/` | Blocking `ureq` 2 (rustls) client, one method per owner endpoint. Errors: transport → `Offline`, 401 → `Unauthorized`, 404 → `NotFound`, other non-2xx → `Rejected{status, error, errors}` (5xx too; `api::is_transient` treats ≥ 500 as retryable). Patch-3 reads return `Ok(None)` on 404. `Debug` redacts the token. |
| `store/` | One `rusqlite` connection behind a mutex, WAL, `user_version` migrations (`schema.rs`, append-only). Items + FTS5 trigram index, outbox, versions cache, reading list, subscriptions. |
| `sync/` | `Engine` (flush, pull, health/status, events) plus the worker thread. |
| `live.rs` | `LiveBackend: Backend`. |
| `tk.rs` | The Worker's TK scope grammar, and provenance validation/remapping (see § Provenance). |
| `config/` | the Ghostty-style config file (`keys.rs` is the single key table; `parse`, `edit` preserves comments on write-back, `paths` XDG → Application Support, `migrate` from the old TOML files, `show` for `+show-config`); `tokens.rs`: `TokenStore` trait (`KeychainTokenStore` behind the default `keychain` feature, `MemoryTokenStore` for tests). |

## Outbox

Ops are `create`, `save`, `recreate` (a pre-publish kind change of a draft that
already exists server-side) and `delete_remote` (retire the old draft after a
recreate). Edit ops carry no content: a push sends the working copy as it is
*at push time*. So coalescing is just "move the newest not-in-flight op's
`not_before`", and 50 keystrokes become one push. A save made while an op is
in flight queues a new op.

Ops run strictly in `seq` order per item. A later op for a `LocalId` waits for
its `create`, which maps the server id. Different items don't block each other.
`net` (a mutex in `Engine`) serialises every item-touching network sequence:
worker flush, pull, publish and delete.

## Conflicts (deviation from the spec's `updated` rule)

The spec says to flag a conflict when the server's `updated` > `base_updated`
and the item has local edits. That doesn't work against the real Worker:
`saveWorkingCopy` bumps `updated` on *our own* draft saves (false positives)
and never bumps it for published items (false negatives). Core compares
**content** instead. There's a conflict when the item has unpushed edits and
the server's `content_md` differs both from `base_content` (what we last synced
against) and from the local working copy. `base_updated` is still recorded.

This is checked in two places:
1. On every pull (`merge_all`).
2. **Before every push** of a `save` or `recreate`: a `GET /api/items/:id`
   first, so an offline edit can't silently overwrite a studio edit when the
   network comes back. `sync_now` flushes before it pulls, so without this
   check it would clobber. The cost is one extra GET per debounced push.

Conflicted items are skipped by the flush. The resolutions:
- `KeepMine`: base := theirs, and a push is queued.
- `TakeServer`: working copy := theirs, and local ops are dropped.
- `KeepBoth`: the same as `TakeServer`, plus a new local draft holding mine.

## set_kind

The Worker only reads `kind` on `POST /api/items` (`api.ts`); `PUT` ignores it,
and there's no other route. Before publish:
- a draft that is still local-only just changes its kind;
- a server-side draft gets `recreate`: POST a new draft of the new kind with the
  current content, remap the server id, then `delete_remote` the old one.

Toggling back before the push is a no-op, because `server_kind` is tracked.
After publish, `set_kind` returns `Rejected{409}`.

## Provenance (client-recorded TK disclosure)

The Worker keys a working copy's TK provenance by scope **position**
(`items.tk_provenance_json`), so a plain `PUT /api/items/:id` that adds,
removes or reorders `[TK]…[/TK]` scopes would shift disclosure onto the wrong
span. Core therefore:

- tracks provenance per scope locally (schema v3: `tk_prov` JSON, keyed to the
  text in `tk_prov_md`, and `tk_prov_dirty`). `Backend::save_with_provenance`
  sets it (validated like the server does: one entry per scope, only on
  generated scopes, only citing sources quoted in the scope);
- carries it along on every plain `save` (`tk::remap`: scopes are matched by
  instruction, in order; a scope whose output was rewritten entirely by hand
  loses it; malformed mid-typing text keeps the old keying until it parses);
- pushes with the combined `PUT /api/items/:id/tk-provenance {content_md,
  scopes}` whenever the scope structure changed since `base_content` or the
  tracked provenance changed, else a plain PUT. Provenance the server holds but
  this client never saw (another client, the Worker's own `/generate`) is
  fetched with `GET …/tk-provenance` and remapped first;
- after a `create`/`recreate` (POST can't carry provenance) queues a combined
  push; a 404 from the endpoint means no extension (plain PUT, and
  `LiveBackend::provenance_available()` turns false); a 400 retries without
  sources, then saves the text alone and emits an `Error` event;
- forgets tracked provenance when the server's text replaces ours (pull,
  take-server, restore: the Worker clears its cache on restore).

## Connecting

`api::verify_connection(url, token)` is the Connect sheet's check:
`GET /api/items` with the token. It maps a transport failure to
`ConnectError::Unreachable`, 401 to `WrongToken`, 404 to `MissingExtensions`
(a blyg without docs/SERVER.md's extensions) and anything else to `Other`.

## End-to-end tests

`crates/blyg-core/tests/e2e.rs` runs against a real Worker under
`wrangler dev --local` (all `#[ignore]`d, so CI doesn't need one):
`BLYG_WORKER_DIR=<worker dir> scripts/e2e-local.sh`. The script derives a
local-only wrangler config (no routes, no Workers AI binding), random dev
secrets and scratch D1/R2 state, starts two instances (the second is a blyg to
subscribe to), and stops only what it started.

## Reading list

One row per `(subscription_id, remote_id)`, upserted. The local `read_version`
(a number only) survives edits. The text of the version read is never kept
(spec §8.4); migration v2 dropped the old `read_snapshot_md` column. An RSS guid change on edit (same
subscription, same resolved page URL, and the old id is no longer listed) is
treated as the same row. `reading()` collapses duplicates on the absolute page
URL, falling back to `(origin, remote_id)`, and prefers the blyg-kind
subscription. It hides tombstones unless they have a thumb or a hopper.
`page` is resolved against `origin` in the API layer. The `next` cursor is
opaque and passed back verbatim as `before`. Rows the server stopped returning
are pruned: all of them after a complete fetch, or only those inside the
fetched window after a partial one.

## Other people's versions & pins

- `api/public.rs::PublicClient` is a **separate** `ureq::Agent` with no token.
  It is the only path to foreign origins, so the owner token can't leak there
  (`tests/versions.rs::the_owner_token_never_reaches_a_foreign_origin`).
- URLs: `model::{item_doc_url, pin_doc_url}` join `items/{id}.json` /
  `items/{id}/v{n}.json` onto the origin (a missing trailing slash is added,
  the id is percent-encoded as one segment, and only http(s) is accepted).
  This works for subdirectory mounts.
- `remote_versions` returns the full changelog, which the reading list uses
  for its notes. `remote_shown_versions` / `model::shown_versions` return only
  current + pinned, which is what the version browser lists. RSS/L0 rows give
  only their current version and make no request.
- `remote_pinned` returns the version from the `remote_pins` cache if it's
  there (pins are immutable, so they're kept forever). Otherwise it refuses
  with `Rejected{404}` and makes no request when the changelog doesn't mark
  the version pinned. It then fetches, and a 404 also means not pinned (and
  drops the cached changelog). The changelog cache (`remote_changelog`) lasts
  `CHANGELOG_TTL_MS` = 2 min, or until the reading row's version passes it.
  Offline, it falls back to the cache.
- `content_hash` = `"sha256:" + hex(SHA-256(content_md UTF-8))` (digest §1.3,
  upstream `util.ts::contentHash`). A mismatch sets `hash_mismatch` and the
  content is adopted anyway. SHA-256 is hand-rolled in `util.rs` (FIPS
  known-answer tests) so there's no new dependency or `Cargo.lock` churn.
  Swapping in `sha2` is a one-liner.
- `pinned_diff_base` returns the pinned version only when `read_version` is
  pinned.
- Withdrawal (`store/reading.rs::retainable`): when a tombstone arrives, its
  content is dropped unless a pin backs it. That's the server's
  `pinned_version_retained` (its bytes are kept) or a pin in `remote_pins`
  (the newest cached one is used). `ReadingItem.pinned_version_retained` +
  `pin_url()` give the attribution. The DB runs with `secure_delete=ON`, so
  purged bytes are overwritten on disk.
- Own items: `pin` refuses an endcap locally with 409, refreshing versions
  first if the version is unknown. `restore` only loads the working copy.

## Performance

5,000 items (`store::tests::bench_search_5000`, release build):
- `search` takes about 0.1 ms, using a trigram phrase match.
- A query shorter than 3 characters scans in Rust.
- `items()` takes about 0.5 ms from the snapshot cache, which is keyed on
  SQLite `total_changes()`, or about 4–5 ms after any write.
