# Blygger protocol digest (for a native desktop client)

Written 2026-09-24. Sources: the normative spec at `https://blygger.org/spec/0.2/`
(built from `blygger/blygger-spec@75f74d9`), `/start/`, TN-1, and upstream docs at
the same commit (`docs/tk-core-plan.md`, `docs/v0.3-plan.md`, `docs/v0.2-plan.md`,
`docs/css-contract.md`, `docs/roadmap.md`, `DEVLOG.md`, `CLAUDE.md`). Behaviour
was checked against the reference Worker source at upstream `75f74d9`.

Conventions: **§n** means spec 0.2 section n. "Ref" means the reference worker.
"(inferred)" marks my own reading, not something a source states.

> **Version skew, read this first.** The published normative spec is **0.2**. The
> reference worker already emits `"blyg": "0.3"` (`PROTOCOL_VERSION = "0.3"`,
> generator `blyg-ref/0.3.0`, "Trunk"). It implements v0.3-plan constructs:
> cross-blyg and thread transclusion, `stub_of`, `forked_from`, `page`,
> `transclusions[].origin`, `data-blyg-origin`, and webmention. `protocol-v0.3.md`
> has **not been drafted yet**; per decision #21 it comes after live testing.
> Where 0.2 and the ref differ, both are given below.

---

## 1. Object model

### 1.1 Blyg / origin
- A **blyg** is a static file tree under an **origin** (its base URL, ending in `/`).
  The origin can be a domain root, any path, or a subdomain. `/blyg/` is only a
  convention and readers "MUST NOT infer anything from the mount path" (§4).
- Invariants (§1): the protocol is a static-file contract; every feed is valid
  RSS 2.0; "**AI is never in the protocol**"; "**Identity is never in the
  protocol**". The only authenticated entity is the origin, and DNS is the namespace.
- There are two planes. The **state plane** (item files plus archive index) is ground
  truth. The **notification plane** (`feed.xml`) is "a lossy, windowed signal".

### 1.2 Surface (§4; the file names are fixed by the protocol)
| Path | What | Required |
|---|---|---|
| `blyg.json` | manifest | MUST |
| `feed.xml` | RSS 2.0 + `blyg:` ns | MUST |
| `items/index.json` | archive index | MUST |
| `items/{id}.json` | canonical item doc | MUST (404 for unknown or draft; 200 **forever** once published, withdrawal included) |
| `items/{id}/v{n}.json` | pinned version | MUST (404 unless pinned; 200 forever once pinned) |
| `media/…` | immutable media | MUST |
| `blogroll.opml` | curated blogroll | MAY |
| HTML (`f/{id}/`, `t/{id}/`, `f/{id}/v{n}/`, `t/{id}/v{n}/`) | presentation | SHOULD / MAY |

- The whole surface MUST be servable as plain static files.
- CORS `Access-Control-Allow-Origin: *` is SHOULD.
- Timestamps are ISO 8601 UTC `…Z`, except the RFC 822 dates RSS requires.

### 1.3 Ids (§5.1)
- An id is 128 random bits written as **26 chars of lowercase Crockford base32**
  (`0123456789abcdefghjkmnpqrstvwxyz`, no padding), from a CSPRNG.
- Ids are permanent. They never change across versions, withdrawal or return, and
  they are never content-addressed.
- `content_hash` = `"sha256:" + hex(SHA-256(content_md as UTF-8))`. It covers
  `content_md` **only**: not author, not media, not HTML.
- Fragments and threads share one id space. In the ref, a draft already has an id
  but returns 404 on the public surface until it is published.

### 1.4 Item kinds (§5.3)
- `"fragment"` is short-form. Publishers SHOULD cap `content_md` at 2,000 chars
  (RECOMMENDED studio cap 1,000); readers MUST NOT reject long fragments. **The ref
  enforces 1,000 chars on the TK-stripped published text** (`FRAGMENT_MAX_CHARS`).
- `"thread"` is long-form, has no length cap, and carries `transclusions`.
- `"withdrawn"` is the endcap.
- Unknown kinds: ignore the item. Do not reject the containing doc.

### 1.5 Versions (§5.2, TN-1)
- `version` is a positive integer, **+1 exactly per publish event**. Draft saves are
  invisible. There is no semver, no edition field and no significance markup, "at
  any level, ever" (locked decision #19). The pin is the significance signal, and
  the changelog `note` is the human-readable one.
- Only the **latest** version's content is served. History stays private unless
  pinned. `changelog` is metadata only:
  `{version, at, note, pinned?: true}`. `updated` MUST equal the last changelog `at`.
- Ref "restore" (`POST /api/items/:id/restore {version}`) only loads an old version
  into the working copy. Publishing it makes vN+1. Versions never go backwards.

