#!/usr/bin/env bash
# Regenerate packaging/rust-dependencies.tsv: every crate linked into the macOS
# app (both architectures; dev- and build-only dependencies excluded) with its
# license. Needs cargo-license: `cargo install cargo-license --locked`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT=packaging/rust-dependencies.tsv
{
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo license --tsv --avoid-dev-deps --avoid-build-deps --filter-platform "$t" | tail -n +2
  done
} | sort -u | { printf 'name\tversion\tauthors\trepository\tlicense\tlicense_file\tdescription\n'; cat; } > "$OUT"
# The summary (crate counts per license), for packaging/THIRD_PARTY.md.
tail -n +2 "$OUT" | cut -f5 | sort | uniq -c | sort -rn
echo "wrote $OUT ($(($(wc -l < "$OUT") - 1)) crates)"
