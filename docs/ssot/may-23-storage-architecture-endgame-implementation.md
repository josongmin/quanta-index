# quanta-index Search Plane Implementation Plan

Status: `Canonical implementation plan for this repo (partially superseded — see SPA-00 note)`

> **SPA-00 owner-model freeze note (jul-7).** Two things below are no longer
> current-tree truth: (1) the standalone `quanta-index-control` (SQLite) crate
> and its `store`/`ControlPlane` module references have been **deleted from the
> workspace** — generation/activation/readiness/delta-apply authority now lives
> in the search plane's persisted authority stores
> (`crates/quanta-index-search-plane/src/{ingest_dispatcher,readiness}.rs`);
> (2) semantic derivation modes live in
> `crates/quanta-index-search-plane/src/semantic_derive.rs`; default is
> `SemanticSourcesWithLegacyFallback`, not legacy-only chunk text. Read the
> `quanta-index-control` table rows below as historical plan context, not as
> current live crates.

## Scope

이 문서는 **external search-plane** (`quanta-index`)의 **내부 구현 순서**만 고정한다.

**외부 인터페이스는 이미 확정**되어 있다. producer(`semantica-codegraph-v2`)와 query client가 보는 계약은 `quanta-index-contract` DTO/IPC envelope이며, 이 레포 작업은 그 계약을 **충실히 구현**하는 것이다. contract shape 변경은 producer coordination 없이 하지 않는다.

| 경계 | SSOT crate / module | 상태 |
|------|---------------------|------|
| **외부 (frozen)** | `quanta-index-contract` | **확정** — cross-repo 유일 surface |
| **내부 (this doc)** | `quanta-index-core` ports, adapters, `searchd` wiring | 구현 진행 중 |

아키텍처 방향:

1. **domain 구분** — bounded context 단위로 책임을 나눈다
2. **hexagonal** — domain은 port로만 바깥과 통신하고, vendor/transport는 adapter에 둔다

핵심 전제:

1. producer/indexing authority는 `semantica-codegraph-v2`에 남는다
2. 이 레포는 published bundle 수신, generation activation, index materialization, query serving만 담당한다
3. **외부 wire/API = contract DTO**; **내부 port trait = core only**
4. storage object engine / LMDB / mmap segment endgame은 **이 레포 범위 밖**이다

## Out of Scope (other repo)

- `quanta-storage-*`, `QueryExecutor` artifact store, reasoning graph storage, published graph segment authority
- producer storage SSOT는 `semantica-codegraph-v2` plan을 따른다

## Domain Model

search-plane는 아래 **4개 domain**으로 나눈다. domain 간 직접 호출은 금지하고, `searchd::app` composition root가 use case를 orchestrate 한다.

| Domain | `quanta-index-core` module | 책임 | Producer seam |
|--------|---------------------------|------|-----------------|
| **bundle-ingest** | `domains::bundle_ingest` | prepared outbox 수신, delta apply, manifest ref 검증 | `prepare_commit_publish_v1` |
| **generation** | `domains::generation` | generation catalog, activation, readiness, inspect | finalize / activation |
| **materialization** | `domains::materialization` | bundle artifact load, lexical/semantic index build·open | (internal, post-prepare/activate) |
| **query** | `domains::query` | lexical / semantic / hybrid / explain 실행 | UDS IPC caller |

**Transport** (UDS byte codec)는 domain이 아니라 **driving adapter** 구현 상세다. request/response **payload shape**는 `quanta-index-contract::ipc`에 확정되어 있다.

### Domain dependency rule

```text
bundle-ingest ──► generation ──► materialization ──► query
       │               │                │
       └───────────────┴────────────────┴──► contract DTOs only (no sideways imports)
```

1. domain 모듈 간 `use crate::domains::<other>` 금지 — `tools/ci/lint/lint-hexagonal-boundaries.py`가 검사
2. cross-domain orchestration은 `quanta-index-searchd::app` only
3. `query`는 materialized index **outbound store port**만 본다 (vendor index 타입 직접 참조 금지)

## Hexagonal Layout

```text
                    ┌──────────────────────────────────────┐
  producer / CLI    │  Driving adapters (primary)           │
  ───────────────►  │  quanta-index-searchd                 │
                    │    app/          composition root     │
                    │    cli/          operator entry        │
                    │    query/        StubQueryEngine       │
                    │    runtime/      bootstrap + state    │
                    │  quanta-index-ipc (transport)          │
                    └──────────────────┬───────────────────┘
                                       │
                    ┌──────────────────▼───────────────────┐
                    │  quanta-index-core :: domains::*      │
                    └──────────┬─────────────┬─────────────┘
                               │ driven ports │
         ┌─────────────────────┼─────────────┼─────────────────────┐
         ▼                     ▼             ▼                     ▼
  quanta-index-control   quanta-index-   quanta-index-   quanta-index-
  store/ (SQLite)        artifact        lexical         semantic
```

