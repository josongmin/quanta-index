# E2 — 외부 제품 native 범위·응답 경계·실제 캡처

- 상태: `PLANNED`. 구현·모델 실행·제품 캡처·verification은 이 계획 작성에서 `NOT_RUN`.
- 담당: E2 담당 1명.
- 6개 티켓. [전체 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md).

## 목적

5제품의 실제 native source 범위와 completed-response 시간을 같은 계약에서 수집하고, 모든 필수 셀의 결과와 blind candidate union을 E1에 전달한다.

## 배경과 현재 구현

- 현재 외부 HTTP/process clock은 request construction/response normalization 일부를 제외한다. Quanta SDK·Semble completed boundary와 직접 비교할 수 있는 completed clock이 필요하다.
- Sourcegraph index_scope validator와 OpenGrok view 함수는 있다. 전체 native indexed-file universe를 실제 backend에서 attest하는 producer와 before/after 범위 확인은 별도 작업이다.
- execution_batch의 ready/failed drain·compatibility union membership projection은 이미 있다. repaired external controller가 이를 실제 소비한 실행 증거는 없다.
- Semble parent process clock은 있으나 과거 qg15 process7.511초 대 worker1.447초의 차이는 아직 귀속되지 않았다. 차이를 전부 IPC/import로 단정할 근거는 없다.

## 목표 계약과 변경 원칙

- request construction 시작 → native HTTP/process → parse/normalization 완료까지 completed clock을 잡고 raw persistence/hash/report는 그 밖에 둔다. 기존 transport elapsed_ms는 별도 의미로 보존한다.
- backend indexed universe·source mapping·query 표현·runtime/config identity가 capture와 함께 검증돼야 한다. capture에 나온 hit 목록을 전체 corpus로 취급하지 않는다.
- required cell은 cohort×repository×product×profile×unit에서 사전 생성한다. success/empty/unsupported/cap/partial/error/missing을 구별하고 operational denominator를 보존한다.
- Quanta·Semble·Sourcegraph·OpenGrok·codesearch native raw를 제품별 보존하고, derived scoring view는 native raw authority를 참조한다. 최종 qrels에 따른 replay와 새 호출 필요성을 구별한다.

## 해야 할 일

1. native completed timer와 sleep/fake-clock 기반 request/normalization/persistence 경계 controls를 기존 collector에 구현한다.
2. Sourcegraph/OpenGrok actual indexed-file inventory를 현재 corpus commit/source mapping과 대조한다.
3. required-cell inventory와 canonical readiness helper의 실제 controller 연결을 완료하고 ready sibling을 실패와 분리해 실행한다.
4. Semble import/env/model asset/corpus copy/worker/validation/assembly 단계를 실제 child clocks로 귀속하고 disjoint totals와 parent unattributed residual을 출력한다.
5. warmup0를 선택할 때만 실제 Quanta/Semble task별 ranked rows·status·f64 bits parity와 각자 protocol의 phase ledger·speed-mode refusal을 입증한다. 동일 RNG는 warmup 유무에 따라 measured 순서가 달라지며 검증 전에는 기존 warmup1을 유지한다.
6. E1의 admitted input와 I0 matching binaries에서 5제품을 실행하고 source/request/response-bound union을 E1에 전달한다. Gin20·ARB original/adapted·외부 cohort는 별도 셀과 lane으로 유지한다.

## 티켓 실행 순서

| 티켓 | 작업 | 우선순위 / 종류 | 선행 결과 |
| --- | --- | --- | --- |
| [O4-E2-01](../tickets/O4-E2-01-native-completed-timer.md) | 외부 제품의 completed-response 시간 경계 | P1 / `CODE_AND_PROOF` | 즉시 조사·fixture 준비 가능 |
| [O4-E2-02](../tickets/O4-E2-02-external-index-universe.md) | Sourcegraph·OpenGrok 전체 native 색인 범위 | P1 / `DATA_AND_PROOF` | 즉시 조사·fixture 준비 가능 |
| [O4-E2-03](../tickets/O4-E2-03-required-cells-and-scheduling.md) | 필수 셀 inventory와 실패 분리 스케줄 | P0 / `INTEGRATION` | 즉시 조사·fixture 준비 가능 |
| [O4-E2-04](../tickets/O4-E2-04-fresh-five-product-captures.md) | 5제품 실제 캡처와 blind union 반환 | P1 / `EXECUTION` | [O4-E1-03](../tickets/O4-E1-03-admission-and-split.md), [O4-E2-02](../tickets/O4-E2-02-external-index-universe.md), [O4-E2-03](../tickets/O4-E2-03-required-cells-and-scheduling.md), [O4-I0-02](../tickets/O4-I0-02-matching-source-proof.md) |
| [O4-E2-05](../tickets/O4-E2-05-semble-process-attribution.md) | Semble process 비용의 phase 귀속 | P2 / `CODE_AND_PROOF` | 즉시 조사·fixture 준비 가능 |
| [O4-E2-06](../tickets/O4-E2-06-quality-only-warmup.md) | 기존 quality-only warmup=0 정책의 실제 parity | P1 / `PROOF_AND_CONFIG` | 즉시 조사·fixture 준비 가능 |

