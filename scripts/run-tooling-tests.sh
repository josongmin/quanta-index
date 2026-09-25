#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"
# The pre-commit hook lints this script alone, without following sourced files.
# shellcheck disable=SC1091
source scripts/quanta-index-env.sh

# Dedicated counterexample gates own these scanners. Keep their focused tests
# out of the broad tooling suite to avoid duplicate work.
python3 -m pytest tools -q \
  --ignore=tools/ci/tests/test_semgrep_policy.py \
  --ignore=tools/ci/tests/test_check_rust_fallbacks.py \
  -o cache_dir="${PYTEST_CACHE_DIR}" "$@"
