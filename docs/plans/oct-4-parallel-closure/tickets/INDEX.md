# OCT-04 티켓 인덱스

[전체 실행 지도](../README.md) · [웨이브별 실행 계획](../WAVES.md). 4개 에픽 26개 티켓 + I0 3개 = **29개**. 각 티켓에 목적·배경·입력·수정 파일/함수/방법·독립 검증·완료/중단 조건이 있다.

모든 작업은 `PLANNED`, product implementation/verification은 `NOT_RUN`이다. 아래 선행 결과는 결과 ISSUE/실행의 초기 DAG이며 source 조사·독립 fixtures·proposal PREPARE를 막지 않는다. E1-03/E2 controller 등의 코드는 먼저 준비→I0-02 VALIDATE→같은 source의 admission ISSUE 순서다. 조건부 gates와 scope별 추가 prerequisites는 각 티켓 본문을 따른다. BLOCKED/NOT_RUN은 완료가 아니다.

## E1 — 정답·검수·admission과 독립 평가

[에픽 목적·배경·파일 지도](../epics/E1-labels-admission-and-gold.md).

| 티켓 | 작업 | 종류 / 우선순위 | 기준 웨이브 | 착수 상태 | 선행 결과 |
| --- | --- | --- | --- | --- | --- |
| [O4-E1-01](O4-E1-01-original-review-resume.md) | 원본 C3 검수 실패 복구와 실제 판단 발행 | `EXECUTION` / P0 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |
| [O4-E1-02](O4-E1-02-supplemental-labels.md) | 미검수 합집합 검수와 원본 라벨 병합 | `EXECUTION` / P0 | [W1](../waves/W1-evidence-and-producers.md) | PREPARE 가능 · ISSUE 대기 | [O4-E1-01](O4-E1-01-original-review-resume.md) |
| [O4-E1-03](O4-E1-03-admission-and-split.md) | 최종 suite·split·admission 연결 | `INTEGRATION` / P0 | [W3](../waves/W3-source-validation-and-admission.md) | PREPARE 가능 · ISSUE 대기 | [O4-E1-02](O4-E1-02-supplemental-labels.md), [O4-I0-02](O4-I0-02-matching-source-proof.md) |
| [O4-E1-04](O4-E1-04-precise-name-span.md) | 정확한 선언 이름 span과 unit 회수 평가 | `CODE_AND_PROOF` / P1 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |
| [O4-E1-05](O4-E1-05-untouched-holdout.md) | 독립 relevance와 미사용 holdout 발행 | `DATA_AND_PROOF` / P1 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |
| [O4-E1-06](O4-E1-06-final-pool-and-scoreboards.md) | 최종 합집합 검수·재채점·정책 판정 | `EXECUTION_AND_REPORT` / P1 | [W5](../waves/W5-final-scoring-and-policy.md) | PREPARE 가능 · ISSUE 대기 | [O4-E1-03](O4-E1-03-admission-and-split.md), [O4-E2-04](O4-E2-04-fresh-five-product-captures.md) |
| [O4-E1-07](O4-E1-07-bounded-bootstrap.md) | cold bootstrap의 결정적 bounded 계산 | `CONDITIONAL_CODE` / P2 | [W2](../waves/W2-repairs-and-selected-optimizations.md) | READY_TO_PREPARE · 조건 판정 우선 | 없음 |

## E2 — 외부 제품 native 범위·응답 경계·실제 캡처

[에픽 목적·배경·파일 지도](../epics/E2-external-capture-and-timing.md).

