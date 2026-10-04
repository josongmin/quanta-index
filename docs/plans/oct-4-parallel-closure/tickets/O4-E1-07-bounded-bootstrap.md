# O4-E1-07 — cold bootstrap의 결정적 bounded 계산

| 항목 | 값 |
| --- | --- |
| 에픽 / 담당 | [E1 — 정답·검수·admission과 독립 평가](../epics/E1-labels-admission-and-gold.md) / E1 담당 |
| 우선순위 / 종류 | P2 / `CONDITIONAL_CODE` |
| 기준 웨이브 | [W2 — 확인된 결함 수리·선택 최적화](../waves/W2-repairs-and-selected-optimizations.md) |
| 실행 상태 | `PLANNED` — 본 티켓의 구현·실행·검증은 `NOT_RUN` |
| 선행 결과 | 없음. 현재 source 확인과 fixture 준비부터 시작 가능 |

[전체 지도](../README.md) · [티켓 인덱스](INDEX.md)

## 목적

최초 평가 계산이 실제 병목일 때 기존 bootstrap method와 memory bound를 지키며 cold 계산 비용을 줄인다.

## 배경과 현재 상태

evaluator.mean_ci/_bootstrap_bounds는 deterministic paired/within-stratum percentile bootstrap과 bounded numeric cache를 이미 구현한다. cache-hit wall만으로 cold cost는 해결되지 않는다. 이 파일의 소유자는 E1이며 E4가 별도로 수정하지 않는다.

## 착수 입력

- cold numeric-only profile, paired delta/strata vectors, predeclared 10000 resamples/method contract
- fixed independent draw-index golden 및 scalar/reference implementation

## 어떤 파일을 어떻게 수정할지

`OWNED`는 에픽 담당 통합, `SHARED`는 I0 반영, `READ`는 기존 구현 소비다. 재현된 결함이나 채택된 계약 변경이 있을 때만 product source를 수정한다. 구현 파일과 독립 검증 파일을 함께 지정한다.

| 파일 | 함수 / 경계 | 구체적인 변경 또는 검증 | 모드 |
| --- | --- | --- | --- |
| [tools/benchmark/retrieval/evaluator.py](../../../../tools/benchmark/retrieval/evaluator.py) | mean_ci / _bootstrap_bounds / query_family_cluster_ci / repository_cluster_ci | bounded batches에서 stratum별 draw index를 공유하며 numeric kernel만 교체한다. evidence/source validation과 cache identity는 유지한다. | OWNED |
| [tools/ci/tests/test_retrieval_benchmark.py](../../../../tools/ci/tests/test_retrieval_benchmark.py) | bootstrap/cache tests | fixed indices mean/difference/CI, task order, degenerate strata, finite/memory negative를 검증한다. | SHARED |
| [benchmarks/retrieval/proof-required-tests.json](../../../../benchmarks/retrieval/proof-required-tests.json) | actual collected test identities | 새 test 등록은 I0가 collection identity에 맞춰 갱신한다. | SHARED |

## 실행 단계

1. 기존 valid replay 또는 고정 independent metric vectors로 cold numeric cost와 memory ceiling을 먼저 측정한다. E1-06의 최종 모델 검수 완료를 기다릴 필요는 없지만 최종 report acceptance는 그 입력에서 재생한다.
2. 기존 resampling/RNG 결과를 유지할지 단일 method를 명시적으로 진화시킬지 결정한다.
3. 기존 numeric cache 앞단의 source/evidence replay는 유지하면서 bounded numeric batch kernel을 구현한다.
4. 10,000 resamples, 현재 stratum/family/repository weighting·seed/order·percentile interpolation을 고정한 scalar/reference와 비교한다. RNG나 reduction rounding/method를 변경하면 현재 method/consumer를 함께 진화시키고 과거 bytes parity를 주장하지 않는다.
5. cold/on-cache/repeated-cache를 분리해 비용·bytes parity 또는 declared method change를 보고한다.

## 검증 계획 — NOT_RUN

아래는 실행할 명령/시나리오다. 본 문서에서 통과를 주장하지 않는다. `<...>`와 외부 root는 실행 전에 실제 값으로 확정한다. test filter는 실제 수집 ID를 확인하고 0 tests를 성공으로 표시하지 않는다.

- `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_benchmark.py -q -k 'bootstrap or cluster_ci or numeric_cache'`
- 실제 test collection에 맞는 selector를 확인하고 zero-selected를 pass로 취급하지 않는다.
- Negative: NaN/Inf, duplicate task, wrong strata, huge cache payload, out-of-order draw, hidden memory growth 거절.

## 완료 조건

- 독립 reference와 declared CI contract가 일치하고 cold compute/RSS가 목표 범위다.
- 병목이 재현되지 않으면 수정 없이 NOT_APPLICABLE과 profile 근거로 종료 가능하다.

## 중단·거절·재개 조건

- RNG 변경 후 과거 same-seed bytes가 동일하다고 주장하지 않는다.
- 필요한 입력 부재는 `BLOCKED`, 미실행은 `NOT_RUN`, 실제 실행 실패는 `FAILED`로 기록한다. 조건 미성립 `NOT_APPLICABLE`에는 실제 판단 근거가 필요하다.
- 변경이 source/input/query/unit/result에 영향을 주면 [I0 source gate](O4-I0-02-matching-source-proof.md)와 영향받는 capture/report를 다시 판정한다.
- 일회성 raw/log/capture/receipt는 checkout 밖 새 root에 둔다. 기존 외부 terminal을 덮어쓰지 않는다.

## 인계 결과

- 실제 source/dirty ownership, 변경 파일과 계약, 실행한 명령/selector, 관측 결과 및 제외 범위.
- raw/model/runtime/binary/input identity는 해당 실행 계약이 요구하는 범위에서 기록한다.
- 완료 조건별 `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE`과 후속 티켓에 넘길 입력을 발행한다.