- `CONDITIONAL_CODE`는 병목/계약 실패 조건이 실제로 성립한 경우 구현한다. 조건 미성립은 근거가 있는 `NOT_APPLICABLE`로 닫는다.
- `PROOF_FIRST`/`PROOF_THEN_CONDITIONAL_CODE`는 baseline 결과와 독립 expected contract를 먼저 발행한다.
- 선행 결과가 `BLOCKED`/`NOT_RUN`이면 의존 실행은 완료로 표시하지 않는다. 소스 조사·fixture 준비는 계속 가능하다.

## 파일 소유권과 수정 위치

`OWNED`: 이 에픽 담당자가 해당 파일의 변경을 통합한다. `SHARED`: I0가 최종 공유 checkout에 반영하며 이 에픽은 구체적인 변경 proposal와 검증을 제출한다. `READ`: 기존 구현을 소비/검증하며 새 수정의 소유권을 뜻하지 않는다. 정확한 수정 내용·알고리즘·테스트는 각 연결 티켓의 파일 표에 있다.

| 파일 | 현재 진입점 / 확인할 경계 | 소유 모드 | 구체적 작업 |
| --- | --- | --- | --- |
| [benchmarks/retrieval/src/main.rs](../../../../benchmarks/retrieval/src/main.rs) | query_protocol / warmup_schedules / emitted warmup_passes | SHARED | [O4-E2-06](../tickets/O4-E2-06-quality-only-warmup.md) |
| [tools/benchmark/retrieval/arb_adapter.py](../../../../tools/benchmark/retrieval/arb_adapter.py) | 현재 input adapter | OWNED | [O4-E2-04](../tickets/O4-E2-04-fresh-five-product-captures.md) |
| [tools/benchmark/retrieval/execution_batch.py](../../../../tools/benchmark/retrieval/execution_batch.py) | iter_repository_admissions / build_execution_pack / verify_execution_membership / project_scoring_view | READ | [O4-E2-03](../tickets/O4-E2-03-required-cells-and-scheduling.md) |
| [tools/benchmark/retrieval/holdout_review.py](../../../../tools/benchmark/retrieval/holdout_review.py) | capture_review_pool | READ | [O4-E2-04](../tickets/O4-E2-04-fresh-five-product-captures.md) |
| [tools/benchmark/retrieval/lexical_file_comparison.py](../../../../tools/benchmark/retrieval/lexical_file_comparison.py) | product_result / external result parsing | SHARED | [O4-E2-01](../tickets/O4-E2-01-native-completed-timer.md) |
| [tools/benchmark/retrieval/live_lexical_external.py](../../../../tools/benchmark/retrieval/live_lexical_external.py) | _http / _process / _sourcegraph / _opengrok / _cs / capture / verify<br>_backend_runtime / _backend_snapshot / _opengrok_indexed_inventory / _opengrok_indexed_view<br>_selected_products / capture / verify<br>capture / verify / BoundRelease | OWNED | [O4-E2-01](../tickets/O4-E2-01-native-completed-timer.md), [O4-E2-02](../tickets/O4-E2-02-external-index-universe.md), [O4-E2-03](../tickets/O4-E2-03-required-cells-and-scheduling.md), [O4-E2-04](../tickets/O4-E2-04-fresh-five-product-captures.md) |
| [tools/benchmark/retrieval/retrieval_contract.py](../../../../tools/benchmark/retrieval/retrieval_contract.py) | shared completed boundary owner | SHARED | [O4-E2-01](../tickets/O4-E2-01-native-completed-timer.md) |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | run_quality_batch / run_quality_matrix / verify_quality_matrix<br>cmd_quanta / cmd_pair / run_quality_matrix / cmd_verdict<br>build_query_protocol / validate_qualified_speed_spec / load_spec / run_quality_batch | SHARED | [O4-E2-03](../tickets/O4-E2-03-required-cells-and-scheduling.md), [O4-E2-04](../tickets/O4-E2-04-fresh-five-product-captures.md), [O4-E2-06](../tickets/O4-E2-06-quality-only-warmup.md) |
| [tools/benchmark/retrieval/semble.py](../../../../tools/benchmark/retrieval/semble.py) | run_adapter / run_completed_worker<br>check_semble_env / build_isolated_corpus / materialize_model_cache / run_completed_worker / run_adapter / assemble_record<br>validate_worker_phase_timings / validate_native_profile_report<br>run_adapter / query protocol validation | OWNED | [O4-E2-04](../tickets/O4-E2-04-fresh-five-product-captures.md), [O4-E2-05](../tickets/O4-E2-05-semble-process-attribution.md), [O4-E2-06](../tickets/O4-E2-06-quality-only-warmup.md) |
| [tools/benchmark/retrieval/sourcegraph.py](../../../../tools/benchmark/retrieval/sourcegraph.py) | validate_capture | OWNED | [O4-E2-02](../tickets/O4-E2-02-external-index-universe.md) |
| [tools/benchmark/retrieval/sourcegraph_index_scope.py](../../../../tools/benchmark/retrieval/sourcegraph_index_scope.py) | verify / _verify | OWNED | [O4-E2-02](../tickets/O4-E2-02-external-index-universe.md) |
| [tools/ci/tests/test_completed_response_timing.py](../../../../tools/ci/tests/test_completed_response_timing.py) | Semble parent-normalized output tests | OWNED | [O4-E2-05](../tickets/O4-E2-05-semble-process-attribution.md) |
| [tools/ci/tests/test_holdout_review.py](../../../../tools/ci/tests/test_holdout_review.py) | admission_queue controls | READ | [O4-E2-03](../tickets/O4-E2-03-required-cells-and-scheduling.md) |
| [tools/ci/tests/test_live_lexical_external.py](../../../../tools/ci/tests/test_live_lexical_external.py) | native clocks/raw replay<br>index inventory mutants<br>cell completion/refusal controls | OWNED | [O4-E2-01](../tickets/O4-E2-01-native-completed-timer.md), [O4-E2-02](../tickets/O4-E2-02-external-index-universe.md), [O4-E2-03](../tickets/O4-E2-03-required-cells-and-scheduling.md) |
| [tools/ci/tests/test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) | Semble adapter/source contract tests<br>test_exploratory_query_protocol_accepts_explicit_zero_warmups / validate_qualified_speed_spec controls | SHARED | [O4-E2-05](../tickets/O4-E2-05-semble-process-attribution.md), [O4-E2-06](../tickets/O4-E2-06-quality-only-warmup.md) |
| [tools/ci/tests/test_sourcegraph_parity_inventory.py](../../../../tools/ci/tests/test_sourcegraph_parity_inventory.py) | Sourcegraph universe parity | OWNED | [O4-E2-02](../tickets/O4-E2-02-external-index-universe.md) |