### Layer roles

| Layer | Location | 허용 | 금지 |
|-------|----------|------|------|
| **Contract** | `crates/quanta-index-contract` | DTO, ids, IPC envelopes | logic, port traits, vendor |
| **Application** | `crates/quanta-index-core/src/domains` | policies, inbound/outbound ports | `rusqlite`, `tantivy`, `lancedb`, raw FS layout |
| **Driven adapter** | `quanta-index-control`, `quanta-index-artifact`, `quanta-index-lexical`, `quanta-index-semantic` | outbound port impl | domain policy duplication |
| **Driving adapter** | `quanta-index-searchd`, `quanta-index-ipc` | wiring, transport, CLI | business rules beyond orchestration |

## Repository Layout (current)

Current workspace members (`Cargo.toml`):

- `quanta-index-contract` — frozen external interface
- `quanta-index-core` — domains + ports
- `quanta-index-control` — control-plane persistence (SQLite today)
- `quanta-index-searchd` — composition root + process entry

```text
quanta-index/
  scripts/
    cargow                      # cargo wrapper → external cache target
    quanta-index-env.sh         # CARGO_TARGET_DIR, PYTEST_CACHE_DIR, state paths
    check_workspace_lints.py
    check-rust-allow-attributes.sh
    run-cargo-deny.sh
    run-semgrep.sh
  tools/ci/
    lint/
      lint-hexagonal-boundaries.py
      lint-root-hygiene.sh
      lint-doc-paths.py
    semgrep/rules.yml
    tests/
  crates/
    quanta-index-contract/src/
      bundle/                   # PreparedBundleOutbox, manifest, mutation delta
      control/                  # prepare/activate/delta/inspect DTOs
      query/                    # LqQuery, filters, options
      results/                  # candidates, explanation
      ipc/                      # request/response envelopes
      ids.rs
    quanta-index-core/src/
      domains/
        mod.rs
        bundle_ingest/
          outbound.rs           # PublishedSearchBundle{Prepare,DeltaApply}Port
          service.rs              # BundlePolicy
        generation/
          outbound.rs           # activate, readiness, inspect, catalog, activation state
          service.rs              # ActivationPolicy
        materialization/
          outbound.rs           # artifact + index build/store ports
        query/
          inbound.rs              # lexical/semantic/hybrid/explain query ports
          outbound.rs             # GenerationPinPort, SearchPlaneQueryValidator
          service.rs              # QueryPolicy
      error.rs                    # CoreError
      lib.rs                      # crate-root re-exports
      benches/policy_bench.rs
      tests/policy_contracts.rs
      tests/property_policies.rs
    quanta-index-control/src/
      lib.rs                      # pub use store::*
      store/
        mod.rs                    # ControlPlane + open()
        schema.rs                 # SQLite DDL bootstrap
        bundle_ingest.rs          # impl PublishedSearchBundlePreparePort
        generation_registry.rs    # impl activate/readiness/inspect
        helpers.rs
        tests.rs
      tests/control_plane.rs
    quanta-index-searchd/src/
      lib.rs
      bin/quanta-index-searchd.rs
      app/
        config.rs               # SearchdConfig, state_root / socket paths
        searchd.rs                # serve entry (bootstrap only today)
        mod.rs
      cli/
      runtime/
        bootstrap.rs              # ControlPlane::open
        state.rs                  # SearchRuntime
      query/
        stub_engine.rs            # NotImplemented query inbound ports
      tests/bootstrap.rs
      tests/serve_smoke.rs
  docs/ssot/                      # this file
```

Planned crates (not in workspace yet):

- `quanta-index-artifact`
- `quanta-index-lexical`
- `quanta-index-semantic`
- `quanta-index-ipc`

### Adapter crate naming (decided)

**crate 이름에 vendor/DB 이름을 넣지 않는다** (`tantivy`, `lance`, `lancedb` 등).  
crate는 capability 기준(`artifact`, `lexical`, `semantic`, `ipc`)이고, 구체 엔진은 **crate 내부 구현**으로 숨긴다.

### Adapter crate split: lexical vs semantic (decided)

**lexical adapter crate와 semantic adapter crate는 분리**한다. 한 crate에 두 인덱스 엔진을 넣지 않는다.

