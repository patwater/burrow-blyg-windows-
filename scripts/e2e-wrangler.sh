#!/usr/bin/env bash
# Start and stop local `wrangler dev` instances of a blyg Worker for the
# end-to-end tests. Used by scripts/e2e-local.sh, and by the e2e tests
# themselves (through BLYG_E2E_CTL) to take the Worker down mid-session.
#
#   e2e-wrangler.sh setup                 derive the scratch config + secrets
#   e2e-wrangler.sh start <name> <port>   start instance <name> (own D1/R2 state)
#   e2e-wrangler.sh stop <name>           stop only the processes this script started
#   e2e-wrangler.sh stop-all
#
# Needs BLYG_WORKER_DIR (the Worker's directory, with wrangler.jsonc and
# node_modules) and BLYG_E2E_SCRATCH (a scratch dir this script owns).
#
# Safety: always local. The derived config drops everything that would reach
# Cloudflare (routes, custom domains, the Workers AI binding, observability);
# this script never passes --remote and never deploys. State lives under
# $BLYG_E2E_SCRATCH/<name>/ via --persist-to, never in the Worker directory.
set -euo pipefail

die() { printf 'e2e-wrangler: %s\n' "$*" >&2; exit 1; }
: "${BLYG_WORKER_DIR:?set BLYG_WORKER_DIR to the blyg Worker directory}"
: "${BLYG_E2E_SCRATCH:?set BLYG_E2E_SCRATCH to a scratch directory}"

WRANGLER="$BLYG_WORKER_DIR/node_modules/.bin/wrangler"
CONF="$BLYG_E2E_SCRATCH/wrangler.e2e.json"
export CI=1                       # never prompt
export WRANGLER_SEND_METRICS=false
export NO_D1_WARNING=true

cmd="${1:-}"; shift || true

setup() {
  [ -x "$WRANGLER" ] || die "no wrangler at $WRANGLER (run npm install in the Worker dir)"
  local src
  for src in wrangler.jsonc wrangler.json; do
    [ -f "$BLYG_WORKER_DIR/$src" ] && break
  done
  [ -f "$BLYG_WORKER_DIR/$src" ] || die "no wrangler.jsonc in $BLYG_WORKER_DIR"
  mkdir -p "$BLYG_E2E_SCRATCH"
  # A local-only copy of the Worker's config: JSONC → JSON, absolute paths,
  # remote-only settings removed.
  node - "$BLYG_WORKER_DIR" "$src" "$CONF" <<'JS'
const fs = require("fs"), path = require("path");
const [dir, src, out] = process.argv.slice(2);
const text = fs.readFileSync(path.join(dir, src), "utf8");
// Strip // and /* */ comments outside strings, then trailing commas.
let s = "", i = 0, str = false;
while (i < text.length) {
  const c = text[i], n = text[i + 1];
  if (str) { s += c; if (c === "\\") { s += n; i += 2; continue; } if (c === '"') str = false; i++; continue; }
  if (c === '"') { str = true; s += c; i++; continue; }
  if (c === "/" && n === "/") { while (i < text.length && text[i] !== "\n") i++; continue; }
  if (c === "/" && n === "*") { i = text.indexOf("*/", i + 2) + 2; continue; }
  s += c; i++;
}
const cfg = JSON.parse(s.replace(/,(\s*[}\]])/g, "$1"));
const keep = {
  name: "blygger-e2e",
  main: path.resolve(dir, cfg.main),
  compatibility_date: cfg.compatibility_date,
};
if (cfg.compatibility_flags) keep.compatibility_flags = cfg.compatibility_flags;
if (cfg.vars) keep.vars = cfg.vars;
if (cfg.triggers) keep.triggers = cfg.triggers;
if (cfg.d1_databases) keep.d1_databases = cfg.d1_databases.map((d) => ({
  binding: d.binding, database_name: d.database_name, database_id: "00000000-0000-0000-0000-000000000000",
  migrations_dir: path.resolve(dir, d.migrations_dir || "migrations"),
}));
if (cfg.r2_buckets) keep.r2_buckets = cfg.r2_buckets.map((b) => ({ binding: b.binding, bucket_name: b.bucket_name }));
if (cfg.kv_namespaces) keep.kv_namespaces = cfg.kv_namespaces.map((k) => ({ binding: k.binding, id: "local" }));
fs.writeFileSync(out, JSON.stringify(keep, null, 2));
JS
  # Fresh random dev secrets next to the derived config (wrangler reads
  # .dev.vars from the config's directory). Never real values.
  if [ ! -f "$BLYG_E2E_SCRATCH/.dev.vars" ]; then
    local rnd; rnd() { LC_ALL=C tr -dc 'A-Za-z0-9' </dev/urandom | head -c "$1"; }
    {
      printf 'OWNER_PASSWORD=%s\n' "$(rnd 24)"
      printf 'COOKIE_SECRET=%s\n' "$(rnd 48)"
      printf 'BLYG_OWNER_TOKEN=%s\n' "$(rnd 40)"
    } >"$BLYG_E2E_SCRATCH/.dev.vars"
    chmod 600 "$BLYG_E2E_SCRATCH/.dev.vars"
  fi
}

