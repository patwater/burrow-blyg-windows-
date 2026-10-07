# Server requirements

Burrow talks to a blyg's **owner API**. Since studio 0.9, the upstream reference Worker
(https://github.com/blygger/blygger-studio) documents that API as an OpenAPI contract
(`openapi.json`; route guide in its `docs/api.md`). The app follows that contract (studio
0.32 is current) and needs **0.9 or later**. Every route the app shares with upstream is
checked against `openapi.json` in its tests (`crates/blyg-core/tests/fixtures/`).

**A stock blygger-studio works.** On studio 0.28 or later, **sign in with the browser**
(the default): Burrow registers itself as an OAuth client of your blyg, opens the
studio's consent page, and keeps the grant (a one-hour access token, renewed with a
rotating refresh token until the grant ends, at most 30 days after your studio
sign-in) in your Keychain. Signing out revokes it. Or make an **API token** in Studio →
More → Client access (REST API, all four permissions: read, draft, publish, manage;
it lasts 30 days) and paste it. A token missing a permission gets a 403 that names
it. On an older studio, or as a fallback, sign in with the **studio password**: Burrow
signs in at `{blyg-url}/studio/login` like the browser does, keeps the session cookie
in memory only (the password is in your Keychain), and signs in again when the
session ends (studio 0.28 also ended every earlier session once). Writing, publishing, versions, pins, subscriptions, mentions, settings and the
reading list all work. The reading list is built from upstream's own routes:
`GET /api/reading` lists the posts, `GET /api/imports/{sub}/{id}` reads each new or
changed one, and thumbs and hoppers come from `/api/signals` and each hopper.

### What a stock server limits

| Feature | On a stock server |
|---|---|
| AI disclosure for text generated **in Burrow** | Recorded on studio 0.28 or later: Burrow sends each TK scope's provenance with the text (`PATCH /api/items/:id {content_md, provenance}`, and on `POST /api/items`). On an older one it isn't, so before publishing such text Burrow says it will go out without the disclosure, and lets you cancel. Generation on the blyg's own server records its disclosure itself. |
| Read state across Macs | Stays on each Mac. |
| Removing an image you pasted, then deleted | Removed on studio 0.26 or later; on an older one it stays on the server. |
| Who a post replies to, or forks | Studio 0.18 or later sends it with each post. An older one doesn't, so Burrow fetches it from the author's blyg, a few posts per sync. |

Burrow says this once, the first time it meets a stock server, and lists it in the
About window.

### Extensions for everything

A Worker that carries these extensions gets all of the above:

