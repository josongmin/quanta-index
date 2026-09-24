#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"
# The pre-commit hook lints this script alone, without following sourced files.
# shellcheck disable=SC1091
source scripts/quanta-index-env.sh

# The Semgrep counterexamples are owned by the Semgrep gate, which installs
# and invokes the scanner. Running them here either repeats scans or skips them
# when Semgrep is unavailable.
python3 -m pytest tools -q \
  --ignore=tools/ci/tests/test_semgrep_policy.py \
  -o cache_dir="${PYTEST_CACHE_DIR}" "$@"
