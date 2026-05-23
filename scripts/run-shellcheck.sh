#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

shell_files=()
while IFS= read -r path; do
  shell_files+=("${path}")
done < <(find scripts tools/ci/lint -type f -name '*.sh' | sort)

if [[ "${#shell_files[@]}" -eq 0 ]]; then
  echo "No shell files found."
  exit 0
fi

exec python3 -m pre_commit run shellcheck --files "${shell_files[@]}"
