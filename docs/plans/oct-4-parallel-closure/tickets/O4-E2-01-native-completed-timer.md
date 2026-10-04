# O4-E2-01 — 외부 제품의 completed-response 시간 경계

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P1 / `CODE_AND_PROOF` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

Sourcegraph/OpenGrok/cs 요청 생성부터 normalized required response 완성까지 연속 clock을 기록해 Quanta/Semble과 같은 경계의 비교 입력을 만든다.

## 배경과 현재 상태

현 _http는 Request 생성 뒤 시작해 raw body read 후 끝난다. _process는 process/stdout 종료까지다. _sourcegraph/_opengrok은 raw persistence 후 response normalization을 호출하므로 기존 elapsed_ms는 decode/normalization을 포함하지 않는다. Quanta/Semble의 completed clock은 이미 있다.

## 착수 입력

- 현 live spec·raw response fixtures, canonical completed-output contract
- 독립 fake monotonic clock와 request construction/transport/decode/normalization 각각 비용을 가진 fixture

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/live_lexical_external.py](../../../../tools/benchmark/retrieval/live_lexical_external.py) | _http / _process / _sourcegraph / _opengrok / _cs / capture / verify | outer per-request clock을 구성 전부터 required normalized row materialization까지 둔다. raw persistence를 종료 뒤로 옮기고 recorded clock/output binding을 verifier에서 확인한다. | OWNED |
| [tools/ci/tests/test_live_lexical_external.py](../../../../tools/ci/tests/test_live_lexical_external.py) | native clocks/raw replay | fake clock golden과 empty/error/partial/timeout/oversize·row mutation controls를 추가한다. | OWNED |
| [tools/benchmark/retrieval/lexical_file_comparison.py](../../../../tools/benchmark/retrieval/lexical_file_comparison.py) | product_result / external result parsing | 새 boundary와 필드를 read/replay하도록 최소 수정한다. report 계산 변경은 E1과 I0가 반영한다. | SHARED |
| [tools/benchmark/retrieval/retrieval_contract.py](../../../../tools/benchmark/retrieval/retrieval_contract.py) | shared completed boundary owner | 새 이름이나 별도 semantic timer authority를 만들지 말고 I0가 기존 boundary에 결속한다. | SHARED |

## 실행 단계

1. 현재 transport clock consumers를 찾아 historical elapsed_ms의 의미와 새 complete observation 계약을 분리한다.
2. request 생성→transport/process→decode→source/row normalization→required serialization의 단일 same-process monotonic interval을 구현한다.
3. raw/sidecar persistence는 end 이후에 실행하고 telemetry bytes는 required response bytes에 섞지 않는다.
4. 성공·empty·적격 capped와 error/timeout/partial을 각각 status+clock sample policy로 처리한다.
5. native raw replay에서 normalized output digest/clock sample·boundary·task identity를 검사한다. 저장된 타이머를 재생 시간으로 덮지 않는다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_live_lexical_external.py tools/ci/tests/test_completed_response_timing.py -q`
- Positive: 구성/전송/정규화 각 비용이 fake clock의 독립 expected interval에 포함되고 저장 지연은 제외된다.
- Negative: clock 순서·task/path/status/output 변조, empty를 실패로 치환, partial top10 승격, 실패 표본을 제외한 전체 속도 주장 거절.

## 완료 조건

- 새 native 요청에서 canonical complete clock과 required normalized output bytes가 결속된다.
- historical transport timing은 같은 경계로 소급 승격되지 않으며 qualified 반복 실행은 E4-06에서 수행한다.

## 중단·거절·재개 조건

- transport+decode substage 숫자를 사후 합산해 연속 clock으로 위장하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
