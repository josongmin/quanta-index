#!/usr/bin/env bash
set -euo pipefail

show_help() {
  cat <<'EOF'
Usage: bash scripts/check-lock-freshness.sh

Checks:
  - all listed locked Rust workspaces resolve against their lockfiles
  - the current Python project resolves against uv.lock
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  show_help
  exit 0
fi

root="$(git rev-parse --show-toplevel)"
cd "$root"
source ./scripts/rust-workspaces.sh

rust_manifests="$(rust_locked_workspace_manifests)"
lockfiles=(uv.lock)
while IFS= read -r manifest; do
  lockfiles+=("${manifest%Cargo.toml}Cargo.lock")
done <<< "$rust_manifests"

for lockfile in "${lockfiles[@]}"; do
  if [[ ! -f "$lockfile" ]]; then
    echo "Missing required lockfile: $lockfile" >&2
    exit 1
  fi
done

while IFS= read -r manifest; do
  ./scripts/cargow metadata --manifest-path "$manifest" --format-version 1 --locked >/dev/null
done <<< "$rust_manifests"
uv lock --check
