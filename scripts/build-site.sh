#!/usr/bin/env bash
# Build the documentation site (site/ -> site/book/) with a pinned mdBook, and
# fail on an mdBook error (such as a missing {{#include}} file)
# or on a broken internal link.
#
#   scripts/build-site.sh          build once
#   MDBOOK=mdbook scripts/build-site.sh   use an mdbook already on PATH
#
# Without MDBOOK, the pinned release is downloaded (checksum-verified) into
# target/mdbook-<version>/ on first use. CI and the Pages workflow run this.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION=0.5.4
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    ASSET="mdbook-v$VERSION-x86_64-unknown-linux-gnu.tar.gz"
    SHA=3f28de05dafca9d0f2eab99c662116b0e37b89b1d96a08f8f430b9eeae958cd7 ;;
  Darwin-arm64)
    ASSET="mdbook-v$VERSION-aarch64-apple-darwin.tar.gz"
    SHA=03e8a6d8b13a2971e0b3280affd03b388373c1485e26f73407c3a76b0b1838df ;;
  Darwin-x86_64)
    ASSET="mdbook-v$VERSION-x86_64-apple-darwin.tar.gz"
    SHA=a47d7bf0d5d670cff9ee6cce95537cbeb62dc10704d9e7131ffbd13e2b59a5de ;;
  *) ASSET="" ;;
esac

if [ -z "${MDBOOK:-}" ]; then
  dir="$ROOT/target/mdbook-$VERSION"
  MDBOOK="$dir/mdbook"
  if [ ! -x "$MDBOOK" ]; then
    if [ -z "$ASSET" ]; then
      echo "no pinned mdBook for $(uname -sm); install mdbook $VERSION and set MDBOOK=mdbook" >&2
      exit 1
    fi
    mkdir -p "$dir"
    curl -fsSL -o "$dir/$ASSET" \
      "https://github.com/rust-lang/mdBook/releases/download/v$VERSION/$ASSET"
    if command -v sha256sum >/dev/null; then sum="sha256sum"; else sum="shasum -a 256"; fi
    (cd "$dir" && echo "$SHA  $ASSET" | $sum -c - >/dev/null)
    tar -xzf "$dir/$ASSET" -C "$dir"
    rm "$dir/$ASSET"
  fi
fi

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
"$MDBOOK" build site 2>&1 | tee "$log"
# Warnings (such as a bare <placeholder> in the text, which renders as
# nothing) are shown as annotations on GitHub but don't fail the build.
if [ -n "${GITHUB_ACTIONS:-}" ]; then
  grep 'WARN' "$log" | sed 's/^ *WARN */::warning title=mdbook::/' || true
fi
# Errors (such as a missing {{#include}} file) do.
if grep -q 'ERROR' "$log"; then
  echo "mdbook reported errors (above); fix them before publishing" >&2
  exit 1
fi
python3 scripts/site-linkcheck.py site/book
