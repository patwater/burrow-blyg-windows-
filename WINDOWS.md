# Blygger Desktop on Windows

This is a Windows build of Blygger Desktop, the native studio for Blygger blogs. It is the same app as the macOS version, built from the same code, with a small set of Windows-specific changes described below. It reads the blygs and RSS feeds you subscribe to and writes to your own blyg, in one window.

## Install

Download the `Blygger-<version>-windows-x64.zip` from the release page, check it against `SHA256SUMS` if you like (`Get-FileHash .\Blygger-…zip` in PowerShell), unzip it anywhere, and run `blygger.exe`.

The build is not code-signed yet, so the first launch shows Microsoft Defender SmartScreen's "Windows protected your PC" dialog. Choose **More info**, then **Run anyway**. You only need to do this once per version.

The app needs the Microsoft Edge WebView2 Runtime to show posts and the full editor. Windows 11 includes it, and most Windows 10 machines already have it through Edge. If posts show "The preview couldn't start", install the runtime from Microsoft's WebView2 download page.

## Connect your blyg

The first launch walks you through **Connect your blyg**: your blyg's address and an owner token. The app talks to your blyg through its owner API with a bearer token, which the server must support (see `docs/SERVER.md`); on the server, the token is the Worker secret `BLYG_OWNER_TOKEN`. Choosing *Skip, just try it with sample data* runs the whole app on built-in sample content instead.

## What is different from macOS

- **Keys.** Every `⌘` shortcut is `Ctrl` on Windows, so Publish is `Ctrl+Enter` and search is `Ctrl+L`. The one exception is Versions, which is `Ctrl+Shift+Y` rather than `Ctrl+Y`, because `Ctrl+Y` is Redo on Windows. Toolbar tooltips, the menu, the tutorial, and the hints written into the app's text all show the Windows keys.
- **Menu.** Windows has no global menu bar, so a **Menu** button at the top left of the window lists every menu item, including the ones without a key (Subscribe…, Site Settings…, Open Config File).
- **Title bar.** The window uses the normal Windows title bar with its minimise, maximise, and close buttons.
- **Where things live.** The config file is `%APPDATA%\Blygger\config` (Menu › Open Config File opens it in Notepad). The local database, caches, and media are in `%LOCALAPPDATA%\Blygger\`. Your token and AI keys are stored in Windows Credential Manager under `org.blygger.desktop`, and the app's text says so wherever the macOS app says Keychain.
- **Settings › AI.** Each key field has a **Save** button and the sheet has a **Done** button, alongside the ⏎ and Esc keys that macOS relies on. The Subscribe sheet's keys are buttons too, and Esc closes any reading sheet from anywhere inside it.
- **A refused token.** When your blyg starts refusing the owner token (401), the app says so once, instead of showing only "sync error".
- **Fonts.** The macOS system fonts in the font list become their nearest Windows equivalents: New York becomes Georgia, Charter becomes Cambria, SF Pro becomes Segoe UI, and Menlo becomes Consolas. The bundled fonts, including the defaults, are the same on both.
- **Updates.** The macOS app updates itself; the Windows build does not yet. Download new versions from the release page.
- **AI helpers.** The "local Claude Code" and "Codex" providers find `claude` and `codex` on your `PATH`, in `%USERPROFILE%\.local\bin`, and in npm's global folder (`%APPDATA%\npm`).

## Build from source

Install Rust with `rustup` (the MSVC toolchain, which needs the Visual Studio Build Tools with the "Desktop development with C++" workload, including the Windows SDK). Then, from this folder:

```powershell
cargo run -p blyg-app                          # debug build
$env:BLYGGER_FAKE = "1"; cargo run -p blyg-app  # on sample data, no server
cargo build --release -p blyg-app              # target\release\blygger.exe
```

`cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` pass on Windows and macOS alike.

## Publishing a Windows release

Every push runs the *Windows* workflow, which checks formatting, lints, tests, builds, and leaves the zip on the run's page under **Artifacts**. To publish a build, open **Releases → Draft a new release**, create a new tag on `main` named like `windows-v0.3.0-win3` (one new tag per build, never a reused one), and publish. The workflow attaches the zip and `SHA256SUMS` to the release about ten minutes later. Avoid tags that start with a plain `v`, because those run upstream's macOS release workflow.

## How the port works

Every Windows change is gated with `cfg(target_os = "windows")` (or `cfg(windows)`), so the macOS build is unchanged. The pieces:

| Area | Change |
|---|---|
| Preview and editor | A WebView2 surface through `wry` (`studio/webview.rs`), reusing the macOS page script, IPC, and navigation guard. GPUI's topmost DirectComposition layer is turned off at startup (`GPUI_DISABLE_DIRECT_COMPOSITION`), because it would cover the child webview. Creating a WebView2 runs a nested message loop, and focusing, showing or hiding one can send messages to GPUI's window synchronously; either crashed the app when it happened mid-frame. So `DeferredSurface` makes no WebView2 call itself: it records what it is asked (`Ops`, coalesced), and a thread timer in GPUI's top-level loop builds the WebView and applies them. |
| Keys | `keymap::platform_keys` respells `cmd` as `ctrl`; `glyphs` and `hotkey_glyphs` write `Ctrl+Shift+X`; `keymap::hint` rewrites key hints in the app's text (`⇧⌘G` → `Ctrl+Shift+G`) and is the identity on macOS; the clash tests use a Windows list of system shortcuts. |
| Window and menu | The native title bar, and `windows_menu.rs`, a Menu button that lists `cx.get_menus()`. The toolbar strip is not a window drag area on Windows (`app.rs`), because GPUI answers `HTCAPTION` for it and clicks on the toolbar and Menu buttons drawn over it would move the window instead. Release builds use the `windows` subsystem, so no console window opens; `blygger +action` attaches to the terminal that ran it. |
| Wording | `keymap::hint` also rewrites macOS words (`Keychain` → `Credential Manager`, `this Mac` → `this PC`); the config file header is Windows-worded. |
| Secrets | `KeychainTokenStore` splits secrets longer than one Credential Manager entry holds (1,280 UTF-16 units, less than a ChatGPT sign-in) across `account#1`, `account#2`… (`config/tokens.rs`). |
| Fonts | `prefs::system` names Windows fonts in place of the macOS system fonts. |
| Paths and tokens | `%APPDATA%` and `%LOCALAPPDATA%` in `blyg-core/src/config/paths.rs`; `keyring`'s `windows-native` backend. |
| External programs | Notepad for the config file, the shell URL handler for the browser, `.exe`/`.cmd` CLI shims. The CLI bridges start with `CREATE_NO_WINDOW`, and npm's `claude.cmd` shim gets the system prompt through `--system-prompt-file`, because cmd.exe can't pass a multi-line argument. |
| Updates | Off on Windows (`update::disabled_reason`); the updater's tests stay macOS-only. |
| Executable | `build.rs` embeds `packaging/Blygger.ico`; GPUI's `windows-manifest` feature supplies the per-monitor-DPI manifest. |

## If it crashes

A crash shows a message and writes what happened to `%LOCALAPPDATA%\Blygger\crash.log` (the latest crash only). Please include that file when you report it.

## Known gaps

- The Windows build has been compiled, linted, and tested in CI, but it has not yet had much use on real Windows machines. Please report anything that looks or behaves wrong.
- Keyboard focus between the webview and the editor is handled more simply than on macOS: when the preview hides or the window regains focus, the keyboard always returns to the editor.
- No installer, auto-update, or code signing yet.
