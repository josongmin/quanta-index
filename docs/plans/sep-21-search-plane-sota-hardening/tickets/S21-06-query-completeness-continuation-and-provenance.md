# S21-06 — Query Completeness, Continuation, Ranking, and Provenance

Status: `planned`

Depends on: S21-00, S21-05

## Goal

query 결과의 pagination, completeness, ranking, explain, availability가 실제 실행과 동일한 의미를
갖도록 공통 execution outcome contract를 만든다.

## Root cause

- cursor가 full pin/canonical query에 결속되지 않음
- nested cap과 outer cap의 중복 contract가 route별로 다르게 해석됨
- `Capped`/partial 상태를 row count 기반 window 계산이 버림
- fusion/dedup/window가 서로 다른 identity를 사용
- contribution rank가 실제 RRF rank와 다름
- engines executed와 lanes contributed를 혼동
- `examined == 0`을 unavailable로 추정
- explain mismatch와 zero digest를 success payload에 숨김

## Canonical types

- `CanonicalRequestIdentityV1`: route + full generation pin + normalized query/constraints/order/cap + aux epochs
- `ContinuationTokenV2`: canonical request digest + read identity + last typed ordering key + expiry/version + integrity proof
- `ExecutionOutcomeV2`: `ExactExhausted`, `LowerBound { continuation }`, `CappedUnknown { cap }`,
  `InterruptedPartial { reason }`, `Approximate { method, quality_contract }`
- unsupported/unavailable/not-ready는 success outcome이 아니라 stable typed refusal
- `CoverageV1`: examined universe, lower bound, exhaustion proof, backend coverage
- `LaneTraceV1`: executed, contributed, filtered, candidate count, cost, model/profile
- `ContributionTraceV1`: typed candidate identity + compact lane rank + score inputs

## Work items

1. cursor encoding/validation을 공통 owner로 통합
2. client-editable seek boundary와 continuation을 API에서 분리
3. 중첩 `top_k` 제거 또는 exact equality/range validation
4. dense admission outcome을 fusion/window까지 보존
5. typed candidate identity를 fusion/dedup/universe/window 전체에 사용
6. dedup 이후 compact rank를 한 owner가 RRF와 provenance에 동시에 공급
7. executed engine과 contributing lane 분리
8. availability는 sealed inventory/readiness에서만 판정
9. explain reconciliation mismatch를 typed status/refusal로 변경
10. zero digest sentinel 제거, typed absence 또는 real digest 사용
11. empty/partial/approximate/exhausted public semantics 문서화

## Negative matrix

- cursor cross repo/revision/route/query/order/cap/aux epoch reuse
- token tamper/expiry/version mismatch
- dense exact filter with refill ceiling reached
- duplicate raw rows before B candidate
- same raw ID with different owner kind/corpus
- zero-hit executed lane
- available-empty vs filtered-empty vs unavailable
- nested cap invalid and in-range mismatch
- explain candidate/rank/score/digest mismatch

## Owner files

- contract cursor/query/result/explanation DTOs
- `crates/quanta-index-core/src/domains/hybrid/`
- `crates/quanta-index-search-plane/src/query_dispatcher/{dense_admission,window,semantic_query}.rs`
- query routes and explain route
- searchctl/harness renderers

## Acceptance

- a continuation is valid for exactly one canonical request/read identity
- `Capped` or unknown coverage can never become `Exact`
- `has_more=false` requires an exhaustion proof
- score is independently reproducible from carried contribution trace
- zero-hit still records executed backend and cost
- unavailable is never inferred from absence of hits
- typed reconciliation mismatch cannot be parsed only from debug string

## Verification

- owner-local property tests for cursor and ordering identity
- independent reference RRF/window oracle
- raw-wire cap/tamper cases
- `e2e_keyset_cursors`, `e2e_exact_count_window`, hybrid filters, explain trace, restart determinism
- public API and fuzz smoke

## No patch-on-patch rule

`Capped => has_more=true` 같은 route-local 보정은 금지한다. lane outcome, fusion identity, cursor, explain
schema를 공통 contract로 바꾼다.

## Final route contract

- pageable: lexical, symbol, history, runtime, structural. continuation은 canonical request/read identity와
  last typed ordering key에 결속한다.
- bounded top-k: semantic, hybrid, hybrid-seed, RepoMap. pagination을 암시하지 않고 coverage/exhaustion을
  별도로 보고한다.
- returned `< top_k`라는 관찰만으로 exact/exhausted를 만들지 않는다. 내부 cap, ANN, refill ceiling,
  cancellation이 하나라도 있으면 해당 typed outcome을 보존한다.

### File-level action list

- `crates/quanta-index-contract-base/src/results/query_window.rs`: `QueryResultWindowV2`, mandatory completeness,
  exhaustion proof, continuation invariants.
- `crates/quanta-index-core/src/domains/semantic/dense_admission.rs`: admission outcome과 examined/refill ceiling 보존.
- `crates/quanta-index-core/src/domains/hybrid/`: typed candidate identity와 compact post-dedup rank를 RRF/trace가 공유.
- `crates/quanta-index-search-plane/src/query_dispatcher/{dense_admission,window,semantic_query}.rs`와
  `crates/quanta-index-search-plane/src/query_dispatcher/routes/{hybrid,hybrid_seed}.rs`:
  cap/partial 상태를 window까지 손실 없이 전달.
- 모든 cursor codec/validator: route, full pin, normalized query/constraints/order/cap, aux epochs, read identity,
  expiry/version/integrity proof를 digest에 포함.
- explanation/dispatcher/harness: `engines_executed`와 `lanes_contributed`, availability와 zero-hit을 분리.

### DoD additions

- `has_more=false`는 해당 route universe에 대한 verifiable exhaustion proof 없이는 생성 불가하다.
- `CappedUnknown`, `InterruptedPartial`, `Approximate`가 outer window에서 `Exact`로 승격되는 변환이 없다.
- 독립 reference oracle이 fusion 순서, compact rank, score trace, cursor next boundary를 재계산한다.
- cursor field 하나를 바꾼 모든 negative vector가 decode/binding에서 mutation/query 실행 전에 거부된다.
