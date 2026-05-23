#!/usr/bin/env bash
set -euo pipefail

show_help() {
  cat <<'EOF'
Usage: bash scripts/check-lock-freshness.sh

Checks:
  - staged Cargo.toml changes still resolve against Cargo.lock
  - staged pyproject.toml changes do not leave obvious tooling drift

Notes:
  - This repo currently has no Python lockfile.
  - Python dependency edits are gated by installable metadata and CI.
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  show_help
  exit 0
fi

root="$(git rev-parse --show-toplevel)"
cd "$root"

staged="$(git diff --cached --name-only)"

if grep -q '^Cargo\.toml$' <<<"$staged" || grep -q '^crates/.*/Cargo\.toml$' <<<"$staged"; then
  cargo metadata --format-version 1 --locked >/dev/null
fi

if grep -q '^pyproject\.toml$' <<<"$staged"; then
  python3 - <<'PY'
from pathlib import Path
import tomllib

with Path("pyproject.toml").open("rb") as fh:
    tomllib.load(fh)
PY
fi