| Crate | Implements (core outbound ports) | Status |
|-------|----------------------------------|--------|
| `quanta-index-artifact` | `PublishedSearchArtifactStorePort` | planned |
| `quanta-index-lexical` | `SearchPlaneLexicalIndexBuildPort`, `SearchPlaneLexicalIndexStorePort` | planned |
| `quanta-index-semantic` | `SearchPlaneSemanticIndexBuildPort`, `SearchPlaneVectorIndexStorePort`, `SearchPlaneEmbeddingProviderPort` (if needed) | planned |
| `quanta-index-ipc` | frozen `contract::ipc` ↔ wire bytes | planned |

규칙:

1. adapter crate 이름은 **capability-only** — vendor 문자열 금지
2. lexical / semantic adapter crate는 **서로 depend 하지 않는다** (lint enforced)
3. vendor dependency는 해당 adapter crate **내부**에만 추가 (core/contract 금지)
4. `domains::query` hybrid는 `searchd::app`이 lexical + semantic store를 조합 (세 번째 search adapter crate 없음)
5. `quanta-index-searchd`가 composition root로 adapter들을 wiring

`SearchPlaneMetadataStorePort` owner는 구현 시 `quanta-index-artifact` 또는 `quanta-index-lexical` 중 하나로 고정한다.

## Runtime Layout

`SearchdConfig` (`quanta-index-searchd::app::config`) 기준:

| Env / input | Path |
|-------------|------|
| `QUANTA_INDEX_STATE_ROOT` | explicit state root |
| default | `{QUANTA_INDEX_CACHE_ROOT}/state` or macOS `~/Library/Caches/quanta-index/state` |
| control plane DB | `{state_root}/control-plane.sqlite3` |
| query UDS socket path | `{state_root}/search-plane/query.sock` |
| control UDS socket path | `{state_root}/search-plane/control.sock` |

Build/cache (repo 밖):

| Variable | Default |
|----------|---------|
| `CARGO_TARGET_DIR` | `{cache_root}/target` |
| `PYTEST_CACHE_DIR` | `{cache_root}/pytest` |
| `RUFF_CACHE_DIR` | `{cache_root}/ruff` |

`scripts/quanta-index-env.sh` / `just` / `pre-commit` / `./scripts/cargow` 가 위 경로를 설정한다.

### Search-plane on-disk layout (decided)

All search-plane runtime data lives under `{state_root}`.

```text
{state_root}/
  control-plane.sqlite3
  search-plane/
    query.sock
    control.sock
  bundles/
    {repo_id}/
      {revision_id}/
        prepared/
          {outbox_id}/
            manifest.json
            artifacts/
        generations/
          {manifest_generation}/
            manifest.json
            delta/
              chunks.jsonl
              symbols.jsonl
              embeddings.jsonl
  indexes/
    lexical/
      {repo_id}/
        {revision_id}/
          g{manifest_generation}/
    semantic/
      {repo_id}/
        {revision_id}/
          g{manifest_generation}/
```

Rules:

1. `BundleArtifactRef.relative_path` is always resolved relative to `{state_root}/bundles/{repo_id}/{revision_id}/`.
2. `PreparedBundleOutbox.manifest_ref` must point into `prepared/{outbox_id}/...`; absolute paths are invalid.
3. `record_generation_manifest(...)` canonicalizes the active manifest to `generations/{manifest_generation}/manifest.json`.
4. lexical and semantic indexes are generation-scoped directories; active generations are discovered from `generation_activation_state`, not by scanning `indexes/`.
5. query UDS transport uses one socket per `state_root`, not one socket per repo or revision.

## Frozen External Interface (`quanta-index-contract`)

확정된 cross-repo surface. **이 레포에서 필드/variant 추가·삭제·rename 금지** (breaking-first, producer sync 필수).

### Public ids (`ids.rs`)

- `RepoId`, `RevisionId`, `ManifestGeneration`, `GenerationId`, `ManifestDigest`, `FileId`, `RepoRelativePath`

### Bundle (`bundle/`)

- `PreparedBundleOutbox`, `PublishedSearchBundleManifest`, `PublishedGenerationSet`
- `BundleArtifactRef`, `BundleMode`, `BundleEncoding`
- `SearchBundleMutationDelta`, `SearchBundleMutationOp` (chunk/symbol/embedding upsert/delete)
- `FileMaterializationPacket` is producer-side materialization context; search-plane does not consume it directly on its public wire

### Control (`control/`)

| Request | Response |
|---------|----------|
| `PublishedSearchBundlePrepareRequest` | `PublishedSearchBundlePrepareResponse` |
| `PublishedSearchBundleDeltaApplyRequest` | `PublishedSearchBundleDeltaApplyResponse` |
| `PublishedSearchGenerationActivateRequest` | `PublishedSearchGenerationActivateResponse` |
| (readiness: `RepoId` + `RevisionId`) | `PublishedSearchGenerationReadinessResponse` |
| (inspect: `PublishedGenerationSet`) | `PublishedSearchBundleInspectResponse` |