### 1.6 Item document `items/{id}.json` (§5; ref additions marked 0.3)
```json
{ "blyg": "0.2", "id": "…26…", "kind": "fragment|thread|withdrawn",
  "origin": "https://example.com/blyg/",
  "page": "t/{id}/",                        // 0.3 (ref always emits; readers SHOULD use, MAY fall back to f/·t/)
  "author": { "name": "…", "url": "…" },    // OPTIONAL, opaque, carry verbatim
  "created": "…Z", "updated": "…Z", "version": 3,
  "content_md": "…", "content_html": "…", "content_hash": "sha256:…",
  "media": [ { "url": "media/x.png", "mime": "image/png", "alt": "…" } ],
  "transclusions": [ { "id": "…", "version": 3, "origin": "https://other/" } ], // threads only; origin is 0.3, remote only
  "stub_of": { "origin": "…", "id": "…", "version": 1 } | { "url": "…" },      // 0.3, threads only
  "forked_from": { "origin": "…", "id": "…", "version": 2 },                   // 0.3 (0.2 reserves it, 2-field shape, "do not emit")
  "generated": [ { "sources": [{ "id": "…", "version": 3 }], "model": "…", "at": "…Z" } ], // §5.7, OPTIONAL
  "changelog": [ { "version": 1, "at": "…", "note": null }, { "version": 2, "at": "…", "note": "typo", "pinned": true } ] }
```
- `author` (§5.5) is client-asserted. It is scoped to `(origin, value)`, "never
  addressable", and "Clients that store or re-emit item JSON MUST carry `author`
  verbatim". Equal authors on different origins are NOT the same entity. In the
  ref, `author` is always `{name: settings.author_name, url: origin}`.
- Media (§5.4) is immutable: a media URL always serves the same bytes, so a new
  image means a new object. Ref: `POST /api/media` (png/jpg/gif/webp/svg, ≤5 MB)
  returns `media/<id>.<ext>`.

### 1.7 Manifest `blyg.json` (§6.1)
```json
{ "blyg": "0.2", "level": 1, "generator": "blyg-ref/0.2.0",
  "site": "https://example.com/blyg/", "title": "…",
  "author": { "name": "…", "bio": "…", "avatar": "media/avatar.png", "links": [{ "label": "…", "url": "…" }] },
  "feed": "feed.xml", "items": "items/index.json",
  "blogroll": "blogroll.opml",   // only when a non-empty blogroll is served
  "webmention": "webmention",    // 0.3, only when the deployment receives
  "updated": "…Z" }
```
- `site` is display-advisory and never identity (§12.2).
- The `"blyg"` key is informative: readers "MUST accept any `0.x` value" (§3.1).

### 1.8 Archive index `items/index.json` (§6.2)
- `{updated, items:[{id, kind, created, updated, version}]}` lists every item ever
  published, withdrawn ones included, with no window, sorted `updated` desc.
- It is the lossless reconciliation surface.

### 1.9 Feed `feed.xml` (§7)
- RSS 2.0 with `xmlns:blyg="https://blygger.org/ns/0.1"`. The namespace URI is a
  **permanent token** and does not track the version.
- Channel elements: `<blyg:level>`, and `<blyg:manifest>` (MUST; this is the upgrade
  hook for resolution).
- One `<item>` per **publish event**, newest first, window RECOMMENDED 50.
  - `guid` = `blyg:{id}:v{n}` with `isPermaLink="false"`.
  - Per-item elements: `<blyg:id>`, `<blyg:kind>`, `<blyg:version>`,
    `<blyg:created>`, `<blyg:item>` (absolute item-doc URL).
- An older event's `<description>` MUST carry the **latest** HTML, so history does
  not leak.
- A withdrawn item contributes exactly one entry: title `withdrawn`, empty
  description. Its earlier events are dropped.
- Pinning emits **no** feed event.
- `<description>` must be self-contained: absolute media URLs, no stylesheet or
  script dependence.
- `<dc:creator>` is SHOULD when `author.name` is present.
- Ref extras, which are presentation: a provenance line under each baked quote, a
  stub citation line, a fork lineage line, and `<img>` for attached media, all
  appended into the description.

### 1.10 Pins `items/{id}/v{n}.json` (§8), irrevocable
- A pin is a "publisher's irrevocable hosting promise". The file is 200 forever and
  survives later edits and withdrawal. Any version *with content* can be pinned,
  retroactively too. Endcaps MUST NOT be pinned (ref: `409 cannot pin an endcap version`).
- Media referenced by a pinned version is retained forever.
- Doc shape:
  `{blyg, id, kind, version, at, note, pinned:true, origin, author, content_md, content_html, content_hash}`,
  plus `transclusions`, `generated`, `stub_of` and `forked_from` when applicable. It
  has **no `media` array**.
