# Copy/paste prompt — P02A RepoMap Compiler Lane

당신은 S21-03 lane owner다. 먼저 repo root 기준 `docs/plans/sep-21-search-plane-sota-hardening/tickets/prompts/`
아래 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 읽고 그대로 적용한다. P01 checkpoint commit과 source-bound
handoff를 exact base로 별도 worktree/branch에서 작업한다. P02B와
병렬 실행하되 shared contract/baseline/inventory/generated docs는 수정하지 않고 P02I integration owner에게 delta만
넘긴다.

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

- `crates/quanta-index-core/src/domains/repomap/`
- `crates/quanta-index-repomap/src/{materializer,model,query}.rs`
- `crates/quanta-index-repomap/tests/`
- `crates/quanta-index-contract/src/repomap.rs`는 P02I가 배정한 compiler DTO section만; 배정이 없으면 `BLOCKED`

구현 요구:

- `variant + domain ID` typed node identity를 모든 key/stats/result에 보존
- duplicate/dangling/illegal edge를 durable mutation 전에 전량 검증
- stage별 item/byte/work budget과 overflow-safe accounting
- deterministic canonical normalization과 commitment
- unbounded owner symbol clone/join 제거
- 공통 Unicode tokenizer; normalized tokenless input은 typed refusal
- non-empty `focus_subjects`는 전부 strict resolve하고 하나라도 없으면 `FOCUS_SUBJECT_NOT_FOUND`; global fallback 0
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

proof node는 `p02a-repomap-compiler`다. 최종 handoff `docs/plans/sep-21-search-plane-sota-hardening/tickets/handoffs/P02A.json`에 source/dirty digest, 변경 파일,
frozen compiler API, commitment inputs, refusal codes, resource limits, command/counts, NOT_RUN, P03 소비 fixture를
남겨라. explicit owner path만 단일 checkpoint commit으로 만들고 push는 별도 요청 시에만 한다.
