# W1 — 근거·정답·producer 병렬 준비

[웨이브 전체 지도](../WAVES.md) · [에픽 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md)

- 상태: `PLANNED`. 본 문서의 product/model/capture/performance/release 실행은 `NOT_RUN`.
- 기준 배치 14개 티켓. 이 배치는 주된 수행 단계이며 PREPARE·후속 검증·새 epoch 반복은 다른 웨이브에서도 가능하다. 모든 티켓 종료를 한 번에 기다리는 전역 장벽이 아니다.

## 목적

라벨/독립 oracle·native scope/timer/controller·safety 반례·전체 비용을 준비한다.

## 기준 티켓·작업 범위

| 티켓 | 담당 | 작업 | 이 웨이브의 실행 범위 |
| --- | --- | --- | --- |
| [O4-E1-01](../tickets/O4-E1-01-original-review-resume.md) | E1 | 원본 C3 검수 실패 복구와 실제 판단 발행 | 원본 FAILED별 raw/cache/source를 재대조하고 실제 reviewer 역할 실행을 재개한다. |
| [O4-E1-02](../tickets/O4-E1-02-supplemental-labels.md) | E1 | 미검수 합집합 검수와 원본 라벨 병합 | 해당 repository 원본 결과 뒤에 supplemental pair를 실제 검수·adjudication해 merged qrels를 발행한다. |
| [O4-E1-04](../tickets/O4-E1-04-precise-name-span.md) | E1 | 정확한 선언 이름 span과 unit 회수 평가 | name byte-span 독립 oracle·negative cases·native witness/evaluator proposal를 준비한다. |
| [O4-E1-05](../tickets/O4-E1-05-untouched-holdout.md) | E1 | 독립 relevance와 미사용 holdout 발행 | 미사용 corpus/family·license·exposure·quota/underfill을 고정하고 독립 relevance를 준비한다. |
| [O4-E2-01](../tickets/O4-E2-01-native-completed-timer.md) | E2 | 외부 제품의 completed-response 시간 경계 | request construction→normalized response 완료 clock을 기존 transport clock과 별도 구현·검증한다. |
| [O4-E2-02](../tickets/O4-E2-02-external-index-universe.md) | E2 | Sourcegraph·OpenGrok 전체 native 색인 범위 | SG/OG native indexed universe·source mapping·전후 scope producer를 실제 backend와 대조한다. |
| [O4-E2-03](../tickets/O4-E2-03-required-cells-and-scheduling.md) | E2 | 필수 셀 inventory와 실패 분리 스케줄 | required-cell inventory와 ready/failed helper를 controller에 연결하고 failure cases를 검증한다. |
| [O4-E2-05](../tickets/O4-E2-05-semble-process-attribution.md) | E2 | Semble process 비용의 phase 귀속 | Semble child phases와 parent residual을 실제 경계로 귀속한다. |
| [O4-E3-01](../tickets/O4-E3-01-active-selection-race.md) | E3 | Active 선택·retention·view 획득 경합 재현 | G1 선택→G2 활성화→G3 retention→G1 acquire 반례와 현 계약을 결정적 fixture에서 판정한다. |
| [O4-E3-04](../tickets/O4-E3-04-maintenance-health-metering.md) | E3 | 느린 디스크 metering과 readiness 분리 판정 | 3cadence 이상 slow walk의 freshness/readiness를 관측하고 실제 수리 필요성을 판정한다. |
| [O4-E3-05](../tickets/O4-E3-05-publish-timeout-replay.md) | E3 | admitted publish timeout과 operation replay | admitted slow publish에서 timeout/disconnect 뒤 inspect·exact replay·ACK/state를 검증한다. |
| [O4-E3-06](../tickets/O4-E3-06-operator-event-proof.md) | E3 | 기존 operator diagnostics·process truth 검증 | 기존 owner-binary diagnostics cases를 소비해 누락된 ring/auth/scope/peer proof만 보완한다. |
| [O4-E4-01](../tickets/O4-E4-01-index-phase-profile.md) | E4 | 인덱싱 lifecycle 비용·resource 원인 분해 | full/delta/delete/no-op/reopen 비용·resource·ingress scope를 같은 부모/자식 clock에서 분해한다. |
| [O4-E4-03](../tickets/O4-E4-03-ascii-scanner-decision.md) | E4 | ASCII scanner 전체 호출 효과 판정 | admitted host에서 ASCII whole-call parity/비용을 측정해 유지·수정·철회를 판정한다. |

