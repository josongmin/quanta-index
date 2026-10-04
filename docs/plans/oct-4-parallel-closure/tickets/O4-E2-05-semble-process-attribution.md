# O4-E2-05 — Semble process 비용의 phase 귀속

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P2 / `CODE_AND_PROOF` |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

Semble worker 외부의 bootstrap/model/source/record 비용을 parent-bounded phase로 분해하고 반복 immutable 작업만 근거를 갖춰 줄인다.

## 배경과 현재 상태

과거 qg15는 process resource7.511초 vs worker1.447초 약6.064초 gap을 보였다. env/import/pip-freeze, model snapshot/digest, corpus copy, worker startup, validation/assembly가 후보다. query complete parent clock은 이미 있어 재구현하지 않는다.

## 착수 입력

- 고정 Semble package/lock/env와 model revision/assets
- same input source/pack, fixed native row fixtures, parent/worker clock 도메인

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/semble.py](../../../../tools/benchmark/retrieval/semble.py) | check_semble_env / build_isolated_corpus / materialize_model_cache / run_completed_worker / run_adapter / assemble_record | 현재 parent lifecycle에 bounded preparation/validation phases를 넣고 source/model bytes 검증과 required row parity를 유지한다. | OWNED |
| [tools/benchmark/retrieval/semble.py](../../../../tools/benchmark/retrieval/semble.py) | validate_worker_phase_timings / validate_native_profile_report | 같은 clock domain에서 child bounds/complete phases를 검사한다. parent-worker timestamp subtraction 금지. | OWNED |
| [tools/ci/tests/test_completed_response_timing.py](../../../../tools/ci/tests/test_completed_response_timing.py) | Semble parent-normalized output tests | same-size wrong row, partial worker line, changed model/source와 phase order/bounds mutants를 확장한다. | OWNED |
| [tools/ci/tests/test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) | Semble adapter/source contract tests | 테스트 hunk는 I0가 통합하며 file 전체를 동시에 수정하지 않는다. | SHARED |

## 실행 단계

1. 현재 process phases를 실제 호출 순서와 부모 envelope로 지도화한다.
2. 준비·worker startup·query·validation·record assembly를 같은 parent monotonic domain에서 분해한다.
3. matching source/model immutable 작업이 반복되고 material cost가 있음을 작은 capture로 입증한다.
4. 중복 작업을 제거할 경우 cache/source/model identity를 cross-run 정확히 recheck하고 stale/mutated assets를 거절한다.
5. query clock과 process prep/resource wall을 별도 보고해 E4-06 equal-boundary profile에 제공한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_completed_response_timing.py tools/ci/tests/test_retrieval_benchmark.py -q -k 'semble or completed or worker_template'`
- Positive: normalized rows/status bytes 및 phase parent bounds 보존.
- Negative: stale env/model assets, phase missing/reorder, request decode omission, worker output mutation, unrelated source cache reuse 거절.

## 완료 조건

- process gap이 측정 phase나 명시적 unattributed residual로 설명되고 query boundary는 유지된다.
- 재사용 최적화는 immutable validation과 native output parity가 입증된 작업에만 적용한다.

## 중단·거절·재개 조건

- 지난 gap6초를 현재 source의 비용이나 달성될 speedup으로 확정하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
