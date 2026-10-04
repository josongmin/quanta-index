# E3 — Active 선택·read-view lifetime·운영 계약

- 상태: `PLANNED`. 구현·모델 실행·제품 캡처·verification은 이 계획 작성에서 `NOT_RUN`.
- 담당: E3 담당 1명.
- 6개 티켓. [전체 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md).

## 목적

선택→view acquisition→응답의 generation/token custody와 admitted publish·readiness·operator truth를 실제 counterexample에서 검증하고 확인된 결함을 소유 권위에서 수정한다.

## 배경과 현재 구현

- un-tokened Active 선택과 acquire_read_view 사이 retirement 가능성은 정적 조사에 있다. 이미 획득한 view의 lifetime tests는 선택 전 구간의 결정적 G1→G2→G3 재현을 대체하지 않는다.
- SDK는 Active resolve 이후 query를 보낸다. Text/Symbol/History/RuntimeMetadata는 보통2RPC, Semantic+lexical scope·Hybrid/HybridSeed는 track별 resolve로3RPC까지 가능하고 rev:at.time에는 별도 ancestor resolve가 있다. pre-resolve 삭제는 variant별 selected token·ABA·joint snapshot·response DTO/binding 계약과 함께 설계한다.
- maintenance tick은 동기 full-tree disk gauge 갱신 뒤 heartbeat를 완료한다. readiness는 freshness를 사용한다. 느린 disk walk가 readiness를 잘못 낮추는 실제 실험은 미실행이다.
- SDK 기본30초, dispatch120초, peer hangup budget cancel과 admitted publish settlement가 공존한다. client timeout은 operation rollback 증거가 아니다.
- operator ring·SDK/searchctl와 실제 owner-binary의 payload 없는 request correlation/lost root/read-failure cases는 이미 있다. process_readiness_owner_v1은 e2e_process_readiness.rs 등을 포함하는 wrapper다. 기존 cases를 소비하고 누락된 seam·authorization·부정 경로만 보완하며 Linux fresh release는 별도 범위다.
- OCT-04-001은 Proposed다. Accepted SEP-21-002/003·SEP-27-005와 현재 source를 먼저 대조한다.

## 목표 계약과 변경 원칙

- 선택 linearization과 bounded admission claim을 actual catalog/retention authority에서 정의한다. claim은 실제 view handle에 이전되고 실패/panic/cancel에서 해제된다.
- atomic Active query는 단일 선택 snapshot을 search와 response에 결속한다. ambient latest 재조회·무제한 active handle pin·retry masking을 추가하지 않는다.
- health/freshness와 disk metering의 pacing은 measured slow-walk 결과에 따라 결정한다. gauge는 logical/allocated/physical-write/transient high-water 의미를 분리한다.
- publish는 durable operation identity로 inspect/replay하고 duplicate ACK/state convergence를 검증한다. 진단 DTO와 operator facade는 기존 구현을 소비한다.

## 해야 할 일

1. G1선택→G2활성화→G3retention→G1획득을 barrier fixture와 실제 daemon seam에서 재현한다.
2. 현 계약 또는 채택한 강화 계약의 결함이 확인된 경우만 short admission claim을 view로 이전하는 최소 수정을 한다.
3. selected snapshot/token을 응답에서 검증하도록 SDK·binding·server producer를 함께 갱신하고 단일RPC row parity·ABA/cursor/refusal을 입증한다.
4. disk walker를3cadence 이상 늦춰 readiness 영향을 확인하고 필요할 때 metering과 health 갱신을 분리한다.
5. admitted slow publish에서 client timeout/disconnect 이후 operation inspect·exact replay·state/ACK 결과를 검증한다.
6. bounded ring overflow/drop·process identity·authorization denial·wrong scope·reconnect·slow/closed peer를 실제 process owner에서 검증한다.

## 티켓 실행 순서

