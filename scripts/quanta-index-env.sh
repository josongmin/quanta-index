#!/usr/bin/env bash
# Shared local cache layout for quanta-index build and runtime artifacts.
#
# macOS:  ~/Library/Caches/quanta-index/
# Linux:  ${XDG_CACHE_HOME:-~/.cache}/quanta-index/
#
# Override the root with QUANTA_INDEX_CACHE_ROOT when needed.

quanta_index_cache_root() {
  if [[ -n "${QUANTA_INDEX_CACHE_ROOT:-}" ]]; then
    printf '%s\n' "$QUANTA_INDEX_CACHE_ROOT"
    return 0
  fi

  if [[ "$(uname -s)" == "Darwin" ]]; then
    printf '%s\n' "${HOME}/Library/Caches/quanta-index"
    return 0
  fi

  printf '%s\n' "${XDG_CACHE_HOME:-${HOME}/.cache}/quanta-index"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  quanta_index_cache_root
  exit 0
fi

_QUANTA_INDEX_CACHE_ROOT="$(quanta_index_cache_root)"
export QUANTA_INDEX_CACHE_ROOT="$_QUANTA_INDEX_CACHE_ROOT"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$_QUANTA_INDEX_CACHE_ROOT/target}"
export QUANTA_INDEX_STATE_ROOT="${QUANTA_INDEX_STATE_ROOT:-$_QUANTA_INDEX_CACHE_ROOT/state}"
export PYTEST_CACHE_DIR="${PYTEST_CACHE_DIR:-$_QUANTA_INDEX_CACHE_ROOT/pytest}"
export RUFF_CACHE_DIR="${RUFF_CACHE_DIR:-$_QUANTA_INDEX_CACHE_ROOT/ruff}"

unset _QUANTA_INDEX_CACHE_ROOT
