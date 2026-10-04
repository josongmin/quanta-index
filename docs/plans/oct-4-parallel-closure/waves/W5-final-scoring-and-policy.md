# W5 — 최종 검수·재채점·정책 판정

[웨이브 전체 지도](../WAVES.md) · [에픽 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md)

- 상태: `PLANNED`. 본 문서의 product/model/capture/performance/release 실행은 `NOT_RUN`.
- 기준 배치 2개 티켓. 이 배치는 주된 수행 단계이며 PREPARE·후속 검증·새 epoch 반복은 다른 웨이브에서도 가능하다. 모든 티켓 종료를 한 번에 기다리는 전역 장벽이 아니다.

## 목적

새 candidate pool을 실제 검수하고 독립 gold/holdout에서 정책을 판정한다.

## 기준 티켓·작업 범위

| 티켓 | 담당 | 작업 | 이 웨이브의 실행 범위 |
| --- | --- | --- | --- |
| [O4-E1-06](../tickets/O4-E1-06-final-pool-and-scoreboards.md) | E1 | 최종 합집합 검수·재채점·정책 판정 | fresh candidate union의 새 pair를 실제 검수하고 final qrel·분모/CI/coverage를 재생한다. |
| [O4-E4-07](../tickets/O4-E4-07-policy-and-semantic-residuals.md) | E4 | 기본 typo·NL·semantic 잔여의 정책 RCA | name/독립 holdout/final scoring 결과에서 default23·Gin4·NL/semantic 원인과 정책을 판정한다. |

## 진입 조건

- 준비된 W4 cohort의 fresh native union·raw/request/index/source binding을 소비한다.
- E4-07의 최종 정책 판정에는 E1-04 name, E1-05 independent holdout, E1-06 final scoring의 해당 결과가 필요하다. RCA 준비는 먼저 가능하다.

## 내부 실행 순서

| 순서 | 담당 / 티켓 | 결과와 다음 전제 |
| --- | --- | --- |
| 5A 마지막 검수 | E1 / E1-06 | 새 candidate pair 실제2review+adjudication→final qrel/suite/admission revision; unknown 합성 금지 |
| 5B raw 재채점 | E1 / E1-06 | native binding이 허용하면 기존 scoring projection, 허용하지 않으면 영향 셀을 W3/4에서 새로 실행 |
| 5C scoreboards | E1 / E1-06 | file/name/symbol/NL·default/explicit·cohort별 분모/제외/coverage/CI와 pool exposure |
| 5D 정책 판정 | E4 / E4-07 | default23·Gin4·NL/semantic을 독립 gold에서 원인 분류→필요한 ablation→사전 acceptance/미사용 holdout 판정 |

## 병렬 처리·반복 조건

- ready cohort의 file/NL report는 새 holdout·name 전체 완료를 기다리지 않는다. 미준비 name/unseen claim은 NOT_RUN/BLOCKED로 남는다.
- E4는 E1 final scoring 준비 중 development RCA/fixture를 준비할 수 있다. 최종 정책 판정은 해당 독립 결과 뒤에 한다.
- qrel만 바뀌어도 native record에 새 suite/qrel digest를 덮어쓰지 않는다. 기존 raw reuse가 허용되는지 실제 binding에서 판단한다.
- E1-07 numeric source가 바뀌면 independent scorer/method proof·영향 report를 다시 수행한다.
- SDK/planner/ranking/model/chunking/policy source가 바뀌면 W3 validation/admission→W4 affected native cells→W5 report를 다시 수행한다. 과거 속도 효과를 새 source 결과로 전용하지 않는다.
- holdout 실패를 보고 튜닝하면 그 population은 development로 전환한다. 새 정책 qualification에는 새 미사용 holdout이 필요하다.

## 인계물·종료 조건

- 각 준비된 cohort의 final qrel/revision·실제 검수 provenance·scoreboard/분모/coverage/CI가 raw에서 재생된다.
- 정책 유지/변경과 원인별 근거·acceptance·critical strata·남은 name/NL/holdout underfill을 원래 티켓에 기록한다.
- full C3/5제품/name/NL/unseen 요청 중 미충족은 잔여다. 최종 report inventory만으로 전체 완료를 선언하지 않는다.

## 인계 규칙

- 정확한 source/dirty ownership·proposal·independent oracle·selected command/result·scope/limits를 넘긴다.
- raw/input/binary/model identity·경로/digest는 원래 producer와 선택 verification 계약이 요구하는 범위에서 기록한다. 일회성 output은 checkout 밖 fresh root다.
- OWNED/SHARED/READ 파일과 명령의 상세는 원래 티켓을 따른다. SHARED는 I0만 공유 checkout에 반영한다.