| 티켓 | 작업 | 우선순위 / 종류 | 선행 결과 |
| --- | --- | --- | --- |
| [O4-E3-01](../tickets/O4-E3-01-active-selection-race.md) | Active 선택·retention·view 획득 경합 재현 | P0 / `PROOF_FIRST` | 즉시 조사·fixture 준비 가능 |
| [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md) | 선택 admission pin의 read-view 이전 | P0 / `CONDITIONAL_CODE` | [O4-E3-01](../tickets/O4-E3-01-active-selection-race.md) |
| [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) | 단일 RPC의 Active 선택·검색·응답 결속 | P1 / `CODE_AND_PROOF` | [O4-E3-01](../tickets/O4-E3-01-active-selection-race.md), [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md) |
| [O4-E3-04](../tickets/O4-E3-04-maintenance-health-metering.md) | 느린 디스크 metering과 readiness 분리 판정 | P1 / `PROOF_THEN_CONDITIONAL_CODE` | 즉시 조사·fixture 준비 가능 |
| [O4-E3-05](../tickets/O4-E3-05-publish-timeout-replay.md) | admitted publish timeout과 operation replay | P1 / `PROOF_THEN_CONDITIONAL_CODE` | 즉시 조사·fixture 준비 가능 |
| [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) | 기존 operator diagnostics·process truth 검증 | P1 / `PROOF_ONLY` | 즉시 조사·fixture 준비 가능 |

- `CONDITIONAL_CODE`는 병목/계약 실패 조건이 실제로 성립한 경우 구현한다. 조건 미성립은 근거가 있는 `NOT_APPLICABLE`로 닫는다.
- `PROOF_FIRST`/`PROOF_THEN_CONDITIONAL_CODE`는 baseline 결과와 독립 expected contract를 먼저 발행한다.
- 선행 결과가 `BLOCKED`/`NOT_RUN`이면 의존 실행은 완료로 표시하지 않는다. 소스 조사·fixture 준비는 계속 가능하다.

## 파일 소유권과 수정 위치

`OWNED`: 이 에픽 담당자가 해당 파일의 변경을 통합한다. `SHARED`: I0가 최종 공유 checkout에 반영하며 이 에픽은 구체적인 변경 proposal와 검증을 제출한다. `READ`: 기존 구현을 소비/검증하며 새 수정의 소유권을 뜻하지 않는다. 정확한 수정 내용·알고리즘·테스트는 각 연결 티켓의 파일 표에 있다.

