# W2 — 확인된 결함 수리·선택 최적화

[웨이브 전체 지도](../WAVES.md) · [에픽 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md)

- 상태: `PLANNED`. 본 문서의 product/model/capture/performance/release 실행은 `NOT_RUN`.
- 기준 배치 5개 티켓. 이 배치는 주된 수행 단계이며 PREPARE·후속 검증·새 epoch 반복은 다른 웨이브에서도 가능하다. 모든 티켓 종료를 한 번에 기다리는 전역 장벽이 아니다.

## 목적

재현된 계약 실패를 수리하고 측정 근거가 있는 최적화를 선택한다.

## 기준 티켓·작업 범위

| 티켓 | 담당 | 작업 | 이 웨이브의 실행 범위 |
| --- | --- | --- | --- |
| [O4-E1-07](../tickets/O4-E1-07-bounded-bootstrap.md) | E1 | cold bootstrap의 결정적 bounded 계산 | cold 병목이 측정된 경우만 독립 draws/scalar reference에서 bounded numeric 계산을 수정한다. |
| [O4-E3-02](../tickets/O4-E3-02-admission-pin-transfer.md) | E3 | 선택 admission pin의 read-view 이전 | E3-01 반례/채택 계약에 필요한 bounded admission claim을 실제 view로 이전한다. |
| [O4-E3-03](../tickets/O4-E3-03-atomic-active-query-rpc.md) | E3 | 단일 RPC의 Active 선택·검색·응답 결속 | 선택 계약 뒤 variant별 snapshot/token 응답·SDK binding·실제 RPC count를 함께 변경한다. |
| [O4-E4-02](../tickets/O4-E4-02-generation-durable-barriers.md) | E4 | generation durable publication의 그룹 barrier | directory sync 병목이 확인되면 canonical durable writer의 group barriers와 crash cuts를 검증한다. |
| [O4-E4-04](../tickets/O4-E4-04-source-token-authority.md) | E4 | source-bound distinct token authority | scanner 판정 후에도 token scan이 지배하면 exhaustive OSA oracle와 lifecycle 비용으로 token authority를 검증한다. |

## 진입 조건

- 각 작업은 W1의 **실제 counterexample·선택 계약 또는 측정된 병목**을 요구한다.
- 이번 실행은 코드·정적 점검을 먼저 준비하고 검증은 I0 중앙 배치에서 수행한다. 필요한 새 counterexample/profile이 아직 실행되지 않았으면 해당 조건부 구현은 대기한다. 중앙 재현 결과가 나온 뒤 필요한 수리를 진행하고 W3에서 영향 검증을 모아 수행한다.
- baseline에 알려진 correctness failure가 있으면 해당 수리는 필수다. E3-03 single-RPC·E4 token/pack을 모든 baseline capture의 자동 선행으로 만들지 않는다.

## 내부 순서·채택 기준

| 경로 | 수행 순서 | 판정 |
| --- | --- | --- |
| Active custody | W1 E3-01→E3-02 반례 수리/비적용 판정→E3-03 | 실제 catalog/retention lock graph·bounded lifetime·variant별 snapshot/token/RPC·ABA/cursor/refusal |
| Storage | W1 E4-01→E4-02 | content/inherited-dir barrier→root publication→참조 안전 cleanup, syscall fault·process kill/reopen |
| Typo | W1 E4-01/03→E4-04 | after-scanner의 persistent 병목, exhaustive token/name witness·OSA completeness와 build/open/RSS/delta/delete 비용 |
| Scoring | 유효한 기존/최종 input cold profile→E1-07 | 독립 draws/scalar reference·seed/weight/percentile/reduction 계약; method 변경은 version/consumer 갱신 |
| W1 티켓 후속 수리 | E3-04/05·E4-03의 확인된 실패→원래 owner에서 수정 | 기존 권위에서 고치고 원래 반례/expected oracle를 재실행 |

## I0에 넘길 결과

- owned patch·SHARED proposal·producer/consumer 영향과 owner command/result, 이번 source에 포함할 수리/최적화 목록.
- 중앙 검증 전에는 준비한 fixture·정적 점검과 실행 예정 명령을 넘기고, owner 실행 결과는 `NOT_RUN`으로 유지한다. W2 구현 완료를 검증 완료로 치환하지 않는다.
- 조건 미성립은 실제 근거가 있을 때만 NOT_APPLICABLE이다. 미측정·입력 부재·미선택은 각각 NOT_RUN/BLOCKED/후속 epoch PLANNED로 남긴다.
- 미선택 single-RPC/token 등도 29개 backlog에서 삭제하지 않는다. 담당·후속 epoch·남은 proof를 원래 티켓에 기록한다.
- 채택한 변경은 W3 source gates와 W4 affected captures/performance를 요구한다. substage 개선·compile pass만으로 완료하지 않는다.

## 종료 조건

- 구현 인계는 producer/consumer·독립 회귀 fixture·정적 점검까지 준비한 상태다. 아래 실행 증거는 중앙 배치에서 판정하고, W3 admission ISSUE의 선행으로 소비한다.
- 선택 source의 알려진 correctness 결함이 독립 oracle에서 해결됐다.
- 채택한 최적화의 출력·lifecycle·비용 tradeoff가 owner 범위에서 검증됐다.
- immutable pack은 batch barrier 뒤에도 content full-sync가 지배할 때 별도 설계다. process crash proof를 실제 storage power-loss qualification으로 표시하지 않는다.

## 인계 규칙

- 정확한 source/dirty ownership·proposal·independent oracle·selected command/result·scope/limits를 넘긴다.
- raw/input/binary/model identity·경로/digest는 원래 producer와 선택 verification 계약이 요구하는 범위에서 기록한다. 일회성 output은 checkout 밖 fresh root다.
- OWNED/SHARED/READ 파일과 명령의 상세는 원래 티켓을 따른다. SHARED는 I0만 공유 checkout에 반영한다.