### Query + results (`query/`, `results/`)

- `LqQuery`, `LqExpr`, `LqFilterSet`, `LqOptionSet`, `LqDirectiveSet`
- `SearchPlaneLexicalQueryRequest`, `SearchPlaneSemanticQueryRequest`, `SearchPlaneHybridQueryRequest`, `SearchPlaneExplainQueryRequest`
- matching `*QueryResponse`, `LexicalCandidate`, explanation types

### IPC (`ipc/`)

- `SearchPlaneQueryIpcRequestEnvelope { request_id, payload }`
- `SearchPlaneQueryIpcRequest`: `Text` | `Symbol` | `Semantic` | `Hybrid` | `History` | `Structural` | `Bridge` | `RepoMapQuery` | `Explain` | `Sourcegraph`
- `SearchPlaneQueryIpcResponseEnvelope { request_id, payload }`
- `SearchPlaneQueryIpcResponse`: typed success variants | `Error(SearchPlaneIpcError { code, message })`
- `SearchPlaneControlIpcRequestEnvelope` / `SearchPlaneControlIpcResponseEnvelope`

Producer/query client는 위 타입만 import한다. `quanta-index-core` port trait는 **repo 내부**이며 외부에 노출하지 않는다.

## Internal Port Inventory (`quanta-index-core`)

내부 port trait는 domain module에만 정의한다. adapter가 impl한다.

### `domains::bundle_ingest`

| File | Symbol | Direction | Impl today |
|------|--------|-----------|------------|
| `outbound.rs` | `PublishedSearchBundlePreparePort` | driven | `quanta-index-control::store` |
| `outbound.rs` | `PublishedSearchBundleDeltaApplyPort` | driven | **missing** |
| `service.rs` | `BundlePolicy` | domain policy | unit/property tests |

### `domains::generation`

| File | Symbol | Direction | Impl today |
|------|--------|-----------|------------|
| `outbound.rs` | `PublishedSearchGenerationActivatePort` | driven | `quanta-index-control::store` |
| `outbound.rs` | `PublishedSearchGenerationReadinessPort` | driven | `quanta-index-control::store` |
| `outbound.rs` | `PublishedSearchBundleInspectPort` | driven | `quanta-index-control::store` (placeholder manifest) |
| `outbound.rs` | `PublishedSearchGenerationCatalogPort` | driven | **missing** |
| `outbound.rs` | `PublishedSearchActivationStatePort` | driven | **missing** |
| `service.rs` | `ActivationPolicy` | domain policy | unit/property tests |

### `domains::materialization`

| File | Symbol | Direction | Impl today |
|------|--------|-----------|------------|
| `outbound.rs` | `PublishedSearchArtifactStorePort` | driven | **missing** |
| `outbound.rs` | `SearchPlaneLexicalIndexBuildPort` | driven | **missing** |
| `outbound.rs` | `SearchPlaneSemanticIndexBuildPort` | driven | **missing** |
| `outbound.rs` | `SearchPlaneLexicalIndexStorePort` | driven | **missing** |
| `outbound.rs` | `SearchPlaneVectorIndexStorePort` | driven | **missing** |
| `outbound.rs` | `SearchPlaneMetadataStorePort` | driven | **missing** |
| `outbound.rs` | `SearchPlaneEmbeddingProviderPort` | driven | **missing** |

### `domains::query`

| File | Symbol | Direction | Impl today |
|------|--------|-----------|------------|
| `inbound.rs` | `SearchPlaneLexicalQueryPort` | driving | `searchd::query::StubQueryEngine` (NotImplemented) |
| `inbound.rs` | `SearchPlaneSemanticQueryPort` | driving | stub |
| `inbound.rs` | `SearchPlaneHybridQueryPort` | driving | stub |
| `inbound.rs` | `SearchPlaneExplainQueryPort` | driving | stub |
| `inbound.rs` | `SearchPlaneQueryContractPort` | driving aggregate | stub |
| `outbound.rs` | `GenerationPinPort` | driven | **missing** |
| `outbound.rs` | `SearchPlaneQueryValidator` | driving hook | policy via `QueryPolicy` |
| `service.rs` | `QueryPolicy` | domain policy | unit/property tests |

내부 orchestration hook (contract 밖, `searchd::app` only):

- materialize-before-activate sequencing
- `GenerationPinPort` lifecycle

## Crate Dependency Matrix

`lint-hexagonal-boundaries.py`가 고정:

| Crate | May depend on |
|-------|----------------|
| `quanta-index-contract` | (none) |
| `quanta-index-core` | `quanta-index-contract` |
| `quanta-index-control` | `contract`, `core` |
| `quanta-index-searchd` | `contract`, `core`, `control` |

