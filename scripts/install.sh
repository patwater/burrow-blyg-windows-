#!/usr/bin/env bash
# Install the latest Blygger Desktop release.
#
#   curl -fsSL https://raw.githubusercontent.com/aneeshsathe/blygger-desktop/main/scripts/install.sh | bash
#
# What it does: downloads Blygger-macos-universal.zip from the latest GitHub
# release, checks it against the release's SHA256SUMS (and that file's
# Ed25519 signature, when OpenSSL 3 is installed), and unzips Blygger.app
# into /Applications (or ~/Applications if /Applications isn't writable).
#
# Files downloaded with curl don't get macOS's quarantine attribute, so
# Gatekeeper doesn't block the unsigned app. The script also strips the
# attribute explicitly, in case a future macOS starts adding it.
#
# Environment overrides:
#   BLYGGER_VERSION=0.1.0   install that version instead of the latest
#   BLYGGER_DEST=<dir>      install into <dir>
#   BLYGGER_NO_VERIFY=1     skip the checksum check (not recommended)
#   BLYGGER_BASE_URL=<url>  download from <url>/<asset> instead of GitHub
set -euo pipefail

REPO="aneeshsathe/blygger-desktop" # set with scripts/set-repo.sh
ASSET="Blygger-macos-universal.zip"
APP="Blygger.app"
# The release signing key (raw Ed25519 public key, base64); the same one is
# embedded in the app (crates/blyg-app/src/update/verify.rs).
UPDATE_PUBKEY="mnXJcOWPGSTrMKx38w6FqoKQxky6+pj3Ch2IVPrk3+I="

say() { printf '%s\n' "$*"; }
die() { printf 'blygger install: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "Blygger Desktop is macOS-only."
major="$(sw_vers -productVersion | cut -d. -f1)"
[ "$major" -ge 11 ] || die "Blygger needs macOS 11 or later (this is $(sw_vers -productVersion))."

if [ -n "${BLYGGER_BASE_URL:-}" ]; then # a mirror, or a local server for testing
  base="${BLYGGER_BASE_URL%/}"
elif [ -n "${BLYGGER_VERSION:-}" ]; then
  base="https://github.com/$REPO/releases/download/v${BLYGGER_VERSION#v}"
else
  base="https://github.com/$REPO/releases/latest/download"
fi

if [ -n "${BLYGGER_DEST:-}" ]; then
  dest="$BLYGGER_DEST"
elif [ -w /Applications ]; then
  dest=/Applications
else
  dest="$HOME/Applications"
fi
mkdir -p "$dest"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Downloading $ASSET…"
curl -fL --progress-bar -o "$tmp/$ASSET" "$base/$ASSET" || die "download failed: $base/$ASSET"

if [ -z "${BLYGGER_NO_VERIFY:-}" ]; then
  curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "couldn't download SHA256SUMS"
  # Releases from 0.3.0 on also sign SHA256SUMS with the project's Ed25519
  # key (the one the app's updater trusts). Check it when OpenSSL 3 is
  # installed; older releases have no .sig and skip this.
  if curl -fsSL -o "$tmp/SHA256SUMS.sig" "$base/SHA256SUMS.sig" 2>/dev/null; then
    ossl=""
    for c in /opt/homebrew/opt/openssl@3/bin/openssl /usr/local/opt/openssl@3/bin/openssl openssl; do
      if command -v "$c" >/dev/null 2>&1 && "$c" version 2>/dev/null | grep -q '^OpenSSL 3'; then
        ossl="$c"
        break
      fi
    done
    if [ -n "$ossl" ]; then
      # The raw 32-byte public key as SPKI DER (fixed Ed25519 prefix + key).
      { printf '302a300506032b6570032100' | xxd -r -p
        printf '%s' "$UPDATE_PUBKEY" | "$ossl" base64 -d -A; } > "$tmp/pub.der"
      "$ossl" pkeyutl -verify -pubin -keyform DER -inkey "$tmp/pub.der" -rawin \
        -in "$tmp/SHA256SUMS" -sigfile "$tmp/SHA256SUMS.sig" >/dev/null 2>&1 \
        || die "SHA256SUMS signature check failed; not installing."
      say "Signature OK."
    else
      say "(Skipping the signature check: it needs OpenSSL 3, e.g. brew install openssl@3.)"
    fi
  fi
  (cd "$tmp" && grep " $ASSET\$" SHA256SUMS | shasum -a 256 -c -) >/dev/null \
    || die "checksum mismatch for $ASSET; not installing."
  say "Checksum OK."
fi

ditto -x -k "$tmp/$ASSET" "$tmp/unpacked"
[ -d "$tmp/unpacked/$APP" ] || die "$APP not found in the archive"

if [ -d "$dest/$APP" ]; then
  if pgrep -qf "$dest/$APP/Contents/MacOS/"; then
    die "Blygger is running from $dest/$APP. Quit it (⌘Q) and run this again."
  fi
  rm -rf "${dest:?}/${APP:?}"
fi
ditto "$tmp/unpacked/$APP" "$dest/$APP"
xattr -dr com.apple.quarantine "$dest/$APP" 2>/dev/null || true

version="$(defaults read "$dest/$APP/Contents/Info" CFBundleShortVersionString 2>/dev/null || echo '?')"
say ""
say "Installed Blygger $version to $dest/$APP"
say "Open it from Launchpad or Spotlight, or run:  open \"$dest/$APP\""
