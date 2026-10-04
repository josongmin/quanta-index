# O4-E2-06 — 기존 quality-only warmup=0 정책의 실제 parity

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P1 / `PROOF_AND_CONFIG` |
| 기준 웨이브 | [W4 — 실제 캡처·성능·scale](../waves/W4-native-capture-performance-and-scale.md) |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

품질 전용 실행의 불필요한 warmup을 제거하되 normalized result와 request/phase 권위를 유지한다.

## 배경과 현재 상태

run.py는 query_warmup_passes=0을 이미 허용하고 qualified speed는 1회 이상을 요구한다. build_query_protocol은 warmup/measurement shuffle에 하나의 RNG를 사용하므로 같은 seed라도 warmup 수가 바뀌면 measured schedule이 달라진다. 실제 helper에서 8tasks/seed7/2repetitions의 0/1 순서 차이를 확인했다. 이 source probe는 제품 결과 parity 증거가 아니다. 과거 qg15 warmup 합계 18.707초는 새 wall 절감 예측값으로 사용하지 않는다.

## 착수 입력

- 작은 fixed corpus/suite/query protocol, matching Quanta runner/daemon와 Semble environment
- 동일 task set/cold probe/profile/seed/repetitions의 0/1 두 quality-only spec; 각 protocol SHA·실제 measured schedule과 새 외부 output roots. 같은 seed의 측정 순서 동등성을 가정하지 않는다.

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | build_query_protocol / validate_qualified_speed_spec / load_spec / run_quality_batch | 기존 품질 spec producer에 0을 명시하고 protocol/phase ledger를 소비한다. 실제 누락만 I0를 통해 수정한다. | SHARED |
| [tools/benchmark/retrieval/semble.py](../../../../tools/benchmark/retrieval/semble.py) | run_adapter / query protocol validation | shared protocol의 zero-warmup schedule과 실제 measured events를 재생한다. 새 mode/clock을 만들지 않는다. | OWNED |
| [benchmarks/retrieval/src/main.rs](../../../../benchmarks/retrieval/src/main.rs) | query_protocol / warmup_schedules / emitted warmup_passes | 현 runner의 cold/warmup/measured row identity를 확인한다. 변경이 필요하면 I0 소유 hunk로 제출한다. | SHARED |
| [tools/ci/tests/test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) | test_exploratory_query_protocol_accepts_explicit_zero_warmups / validate_qualified_speed_spec controls | 기존 zero acceptance·speed refusal을 실행하고 request/phase identity가 없는 경우 거절하는 independent fixture를 보강한다. | SHARED |

## 실행 단계

1. task set/cold probe/profile/seed/repetitions를 고정하고 warmup 수만 설정상 바꾼다. canonical helper로 각 protocol을 생성해 별도 SHA와 실제 schedule을 보존한다. measured order·전체 protocol bytes가 같다고 표시하지 않는다.
2. 현 runner/adapter로 동일 fixture를 실제 실행한다. (task_id, measured repetition)으로 정확히 대응해 그 task의 ranked rows/order/status/score bits를 비교하고, 각 원본 phase ledger는 자기 protocol과 replay한다. 전체 event 순서를 강제로 맞추거나 hash를 재작성하지 않는다. identity hashing과 record persistence는 timer 밖에 둔다.
3. cold·warmup·measured request ledger와 canonical schema를 검증하고 zero-warmup에서 가짜 warm observation을 발행하지 않는다.
4. qualified-speed zero-warmup 거절을 유지하고 zero quality result에는 speed qualification을 발행하지 않는다.
5. 실제 parity가 확인된 cohort의 quality spec만 0으로 바꾸고 절감된 phase/wall을 별도 diagnostic으로 보고한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_benchmark.py -q -k 'zero_warmups or qualified_speed or query_protocol'`
- 실제 0/1 Quanta+Semble fixture capture: 같은 task/repetition의 measured rows/status/score bits, 각자의 request/phase ledger와 protocol SHA, elapsed clock 범위를 비교한다. order-sensitive 결과 차이가 있으면 warmup0을 채택하지 않는다.
- Negative: speed-mode warmup0, protocol/spec mismatch, 같은 row count지만 후보가 다른 응답, 누락 measured phase 거절.

## 완료 조건

- 실제 task별 0/1 parity·schedule 차이를 포함한 ledger replay·zero-warmup speed refusal이 있고 quality specs에 정책이 명시돼 있다. 같은 measured order로만 격리된 비용 실험이 필요하면 별도 method 계약을 결정한다; 현재 protocol을 변조하지 않는다.
- 실측 없는 wall 절감이나 qualified latency를 주장하지 않는다.

## 중단·거절·재개 조건

- 실제 runner/model 환경이 없으면 parity는 BLOCKED/NOT_RUN으로 남기고 기존 warmup1을 유지한다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