- §8.4: no route ever serves an *unpinned* older version, in any representation. A
  version UI "MUST NOT offer, imply, or hint at access to unpinned history". Pinned
  HTML pages at `f|t/{id}/v{n}/` are optional and MUST NOT appear in the feed or
  index.
- Ref: `POST /api/items/:id/pin {version}` is idempotent (`already: bool`) and has
  no undo.

### 1.11 Withdrawal, endcaps, tombstones (§9)
- "Withdrawing is the only exit". **No delete exists** for published items. Hard
  delete applies only to never-published drafts (ref: `DELETE /api/items/:id`,
  which returns `409 published items are withdrawn, not deleted` otherwise).
- The endcap is a version bump with `content_md:""`, `content_html:""`, `media:[]`,
  `kind:"withdrawn"`, an optional note, and the changelog kept.
  - Threads' `transclusions` becomes `[]`.
  - It never carries `generated`.
  - Ref: `stub_of` is dropped, but **`forked_from` and `page` survive**.
- It is reversible: publishing again gives vN+1 with the authored kind, same id.
- In the ref, the working copy survives withdrawal.
- Readers: "roll up to null". Reader-side this state is a *tombstone*.

### 1.12 Conformance levels (§3), strict supersets
| Level | Meaning |
|---|---|
| L0 | Any plain RSS/Atom feed, wrapped reader-side (summary fragment + link, §13.6). |
| L1 | Spec 0.2 publish side (ids, versions, item files, manifest, index, withdrawal, pins, threads + local transclusion, `generated` provenance) and subscribe side (resolution, importer rules, lossless recovery). |
| L2 | Cross-client, arriving incrementally. Blogroll ships at 0.2. Stubs, nesting, `forked_from` and webmention arrive at 0.3 (already live in the ref). |
| L3 | (future) encrypted / permissioned content. |

"A conforming reader at any level MUST ignore constructs it does not understand
rather than reject the document containing them."

---

## 2. Authoring syntax (studio-side)

Published `content_md` is ordinary markdown. Ref renderer: markdown-it with
`html:false`, `linkify:true`, so **raw HTML is escaped**. Only two grammars exist on
top of markdown: transclusion directives (which stay in the published `content_md`)
and TK scopes (which are stripped before publish). Stub and fork are **actions**,
not syntax.

### 2.1 Transclusion `![[id]]`
- **Grammar (§10.1, "permanent protocol surface")**: "A line consisting solely of
  `![[` + a 26-character item id + `]]` (surrounding whitespace allowed) is a
  transclusion directive."
  - "Anywhere else — inline, inside code blocks — the same character sequence is
    inert text."
  - Ref regex: `^\s*!\[\[([0-9a-hjkmnp-tv-z]{26})\]\]\s*$`. It is lowercase-only,
    applied line by line.
