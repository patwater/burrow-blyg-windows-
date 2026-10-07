# Building and testing

## Building from source

You need macOS and Rust via [rustup](https://rustup.rs). The toolchain version
is pinned in `rust-toolchain.toml` and installs automatically.

```sh
git clone https://github.com/aneeshsathe/blygger-desktop && cd blygger-desktop
BLYGGER_FAKE=1 cargo run -p blyg-app --release   # the full UI on sample data, no server
cargo test --workspace
```

`BLYGGER_FAKE=1` runs the app on in-memory sample data, so you can try every
screen without a blyg. It never touches the real Keychain or checks for
updates.

To build the app bundle, zip and dmg in `dist/`:

```sh
rustup target add x86_64-apple-darwin   # only for universal builds
scripts/bundle.sh                        # universal (the default)
scripts/bundle.sh --arch arm64           # this Mac's architecture only
```

`scripts/bundle.sh` signs ad-hoc unless Developer ID credentials are set in
the environment. With them, it signs, notarizes and staples the app and the
dmg. `scripts/sign.sh` documents the variables.

## The checks

Every commit must pass:

```sh
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

CI (`.github/workflows/ci.yml`) runs the same on every pull request, and
builds this documentation site.

`cargo test` covers the core against a mock HTTP server (sync, the outbox,
conflicts, scratch notes that must never touch the network), the config
parser, the renderer's parity fixtures, the AI prompts' parity with the
server, the updater (offline, with a throwaway signing key), and the app's
view models, keymap, menus and toolbar.

## End-to-end tests

End-to-end tests run blyg-core against a real Worker under `wrangler dev
--local` (never a deployed blyg). Point them at a Worker checkout that carries
the extensions in [Server requirements](../server.md) (blygger-studio 0.9 or
later; build it first):

```sh
(cd /path/to/worker && npm install && npm run build)
BLYG_WORKER_DIR=/path/to/worker scripts/e2e-local.sh
```

Never test against a real blyg: use the mocks, the sample data, or
`wrangler dev`.

## Parity fixtures

The preview renderer and the AI prompts are checked byte for byte against the
reference Worker's own TypeScript. When the Worker changes, regenerate the
fixtures:

```sh
node crates/blyg-render/tests/fixtures/gen_parity.mjs /path/to/worker/package
node crates/blyg-ai/tests/fixtures/gen_parity.mjs /path/to/worker/src
```

See [blyg-render](render.md#parity) and [blyg-ai](ai.md) for the details.

## This documentation site

The site is an [mdBook](https://rust-lang.github.io/mdBook/) in `site/`. Its
pages include the files in `docs/` where they exist, so there's one source for
each. Two pages are generated from the code:

| Page | Generated from | By |
|---|---|---|
| `site/src/generated/config-keys.md` | `crates/blyg-core/src/config/keys.rs` | `crates/blyg-core/tests/config_docs.rs` |
| `site/src/generated/keybindings.md` | `crates/blyg-app/src/keymap.rs` | `crates/blyg-app/src/keymap_docs.rs` |

`cargo test` fails while either is out of date. After changing a config key
or a shortcut, regenerate them and commit the result:

```sh
scripts/gen-docs.sh
```

To build the site locally:

```sh
scripts/build-site.sh  # into site/book/, with the pinned mdBook; checks links
```

It downloads the pinned mdBook release into `target/` the first time
(checksum-verified), and fails on an mdBook error (such as a missing
`\{{#include}}` file) or a broken internal link. CI runs it on every pull request. With mdBook installed
yourself, `mdbook serve site` serves the site at `http://localhost:3000` and
rebuilds it as you edit.

`.github/workflows/pages.yml` publishes the site to GitHub Pages on every push
to `main` that touches the site, `docs/`, the config keys, the keymap or the
changelog.
