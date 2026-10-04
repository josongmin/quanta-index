# E4 — 인덱싱·typo 실행 비용·release 성능·scale

- 상태: `PLANNED`. 구현·모델 실행·제품 캡처·verification은 이 계획 작성에서 `NOT_RUN`.
- 담당: E4 담당 1명.
- 7개 티켓. [전체 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md).

## 목적

현재 lexical lifecycle과 전체 query 비용의 병목을 먼저 측정하고, output·durability·resource 계약을 지키는 구조 변경만 채택한 뒤 정식 반복 성능과 scale을 판정한다.

## 배경과 현재 구현

- index_store는 file sync→rename→parent sync를 수행하고 coverage/source roots가 참조 artifact 뒤에 공개된다. 격리182artifact directory-barrier 실험은 엔진 개선·crash proof가 아니다.
- ASCII scanner A/B는 결과 동등성을 보였지만 deletion whole-call은 혼잡한 host에서 평균+8.75%였다. 현재 효과는 미적격이다.
- trigram shortlist·OSA cache·source authority·ingest observations·scale/open-loop harness는 있다. 새 token index/pack/harness는 자동 선행 과제가 아니다.
- 과거4096 timeout·32768 posting-cap refusal,256제한 성공은 전체 release scale 성공이 아니다. sampled RSS와 logical storage bytes를 실제 peak/physical IO로 해석하지 않는다.
- default typo23 ordinary와 weak NL/semantic은 policy·candidate·relevance 잔여다. explicit4363/4363과2query semantic/hybrid pilot은 그 잔여의 해결 증거가 아니다.

## 목표 계약과 변경 원칙

- full/delta/delete/no-op/reopen→commit/seal/activate와 query를 실제 자식 clock·CPU·RSS·storage high-water로 분해한다.
- batch durable writer를 채택하면 content sync/rename·inherited hardlink dirs barrier→root/manifest publish+barrier→cleanup 순서를 하나의 canonical authority에서 보존한다.
- ASCII/token 최적화는 whole-call output parity·exhaustive OSA1 witness·cursor/budget/cancel·ingest/open/residency 비용을 함께 판단한다.
- 정식 성능은 동일 release/source/input/response boundary, 사전 반복·효과/불확실성 기준, host admission·직렬 실행·fresh roots에서 발행한다.
- typo default/NL/semantic 변경은 E1 독립 gold/holdout·pool exposure RCA와 candidate/ranking/model/chunking ablation 이후에만 한다.

## 해야 할 일

1. actual lifecycle phase attribution과 serial slot/durable coordinator scope를 현재 source에서 확인한다.
2. directory sync 지배 비용이 확인된 경우 canonical group barriers를 구현하고 syscall fault+process crash+reopen oracle를 실행한다.
3. ASCII scanner를 허용된 host에서 whole-call 재측정해 유지/수정/철회를 결정한다.
4. 반복 source token scan이 계속 지배하면 source/generation-bound distinct raw token/witness authority를 도입하되 completeness 전 complete fallback을 보존한다.
5. matching release binaries로 scale256→4096→32768와 open-loop/full/delta/delete/reopen/restart를 별도 관측한다.
6. 선택한 source epoch·E1 admission·E2 scope/completed clocks/Semble attribution에서 정식 반복 성능을 수행한다. single-RPC/token/pack은 채택한 개선 효과 claim에만, 대규모 tier 결과는 해당 capacity/tail/restart claim에만 요구한다.
7. E1 final qrels+미사용 holdout 이후 default23·Gin4·NL/semantic misses를 분류하고 필요한 정책 수정만 독립 fixture·holdout에서 판정한다.

## 티켓 실행 순서

