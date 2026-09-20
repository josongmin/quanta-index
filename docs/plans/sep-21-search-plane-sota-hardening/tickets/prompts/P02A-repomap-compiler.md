# Copy/paste prompt — P02A RepoMap Compiler Lane

당신은 S21-03 lane owner다. P01이 DONE이고 canonical identity API가 merge된 source에서 작업한다. P02B와 병렬
실행 가능하지만 시작 전에 shared contract/baseline 파일의 단일 writer를 합의하라. 충돌 가능 파일을 양쪽에서
동시에 수정하지 마라.

읽을 문서:

- repo instructions
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-AUDIT.md`
- `docs/plans/sep-21-search-plane-sota-hardening/tickets/S21-03-repomap-graph-compiler-and-resource-envelope.md`
- S21-00 identity/resource decisions와 S21-01 handoff

목표: RepoMap materializer를 permissive transformer에서 whole-bundle validated, typed, bounded compiler로 교체한다.

필수 API:

```rust
RepoMapGraphCompiler::compile(bundle)
    -> Result<CompiledRepoMapCandidateV1, RepoMapCompileRefusalV1>
```

write scope:

- `crates/quanta-index-contract/src/repomap.rs` 중 사전 배정받은 compiler DTO section
- `crates/quanta-index-core/src/domains/repomap/`
- `crates/quanta-index-repomap/src/{materializer,model,query}.rs`
- `crates/quanta-index-repomap/tests/`

구현 요구:

- `variant + domain ID` typed node identity를 모든 key/stats/result에 보존
- duplicate/dangling/illegal edge를 durable mutation 전에 전량 검증
- stage별 item/byte/work budget과 overflow-safe accounting
- deterministic canonical normalization과 commitment
- unbounded owner symbol clone/join 제거
- 공통 Unicode tokenizer; normalized tokenless input은 typed refusal
- `focus_subjects` frozen strict/hint semantics를 정확히 구현
- compiled output에 graph/content/schema/profile commitments와 resource receipt 포함
- raw bundle을 S21-02가 재해석할 수 없도록 API 경계 설정

금지: invalid entry `continue`, last-write-wins, infallible partial output, input 순서/map iteration에 따라 변하는
commitment, route-local tokenizer.

DoD:

- refusal 전 durable write와 operation claim 0
- negative corpus 전부 typed refusal: cross-variant same raw ID, duplicate, missing endpoint, illegal variant,
  self-loop policy, amplification, multilingual/tokenless
- identical canonical graph의 commitment가 입력 순서에 무관
- configured cap이 materialized bytes/work/RSS upper bound를 설명
- independent golden/property/allocation proof와 owning integration proof

최종 handoff에 source/dirty digest, 변경 파일, frozen compiler API, commitment inputs, refusal codes, resource
limits, command/counts, NOT_RUN, P03 소비 fixture를 남겨라. commit/push는 요청 시에만 한다.
