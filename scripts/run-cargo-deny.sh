#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"
source "${ROOT_DIR}/scripts/quanta-index-env.sh"

if ! command -v cargo-deny >/dev/null 2>&1; then
  echo "cargo-deny is required but not installed." >&2
  echo "Install it with: cargo install cargo-deny --locked" >&2
  exit 1
fi

exec cargo deny --manifest-path "${ROOT_DIR}/Cargo.toml" check advisories bans licenses sources "$@"