| 티켓 | 작업 | 우선순위 / 종류 | 선행 결과 |
| --- | --- | --- | --- |
| [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md) | 인덱싱 lifecycle 비용·resource 원인 분해 | P1 / `EXECUTION_AND_PROOF` | 즉시 조사·fixture 준비 가능 |
| [O4-E4-02](../tickets/O4-E4-02-generation-durable-barriers.md) | generation durable publication의 그룹 barrier | P1 / `CONDITIONAL_CODE` | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md) |
| [O4-E4-03](../tickets/O4-E4-03-ascii-scanner-decision.md) | ASCII scanner 전체 호출 효과 판정 | P1 / `EXECUTION_THEN_CONDITIONAL_CODE` | 즉시 조사·fixture 준비 가능 |
| [O4-E4-04](../tickets/O4-E4-04-source-token-authority.md) | source-bound distinct token authority | P2 / `CONDITIONAL_CODE` | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md), [O4-E4-03](../tickets/O4-E4-03-ascii-scanner-decision.md) |
| [O4-E4-05](../tickets/O4-E4-05-release-scale-load.md) | matching release scale·load·restart 실행 | P2 / `EXECUTION_AND_PROOF` | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md), [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md) |
| [O4-E4-06](../tickets/O4-E4-06-qualified-performance.md) | 동일 응답 경계의 정식 반복 성능 | P1 / `EXECUTION` | [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md), [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md), [O4-E2-01](../tickets/O4-E2-01-native-completed-timer.md), [O4-E2-02](../tickets/O4-E2-02-external-index-universe.md), [O4-E2-05](../tickets/O4-E2-05-semble-process-attribution.md) |
| [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) | 기본 typo·NL·semantic 잔여의 정책 RCA | P2 / `PROOF_THEN_CONDITIONAL_CODE` | [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md), [O4-E1-05](../tickets/O4-E1-05-untouched-holdout.md), [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md) |

- `CONDITIONAL_CODE`는 병목/계약 실패 조건이 실제로 성립한 경우 구현한다. 조건 미성립은 근거가 있는 `NOT_APPLICABLE`로 닫는다.
- `PROOF_FIRST`/`PROOF_THEN_CONDITIONAL_CODE`는 baseline 결과와 독립 expected contract를 먼저 발행한다.
- 선행 결과가 `BLOCKED`/`NOT_RUN`이면 의존 실행은 완료로 표시하지 않는다. 소스 조사·fixture 준비는 계속 가능하다.

## 파일 소유권과 수정 위치

`OWNED`: 이 에픽 담당자가 해당 파일의 변경을 통합한다. `SHARED`: I0가 최종 공유 checkout에 반영하며 이 에픽은 구체적인 변경 proposal와 검증을 제출한다. `READ`: 기존 구현을 소비/검증하며 새 수정의 소유권을 뜻하지 않는다. 정확한 수정 내용·알고리즘·테스트는 각 연결 티켓의 파일 표에 있다.

