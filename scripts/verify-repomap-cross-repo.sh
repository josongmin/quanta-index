#!/usr/bin/env bash
set -euo pipefail

# The registered producer alone receives its dedicated signing agent.
paired_signer_socket="${QUANTA_P11_SIGNER_SOCKET:-}"
export -n paired_signer_socket
unset QUANTA_P11_SIGNER_SOCKET
if [[ -n "$paired_signer_socket" && -n "${SSH_AUTH_SOCK:-}" ]] &&
   [[ ! -S "$paired_signer_socket" || ! -S "$SSH_AUTH_SOCK" ||
      "$SSH_AUTH_SOCK" -ef "$paired_signer_socket" ]]; then
  unset SSH_AUTH_SOCK
fi

quanta_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
semantica_root="${1:?Semantica checkout path is required}"
semantica_root="$(cd -- "$semantica_root" && pwd -P)"
provided_binary="${QUANTA_INDEX_SEARCHD_BIN:-}"
r5_lane="${QUANTA_P11_R5_QBC_LANE:?registered QBC lane is required}"
if [[ -n "$provided_binary" && "$provided_binary" != /* ]]; then
  provided_binary="$(pwd -P)/$provided_binary"
fi

require_frozen_source() {
  local root="$1"
  local head="$2"
  local current_head status
  current_head="$(git --no-replace-objects -C "$root" rev-parse --verify HEAD)" || exit 1
  if [[ "$current_head" != "$head" ]]; then
    printf 'source HEAD changed during cross-repo proof: %s\n' "$root" >&2
    exit 1
  fi
  status="$(git --no-replace-objects -C "$root" status --porcelain=v1 --untracked-files=all)" || exit 1
  if [[ -n "$status" ]]; then
    printf 'cross-repo proof requires a clean checkout: %s\n' "$root" >&2
    exit 1
  fi
}

quanta_head="$(git --no-replace-objects -C "$quanta_root" rev-parse --verify HEAD)"
semantica_head="$(git --no-replace-objects -C "$semantica_root" rev-parse --verify HEAD)"
require_frozen_source "$quanta_root" "$quanta_head"
require_frozen_source "$semantica_root" "$semantica_head"

# Resolve Python with the same policy as QBC. Validate the worktree helper and
# source the checked Git bytes so a later worktree replacement cannot run.
paired_python="$(
  helper_relative="scripts/lib/python-env.sh"
  helper="$semantica_root/$helper_relative"
  if [[ -L "$semantica_root/scripts" || -L "$semantica_root/scripts/lib" ||
        -L "$helper" || ! -f "$helper" ]]; then
    printf 'paired Python helper is not a physical regular file: %s\n' "$helper" >&2
    exit 1
  fi
  entry="$(git --no-replace-objects -C "$semantica_root" ls-tree --full-tree \
    "$semantica_head" -- "$helper_relative")" || exit 1
  IFS=$' \t' read -r mode type blob path <<< "$entry"
  if [[ "$type" != blob || "$path" != "$helper_relative" ||
        ( "$mode" != 100644 && "$mode" != 100755 ) ]]; then
    printf 'paired Python helper is not a regular file in frozen HEAD: %s\n' "$helper" >&2
    exit 1
  fi
  helper_snapshot="$(mktemp "${TMPDIR:-/tmp}/quanta-paired-python-XXXXXX")" || exit 1
  trap 'rm -f -- "$helper_snapshot"' EXIT
  git --no-replace-objects -C "$semantica_root" cat-file blob "$blob" > "$helper_snapshot" || exit 1
  if ! cmp -s -- "$helper" "$helper_snapshot"; then
    printf 'paired Python helper differs from the frozen Git blob: %s\n' "$helper" >&2
    exit 1
  fi
  # shellcheck source=/dev/null
  source "$helper_snapshot" || exit 1
  cg_resolve_python_bin "$semantica_root"
)"

resolved_pair() {
  local consumer="$1"
  local feature="$2"
  "$paired_python" "$quanta_root/tools/ci/paired_cargo_resolution.py" \
    --quanta-root "$quanta_root" --paired-root "$semantica_root" \
    --consumer "$consumer" --feature "$feature" --qbc-lane "$r5_lane"
}

# Check the actual Cargo resolver before the expensive build, not the spelling
# of declared relative paths. Both selected feature profiles must use this
# exact Quanta checkout and the nested workspace's actual lockfile.
paired_consumers="$("$paired_python" -I -c '
import sys
from pathlib import Path

sys.path.insert(0, sys.argv[1])
from tools.ci.daemon_execution_evidence import R5_PROOF, live_proof, r5_cases

for _, package, features, _, _, _ in r5_cases(live_proof(Path(sys.argv[1]), R5_PROOF)):
    print(package, features, sep="\t")
' "$quanta_root")"
IFS=$'\t' read -r runtime_package runtime_features <<< "${paired_consumers%%$'\n'*}"
IFS=$'\t' read -r kernel_package kernel_features <<< "${paired_consumers#*$'\n'}"
runtime_resolution="$(resolved_pair "$runtime_package" "$runtime_features")"
kernel_resolution="$(resolved_pair "$kernel_package" "$kernel_features")"
require_frozen_source "$quanta_root" "$quanta_head"
require_frozen_source "$semantica_root" "$semantica_head"

release_binary="$(PYTHONPATH="$quanta_root${PYTHONPATH:+:$PYTHONPATH}" "$paired_python" "$quanta_root/tools/ci/paired_r5_result.py" \
  --verify-resolution-only --quanta-root "$quanta_root" --semantica-root "$semantica_root" \
  --runtime-resolution "$runtime_resolution" --kernel-resolution "$kernel_resolution")"
require_frozen_source "$quanta_root" "$quanta_head"
require_frozen_source "$semantica_root" "$semantica_head"

cd -- "$quanta_root"
pin_archive_option=""
verify_archive_option=""
if [[ -n "$release_binary" ]]; then
  # The formal path uses the exact P10 content archive. Execute only its private copy.
  built_binary="$release_binary"
  provided_binary="${provided_binary:-$release_binary}"
  pin_archive_option="--archive-sha256=${release_binary##*/}"
  verify_archive_option="--archive=$release_binary"
elif [[ -n "${QUANTA_RETIREMENT_DAEMON_BUILD:-}${QUANTA_RETIREMENT_DAEMON_RECEIPT_SHA256:-}" ]]; then
  selection="$("$paired_python" "$quanta_root/tools/ci/check-tantivy-retirement.py" \
    --select-captured-daemon \
    --release-build "${QUANTA_RETIREMENT_DAEMON_BUILD:?recorded daemon build directory is required}" \
    --expected-build-receipt-sha256 "${QUANTA_RETIREMENT_DAEMON_RECEIPT_SHA256:?recorded build receipt digest is required}")"
  selected_digest="${selection%%$'\n'*}"
  built_binary="${selection#*$'\n'}"
  if [[ ! "$selected_digest" =~ ^[0-9a-f]{64}$ || "$built_binary" != /* || "$built_binary" == *$'\n'* ]]; then
    printf 'Recorded Native release selector did not return a daemon digest and path\n' >&2
    exit 1
  fi
  provided_binary="${provided_binary:-$built_binary}"
  pin_archive_option="--archive-sha256=$selected_digest"
else
  : "${provided_binary:?QUANTA_INDEX_SEARCHD_BIN is required for staged R5}"
  native_build="$(just rust-build-release-native-daemon)"
  printf '%s\n' "$native_build"
  built_binary="$(sed -n 's/^Verified Native daemon //p' <<< "$native_build")"
  if [[ "$built_binary" != /* || "$built_binary" == *$'\n'* ]]; then
    printf 'Native release producer did not select one absolute daemon path\n' >&2
    exit 1
  fi
fi
custody_directory="$(mktemp -d)"
custody_directory="$(cd -- "$custody_directory" && pwd -P)"
custody_binary="$custody_directory/quanta-index-searchd"
cleanup_custody() {
  rm -f -- "$custody_binary"
  rmdir -- "$custody_directory"
}
trap cleanup_custody EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
binary_digest="$("$paired_python" "$quanta_root/tools/ci/binary_custody.py" pin \
  "$built_binary" "$provided_binary" "$custody_binary" ${pin_archive_option:+"$pin_archive_option"})"
require_binary_custody() {
  "$paired_python" "$quanta_root/tools/ci/binary_custody.py" verify "$binary_digest" \
    "$built_binary" "$provided_binary" "$custody_binary" ${verify_archive_option:+"$verify_archive_option"}
}
require_frozen_source "$quanta_root" "$quanta_head"
require_frozen_source "$semantica_root" "$semantica_head"

cd -- "$semantica_root"
if [[ -n "${QUANTA_P11_R5_EVIDENCE_ROOT:-}" ]]; then
  evidence_root="$QUANTA_P11_R5_EVIDENCE_ROOT"
else
  temporary_root="$(cd -- "${TMPDIR:-/tmp}" && pwd -P)"
  evidence_parent="$(mktemp -d "$temporary_root/quanta-paired-r5-XXXXXX")"
  evidence_root="$evidence_parent/result"
fi
# The typed runner owns the final resolver, source and daemon checks before signing.
require_binary_custody
QUANTA_P11_SIGNER_SOCKET="$paired_signer_socket" \
PYTHONPATH="$quanta_root${PYTHONPATH:+:$PYTHONPATH}" "$paired_python" "$quanta_root/tools/ci/paired_r5_result.py" \
  --quanta-root "$quanta_root" --semantica-root "$semantica_root" \
  --evidence-root "$evidence_root" --qbc-lane "$r5_lane" \
  --quanta-head "$quanta_head" --semantica-head "$semantica_head" \
  --daemon-digest "$binary_digest" --built-binary "$built_binary" \
  --provided-binary "$provided_binary" --custody-binary "$custody_binary" \
  --runtime-resolution "$runtime_resolution" --kernel-resolution "$kernel_resolution"
printf 'paired-daemon-sha256: %s\n' "$binary_digest"
printf 'paired-cargo-resolution-runtime: %s\n' "$runtime_resolution"
printf 'paired-cargo-resolution-kernel: %s\n' "$kernel_resolution"