| 파일 | 현재 진입점 / 확인할 경계 | 소유 모드 | 구체적 작업 |
| --- | --- | --- | --- |
| [benchmarks/retrieval/tests/sdk_roundtrip.rs](../../../../benchmarks/retrieval/tests/sdk_roundtrip.rs) | real-daemon query observation | SHARED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-contract/src/ipc/control.rs](../../../../crates/quanta-index-contract/src/ipc/control.rs) | ProcessRequestEvents DTO | READ | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-contract/src/ipc/split.rs](../../../../crates/quanta-index-contract/src/ipc/split.rs) | SearchPlaneQueryIpcRequest/Response | SHARED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-contract/src/results/query_responses.rs](../../../../crates/quanta-index-contract/src/results/query_responses.rs) | TextQueryResponse / SymbolQueryResponse / SemanticQueryResponse / HybridQueryResponse / HybridSeedQueryResponse / SearchPlaneHistoryQueryResponse / SearchPlaneRuntimeMetadataQueryResponse | SHARED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-ipc/src/admission.rs](../../../../crates/quanta-index-ipc/src/admission.rs) | serial ingest vs query slot | READ | [O4-E3-05](../tickets/O4-E3-05-publish-timeout-replay.md) |
| [crates/quanta-index-ipc/src/counters.rs](../../../../crates/quanta-index-ipc/src/counters.rs) | RequestEventWindowV1 / existing event ring | READ | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-ipc/src/server/peer_watch.rs](../../../../crates/quanta-index-ipc/src/server/peer_watch.rs) | hangup budget cancellation | OWNED | [O4-E3-05](../tickets/O4-E3-05-publish-timeout-replay.md) |
| [crates/quanta-index-sdk/src/binding.rs](../../../../crates/quanta-index-sdk/src/binding.rs) | query generation/domain/response validation | OWNED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-sdk/src/client.rs](../../../../crates/quanta-index-sdk/src/client.rs) | pin_active_selector / pin_active_query / resolve_lexical_query_generation / dispatch_query_inner<br>default I/O/dispatch timeout | OWNED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md), [O4-E3-05](../tickets/O4-E3-05-publish-timeout-replay.md) |
| [crates/quanta-index-sdk/src/observability.rs](../../../../crates/quanta-index-sdk/src/observability.rs) | request_events | OWNED | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-sdk/src/tests/control_tests.rs](../../../../crates/quanta-index-sdk/src/tests/control_tests.rs) | ProcessRequestEventsV1 controls | OWNED | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-sdk/src/tests/query_tests.rs](../../../../crates/quanta-index-sdk/src/tests/query_tests.rs) | observed active query RPCs | OWNED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/control_dispatcher.rs](../../../../crates/quanta-index-search-plane/src/control_dispatcher.rs) | ProcessRequestEventsPort / authorization route | OWNED | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs](../../../../crates/quanta-index-search-plane/src/ingest_dispatcher/dispatcher.rs) | dispatch / operation journal terminal | OWNED | [O4-E3-05](../tickets/O4-E3-05-publish-timeout-replay.md) |
| [crates/quanta-index-search-plane/src/ingest_dispatcher/tests/idempotency.rs](../../../../crates/quanta-index-search-plane/src/ingest_dispatcher/tests/idempotency.rs) | operation replay tests | OWNED | [O4-E3-05](../tickets/O4-E3-05-publish-timeout-replay.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/dispatcher.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/dispatcher.rs) | ResolveActiveGeneration / dispatch | OWNED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/planning.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/planning.rs) | selection to read-view assembly<br>planned lexical/semantic query | SHARED | [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md), [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/read_view/view.rs) | acquire_read_view<br>ReadViewRequestV1 / acquire_read_view / QueryReadViewV2 | OWNED | [O4-E3-01](../tickets/O4-E3-01-active-selection-race.md), [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/history.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/history.rs) | SearchPlaneHistoryQueryResponse materialization | SHARED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs) | hybrid response materialization | SHARED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid_seed.rs) | HybridSeedQueryResponse materialization | SHARED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/lexical.rs) | lexical response materialization | SHARED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/runtime_metadata.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/runtime_metadata.rs) | SearchPlaneRuntimeMetadataQueryResponse materialization | SHARED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/routes/semantic.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/semantic.rs) | semantic response materialization | SHARED | [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/selection.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/selection.rs) | resolve_optional_selection / resolve_joint_active_selection<br>active and joint selection | OWNED | [O4-E3-01](../tickets/O4-E3-01-active-selection-race.md), [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md), [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/tests/read_view.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/tests/read_view.rs) | Active/pinned refusal cases | OWNED | [O4-E3-01](../tickets/O4-E3-01-active-selection-race.md) |
| [crates/quanta-index-search-plane/src/query_dispatcher/tests/read_view_lifetime.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/tests/read_view_lifetime.rs) | view lifetime cases | OWNED | [O4-E3-01](../tickets/O4-E3-01-active-selection-race.md) |
| [crates/quanta-index-search-plane/src/readiness/activation_catalog.rs](../../../../crates/quanta-index-search-plane/src/readiness/activation_catalog.rs) | active selection API | OWNED | [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md) |
| [crates/quanta-index-search-plane/src/search_corpus_retention.rs](../../../../crates/quanta-index-search-plane/src/search_corpus_retention.rs) | retention plan | OWNED | [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md) |
| [crates/quanta-index-search-plane/src/snapshot_registry.rs](../../../../crates/quanta-index-search-plane/src/snapshot_registry.rs) | snapshot admission/acquire/reconcile | OWNED | [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md) |
| [crates/quanta-index-searchctl/src/lib.rs](../../../../crates/quanta-index-searchctl/src/lib.rs) | RequestEvents dispatch/rendering | OWNED | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-searchctl/src/render.rs](../../../../crates/quanta-index-searchctl/src/render.rs) | render_request_events | OWNED | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-searchctl/src/tests/control.rs](../../../../crates/quanta-index-searchctl/src/tests/control.rs) | request-events parse / render tests | OWNED | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-searchctl/tests/cli_smoke.rs](../../../../crates/quanta-index-searchctl/tests/cli_smoke.rs) | control CLI process smoke | OWNED | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs](../../../../crates/quanta-index-searchd-runtime/tests/common/searchd_binary_process.rs) | SearchdBinaryProcess / searchd_binary_path | READ | [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-searchd-runtime/tests/e2e_ingest_idempotency.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_ingest_idempotency.rs) | runtime_fast_suite ingress seam | OWNED | [O4-E3-05](../tickets/O4-E3-05-publish-timeout-replay.md) |
| [crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_process_readiness.rs) | runtime_extended_suite process cases<br>binary_daemon_exposes_one_correlated_query_without_payload / binary_daemon_detects_lost_active_backend_root | OWNED | [O4-E3-04](../tickets/O4-E3-04-maintenance-health-metering.md), [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-searchd-runtime/tests/e2e_read_view.rs](../../../../crates/quanta-index-searchd-runtime/tests/e2e_read_view.rs) | runtime_fast_suite의 read_view module | OWNED | [O4-E3-01](../tickets/O4-E3-01-active-selection-race.md) |
| [crates/quanta-index-searchd-runtime/tests/process_readiness_owner_v1.rs](../../../../crates/quanta-index-searchd-runtime/tests/process_readiness_owner_v1.rs) | owner readiness rail<br>real operator process proofs | OWNED | [O4-E3-04](../tickets/O4-E3-04-maintenance-health-metering.md), [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) |
| [crates/quanta-index-searchd/src/app/maintenance.rs](../../../../crates/quanta-index-searchd/src/app/maintenance.rs) | tick / observe_backend / refresh_disk_usage / MaintenanceTallies | OWNED | [O4-E3-04](../tickets/O4-E3-04-maintenance-health-metering.md) |
| [crates/quanta-index-searchd/src/app/readiness.rs](../../../../crates/quanta-index-searchd/src/app/readiness.rs) | backend and heartbeat readiness | OWNED | [O4-E3-04](../tickets/O4-E3-04-maintenance-health-metering.md) |
| [crates/quanta-index-searchd/src/app/runtime.rs](../../../../crates/quanta-index-searchd/src/app/runtime.rs) | maintenance composition/lifecycle | OWNED | [O4-E3-04](../tickets/O4-E3-04-maintenance-health-metering.md) |

