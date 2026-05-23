# Storage Architecture Endgame Implementation Plan

Status: `Canonical implementation plan`

## Scope

이 문서는 storage RFC를 실제 코드 구조로 내리는 구현 순서를 고정한다.

핵심 전제:

1. `Phase 1`은 hexagonal abstraction + LMDB backend
2. `Phase 2`는 custom `mmap packed segment`
3. storage SSOT는 이 레포 내부에 유지

## Final Verdict

### Keep

1. `ChangeEventIncrementalIndexingRailV1`
2. `indexing_machine_v2` SQLite WAL control plane
3. `PublishedManifestStoreV1`
4. query/runtime public ids and public rail naming

### Replace

1. `ArtifactStore<Vec<u8>>` as primary artifact persistence API
2. reasoning graph `sync -> async artifact store` dispatch bridge
3. structured graph docs as traversal authority

## Current Integration Seams

### 1. QueryExecutor artifact write

- `persist_prepared_execution_v1()` writes memo outputs into generic artifact store
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/query_executor/execute_core_store.rs:259`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/query_executor/execute_core_store.rs:259>)
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/query_executor/execute_core_store.rs:310`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/query_executor/execute_core_store.rs:310>)

### 2. QueryExecutor artifact read

- `serve_memo_hit_output()` loads raw bytes and decodes them
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/artifact_codec.rs:195`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/artifact_codec.rs:195>)

### 3. QueryExecutor constructor surface

- runtime generic constructor takes `artifact_store: Arc<A>`
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/query_executor/constructors.rs:61`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/query_executor/constructors.rs:61>)

### 4. Reasoning graph storage surface

- contract:
  - [`packages/analysis/quanta-v2/crates/quanta-core-contract/src/ports/reasoning_graph_store_port.rs:29`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-core-contract/src/ports/reasoning_graph_store_port.rs:29>)
- current impl:
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/adapters/reasoning_graph_runtime/artifact_backed.rs:23`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/adapters/reasoning_graph_runtime/artifact_backed.rs:23>)
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/adapters/reasoning_graph_runtime/coordinator.rs:41`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/adapters/reasoning_graph_runtime/coordinator.rs:41>)

### 5. Published graph read surface

- current published graph reader pins manifest and materializes `Vec<IndexProjectionDocV1>`
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/published_surfaces.rs:72`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/published_surfaces.rs:72>)
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/structured_store.rs:970`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/structured_store.rs:970>)

### 6. Publish/bundle seam

- current best seam is `prepare_commit_publish_v1()`
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/commit_prepare.rs:51`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/commit_prepare.rs:51>)
- current publish finalize side effects happen in `apply_publish_store_mutations_v1()`
  - [`packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/commit_finalize/publish_store_mutations.rs:71`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/commit_finalize/publish_store_mutations.rs:71>)

## Canonical Crate Split

```text
packages/analysis/quanta-v2/crates/
  quanta-contract-storage/
    src/
      object_store.rs
      retention.rs
      graph_segments.rs
      ids.rs
      manifest_registry.rs

  quanta-storage-in-memory/
    src/
      object_store.rs
      graph_segments.rs

  quanta-storage-lmdb/
    src/
      object_store.rs
      retention.rs
      index_tables.rs
      env.rs

  quanta-storage-segment/
    src/
      object_segments/
      graph_segments/
      compaction/
      inspect/
      verify/

  quanta-storage-runtime-adapters/
    src/
      query_executor_storage.rs
      reasoning_graph.rs
      published_graph.rs