## 병렬 착수와 의존 경계

E2-01/02/03/05/06의 source 조사·fixture 준비는 시작 가능하다. live_lexical_external.py·semble.py는 E2 담당자 한 명이 통합한다. 실제 외부 service index 교체와 제품 호출은 동일 resource별 직렬 실행한다.

E2-04의 hard prerequisites는 해당 repository의 E1-03·E2-02/03·I0-02다. E2-01은 completed speed claim, E2-06은 warmup0 선택에만 요구한다. warmup1과 의미가 명시된 historical transport quality diagnostic은 가능한 범위에서 진행한다. controller/schema/collector 코드는 PREPARE 때 제출해 I0가 VALIDATE한 뒤 admission을 ISSUE한다. query/source/unit/model/index/profile/clock 변경 시 영향 셀을 새 commitment에서 실행한다.

## 에픽 완료 조건

- 모든 필수 셀이 native input/runtime/index scope/request/raw response에서 설명된다. unsupported/cap/partial/error/missing inventory는 운영 coverage이며 성공 비교/전체 qualification 달성을 뜻하지 않는다.
- completed-response와 transport/worker clocks의 의미가 구분되고 persistence/hash가 completed clock에 섞이지 않는다.
- blind union과 operational outcomes를 E1에 넘기고 final qrels에서 raw replay가 일치한다.

## 원본과 계약 근거

- [docs/handoff/oct-4/agent-1.md](../../../../docs/handoff/oct-4/agent-1.md)
- [docs/handoff/oct-4/agent-3.md](../../../../docs/handoff/oct-4/agent-3.md)
- [docs/handoff/oct-4/agent-4.md](../../../../docs/handoff/oct-4/agent-4.md)
- [docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B09-external-robustness-adoption.md)
