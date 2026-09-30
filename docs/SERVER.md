# Server requirements

Blygger Desktop talks to a blyg's **owner API**. The upstream reference Worker
(https://github.com/blygger/blygger-spec, `worker/`) has a write-only owner API behind a
studio password cookie. This app needs four small, additive extensions to it. They're
read-only or bookkeeping endpoints, with no schema changes. Until they're upstream, you
need a Worker that carries them. A fifth, optional extension syncs reading-list read state
between your Macs; it adds one table.

| # | Extension | Why the app needs it |
|---|---|---|
| 1 | **Bearer-token owner auth**: `Authorization: Bearer <token>` is accepted wherever the owner session cookie is, when the Worker secret `BLYG_OWNER_TOKEN` is set. | A native app can't hold a studio cookie cleanly. |
| 2 | **Owner JSON reads**: `GET /api/items`, `GET /api/items/:id` (with `versions`), `GET /api/subscriptions`. | Upstream renders these lists as HTML only. |
| 3 | **Read extensions**: `GET /api/reading?limit&before=<cursor>` (opaque keyset cursor, pages ≤ limit), `GET /api/mentions`, `GET /api/settings` (public-safe fields only), `GET /api/hoppers`; `show_responses` added to the item JSON. | The reading list, mentions and settings screens. |
| 4 | **Client-recorded TK provenance**: `PUT /api/items/:id/tk-provenance {content_md?, scopes:[{index, model, sources?, at?} \| null]}` and `GET` of the same. Validated first, atomic with the text, never stores the instruction. | So text generated **in the app** is disclosed (`generated` + `blyg-tk-gen`) exactly like text the Worker generates itself. |
| 5 | *Optional.* **Read-state sync**: `read_state: true` and a per-item `read_version` on `GET /api/reading`; `PUT /api/reading/:sub/:remoteId/read` and `POST /api/reading/read`. See [Extension 5](#extension-5-read-state-sync). | So a post you read on one Mac reads as read on your others, and a fresh install doesn't show everything unread. |

Field-level contracts: `docs/SPEC.md` § API and § Client-recorded provenance.

## Extension 5: read-state sync

The server keeps, per reading row, the highest version the owner has read. It never
lowers that value, so every write is idempotent and replay-safe.

**Migration.** One new table (in the reference Worker: `migrations/0012_read_state.sql`):

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
is deleted.

**Capability.** `GET /api/reading` answers `{items, next, read_state: true}`, and each item
carries `read_version` (an integer, or `null` when unread). The app reads `read_version`
only when `read_state` is `true`. It checks the flag on every pull and remembers the
last answer.

**Writes** (owner bearer auth, like every `/api` call):

| Call | Body | Success | Errors |
|---|---|---|---|
| `PUT /api/reading/:sub/:remoteId/read` | `{version: integer ≥ 0}` | `200 {ok: true, stored: bool, read_version: n \| null}`. Stores `max(existing, version)`. For an unknown subscription or item: `stored: false`, nothing written. | `400` bad body, `401` |
| `POST /api/reading/read` | `{items: [{sub, remote_id, version}]}`, at most 500 | `200 {ok: true, received: n}`. The same max-merge per row. Unknown rows are skipped. | `400 {error, errors?: [{index, reason}]}` for any malformed entry or more than 500 entries (nothing is written); `401` |

Unknown rows are acknowledged, never `404`, because the app reads a `404` from these
endpoints as "this server doesn't have extension 5".

**What the app does:**

- Marking a post read stays instant and local. On a server with the capability, it also
  queues one `read` op per reading row it marked (duplicates of the same post, too) in the
  outbox. The outbox sends them like any other change, so they survive going offline and
  restarting. Repeated reads of one row coalesce to its highest version.
- A pull sets the local value to `max(local, server)`. It never lowers it.
- The first time a database sees the capability, the app batch-uploads every read version
  it holds (500 per call) and records that in its `meta` table. After that, a row held
  locally that is ahead of the server is queued again on the next pull.
- **Without extension 5**, or without extension 3, read state stays on this Mac. The app
  makes no requests to these endpoints and shows no errors. A `404` from them turns sync off
  and drops anything queued.

**Privacy.** The table is owner-only. No public page, feed, `blyg.json`, item document or
export reads it. It holds only numbers keyed by subscription and item, never text.

## Also used when present

- `DELETE /api/media/:id` (removes an upload; 404 unknown, 409 for the avatar)
  and `POST /api/media` answering `200 {…, duplicate: true}` for identical bytes
  on the same item. Without them, an abandoned paste leaves its file on the
  server.

## Degrading gracefully

- **No extension 1:** the app can't sign in with a token. (Password sign-in is planned.)
- **No extensions 2/3:** drafts still work locally and publish. Drafts written in the web studio
  won't appear, and the reading, mentions and settings screens say "not available on this server".
- **No extension 4:** before publishing text that was generated in the app, the app warns that
  it will go out **without** AI disclosure, and lets you cancel.
- **No extension 5:** read state stays on each Mac, as before.

## Optional: server-side generation with Gemma 4

A Worker with a Workers AI binding (`"ai": {"binding": "AI"}`) can run TK generation on
`@cf/google/gemma-4-26b-a4b-it` with no API key. The app's "Generate on my blyg server" provider
uses whatever the Worker is configured to use.

## Status

The plan is to propose these extensions upstream. See the repo's issues for progress.