| 티켓 | 작업 | 종류 / 우선순위 | 기준 웨이브 | 착수 상태 | 선행 결과 |
| --- | --- | --- | --- | --- | --- |
| [O4-E2-01](O4-E2-01-native-completed-timer.md) | 외부 제품의 completed-response 시간 경계 | `CODE_AND_PROOF` / P1 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |
| [O4-E2-02](O4-E2-02-external-index-universe.md) | Sourcegraph·OpenGrok 전체 native 색인 범위 | `DATA_AND_PROOF` / P1 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |
| [O4-E2-03](O4-E2-03-required-cells-and-scheduling.md) | 필수 셀 inventory와 실패 분리 스케줄 | `INTEGRATION` / P0 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |
| [O4-E2-04](O4-E2-04-fresh-five-product-captures.md) | 5제품 실제 캡처와 blind union 반환 | `EXECUTION` / P1 | [W4](../waves/W4-native-capture-performance-and-scale.md) | PREPARE 가능 · ISSUE 대기 | [O4-E1-03](O4-E1-03-admission-and-split.md), [O4-E2-02](O4-E2-02-external-index-universe.md), [O4-E2-03](O4-E2-03-required-cells-and-scheduling.md), [O4-I0-02](O4-I0-02-matching-source-proof.md) |
| [O4-E2-05](O4-E2-05-semble-process-attribution.md) | Semble process 비용의 phase 귀속 | `CODE_AND_PROOF` / P2 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |
| [O4-E2-06](O4-E2-06-quality-only-warmup.md) | 기존 quality-only warmup=0 정책의 실제 parity | `PROOF_AND_CONFIG` / P1 | [W4](../waves/W4-native-capture-performance-and-scale.md) | READY_TO_PREPARE | 없음 |

## E3 — Active 선택·read-view lifetime·운영 계약

[에픽 목적·배경·파일 지도](../epics/E3-selection-and-operational-safety.md).

| 티켓 | 작업 | 종류 / 우선순위 | 기준 웨이브 | 착수 상태 | 선행 결과 |
| --- | --- | --- | --- | --- | --- |
| [O4-E3-01](O4-E3-01-active-selection-race.md) | Active 선택·retention·view 획득 경합 재현 | `PROOF_FIRST` / P0 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |
| [O4-E3-02](O4-E3-02-admission-pin-transfer.md) | 선택 admission pin의 read-view 이전 | `CONDITIONAL_CODE` / P0 | [W2](../waves/W2-repairs-and-selected-optimizations.md) | PREPARE 가능 · ISSUE 대기 · 조건 판정 우선 | [O4-E3-01](O4-E3-01-active-selection-race.md) |
| [O4-E3-03](O4-E3-03-atomic-active-query-rpc.md) | 단일 RPC의 Active 선택·검색·응답 결속 | `CODE_AND_PROOF` / P1 | [W2](../waves/W2-repairs-and-selected-optimizations.md) | PREPARE 가능 · ISSUE 대기 | [O4-E3-01](O4-E3-01-active-selection-race.md), [O4-E3-02](O4-E3-02-admission-pin-transfer.md) |
| [O4-E3-04](O4-E3-04-maintenance-health-metering.md) | 느린 디스크 metering과 readiness 분리 판정 | `PROOF_THEN_CONDITIONAL_CODE` / P1 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE · 조건 판정 우선 | 없음 |
| [O4-E3-05](O4-E3-05-publish-timeout-replay.md) | admitted publish timeout과 operation replay | `PROOF_THEN_CONDITIONAL_CODE` / P1 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE · 조건 판정 우선 | 없음 |
| [O4-E3-06](O4-E3-06-operator-event-proof.md) | 기존 operator diagnostics·process truth 검증 | `PROOF_ONLY` / P1 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |

## E4 — 인덱싱·typo 실행 비용·release 성능·scale

[에픽 목적·배경·파일 지도](../epics/E4-storage-query-and-scale.md).

