#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

workflow_files=()
for path in .github/workflows/*.yml .github/workflows/*.yaml; do
  if [[ -e "${path}" ]]; then
    workflow_files+=("${path}")
  fi
done

if [[ "${#workflow_files[@]}" -eq 0 ]]; then
  echo "No workflow files found."
  exit 0
fi

exec python3 -m pre_commit run actionlint --files "${workflow_files[@]}"
