# Artifacts

- generated docs are build artifacts owned by prompt-manager
- build/runtime caches default outside the repo via `scripts/quanta-index-env.sh`
- macOS cache root: `~/Library/Caches/quanta-index/` (`target/`, `state/`, `pytest/`, `ruff/`)
- large bundle/vector payloads belong in artifact stores, not git