## Producer Integration Seams

모든 seam은 **확정된 contract DTO**로만 crossing한다.

Transport split (decided):

| Path | Transport | Owner | Notes |
|------|-----------|-------|-------|
| prepare / finalize / delta / readiness | direct Rust call against `quanta-index-control::ControlPlane` on shared `state_root` | producer + control-plane adapter | query UDS를 재사용하지 않는다 |
| lexical / semantic / hybrid / explain query | UDS stream socket at `{state_root}/search-plane/query.sock` | `searchd` | read-only query plane |
| repo-map ingest / activation | UDS stream socket at `{state_root}/search-plane/control.sock` | `searchd` | mutation/control plane |

### 1. bundle-ingest ← prepare

- producer seam: `prepare_commit_publish_v1(...)`
- **external**: `PublishedSearchBundlePrepareRequest` / `PublishedSearchBundlePrepareResponse`
- **internal**: `PublishedSearchBundlePreparePort` → `quanta-index-control/src/store/bundle_ingest.rs`
- reference: `semantica-codegraph-v2/.../index_projection_writer/commit_prepare.rs`

### 2. generation ← finalize

- producer seam: after `publish_prepared_manifest_after_prepare_v1`, `finalize_published_commit_receipt_v1`
- **external**: `PublishedSearchGenerationActivateRequest`, `PublishedSearchGenerationReadinessResponse`, `PublishedSearchBundleDeltaApplyRequest` (when delta present)
- **internal**: generation outbound ports → `quanta-index-control/src/store/generation_registry.rs`
- reference: `semantica-codegraph-v2/.../index_projection_writer/commit_finalize.rs`

### 3. query ← fluent engine UDS client

- producer seam: `search_*_hits_v1`
- **external**: split query/control IPC envelopes (payload types frozen per socket)
- **internal**: UDS byte codec adapter + `domains::query` inbound handlers
- reference: `semantica-codegraph-v2/.../codegraph_shared/infra/fluent_engine.py`

## Runtime Orchestration (decided)

`searchd::app` is the only cross-domain orchestrator. Producer never drives index build directly and never flips readiness bits itself.

### Lifecycle owner

1. producer owns bundle creation and prepare/finalize intent
2. `quanta-index-control` owns durable control-plane state
3. `searchd` owns materialization, readiness, activation ordering, and query serving

### Single-daemon model

1. one `searchd` process per `{state_root}`
2. one UDS listener per `{state_root}`
3. the daemon multiplexes repo/revision/generation inside the process; it does not spawn one socket per repo

### Canonical sequence

1. producer writes bundle artifacts under `bundles/{repo_id}/{revision_id}/prepared/{outbox_id}/...`
2. producer calls `prepare_bundle(PublishedSearchBundlePrepareRequest)`
3. control-plane persists `prepared_bundle_outbox`, sets `claim_state='prepared'`, returns duplicate-aware response
4. producer records canonical generation manifest through `record_generation_manifest(PublishedSearchBundleManifest)`
5. if `mutation_delta` exists, producer calls `apply_bundle_delta(PublishedSearchBundleDeltaApplyRequest)`
6. `searchd` control loop claims the prepared generation, loads the canonical manifest, and materializes lexical first and semantic second
7. only after both materializations open successfully does `searchd` call `activate_generation(...)` with `lexical_ready=true` and `semantic_ready=true`
8. queries resolve against the pinned or active generation only after activation commits

Fail-closed rules:

1. producer does not call `activate_generation(...)` directly in the steady-state architecture
2. `activate_generation(...)` is the last step after successful materialization, never a trigger for materialization
3. partial lexical-only or semantic-only success never produces an active generation
4. if materialization fails, query serving remains on the prior active generation and the new generation stays non-active

## Control-plane State Model (decided)

Current SQLite schema is the control-plane SSOT until dedicated storage adapters land.

### Table roles

| Table | Authority | Meaning |
|-------|-----------|---------|
| `prepared_bundle_outbox` | prepare ingress | durable prepare receipt + manifest ref + claim state |
| `generation_catalog` | generation registry | per-generation manifest and component generation metadata |
| `generation_activation_state` | serve head | exactly one active manifest generation per repo/revision |
| `external_search_consumer_ack` | producer feedback | duplicate / accepted / failed outcome visible to producer |
| `indexing_jobs` + dependency tables | `searchd` internal orchestration | future async job ledger; producer must not write them directly |
| replay / closeout tables | recovery | daemon recovery and replay bookkeeping |

### State transitions

`prepared_bundle_outbox.claim_state`:

```text
prepared -> claimed -> materialized -> activated
prepared -> claimed -> failed
```

`external_search_consumer_ack.ack_state`:

```text
accepted | duplicate | failed
```

