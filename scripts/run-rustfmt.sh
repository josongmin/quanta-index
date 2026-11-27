#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
source ./scripts/rust-workspaces.sh

mode="${1:-}"
case "$mode" in
  check|write) ;;
  *) echo "unknown format mode: $mode" >&2; exit 2 ;;
esac

status=0
format_workspace() {
  if [[ "$mode" == check ]]; then
    ./scripts/cargow --lane fmt-lane fmt "$@" -- --check || status=1
  else
    ./scripts/cargow --lane fmt-lane fmt "$@" || status=1
  fi
}

# cargo fmt cannot select a virtual workspace root via --manifest-path.
manifests="$(rust_locked_workspace_manifests)"
while IFS= read -r manifest; do
  if [[ "$manifest" == Cargo.toml ]]; then
    format_workspace
  else
    format_workspace --manifest-path "$manifest"
  fi
done <<< "$manifests"
exit "$status"
