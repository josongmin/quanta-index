# quanta-index

External search-plane for Semantica/Quanta indexing and serving.

Phase 1–3 status (lexical path closed; semantic deferred to Phase 3.5):

- shared contract crate with bundle/control/query DTOs (manual `Serialize` /
  `Deserialize` impls, no proc-macro derives per workspace rule D18)
- hexagonal core crate with port traits and validation services
- control-plane adapter (SQLite; manifest catalog, activation, generation pin,
  delta-apply governance with stale/active/missing-manifest guards)
- `quanta-index-lexical` Tantivy 0.22 adapter with reader caching
- `quanta-index-semantic` Lance adapter (build + open)
- `quanta-index-ipc` CBOR wire codec (16 MiB frame cap)
- `searchd` binary that actually runs: tokio current-thread UDS listener,
  `DomainQueryEngine` wired to lexical + semantic + control, SIGINT/SIGTERM
  draining shutdown

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
- `crates/quanta-index-lexical`, `quanta-index-semantic`
  - driven adapters (Tantivy + Lance live inside each crate, names stay
    purpose-driven so the backend can swap without renaming)
- `crates/quanta-index-ipc`
  - IPC wire codec (CBOR framing via `ciborium`, 16 MiB cap, manual error
    enum — no proc-macro derives)
- `crates/quanta-index-searchd`
  - composition root + process entry
  - `searchd serve` binds the UDS, wires `DomainQueryEngine` to all adapters,
    handles SIGINT/SIGTERM with drain semantics

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

Non-goals in this Phase 1–3 cut:

- no raw HIR/source ingestion (producer responsibility)
- no HTTP transport (UDS only)
- no semantic / hybrid query ANN path yet (no embedder shipped; both return
  `NotImplemented` fail-closed)
- no `materialized` / `failed` catalog-state transitions yet (only `prepared`
  and `active` are written; SSOT lifecycle is a Phase 3.5 follow-up)
- no production observability (tracing/metrics) — Phase 4
- no TLS/authz on UDS — Phase 4; UDS access controlled by filesystem perms