`generation_catalog.state` target values:

```text
prepared | materialized | active | failed
```

Rules:

1. `claim_state` is daemon-owned after the prepare row is inserted.
2. `generation_activation_state` is the only query-time serve-head authority.
3. `generation_catalog` may contain future generations that are not active yet.
4. indexing job tables are implementation detail; they do not replace `prepared_bundle_outbox` or `generation_activation_state` as authority.

## Bundle-ingest and Delta Apply Rules (decided)

### Prepare semantics

1. `prepare_bundle(...)` validates `outbox_id`, `manifest_ref.relative_path`, and `bundle_schema_version` through `BundlePolicy`.
2. duplicate prepare (`UNIQUE(repo_id, revision_id, manifest_digest)` hit) returns `accepted=false`, `state="prepared"`, `reason="prepared bundle already exists"`.
3. prepare stores the manifest reference only; it does not activate, materialize, or mutate indexes.

### `record_generation_manifest(...)`

1. canonical manifest bytes are stored at `bundles/{repo_id}/{revision_id}/generations/{manifest_generation}/manifest.json`
2. `inspect_bundle(...)` must load from that canonical path once implemented; placeholder artifacts are temporary only
3. manifest persistence happens before delta apply and before materialization

### Delta apply target

`apply_bundle_delta(...)` never patches the active query indexes in place. It mutates the staging area of the target generation only.

| Operation | Target staging surface | Materialization effect |
|-----------|------------------------|------------------------|
| `UpsertChunk` / `DeleteChunk` | `delta/chunks.jsonl` | affects lexical chunk rows for the target generation |
| `UpsertSymbol` / `DeleteSymbol` | `delta/symbols.jsonl` | affects symbol rows and lexical symbol lookups |
| `UpsertEmbedding` / `DeleteEmbedding` | `delta/embeddings.jsonl` | affects semantic embedding inputs / records |

Rules:

1. delta apply is scoped by `(repo_id, revision_id, manifest_generation)` and never crosses generations
2. repeated identical delta against the same target generation is idempotent and returns `applied=false` with a duplicate reason
3. delta apply after activation of the same manifest generation is rejected fail-closed
4. delta apply against a generation without a recorded canonical manifest is rejected fail-closed

## Generation Rules (decided)

### Stale activation (`E-SP2`)

Reject activation when any of the following holds for the same `(repo_id, revision_id)`:

1. requested `manifest_generation` is lower than `generation_activation_state.active_manifest_generation`
2. `generation_catalog` already contains a strictly higher `manifest_generation` in `materialized` or `active` state
3. requested component generations do not match the recorded manifest generation row

Return shape:

- control path: `PublishedSearchGenerationActivateResponse { activated: false, active_generation: <current>, reason: Some("stale generation") }`

### Inspect semantics

1. `inspect_bundle(...)` is generation-catalog-backed, not placeholder-backed
2. it returns the canonical manifest plus the artifact refs reachable from that manifest
3. failure to load the canonical manifest is a storage failure, not an empty success

## Query Semantics (decided)

### Generation resolution

For lexical / semantic / hybrid requests:

1. if `request.generation` is `Some(g)`, the daemon must use exactly `g`
2. else if a query-time pin exists, use the pinned generation
3. else use `generation_activation_state` active generation
4. if none exists, return `SearchPlaneIpcError { code: "NOT_READY", ... }`

Failure mapping:

1. explicit generation not found in `generation_catalog` -> `UNKNOWN_GENERATION`
2. generation exists but `lexical_ready=false` or `semantic_ready=false` for the requested mode -> `NOT_READY`
3. `LqExpr::MatchAll` or malformed request contract -> `INVALID_REQUEST`

### Hybrid merge

Hybrid query uses reciprocal-rank fusion.

Rules:

1. run lexical and semantic retrieval against the same resolved generation
2. fuse by `rrf_score = 1/(60 + lexical_rank) + 1/(60 + semantic_rank)`
3. stable tie-break order: higher `rrf_score`, then lexical presence, then `candidate_id`
4. `top_k` is applied after fusion

### Explain

1. explain accepts a `LexicalCandidate` that came from a prior lexical / semantic / hybrid response for the same generation
2. searchd does not invent candidate identities for explain
3. generation mismatch between request candidate and resolved generation is `INVALID_REQUEST`

### CoreError -> IPC mapping

| Source | IPC error code |
|--------|----------------|
| `CoreError::InvalidContract` | `INVALID_REQUEST` |
| `CoreError::NotReady` | `NOT_READY` |
| `CoreError::NotImplemented` | `NOT_IMPLEMENTED` |
| `CoreError::Storage` | `INTERNAL` |

Adapter-only codes:

- `UNKNOWN_GENERATION`
- `FRAMING_ERROR`

## UDS Wire Protocol (decided)

Transport is AF_UNIX stream socket. Payload types remain the frozen `contract::ipc` envelopes.

### Frame format

1. 4-byte little-endian unsigned length prefix
2. CBOR body containing exactly one split query/control IPC envelope, depending on socket
3. maximum frame size: 8 MiB request, 8 MiB response

### Connection model

1. one connection may carry multiple sequential requests
2. phase-1 server rule: at most one in-flight request per connection
3. responses preserve request order and echo `request_id`

### Framing failure policy

1. invalid length prefix or payload larger than cap -> close connection immediately
2. JSON decode failure before a full envelope is available -> close connection immediately
3. decoded envelope with invalid payload contract -> `Error(SearchPlaneIpcError { code: "INVALID_REQUEST", ... })`
4. transport/frame failure never falls back to empty results

## Architecture Principles

1. **contract-first** — 외부 interface는 `quanta-index-contract`에 frozen; port trait는 내부 only
2. **domain isolation** — enforced by `lint-hexagonal-boundaries.py` + semgrep
3. **hexagonal boundary** — application I/O는 outbound port impl을 통해서만
4. **fail-closed** — unreadiness / incomplete materialization에 silent empty fallback 금지
5. **generation pin** — `GenerationPinPort` + query path (impl pending)
6. **breaking-first** — legacy top-level core modules (`generation_registry`, `artifact_objects`, …) 재도입 금지

## Implementation Status

| Area | Layer | Status |
|------|-------|--------|
| **external interface** (`quanta-index-contract`) | frozen | **done / 확정** |
| `domains/*` module tree + crate re-exports | internal | **done** |
| hexagonal boundary lint + pre-commit + CI | internal | **done** |
| `BundlePolicy`, `ActivationPolicy`, `QueryPolicy` | internal | **done** |
| control: `prepare_bundle`, `activate_generation`, readiness, inspect | internal adapter | **partial** (inspect placeholder) |
| control: `apply_bundle_delta`, `record_generation_manifest` | internal adapter | **missing** |
| materialization adapters | internal adapter | **missing** |
| query inbound handlers | internal | **stub** |
| UDS byte codec + listener | internal transport | **missing** |
| `searchd::app` orchestration | internal | **missing** (bootstrap-only serve) |
| producer E2E against frozen contract | integration | **missing** |

## Scenario Matrix

### Usecase

| ID | Given | When | Then | Observe |
|----|-------|------|------|---------|
| `U-SP1` | producer created prepared manifest under `prepared/{outbox_id}/` | producer calls `prepare_bundle(...)` | outbox row exists with `claim_state='prepared'` and duplicate-aware response | `prepared_bundle_outbox`, `external_search_consumer_ack` |
| `U-SP2` | canonical manifest exists for `G2`, optional delta staged | `searchd` materializes then activates `G2` | `generation_catalog.state='active'`, `generation_activation_state.active_manifest_generation=G2`, query requests serve `G2` | `generation_catalog`, `generation_activation_state`, UDS query response generation |
| `U-SP3` | `G1` was active before daemon restart | `searchd` restarts and reopens control plane + indexes | active generation remains `G1`; no re-prepare required | restart smoke test + readiness query |
| `U-SP4` | query request pinned on `G1`, producer prepares `G2` concurrently | pinned query runs while `G2` is materialized/activated | in-flight query stays on `G1`; only subsequent queries may observe `G2` | query response generation, activation timestamp ordering |

### Edge

| ID | Given | When | Then | Observe |
|----|-------|------|------|---------|
| `E-SP1` | identical `(repo_id, revision_id, manifest_digest)` already prepared | producer calls `prepare_bundle(...)` again | response is `accepted=false`; no new outbox state transition | prepare response + unchanged rowcount |
| `E-SP2` | `G5` already active or `G6` already materialized | caller tries to activate `G4` | activation rejected with `activated=false`, `reason="stale generation"` | activation response + unchanged active generation |
| `E-SP3` | manifest ref path escapes bundle root or file is missing | `searchd` tries to materialize | materialization fails closed; prior active generation remains authoritative | failure reason + no activation row change |

### Corner

| ID | Given | When | Then | Observe |
|----|-------|------|------|---------|
| `C-SP1` | no active generation and no pin | client sends query with `generation=None` | `SearchPlaneIpcError { code: "NOT_READY", ... }` | UDS error envelope |
| `C-SP2` | generation exists but requested mode is not fully ready | lexical/semantic/hybrid query arrives | request fails with `NOT_READY`; no empty-hit fallback | UDS error envelope + readiness row |

### Hellgate

