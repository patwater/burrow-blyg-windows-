#!/usr/bin/env bash
# The browser pane's content blocking, end to end, on this Mac (no network):
#
#   scripts/browser-block-check.sh [out.png]
#
# 1. serves a test page from a local HTTP server (python3, an ephemeral port)
#    with "ads": an image and a script from 127.0.0.1 (third-party to the
#    page's localhost) and a `.ad-slot` box;
# 2. writes a filter list that blocks them (`||127.0.0.1^$third-party`,
#    `##.ad-slot`) into a scratch data dir, as if it had been downloaded;
# 3. runs the app in fake mode with BLYGGER_DEMO=br-block: it opens the pane
#    on the page, probes the ads (JS in the page, printed as
#    `browser-probe shield=on {…}`), switches the shield off, probes again,
#    switches it back on, and saves a snapshot of the window;
# 4. checks the probes, then stops the server it started (by PID).
#
# BLYGGER_REAL_LISTS=<dir of <id>.txt files> adds real filter lists (e.g.
# from `cargo test -p blyg-app real_lists -- --ignored`) to measure compile
# times; BLYGGER_RUNS=2 runs the app twice on the same data dir (the second
# run finds the compiled lists cached).
#
# Needs a snapshot build:
#   RUSTFLAGS="--cfg blygger_snap" CARGO_TARGET_DIR=target/snap \
#     cargo build -p blyg-app --features gpui-kit/test-support
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BLYGGER_BIN:-$ROOT/target/snap/debug/blygger}"
OUT="${1:-${TMPDIR:-/tmp}/blygger-browser.png}"
[ -x "$BIN" ] || { echo "build the snapshot binary first (see the header)" >&2; exit 2; }

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/blygger-browser.XXXXXX")"
SERVER_PID=""
APP_PID=""
cleanup() {
  [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null || true
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
  rm -rf "$SCRATCH"
}
trap cleanup EXIT

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
mkdir -p "$SCRATCH/site" "$SCRATCH/data/browser/lists" "$SCRATCH/config"
cat > "$SCRATCH/site/index.html" <<EOF
<!doctype html>
<html><head><meta charset="utf-8"><title>Tide tables (test page)</title></head>
<body>
<h1>Tide tables</h1>
<p>A test page with a few "ads" from another host.</p>
<div class="ad-slot" style="width:300px;height:80px;background:#fc0">AD SLOT</div>
<img id="ad-img" src="http://127.0.0.1:$PORT/ad.png" alt="">
<script src="http://127.0.0.1:$PORT/ads.js"></script>
</body></html>
EOF
# The same "ads", from localhost, on a page served as 127.0.0.1 (another
# host, whose shield stays on).
cat > "$SCRATCH/site/page2.html" <<EOF
<!doctype html>
<html><head><meta charset="utf-8"><title>Page two (test page)</title></head>
<body>
<h1>Page two</h1>
<div class="ad-slot" style="width:300px;height:80px;background:#fc0">AD SLOT</div>
<img id="ad-img" src="http://localhost:$PORT/ad.png" alt="">
<script src="http://localhost:$PORT/ads.js"></script>
</body></html>
EOF
echo 'window.__adScript = true;' > "$SCRATCH/site/ads.js"
python3 - "$SCRATCH/site/ad.png" <<'EOF'
import struct, sys, zlib
w, h = 4, 4
raw = b"".join(b"\x00" + b"\xff\x00\x00" * w for _ in range(h))
def chunk(t, d):
    c = struct.pack(">I", len(d)) + t + d
    return c + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) \
    + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
EOF
# The "downloaded" list, and a manifest that says it's fresh (no download).
if [ -n "${BLYGGER_REAL_LISTS:-}" ]; then
  cp "$BLYGGER_REAL_LISTS"/*.txt "$SCRATCH/data/browser/lists/"
fi
printf '\n! Test rules\n||127.0.0.1^$third-party\n||localhost^$third-party\n##.ad-slot\n' \
  >> "$SCRATCH/data/browser/lists/easylist.txt"
printf '{"fetched_at": %s}\n' "$(date +%s)" > "$SCRATCH/data/browser/manifest.json"
: > "$SCRATCH/config/config"

(cd "$SCRATCH/site" && exec python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
SERVER_PID=$!
for _ in $(seq 50); do
  curl -fsS "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break
  sleep 0.1
done

LOG="$SCRATCH/app.log"
for run in $(seq "${BLYGGER_RUNS:-1}"); do
[ "$run" -gt 1 ] && echo "-- run $run (same data dir)"
BLYGGER_FAKE=1 BLYGGER_NO_ACTIVATE=1 BLYGGER_TIMING=1 BLYGGER_NO_UPDATE=1 \
  BLYGGER_CONFIG="$SCRATCH/config/config" BLYGGER_DATA_DIR="$SCRATCH/data" \
  BLYGGER_DEMO=br-block BLYGGER_DEMO_URL="http://localhost:$PORT/" \
  BLYGGER_DEMO_URL2="http://127.0.0.1:$PORT/page2.html" \
  BLYGGER_SNAPSHOT="$OUT" "$BIN" >"$LOG" 2>&1 &
APP_PID=$!
for _ in $(seq 600); do
  kill -0 "$APP_PID" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$APP_PID" 2>/dev/null; then
  echo "the app didn't finish in 60 s" >&2
fi
grep -E '^(browser-|snapshot)' "$LOG" || true
APP_PID=""
done

on="$(grep '^browser-probe shield=on ' "$LOG" | tr -d '\\' || true)"
off="$(grep '^browser-probe shield=off ' "$LOG" | tr -d '\\' || true)"
other="$(grep '^browser-probe other-host ' "$LOG" | tr -d '\\' || true)"
fail=0
case "$on" in
  *'"img":0'*'"script":false'*'"slot":"none"'*'"ipc":"undefined"'*) echo "ok: blocked with the shield on" ;;
  *) echo "FAIL: shield on: $on" >&2; fail=1 ;;
esac
case "$off" in
  *'"img":4'*'"script":true'*'"slot":"block"'*) echo "ok: loaded with the shield off" ;;
  *) echo "FAIL: shield off: $off" >&2; fail=1 ;;
esac
case "$other" in
  *'"img":0'*'"script":false'*'"slot":"none"'*'Page two'*) echo "ok: blocked again on another host (shield applied per navigation)" ;;
  *) echo "FAIL: other host: $other" >&2; fail=1 ;;
esac
exit "$fail"
