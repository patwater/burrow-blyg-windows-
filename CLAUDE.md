# Blygger Desktop

A native (Rust + GPUI), Notational-Velocity-fast, local-first studio for Blygger blogs
("blygs", https://blygger.org). Read `docs/SPEC.md` first. The agreed UX lives in the
clickable mocks under `docs/prototype/`.

- `crates/blyg-core`: API client, SQLite store, sync, and the `Backend` trait (the contract).
- `crates/blyg-ai`: AI providers, sign-in, TK prompts.
- `crates/blyg-app`: the GPUI UI. It talks only to `Arc<dyn Backend>`. `BLYGGER_FAKE=1 cargo run -p blyg-app` runs it on in-memory sample data.
- Server requirements (owner-API extensions to the reference Worker): `docs/SERVER.md`.

Rules
- **This is a public repo.** Never commit personal data: names, emails, real blyg URLs,
  home-directory paths, account ids. Use `blyg.example.com` and invented sample content.
  User configuration lives in the Ghostty-style config file (`~/.config/blygger/config`),
  and secrets live in the macOS Keychain. Neither is ever in the repo.
- Never publish to, or write to, a real blyg during development. Use mocks or `wrangler dev`.
- Never print or log tokens.
- Never send synthetic keystrokes to the OS, or `pkill` by name, when testing the UI. Kill only PIDs you started, and capture only the app's own windows.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` must pass before any commit.
- Personal and orchestration notes go in `CLAUDE.local.md` and `docs/private/` (both gitignored).