| ID | Given | When | Then | Observe |
|----|-------|------|------|---------|
| `H-SP1` | build/open of lexical or semantic index has not completed | any actor attempts activation | activation is blocked; `generation_activation_state` unchanged | activation response or missing write |
| `H-SP2` | generation row exists but readiness flags are false | query explicitly targets that generation | daemon returns `NOT_READY`; never serves partial results | UDS error envelope |
| `H-SP3` | malformed length prefix or oversized frame | client writes invalid bytes to socket | connection closes or `FRAMING_ERROR` is emitted if envelope boundary was already known | socket close / transport error telemetry |

## Phase Plan

### Phase 0: External interface + structure freeze

산출물:

1. **frozen** `quanta-index-contract` (producer/query client SSOT)
2. `domains/*` internal port layout
3. boundary lint

상태: **done** — 이후 작업은 contract를 바꾸지 않고 adapter/orchestration만 채운다

### Phase 1: control adapter completion

1. `PublishedSearchBundleDeltaApplyPort` on `quanta-index-control::store`
2. `PublishedSearchGenerationCatalogPort` (+ manifest persistence policy)
3. replace `inspect_bundle` placeholder with artifact-backed path once Phase 2.1 lands

gates: `U-SP1`, `E-SP1`, `E-SP2`

### Phase 2: materialization adapters

1. `quanta-index-artifact` — `PublishedSearchArtifactStorePort`
2. `quanta-index-lexical` — lexical build/open ports
3. `quanta-index-semantic` — semantic/vector build/open ports (vendor deps only inside this crate)
4. `searchd::app` orchestration hook for materialize-before-activate

gates: `U-SP2`, `E-SP3`, `H-SP1`

### Phase 3: query + transport

1. real `domains::query` inbound impl (replace `StubQueryEngine`) — **frozen IPC payload** decode/encode
2. `GenerationPinPort` impl + `U-SP4`
3. `quanta-index-ipc` — UDS **wire codec only** (envelope types 변경 없음)

gates: `U-SP3`, `U-SP4`, `C-SP*`, `H-SP2`, `H-SP3`

## Verification Plan

| Rail | Command |
|------|---------|
| format | `just fmt-check` |
| compile | `just rust-check` |
| clippy | `just rust-clippy` |
| hexagonal boundaries | `just rust-hexagonal` |
| workspace lints / deny / no-allow | `just rust-policy` |
| unit | `just rust-test-unit` |
| integration | `just rust-test-integration` |
| e2e smoke | `just rust-test-e2e` |
| full verify | `just verify` |

Enforcement:

- `tools/ci/lint/lint-hexagonal-boundaries.py` — crate deps, domain isolation, legacy module ban, contract-no-trait
- semgrep — `core-no-vendor-import`, `contract-no-port-trait`, `rust-no-unwrap`, …
- pre-commit hook `hexagonal-boundaries`

## Hellgate Policy

1. `H-SP1` red → activation 승격 금지
2. `H-SP2` red → query serve enable 금지
3. `H-SP3` red → UDS production listener 금지

## No-Resurrection Rules

1. **frozen contract 변경 금지** — producer sync 없이 `quanta-index-contract` public type/field/variant 수정하지 않는다
2. domain 간 direct import 금지 (`lint-hexagonal-boundaries.py`)
3. contract에 port trait / vendor type 금지
4. adapter에 domain policy 복제 금지
5. legacy core modules (`generation_registry`, `artifact_objects`, `query_serving`, top-level `ports.rs`) 재도입 금지
6. unreadiness empty-hit fallback 금지
7. repo-local `target/`, `state/`, pytest/ruff cache 커밋 금지 (`lint-root-hygiene.sh`)

## First PR Shape (updated)

### PR-1 — control outbound ports

1. `PublishedSearchBundleDeltaApplyPort` impl
2. `PublishedSearchGenerationCatalogPort` impl
3. integration tests (`control_plane.rs`)

### PR-2 — `quanta-index-artifact`

### PR-3 — `quanta-index-lexical` + `quanta-index-semantic` + materialize orchestration

### PR-4 — query engine + `quanta-index-ipc` + `searchd::app` wiring

### PR-5 — `GenerationPinPort` + `U-SP4` proofs

## Done Definition

1. **frozen** `quanta-index-contract`를 producer/query client가 그대로 사용 가능
2. `quanta-index-core/src/domains/*` + adapters가 contract DTO를 end-to-end로 honor
3. boundary lint + semgrep + CI/pre-commit green
4. `searchd::app`만 cross-domain orchestration
5. driven adapters가 outbound port만 구현
6. producer prepare/finalize + UDS query E2E green (contract types unchanged)
7. `quanta-index-core`에 vendor import 없음
8. `U/E/C/H-SP*` scenario matrix green
