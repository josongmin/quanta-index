# O4-E2-03 — 필수 셀 inventory와 실패 분리 스케줄

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E2 — 외부 제품 native 범위·응답 경계·실제 캡처](../epics/E2-external-capture-and-timing.md) / E2 담당 |
| 우선순위 / 종류 | P0 / `INTEGRATION` |
| 기준 웨이브 | [W1 — 근거·정답·producer 병렬 준비](../waves/W1-evidence-and-producers.md) |
| 실행 상태 | admission consumer·required-cell controller 통합 및 fixtures `VERIFIED`; 실제 ready matrix 실행은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

오래된 남은 셀 수나 살아 있는 watcher 대신 정확한 required cells와 repository별 terminal에 따라 실행을 진행한다.

## 배경과 현재 상태

iter_repository_admissions는 ready/failed를 drain하고 pending을 poll하는 기능이 이미 있다. original failed controller와 repaired external driver는 같은 실행이 아니다. compatibility별 native union indexing/membership projection도 이미 있다.

## 2026-10-04 중앙 검증

- 현재 controller는 repository별 canonical admission 결과와 입력 bundle을 검증하고 distinct output roots를 사용한다. failed member의 prevalidation은 ready sibling 실행을 막지 않으며 deadline/upstream failure와 not_run을 구분한다.
- child batch의 strict 입력과 matrix의 member-admission 입력을 분리했다. 실제 completed/failed/not_run count와 required-cell ledger를 발행한다. 현재 matrix의 Quanta/Semble 소비는 5제품 external capture 전체를 대신하지 않는다.
- `VERIFIED`: `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_native_span_projection.py tools/ci/tests/test_source_oracle_suite.py tools/ci/tests/test_holdout_review.py tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_retrieval_latency_status.py -q --tb=short` — 786 passed, 618.45s, exit 0. admission queue·matrix 반례가 포함된 중앙 배치 결과다.
- 실제 cohort admission/ready matrix 및 E2-04의 새 5제품 캡처는 `NOT_RUN`이다. historical helper 통과나 synthetic fixture를 actual controller execution으로 표시하지 않는다.

## 착수 입력

- 실제 cohort suites/packs와 product capability/profile/unit
- 각 repository admission terminal, 기존 capture raw/source/queries/binary identity, 새 output namespace

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/execution_batch.py](../../../../tools/benchmark/retrieval/execution_batch.py) | iter_repository_admissions / build_execution_pack / verify_execution_membership / project_scoring_view | READ: E1 소유 helper를 실제 external controller에서 소비한다. readiness logic 복제 금지. | READ |
| [tools/benchmark/retrieval/run.py](../../../../tools/benchmark/retrieval/run.py) | run_quality_batch / run_quality_matrix / verify_quality_matrix | E2가 required-cell inventory와 consumer 연결을 제안하고 I0가 shared driver를 반영한다. | SHARED |
| [tools/benchmark/retrieval/live_lexical_external.py](../../../../tools/benchmark/retrieval/live_lexical_external.py) | _selected_products / capture / verify | 셀 identity와 product-specific unsupported·failed outcome을 exact input에 연결한다. | OWNED |
| [tools/ci/tests/test_holdout_review.py](../../../../tools/ci/tests/test_holdout_review.py) | admission_queue controls | 기존 drain/final publish-race 검증을 소비하고 missing case만 E1에 제출한다. | READ |
| [tools/ci/tests/test_live_lexical_external.py](../../../../tools/ci/tests/test_live_lexical_external.py) | cell completion/refusal controls | 동일 cell 중복 실행·wrong repo/source·stale ready receipt를 거절한다. | OWNED |

## 실행 단계

1. cohort×repo×product×profile×unit을 current input에서 생성하고 required/unsupported/excluded 셀을 사전에 고정한다.
2. source/input/request/qrel/unit 변경이 영향을 주는 셀과 재사용 가능한 historical raw를 독립적으로 분류한다.
3. 외부 controller를 새 root에서 canonical readiness helper에 연결하고 actual ready/failed/pending을 확인한다.
4. deadline/upstream failure와 completed-empty를 구분하고 첫 실패가 ready sibling을 막지 않도록 실제 consumer를 실행한다.
5. 동일 service index 교체·output root·모델 job 경합은 resource owner를 하나로 고정하고 heavy product 실행은 직렬화한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_holdout_review.py -q -k 'admission_queue'`
- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_live_lexical_external.py -q`
- Positive: ready7+failed2 fixture에서 ready를 먼저 drain; final publish race 후 ready를 빠뜨리지 않음.
- Negative: process live but terminal FAILED, malformed/wrong-repo result, upstream 종료 후 missing, duplicate cell/output 경합 refusal.

## 완료 조건

- 모든 required cell ID에 실행·재사용·unsupported·failed·blocked·not_run 결정과 근거가 있다.
- ready cells가 다른 repository 실패 때문에 영구 대기하지 않고 입력 receipt bytes가 검증된다.

## 중단·거절·재개 조건

- scheduler helper 자체의 과거 unit pass를 actual repaired controller execution으로 표시하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