token() { sed -n 's/^BLYG_OWNER_TOKEN=//p' "$BLYG_E2E_SCRATCH/.dev.vars"; }

free_port() { node -e 'const s=require("net").createServer().listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})'; }

start() {
  local name="${1:?name}" port="${2:?port}"
  local state="$BLYG_E2E_SCRATCH/$name"
  mkdir -p "$state"
  [ -f "$CONF" ] || setup
  # Idempotent: applies only what's new (the Worker may have moved on).
  (cd "$BLYG_WORKER_DIR" && "$WRANGLER" d1 migrations apply DB --local \
    --persist-to "$state/d1" -c "$CONF") >"$state/migrate.log" 2>&1 \
    || { cat "$state/migrate.log" >&2; die "migrations failed for $name"; }
  local inspector; inspector="$(free_port)"
  # Own process group (set -m), so stop can signal exactly what we started.
  set -m
  (cd "$BLYG_WORKER_DIR" && exec "$WRANGLER" dev --local --ip 127.0.0.1 --port "$port" \
    --inspector-port "$inspector" --persist-to "$state/d1" -c "$CONF" \
    --test-scheduled --show-interactive-dev-session=false) >>"$state/wrangler.log" 2>&1 &
  local pid=$!
  set +m
  echo "$pid" >"$state/pid"
  echo "$port" >"$state/port"
  local t=0
  until curl -fsS -o /dev/null "http://127.0.0.1:$port/blyg.json" 2>/dev/null; do
    kill -0 "$pid" 2>/dev/null || { tail -40 "$state/wrangler.log" >&2; die "wrangler ($name) exited"; }
    t=$((t + 1)); [ "$t" -lt 240 ] || { tail -40 "$state/wrangler.log" >&2; die "wrangler ($name) not ready"; }
    sleep 0.5
  done
}

stop() {
  local name="${1:?name}" state="$BLYG_E2E_SCRATCH/${1:-}"
  [ -f "$state/pid" ] || return 0
  local pid; pid="$(cat "$state/pid")"
  rm -f "$state/pid"
  # The whole process group we created (wrangler + its workerd child), and nothing else.
  kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
  local t=0
  while kill -0 -- "-$pid" 2>/dev/null; do
    t=$((t + 1))
    if [ "$t" -gt 40 ]; then kill -KILL -- "-$pid" 2>/dev/null || true; break; fi
    sleep 0.25
  done
  # Wait until the port is really closed.
  local port; port="$(cat "$state/port" 2>/dev/null || echo)"
  if [ -n "$port" ]; then
    t=0
    while curl -fsS -o /dev/null "http://127.0.0.1:$port/blyg.json" 2>/dev/null && [ "$t" -lt 40 ]; do
      t=$((t + 1)); sleep 0.25
    done
  fi
}

case "$cmd" in
  setup) setup ;;
  token) token ;;
  free-port) free_port ;;
  start) start "$@" ;;
  stop) stop "$@" ;;
  stop-all)
    for d in "$BLYG_E2E_SCRATCH"/*/; do [ -f "$d/pid" ] && stop "$(basename "$d")"; done; true ;;
  *) die "usage: $0 setup|token|free-port|start <name> <port>|stop <name>|stop-all" ;;
esac
