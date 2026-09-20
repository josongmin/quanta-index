#!/usr/bin/env bash
# Shared local cache layout for quanta-index build and runtime artifacts.
#
# macOS:  ~/Library/Caches/quanta-index/
# Linux:  ${XDG_CACHE_HOME:-~/.cache}/quanta-index/
#
# Override the root with QUANTA_INDEX_CACHE_ROOT when needed.
# Override the compile lane with QUANTA_INDEX_BUILD_LANE when a command should
# use an isolated incremental/cache root.

if [[ -n "${ZSH_VERSION:-}" ]]; then
  _QUANTA_INDEX_ENV_SOURCE="${0}"
else
  _QUANTA_INDEX_ENV_SOURCE="${BASH_SOURCE[0]}"
fi

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

if [[ -z "${ZSH_VERSION:-}" && "$_QUANTA_INDEX_ENV_SOURCE" == "${0}" ]]; then
  quanta_index_cache_root
  exit 0
fi

_QUANTA_INDEX_CACHE_ROOT="$(quanta_index_cache_root)"
export QUANTA_INDEX_CACHE_ROOT="$_QUANTA_INDEX_CACHE_ROOT"
_QUANTA_INDEX_REPO_ROOT="$(cd -- "$(dirname -- "$_QUANTA_INDEX_ENV_SOURCE")/.." && pwd)"
export QUANTA_INDEX_REPO_ROOT="$_QUANTA_INDEX_REPO_ROOT"
export QUANTA_INDEX_BUILD_LANE="${QUANTA_INDEX_BUILD_LANE:-shared}"
export QUANTA_INDEX_BUILD_LOGGING="${QUANTA_INDEX_BUILD_LOGGING:-1}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$_QUANTA_INDEX_CACHE_ROOT/target/${QUANTA_INDEX_BUILD_LANE}}"
export QUANTA_INDEX_STATE_ROOT="${QUANTA_INDEX_STATE_ROOT:-$_QUANTA_INDEX_CACHE_ROOT/state}"
export PYTEST_CACHE_DIR="${PYTEST_CACHE_DIR:-$_QUANTA_INDEX_CACHE_ROOT/pytest}"
export RUFF_CACHE_DIR="${RUFF_CACHE_DIR:-$_QUANTA_INDEX_CACHE_ROOT/ruff}"

# Reuse non-incremental dependency compilation after a lane is cleaned or its
# target directory is recreated. Workspace crates keep Cargo's incremental
# profile and remain non-cacheable. A repo-specific server port avoids attaching
# to another repository's differently configured sccache daemon. CI remains
# explicit and does not opt into a host-local cache.
_QUANTA_INDEX_SCCACHE_MODE="${QUANTA_INDEX_SCCACHE:-auto}"
_QUANTA_INDEX_INHERITED_WRAPPER="${RUSTC_WRAPPER:-}"
_QUANTA_INDEX_WRAPPER_NAME="${_QUANTA_INDEX_INHERITED_WRAPPER##*/}"
if [[ "$_QUANTA_INDEX_SCCACHE_MODE" == "0" && "$_QUANTA_INDEX_WRAPPER_NAME" == "sccache" ]]; then
  unset RUSTC_WRAPPER SCCACHE_DIR SCCACHE_CACHE_SIZE SCCACHE_BASEDIRS SCCACHE_SERVER_PORT
elif [[ "$_QUANTA_INDEX_SCCACHE_MODE" != "0" && "${CI:-}" != "true" ]]; then
  _QUANTA_INDEX_SCCACHE_BIN="$(command -v sccache || true)"
  if [[ -z "$_QUANTA_INDEX_SCCACHE_BIN" && "$_QUANTA_INDEX_SCCACHE_MODE" == "1" ]]; then
    printf 'QUANTA_INDEX_SCCACHE=1 but sccache is not installed\n' >&2
    return 1
  fi
  if [[ -n "$_QUANTA_INDEX_SCCACHE_BIN" && ( -z "$_QUANTA_INDEX_INHERITED_WRAPPER" || "$_QUANTA_INDEX_WRAPPER_NAME" == "sccache" ) ]]; then
    _QUANTA_INDEX_REPO_HASH="$(printf '%s' "$QUANTA_INDEX_REPO_ROOT" | cksum | awk '{print $1}')"
    export RUSTC_WRAPPER="$_QUANTA_INDEX_SCCACHE_BIN"
    export SCCACHE_DIR="$_QUANTA_INDEX_CACHE_ROOT/sccache"
    export SCCACHE_CACHE_SIZE="10G"
    export SCCACHE_BASEDIRS="$QUANTA_INDEX_REPO_ROOT"
    export SCCACHE_SERVER_PORT="$((40000 + _QUANTA_INDEX_REPO_HASH % 20000))"
  fi
fi

unset _QUANTA_INDEX_CACHE_ROOT _QUANTA_INDEX_REPO_ROOT _QUANTA_INDEX_SCCACHE_MODE
unset _QUANTA_INDEX_SCCACHE_BIN _QUANTA_INDEX_REPO_HASH
unset _QUANTA_INDEX_ENV_SOURCE _QUANTA_INDEX_INHERITED_WRAPPER _QUANTA_INDEX_WRAPPER_NAME