- **`![[id@vN]]` is reserved**: "0.2 publishers MUST reject it at publish time;
  readers MUST tolerate it". Ref error: `explicit-version references (@vN) are
  reserved, not supported`.
- **Threads only.** A fragment's `![[id]]` is not resolved. In the ref it publishes
  as literal text: fragments go straight to `renderMarkdown`, so nothing rejects it
  (inferred from `model.publish`).
- **What resolves:**
  - *0.2 spec (§10.2)*: only a "local, currently-published `kind: "fragment"`
    item". Drafts, withdrawn items, unknown ids and threads are publish errors.
  - *Ref / v0.3 (decision #26)*, `resolveTarget`, in this order:
    1. A local published item of **either kind**. Thread targets nest; their baked
       HTML is nested blockquotes.
    2. An **imported** item from a *blyg* subscription with `remote_id = id`, in
       state `current`, or a tombstone with a pinned-retained snapshot.
    3. Otherwise an error. The reasons are: `unknown item`,
       `item is a draft, not published`, `item is withdrawn`,
       `ambiguous id: imported from more than one origin`,
       `source withdrawn by origin`,
       `source is a plain RSS (L0) item, not a blyg item`,
       `a thread cannot transclude itself`,
       `circular transclusion: that thread already quotes this one` (local DAG
       check; remote chains are not walked).
  - "A directive names an *identity*, not an origin." **Cross-blyg quoting only works
    for items the *server* already imported** ("quoting follows reading"). Publish
    never fetches the network.
- **Baking (§10.2)**: the target's latest HTML is snapshotted into `content_html` as
  ```html
  <blockquote class="blyg-transclusion" data-blyg-id="{id}" data-blyg-version="{n}"
              data-blyg-origin="{origin}">…</blockquote>   <!-- data-blyg-origin: 0.3, remote only -->
  ```
  - There is no link inside. `content_md` keeps the directive.
  - Republishing re-resolves to the then-latest version.
  - `transclusions[]` = direct quotes in directive order, `{id, version, origin?}`.
    `origin` is omitted for own-origin sources.
  - Snapshot independence (§10.4): source edits, withdrawal or pins never alter a
    baked thread. There is no auto-pin.
- Any bad directive fails the **whole** publish:
  `400 {"error":"one or more transclusions do not resolve","errors":[{directive, reason}]}`.
- Code fences: the spec says a directive inside a code block is inert, but the ref
  walker is line-based and has no fence awareness. An own-line `![[id]]` inside a
  fenced block would probably be resolved (inferred from `transclusion.ts`; not
  tested). A client should not depend on either behaviour.

### 2.2 TK instructed generation `[TK]…[=]…[/TK]` (grammar is studio-private)
- Forms (tk-core-plan §2.1, respelled in session 16):
  - Ungenerated: `[TK]instruction[/TK]`.
  - Generated: `[TK]instruction[=]output[/TK]`.
- Tokens are case-sensitive uppercase. The instruction is trimmed.
  `[TK][/TK]` (empty instruction) is valid.
- Scopes may be **inline or span lines**. A scope is "block" if it sits alone in its
  own paragraph (blank line or document edge on both sides); otherwise it is inline.
- Parsing is a linear scan for the three tokens with **no bracket balancing**. `![[id]]`
  inside a scope is safe because no token starts with `]`.
- **No nesting.** Errors are `nested TK scopes are not supported` and
  `unterminated scope (missing [/TK])`.
- Valid in both fragments and threads.
- **Quote vs source**: inside a scope, *every* `![[id]]` (inline or own-line, in the
  instruction or the output) is a **source reference**, never a transclusion.
  - Sources must be local, published **fragments**. This is still the 0.2 rule in
    the ref (`resolveFragment`); v0.3 left TK untouched.
  - Scopes cannot contain transclusions. To quote verbatim, close the scope, quote,
    and reopen.
- **Publish** (§2.4):
  - Any scope without `[=]` is an error, `scope has no output (never generated)`,
    returned as `400 {"error":"one or more TK scopes are not publish-ready","errors":[{at, reason}]}`.
    "generation is always author-reviewed, never publish-triggered."
  - Each scope is stripped to its bare output, and the wire `content_md` has **no
    markers**. The hash covers the stripped text.
  - The working copy keeps the full grammar.
  - The fragment cap is checked on the stripped length.

### 2.3 Stubs (v0.3, decision #27; action, not syntax)
- A stub is a **thread** that declares itself a response to exactly one target:
  - `stub_of: {origin, id, version}` for a blyg target. `origin` is REQUIRED even
    when it is your own: "citations are absolute".
  - `stub_of: {url}` for any plain-web URL.
- The body is the author's and is never inspected. The ref prefills `![[remote_id]]\n\n`
  for blyg targets and `[title](url)\n\n` for L0 targets.
- **Version agreement**: if the body transcludes the target, `stub_of.version` =
  the version actually baked.
- `stub_of` lives on versions, so it travels with pins. The endcap drops it.
- API:
  - `POST /api/items {kind:"thread", stub_of}` (fragments: `400 only threads can be stubs`).
  - `PUT /api/items/:id {stub_of | null}`.
  - `POST /api/stubs {subscription_id, remote_id}`.
- "Citation" human text (source title, author, excerpt, url, retrieved) is frozen
  client-side in `versions.stub_cite`. It is **not on the wire**. Whether it should
  be is an open ⚠️ FABLE question (v0.3-plan §8).

### 2.4 Forks (v0.3 Phase B, built 2026-09-22)
- `forked_from: {origin, id, version}` must name a **pinned** version, own or remote.
- `POST /api/fork {origin,id,version}` creates a draft of the same kind whose working
  copy is the pinned file's `content_md`.
  - Remote: fetches `{origin}items/{id}/v{n}.json` and checks `id`, `version` and
    `pinned:true`.
  - Local: checks `versions.pinned`.
- Lineage is written once and **has no setter**. It survives withdrawal and appears
  on pins, pages and RSS.
- Publish re-checks the pin:
  - A definite 4xx, or a mismatching doc, fails with 400.
  - A network error or 5xx lets publish proceed with `warning`.

### 2.5 Everything publish can reject (ref `POST /api/items/:id/publish`)
- Malformed or ungenerated TK scopes.
- A fragment over 1000 chars after stripping (`fragment exceeds 1000 characters`).
- Any unresolvable or reserved (`@vN`) transclusion.
- A fork lineage failure.

Everything is atomic: nothing is written on error. Publish then enqueues webmentions,
fire-and-forget.

---

## 3. AI / TK in depth

### 3.1 Purpose and stance
- TK (journalism's "to come") is **instructed, in-prose generation at authoring
  time**. Locked decision #5 / §1: "AI is never in the protocol. Generation, if any,
  happens in the studio at authoring time; the page publishes output plus provenance
  (§5.7). Readers need no models or keys."
- Instructions are "studio-private, permanently".

### 3.2 Server-side generation (ref)
- `POST /api/items/:id/generate` with body `{"scope": n}` (0-based index in the
  working copy).
  - Needs `AI_PROVIDER_KEY` (wrangler secret). Otherwise it fails with 502
    `AI_PROVIDER_KEY is not configured`.
  - Model = settings `ai_model` || `"claude-opus-5"`.
  - `ai_style_prompt` is appended to a fixed system prompt. Both settings are set
    via `PUT /api/settings`.
- What it sends to the Anthropic Messages API (`api.anthropic.com/v1/messages`, raw
  fetch, `max_tokens` 4096):
  - the instruction;
  - each source's `content_md`, labelled `--- source {id} ---`;
  - the current output, if regenerating;
  - the full working copy with the active scope wrapped in
    `<<<TK-SCOPE>>>…<<<END-TK-SCOPE>>>`.
- It splices the output after `[=]` (inserting `[=]` if missing), saves the working
  copy, and records provenance. It returns `{text, model}`.
- Errors:
  - `400` for malformed scopes, `unknown scope index`, or
    `unresolvable source {id, reason}`;
  - `502` for provider failure or refusal, surfaced verbatim with no retry.

### 3.3 Provenance path: `tk_provenance_json` to `generated_json` to wire
1. `/generate` writes `items.tk_provenance_json`. This is a **working-copy cache**:
   an array aligned by position to the current scope order, holding
   `{sources:[{id,version}], model, at}` or `null` per scope. It is not a wire artifact.
2. `publish()` builds `hasProvenance[i] = cache[i] != null`.
   - Provenanced spans are wrapped in `content_html`: `<span class="blyg-tk-gen">`
     inline, or `<div class="blyg-tk-gen">` for block (independently rendered).
   - `versions.generated_json` = the non-null cache entries in scope order.
3. The item doc and pinned files emit `"generated"` when non-null. Endcaps never do.

Wire rules (§5.7, quoted):
- "a publisher whose version contains machine-generated prose **SHOULD** disclose it
  via an OPTIONAL top-level `"generated"` array".
- "`sources` … MAY be empty (pure instructed generation). `model` and `at` are
  RECOMMENDED."
- "The array MUST NOT carry instruction text or any other pre-generation authoring
  state".
- "A source is not a transclusion": sources never appear in `transclusions`.
- "the renderer MUST wrap each generated span in `content_html` as `<span
  class="blyg-tk-gen">…</span>` (inline output) or `<div class="blyg-tk-gen">…</div>`
  (block output)". `blyg-tk-gen` is "a permanent wire token". No data attributes.
  Span-level source mapping is deliberately not promised.
- "Like all provenance in this protocol, `generated` is self-asserted and
  unverifiable".
- §14: "nothing verifies it, and its absence proves nothing. The protocol's stance
  is symmetric honesty: it neither verifies the claim nor pretends undisclosed
  generation is detectable."
- The CSS contract says "Styling is free; suppression is not": never strip or
  rename the classes, and keep them through sanitizers. The ref leaves
  `.blyg-tk-gen` **deliberately unstyled** (decision #25). Tinting it "would
  present self-asserted provenance as a verified authorship badge".

### 3.4 Is disclosure required? Only as SHOULD, and only in the protocol's sense
- The only normative disclosure rule is §5.7's **SHOULD** for the `generated` array.
  The HTML wrapper **MUST** applies to "each generated span", meaning spans the
  publisher treats as generated.
- The tk-core-plan open decision #1 kept "SHOULD-record in spec language,
  always-recorded in the reference client". MUST-record was rejected as
  "unverifiable but normatively louder".
- There is **no MUST anywhere** that forces disclosure of AI-written text.

### 3.5 Text that the server did not generate
- **Hand-typed output inside a scope** (`[TK]x[=]I typed this[/TK]`) with no cache
  entry is published as **plain text**: no wrapper, no `generated[]`.
  - Test: "hand-authored output (no /generate call) publishes as plain text, no
    wrapper, no generated[] entry".
  - DEVLOG session 14: "nothing was actually generated, so there's nothing to disclose."
- **Text generated by a desktop client's own LLM call and uploaded via
  `PUT /api/items/:id {content_md}`** looks the same to the server as hand-typed
  text. It is **published undisclosed**:
  - whether it is inside a `[TK]…[=]…[/TK]` scope with no cache entry,
  - or plain prose with no scope at all.
  - Nothing detects it or rejects it. The protocol has no way to object; §14 admits
    "undisclosed generation" is undetectable.
- **Hand-edited server output stays disclosed.** `saveWorkingCopy` does not touch the
  cache, and the test "generate, hand-edit, publish" expects both `generated`
  entries and `blyg-tk-gen` after an edit. The studio syntax page says "hand-written
  or hand-edited output carries no disclosure". **That is wrong for hand-edited
  output**, per code and tests.
  - Corollary: if a client overwrites a server-generated span through PUT, the old
    `model`/`sources` provenance still gets published.
- **Positional fragility** (documented limitation): adding, removing or reordering
  scopes via PUT between `/generate` calls can attach provenance to the wrong scope.
  The cache is only resized on the next `/generate`.
  - `restore` clears the cache.
  - Publish does **not** clear it, so republishing unchanged scopes re-discloses.

### 3.6 Recording provenance for locally generated text
- **No endpoint exists.** `PUT /api/items/:id` accepts only `content_md` and
  `stub_of`. `PUT /api/settings` has no provenance key. `setTkProvenance` is only
  called from `runGenerateScope`. The local patches (`owner-token`,
  `owner-read-api`) add none either.
- **Options:**
  1. Proxy all generation through the server's `/generate`. This works today, but
     only with the server's key, model and prompt. The client then gets
     `{text, model}` back and must re-GET the working copy.
  2. Add a new owner endpoint (inferred design). For example
     `PUT /api/items/:id/tk-provenance {scope, sources:[{id,version}], model, at}`
     or `{provenance: (ScopeProvenance|null)[]}`, which calls `setTkProvenance`.
     - It must validate each source like `resolveFragment`: local, published,
       fragment. It must record the source's version at generation time.
     - It could be combined with the content PUT atomically to avoid the positional race.
     - The model string is self-asserted either way, which is consistent with §5.7.
  3. Extend `POST /generate` with a client-supplied `{scope, text, model, sources}`
     mode that skips the provider (inferred).
- **What a client needs to record** (from the wire shape):
  - per span, the **exact published versions** of the local fragments it drew on
    (`{id, version}` at generation time, not at publish time);
  - the provider model id (RECOMMENDED);
  - the generation timestamp (RECOMMENDED);
  - the span's position (scope index) so publish can wrap it.
  - Never the instruction.

---

## 4. Subscriptions and reading (§§11–13, ref importer)

### 4.1 Resolution (§12.1, ≤6 fetches, deterministic)
1. **Normalize**: strip query and fragment; append `/`.
2. **Probe** `{candidate}blyg.json`. "parse-success is the test" (a JSON object with
   a `"blyg"` key). Ignore Content-Type.
3. **Feed upgrade**: if the input is RSS/Atom with `<blyg:manifest>`, it is a blyg.
   Otherwise hold it as the L0 candidate.
4. **rel probe**: find `<link rel="blyg" href>` in HTML. `href` is the **origin**
   (append `blyg.json`). One hop only.
5. **Conventional mounts**: `/blyg/blyg.json`, then `/blyg.json` at the host root.
6. **RSS fallback**: the L0 candidate or `rel="alternate"` autodiscovery gives
   `{feed_url}`. Otherwise fail with the list of URLs tried.

- **Identity = final fetch origin after redirects** (≤5). "MUST NOT adopt the
  asserted [`site`] value as identity"; surface any mismatch.
- Re-resolving to a new origin MUST be user-confirmed.
- Ref: `POST /api/subscriptions {url}` returns
  `{needsConfirm, kind:"blyg"|"rss", origin|feedUrl, title, siteMismatch?}`.
  Then `{url, confirm:true, title?}` gives `201` and a full backfill.
  Unresolvable input gives `422 {error, tried}`. There is **no dedupe**.

### 4.2 Polling (§12.3, §13.2; v0.2-plan §3.2)
- Conditional GET of `feed.xml` (ETag / If-Modified-Since, honour 304). Parse leniently.
- **Trigger set** = entries whose `(blyg:id, blyg:version)` is new or ahead of the
  watermark. For each, fetch the item doc (`blyg:item`, else `{origin}items/{id}.json`).
  "The feed never mutates state directly."
- **Index reconciliation** (diff `items/index.json`) runs:
  - on first subscribe;
  - on a suspected gap (the previous newest GUID left the window);
  - after failure recovery;
  - periodically (ref: every 24h).
- A 404 on the index means a nonconforming publisher: switch to lossy feed-only mode.
- Failure handling: exponential backoff, honour `Retry-After`, never tighten the
  interval, never alter stored state on failure. Send an identifying `User-Agent`.
- Ref timing: cron every 15 min; per-sub interval 30 min ±5 min deterministic
  jitter; `degraded` after repeated failures; concurrency 4.
- Ref subscription controls: `/api/subscriptions/:id/pause`, `/resume`, `/resync`,
  `PUT {in_blogroll,title}`, and `DELETE` (cascades imports, hopper items, signals).

### 4.3 Import rules (§13.1, §13.3–13.4)
- Roll up by `blyg:id`: highest `version` wins, ties broken by `updated`. Scope
  everything to the origin.
- **Watermark** (highest version seen) is monotonic.
  - A lower version is a *regression*: do not adopt it, surface it, offer a
    user-confirmed reset.
  - Same version with a different `content_hash` is a *stealth edit*: adopt it and
    flag it.
  - Verify `content_hash` on import. A mismatch is a flag; adopt the content anyway.
- **Withdrawal** rolls to null: stop presenting it and SHOULD delete it.
  - Exception: content for a version the origin has **pinned** MAY be retained,
    with attribution linking the pin.
  - "Local hoarding past withdrawal is nonconforming."
  - A later version means the item has returned.
- **Never re-emit** imported items in your own feed, index or item docs (§13.5).
  Public display is curation only.
- **L0 wrapper** (§13.6):
  - Synthetic id from the GUID (or link). It is never exported.
  - Content is the title link plus the sanitized summary.
  - The same GUID with changed content is a local version bump.
  - No withdrawal. Entries that leave the window are kept.
- **Ordering** is reader policy (§13.7). Ref: `min(claimed updated, first observed)`.
- **HTML safety** (§14): "Store verbatim, sanitize at render". Keep
  `blyg-transclusion`, `blyg-tk-gen` and the `data-blyg-*` attributes in the
  allowlist.

### 4.4 Signals, hoppers, blogroll
- **Signals (thumbs)**: `PUT /api/signals/:sub/:remoteId {thumb: 1|-1}`, `DELETE`.
  They are private studio state with "no protocol surface, no transmission anywhere".
  v0.4 plans to use them for AI auto-hoppers.
- **Hoppers**: named curation lists of imported items.
  - API: `POST /api/hoppers {name}`, `PUT /api/hoppers/:id {name?, public?}`,
    `PUT|DELETE /api/hoppers/:id/items/:sub/:remoteId`.
  - A hopper snapshot tracks the living item. On withdrawal it becomes a placeholder
    unless pin-backed.
  - A public hopper is served at `{origin}h/{slug}/`. The slug freezes once public.
    This is curation display, never feed re-emission.
  - Own items as members are planned (v0.3 Phase B, not built).
- **Blogroll** (§11): OPTIONAL plain OPML 2.0 at `blogroll.opml`.
  - Entries are `<outline type="rss" text xmlUrl htmlUrl>`, with no blyg-specific
    attributes.
  - It is curated and "claims no completeness — ever". Inclusion is opt-in per
    subscription, default excluded.
  - Absent means 404 and no manifest key.
  - Ref: derived from subs with `in_blogroll=1` and not paused.
  - "There is no protocol 'follow.'"

### 4.5 Webmention and responses (v0.3, decision #28; the ref implements it)
- **Send**: on publish, for each *remote* reference (`stub_of`, `transclusions[]`
  with `origin`, `forked_from` to another origin):
  - `source` = own permalink (`origin` + `page`); `target` = the target's permalink or URL.
  - Endpoint = the target manifest's `webmention` key, else W3C discovery.
  - Retries 15m, 1h, 4h, 12h, 24h. 4xx is terminal (except 429).
  - Republish re-sends only new or changed references. Withdrawing a stub re-sends
    once (the receiver marks it `gone`).
- **Receive** at `POST {origin}webmention` (form `source&target`):
  - 400 for syntactic problems; 429 over 60/hour per source host; otherwise 202 and
    async **structural verification**.
  - Verification fetches the source. If it is HTML, follow
    `<link rel="alternate" type="application/json">` to the item doc.
  - The doc's `origin` must equal the source URL's origin, and it must not be withdrawn.
  - The relation is `stub`, `transclusion` or `fork` (a fork must name a version the
    receiver has pinned). Otherwise the mention fails.
  - No content is stored: a mention is a pointer.
  - Plain (non-blyg) webmentions are accepted with 202 but end `failed`.
- **Responses list**: `PUT /api/items/:id/responses {show: bool}` sets
  `items.show_responses` (default off). `PUT /api/mentions/:id/hidden {hidden}`.
  - It renders a citation trail on the live permalink page only. It is "never a
    count", and never in item docs, the feed, pins or hashes. A stranger's response
    must never change the author's bytes.
- Pages SHOULD carry `<link rel="alternate" type="application/json" href="…items/{id}.json">`
  and `rel="webmention"`.
- "What will never appear: reply primitives …, follower graphs …, addressable
  authors, AI constructs on the wire beyond the passive provenance of §5.7,
  version-significance markup … and content-addressed identity." (§15)

---

## 5. Stability caveats and tolerance rules
- Status: "Every spec version numbered below 1.0 is a working draft … assume no
  promises — including of the wire surface." Breaking changes pre-1.0 bump the
  manifest version and go in the devlog.
- `/start/`: "don't build something load-bearing on it yet". The wire changed
  "twice in the last month".
- What is stable: RSS validity, static files, "nothing … requires a company in the middle".
- Grammar churn has already happened:
  - the TK opener was respelled `[TK` → `[TK]` (old working copies were migrated,
    not dual-supported);
  - the wire key moved 0.2 → 0.3 before any 0.3 spec existed.
- Open design items that may change the wire:
  - `stub_of` / `forked_from` / remote-byline "cited" labels (⚠️ FABLE §8);
  - `@vN`;
  - v0.4 staleness and the filter-plugin API;
  - L3.
- **Tolerate**:
  - any `0.x` `blyg` key;
  - unknown kinds, JSON members and `blyg:*` elements, and reserved constructs;
  - any `author` object;
  - `![[id@vN]]` in foreign content;
  - long fragments;
  - missing `page` (fall back to `f/`·`t/`);
  - missing `generated` (it proves nothing);
  - per-entry feed parse failures;
  - index 404 (lossy mode);
  - `site` ≠ fetch origin (warn, keep the fetch origin).
- **Must not**:
  - re-emit imports;
  - rewrite `author` or the wire classes;
  - expose unpinned history;
  - silently accept version regressions;
  - keep withdrawn unpinned content.

---

## 6. Implications for a desktop client
1. **Two roles, two trust models.**
   - Authoring goes through the owner's server API. Upstream uses a cookie session
     from `/studio/login`. The vendored copy adds `Authorization: Bearer
     <BLYG_OWNER_TOKEN>`, which is a full owner credential.
   - Reading can go direct to public surfaces (CORS `*`) or through the server.
     Pick one per feature, because server subscription state gates cross-blyg
     transclusion and the blogroll.
2. **Save ≠ publish.**
   - `PUT /api/items/:id` saves the working copy. `POST …/publish {note?}` makes vN+1.
   - Show a dirty flag, a version counter and pins as distinct concepts.
   - Pin and withdraw need explicit, scary confirmation: pins are irrevocable,
     withdraw is permanent but reversible, and there is no delete after publish.
3. **Editor must understand both grammars.**
   - Own-line `![[id]]` (threads only, lowercase 26-char id) versus inline, which is
     inert.
   - `[TK]…[=]…[/TK]` scopes with block/inline detection, no nesting, and
     `![[id]]`-as-source inside scopes.
   - Reject or flag `@vN` locally.
   - Mirror the server's error reasons. For preview parity, reuse
     `/studio/preview{,-thread}` (JSON, owner-auth) instead of reimplementing
     resolution.
4. **Transclusion picker** needs own published items plus the server's *imported*
   blyg items. No upstream JSON endpoint lists imported items, hoppers, mentions or
   signals; the local patch only lists items and subscriptions. **New read
   endpoints needed** (inferred), or use the owner-auth JSON picker
   `GET /studio/fragments/search?q=` (returns `{id, excerpt, version, updated, badge}`
   for own and imported items).
5. **Local LLM generation will publish undisclosed** unless a provenance endpoint is
   added (§3.6). Either route generation through `/generate` or add
   `PUT …/tk-provenance`. Record `{sources:[{id,version}], model, at}` at generation
   time and keep scope indices stable between generation and publish.
   - Also note that hand-edited server output stays disclosed, and that
     index-aligned provenance breaks when scopes are reordered.
6. **Fragment cap** is 1000 chars *after* TK stripping. Compute the stripped length
   client-side.
7. **Reader engine** (if local):
   - implement §12 resolution exactly;
   - poll by conditional GET plus index reconcile;
   - keep a watermark per item, a hash check, and roll-to-null with a pin-retention
     exception;
   - use L0 wrapping and observation-clamped ordering;
   - sanitize at render while preserving the wire classes and `data-blyg-*`.
8. **Stubs, forks and responses** are server actions:
   - `/api/stubs`, `/api/items {stub_of}`, `/api/fork` (pinned only).
   - `show_responses` and `hidden` toggles.
   - Webmention delivery is automatic and fire-and-forget. Surface `warning` from
     publish.
9. **Version-skew tolerance**: expect `"blyg":"0.3"` from the ref and 0.2 docs from
   others. Code to the ignore-unknown rule, keep wire parsing lenient, and put
   grammar and API shapes behind an adapter layer. Both are pre-1.0 and have
   already changed.
