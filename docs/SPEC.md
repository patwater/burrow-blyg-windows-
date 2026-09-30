# Blygger Desktop — spec

A native macOS client for a Blygger blog ("blyg") running the Blygger reference
Worker plus the owner-API extensions in `docs/SERVER.md`. Goal: **zero
friction between a thought and a published post.** The feel target is
Notational Velocity: one window, type to search, ⏎ to create, nothing ever
waits on the network.

The agreed interaction design is the clickable mock in `docs/prototype/index.html`
(open it in a browser, and play the ▶ demos). When this document and the mock
disagree on *feel*, follow the mock. On *API behaviour*, this document wins.

## Decisions (settled)

| Area | Decision |
|---|---|
| UI | **GPUI** (Zed's framework) + `gpui-component` where it helps (text input/editor). |
| Local data | **SQLite** via `rusqlite` (`bundled`, FTS5). Local-first: UI reads only from SQLite. DuckDB was considered and declined because this is an OLTP/tiny-write workload, and DuckDB can read the SQLite file later for analytics. |
| Network | Blocking HTTP (`ureq`, rustls) on background threads. No tokio in the app. |
| Auth | Base URL + `BLYG_OWNER_TOKEN` bearer. Token lives in the **macOS Keychain** (`keyring` crate), never in a file. |
| Platform | macOS first. |
| Layout | **Side by side**: item list on the left (~38%), editor on the right. |
| Theme | **Tufte colours** (paper `#fffff8`, ink `#111`, accent `#a4271b`; dark: `#161513` / `#e4dfd3` / `#e0775a`), follows system light/dark. Default in the mock was dark. |
| Fonts | ET Book reads poorly on screen per the user. **Fonts are user-switchable in the app** (separate choices for writing font and UI font) with screen-first defaults: writing = **Literata** (bundled, OFL), UI/list = **Inter** (bundled, OFL). Offer at least: Literata, Source Serif 4, iA Writer Quattro (bundled, OFL); system New York, Charter, SF Pro, Menlo; ET Book stays available as an option. Size adjustable (⌘+ / ⌘−). |
| Quick capture | Global hotkey, default **⌃⌥B**, **configurable** in settings. |
| Scope | Everything in §Scope is in v1. |

## Interaction spec (main window)

- **Omnibar** at top, always there. Typing filters the list live (substring,
  case-insensitive, matched text highlighted). `↑/↓` move the selection, and the
  editor previews the selected item as you move (NV behaviour).
  `⏎` opens the selection; **`⏎` with no matches creates a draft seeded with the
  query** and puts the caret at the end of it. `esc` clears the query.
- **Editor**: plain Markdown text, no toolbar. Every keystroke saves locally;
  push to the server ~800 ms after typing stops. There is no save button.
- **Status bar** (bottom): `◦ fragment` / `≡ thread` (clickable, toggles),
  counter `214 / 1000` (amber >900, red >1000, for fragments only; threads show
  "N chars · no limit"), a centre banner when over the limit ("Too long for a
  fragment · ⌘T makes it a thread"), sync state (`synced` / `saved on this Mac`
  / `syncing…` / `offline · N changes waiting`), version (`draft` / `public v3`
  / `public v3 · unpublished edits`).
- **List rows**: title (first line), then `◦ fragment`/`≡ thread`, a status pill
  (`draft` or `vN`), a dot for unpublished edits, and relative time.
- **Keys**: `⌘L` focus omnibar · `esc` (in editor) back to omnibar ·
  `⌘⏎` publish · `⌘T` fragment⇄thread · `⌘E` preview (see § Full editor) · `⌘O` open
  permalink in browser · `⌘,` settings · `⌘+/⌘−` font size · `⇧⌘⌫` delete a
  draft or scratch note.
- **Delete / Withdraw** (rule 5 below): `⇧⌘⌫` (Post › Delete Draft…) on a draft
  or scratch note drops a sheet: "Delete this draft? … This can't be undone."
  `⏎` deletes, `esc` cancels; the list moves on to the next post and a toast
  says "Draft deleted". Published posts are never deleted: `⇧⌘⌫` on one says
  so, and Post › Withdraw… asks instead, with an optional note ("Withdrawn
  posts stay listed as withdrawn. You can't undo this."), then calls
  `withdraw(id, note)`. Withdraw has no key on purpose, since it's permanent.
  (`⌘⌫` stays delete-to-line-start in the editor.)
- **Publish** (`⌘⏎`): a sheet drops from the title bar: "Publish "…" as vN+1",
  with an optional version note input. `⏎` publishes and `esc` cancels. If the
  item is a fragment over 1000 chars, don't open the sheet: shake the status bar
  and say ⌘T. A toast shows the result: "Published v2 · blyg.example.com/f/… · ⌘O opens it".
- **Paste a link over selected text** (as WordPress does): when the selection
  is on one line and the clipboard holds only a web or `mailto:` address, ⌘V
  makes it `[text](url)` (brackets escaped, `<…>` around an address with
  parentheses; spaces caught in the selection stay outside). An address or an
  existing link that's selected is simply replaced. One undo step.
- **Paste/drop an image**: this inserts `![uploading…]()` at the caret, uploads
  via `POST /api/media` (without `item_id`), then replaces the placeholder with
  `![](<blyg origin>/media/<id>.<ext>)`. Two Worker behaviours decide this: public
  pages append every *attachment* (media with the item's id) after the text, so
  an attached image that's also inline shows twice; and a relative `media/…` in
  the text renders page-relative, which 404s under `/f/<id>/`. If the placeholder
  is gone by the time the upload finishes, the file is deleted again
  (`DELETE /api/media/:id`).
- **Conflict** (the server changed since the last sync and there are local edits):
  a sheet with a side-by-side view "On this Mac" vs "On the server", with keys `1`
  keep mine / `2` take the server's / `3` keep both.
- **Offline**: nothing blocks. The status shows the queued count, and the queue
  flushes when the network is back.
- **Quick capture**: the global hotkey shows a small floating panel over any
  app, containing a text area and a fragment counter. `esc` keeps it as a local
  scratch note (see § Scratch notes), `⌘D` saves a draft, `⌘⏎` publishes.

## Scope (v1, all in)

Writing: fragments & threads, autosave, publish with note, paste/drop images,
quick capture, markdown preview, withdraw, version history + restore, pin
(irrevocable, so it confirms), delete drafts, quote another blyg (picker, ⌘K or typing `![[`, that
inserts `![[<id>]]`), fork.
Reading & managing: open on the web, read subscriptions (reading list), subscribe
and unsubscribe, blogroll flag, blyg settings (title, bio, links), mentions &
responses.

Phase 1 (parallel agents): core crate, writing UI, and Worker read endpoints.
Phase 2: reading/management UI, quote picker, fork, settings UI.

## Architecture

```
crates/blyg-core   model.rs (types) · backend.rs (Backend trait, the UI seam)
                   api/  (ureq client, one fn per endpoint, typed errors)
                   store/ (rusqlite: items, versions cache, outbox, reading, subs, FTS5)
                   sync/ (worker thread: debounce, outbox flush, periodic pull, conflicts)
                   live.rs (LiveBackend: impl Backend over store+api+sync)
crates/blyg-app    GPUI app. Talks only to `Arc<dyn Backend>`. Contains a
                   FakeBackend (in-memory, seeded) so the UI runs with no server:
                   `BLYGGER_FAKE=1 cargo run`.
```

The `Backend` trait and the model types in `blyg-core/src/{backend,model}.rs`
are the contract. Changing them is the orchestrator's call. An agent who needs
a change says so in its final report and doesn't make it unilaterally.
(Adding *private* helpers is fine.)

Configuration: one Ghostty-style plain-text file, `~/.config/blygger/config` (list every key with
`blygger +show-config --default --docs`). Secrets live in the Keychain. App state (SQLite db, caches,
media) lives in `~/Library/Application Support/org.blygger.desktop/`.

## API (owner, bearer auth)

Base: the configured `blyg-url`. Every `/api` call sends
`Authorization: Bearer <token>`, and JSON bodies are sent with `content-type: application/json`.
Failure `401 {"error":"unauthorized"}`. Error bodies are `{error, errors?}`.

The authoritative contract for existing endpoints is
`docs/SERVER.md`. Upstream source: `worker/src/{api.ts,importer/api.ts,mentions/api.ts}` in
https://github.com/blygger/blygger-spec.
Read it before guessing a body shape. The key points:

- `GET /api/items` → `{items: Item[]}` newest-updated first (drafts, public and withdrawn).
  Item = `{id, kind, authored_kind, status, version, dirty, created, updated,
  content_md, stub_of, forked_from, permalink, show_responses*}`.
- `GET /api/items/:id` → Item + `versions: [{version, published_at, note, pinned, endcap}]`
- `POST /api/items {content_md?, kind?, stub_of?}` → `201 {id, kind, status}`
- `PUT /api/items/:id {content_md}` → saves the working copy (doesn't publish)
- `POST /api/items/:id/publish {note?}` → `{ok, version, warning?}`; 400 on over-limit / bad transclusion
- `POST /api/items/:id/withdraw {note?}`, `/pin {version}`, `/restore {version}`, `DELETE /api/items/:id` (drafts only)
- `POST /api/media` multipart `file` (+`item_id`, `alt`) → `201 {id, url, mime}`
- `POST /api/fork {origin,id,version}`
- `PUT /api/items/:id/responses`, `PUT /api/mentions/:id/hidden`, `PUT/DELETE /api/signals/:sub/:remoteId` (read the source for the bodies)
- Subscriptions: `GET/POST /api/subscriptions`, `PUT/DELETE /api/subscriptions/:id`, `POST …/pause|resume|resync`
- `PUT /api/settings {...}`

**Read extensions** (an owner-API extension, see `docs/SERVER.md`. Where it's missing, core must degrade gracefully: a 404 means "feature unavailable", not a crash):

- `GET /api/reading?limit=N&before=<cursor>` → `{items: ReadingItem[], next: string|null}`,
  newest `observed_at` first; the default limit is 100 and the max is 500. `next` is an **opaque cursor**:
  pass it back as `before` verbatim. Pages are ≤ limit. `page` is origin-relative (resolve it against `origin`).
  Settings come back with upstream defaults as strings ("" = unset). ReadingItem =
  `{subscription_id, remote_id, subscription_title, origin, kind, state, version,
  created, updated, observed_at, content_md, content_html, author: {name,url}|null,
  page, thumb: 1|-1|null, hoppers: string[]}`
- `GET /api/mentions` → `{mentions: Mention[]}` (fields as in `model.rs::Mention`)
- `GET /api/settings` → `{site_title, author_name, author_bio, site_url, theme,
  avatar_media_id, author_links: [{label,url}]}` (never secrets)
- `GET /api/hoppers` → `{hoppers: [{id, name, slug, public, count}]}`
- `show_responses: bool` added to the Item JSON.

**Read-state sync** (optional extension 5, see `docs/SERVER.md`): `GET /api/reading` adds
`read_state: true` and a per-item `read_version: number|null`;
`PUT /api/reading/:sub/:remoteId/read {version}` and
`POST /api/reading/read {items: [{sub, remote_id, version}]}` (≤ 500) store
`max(existing, version)`.

Transclusion syntax is `![[<26-char id>]]` alone on its own line, and is only
valid in threads. See upstream `worker/src/transclusion.ts` for which ids resolve
(local and/or imported).

## Safety

- **Never write to the production blyg while developing.** Test against a mock
  HTTP server (core) or a local `wrangler dev` of a patched reference Worker (integration).
  The first real publish belongs to the user.
- Never log or print the token.

## Reading

**One entry per post, however many times it's edited.** An edit updates the existing row (the "edited · vN" badge plus a diff since you last read it) and never creates a new unread item. Cross-subscription duplicates (someone's blyg + their RSS feed) collapse to one row, preferring the blyg row. RSS items whose id changes on edit are matched by their resolved page URL. Tombstones are hidden unless signalled or hoppered. The mock is at `docs/prototype/ai-and-reading.html` §5.

**Read state syncs through your blyg when the server supports it.** Read state is a number per reading row: the highest version you've read. Marking a post read is always instant and local, and it marks every duplicate of the same post. When the blyg advertises read-state sync (extension 5, `docs/SERVER.md`), the same mark also queues a `read` op per row in the outbox, which is sent like any other change (offline-safe, retried, coalesced to the highest version per row). A pull merges `max(local, server)`, so read state never goes backwards. The first time a database sees the capability, it uploads everything it has read in batches, once, and records that in `meta`. So a post read on one Mac reads as read on your others, and a fresh install isn't all unread. Without the extension, read state stays on this Mac, and nothing is sent. Nothing new is shown in the UI.

**Search, Notational Velocity style.** A search field sits above the reading list (the Posts omnibar isn't on this screen). ⌘F or `/` (with the list focused) puts the caret in it; typing filters the list live: a case-insensitive substring over the title, the author and blyg (subscription, origin) names, and the post's text (`content_md`, or the published HTML's text when an item has no Markdown), over posts **already held locally**. Nothing is fetched to search. Title matches are highlighted as in the Posts list. ↑/↓ move through the matches from the field, ⏎ goes to the list (opening the first match when nothing is open), and esc clears the search (esc on an empty field returns to the list; esc in the list with a search clears it before it leaves the screen). An empty result shows "No posts match “…”". The open post stays open while it still matches; otherwise the selection and the reader clear, so typing never opens (or marks read) anything.

**The post is shown as its blyg published it.** The body is the published `content_html` (a pin's own `content_html` when the pill is on a pin), with its transclusion snapshots already baked in, followed by the attached images from the item document's `media[]`. It is sanitized (an `ammonia` allowlist: no scripts, handlers, `javascript:` URLs, forms, objects, or iframes other than youtube-nocookie embeds; images get `referrerpolicy="no-referrer"`) and shown in a WKWebView under a strict CSP, with `<base href>` at the author's origin (protocol media paths such as `media/x.png` are origin-relative) and the app's reader theme (Literata, Tufte palette, light/dark). Only an item without HTML falls back to rendering its `content_md` with `blyg-render`, resolving quotes from items held for the same origin. The header, pill, thumbs, notes, diff and actions stay native. The same view shows the current version in ⌘Y. Only one WebView is on screen at a time: the studio preview hides wherever the reader shows.

## AI / TK

The blyg's AI feature is TK: `[TK]instruction[/TK]` → `[TK]instruction[=]output[/TK]`. See `docs/BLYGGER-SPEC-DIGEST.md` §AI and the mock at `docs/prototype/ai-and-reading.html`. Text generated in the app must be recorded as provenance (the provenance extension in `docs/SERVER.md`) so that it's disclosed as `blyg-tk-gen`. The app never offers a direct claude.ai or ChatGPT subscription login (Anthropic prohibits it; OpenAI has no sanctioned route).

## AI providers & sign-in

The app will be used by **other people** too, so every user signs in to their own accounts. Providers:

| Provider | How | Notes |
|---|---|---|
| ChatGPT account | Sign in with ChatGPT (browser PKCE + device-code fallback) via OpenAI's Codex OAuth client → ChatGPT Codex Responses endpoint, the way pi does it | Requested feature. Unofficial: it may break or be blocked by OpenAI, and the UI says so once. |
| OpenAI API key | Paste the key (Keychain) | Official. |
| Anthropic API key | Paste the key (Keychain) | Official. |
| Cloudflare Workers AI | Account ID + API token (Keychain); OpenAI-compatible chat completions | Default model **Gemma 4** `@cf/google/gemma-4-26b-a4b-it`. |
| Local Claude Code / Codex | Spawn the user's installed `claude -p` / `codex exec` | Uses whatever account those tools are signed into. |
| Blyg server | Existing `/api/items/:id/generate` | Off by default today. A Worker can generate with Gemma 4 through its Workers AI binding (see `docs/SERVER.md`). |

**Not built:** a direct claude.ai (Pro/Max) subscription login. Anthropic's terms prohibit third-party apps from offering claude.ai login or using subscription limits. The local Claude Code bridge is the supported way to use a Claude subscription.

Disclosure: **always on**. Edited reading items move **to the top**. Helpers in v1: fill a gap (TK), shorten to fit 1000, continue this thought, outline a thread, proofread (not disclosed), reply to a reading item.

## Onboarding & tutorial

The first launch is an onboarding flow: connect a blyg (URL plus token, or owner password), then optionally connect AI accounts. After that comes an **interactive tutorial** that walks through the features (the ▶ demos in the mocks are the script), with a "Show this tutorial every time I open Blygger" checkbox. It can be re-enabled or replayed from Settings › Help.

## Protocol philosophy → UI rules (from the creator's talk, blygger.org/talks/2026-09-24-blygger/)

Core principle: *"anything the protocol can't verify, it declines to represent."* The app is a **studio**. It follows these rules:

1. **No counts, anywhere social.** Responses and mentions are shown as *a list, never a count*: who, origin, relation, when. Mentions and responses never get numeric badges; use a dot for "something new". There are no follower lists or follower counts: following is client-local and invisible. (A reader-local unread count for *your own* reading list is fine; it's private state, not a social metric.)
2. **Generation happens in the studio, at authoring time, with review.** Publishing never generates. The TK tint shows in the editor only; published bytes are identical, apart from the `generated` metadata and the `blyg-tk-gen` class. Provenance is self-asserted, so the app always records it (the provenance extension).
3. **Transclusion is quoting, and it's snapshotted at publish.** Later edits to the source never rewrite the quote. The picker offers only what's already held (your own posts plus imported items from blyg subscriptions). It never fetches by URL. The picker opens with ⌘K, or by typing `![[` at the start of a line in a thread (after optional indent, outside a fenced code block): the typed `![[` comes out, what you type next filters the picker, ⏎ inserts the whole `![[id]]` line, and esc puts the `![[` back so it can be typed literally. Only typing triggers it, never a paste. In a fragment it shows the "Quotes go in threads" toast and leaves the text alone.
4. **Forking descends from pins only.** Pins are the costly, irrevocable signal: confirm with plain words ("This version will be served forever. You can't undo this.").
5. **No deletes of published work. Withdraw instead**: permanent, visible, and irreversible. Only drafts can be discarded. The UI says "Withdraw", never "Delete", for published items.
6. **No identity layer.** No @handles or accounts. The origin (domain) is the name, and author names are optional decoration.
7. **Stubs are the reply shape.** "Reply to a reading item" creates a stub thread (`stub_of`), usually transcluding the source. The reading actions name the primitive they create: **Reply · new stub** (`stub_of`), **Quote into a thread** (`![[id]]` in a thread of yours) and **Fork** (`forked_from`, from a pin), each with a one-sentence tooltip saying what gets made.
8. **Tolerate the unknown.** Ignore unknown kinds and fields; never reject. The wire is v0.3 and pre-1.0 unstable.

## Client-recorded provenance (owner-API extension, see `docs/SERVER.md`)

- `PUT /api/items/:id/tk-provenance {content_md?, scopes: [ {index, model, sources?:[{id,version}], at?} | null ]}` → `{ok, disclosed}`. Validation runs first and nothing is written on a 400. `scopes.length` must equal the number of TK scopes in the (new) working copy.
- `GET /api/items/:id/tk-provenance` → `{scopes: [...]}`.
- **The server keys provenance by scope POSITION.** A plain `PUT /api/items/:id` that adds, removes or reorders scopes shifts disclosure onto the wrong span. Rule for blyg-core: **whenever the set of TK scopes changes, push the text with the combined call** (`content_md` + the full `scopes` array), never a plain PUT. Track provenance locally per scope (model, sources, at). A scope the user rewrote entirely by hand → `null`.

## Versions & pins (user requirement + spec §5.2/§8.4, mock: `docs/prototype/versions.html`)

- **Reading list: one entry per post** (latest version). A **version browser** (⌘Y) opens from any post.
- **Other people's posts:** the version UI shows **only the current version and pinned versions**. Unpinned versions don't appear at all, in any form (§8.4, and a deliberate product decision). The pinned list comes from the public item document's `changelog` (`pinned: true`) and each pinned body from `{origin}items/{id}/v{n}.json`. **Compact control:** there's no sidebar. The version pill in the post header is a `‹ vN ▾ ›` control: the arrows step through current + pinned versions, and the pill opens a small dropdown list. Actions are shown inline, labelled with what they create: **Quote into a thread** / **Reply · new stub** / **AI reply · new stub** / **Fork** / **Open on web** for the current version; **Quote this version** / **Fork this pin** / **Diff vs now** / **Back to current** for a pinned one. Fork is available only on pinned versions: on the current version it's shown **greyed out**, and its tooltip says why and where to go ("Fork needs a pinned version: pick 📌 vN in ‹ vM ▾ › (or press ←), then Fork this pin", or that the post has no pins). Every action chip has a one-sentence tooltip saying what gets made (a stub thread, `![[id]]` in a thread, a quoted pin with a link, a forked thread draft).
- **"Edited since you read it":** store only `read_version` (a number). Show the author's changelog notes for the versions in between. A **text diff only when the version you read was pinned** (both sides public). Never retain the unpinned text of past versions of other people's posts.
- **Withdrawal of others' posts:** drop the content locally (*"Local hoarding past withdrawal is nonconforming"*), except pinned versions, which may be retained with attribution linking the pin.
- **Own posts:** your history is private to you. Every version can be opened and restored (a restore loads it into the editor; publishing makes vN+1, and versions never go backwards). Pinning uses a type-to-confirm sheet ("You can't undo this").
- Public fetches to other origins are **unauthenticated**: never send the owner token anywhere but the user's own blyg.

## Profiles (mock: `docs/prototype/profiles.html`, agreed as-is)

Who someone is, whom they read, and one click to follow them or anyone they quote, stub or fork.
- **Sources, all public:** a blyg's `blyg.json` (author name, bio, avatar resolved against the origin, links), `blogroll.opml` only when the manifest lists it (OPML outlines: title, xmlUrl, htmlUrl), `items/index.json` (recent items; titles from `feed.xml` or from posts already held), or, for a plain RSS/Atom feed, a simpler card (title, link, recent items). A feed with `<blyg:manifest>` is treated as a blyg.
- **Privacy:** fetched with the token-less `PublicClient` (no token, no cookies) and **only when the user opens a profile**, never in the background or on a poll. Cached in SQLite (`profiles`, with `fetched_at`); opening a profile younger than an hour uses the cache without a request, ↻ refreshes, and a failed fetch falls back to the cached copy (marked stale). The origin sees the user's IP, as a browser visit would.
- **Discovery** reuses the subscribe preview's resolution: your own blyg and your subscriptions first, then `POST /api/subscriptions` (preview). Only when that's unavailable does core probe `{url}blyg.json`, then the URL as a feed. No crawling.
- **Connections** (origins a blyg quotes, stubs and forks) come from that blyg's posts already held locally (`stub_of`, `forked_from`, `transclusions`, `data-blyg-origin` in their HTML) plus quote sources in their feed. Each row says "stubbed 2 posts" / "forked 1 pin" / "quoted 3 times": the author's own data, not a social metric. Your own profile's connections come from your published posts.
- **No counts:** no follower, subscriber or reader counts anywhere. Lists only.
- **Follow** = subscribe preview + subscribe in one step, with a toast; following is private and client-local. Unfollow lives in Subscriptions. **Add to my blogroll** is separate and deliberate (subscribes first if needed, then sets `in_blogroll`), because the blogroll is public.
- **Entry points:** ⌘I (the reading item's origin; your own blyg anywhere else), the author's address in the reading header, the lineage line under it (`↳ stub of …`, `⑂ forked from … vN 📌`), the origin in each mention, blogroll and connection entries inside a profile (profile → profile, with a back arrow), Blyg › My Profile, and ⇧⌘O "Open profile…" (paste any URL).
- **Own profile:** what visitors see (your public manifest), "Edit site settings…", and every subscription with its blogroll toggle.
- **Sheet:** slides in from the right over the reading pane; esc closes, ↑/↓ and ⏎ move and open, F follows the selected entry, ⇥ switches tabs, ← goes back. It suppresses the native web views like other sheets.
- Core: `Backend::profile(url, refresh)` and `Backend::cached_profile(url)` (`blyg_core::profile`); `ReadingItem` carries `stub_of`, `forked_from` and `transclusions` (read leniently from the reading JSON; the owner-API reading extension should pass them through from the item document).

- **Lineage** ("↳ stub of", "⑂ forked from", quote origins) comes from the reading item when the server sends it, and otherwise from the post's public item document (`items/{id}.json`), which the app already fetches, unauthenticated, when a post is opened. The reference Worker's importer doesn't keep `stub_of`/`forked_from`. Connections use only documents already fetched; nothing is fetched just for them.

## Full editor ("studio mode"; mock: `docs/prototype/studio.html`)

A local full editor like the web studio: **source | live preview**, the preview showing the document exactly as published.
- **Modes:** ⌘1 write (list + editor), ⌘2 list + editor + preview, ⌘3 full editor (editor + preview, list hidden). ⌘E toggles the preview in place. There is one renderer for all of them (retire the separate native pulldown→GPUI preview).
- **Renderer:** a Rust port of the reference Worker's studio preview pipeline. Markdown with markdown-it semantics (`html: false`, linkify), plus the common embeds extension (a bare YouTube link on its own line → the click-to-load facade `figure.blyg-yt`; off-origin `<img>` gets `referrerpolicy="no-referrer"`; a failed remote image → a visible link fallback). TK scopes → output wrapped in `blyg-tk-gen` with the studio tint; ungenerated → `⚠ ungenerated — <instruction>`. Transclusions `![[id]]` (threads) resolve from the **local store only** (own published items + imported reading items), rendered as `blockquote.blyg-transclusion` with provenance; unresolved → the `.unresolved` marker. **Parity tests** use fixtures generated by running the Worker's own TS renderer via node.
- **View:** a WKWebView (e.g. `wry`) embedded in the GPUI window. Styled with the blyg's own public `/style.css` (fetched, cached, and used offline), plus the studio additions (tint, unresolved). No JS from content; our own small script handles the YouTube click-to-play (youtube-nocookie) and the image fallback. Links open in the default browser; no in-view navigation.
- **Speed:** re-render ~100 ms after typing pauses, patching the DOM in place (no flicker, scroll kept). Source ↔ preview jump via block line maps (`data-line`).
- **Status bar** counts quotes, AI spans, videos and images, and warns about unresolved quotes before publish.

## Scratch notes (local-only)

Quick capture is for collecting thoughts, not for deciding. So:
- **Quick capture saves a local scratch note by default.** esc, ⌘S or clicking away saves it. ⌘D saves it as a **draft** on the blyg instead (synced, unpublished). ⌘⏎ **publishes** it immediately.
- **Scratch notes are local-only.** They're stored in the local SQLite database, never enqueued to the outbox, never pushed or pulled, and never sent anywhere unless the user invokes AI on them. They show in the main list with a `scratch` pill, are searchable, and are fully editable.
- **Promotion:** ⌘D (make draft) creates the server draft from the scratch note (the same item keeps its local id, so there's no duplicate). ⌘⏎ publishes, creating it first if needed. The kind is chosen at promotion (`blyg_core::promotion_kind`): fragment when ≤ 1000 (server count), otherwise a thread; a note the user already made a thread (⌘T) stays one. `Backend::promote` returns the chosen kind, and the UI says so ("published as a thread"). A scratch note's length never blocks ⌘⏎. Offline, `Promote::Publish` still promotes (the draft's `create` queues in the outbox) and the publish fails with `Offline`. There's no demotion back to scratch once a note is on the server; drafts are discarded or withdrawn instead.
- **Images in scratch notes stay local.** A paste or drop into a scratch note copies the image to
  `<data dir>/scratch-media/<sha256>.<ext>` and inserts `![](blyg-local:<sha256>.<ext>)`; both previews
  show it from disk. Promotion uploads each one (without `item_id`) and rewrites the references to
  `<blyg origin>/media/<id>.<ext>` before the item is created. A failed upload refuses the promotion
  (the note stays scratch). Offline, the uploads go with the queued `create`.
- The main window's omnibar create stays a **draft** (unchanged), unless the config sets `new-note = scratch`.
- Core: `Status::Scratch` (stored as `items.status = 'scratch'`), `Backend::create_scratch(kind, content_md)`, `Backend::promote(id, to: Promote::Draft|Promote::Publish{note}) -> Promoted{kind, published}`. `save`, `set_kind`, `search` and `delete_draft` work on scratch items locally; `publish` on one promotes it first. The sync engine ignores local-only items. Tests: scratch never hits the network (mock server asserts zero requests), promotion keeps the id, and offline promotion queues.
- Config keys: `capture-default = scratch|draft` (default scratch) and `new-note = draft|scratch` (default draft).

## Buttons (optional toolbar)

Keyboard-first, but not keyboard-only. Config `show-buttons = true|false` (**default true** for new installs; the maintainer sets `false`). It's toggled in Settings (⌘,) and offered in the first-run tutorial ("Buttons or keyboard?").
- A quiet toolbar in the title-bar row, with icons plus short labels: **New** (draft/scratch per `new-note`), **Make draft**, **Publish**, **View: Write / Preview / Full editor**, **Versions**, **Generate (AI)**, **Delete** / **Withdraw** (one slot: Delete on a draft or scratch note, Withdraw on a published post; never Delete for published work), **Quick capture**. The quick-capture panel gets the same row: **Scratch · Draft · Publish**.
- **Generated from the single keymap table** (action, key, context, menu label, icon, button label), so buttons, menus and shortcuts can't drift. Every tooltip shows the shortcut, so the buttons teach the keys.
- Buttons are disabled with a reason in the tooltip when unavailable (e.g. Publish on an over-limit fragment: "Too long for a fragment. ⌘T makes it a thread").
- With `show-buttons = false`, the window is exactly the minimalist layout in the mocks.
- As built (`crates/blyg-app/src/toolbar.rs`): the rows live in `keymap::table()` (`icon`, `button`), the order in `keymap::TOOLBAR` and `keymap::CAPTURE_ROW`; a click dispatches the row's action. The row sits between the traffic lights and the view switcher; the centred title shows only when there's room, and a narrow window gets icons only. Buttons other than Quick capture work on the Posts screen and wait for an open sheet. Disabled reasons reuse `vm::publish_decision` / `vm::make_draft_blocked`. In quick capture the row replaces the key hints (each button shows its key); with `capture-default = draft` there's no Scratch button. Reading (⌘R) and Quote (⌘K) have no button: the view switcher already is Reading, and Quote is thread-only. Delete and Withdraw share a slot (`toolbar::visible`, `vm::discard`); Withdraw is the one button without a key, so its tooltip is just "Withdraw…". Icons: twelve Lucide icons (ISC). Tests: `keymap` (every button bound, key in tooltip), `menu_tests` (menus = table), `toolbar_tests`, `discard_tests`, capture tests.

## Updates (in-app, signed)

Config `auto-update = install | notify | off` (**default install**). Blygger › Check for Updates… always checks, whatever the key says, and answers with a toast ("You're up to date (0.3.0)").

- **When:** 15 s after launch, then about every 24 h while running. `state.json` remembers the last check that found nothing newer (`last_update_check`), so relaunches within a day don't call the API again; a check that found an update isn't recorded, so a relaunch looks again. A failed check retries in an hour. Never in `cfg(test)`, with `BLYGGER_FAKE=1`, or with `BLYGGER_NO_UPDATE=1`.
- **What:** `GET https://api.github.com/repos/<repo>/releases/latest` (unauthenticated, with a User-Agent). Drafts, prereleases and pre-release tags are skipped; the tag is compared as semver against `CARGO_PKG_VERSION`.
- **install:** download and verify in the background, then a quiet status-bar notice: "Blygger X is ready · Restart to update · What's new". Restart installs and relaunches; quitting with an update ready installs it too (no relaunch). **notify:** "Blygger X is available · Download · What's new"; Download proceeds as install. **off:** no automatic checks.
- **Security model** (all must pass, or nothing is installed):
  1. HTTPS only, from `api.github.com`, `github.com`, and GitHub's release-asset storage hosts (`objects.githubusercontent.com`, `release-assets.githubusercontent.com`). Redirects are followed by hand and every hop is checked.
  2. The release workflow signs `SHA256SUMS` with the project's Ed25519 key (secret `UPDATE_SIGNING_KEY`, `scripts/sign-sums.sh`, OpenSSL 3 `pkeyutl -sign -rawin`) and publishes `SHA256SUMS.sig`: the **raw 64-byte signature** over the exact bytes of `SHA256SUMS` (not base64). CI verifies it with the key embedded in the app before uploading, so a wrong key fails the release.
  3. The app embeds the public key (`update/verify.rs`, `RELEASE_PUBLIC_KEY_B64`, raw 32 bytes base64) and checks the signature with `ed25519-dalek`'s `verify_strict` before downloading the zip.
  4. `Blygger-<ver>-macos-universal.zip` must match its SHA-256 line in the signed `SHA256SUMS`.
  5. The zip is extracted with `ditto -x -k` into a staging folder on the same volume as the running bundle; the extracted `Blygger.app` must have `CFBundleIdentifier = org.blygger.desktop`, a `CFBundleShortVersionString` equal to the release's version and strictly newer than the running one, its executable, and pass `codesign --verify --strict` (the ad-hoc signature). These checks run again right before installing.
  6. Only the bundle the app runs from is replaced (from `current_exe()` → `…/X.app/Contents/MacOS/blygger`, and only if that bundle is `org.blygger.desktop`). The swap: old bundle → `.X.app.old`, new bundle in, old removed; if the new one can't be moved in, the old one is put back. A detached `/bin/sh` waits for the process to exit and runs `open -n <bundle>`.
  7. Not running from a bundle (`cargo run`), a translocated copy, or an unwritable folder → notify-only: "Can't update in place: …; download from the release page". A release without `SHA256SUMS.sig` (0.2.0 and earlier) is notify-only too. Quarantine is neither added nor stripped (the app's own downloads aren't quarantined).
- `scripts/install.sh` also checks `SHA256SUMS.sig` when OpenSSL 3 is installed (and the release has one).
- As built: `crates/blyg-app/src/update/` (`check.rs`, `verify.rs`, `net.rs` with the `Http` trait, `install.rs` with the `Tools` trait, `mod.rs` GPUI glue, `view.rs` the status-bar notice). Tests are offline: a fake HTTP client serving a release signed with a throwaway key, fake bundles in temp dirs, swap and rollback. Debug builds only: `BLYGGER_UPDATE_URL` (a local test server; plain HTTP to localhost allowed), `BLYGGER_UPDATE_PUBKEY` (a test key) and `BLYGGER_UPDATE_SMOKE=restart|quit` (act on a ready update without input) for manual smoke tests.
