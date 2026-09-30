#!/usr/bin/env bash
# End-to-end tests of blyg-core against a REAL blyg Worker running locally.
#
#   BLYG_WORKER_DIR=/path/to/worker scripts/e2e-local.sh [extra test args]
#
# BLYG_WORKER_DIR is the Worker's directory (the one with wrangler.jsonc and
# node_modules; run `npm install` there first). The Worker must carry the
# owner-API extensions in docs/SERVER.md.
#
# What it does:
#   1. derives a local-only wrangler config and random dev secrets in a
#      scratch dir (nothing is written to the Worker directory);
#   2. starts two `wrangler dev --local` instances on free ports, each with
#      its own scratch D1/R2 state (--persist-to): `a` is the blyg under test,
#      `b` a second blyg to subscribe to;
#   3. runs `cargo test -p blyg-core --test e2e -- --ignored --test-threads=1`;
#   4. stops only the wrangler processes it started, and deletes the scratch
#      dir (set BLYG_E2E_KEEP=1 to keep it, e.g. to read the wrangler logs).
#
# It never uses --remote, never deploys, and never talks to a real blyg.
set -euo pipefail

die() { printf 'e2e-local: %s\n' "$*" >&2; exit 1; }
: "${BLYG_WORKER_DIR:?set BLYG_WORKER_DIR to the blyg Worker directory}"
[ -d "$BLYG_WORKER_DIR" ] || die "no such directory: $BLYG_WORKER_DIR"
BLYG_WORKER_DIR="$(cd "$BLYG_WORKER_DIR" && pwd)"
export BLYG_WORKER_DIR

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CTL="$ROOT/scripts/e2e-wrangler.sh"
BLYG_E2E_SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/blygger-e2e.XXXXXX")"
export BLYG_E2E_SCRATCH

cleanup() {
  "$CTL" stop-all || true
  if [ -z "${BLYG_E2E_KEEP:-}" ]; then
    rm -rf "$BLYG_E2E_SCRATCH"
  else
    printf 'e2e-local: kept %s\n' "$BLYG_E2E_SCRATCH" >&2
  fi
}
trap cleanup EXIT INT TERM

"$CTL" setup
PORT_A="$("$CTL" free-port)"
PORT_B="$("$CTL" free-port)"
[ "$PORT_A" != "$PORT_B" ] || PORT_B="$("$CTL" free-port)"
printf 'e2e-local: starting the Worker on :%s and :%s…\n' "$PORT_A" "$PORT_B" >&2
"$CTL" start a "$PORT_A"
"$CTL" start b "$PORT_B"

export BLYG_E2E_URL="http://127.0.0.1:$PORT_A"
export BLYG_E2E_URL_B="http://127.0.0.1:$PORT_B"
BLYG_E2E_TOKEN="$("$CTL" token)"
export BLYG_E2E_TOKEN
export BLYG_E2E_CTL="$CTL"

cd "$ROOT"
cargo test -p blyg-core --test e2e -- --ignored --test-threads=1 "$@"
