#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_PATH="${ROOT_DIR}/tools/ci/semgrep/rules.yml"
FULL_TARGETS=(.github/workflows)

cd "${ROOT_DIR}"

if [[ $# -eq 0 ]]; then
  set -- "${FULL_TARGETS[@]}"
else
  # A rule, ignore, or scanner-wrapper change can create findings in files
  # outside this push. Rescan the full configured rule scope.
  for path in "$@"; do
    relative_path="${path#"${ROOT_DIR}"/}"
    relative_path="${relative_path#./}"
    case "$relative_path" in
      .semgrepignore | scripts/run-semgrep.sh | tools/ci/semgrep/*)
        set -- "${FULL_TARGETS[@]}"
        break
        ;;
    esac
  done
fi

exec uv run --frozen --extra dev semgrep --config "${CONFIG_PATH}" --error --strict --metrics off --disable-version-check --timeout 300 "$@"
