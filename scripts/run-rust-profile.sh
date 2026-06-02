#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if [[ "$#" -ne 2 ]]; then
  printf 'usage: %s <profile> <delegated-recipe>\n' "$0" >&2
  exit 2
fi

profile="$1"
delegated_recipe="$2"

# shellcheck disable=SC1091
source "${SCRIPT_DIR}/quanta-index-env.sh"

# Profile logging needs the cache/state roots, but the delegated recipe must own
# its lane-specific target dir. Otherwise nested `scripts/cargow --lane ...`
# invocations inherit the ambient shared target and silently collapse distinct
# release/test lanes into one cache root.
unset CARGO_TARGET_DIR

log_profile_event() {
  local exit_code="$1"
  local duration_ms="$2"

  if [[ "${QUANTA_INDEX_BUILD_LOGGING:-1}" == "0" ]]; then
    return 0
  fi

  if ! python3 "${SCRIPT_DIR}/../tools/ci/timing/rust_profile_history.py" \
      append-profile \
      "$profile" \
      "$delegated_recipe" \
      "$exit_code" \
      "$duration_ms"; then
    printf 'warning: rust profile log append failed for profile=%s\n' "$profile" >&2
  fi
}

start_ms="$(python3 -c 'import time; print(int(time.time() * 1000))')"

if just "$delegated_recipe"; then
  status=0
else
  status="$?"
fi

end_ms="$(python3 -c 'import time; print(int(time.time() * 1000))')"
duration_ms="$((end_ms - start_ms))"

log_profile_event "$status" "$duration_ms"
exit "$status"
