#!/usr/bin/env bash
set -euo pipefail

show_help() {
  cat <<'EOF'
Usage: bash tools/ci/lint/lint-root-hygiene.sh

Checks:
  - fail if git index tracks local build/runtime/cache artifacts
  - fail if any repo-local target directory exists
  - fail on broken root residue like "~" or ".codex-tmp-target"
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  show_help
  exit 0
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$repo_root"

fail=0

if ! git ls-files -z | python3 -c '
from __future__ import annotations

import re
import sys

pattern = re.compile(
    rb"(^|/)(target|\.pytest_cache|\.ruff_cache|\.mypy_cache|__pycache__|state)($|/)|"
    rb"(^|/)\.codex-tmp-target($|/)|"
    rb"(^|/)~($|/)|"
    rb"\.sock$|"
    rb"\.db(-.*)?$|"
    rb"\.sqlite3$"
)

bad: list[str] = []
for record in sys.stdin.buffer.read().split(b"\0"):
    if not record:
        continue
    if pattern.search(record):
        bad.append(record.decode("utf-8", errors="replace"))

if bad:
    print("Tracked local artifact paths are forbidden:", file=sys.stderr)
    for path in sorted(bad):
        print(f"  - {path}", file=sys.stderr)
    sys.exit(1)
'
then
  fail=1
fi

for forbidden in target .codex-tmp-target '~'; do
  if [[ -e "$forbidden" ]]; then
    echo "Forbidden root artifact present: $forbidden" >&2
    fail=1
  fi
done

while IFS= read -r path; do
  [[ -n "$path" ]] || continue
  echo "Forbidden nested target directory present: ${path#./}" >&2
  fail=1
done < <(find . -path ./.git -prune -o -type d -name target -print)

if [[ "$fail" -ne 0 ]]; then
  cat <<'EOF' >&2

Root hygiene check failed.
Source scripts/quanta-index-env.sh (or use `just`) so build caches stay outside the repo.
Remove accidental repo-local artifacts before committing.
EOF
  exit 1
fi

echo "Root hygiene check passed."
