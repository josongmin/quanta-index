# Artifacts

- generated docs are build artifacts owned by prompt-manager
- build/runtime caches default outside the repo via `scripts/quanta-index-env.sh`
- macOS cache root: `~/Library/Caches/quanta-index/` (`target/`, `state/`, `pytest/`, `ruff/`, optional `sccache/`)
- local `sccache` uses a repository-derived server port and a 10 GiB maximum;
  inspect with `just rust-sccache-stats`, disable with `QUANTA_INDEX_SCCACHE=0`
- large bundle/vector payloads belong in artifact stores, not git
