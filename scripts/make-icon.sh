#!/usr/bin/env bash
# Regenerate packaging/Blygger.icns (and packaging/icon-512.png, which the
# bare binary embeds as its Dock icon) from packaging/icon.svg.
# Needs macOS (sips, iconutil) and an SVG rasteriser: rsvg-convert
# (`brew install librsvg`) when present, otherwise Quick Look (qlmanage).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SVG="$ROOT/packaging/icon.svg"
OUT="${1:-$ROOT/packaging/Blygger.icns}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

MASTER="$WORK/icon-1024.png"
if command -v rsvg-convert >/dev/null 2>&1; then
  rsvg-convert -w 1024 -h 1024 "$SVG" -o "$MASTER"
else
  qlmanage -t -s 1024 -o "$WORK" "$SVG" >/dev/null 2>&1
  mv "$WORK/$(basename "$SVG").png" "$MASTER"
fi

SET="$WORK/Blygger.iconset"
mkdir -p "$SET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$MASTER" --out "$SET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z "$d" "$d" "$MASTER" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o "$OUT"
echo "wrote $OUT"
sips -z 512 512 "$MASTER" --out "$ROOT/packaging/icon-512.png" >/dev/null
echo "wrote $ROOT/packaging/icon-512.png"