```

## Interface Set

### 1. Object storage

```rust
pub trait ArtifactObjectStorePort: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;
    type ReadView<'a>: AsRef<[u8]> + 'a
    where
        Self: 'a;

    fn get_view<'a>(
        &'a self,
        artifact_ref: &'a ArtifactRef,
    ) -> BoxFuture<'a, Result<Option<Self::ReadView<'a>>, Self::Error>>;

    fn put_canonical(
        &self,
        bytes: Vec<u8>,
        kind: ArtifactKind,
        schema_version: SchemaVersion,
        codec: Codec,
    ) -> BoxFuture<'_, Result<ArtifactPublishOutcome, Self::Error>>;

    fn exists<'a>(
        &'a self,
        artifact_ref: &'a ArtifactRef,
    ) -> BoxFuture<'a, Result<bool, Self::Error>>;
}
```

계약 원칙:

1. 이 포트가 `Phase 1`과 `Phase 2` 공통 hot-path read surface다
2. `LMDB` adapter는 이 포트를 만족해야 하며 `ArtifactStore::get() -> Vec<u8>` 형태를 새 hot path에 남기지 않는다
3. `Phase 2` 자체엔진으로 넘어가도 이 포트 방향은 유지된다

### 2. Rollback / delete

```rust
pub trait ArtifactRollbackPort: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn delete_created(
        &self,
        artifact_hash: &ArtifactHash,
    ) -> Result<DeleteCreatedOutcome, Self::Error>;
}
```

### 3. Retention / GC

```rust
pub enum RetentionRoot {
    Snapshot(SnapshotId),
    PublishedGeneration(ManifestGeneration),
    ReasoningHead {
        namespace: String,
        state_ref: ArtifactRef,
    },
    Session(StorageSessionId),
}

