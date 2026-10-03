# Artifacts

- generated docs are build artifacts owned by prompt-manager
- build/runtime caches default outside the repo via `scripts/quanta-index-env.sh`
- macOS cache root: `~/Library/Caches/quanta-index/` (`target/`, `state/`, `pytest/`, `ruff/`, optional `sccache/`)
- local `sccache` uses a repository-derived server port and a 10 GiB maximum;
  inspect with `just rust-sccache-stats`, disable with `QUANTA_INDEX_SCCACHE=0`
- Cargo target lanes are pruned by `tools/ci/target_gc.py` (idle >24h unprotected
  lanes, orphan checkout ids, superseded test executables/incremental dirs; locked
  lanes are never touched); `scripts/cargow` starts it detached at most every 6h,
  disable with `QUANTA_INDEX_TARGET_GC=0`, preview with `just target-gc --dry-run`
- large bundle/vector payloads belong in artifact stores, not git