| # | Extension | Why the app needs it |
|---|---|---|
| 1 | *Optional.* **Bearer-token owner auth**: `Authorization: Bearer <token>` is accepted wherever the owner session cookie is, when the Worker secret `BLYG_OWNER_TOKEN` is set. | On studio 0.28+ upstream's manual tokens and OAuth supersede it; a fork may still offer it (its owner token goes in the same "API token" field). Upstream's scoped tokens may not reach a fork's own extension routes, so such a fork keeps it for those. |
| 3 | **Reading rows for the app** (without it, Burrow builds the same rows from upstream's routes, more slowly): `GET /api/reading/imported?limit&before=<cursor>` → `{items, next, read_state, lineage}`. Imported items only (tombstones too), newest `observed_at` first, in the app's row shape: raw `content_md`, `origin`, `author`, `page`, `thumb`, `hoppers`, `read_version`, `pinned_version_retained`, `transclusions[]` with `cited`, and `stub_of`/`forked_from` as imported (null when none). `lineage: true` says the rows carry those two; without it the app fetches them from each post's public document. `limit` defaults to 100, max 500; `next` is an opaque cursor passed back as `before`; a bad cursor is `400`. `content_html` is raw, as stored, so the app sanitizes it before display. | Upstream's `/api/reading` is a different resource: own and imported posts, rendered and sanitized, offset-paged (≤ 50), with no Markdown, author, thumb or hoppers. The reader can't use it as-is. |
| 4 | **Client-recorded TK provenance**: `PUT /api/items/:id/tk-provenance {content_md?, scopes:[{index, model, sources?, at?} \| null]}` and `GET` of the same. Validated first, atomic with the text, never stores the instruction. | So text generated **in the app** is disclosed (`generated` + `blyg-tk-gen`) exactly like text the Worker generates itself. **Retired for studio 0.28+**, whose `PATCH /api/items/:id` takes `provenance` itself; Burrow uses this only when a server refuses that field (a strict 400 on the unknown key). A 403 here (a Worker that closes its extension routes to scoped tokens) reads as "disclosure not recordable". |
| 5 | *Optional.* **Read-state sync**: `read_state: true` and a per-item `read_version` on `GET /api/reading/imported`; `PUT /api/reading/:sub/:remoteId/read` and `POST /api/reading/read`. See [Extension 5](#extension-5-read-state-sync). | So a post you read on one Mac reads as read on your others, and a fresh install doesn't show everything unread. |

Extension 2 (owner JSON reads) is upstream now, so its number is retired. Extension 1
is superseded upstream (studio 0.28's manual tokens and OAuth) but not retired: forks
may still carry it.
Field-level contracts: `docs/SPEC.md` § API and § Client-recorded provenance.

## Extension 5: read-state sync

**Becoming upstream.** blygger-studio is taking this extension in, as two PRs agreed
with this app: PR 1 (read, built: the same write routes and bodies, plus read state on
upstream's own `GET /api/reading`) and PR 2 (unread: clearing read state, with a
tombstone against stale reads). Once they ship, a stock studio needs nothing from this
section. The app speaks both: the fork's extension below, and upstream's routes.

The server keeps, per reading row, the highest version the owner has read. It never
lowers that value on a read, so every read write is idempotent and replay-safe; only a
clear (PR 2) resets it.

**Migration.** One new table (in the reference Worker: `migrations/0012_read_state.sql`;
upstream: `0025_read_state.sql`, rebuilt by `0026_read_state_clear.sql` with a nullable
`read_version` and an `unread_at` column):

```sql
CREATE TABLE read_state (
  subscription_id TEXT NOT NULL,
  remote_id       TEXT NOT NULL,
  read_version    INTEGER NOT NULL,
  updated         TEXT NOT NULL,
  PRIMARY KEY (subscription_id, remote_id)
);
```

It also adds two triggers, so a row goes away when its imported item or its subscription
is deleted. Upstream's change triggers also advance the `reading` revision of
`GET /api/changes`, so a mark on one device shows on another's next pull.

**Capability.**

- The fork: `GET /api/reading/imported` answers `{items, next, read_state: true}`, and
  each item carries `read_version` (an integer, or `null` when unread).
- Upstream (PR 1): `GET /api/reading` answers with top-level `read_state: true`, and each
  imported entry carries `imported.readVersion` (camelCase; an integer, or `null`), and
  (with PR 2's studio) `imported.version`, the version held. PR 2 adds
  `read_state_clear: true`.

The app reads the read version only when `read_state` is `true`. It checks the flags on
every pull and remembers the last answer.

**Writes** (owner auth, like every `/api` call):

| Call | Body | Success | Errors |
|---|---|---|---|
| `PUT /api/reading/:sub/:remoteId/read` | `{version: integer ≥ 0, read_at?}` | `200 {ok: true, stored: bool, read_version: n \| null}`. Stores `max(existing, version)`. For an unknown subscription or item, or a read older than the row's last clear: `stored: false`, nothing written. | `400` bad body, `401`, `403` |
| `POST /api/reading/read` | `{items: [{sub, remote_id, version, read_at?}]}`, at most 500 | `200 {ok: true, received: n}`. The same max-merge per row. Unknown or stale rows are skipped. | `400` for any malformed entry or more than 500 entries (nothing is written), `401`, `403` |
| `DELETE /api/reading/:sub/:remoteId/read` (PR 2) | none | `200 {ok: true, stored: false, read_version: null}`. Clears the row and records `unread_at` (the server's time). Idempotent. | `401`, `403` |
| `POST /api/reading/unread` (PR 2) | `{items: [{sub, remote_id}]}`, at most 500 | `200 {ok: true, received: n}`, all or nothing. | `400`, `401`, `403` |

`read_at` (ISO-8601 with an offset, e.g. `2026-10-06T12:00:00Z`) is accepted only by a
server that advertises `read_state_clear`: a read earlier than the row's `unread_at` is
ignored, so a queued read can't undo a later "mark unread" from another device. Upstream
bodies are strict, so the app **never** sends `read_at` to a server that doesn't advertise
the flag. Upstream's `400` body is `{error, issues: [{path, message}]}`.

Unknown rows are acknowledged, never `404`, because the app reads a `404` from these
endpoints as "this server doesn't keep read state".

**What the app does:**

- Marking posts read or unread is instant and local: one post, a selection (⌘-click,
  ⇧-click, ⌘A in the Reader's list; r / u, or the row's menu), or the open post. On a
  server with the capability it also queues one op per reading row (duplicates of the
  same post, too) in the outbox, so marks survive going offline and restarting. A row has
  at most one waiting op, and the last action wins: an unread replaces a waiting read and
  the other way round; repeated reads coalesce to the highest version.
- Several waiting ops go as one `POST /api/reading/read` or `/unread` (500 per call), so
  marking a whole list is one write against the owner budget; a single mark is one
  `PUT` or `DELETE`.
- A read carries `read_at`: when it was marked on this Mac, kept with the row until the
  server confirms it (reads from before this was tracked count as the epoch, so any clear
  wins over them). A `PUT` the server answers `stored: false` because of a later clear
  takes the server's state: the row is unread here too.
- A pull sets the local value to `max(local, server)`, except that a row marked unread
  here ignores server read versions at or below the one it was read at (its *floor*). On
  a server with `read_state_clear`, a row this Mac had confirmed but the server now
  reports lower or unread was cleared on another device, and it becomes unread here; a
  row read here that the server hasn't got is queued again (with its `read_at`).
- **Unread without `read_state_clear`** (the fork's extension 5, or upstream PR 1 alone)
  stays on this Mac: no unread op is sent, and the floor keeps the server's old read
  version from re-marking the row read on every pull, so there's no loop of requests.
  Reading the post again (here, or a newer version on another device) ends it. If the
  server later gains `read_state_clear`, those local unreads are sent then.
- The first time a database sees the capability, the app batch-uploads every read version
  it holds (500 per call) and records that in its `meta` table.
- A `403` on a write (a Worker that keeps these writes to the owner's own token) keeps read
  state on this Mac for the rest of the session, says so once, and doesn't retry.
- **Without the capability**, read state stays on this Mac. The app makes no requests to
  these endpoints and shows no errors. A `404` from them turns sync off and drops anything
  queued.

**Privacy.** The table is owner-only. No public page, feed, `blyg.json`, item document or
export reads it. It holds only numbers (and, upstream, the time of a clear) keyed by
subscription and item, never text.

## Also used when present

- `POST /api/media` answering `200 {…, duplicate: true}` for identical bytes on
  the same item. Not upstream (upstream issue #7).
- `DELETE /api/media/:id` (removes an upload) is upstream since studio 0.16,
  answering `{ok, outcome: "deleted" | "detached"}`. On an older server it 404s, and
  an abandoned paste leaves its file there.

## What the app uses from upstream

These follow `openapi.json` exactly. The differences from the pre-0.9 API that
mattered here:

- Collections answer `{items, total, offset, limit}`; the app pages with
  `offset`/`limit` (100 a page).
- Items are created with `POST /api/items {mode: "blank" | "fork", …}`, which
  answers `201` with the item. `POST /api/fork` is gone.
- Edits are `PATCH /api/items/:id {content_md?, kind?, responses?}`. A draft's kind
  changes in place until its first publish, so changing it no longer makes a new
  draft. Unknown fields are a 400.
- Pins are `PUT /api/items/:id/versions/:version/pin`, with no body.
- An item's `responses` is `"default" | "show" | "hide"`. The app works out whether a
  page shows responses from that and settings' `show_responses_default`.
- Items carry no permalink. The app uses `{blyg-url}/f/{id}` (`t/` for threads);
  `blyg-url` includes any mount path.
- Pause and resume are `PATCH /api/subscriptions/:id {paused}`. Settings, mention
  hiding and subscription edits are `PATCH` too.
- Errors are `{error, errors?, issues?}`. The app shows `issues` (validation
  failures) as details.

## Degrading gracefully

- **No extension 1:** sign in with the studio password instead.
- **A server older than studio 0.9:** the connect sheet says it needs updating. A
  blyg already connected gets a notice when Burrow starts (and a status-bar
  notice with a **How to update** link), and Burrow pushes nothing to it: on an
  old server a new route's 404 would read as "deleted" and duplicate your posts.
  Your edits wait in the outbox and go up once the server is updated. Burrow
  recognises an old server by `GET /api/items` answering without `total` (or
  404).
- **No extension 3:** the reading list comes from upstream's own routes (see above).
- **No extension 4, and older than studio 0.28:** before publishing text that was generated
  in the app, the app warns that it will go out **without** AI disclosure, and lets you cancel.
- **Studio 0.32+ (`GET /api/changes`):** each sync first reads the change counters and
  fetches only the collections whose domains moved (everything, every 10 minutes), so the
  app syncs every 15 s instead of every 60 s. An older server is read whole every 60 s.
- **Work budgets (studio 0.28+):** a `429` keeps the change queued and retries after
  `Retry-After`; the app says "The blyg asked Burrow to slow down". Autosave pushes for
  one post are at least 3 s apart, about 20 writes a minute at most.
- **No extension 5:** read state stays on each Mac, as before.

## Updating an older server

Follow upstream's guide,
[Upgrading to Blygger Studio 0.11](https://github.com/blygger/blygger-studio/blob/main/docs/upgrading-to-0.11.md).
It covers source and Worker-archive installs, the one database migration, and
path-mounted blygs. A server that carries the extensions above needs them
re-applied on top of the new version, so update it from wherever its extensions
come from. Once it's updated, Burrow picks it up on its next sync; there's
nothing to do in the app.

## Optional: server-side generation with Gemma 4

A Worker with a Workers AI binding (`"ai": {"binding": "AI"}`) can run TK generation on
`@cf/google/gemma-4-26b-a4b-it` with no API key. The app's "Generate on my blyg server" provider
uses whatever the Worker is configured to use.

## Status

Upstream's plan (its `docs/migration.md` §3) is for this app to run on the documented
API alone, with OAuth in place of extension 1. The remaining extensions are proposed in
upstream issues #11 (provenance, read state, reading rows), #12 (lineage in reading
rows) and #7 (attachments).