pub trait ArtifactRetentionPort: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn pin_root(&self, root: &RetentionRoot)
        -> Result<(), Self::Error>;

    fn release_root(&self, root: &RetentionRoot)
        -> Result<(), Self::Error>;

    fn sweep_unpinned(&self) -> Result<SweepStats, Self::Error>;
}
```

### 4. Manifest registry

```rust
pub trait ManifestRegistryPort: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn register_generation(
        &self,
        request: ManifestGenerationRegistration,
    ) -> Result<ManifestGenerationRecord, Self::Error>;

    fn pin_latest(&self) -> Result<ManifestPin, Self::Error>;

    fn pin_generation(
        &self,
        generation: ManifestGeneration,
    ) -> Result<ManifestPin, Self::Error>;

    fn activate_generation(
        &self,
        generation: ManifestGeneration,
    ) -> Result<(), Self::Error>;
}
```

### 5. Graph topology writer

```rust
pub trait GraphTopologyWriterPort: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn write_generation(
        &self,
        request: GraphTopologyWriteRequest,
    ) -> Result<GraphTopologyGenerationBundle, Self::Error>;
}
```

### 6. Graph topology reader

```rust
pub trait GraphTopologyReaderPort: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn open_generation(
        &self,
        generation: &GraphTopologyGenerationBundle,
    ) -> Result<Box<dyn GraphTopologyCursorPort>, Self::Error>;
}
```

## Identity Model

이 계획은 ID를 3계층으로 나눈다.

```rust
pub struct EntityLogicalId(pub [u8; 32]);
pub struct RevisionNodeId(pub [u8; 32]);
pub struct PhysicalNodeId(pub u32);
```

원칙:

1. `EntityLogicalId`
   - file/module/top-level symbol/import/export 같은 비교적 stable entity
2. `RevisionNodeId`
   - revision/snapshot scoped local HIR node
3. `PhysicalNodeId`
   - packed segment 내부 traversal id

금지:

1. `file_digest`를 stable logical id 핵심축으로 직접 쓰는 것
2. local HIR node에 revision을 넘어가는 강한 stable id를 억지로 부여하는 것

## Hot-Path Genericity Policy

1. `QueryExecutor`, reasoning graph coordinator, published graph reader는 backend에 대해 generic 유지
2. hot path에서 `dyn ArtifactObjectStorePort` 같은 trait object 금지
3. `StorageBackendFactory`는 concrete backend를 선택하고 fully-wired runtime handle을 만든다
4. borrowed read capability를 유지해야 하므로 object safety 때문에 hot-path surface를 후퇴시키지 않는다
5. app-level payload cache miss/fallback 계층은 도입하지 않는다
6. `LMDB`는 primary read substrate로 직접 붙고 residency는 OS page cache에 맡긴다

## Scenario Matrix

이 문서는 아래 scenario ids를 구현/검증 단위로 사용한다.

### Usecase

1. `U1 Cold restart memo reuse`
   - owner:
     - `QueryExecutor`
     - `ArtifactObjectStorePort`
   - proof:
     - restart 후 same snapshot / same args memo hit
     - source re-read / HIR rebuild 없이 artifact reuse
2. `U2 Published generation pin under concurrent prepare`
   - owner:
     - `ManifestRegistryPort`
     - `ArtifactRetentionPort`
   - proof:
     - `G1` pin reader가 `G2` prepare 동안 끝까지 `G1`만 본다
     - `G1` release 전 sweep 금지
3. `U3 Reasoning graph restore after restart`
   - owner:
     - reasoning graph runtime adapter
     - object store
   - proof:
     - restart 후 state snapshot / fragment lookup restore
     - stale-write CAS 유지
4. `U4 Incremental small edit with old roots retained`
   - owner:
     - retention roots
     - incremental control plane integration
   - proof:
     - new writes가 old roots를 손상시키지 않는다
     - retained roots가 있는 object는 sweep되지 않는다

### Edge

1. `E1 Duplicate canonical put`
   - owner:
     - object store publish path
     - rollback delete path
   - proof:
     - repeated put does not duplicate physical object
     - rollback delete does not remove shared artifact
2. `E2 Multi-root retention overlap`
   - owner:
     - retention root accounting
   - proof:
     - one root release 후에도 remaining root가 있으면 object 보존
3. `E3 Stale manifest pin / activation request`
   - owner:
     - manifest registry
   - proof:
     - stale pin/activate reject
     - active head unchanged

### Corner

1. `C1 Empty or tiny graph shard`
   - owner:
     - graph segment writer/reader
   - proof:
     - empty shard open succeeds
     - footer/offset invariants hold
2. `C2 High-fanout node`
   - owner:
     - graph topology cursor
   - proof:
     - no offset overflow
     - no full materialization requirement
3. `C3 Physical repack without logical drift`
   - owner:
     - graph compaction
     - logical/physical mapping
   - proof:
     - logical query result parity across repack

### Hellgate

1. `H1 Crash between segment write and registry swap`
   - owner:
     - segment writer
     - manifest registry
   - proof:
     - incomplete segment reject
     - last committed registry head preserved
2. `H2 Corrupted footer or checksum mismatch`
   - owner:
     - segment reader/open path
   - proof:
     - fail-closed open reject
     - no best-effort recovery on active path
3. `H3 Rollback delete races with shared reachability`
   - owner:
     - rollback delete
     - retention accounting
   - proof:
     - only truly-created object deleted
4. `H4 Graph authority split-brain during cutover`
   - owner:
     - shadow parity gate
   - proof:
     - parity red blocks authority promotion
5. `H5 Sweep under mixed roots`
   - owner:
     - retention sweep
     - manifest/session/reasoning root accounting
   - proof:
     - deterministic reachability
     - no premature delete
     - no immortal leaked objects

## Phase 0: Spec Freeze

목표:

1. storage contract namespace 고정
2. old/new owner boundary 고정
3. blast radius 목록 고정
4. retention root taxonomy 고정
5. manifest registry contract 고정
6. hot-path genericity policy 고정

수정 파일:

1. `docs/plans/may-23-storage-architecture-endgame/*`
2. optional `docs/ssot/*` cross-link only when implementation starts

산출물:

1. crate tree
2. port signatures
3. generation/retention model
4. test matrix
5. workspace membership policy
6. shadow parity gate
7. scenario matrix

## Phase 1: Hex Ports + LMDB Adapter

### Goal

`LMDB`는 최종 backend가 아니라:

1. production-grade disk-backed adapter
2. port semantics 검증기
3. later segment cutover를 위한 backend isolation

역할만 맡는다.

추가 원칙:

1. `LMDB` 단계도 mmap-native read surface를 그대로 쓴다
2. `LMDB` 채택을 이유로 owned-bytes artifact API를 보존하지 않는다
3. `Phase 1`에서 만든 hot-path port shape는 `Phase 2` 자체엔진 선행작업이다

### Phase-1.1 Contract Crate

새 crate:

- `quanta-contract-storage`

작업:

1. `ArtifactObjectStorePort`
2. `ArtifactRollbackPort`
3. `ArtifactRetentionPort`
4. `ManifestRegistryPort`
5. `GraphTopology*` contracts
6. `EntityLogicalId / RevisionNodeId / PhysicalNodeId`

기존 legacy residue:

- `ArtifactStore`
- `ArtifactGcPort`

처리:

1. breaking-first 기준으로 new runtime hot path는 new storage ports만 사용
2. compat residue는 owner-local test scaffolding 또는 bounded bridge로만 허용
3. compat residue를 production hot path에 남기지 않는다

### Phase-1.2 In-Memory Adapter

새 crate:

- `quanta-storage-in-memory`

역할:

1. test harness
2. unit tests
3. no-disk fixtures

이유:

- 지금 test harness들이 `InMemoryArtifactStore::new()`에 강하게 묶여 있다
  - 예: [`packages/analysis/quanta-v2/crates/quanta-runtime/tests/support/harness_orchestrator.rs:447`](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/tests/support/harness_orchestrator.rs:447>)

phase gate:

1. `U1`
2. `E1`
3. `E2`

### Phase-1.3 LMDB Adapter

새 crate:

- `quanta-storage-lmdb`

역할:

1. `ArtifactObjectStorePort`
2. `ArtifactRollbackPort`
3. `ArtifactRetentionPort`

하지 않을 것:

1. graph topology CSR/CSC
2. graph published reader cutover

phase gate:

1. `U1`
2. `U2`
3. `E1`
4. `E2`
5. `E3`
6. `H3`

### Phase-1.4 QueryExecutor Cutover

수정 파일:

1. `packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/query_executor/constructors.rs`
2. `packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/query_executor/execute_core_store.rs`
3. `packages/analysis/quanta-v2/crates/quanta-runtime/src/executor/artifact_codec.rs`

변경:

1. generic bound를 `ArtifactStore`에서 `ArtifactObjectStorePort`로 교체
2. rollback bound를 `ArtifactGcPort`에서 `ArtifactRollbackPort`로 교체
3. `get() -> Vec<u8>`를 `get_view()`로 교체
4. decode rail은 borrowed bytes를 허용하도록 조정

의미:

1. `LMDB` 단계에서도 direct object-store read를 primary path로 사용
2. app-level payload cache layer는 추가하지 않음
3. 이 cutover 자체가 `Phase 2` packed segment용 선행작업

phase gate:

1. `U1`
2. `E1`
3. `H3`

### Phase-1.5 Bootstrap / Harness Factory Cutover

수정 파일:

1. `packages/analysis/quanta-v2/crates/quanta-pyo3/src/client/helpers/executor.rs`
2. `packages/analysis/quanta-v2/crates/quanta-runtime/tests/support/**`
3. `packages/analysis/quanta-v2/crates/quanta-bench/tests/common/**`

변경:

1. `InMemoryArtifactStore` concrete alias 제거
2. `StorageBackendFactory` 도입
3. prod/dev/test backend 선택을 wiring layer로 이동

정책:

1. production wiring은 `LMDB`를 primary substrate로 직접 선택
2. fallback cache 계층을 bootstrap에서 추가하지 않는다

### Phase-1.6 Reasoning Graph on New Object Store

수정 파일:

1. `packages/analysis/quanta-v2/crates/quanta-runtime/src/adapters/reasoning_graph_runtime/coordinator.rs`
2. `packages/analysis/quanta-v2/crates/quanta-runtime/src/adapters/reasoning_graph_runtime/artifact_backed.rs`

변경:

1. generic CAS object persistence는 유지
2. backend만 new object store로 교체
3. stale-write CAS / head pointer semantics 유지

하지 않을 것:

1. 이 단계에서 graph topology CSR/CSC 구현
2. public `ReasoningGraphStorePort` shape 변경

phase gate:

1. `U3`
2. `U4`
3. `H5`

### Phase-1.7 Existing Graph Reader Keep-As-Is

이 단계에서는:

- `PublishedStructuredGraphSearchPortV1`
- `StructuredProjectionBackendStoreV1`

를 유지한다.

이유:

1. Phase 1은 storage object abstraction 단계
2. graph topology reader cutover는 Phase 2 책임

## Phase 2: Custom mmap Packed Segment

### Goal

1. LMDB prod default 제거
2. custom object segment + graph topology segment 도입

### Phase-2.1 Segment Object Store

새 crate:

- `quanta-storage-segment`

하위 모듈:

1. `object_segments`
2. `object_index`
3. `retention`
4. `inspect`
5. `verify`

형식:

1. immutable segment files
2. side index mmap files
3. generation pin/sweep

핵심:

1. `Phase 1`에서 이미 도입한 mmap-native read surface를 backend만 segment로 교체한다
2. 이 단계는 read contract 재설계가 아니라 physical layout / locality / compaction 교체다

필수 포맷 규약:

1. header에 `magic / schema_version / segment_kind / segment_id / checksum_kind`
2. footer에 `offset table / object count / complete marker / footer checksum`
3. footer 검증 전 segment는 invalid
4. manifest registry는 sealed segment만 참조 가능
5. compaction publish는 `new segment set write -> registry swap -> old segment retire` 순서
6. corruption / partial write / checksum mismatch는 fail-closed open reject

phase gate:

1. `H1`
2. `H2`
3. `H5`

### Phase-2.2 Graph Topology Segment

형식:

1. `out_offsets.bin`
2. `out_edges.bin`
3. `in_offsets.bin`
4. `in_edges.bin`
5. `entity_logical_to_physical.bin`
6. `revision_logical_to_physical.bin`
7. `node_columns.bin`
8. `edge_columns.bin`

정렬:

1. `(src_physical, edge_kind, dst_physical)`
2. reverse side는 별도 CSC 유지

phase gate:

1. `C1`
2. `C2`

### Phase-2.3 Delta Overlay

구성:

1. base packed segment
2. delta segments
3. tombstone bitmaps
4. overlay read order in manifest

정책:

1. delta depth budget
2. tombstone ratio threshold
3. background repack trigger

phase gate:

1. `U4`
2. `C3`
3. `H5`

### Phase-2.4 Shadow Parity Gate

목표:

1. structured docs authority와 segment authority를 같은 generation에서 동시에 산출
2. graph exact-hit / graph_path / basic traversal parity 검증
3. parity green 이후에만 segment authority 승격

금지:

1. parity proof 없이 direct authority flip
2. docs authority와 segment authority를 장기간 병행 SSOT로 두는 것

phase gate:

1. `H4`
2. `C3`

### Phase-2.5 Reasoning Graph Unification

새 구현:

- `SegmentBackedReasoningGraphStore`
- `SegmentBackedQueryToFragmentLookup`

목표:

1. artifact-backed state snapshot CAS를 segment storage로 내림
2. sync-over-async dispatch bridge 제거
3. high-level `ReasoningGraphStorePort`는 유지

### Phase-2.6 Published Graph Reader Cutover

새 구현:

- `PublishedSegmentGraphSearchPort`

수정 파일:

1. `packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/published_surfaces.rs`
2. graph reader consumers

변경:

1. `published_docs_for_generation_v1()` full-doc restore를 graph authority에서 제거
2. manifest pin 후 graph segment refs를 열도록 변경

phase gate:

1. `U2`
2. `C1`
3. `C2`
4. `H4`

## Detailed Integration Points

### A. QueryExecutor

현재:

1. `persist_prepared_execution_v1()`가 CAS write owner
2. `serve_memo_hit_output()`가 CAS read owner

최종:

1. `query_executor_storage.rs`가 new storage port 호출을 캡슐화
2. `QueryExecutor`는 storage backend concrete type을 모름

### B. Reasoning Graph

현재:

1. `ArtifactBackedReasoningGraphCoordinator`
2. `ArtifactBackedReasoningGraphStore`

최종:

1. `SegmentBackedReasoningGraphStore`
2. `SegmentBackedQueryToFragmentLookup`
3. `ReasoningGraphStorePort` surface 유지

### C. Published Graph Search

현재:

1. `PublishedStructuredGraphSearchPortV1`
2. manifest pin
3. full doc vector restore

최종:

1. `PublishedSegmentGraphSearchPort`
2. manifest pin
3. segment bundle open
4. topology cursor traversal

## Compile Fallout

Workspace policy:

1. 새 crate는 `packages/analysis/quanta-v2/crates/quanta-storage-*` immediate child로만 추가
2. root workspace 새 top-level package 추가 금지
3. initial admission은 `packages/analysis/quanta-v2/Cargo.toml`의 `members` only
4. `default-members` 승격은 owner-local compile proof 뒤에만 허용

Phase 1에서 예상되는 compile fallout:

1. `ArtifactStore` generic bound를 가진 runtime/executor 타입 alias 전반
2. `InMemoryArtifactStore`를 concrete type으로 박아둔 PyO3 helper
3. `InMemoryArtifactStore::new()`를 직접 쓰는 test harness/benches
4. reasoning graph runtime generic bound

Phase 2에서 예상되는 compile fallout:

1. published graph search port consumers
2. graph-path / dependency reasoning readers

## Verification Plan

### Phase 1

1. object store parity tests
2. rollback delete tests
3. retention root pin/release/sweep tests
4. LMDB reopen/durability tests
5. QueryExecutor memo hit/write/read tests
6. reasoning graph persist/restore/stale-write tests
7. harness/bootstrap compile proofs
8. scenario gates:
   - `U1`
   - `U2`
   - `U3`
   - `U4`
   - `E1`
   - `E2`
   - `E3`
   - `H3`
   - `H5`

### Phase 2

1. segment inspect/verify tests
2. graph topology parity tests
3. overlay depth / tombstone / compaction tests
4. manifest generation pin correctness tests
5. shadow parity gate proofs
6. scenario gates:
   - `C1`
   - `C2`
   - `C3`
   - `H1`
   - `H2`
   - `H4`
   - `H5`

## Hellgate Policy

아래는 green 아니면 cutover 금지다.

1. `H1` red면 segment backend publish 금지
2. `H2` red면 segment reader enable 금지
3. `H3` red면 new object store를 `QueryExecutor` hot path에 올리지 않는다
4. `H4` red면 published graph authority flip 금지
5. `H5` red면 retention sweep 자동화 금지

## No-Resurrection Rules

1. new runtime code에서 `InMemoryArtifactStore::new()` 직접 호출 금지
2. new hot path에서 `ArtifactStore::get() -> Vec<u8>` 복사 재도입 금지
3. graph traversal authority에 `published_docs_for_generation_v1()` 재사용 금지
4. rollback delete semantics를 retention sweep로 대체 금지
5. physical segment packing이 logical identity에 영향을 주게 만들기 금지
6. scenario gates를 우회하고 owner-local manual override로 cutover 승인 금지
7. `LMDB` 단계라는 이유로 app-level payload cache fallback을 새로 도입하는 것 금지

## First PR Shape

### PR-1

1. `quanta-contract-storage`
2. `quanta-storage-in-memory`
3. owner-local test bridge only when compile bootstrap requires it

### PR-2

1. `QueryExecutor` storage port cutover
2. rollback / retention contract cutover

### PR-3

1. PyO3/bootstrap/harness factory cutover
2. LMDB adapter
3. reasoning graph on new object store

### PR-4

1. `quanta-storage-segment` object tier
2. LMDB prod default -> segment prod default

### PR-5

1. graph topology segment
2. shadow parity gate
3. `PublishedSegmentGraphSearchPort`

## Done Definition

storage RFC closeout은 아래를 만족해야 한다.

1. runtime hot path가 backend concrete type을 모르며 port only로 동작
2. prod backend가 in-memory/generic CAS가 아님
3. rollback delete와 retention sweep가 분리된 계약으로 검증됨
4. reasoning graph가 dispatch bridge 없이 storage backend를 직접 사용
5. published graph read가 full doc vector materialization에 의존하지 않음
6. shadow parity gate 없이 graph authority가 승격되지 않음
7. generation pin / retention / manifest registry가 fail-closed로 검증됨
8. `U* / E* / C* / H*` scenario matrix가 owner-local proof rail로 green
9. `Phase 1` hot-path storage surface가 `Phase 2` 자체엔진으로 그대로 carry 가능한 mmap-native contract임이 증명됨
