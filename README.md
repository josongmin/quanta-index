# quanta-index

External search-plane scaffold for Semantica/Quanta indexing and serving.

Current scope:

- shared contract crate with bundle/control/query DTOs
- hexagonal core crate with port traits and validation services
- control-plane adapter (current backend: SQLite, kept as internal implementation detail)
- `searchd` binary scaffold for same-host UDS-based serving

Build artifacts:

- use `./scripts/cargow ...` for raw Cargo commands
- use `just ...` for repo recipes
- both route `target/` and related local caches to the shared external cache root instead of the repo working tree

Quality gates:

- lint front door: `just rust-check`, `just rust-clippy`, `just python-lint`, `just semgrep`
- test pyramid:
  - unit: `just rust-test-unit`
  - integration/component: `just rust-test-integration`
  - e2e smoke: `just rust-test-e2e`
  - full workspace: `just rust-test`

Repo layout:

- `crates/quanta-index-contract`
  - contract-only crate
  - bundle DTOs
  - query DTOs
  - control-plane DTOs
  - IPC envelopes
- `crates/quanta-index-core`
  - vendor-neutral port traits
  - application validation services
  - no `rusqlite`, no `tantivy`, no `lancedb`
- `crates/quanta-index-control`
  - control-plane adapter (backend is an internal implementation detail; currently SQLite)
  - schema bootstrap
  - outbox/activation/readiness plumbing
- `crates/quanta-index-artifact`, `quanta-index-lexical`, `quanta-index-semantic`
  - driven adapters (skeleton; vendor engines live inside each crate, not in the name)
- `crates/quanta-index-ipc`
  - IPC transport adapter (skeleton)
- `crates/quanta-index-searchd`
  - composition root + process entry
  - state-root/bootstrap/wiring
  - query engine stubs (until lexical/semantic adapters land)

Implementation packet (search-plane SSOT for this repo):

- [`docs/ssot/may-23-storage-architecture-endgame-implementation.md`](docs/ssot/may-23-storage-architecture-endgame-implementation.md)

Producer integration points in `semantica-codegraph-v2`:

- prepare-side bundle registration:
  - `packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/mod.rs`
  - `commit_internal()` / `commit_internal_with_source_bound_dense_carry_forward_v1()`
  - after `prepare_commit_publish_v1(...)`
  - before `publish_prepared_commit_v1(...)`
- finalize-side generation activation:
  - `packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/commit_finalize.rs`
  - after `publish_prepared_manifest_after_prepare_v1(...)`
  - after `finalize_published_commit_receipt_v1(...)`
- query-side IPC caller:
  - `packages/analysis/codegraph-shared/codegraph_shared/infra/fluent_engine.py`
  - `search_text_hits_v1`
  - `search_symbol_hits_v1`
  - `search_semantic_hits_v1`
  - `search_hybrid_hits_v1`

Non-goals in this scaffold:

- no raw HIR/source ingestion
- no HTTP transport
- no Tantivy/LanceDB implementation yet
- no production UDS framing implementation yet
