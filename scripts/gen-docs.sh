#!/usr/bin/env bash
# Regenerate the documentation site's generated pages from the source of truth:
#   site/src/generated/config-keys.md  <- crates/blyg-core/src/config/keys.rs
#   site/src/generated/keybindings.md  <- crates/blyg-app/src/keymap.rs
# `cargo test` fails while either file is out of date, so CI catches drift.
set -euo pipefail
cd "$(dirname "$0")/.."
export BLYGGER_UPDATE_DOCS=1
cargo test -q -p blyg-core --test config_docs
cargo test -q -p blyg-app keymap_docs
git status --short site/src/generated
