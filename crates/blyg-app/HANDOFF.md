# blyg-app status (phase 1 complete; live backend wired)

## Spike verdict: GPUI works for the editor. Go.
- Stack: `gpui-kit =0.6.6` (default-features off), which pins `gpui-pre 0.3.6` plus `gpui-base 0.6.6`.
  We draw everything ourselves on gpui-base's **unstyled** `Textarea`/`Input`. The styled gpui-component layer isn't used.
- Evidence: `tests/editor_spike.rs` (headless) covers 20k chars, soft wrap, ⌘↑/⌘↓, undo/redo, IME marked text and emoji.
  It measured about 0.7 ms per keystroke (edit plus layout) in a debug build. Caret, selection and colours are themed
  through `InputEditorStyle`.

## Layout of the crate
- `connection.rs`: which backend runs (Live / Fake / Disconnected) behind a
  `SwitchBackend`, so Connect and Disconnect swap it in place; the verified
  connect (`GET /api/items` first) and `Disconnect…`. A local database that
  belongs to another blyg is moved aside, never pushed to the new one.
- `keymap.rs`: every key binding in one table (`blygger +list-keybinds`), with
  clash tests (gpui-base Input keys, the capture hotkey, macOS-reserved keys,
  planned ⌘Y/⌘G/⇧⌘G). ⌘⏎ is bound in `Blygger > Input` so it beats
  gpui-base's submit binding; the Publish menu gets a ⌘↩ shim binding because
  GPUI turns "enter" into the key equivalent "e" (it showed ⌘E).
- `platform.rs`: the Dock icon from `packaging/icon-512.png` when not bundled.
- `vm.rs`: pure view-model with unit tests.
  - List/omnibar state machine, counter and banner rules, sync and version labels, publish decision.
  - Highlight ranges, relative time.
  - Placeholder insert/replace/remove, minimal splice, conflict word diff.
- `fake.rs`: FakeBackend seeded with 6 invented sample items (origin `https://blyg.example.com`),
  plus one imported post from `https://notes.example.org/` so a thread can quote it.
  - Save → Saving → (800 ms debounce) → Syncing → (350 ms) → Synced.
  - Offline and conflict hooks, bound to ⌃⌥⌘O / ⌃⌥⌘C in the app.
  - Uploads go to `<data dir>/media` (`~/Library/Application Support/org.blygger.desktop/media`).
- `app.rs`: the main window, sheets (incl. first-run "Connect your blyg"), toasts, status bar, config-problems banner.
- `studio/` (a child module of `app`): the full editor (SPEC § Full editor).
  - `mod.rs`: modes ⌘1 write / ⌘2 list + editor + preview / ⌘3 editor + preview, ⌘E toggles
    the preview in place (⌘3 → editor alone). The mode is remembered in `state.json` (`view`).
    Renders with `blyg-render` 100 ms after typing pauses, on a background thread; the first
    load is a `page_shell` page, then `.item-content` is patched in place (unchanged blocks,
    a playing video and the scroll survive). Status bar counts and the unresolved-quote warning
    (also in the publish sheet). The WebView is hidden while a sheet is open (a native view
    sits above GPUI), and toasts move left of it.
  - `webview.rs`: `PreviewSurface` (a trait, so tests use a stub) and the `wry` WKWebView
    attached as a child of GPUI's NSView, framed from the pane's bounds every paint. Our host
    script (patch, scroll-to-block, click → `line:N` over IPC) is a WKUserScript; content gets
    no script (the page CSP). Every navigation is refused; links go to the default browser.
  - `style_cache.rs`: the blyg's public `/style.css`, fetched once per connection with the
    token-less `PublicClient`, cached in `<data dir>/style-cache/`, used offline; built-in CSS
    otherwise (always in fake mode).
  - `resolver.rs`: `![[id]]` from the local store only (own items, then imported reading items).
