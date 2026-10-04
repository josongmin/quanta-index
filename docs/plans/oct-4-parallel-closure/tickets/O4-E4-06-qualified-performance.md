# O4-E4-06 — 동일 응답 경계의 정식 반복 성능

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E4 — 인덱싱·typo 실행 비용·release 성능·scale](../epics/E4-storage-query-and-scale.md) / E4 담당 |
| 우선순위 / 종류 | P1 / `EXECUTION` |
| 기준 웨이브 | [W4 — 실제 캡처·성능·scale](../waves/W4-native-capture-performance-and-scale.md) |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | [O4-I0-02](O4-I0-02-matching-source-proof.md), [O4-E1-03](O4-E1-03-admission-and-split.md), [O4-E2-01](O4-E2-01-native-completed-timer.md), [O4-E2-02](O4-E2-02-external-index-universe.md), [O4-E2-05](O4-E2-05-semble-process-attribution.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

final source와 허용된 host에서 query/index 성능 효과를 사전 기준·출력 동등성·불확실성으로 판정한다.

## 배경과 현재 상태

B07에는 complete output/phase clocks/host timeline/required observations 검증이 있다. 과거 Mac은 concurrent load와 frequency unavailable로 admission을 충족하지 못했다. Quanta SDK·Semble workerBM25·외부 transport clocks 또는 서로 다른 chunk workloads를 동일 speed rank로 합칠 수 없다.

## 착수 입력

- final source/binary/input/config tuple, E2 canonical timer와 topology/request policy
- B07 최소5 fresh roots/route당1000 warm observations 규약, randomized paired schedule, predeclared effect/uncertainty decision
- admitted quiet host의 continuous load/frequency/thermal/power/disk probes

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | host_probe / validate_host_timeline / validate_qualified_speed_spec / cmd_pair / cmd_verdict | 현 qualification gates·continuous timeline·phase digest validator를 I0 소유 경로로 소비한다. admission bypass 금지. | READ |
| [tools/benchmark/retrieval/pair-spec.schema.json](../../../../tools/benchmark/retrieval/pair-spec.schema.json) | speed protocol | 기존 warmup/repetition/host/unit 요구를 소비하며 실제 contract evolution만 I0가 반영한다. | READ |
| [tools/benchmark/retrieval/semble.py](../../../../tools/benchmark/retrieval/semble.py) | completed parent clock/index phases | E2 source와 동일 package/model/revision/profile로 실행한다. | READ |
| [tools/benchmark/retrieval/live_lexical_external.py](../../../../tools/benchmark/retrieval/live_lexical_external.py) | completed response clocks | E2 product topology/native scope를 동일 request/output 경계에서 사용한다. | READ |
| [tools/benchmark/retrieval/query_timing_overhead.py](../../../../tools/benchmark/retrieval/query_timing_overhead.py) | existing measurement controls | 관측 overhead와 work/output identity를 A/B에서 확인한다. | OWNED |
| [docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md](../../../../docs/plans/sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md) | performance decision | 같은 boundary/workload/resource의 current observed decision과 unrun seams를 기록한다. | OWNED |

## 실행 단계

1. output rows/order/status/span/identity·work counters의 parity를 먼저 닫고 final host/unit protocol을 freeze한다.
2. build/model/corpus prep/index/publish/seal/activate·query timers 포함 관계와 topology를 사전 선언한다.
3. quiet host continuous admission을 통과시키고 heavy builds/indexers/model reviews와 perf captures의 실행권을 분리한다.
4. 기존 randomized paired blocks/fresh roots/warm observations로 exact 및 영향 typo/NL lanes를 실행한다.
5. query sum/p50/p95, coverage, full/delta/delete/no-op/reopen, CPU/RSS/disk high water와 uncertainty를 보고한다.
6. phase digest와 independent raw replay를 확인해 regression/acceptance/미확정을 source별로 판정한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_completed_response_timing.py tools/ci/tests/test_retrieval_benchmark.py -q -k 'completed or qualified_speed or host_timeline or phase_digest'`
- 실제 host-probe와 continuous timeline refusal controls: frequency unavailable/load/thermal/power drift/capture overlap/wrong phase bytes.
- full expected schedule와 source closure를 independent verdict replay로 검증한다.

## 성능 claim별 추가 조건

- E3-03 single-RPC 또는 E4 storage/scanner/token 변경의 효과를 주장할 때는 그 선택된 변경·독립 결과 oracle를 I0 epoch에서 먼저 통합한다. baseline 두 RPC를 측정하는 데 single-RPC 구현 완료를 요구하지 않는다.
- E4-05 전체 scale 성공은 작은 corpus의 query-speed 비교 선행 조건이 아니다. large-corpus capacity/tail/restart 성능 claim에는 해당 tier/profile의 E4-05 proof가 필요하다.
- 외부 제품 speed claim은 해당 E2-02 native index scope와 서비스 topology/remote resource identity를 요구한다. Quanta-only이면 외부 scope는 NOT_APPLICABLE이고, Semble이 없으면 E2-05 범위도 비적용이다.
- 현재 driver의 바닥값은 **20 frozen tasks, 5 fresh roots, warmup≥1, route당 합계1,000 measured warm observations, 정확히 한 Quanta route**다. E1의 lane별 unique-query≥1,000 요구와 observations를 혼동하지 않는다. 추가 표본/효과/CI 기준은 selected B07 contract에 맞춰 사전 확정한다.

## 완료 조건

- 허용된 host·동일 output/request 경계·complete schedule·source/raw binding에서 observed effect와 CI가 acceptance 기준으로 판정된다.
- host가 미충족이면 timing 실행은 BLOCKED/NOT_RUN이며 diagnostic samples만 보고한다.

## 중단·거절·재개 조건

- B07의 local 최소표본 규약을 보편적 통계 법칙으로 설명하지 않는다. overlapped parent/child wall을 합산하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
