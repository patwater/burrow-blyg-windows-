# Architecture

Burrow is a Rust workspace of four crates. The UI never talks to the network
or the database directly: it talks to one trait, `Backend`, and everything
behind it is local-first.

```
crates/blyg-core    model.rs (types) · backend.rs (the Backend trait, the UI seam)
                    api/    ureq client, one function per owner endpoint, typed errors
                    store/  rusqlite: items, versions, outbox, reading list, FTS5 search
                    sync/   worker thread: debounce, outbox flush, periodic pull, conflicts
                    live.rs LiveBackend: Backend over store + api + sync
                    config/ the Ghostty-style config file and its one key table
crates/blyg-render  the studio preview renderer: a Rust port of the reference
                    Worker's pipeline, checked against it by parity fixtures
crates/blyg-ai      AI providers, sign-in and credentials, TK and helper prompts
crates/blyg-app     the GPUI app. It talks only to Arc<dyn Backend>, and has a
                    FakeBackend with sample data (BLYGGER_FAKE=1)
```

- **Local-first.** The UI reads only from SQLite. Every edit is saved locally
  and queued in an outbox; a background thread pushes it about 800 ms after
  typing stops, and pulls the server's changes periodically.
- **No async runtime in the app.** Network calls are blocking `ureq` (rustls)
  on background threads.
- **Secrets** (the owner token, AI keys) live in the macOS Keychain, never in
  a file and never in logs.
- **One table each** for configuration keys (`blyg-core/src/config/keys.rs`)
  and keyboard shortcuts (`blyg-app/src/keymap.rs`). Defaults, validation,
  menus, toolbar buttons, `+show-config` and this site's
  [Configuration](../config.md) and [Keyboard shortcuts](../keys.md) pages
  are all generated from them.

## Where to read next

- [The spec](spec.md) (`docs/SPEC.md`): the settled decisions, the interaction
  design and every feature as built. The clickable mocks in `docs/prototype/`
  are the agreed UX; open `docs/prototype/index.html` in a browser.
- [blyg-core](core.md) (`docs/CORE.md`): the outbox, conflicts, provenance,
  the reading list and performance.
- [blyg-render](render.md) (`docs/RENDER.md`): the preview pipeline and
  parity with the reference Worker.
- [blyg-ai](ai.md) (`docs/AI.md`): providers, sign-in, prompts, provenance.
- [Blygger protocol digest](protocol.md) (`docs/BLYGGER-SPEC-DIGEST.md`): the
  parts of the Blygger protocol a desktop client needs.
- [Server requirements](../server.md) (`docs/SERVER.md`): the owner-API
  extensions the app needs.

These pages include the files in `docs/` directly, so the repository and this
site never disagree.
