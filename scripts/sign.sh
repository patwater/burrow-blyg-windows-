#!/usr/bin/env bash
# Sign (and, when possible, notarize + staple) Blygger.app or its dmg.
#
#   scripts/sign.sh app dist/Blygger.app
#   scripts/sign.sh dmg dist/Blygger-0.1.0-macos-universal.dmg
#
# Behaviour depends on which environment variables are set:
#
#   APPLE_DEVELOPER_ID_APP   "Developer ID Application: <Name> (<TEAMID>)".
#                            Unset → ad-hoc signing (codesign -s -), no notarization.
#   APPLE_CERTIFICATE_P12_BASE64 + APPLE_CERTIFICATE_PASSWORD
#                            (CI) a base64 .p12 of that identity. It's imported
#                            into a throwaway keychain first.
#   Notarization credentials, either
#     APPLE_ID + APPLE_TEAM_ID + APPLE_APP_PASSWORD  (app-specific password), or
#     APPLE_API_KEY_ID + APPLE_API_ISSUER + APPLE_API_KEY_P8_BASE64  (App Store Connect API key).
#                            Developer ID set but no notary credentials → signed, not notarized.
#
# Never prints secrets.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENTITLEMENTS="$ROOT/packaging/Blygger.entitlements"

KIND="${1:?usage: sign.sh app|dmg <path>}"
TARGET="${2:?usage: sign.sh app|dmg <path>}"

log() { echo "==> sign: $*"; }

have() { [ -n "${!1:-}" ]; }

CLEANUP=()
cleanup() {
  local c
  for c in "${CLEANUP[@]:-}"; do
    if [ -n "$c" ]; then eval "$c" || true; fi
  done
  return 0
}
trap cleanup EXIT

# ---- (CI) import the certificate into a temporary keychain ------------------
import_certificate() {
  have APPLE_CERTIFICATE_P12_BASE64 || return 0
  have APPLE_CERTIFICATE_PASSWORD || { echo "APPLE_CERTIFICATE_PASSWORD is required with the .p12" >&2; exit 1; }
  local tmp kc kcpass
  tmp="$(mktemp -d)"
  kc="$tmp/blygger-signing.keychain-db"
  kcpass="$(uuidgen)"
  printf '%s' "$APPLE_CERTIFICATE_P12_BASE64" | base64 --decode > "$tmp/cert.p12"
  security create-keychain -p "$kcpass" "$kc"
  security set-keychain-settings -lut 3600 "$kc"
  security unlock-keychain -p "$kcpass" "$kc"
  security import "$tmp/cert.p12" -k "$kc" -P "$APPLE_CERTIFICATE_PASSWORD" \
    -T /usr/bin/codesign -T /usr/bin/security >/dev/null
  security set-key-partition-list -S apple-tool:,apple: -s -k "$kcpass" "$kc" >/dev/null
  # Put it first in the search list, keeping the existing ones.
  local existing
  existing="$(security list-keychains -d user | tr -d '"' | xargs)"
  # shellcheck disable=SC2086
  security list-keychains -d user -s "$kc" $existing
  rm -f "$tmp/cert.p12"
  CLEANUP+=("security list-keychains -d user -s $existing; security delete-keychain '$kc' 2>/dev/null; rm -rf '$tmp'")
  log "imported the signing certificate into a temporary keychain"
}

# ---- notarytool credentials -------------------------------------------------
NOTARY_ARGS=()
notary_setup() {
  if have APPLE_API_KEY_ID && have APPLE_API_ISSUER && have APPLE_API_KEY_P8_BASE64; then
    local keyfile
    keyfile="$(mktemp -d)/AuthKey_${APPLE_API_KEY_ID}.p8"
    printf '%s' "$APPLE_API_KEY_P8_BASE64" | base64 --decode > "$keyfile"
    CLEANUP+=("rm -rf '$(dirname "$keyfile")'")
    NOTARY_ARGS=(--key "$keyfile" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER")
  elif have APPLE_ID && have APPLE_TEAM_ID && have APPLE_APP_PASSWORD; then
    NOTARY_ARGS=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD")
  fi
}

notarize() {
  local file="$1" staple="$2"
  log "submitting $(basename "$file") to the notary service (this can take minutes)"
  xcrun notarytool submit "$file" "${NOTARY_ARGS[@]}" --wait --timeout 30m
  xcrun stapler staple "$staple"
  xcrun stapler validate "$staple"
}

# ---- main -------------------------------------------------------------------
if ! have APPLE_DEVELOPER_ID_APP; then
  log "no APPLE_DEVELOPER_ID_APP: ad-hoc signing $(basename "$TARGET") (not notarized)"
  if [ "$KIND" = app ]; then
    codesign --force --sign - --options runtime --entitlements "$ENTITLEMENTS" "$TARGET"
    codesign --verify --strict --verbose=2 "$TARGET"
  fi
  # An ad-hoc signature on a dmg adds nothing; leave the image unsigned.
  exit 0
fi

import_certificate
notary_setup

case "$KIND" in
  app)
    log "Developer ID signing $(basename "$TARGET") with the hardened runtime"
    codesign --force --timestamp --options runtime \
      --entitlements "$ENTITLEMENTS" \
      --sign "$APPLE_DEVELOPER_ID_APP" "$TARGET"
    codesign --verify --strict --deep --verbose=2 "$TARGET"
    if [ "${#NOTARY_ARGS[@]}" -gt 0 ]; then
      sub="$(mktemp -d)/notarize.zip"
      ditto -c -k --keepParent "$TARGET" "$sub"
      notarize "$sub" "$TARGET"
      rm -rf "$(dirname "$sub")"
      spctl --assess --type execute --verbose=2 "$TARGET"
    else
      log "no notarization credentials: signed but NOT notarized"
    fi
    ;;
  dmg)
    log "Developer ID signing $(basename "$TARGET")"
    codesign --force --timestamp --sign "$APPLE_DEVELOPER_ID_APP" "$TARGET"
    if [ "${#NOTARY_ARGS[@]}" -gt 0 ]; then
      notarize "$TARGET" "$TARGET"
    else
      log "no notarization credentials: signed but NOT notarized"
    fi
    ;;
  *) echo "usage: sign.sh app|dmg <path>" >&2; exit 2 ;;
esac
