#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_PATH="${ROOT_DIR}/tools/ci/semgrep/rules.yml"

cd "${ROOT_DIR}"

if [[ $# -eq 0 ]]; then
  set -- .
fi

# --timeout 300: raise the per-file analysis budget from semgrep's 30s default so the
# largest production files (query_dispatcher.rs ~5.4k LOC, lexical/lib.rs ~3.4k LOC) are
# fully scanned instead of being silently skipped on timeout, which would hide
# silent-fallback findings in exactly the hottest files.
exec semgrep --config "${CONFIG_PATH}" --error --timeout 300 "$@"