## 병렬 착수와 의존 경계

E3-01/04/05/06 proof 준비를 시작할 수 있다. E3-02는 E3-01의 counterexample/disposition 이후, E3-03은 그 선택 계약 이후 진행한다. SDK client/binding·runtime maintenance/readiness source는 E3 담당자가 통합한다.

public SDK/contract·wire·generation/state 변경의 mandatory gates를 I0에서 실행한다. E3 single-RPC/selection 변경 효과는 그 변경을 포함한 matching epoch에서 측정한다. 현 supported 계약의 baseline 성능은 E3 전체 최적화 완료를 기다리지 않는다. routes/hybrid·semantic와 query DTO는 E4 정책 proposal와 I0가 함께 통합한다.

## 에픽 완료 조건

- Active/explicit/token/ABA/cursor/GC/cancel의 deterministic result와 physical lifetime이 독립 oracle와 일치한다.
- timeout/replay·slow-walk/readiness·operator authorization/events의 실제 daemon 관측이 있다.
- 재현되지 않은 위험·현 계약상 허용 결과·실제 결함·제안 변경을 구별한다.

## 원본과 계약 근거

- [docs/handoff/oct-4/agent-4.md](../../../../docs/handoff/oct-4/agent-4.md)
- [docs/handoff/oct-4/agent-5.md](../../../../docs/handoff/oct-4/agent-5.md)
- [docs/adr/OCT-04-001-search-corpus-selection-and-ingest-pressure.md](../../../../docs/adr/OCT-04-001-search-corpus-selection-and-ingest-pressure.md)
- [docs/adr/SEP-21-002-durable-authority-and-operation-lifecycle.md](../../../../docs/adr/SEP-21-002-durable-authority-and-operation-lifecycle.md)
- [docs/adr/SEP-21-003-read-view-continuation-and-provider-policy.md](../../../../docs/adr/SEP-21-003-read-view-continuation-and-provider-policy.md)
- [docs/adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md](../../../../docs/adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)
- [docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md](../../../../docs/plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md)
