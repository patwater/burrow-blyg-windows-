#!/usr/bin/env bash
# Point every repo reference at the real GitHub repository, in one go.
#   scripts/set-repo.sh <owner>/<name>
# Rewrites the current value (read from packaging/config.env) in:
#   packaging/config.env, Cargo.toml ([workspace.package] repository/homepage),
#   scripts/install.sh, README.md, crates/blyg-app/src/update/check.rs
# The workflows use github.repository at run time and need no edit. Nothing
# else is compiled into the app binary (update/check.rs names the repo the
# updater asks for releases).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
NEW="${1:?usage: set-repo.sh <owner>/<name>}"
[[ "$NEW" =~ ^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$ ]] || { echo "not an owner/name: $NEW" >&2; exit 2; }
OLD="$(grep -E '^GITHUB_REPO=' packaging/config.env | cut -d= -f2)"
[ "$OLD" != "$NEW" ] || { echo "already $NEW"; exit 0; }
for f in packaging/config.env Cargo.toml scripts/install.sh README.md crates/blyg-app/src/update/check.rs; do
  sed -i '' "s|$OLD|$NEW|g" "$f"
done
echo "replaced $OLD -> $NEW in packaging/config.env Cargo.toml scripts/install.sh README.md update/check.rs"
