#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"
# The changed-file hook may lint this wrapper alone.
# shellcheck disable=SC1091
source "${ROOT_DIR}/scripts/quanta-index-env.sh"

if ! command -v cargo-deny >/dev/null 2>&1; then
  echo "cargo-deny is required but not installed." >&2
  echo "Install it with: cargo install cargo-deny --locked" >&2
  exit 1
fi

manifests=Cargo.toml
if [[ "${1:-}" == --all-workspaces ]]; then
  shift
  # shellcheck disable=SC1091
  source "${ROOT_DIR}/scripts/rust-workspaces.sh"
  manifests="$(rust_locked_workspace_manifests)"
  if [[ -z "$manifests" ]]; then
    echo "No locked Rust workspace manifests found." >&2
    exit 1
  fi
fi

deny_status=0
while IFS= read -r manifest; do
  checks=(advisories licenses sources)
  # Root bans include exact feature and duplicate-version rules for the root graph.
  if [[ "$manifest" == Cargo.toml ]]; then
    checks+=(bans)
  fi
  if cargo deny --manifest-path "${ROOT_DIR}/${manifest}" --locked --workspace \
    check --config "${ROOT_DIR}/deny.toml" "${checks[@]}" "$@" </dev/null; then
    continue
  else
    manifest_status=$?
    printf 'cargo-deny failed for %s (exit %d)\n' "$manifest" "$manifest_status" >&2
    if (( deny_status == 0 )); then
      deny_status=$manifest_status
    fi
  fi
done <<< "$manifests"
exit "$deny_status"
