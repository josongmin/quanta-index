# quanta-index

External search-plane for Semantica/Quanta indexing and serving.

> Current owner model (SPA-00 freeze plus Semantic Corpus V2): the producer
> (`semantica-codegraph-v2`) mints search truth — `ChunkRecord`, `SymbolRecord`,
> structural, dirty/runtime, and repo-map records — and may publish typed
> semantic-source replace/tombstone scopes. `quanta-index` validates those
> sources, derives semantic vectors, and owns generation/readiness, fusion, and
> lexical/semantic/hybrid serving. Default semantic derivation is
> `SemanticSourcesWithLegacyFallback`; override with
> `QUANTA_INDEX_SEMANTIC_DERIVE_MODE=legacy_all_chunk|semantic_with_legacy_fallback|semantic_only`.
> The cross-repo boundary is typed contract DTOs plus the
> `quanta-index-sdk` ingress facade over UDS transport.

Current status (lexical + semantic/hybrid serving live on the current tree;
live-network semantic proof and production ops hardening remain separate gates):

- shared contract crate with bundle/control/query DTOs (manual `Serialize` /
  `Deserialize` impls, no proc-macro derives per workspace rule D18)
- hexagonal core crate with port traits and validation services
- search-plane authority/runtime path with manifest catalog, activation,
  generation pin, readiness, delta-apply governance, and typed ingest/query
  dispatch, owned by the persisted authority stores in
  `crates/quanta-index-search-plane`; the historical standalone
  `quanta-index-control` crate has been deleted
- `quanta-index-lexical` Tantivy 0.22 adapter: per-query open via
  `LexicalIndexOpenPort` (no generation read cache on the lexical path today)
- `quanta-index-semantic` persisted, generation-scoped semantic adapter:
  durable LanceDB build + direct open from sealed generations, with a bounded
  open cache (`OPEN_CACHE_CAPACITY=8`) and no boot-time journal replay.
  Legacy `state_root/semantic/journal.cbor` is one-shot migration input only.
  Logical corpus predicates are pushed into the storage query instead of
  applied after an unfiltered ANN read.
- `quanta-index-ipc` CBOR wire codec (16 MiB frame cap)
- `searchd` binary that actually runs: blocking `std::thread` UDS accept loops
  over `UnixListener` (query/control/ingest sockets), lexical + semantic +
  repomap serving, SIGINT/SIGTERM draining shutdown. Tokio exists inside the
  semantic LanceDB adapter seam, not as the UDS listener runtime.

Semantic Corpus V2 current state (code-truth snapshot, HEAD `526349b`, 2026-09-16):

- typed semantic-source wire covers symbol/module/cluster/document/test/raw-fallback corpora;
- semantic storage v4 preserves owner/corpus/provenance metadata and exact owner-scoped replacement;
- HybridSeed accepts typed per-corpus budgets, runs storage-prefiltered dense
  lanes independently, collapses views to stable owner identity, and fuses
  stable IDs without manufacturing lexical candidates;
- SCV2 source-wire and persisted scenario rails exist under
  `crates/quanta-index-contract/tests/scv2_01_semantic_source_wire.rs` and
  `crates/quanta-index-semantic/tests/scv2_persisted_scenarios.rs`;
- owner-local unit coverage is broad across contract/core/SDK/search-plane/
  semantic crates; exact counts drift — use `just rust-profile test-fast` rather
  than frozen README numbers;
- semantic-source-only cutover and live card-required activation are not complete;
- Semantica remains responsible for graph facts, Stage3 graph expansion, and source hydration.

Current verification posture (2026-09-16):

- substrate rails that remain the merge gate:
  - `just rust-profile verify-rust`
  - `just rust-public-api`
  - `just rust-cargo-modules`
  - `just rust-hexagonal`
  - `just rust-fuzz-smoke`
  - `just rust-profile test-daemon`
- these prove correctness of the current code substrate, not production ops readiness;
- [`docs/bugbash/sep-16/findings.md`](docs/bugbash/sep-16/findings.md) records
  open P1/P2 operational and contract gaps on HEAD `4914156` and later; treat
  bugbash as the current production-readiness inventory until remediated.

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
    LanceDB, each living inside its crate. Names stay purpose-driven so the
    backend can swap without renaming.
- `crates/quanta-index-ipc`
  - IPC wire codec (CBOR framing via `ciborium`, 16 MiB cap, manual error
    enum — no proc-macro derives)
- `crates/quanta-index-searchd`
  - composition root + process entry
  - `searchd serve` binds the UDS, wires `DomainQueryEngine` to all adapters,
    handles SIGINT/SIGTERM with drain semantics

Implementation packet (search-plane SSOT for this repo):

- [`docs/ssot/README.md`](docs/ssot/README.md)
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
- no production observability exporter (tracing/metrics shipping) — Phase 4;
  process-local query/embedding diagnostic stores exist for harness and bounded
  in-process sampling only
- no TLS/authz on UDS — Phase 4; UDS access controlled by filesystem perms
