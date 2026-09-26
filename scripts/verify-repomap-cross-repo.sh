#!/usr/bin/env bash
set -euo pipefail

quanta_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
semantica_root="${1:?Semantica checkout path is required}"
semantica_root="$(cd -- "$semantica_root" && pwd -P)"
provided_binary="${QUANTA_INDEX_SEARCHD_BIN:?QUANTA_INDEX_SEARCHD_BIN is required}"

require_frozen_source() {
  local root="$1"
  local head="$2"
  if [[ "$(git -C "$root" rev-parse HEAD)" != "$head" ]]; then
    printf 'source HEAD changed during cross-repo proof: %s\n' "$root" >&2
    exit 1
  fi
  if [[ -n "$(git -C "$root" status --porcelain=v1 --untracked-files=all)" ]]; then
    printf 'cross-repo proof requires a clean checkout: %s\n' "$root" >&2
    exit 1
  fi
}

quanta_head="$(git -C "$quanta_root" rev-parse HEAD)"
semantica_head="$(git -C "$semantica_root" rev-parse HEAD)"
require_frozen_source "$quanta_root" "$quanta_head"
require_frozen_source "$semantica_root" "$semantica_head"

resolved_pair() {
  local consumer="$1"
  local feature="$2"
  (
    cd -- "$semantica_root"
    CODEGRAPH_PERSONA=agent ./scripts/quanta-build-cli cargo --lane local -- metadata \
      --locked --format-version 1 --no-default-features \
      --manifest-path "packages/analysis/quanta-v2/crates/$consumer/Cargo.toml" \
      --features "$feature"
  ) | python3 "$quanta_root/tools/ci/paired_cargo_resolution.py" \
    --quanta-root "$quanta_root" --paired-root "$semantica_root" --consumer "$consumer"
}

# Check the actual Cargo resolver before the expensive build, not the spelling
# of declared relative paths. Both selected feature profiles must use this
# exact Quanta checkout and the nested workspace's actual lockfile.
runtime_resolution="$(resolved_pair quanta-runtime index-sdk-ingress)"
kernel_resolution="$(resolved_pair quanta-runtime-retrieval-kernel index-sdk-ingress-surface)"
require_frozen_source "$quanta_root" "$quanta_head"
require_frozen_source "$semantica_root" "$semantica_head"

cd -- "$quanta_root"
just rust-build-release-daemon-fresh
target_dir="$(./scripts/cargow --lane release-daemon-bin-lane metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
built_binary="$target_dir/release/quanta-index-searchd"
if [[ ! -x "$built_binary" || ! -x "$provided_binary" ]]; then
  printf 'fresh release daemon and QUANTA_INDEX_SEARCHD_BIN must both be executable\n' >&2
  exit 1
fi
if ! cmp -s -- "$built_binary" "$provided_binary"; then
  printf 'QUANTA_INDEX_SEARCHD_BIN differs from the fresh release daemon\n' >&2
  exit 1
fi
require_frozen_source "$quanta_root" "$quanta_head"
require_frozen_source "$semantica_root" "$semantica_head"

cd -- "$semantica_root"
CODEGRAPH_PERSONA=agent ./scripts/quanta-build-cli cargo --lane local -- test \
  --locked \
  --manifest-path packages/analysis/quanta-v2/Cargo.toml -p quanta-runtime \
  --no-default-features --features index-sdk-ingress \
  --test index_sdk_ingress_publish_contract_test -- --list \
  | rg '^index_sdk_ingress_live_repomap_roundtrip_survives_runtime_restart_v1: test$'
QUANTA_INDEX_SEARCHD_BIN="$built_binary" CODEGRAPH_PERSONA=agent \
  ./scripts/quanta-build-cli cargo --lane local -- test \
  --locked \
  --manifest-path packages/analysis/quanta-v2/Cargo.toml -p quanta-runtime \
  --no-default-features --features index-sdk-ingress \
  --test index_sdk_ingress_publish_contract_test \
  index_sdk_ingress_live_repomap_roundtrip_survives_runtime_restart_v1 \
  -- --exact --nocapture

CODEGRAPH_PERSONA=agent ./scripts/quanta-build-cli cargo --lane local -- test \
  --locked \
  --manifest-path packages/analysis/quanta-v2/Cargo.toml \
  -p quanta-runtime-retrieval-kernel --no-default-features \
  --features index-sdk-ingress-surface --lib -- --list \
  | rg '^index_sdk_ingress::terminal_receipt_v1::tests::repomap_v2_receipts_require_exact_full_bundle_and_transition_v2: test$'
CODEGRAPH_PERSONA=agent ./scripts/quanta-build-cli cargo --lane local -- test \
  --locked \
  --manifest-path packages/analysis/quanta-v2/Cargo.toml \
  -p quanta-runtime-retrieval-kernel --no-default-features \
  --features index-sdk-ingress-surface --lib \
  index_sdk_ingress::terminal_receipt_v1::tests::repomap_v2_receipts_require_exact_full_bundle_and_transition_v2 \
  -- --exact --nocapture

require_frozen_source "$quanta_root" "$quanta_head"
require_frozen_source "$semantica_root" "$semantica_head"
if ! cmp -s -- "$built_binary" "$provided_binary"; then
  printf 'release daemon bytes changed during cross-repo proof\n' >&2
  exit 1
fi
runtime_resolution_after="$(resolved_pair quanta-runtime index-sdk-ingress)"
kernel_resolution_after="$(resolved_pair quanta-runtime-retrieval-kernel index-sdk-ingress-surface)"
if [[ "$runtime_resolution_after" != "$runtime_resolution" || "$kernel_resolution_after" != "$kernel_resolution" ]]; then
  printf 'resolved cross-repo dependency identities changed during proof\n' >&2
  exit 1
fi
require_frozen_source "$quanta_root" "$quanta_head"
require_frozen_source "$semantica_root" "$semantica_head"
printf 'paired-cargo-resolution-runtime: %s\n' "$runtime_resolution"
printf 'paired-cargo-resolution-kernel: %s\n' "$kernel_resolution"
