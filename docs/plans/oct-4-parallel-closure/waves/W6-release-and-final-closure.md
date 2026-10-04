# W6 — release·운영·전체 잔여 판정

[웨이브 전체 지도](../WAVES.md) · [에픽 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md)

- 상태: `PLANNED`. 본 문서의 product/model/capture/performance/release 실행은 `NOT_RUN`.
- 기준 배치 1개 티켓. 이 배치는 주된 수행 단계이며 PREPARE·후속 검증·새 epoch 반복은 다른 웨이브에서도 가능하다. 모든 티켓 종료를 한 번에 기다리는 전역 장벽이 아니다.

## 목적

CODE/release/actions를 별도 증거로 판정하고 모든 29개 티켓의 미완료를 남긴다.

## 기준 티켓·작업 범위

| 티켓 | 담당 | 작업 | 이 웨이브의 실행 범위 |
| --- | --- | --- | --- |
| [O4-I0-03](../tickets/O4-I0-03-release-operational-gates.md) | I0 | SEP-21 release·paired producer·배포 게이트 | P00–P12·paired/real-provider/Linux/restore/actions의 실제 결과로 CODE와 운영 상태를 따로 판정한다. |

## 진입·조기 착수

- I0-03 input/target inventory는 W1부터 준비한다. CODE source qualification은 W3의 selected source가 준비됐을 때 자체 registry closure로 진행 가능하다.
- 제품 quality/unseen/performance를 포함한 출하 주장은 W5 final labels/policy와 W4 대응 proof도 추가 요구한다.
- W5나 후속 W2 source가 바뀌었으면 W3와 영향 proof/captures/report를 갱신한 current source를 소비한다.

## 분리된 gate

| gate | 필요한 실제 결과 | 부재 시 |
| --- | --- | --- |
| CODE_QUALIFIED | registry P00/P01/P02A/P02B, P03–P10 owner+release scopes, P11 exact source pair·resolved graph·QBC/binary custody와 code-gate acceptance | 해당 FAILED/BLOCKED/NOT_RUN; owner-only pass를 release로 승격 금지 |
| real-provider/Linux/restore | 실제 provider policy·Linux fresh binary signals/child-loss/P09 process truth·P10 current-format custody/backup/verify/restore-forward | actual input/target 부재를 남기고 scope 미완료 |
| P11 deployment/activation/rollback | 현재 recipe가 없는 actions의 typed authority/manifest/checker와 실제 authorized target·단계별 pre/post 결과 | 선언된 registry만으로 executable/passed promotion 금지 |
| P12 aggregate | current p12a-proof-infrastructure prerequisite·정확한 source pair의 required manifests와 require-all/bind-source acceptance | forged/partial/stale result로 aggregate 발행 금지 |
| 제품 출하 주장 | final quality·독립 name/NL/holdout·same-boundary performance·주장하는 tier의 scale/restart 결과 | 준비된 partial diagnostic과 전체 qualification을 구분 |

## 전체 29개 티켓 종료 판정

1. 각 원래 티켓의 scope·owner·actual result·남은 입력/명령·후속 epoch를 점검한다. 기존 B07/B08/B09·SEP21 owning status를 갱신한다.
2. W2 미선택 single-RPC/token/numeric/storage와 W1 미충족 labels/holdout은 실제 잔여로 유지한다. baseline 성공을 그 작업 완료로 치환하지 않는다.
3. 조건 미성립 NOT_APPLICABLE에는 실제 근거가 있어야 한다. 입력 부재/미실행은 BLOCKED/NOT_RUN이며 마감의 대체가 아니다.
4. CODE_QUALIFIED·DEPLOYED·ACTIVATED·ROLLBACK_PROVEN은 각 필수 proof가 충족된 범위에서만 발행한다.
5. 제품 데이터·runtime 결과가 부족하면 정확히 남은 scope와 인계를 표시한다. 상태 ledger 정리와 qualification 달성을 분리한다.

## 종료 조건

- 요청된 각 qualification에 actual prerequisites/current source/result가 있다.
- 없으면 해당 작업은 미완료이며 blockers·담당·재개 조건을 원래 티켓에 남긴다.
- 이 문서는 계획이다. 현재 사용자 요청이 제품 실행·배포 action 착수를 뜻하지 않는다.

## 인계 규칙

- 정확한 source/dirty ownership·proposal·independent oracle·selected command/result·scope/limits를 넘긴다.
- raw/input/binary/model identity·경로/digest는 원래 producer와 선택 verification 계약이 요구하는 범위에서 기록한다. 일회성 output은 checkout 밖 fresh root다.
- OWNED/SHARED/READ 파일과 명령의 상세는 원래 티켓을 따른다. SHARED는 I0만 공유 checkout에 반영한다.
