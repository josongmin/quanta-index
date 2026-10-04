# W4 — 실제 캡처·성능·scale

[웨이브 전체 지도](../WAVES.md) · [에픽 지도](../README.md) · [티켓 인덱스](../tickets/INDEX.md)

- 상태: `PLANNED`. 본 문서의 product/model/capture/performance/release 실행은 `NOT_RUN`.
- 기준 배치 4개 티켓. 이 배치는 주된 수행 단계이며 PREPARE·후속 검증·새 epoch 반복은 다른 웨이브에서도 가능하다. 모든 티켓 종료를 한 번에 기다리는 전역 장벽이 아니다.

## 목적

필요한 warmup parity 뒤 native captures·반복 성능·tier별 load/restart를 실행한다.

## 기준 티켓·작업 범위

| 티켓 | 담당 | 작업 | 이 웨이브의 실행 범위 |
| --- | --- | --- | --- |
| [O4-E2-06](../tickets/O4-E2-06-quality-only-warmup.md) | E2 | 기존 quality-only warmup=0 정책의 실제 parity | 선택한 matching source에서 실제 Quanta/Semble task별 0/1 parity·각자 protocol ledger를 확인한다. |
| [O4-E2-04](../tickets/O4-E2-04-fresh-five-product-captures.md) | E2 | 5제품 실제 캡처와 blind union 반환 | issued admission·scope·matching binaries에서 ready repository의 5제품 native raw와 union을 발행한다. |
| [O4-E4-05](../tickets/O4-E4-05-release-scale-load.md) | E4 | matching release scale·load·restart 실행 | release tier256/4096/32768·open-loop·full/delta/delete/reopen·OS restart를 각 scope에서 판정한다. |
| [O4-E4-06](../tickets/O4-E4-06-qualified-performance.md) | E4 | 동일 응답 경계의 정식 반복 성능 | 같은 completed boundary·host admission·20tasks/5roots/1000warm observations에서 성능을 판정한다. |

## 진입 조건

- W3 same-source matching binaries·repository admission·required cells를 소비한다.
- 제품/ingest/scale/정식 성능은 같은 host의 resource admission에서 직렬 실행한다. ready 저장소·claim은 실패 sibling을 기다리지 않는다.

## 실행 lane과 조건

| lane | 담당 / 티켓 | 실제 실행·전제 |
| --- | --- | --- |
| 4A 선택 warmup0 | E2 / E2-06 | 현 matching source의 Quanta/Semble 0/1 task별 ranked rows/status/f64 parity·각자 protocol ledger·speed refusal. W1은 fixtures 준비이며 이곳에서 actual product proof를 소비/재검증한다. |
| 4B 품질 캡처 | E2 / E2-04 | E1-03·E2-02/03·I0-02 결과에서 5제품 required cells/native raw/union. warmup0 미입증이면 warmup1. completed timer 미준비 시 정확한 transport quality diagnostic 범위만 허용한다. |
| 4C tier/load/restart | E4 / E4-05 | I0-02·E4-01 이후 tier별 release scale와 offered/completed/drop·lifecycle/OS restart. posting-cap/timeout은 해당 실패/제한이다. |
| 4D 정식 속도 | E4 / E4-06 | I0-02·E1-03·E2-01·해당 E2-02/05, admitted host·동일 completed boundary·20 frozen tasks/5fresh roots/route당1000warm observations·warmup≥1·한 Quanta route |

## 내부 스케줄·소스 결속

1. 실제 workload 전 I0가 host/service index/model jobs·output root와 겹침을 검사한다.
2. warmup0를 선택할 때만 4A를 먼저 검증하고 protocol/config/admission binding을 W3 ISSUE에서 갱신한다. parity 실패/미실행이면0을 채택하지 않는다. 이때 source 변경이 있으면 W3 source gates도 다시 수행한다.
3. 각 admitted repository에서4B를 실행하고 immutable request/response·runtime/index_scope·required-cell outcomes와 blind union을 E1에 넘긴다. 다음 ready repository로 계속 간다.
4. 4C/4D는 입력이 준비된 범위에서 순서를 잡는다. 모든 캡처/XL tier 성공 뒤에만 작은 corpus baseline speed를 할 필요는 없다. 같은 host에서는 서로 겹치지 않는다.
5. W1 E2 timer/phase 또는 W2 SDK/storage/token 변경의 효과를 주장하려면 그 변경을 포함한 W3 epoch와 대응 oracle를 소비한다.

## 인계물·종료 조건

- native raw/commitment·scope·protocol/clock·union과 operational coverage를 기존 producer 형식으로 E1에 넘긴다.
- small-corpus speed와 large-corpus capacity/tail/restart는 별도 claim이다. Quanta-only면 외부 scope, Semble 미포함이면 Semble attribution은 해당 범위에 NOT_APPLICABLE이다.
- host admission 실패는 qualified timing BLOCKED/NOT_RUN이다. diagnostic sample을 적격 속도로 승격하지 않는다.
- required inventory가 완성됐더라도 missing/error/unsupported/underfill 셀의 비교 qualification은 남는다.

## 인계 규칙

- 정확한 source/dirty ownership·proposal·independent oracle·selected command/result·scope/limits를 넘긴다.
- raw/input/binary/model identity·경로/digest는 원래 producer와 선택 verification 계약이 요구하는 범위에서 기록한다. 일회성 output은 checkout 밖 fresh root다.
- OWNED/SHARED/READ 파일과 명령의 상세는 원래 티켓을 따른다. SHARED는 I0만 공유 checkout에 반영한다.