| 티켓 | 작업 | 종류 / 우선순위 | 기준 웨이브 | 착수 상태 | 선행 결과 |
| --- | --- | --- | --- | --- | --- |
| [O4-E4-01](O4-E4-01-index-phase-profile.md) | 인덱싱 lifecycle 비용·resource 원인 분해 | `EXECUTION_AND_PROOF` / P1 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE | 없음 |
| [O4-E4-02](O4-E4-02-generation-durable-barriers.md) | generation durable publication의 그룹 barrier | `CONDITIONAL_CODE` / P1 | [W2](../waves/W2-repairs-and-selected-optimizations.md) | PREPARE 가능 · ISSUE 대기 · 조건 판정 우선 | [O4-E4-01](O4-E4-01-index-phase-profile.md) |
| [O4-E4-03](O4-E4-03-ascii-scanner-decision.md) | ASCII scanner 전체 호출 효과 판정 | `EXECUTION_THEN_CONDITIONAL_CODE` / P1 | [W1](../waves/W1-evidence-and-producers.md) | READY_TO_PREPARE · 조건 판정 우선 | 없음 |
| [O4-E4-04](O4-E4-04-source-token-authority.md) | source-bound distinct token authority | `CONDITIONAL_CODE` / P2 | [W2](../waves/W2-repairs-and-selected-optimizations.md) | PREPARE 가능 · ISSUE 대기 · 조건 판정 우선 | [O4-E4-01](O4-E4-01-index-phase-profile.md), [O4-E4-03](O4-E4-03-ascii-scanner-decision.md) |
| [O4-E4-05](O4-E4-05-release-scale-load.md) | matching release scale·load·restart 실행 | `EXECUTION_AND_PROOF` / P2 | [W4](../waves/W4-native-capture-performance-and-scale.md) | PREPARE 가능 · ISSUE 대기 | [O4-I0-02](O4-I0-02-matching-source-proof.md), [O4-E4-01](O4-E4-01-index-phase-profile.md) |
| [O4-E4-06](O4-E4-06-qualified-performance.md) | 동일 응답 경계의 정식 반복 성능 | `EXECUTION` / P1 | [W4](../waves/W4-native-capture-performance-and-scale.md) | PREPARE 가능 · ISSUE 대기 | [O4-I0-02](O4-I0-02-matching-source-proof.md), [O4-E1-03](O4-E1-03-admission-and-split.md), [O4-E2-01](O4-E2-01-native-completed-timer.md), [O4-E2-02](O4-E2-02-external-index-universe.md), [O4-E2-05](O4-E2-05-semble-process-attribution.md) |
| [O4-E4-07](O4-E4-07-policy-and-semantic-residuals.md) | 기본 typo·NL·semantic 잔여의 정책 RCA | `PROOF_THEN_CONDITIONAL_CODE` / P2 | [W5](../waves/W5-final-scoring-and-policy.md) | PREPARE 가능 · ISSUE 대기 · 조건 판정 우선 | [O4-E1-04](O4-E1-04-precise-name-span.md), [O4-E1-05](O4-E1-05-untouched-holdout.md), [O4-E1-06](O4-E1-06-final-pool-and-scoreboards.md) |

## I0 — 단일 통합 담당·source 검증·release 게이트

[에픽 목적·배경·파일 지도](../epics/I0-integration-and-release-gates.md).

| 티켓 | 작업 | 종류 / 우선순위 | 기준 웨이브 | 착수 상태 | 선행 결과 |
| --- | --- | --- | --- | --- | --- |
| [O4-I0-01](O4-I0-01-ownership-and-contract-freeze.md) | 공통 파일 소유권·계약·source epoch 관리 | `INTEGRATION` / P0 | [W0](../waves/W0-ownership-and-scope.md) | READY_TO_PREPARE | 없음 |
| [O4-I0-02](O4-I0-02-matching-source-proof.md) | 최종 source의 Contract·SDK·CI 검증 | `PROOF_AND_BUILD` / P0 | [W3](../waves/W3-source-validation-and-admission.md) | PREPARE 가능 · ISSUE 대기 | [O4-I0-01](O4-I0-01-ownership-and-contract-freeze.md) |
| [O4-I0-03](O4-I0-03-release-operational-gates.md) | SEP-21 release·paired producer·배포 게이트 | `RELEASE_GATE` / P1 | [W6](../waves/W6-release-and-final-closure.md) | PREPARE 가능 · ISSUE 대기 | [O4-I0-02](O4-I0-02-matching-source-proof.md) |

## 결과 인계

- actual source/dirty ownership·owned diff·shared proposal·selected commands/results/limits를 담당이 I0에 넘긴다.
- execution/data tickets는 기존 canonical raw/model/runtime/input/output 경로·revision·digest와 해당 scope 상태를 downstream에 넘긴다. 자세한 bundle 계약은 [실행 지도](../README.md)의 인계 표를 따른다.
- 선택 epoch에 들어간 코드만 검증·발행한다. 알려진 scope correctness 실패를 optional로 분류해 우회하지 않는다.
- 소스가 바뀌면 해당 I0 epoch·영향받는 capture/report를 다시 판정한다. qrel-only raw reuse도 실제 native binding을 검증한다.
- 실제 B07/B08/B09·SEP21 상태를 새 결과로 갱신한다. 실패 inventory와 요청된 전체 qualification 완료를 구분한다.
