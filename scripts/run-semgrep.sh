#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_PATH="${ROOT_DIR}/tools/ci/semgrep/rules.yml"

cd "${ROOT_DIR}"

if [[ $# -eq 0 ]]; then
  set -- .
fi

exec semgrep --config "${CONFIG_PATH}" --error "$@"
