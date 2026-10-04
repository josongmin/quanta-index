# O4-E2-04 — 5제품 실제 캡처와 blind union 반환

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P1 / `EXECUTION` |
| 기준 웨이브 | [W4 — 실제 캡처·성능·scale](../waves/W4-native-capture-performance-and-scale.md) |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | [O4-E1-03](O4-E1-03-admission-and-split.md), [O4-E2-02](O4-E2-02-external-index-universe.md), [O4-E2-03](O4-E2-03-required-cells-and-scheduling.md), [O4-I0-02](O4-I0-02-matching-source-proof.md) |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

최종 admitted cells를 실제 제품에 실행해 최신 비교용 raw를 만들고 새로운 미검수 candidates를 E1로 전달한다.

## 배경과 현재 상태

Quanta33캡처/11,272응답과 historical5제품21,815행은 별도의 source/시점이다. Gin exact1196, default/explicit typo, NL/literal API, ARB original/adapted를 서로 다른 request 계약으로 유지해야 한다.

## 착수 입력

- E2-03 required/reuse inventory, E1-03 issued admissions, I0-02 matching runner/daemon/proof
- 제품별 native indexed scope와 config/service/binary identity, fresh external roots
- Gin20 및 ARB 원문17/88 부분과 adapted88 historical 범위를 구별한 input manifest

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/live_lexical_external.py](../../../../tools/benchmark/retrieval/live_lexical_external.py) | capture / verify / BoundRelease | current producer로 fresh raw와 transport/status/complete clocks를 발행한다. product execution 성공을 scoring 성공과 구분한다. | OWNED |
| [tools/benchmark/retrieval/semble.py](../../../../tools/benchmark/retrieval/semble.py) | run_adapter / run_completed_worker | 현 resident worker·source/model lock·normalized output producer를 실행한다. | OWNED |
| [tools/benchmark/retrieval/arb_adapter.py](../../../../tools/benchmark/retrieval/arb_adapter.py) | 현재 input adapter | 원문 query/snapshot과 adapted query의 admission을 분리한다. 실제 요청 결함이 재현될 때만 변경한다. | OWNED |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | cmd_quanta / cmd_pair / run_quality_matrix / cmd_verdict | I0 소유 driver로 Quanta/Semble current-source capture와 replay를 실행한다. | READ |
| [tools/benchmark/retrieval/holdout_review.py](../../../../tools/benchmark/retrieval/holdout_review.py) | capture_review_pool | E1 helper로 product-blind fresh union을 생성해 E1-06에 전달한다. | READ |

## 실행 단계

1. runtime/source/model/license/request/source-scope preflight를 셀마다 확인한다. ready repository부터 serial products를 실행한다.
2. exact1196와 prefix/infix/components, typo4 default/explicit, no-answer, C3 NL240, Gin20, ARB, B09 OSA/CLARC/CSN의 native-mode case series를 개별 실행한다.
3. success/empty/unsupported/cap/partial/error/timeout/missing raw와 process exit를 모두 남기고 native response를 replay한다.
4. canonical source/clock/window/phase validator로 같은 record에 묶인 입력·상태·unit을 검증한다.
5. fresh top-k/source alternative union을 E1-06에 반환한다. 새 qrel revision이 request/pack binding에 영향을 주면 affected cells를 새 root에서 재실행한다.
6. historical reuse와 actual new calls를 final cell inventory에서 구분한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_live_lexical_external.py tools/ci/tests/test_retrieval_capture.py tools/ci/tests/test_lexical_five_product_oracle.py -q`
- `uv run --frozen --extra dev python tools/benchmark/retrieval/run.py quality-matrix --spec <issued-quality-matrix.json>`
- `uv run --frozen --extra dev python tools/benchmark/retrieval/run.py quality-matrix-verify --spec <same-quality-matrix.json>`
- `uv run --frozen --extra dev python tools/benchmark/retrieval/live_lexical_external.py --spec <issued-external-spec.json>`
- `uv run --frozen --extra dev python tools/benchmark/retrieval/live_lexical_external.py --verify <fresh-native-capture-root>`
- 실제 loaded spec의 outside-checkout output·제품·scope를 먼저 검증한다. quality matrix의 Quanta/Semble 실행과 별도 native external collector를 같은 required-cell inventory에서 join한다. 위 명령은 NOT_RUN이다.
- Raw independent replay: wrong request/source/indexed file set, duplicate file ranks, partial underfill, mismatched pack, unsupported normalization을 거절한다.

## 셀별 선택 조건

- 위 선행 결과는 해당 repository/product/capture epoch 범위에 적용한다. 실패한 sibling과 신규 unseen holdout은 ready 셀을 막지 않는다.
- E2-01 completed timer가 미완료이면 품질 raw는 historical transport boundary를 정확히 유지한 diagnostic으로만 발행하고 completed-response speed qualification은 NOT_RUN이다.
- E2-06 실제 parity가 미완료이면 quality spec은 기존 warmup1을 유지한다. warmup0을 선택한 셀은 E2-06 parity·protocol ledger가 선행 결과다.
- name metric·single-RPC·durable batching·token authority를 해당 epoch에 도입했으면 I0에서 그 producer/consumer 및 narrow rails를 먼저 통합한다.

## 완료 조건

- required cells가 모두 terminal outcome으로 설명되고 실제 호출 raw/exit/request/source/unit/clock identity가 replay된다.
- 모든 새 미검수 pair를 E1에 전달하고 scoring 가능한 input의 최종 iteration을 종료한다.

## 중단·거절·재개 조건

- full upstream CoIR/CORE/CSN import나 unseen/human/perf qualification은 이 캡처 자체로 달성하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
