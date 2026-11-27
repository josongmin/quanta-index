#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

if [[ ! -f scripts/cargow ]]; then
  echo "Required shell wrapper missing: scripts/cargow" >&2
  exit 1
fi
if [[ ! -x scripts/cargow ]]; then
  echo "scripts/cargow is not executable" >&2
  exit 1
fi

shell_files=(scripts/cargow)
shell_inventory="$(mktemp "${TMPDIR:-/tmp}/quanta-shell-lint.XXXXXX")"
trap 'rm -f -- "$shell_inventory"' EXIT
if ! find scripts tools/ci/lint -type f -name '*.sh' | sort > "$shell_inventory"; then
  echo "Shell source inventory failed." >&2
  exit 1
fi
while IFS= read -r path; do
  shell_files+=("${path}")
done < "$shell_inventory"

uv run --frozen --extra dev python tools/ci/run-read-only-hook.py shellcheck "${shell_files[@]}"
