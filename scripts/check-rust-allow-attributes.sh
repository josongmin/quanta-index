#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

set +e
rg -n '^\s*#!?\[allow\(' crates benchmarks --glob '*.rs'
rg_status=$?
set -e
if [[ ${rg_status} -eq 0 ]]; then
  echo "Rust #[allow] attributes are banned. Use #[expect(..., reason = ...)] only through an approved exception." >&2
  exit 1
fi
if [[ ${rg_status} -ne 1 ]]; then
  echo "Rust #[allow] scan failed (rg exit ${rg_status})." >&2
  exit "${rg_status}"
fi
