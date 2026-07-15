# quanta-index

External search-plane for Semantica/Quanta indexing and serving.

> Current owner model (SPA-00 freeze plus Semantic Corpus V2): the producer
> (`semantica-codegraph-v2`) mints search truth — `ChunkRecord`, `SymbolRecord`,
> structural, dirty/runtime, and repo-map records — and may publish typed
> semantic-source replace/tombstone scopes. `quanta-index` validates those
> sources, derives semantic vectors, and owns generation/readiness, fusion, and
> lexical/semantic/hybrid serving. `LegacyAllChunkText` remains the explicit
> migration default. The cross-repo boundary is typed contract DTOs plus the
> `quanta-index-sdk` ingress facade over UDS transport.

Current status (lexical + semantic/hybrid serving live; live-network semantic
proof remains a separate gated rail):

- shared contract crate with bundle/control/query DTOs (manual `Serialize` /
  `Deserialize` impls, no proc-macro derives per workspace rule D18)
- hexagonal core crate with port traits and validation services
- search-plane authority/runtime path with manifest catalog, activation,
  generation pin, readiness, delta-apply governance, and typed ingest/query
  dispatch, owned by the persisted authority stores in
  `crates/quanta-index-search-plane`; the historical standalone
  `quanta-index-control` crate has been deleted
- `quanta-index-lexical` Tantivy 0.22 adapter with reader caching
- `quanta-index-semantic` persisted, generation-scoped semantic adapter:
  durable build + direct open from sealed generations (in-house CBOR columnar
  shard plus a persisted HNSW graph), no boot-time replay. See
  `docs/plans/may-28-lancedb-adoption/` — "Lance" is the planning label for
  this durable shape; the `lance` crate is intentionally not a dependency.
- `quanta-index-ipc` CBOR wire codec (16 MiB frame cap)
- `searchd` binary that actually runs: tokio current-thread UDS listener,
  owner query/control/ingest sockets, lexical + semantic + repomap serving,
  SIGINT/SIGTERM draining shutdown

Semantic Corpus V2 current state (2026-07-15):

- typed semantic-source wire covers symbol/module/cluster/document/test/raw-fallback corpora;
- semantic storage v4 preserves owner/corpus/provenance metadata and exact owner-scoped replacement;
- source-wire (7), semantic library (13), and dedicated persisted SCV2 scenarios (4) pass on latest targeted local rails;
- default derivation remains `LegacyAllChunkText`; semantic-source-only cutover and live searchd activation are not complete;
- Semantica remains responsible for graph facts, Stage3 graph expansion, and source hydration.

Current verification snapshot (2026-05-27):

- green on current live-source rerun:
  - `cargo check -p quanta-index-contract`
  - `cargo check -p quanta-index-sdk`
  - `cargo test -p quanta-index-searchd-runtime --test repo_map_end_to_end`
  - `cargo test -p quanta-index-sdk --lib`
  - `cargo test -p quanta-index-searchd-runtime`
- this snapshot re-proves the current closeout rails only; broader
  semantic/hybrid/full-corpus closure remains tracked in the `may-25`
  packet docs

Build artifacts:

- use `./scripts/cargow ...` for raw Cargo commands
- use `just ...` for repo recipes
- agent/default entrypoint: `just rust-profile <name>`
- both route `target/` and related local caches to the shared external cache root instead of the repo working tree

Recommended Rust profiles:

- `just rust-profile dev-fast` — default local edit loop
- `just rust-profile dev-daemon` — daemon/runtime-only loop
- `just rust-profile dev-all-targets` — widest compile rail after shared-surface edits
- `just rust-profile validate-shared-surface` — contract/core/sdk/search-plane shared-surface validation
- `just rust-profile test-fast` — default local test loop
- `just rust-profile test-integration` — contract/core/channel/lexical/repomap integration rail
- `just rust-profile test-daemon` — `searchd-runtime` scenario/e2e rail
- `just rust-profile verify-rust` — standard merge gate
- `just rust-profile verify-rust-heavy` — nightly/heavy correctness rail
- `just rust-profile timings-fast` / `timings-daemon` — build-regression capture rails

Rule:

- prefer a named profile over synthesizing raw `cargo` feature/target sets
- only drop to raw `cargo` when the profile catalog does not cover the task

Build profile history:

- `scripts/cargow` appends lane-level JSONL history under `{state_root}/build-profile/history.jsonl`
- `just rust-profile <name>` appends high-level profile selection history to the same file
- entries are compact by default: timestamp, lane/profile key, subcommand/recipe, duration, and exit code only
- `just rust-profile-history-summary` renders the accumulated profile/lane/failure summary

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
- `crates/quanta-index-search-plane`
  - ingest/query/control authority
  - generation/activation/readiness plumbing in persisted authority stores
  - semantic/hybrid serving helpers
- `crates/quanta-index-lexical`, `quanta-index-semantic`
  - driven adapters; the lexical backend is Tantivy and the semantic backend is
    an in-house persisted columnar shard plus an HNSW graph, each living inside
    its crate. Names stay purpose-driven so the backend can swap without
    renaming.
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
- no producer-authored public vector publish path; semantic corpus is derived
  inside `quanta-index` from typed semantic sources, with legacy chunk-text
  derivation retained as the current migration default
- no `materialized` / `failed` catalog-state transitions yet (only `prepared`
  and `active` are written; SSOT lifecycle is a Phase 3.5 follow-up)
- no production observability (tracing/metrics) — Phase 4
- no TLS/authz on UDS — Phase 4; UDS access controlled by filesystem perms
