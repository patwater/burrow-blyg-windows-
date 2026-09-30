#!/usr/bin/env bash
# Build Blygger.app and package it as a zip and a dmg in dist/.
#
#   scripts/bundle.sh                 # universal (arm64 + x86_64), the default
#   scripts/bundle.sh --arch arm64    # or x86_64: one architecture only
#
# Signing is delegated to scripts/sign.sh: Developer ID + notarization when the
# APPLE_* secrets are set, ad-hoc otherwise (see that script).
# Outputs:
#   dist/Blygger.app
#   dist/Blygger-<version>-macos-<universal|arm64|x86_64>.zip
#   dist/Blygger-<version>-macos-<universal|arm64|x86_64>.dmg
#   dist/Blygger-macos-<arch>.{zip,dmg}   (version-less copies for releases/latest/download)
#   dist/SHA256SUMS
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
source packaging/config.env

ARCH=universal
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --arch=*) ARCH="${1#--arch=}"; shift ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  arm64|aarch64) ARCH=arm64; TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "--arch must be universal, arm64 or x86_64" >&2; exit 2 ;;
esac

# Version from Cargo (workspace.package.version), e.g. "path+file:///…#blyg-app@0.1.0".
VERSION="$(cargo pkgid -p "$CARGO_PACKAGE" | sed -E 's/.*[#@]//')"
BUILD="${BUILD_NUMBER:-$VERSION}"
echo "==> $APP_NAME $VERSION ($ARCH)"

installed="$(rustup target list --installed)"
for t in "${TARGETS[@]}"; do
  if ! grep -qx "$t" <<<"$installed"; then
    echo "Rust target $t is missing: rustup target add $t (or pass --arch arm64)" >&2
    exit 1
  fi
done

# ---- build ------------------------------------------------------------------
export MACOSX_DEPLOYMENT_TARGET="$MACOS_MIN"
BINS=()
for t in "${TARGETS[@]}"; do
  echo "==> cargo build --release --target $t"
  # Panic messages embed source paths; keep the builder's home directory
  # (and so their username) out of the shipped binary.
  RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~" \
  cargo build --release --locked -p "$CARGO_PACKAGE" --bin "$BINARY_NAME" --target "$t"
  BINS+=("target/$t/release/$BINARY_NAME")
done

DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/licenses"

if [ "${#BINS[@]}" -gt 1 ]; then
  lipo -create "${BINS[@]}" -output "$APP/Contents/MacOS/$BINARY_NAME"
else
  cp "${BINS[0]}" "$APP/Contents/MacOS/$BINARY_NAME"
fi
lipo -info "$APP/Contents/MacOS/$BINARY_NAME"

# ---- assemble ---------------------------------------------------------------
sed -e "s/@APP_NAME@/$APP_NAME/g" \
    -e "s/@BINARY_NAME@/$BINARY_NAME/g" \
    -e "s/@BUNDLE_ID@/$BUNDLE_ID/g" \
    -e "s/@VERSION@/$VERSION/g" \
    -e "s/@BUILD@/$BUILD/g" \
    -e "s/@APP_CATEGORY@/$APP_CATEGORY/g" \
    -e "s/@MACOS_MIN@/$MACOS_MIN/g" \
    packaging/Info.plist.in > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
printf 'APPL????' > "$APP/Contents/PkgInfo"

[ -f packaging/Blygger.icns ] || scripts/make-icon.sh
cp packaging/Blygger.icns "$APP/Contents/Resources/$APP_NAME.icns"

# Fonts are compiled into the binary (include_bytes! in crates/blyg-app/src/fonts.rs),
# so only their licenses ship as files.
cp LICENSE "$APP/Contents/Resources/licenses/LICENSE"
cp LICENSE-docs "$APP/Contents/Resources/licenses/LICENSE-docs" # covers the icon
cp packaging/THIRD_PARTY.md "$APP/Contents/Resources/licenses/THIRD_PARTY.md"
for dir in crates/blyg-app/assets/fonts/*/; do
  name="$(basename "$dir")"
  for lic in "$dir"LICENSE* "$dir"OFL*; do
    [ -f "$lic" ] && cp "$lic" "$APP/Contents/Resources/licenses/font-$name-$(basename "$lic")"
  done
done
# Toolbar icons (Lucide, ISC; compiled in by crates/blyg-app/src/toolbar.rs).
cp crates/blyg-app/assets/icons/LICENSE "$APP/Contents/Resources/licenses/icons-lucide-LICENSE"

# ---- sign (+ notarize the app when credentials exist) -----------------------
scripts/sign.sh app "$APP"

# ---- package ----------------------------------------------------------------
STEM="$APP_NAME-$VERSION-macos-$ARCH"
ZIP="$DIST/$STEM.zip"
DMG="$DIST/$STEM.dmg"
rm -f "$ZIP" "$DMG"

echo "==> $ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"

echo "==> $DMG"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/$APP_NAME.app"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$STAGE" -fs HFS+ \
  -format UDZO -imagekey zlib-level=9 -ov "$DMG"

scripts/sign.sh dmg "$DMG"

# Version-less copies, so https://github.com/<repo>/releases/latest/download/<name>
# is a stable URL (scripts/install.sh uses it).
LATEST="$APP_NAME-macos-$ARCH"
cp "$ZIP" "$DIST/$LATEST.zip"
cp "$DMG" "$DIST/$LATEST.dmg"

(cd "$DIST" && shasum -a 256 "$STEM.zip" "$STEM.dmg" "$LATEST.zip" "$LATEST.dmg" > SHA256SUMS)
echo "==> done"
ls -lh "$ZIP" "$DMG" "$DIST/$LATEST.zip" "$DIST/$LATEST.dmg"
cat "$DIST/SHA256SUMS"