| 파일 | 현재 진입점 / 확인할 경계 | 소유 모드 | 구체적 작업 |
| --- | --- | --- | --- |
| [benchmarks/retrieval/src/diagnostics.rs](../../../../benchmarks/retrieval/src/diagnostics.rs) | current ingest/query diagnostics | SHARED | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md) |
| [benchmarks/retrieval/src/query_plan.rs](../../../../benchmarks/retrieval/src/query_plan.rs) | Rust planner/effective request | SHARED | [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [crates/quanta-index-contract/src/ipc/ingest_observation.rs](../../../../crates/quanta-index-contract/src/ipc/ingest_observation.rs) | ingest stage observation | READ | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md) |
| [crates/quanta-index-ipc/src/server.rs](../../../../crates/quanta-index-ipc/src/server.rs) | RequestEventScope / DispatchContextV1.record_event_v1 | SHARED | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md) |
| [crates/quanta-index-ipc/src/server/tests.rs](../../../../crates/quanta-index-ipc/src/server/tests.rs) | request event / ingress/deadline controls | SHARED | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md) |
| [crates/quanta-index-lexical/src/adapter_ingest.rs](../../../../crates/quanta-index-lexical/src/adapter_ingest.rs) | publish preparation/build observations | OWNED | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md) |
| [crates/quanta-index-lexical/src/adapter_open.rs](../../../../crates/quanta-index-lexical/src/adapter_open.rs) | cold-open phase<br>verified cold-open authority | OWNED | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md), [O4-E4-04](../tickets/O4-E4-04-source-token-authority.md) |
| [crates/quanta-index-lexical/src/file_authority.rs](../../../../crates/quanta-index-lexical/src/file_authority.rs) | plan_ops / apply_plan / from_verified_files<br>apply_plan<br>from_verified_files / source_posting_memberships | OWNED | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md), [O4-E4-02](../tickets/O4-E4-02-generation-durable-barriers.md), [O4-E4-04](../tickets/O4-E4-04-source-token-authority.md) |
| [crates/quanta-index-lexical/src/index_store.rs](../../../../crates/quanta-index-lexical/src/index_store.rs) | write_atomic_durable / write_atomic_durable_at | OWNED | [O4-E4-02](../tickets/O4-E4-02-generation-durable-barriers.md) |
| [crates/quanta-index-lexical/src/sealed_generation/coverage_pages.rs](../../../../crates/quanta-index-lexical/src/sealed_generation/coverage_pages.rs) | write_coverage_pages | OWNED | [O4-E4-02](../tickets/O4-E4-02-generation-durable-barriers.md) |
| [crates/quanta-index-lexical/src/searcher/code_search.rs](../../../../crates/quanta-index-lexical/src/searcher/code_search.rs) | typo_text_is_ascii / typo_witness / scan_typo_token_spans<br>typo shortlist/witness/distance cache path<br>literal-first / explicit OSA1 execution mode | OWNED | [O4-E4-03](../tickets/O4-E4-03-ascii-scanner-decision.md), [O4-E4-04](../tickets/O4-E4-04-source-token-authority.md), [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [crates/quanta-index-lexical/src/searcher/code_search/ranking.rs](../../../../crates/quanta-index-lexical/src/searcher/code_search/ranking.rs) | distance/declaration/occurrence ordering<br>source-attested declaration ranking | OWNED | [O4-E4-04](../tickets/O4-E4-04-source-token-authority.md), [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [crates/quanta-index-lexical/tests/l2_file_mutation.rs](../../../../crates/quanta-index-lexical/tests/l2_file_mutation.rs) | full/delta/delete/no-op fixture<br>lifecycle parity<br>token authority delta/delete | OWNED | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md), [O4-E4-02](../tickets/O4-E4-02-generation-durable-barriers.md), [O4-E4-04](../tickets/O4-E4-04-source-token-authority.md) |
| [crates/quanta-index-lexical/tests/l3_exact_source.rs](../../../../crates/quanta-index-lexical/tests/l3_exact_source.rs) | actual file/case/typo ranking<br>typo exhaustive fixtures | OWNED | [O4-E4-03](../tickets/O4-E4-03-ascii-scanner-decision.md), [O4-E4-04](../tickets/O4-E4-04-source-token-authority.md) |
| [crates/quanta-index-lexical/tests/sealed_manifest.rs](../../../../crates/quanta-index-lexical/tests/sealed_manifest.rs) | sealed artifact integrity | OWNED | [O4-E4-02](../tickets/O4-E4-02-generation-durable-barriers.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/ranking.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/ranking.rs) | query ranking kernel | SHARED | [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs) | candidate execution/contribution merge | SHARED | [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/semantic.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/semantic.rs) | semantic candidate route | SHARED | [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/tests/hybrid.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/tests/hybrid.rs) | hybrid lane contribution fixtures | SHARED | [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/tests/semantic.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/tests/semantic.rs) | semantic route independent fixtures | SHARED | [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [crates/quanta-index-search-plane/src/query_embedder.rs](../../../../crates/quanta-index-search-plane/src/query_embedder.rs) | provider/model identity / query embedding | READ | [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [crates/quanta-index-searchd-harness/src/bin/open_loop_matrix.rs](../../../../crates/quanta-index-searchd-harness/src/bin/open_loop_matrix.rs) | current CLI/result gates | OWNED | [O4-E4-05](../tickets/O4-E4-05-release-scale-load.md) |
| [crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs](../../../../crates/quanta-index-searchd-harness/src/bin/scale_matrix.rs) | current CLI/output | OWNED | [O4-E4-05](../tickets/O4-E4-05-release-scale-load.md) |
| [crates/quanta-index-searchd-harness/src/open_loop.rs](../../../../crates/quanta-index-searchd-harness/src/open_loop.rs) | schedule / measure_point / run / artifact | OWNED | [O4-E4-05](../tickets/O4-E4-05-release-scale-load.md) |
| [crates/quanta-index-searchd-harness/src/scale.rs](../../../../crates/quanta-index-searchd-harness/src/scale.rs) | ScaleRuntimeConfig / generate_scoped_corpus / ScopedOracle / lifecycle 실행 | OWNED | [O4-E4-05](../tickets/O4-E4-05-release-scale-load.md) |
| [crates/quanta-index-searchd-runtime/tests/e2e_crash_matrix.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_crash_matrix.rs) | runtime_extended_suite crash cuts | SHARED | [O4-E4-02](../tickets/O4-E4-02-generation-durable-barriers.md) |
| [crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_restart_replay_determinism.rs) | runtime_risk_suite restart | SHARED | [O4-E4-05](../tickets/O4-E4-05-release-scale-load.md) |
| [docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md](../../../../docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md) | scale verdict | OWNED | [O4-E4-05](../tickets/O4-E4-05-release-scale-load.md) |
| [docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md](../../../../docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md) | load verdict | OWNED | [O4-E4-05](../tickets/O4-E4-05-release-scale-load.md) |
| [docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md) | performance decision | OWNED | [O4-E4-06](../tickets/O4-E4-06-qualified-performance.md) |
| [tools/benchmark/retrieval/live_lexical_external.py](../../../../tools/benchmark/retrieval/live_lexical_external.py) | completed response clocks | READ | [O4-E4-06](../tickets/O4-E4-06-qualified-performance.md) |
| [tools/benchmark/retrieval/pair-spec.schema.json](../../../../tools/benchmark/retrieval/pair-spec.schema.json) | speed protocol | READ | [O4-E4-06](../tickets/O4-E4-06-qualified-performance.md) |
| [tools/benchmark/retrieval/query_plan.py](../../../../tools/benchmark/retrieval/query_plan.py) | canonical lexical NL planner | OWNED | [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |
| [tools/benchmark/retrieval/query_timing_overhead.py](../../../../tools/benchmark/retrieval/query_timing_overhead.py) | observation profile<br>existing A/B observations<br>existing measurement controls | OWNED | [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md), [O4-E4-03](../tickets/O4-E4-03-ascii-scanner-decision.md), [O4-E4-06](../tickets/O4-E4-06-qualified-performance.md) |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | host_probe / validate_host_timeline / validate_qualified_speed_spec / cmd_pair / cmd_verdict | READ | [O4-E4-06](../tickets/O4-E4-06-qualified-performance.md) |
| [tools/benchmark/retrieval/semble.py](../../../../tools/benchmark/retrieval/semble.py) | completed parent clock/index phases | READ | [O4-E4-06](../tickets/O4-E4-06-qualified-performance.md) |
| [tools/ci/tests/test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) | scanner phase/read replay<br>planner/policy oracle | SHARED | [O4-E4-03](../tickets/O4-E4-03-ascii-scanner-decision.md), [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) |

## 병렬 착수와 의존 경계

E4-01/03의 attribution·fixture·독립 oracle 준비는 시작 가능하다. lexical storage/searcher 변경은 E4 한 담당자가 순서대로 통합한다. Rust heavy builds·ingest/capture/scale/performance는 I0 resource admission을 공유하고 직렬 실행한다.

E4-02/04는 measured bottleneck 조건부다. E4-05/06은 I0 선택 epoch 이후, E4-06은 E1 admission·E2 scope/timers/Semble attribution을 요구한다. baseline speed는 single-RPC와 XL scale 전체 완료를 기다리지 않는다. 개선/대규모 효과 claim에는 해당 E3/E4 proof를 추가 요구한다. E4-07은 E1 name/holdout/final scoring을 소비해 정책 판정하며 수정 시 affected I0/capture gates를 다시 연다.

## 에픽 완료 조건

- 각 채택한 변경의 independent output/lifecycle/crash equivalence와 bounded resource 비용이 증명된다. syscall fault·process kill/reopen과 실제 저장장치 power-loss/flush qualification을 구별한다.
- whole-call effects·uncertainty·refusal·host limits가 정식 release 결과에서 설명된다.
- 기본 정책/NL/semantic을 바꾸면 untouched holdout과 critical strata의 acceptance가 충족된다.

## 원본과 계약 근거

- [docs/handoff/oct-4/agent-4.md](../../../../docs/handoff/oct-4/agent-4.md)
- [docs/handoff/oct-4/agent-5.md](../../../../docs/handoff/oct-4/agent-5.md)
- [docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md)
- [docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md](../../../../docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-03-large-corpus-scale-tiers.md)
- [docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md](../../../../docs/plans/jun-7-search-product-quality/tickets-wave2/J7Q-04-latency-tail-hardening.md)