## 진입 조건

- W0의 ownership·host resource·출력 namespace를 소비한다.
- source 조사·독립 fixture·proposal는 시작 가능하다. 실제 model/외부 service/ingest jobs는 resource admission을 먼저 적용한다.

## 담당별 병렬 실행

이번 배치에서는 아래 작업의 코드·fixture·입력 준비와 정적 점검을 먼저 병렬로 진행한다. 표의 실제 검수·재현·비용 측정·owner commands/results는 I0의 중앙 실행 배치에서 수행하며, 준비만으로 통과를 주장하지 않는다. root는 E1/I0를 겸하고 E2/E3/E4에 각각 한 슬롯을 배정한다.

| 담당 | 내부 순서와 실행 범위 | 인계물 |
| --- | --- | --- |
| E1 | repository별 E1-01→E1-02. 같은 owner가 E1-04 name oracle·E1-05 holdout/gold를 준비하고 **E1-03 admission producer PREPARE**도 수행한다. 같은 evaluator/review 파일 patch는 순서대로 통합한다. | 실제 raw 역할 provenance·merged qrels·name/gold/coverage/underfill·split/producer proposals |
| E2 | E2-01 timer·E2-02 native scope·E2-03 controller·E2-05 Semble phase를 준비한다. live collector/adapter의 공통 파일은 한 owner가 순서대로 합친다. E2-06의 0/1 spec·protocol·fixtures도 먼저 준비한다. | code-ready collector/controller·필수 셀 inventory·scope/clock/phase oracle와 입력 |
| E3 | E3-01 race, E3-04 slow walk, E3-05 timeout/replay, E3-06 실제 operator owner cases를 수행한다. 각 결과를 현 계약상 허용·실제 결함·미실행으로 구분한다. | 결정적 counterexample/disposition·independent expected result·owner commands/results |
| E4 | E4-01 lifecycle profile과 E4-03 whole-call ASCII 판정을 동일 source/host 범위에서 수행한다. backend/ingress·parent/child·RSS/disk 의미를 분리한다. | measured bottleneck·ASCII 유지/수정/철회·독립 출력/lifecycle oracle |
| I0 | SHARED proposal를 정리하고 **I0-03 release 준비**로 real-provider/Linux/paired/restore/actions의 입력·target 부재를 일찍 확인한다. | 선택 epoch 후보·missing input/target owner·release 준비 목록 |

## 종료와 다음 웨이브

- 준비된 repository/claim마다 labels·scope·producer patch와 독립 fixture를 I0에 넘긴다. 모든 repository·holdout·최적화가 끝날 때까지 global wait를 하지 않는다.
- 선택 scope에 실제 correctness 결함이 있으면 W2 수리 후 W3로 간다. 결함이 없고 W2 최적화를 포함하지 않는 baseline은 W3로 진행한다.
- 새 실행 증거가 필요한 조건은 fixture/profile 준비 후 I0 중앙 배치에서 판정한다. 그 결과 수리가 필요하면 W2→W3 영향 재검증으로 돌아간다. 미실행 반례를 확인된 결함으로 표시하지 않는다.
- E3-04/05 또는 E4-03에 수리가 필요하면 원래 티켓의 code phase를 W2에서 계속 수행한다. proof-only 성공으로 수리 완료를 대체하지 않는다.
- full C3·name/NL·holdout quota가 부족한 범위는 FAILED/BLOCKED/NOT_RUN으로 남긴다. 준비된 file/NL 셀을 발행할 수 있어도 전체 qualification은 남는다.
- E1-05 holdout은 정책 후보/acceptance가 고정되기 전 결과로 튜닝하지 않는다. exposure가 생기면 development로 바꾸고 새 holdout을 요구한다.

## 인계 규칙

- 정확한 source/dirty ownership·proposal·independent oracle·selected command/result·scope/limits를 넘긴다.
- raw/input/binary/model identity·경로/digest는 원래 producer와 선택 verification 계약이 요구하는 범위에서 기록한다. 일회성 output은 checkout 밖 fresh root다.
- OWNED/SHARED/READ 파일과 명령의 상세는 원래 티켓을 따른다. SHARED는 I0만 공유 checkout에 반영한다.
