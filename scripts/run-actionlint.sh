#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

workflow_files=()
for path in .github/workflows/*.yml .github/workflows/*.yaml; do
  if [[ -f "${path}" ]]; then
    workflow_files+=("${path}")
  fi
done

if [[ "${#workflow_files[@]}" -eq 0 ]]; then
  echo "No workflow files found." >&2
  exit 1
fi

exec uv run --frozen --extra dev python tools/ci/run-read-only-hook.py actionlint "${workflow_files[@]}"
