#!/usr/bin/env bash
# Sign dist/SHA256SUMS for in-app updates, then check the signature against
# the public key the app embeds (so a wrong key can't ship).
#
#   UPDATE_SIGNING_KEY="$(cat key.pem)" scripts/sign-sums.sh [dist/SHA256SUMS]
#
# UPDATE_SIGNING_KEY is a PEM Ed25519 private key (the release workflow's
# secret of the same name). The output, <sums>.sig, is the raw 64-byte
# Ed25519 signature over the exact bytes of SHA256SUMS (not base64).
# The public key is read from crates/blyg-app/src/update/verify.rs
# (RELEASE_PUBLIC_KEY_B64). For a local dry run with a throwaway key, pass
# its raw public key as UPDATE_PUBLIC_KEY_B64.
#
# Needs OpenSSL 3 (for -rawin); macOS's LibreSSL can't sign Ed25519. Uses
# Homebrew's openssl@3, installing it if it's missing.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
SUMS="${1:-dist/SHA256SUMS}"
SIG="$SUMS.sig"

die() { echo "::error::sign-sums: $*" >&2; exit 1; }

[ -n "${UPDATE_SIGNING_KEY:-}" ] || die "UPDATE_SIGNING_KEY is not set; refusing to publish an unsigned release"
[ -f "$SUMS" ] || die "$SUMS not found"

OPENSSL=""
if command -v brew >/dev/null; then
  prefix="$(brew --prefix openssl@3 2>/dev/null || true)"
  if [ -z "$prefix" ] || [ ! -x "$prefix/bin/openssl" ]; then
    brew install openssl@3 >/dev/null
    prefix="$(brew --prefix openssl@3)"
  fi
  OPENSSL="$prefix/bin/openssl"
fi
[ -x "$OPENSSL" ] || die "OpenSSL 3 not found (brew install openssl@3)"
case "$("$OPENSSL" version)" in
  "OpenSSL 3"*) ;;
  *) die "$OPENSSL is not OpenSSL 3" ;;
esac

pub="${UPDATE_PUBLIC_KEY_B64:-$(sed -nE 's/^pub const RELEASE_PUBLIC_KEY_B64: &str = "([^"]+)";.*/\1/p' crates/blyg-app/src/update/verify.rs)}"
[ -n "$pub" ] || die "couldn't read RELEASE_PUBLIC_KEY_B64"

work="$(mktemp -d)"
ok=0
trap 'rm -rf "$work"; [ "$ok" = 1 ] || rm -f "$SIG"' EXIT
# The key only ever lives in this 0600 file, and is never echoed.
(
  umask 077
  printf '%s\n' "$UPDATE_SIGNING_KEY" > "$work/key.pem"
)
"$OPENSSL" pkeyutl -sign -inkey "$work/key.pem" -rawin -in "$SUMS" -out "$SIG" \
  || die "signing failed (is UPDATE_SIGNING_KEY a PEM Ed25519 private key?)"
rm -f "$work/key.pem"

[ "$(wc -c < "$SIG" | tr -d ' ')" = 64 ] || die "$SIG is not a 64-byte Ed25519 signature"

# The app's raw 32-byte key as an SPKI DER: the fixed Ed25519 prefix + key.
{
  printf '302a300506032b6570032100' | xxd -r -p
  printf '%s' "$pub" | "$OPENSSL" base64 -d -A
} > "$work/pub.der"
[ "$(wc -c < "$work/pub.der" | tr -d ' ')" = 44 ] || die "the embedded public key isn't 32 bytes"
"$OPENSSL" pkeyutl -verify -pubin -keyform DER -inkey "$work/pub.der" -rawin \
  -in "$SUMS" -sigfile "$SIG" >/dev/null \
  || die "the signature doesn't verify with the app's embedded public key; wrong UPDATE_SIGNING_KEY?"
ok=1
echo "==> signed $SUMS -> $SIG (verified against the app's key)"
