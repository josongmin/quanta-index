#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_PATH="${ROOT_DIR}/tools/ci/semgrep/rules.yml"

cd "${ROOT_DIR}"

if [[ $# -eq 0 ]]; then
  set -- .
else
  # A rule, ignore, or scanner-wrapper change can create findings in files
  # outside this push. Keep those runs repository-wide.
  for path in "$@"; do
    relative_path="${path#"${ROOT_DIR}"/}"
    relative_path="${relative_path#./}"
    case "$relative_path" in
      .semgrepignore | scripts/run-semgrep.sh | tools/ci/semgrep/*)
        set -- .
        break
        ;;
    esac
  done
fi

# --timeout 300: raise the per-file analysis budget above Semgrep's short default so the
# largest production files (query_dispatcher.rs ~5.4k LOC, lexical/lib.rs ~3.4k LOC) are
# fully scanned instead of being silently skipped on timeout, which would hide
# silent-fallback findings in exactly the hottest files.
exec semgrep --config "${CONFIG_PATH}" --error --strict --metrics off --disable-version-check --timeout 300 "$@"