- `capture.rs`: global hotkey and the quick-capture panel.
- `prefs.rs`: the UI's view of the config file (fonts, size, theme, layout, hotkey) and the diff written back.
- `settings.rs`: the config global (`ConfigStore` + token store), app-level validation, write-back, reload, migration.
- `cli.rs`: `blygger +show-config [--default] [--docs]`, `+validate-config`, `+list-fonts`, `+version`, `+help`.
- `fonts.rs`: bundled fonts.
- `theme.rs`: Tufte tokens.
- `demo.rs`: `BLYGGER_DEMO=…` scripted scenarios.
- `ui_tests.rs`: headless GPUI keystroke tests.

## Dev env vars
- `BLYGGER_DEMO=studio|studio-edit`: the sample thread (quote, TK, video, unresolved quote) in the full editor (⌘3); `studio-edit` also types into it.
- `BLYGGER_TIMING=1` also prints `preview-ready frame=…`, whether the WebView holds the keyboard, and a JSON probe of what the page rendered.
- `BLYGGER_DEMO=search|create|edit|publish|published|long|preview|conflict|offline|settings|capture|image`
- `BLYGGER_THEME=light|dark` (not persisted).
- `BLYGGER_CONFIG=<file>`: use this config file instead of `~/.config/blygger/config` + `~/Library/Application Support/org.blygger.desktop/config`.
- `BLYGGER_DATA_DIR=<dir>`: app state (db, media) goes here instead of `~/Library/Application Support/org.blygger.desktop/` (also skips the old-dir migration).
- `BLYGGER_FAKE=1`: sample data, and the token store is in memory (the Connect sheet never touches the Keychain or the network).
- `BLYGGER_TEST_TOKEN=<token>`: automation against a local `wrangler dev`: an in-memory token for the config's `blyg-url`, and no Keychain at all.
- `BLYGGER_DEMO=live-publish|live-open|live-image|live-preview|connect-check` (with `BLYGGER_DEMO_URL`): scenarios for a real (local) blyg.
- `BLYGGER_NO_ACTIVATE=1`: never take focus (uses `orderFrontRegardless`), for automated screenshots.
- `BLYGGER_TIMING=1`: prints ms from `main()` to the first frame, the backend in use, the initial sync (`initial-sync items=N ok=…`) and each publish (`publish ok vN <permalink>`).

## Screenshots
- Window-only captures are in `docs/screenshots/`.
- Method: run the app with its own PID, find its CGWindowID, then `screencapture -l`.
- Never full-screen captures, never synthetic OS input, never `pkill` by name.
- Run with a scratch `HOME`, `BLYGGER_CONFIG` and `BLYGGER_DATA_DIR`. Fake-data shots: `BLYGGER_FAKE=1` and `blyg-url = https://blyg.example.com`. Live shots (`live-*.png`, `connect-check.png`): a local `wrangler dev` seeded with invented posts, `blyg-url = http://127.0.0.1:<port>`, `BLYGGER_TEST_TOKEN`, `BLYGGER_NO_ACTIVATE=1`.

## Known issues / rough edges
- The preview's remote images load in the WebView itself (a failed one becomes a link, per `preview_script`).
- A quoted own post is baked from its local working copy (the published snapshot isn't stored locally), so unpublished edits to it show.
- Attachment rows (`preview_media`) aren't shown: the Backend has no attachment list, and pasted images are inline.
- Pasted images upload unattached and go into the text as an absolute URL (see docs/SPEC.md: the Worker appends attachments to public pages and renders relative `media/…` page-relative).
- The caret blink is gpui-base's (500 ms, solid while typing, off when the window isn't active). Verified in the release app with window-only captures 200 ms apart (the caret alternates) and headlessly (`the_caret_blinks_and_holds_while_typing`).
- The omnibar placeholder isn't italic. The quick-capture panel has no drop shadow.
- Withdraw, versions, pin and delete have no UI yet (phase 2).
- ⌘. is unbound on purpose (a tester pressed it expecting something; nothing is planned for it).

## Verify
```
cargo fmt && cargo clippy -p blyg-app --all-targets -- -D warnings && cargo test -p blyg-app
cargo build -p blyg-app --release
BLYGGER_FAKE=1 cargo run -p blyg-app --release
```
