# S21-03 — RepoMap Graph Compiler and Resource Envelope

Status: `planned`

Depends on: S21-00, S21-01

## Goal

RepoMap materializer를 permissive transformer에서 validated, typed, bounded graph compiler로 교체한다.

## Root cause

- duplicate nodes와 dangling edges를 ingest boundary에서 검증하지 않음
- node-ref variant를 raw string으로 축약해 graph statistics가 교차 오염됨
- duplicate map insertion이 last-write-wins
- file별 symbol clone/join으로 bounded frame이 초선형 output을 만들 수 있음
- ASCII-only tokenizer가 valid Unicode query를 tokenless global fallback으로 바꿈

## Pipeline

```text
Decoded bundle
 -> Shape and cardinality gate
 -> Typed node table build
 -> Edge referential and variant validation
 -> Canonical graph normalization
 -> Bounded feature aggregation
 -> Immutable search projection
```

각 단계는 input/output byte and item budget을 차감하고 typed refusal을 반환한다.

## Work items

1. `TypedNodeIdentity = variant + domain ID`를 모든 map key에 사용
2. node uniqueness와 edge endpoint existence/allowed variant matrix 검증
3. duplicate/dangling edge에 silent continue/overwrite 금지
4. graph coverage와 exactness claim을 validated inventory에 결속
5. symbol/preview aggregation을 shared immutable representation 또는 streaming builder로 변경
6. node/edge/per-owner-symbol/preview/materialized-byte hard caps
7. worst-case complexity를 코드와 budget contract에 문서화
8. Unicode-aware canonical tokenization을 공통 owner로 이동
9. normalized tokenless query는 typed refusal; global fallback 금지
10. `focus_subjects` owner 결정에 따라 strict scope 또는 explicit hint fallback 구현

## Negative corpus

- same raw ID across File/Symbol/Chunk variants
- duplicate same typed node with same/different body
- missing edge endpoint, illegal edge variant, self-loop policy
- repeated owner path with maximal symbols/previews
- Korean/Japanese/mixed-script query
- punctuation-only and combining-mark-only query
- unresolved focus under strict/hint mode

## Owner files

- `crates/quanta-index-contract/src/repomap.rs`
- `crates/quanta-index-core/src/domains/repomap/`
- `crates/quanta-index-repomap/src/{materializer,model,query}.rs`
- `crates/quanta-index-repomap/tests/`

## Acceptance

- malformed graph causes zero durable mutation
- typed node discriminant is preserved through stats, index, result and explain
- input envelope implies a configured upper bound on materialized bytes and work
- no per-file clone of an unbounded owner symbol vector
- supported Unicode queries are searchable; unsupported tokenless input is refused explicitly
- focus semantics have one documented public meaning

## Verification

- property/golden tests for graph validation and typed identity
- allocation/cardinality counting test and deterministic large synthetic corpus
- RepoMap bounded query and owner flow integration
- runtime ingest resource envelope and multilingual E2E
- benchmark receipt with source/corpus/config/host/RSS provenance

## No patch-on-patch rule

개별 `continue`를 error로 바꾸는 방식으로 끝내지 않는다. 전체 bundle을 durable mutation 전에 검증하는
compiler boundary와 공통 resource ledger를 먼저 만든다.

## Final compiler contract

```rust
RepoMapGraphCompiler::compile(bundle)
    -> Result<CompiledRepoMapCandidateV1, RepoMapCompileRefusalV1>
```

- `CompiledRepoMapCandidateV1`은 canonical node/edge tables, bounded search projection, resource-usage
  receipt, graph/content/schema/profile commitments를 한 번 계산해 보존한다.
- S21-02는 이 타입만 seal한다. raw decoded bundle을 다시 해석하거나 commitment를 재계산하지 않는다.
- `RepoMapMaterializer::materialize` 같은 infallible/partially-valid output API는 삭제한다.

### File-level action list

- `crates/quanta-index-contract/src/repomap.rs`: typed node identity, graph limits, compile refusal, commitment DTO.
- `crates/quanta-index-core/src/domains/repomap/`: compiler port와 budget ledger; adapter type을 core trait에 노출하지 않는다.
- `crates/quanta-index-repomap/src/materializer.rs`: 단계별 validate/build/normalize/aggregate/project; `continue`/overwrite 제거.
- `crates/quanta-index-repomap/src/model.rs`: variant-preserving keys와 immutable shared symbol/preview storage.
- `crates/quanta-index-repomap/src/query.rs`: 공통 Unicode tokenizer와 tokenless typed refusal.
- `crates/quanta-index-repomap/tests/`: negative corpus, worst-case bytes/work, deterministic commitment golden.

### DoD additions

- compile 성공 전 durable write/operation claim이 0이다.
- 모든 refusal은 stage, limit, observed, stable code를 갖고 payload를 누출하지 않는다.
- identical canonical graph는 입력 순서와 map iteration order에 무관하게 동일 commitment를 만든다.
- resource receipt의 observed work가 설정 cap을 넘을 수 없고 overflow는 계산 전에 거부된다.
