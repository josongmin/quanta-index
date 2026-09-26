# RBR-09 — Hybrid fetch 비용 측정과 제한된 정책 최적화

## 현행 판정 — 2026-09-26, `af640562` + dirty overlay

- 구현/검증: `MIN_INTERNAL_FETCH_K=100`은 그대로다. 서버 내부 단계 timing, floor 100/25/50 matrix, ANN quality guard, quiet-host p95와 정책 변경 proof는 `NOT_RUN`.
- 잔여 결정: RBR-01의 source-bound stage timing 후 같은 질의·filter·k에서 유한 matrix를 실행한다. 개선/품질/CI 근거가 부족하면 floor 100 **유지**. 측정 전 floor 축소나 순차 실행의 비용 비율 추정 금지. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P2. fetch matrix·stage 비용·quiet-host 효과 측정 `NOT_RUN`; floor 100 유지. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-01/02/03/07.
- 확인된 사실: hybrid fetch floor 100, semantic-only top-10 probe 11. 과거 지연의 원인 비율은 미측정.

## 파일·함수

- [hybrid/service.rs](../../../../crates/quanta-index-core/src/domains/hybrid/service.rs): `MIN_INTERNAL_FETCH_K`, `over_fetch_top_k`, admission ceiling.
- [routes/hybrid.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs): `execute_hybrid_fusion`.
- [semantic_query.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs): probe/window/explanation.
- [vector_index.rs](../../../../crates/quanta-index-semantic/src/vector_index.rs): `LoadedApproximateIndexV1::apply` effort/refine.

## 실험과 결정

1. RBR-01 trace로 lexical, embedding, dense/admission, fusion, projection/transport 비용과 호출/후보량을 분리한다. sequential 실행이라는 사실만으로 병렬화 이득을 계산하지 않는다.
2. 첫 fetch matrix는 baseline floor=100과 explicit experimental floor={25,50}. 유효 top-k/probe/ceiling 규칙을 그대로 지키고 `k={1,10,100}` 및 sparse constraints를 포함한다. 실험 profile/config hash로 고정하고 기본값은 유지한다.
3. 요청 fetch가 ANN ef/refine에 미치는 비용도 기록한다. ANN effort 변경과 fetch 변경은 별도 실험이다. RBR-07의 exact oracle로 후보 누락을 감시한다.
4. stage 데이터에서 lexical/dense 순차 실행이 지배적일 때만 bounded concurrency 한 후보를 추가한다. 동일 read view/pin/budget, cancellation, error precedence와 cleanup을 보존하고 무제한 spawn을 금지한다.
5. development의 quality·warm p95·resource로 한 후보만 선택해 실험 profile에 고정한다. 기본 정책 승격은 RBR-12에서 다른 변경과 묶인 단일 holdout의 quality guard와 warm p95 판정 후 결정한다. Semble의 native-default와 순수 lane 시간을 혼합하지 않는다.

## 회귀 테스트와 종료

- top-k 경계, `k+1` probe, exhaustive/capped window, dense admission refetch/ceiling, force-empty constraints.
- RRF/score/order fixture, duplicated lane IDs, one lane empty/error/timeout, deterministic ties.
- 같은 generation/read-view 사용, deadline/cancel 이후 진행/프로세스 누수 없음.
- 성능 기본값 변경은 [TEST-PLAN](TEST-PLAN.md) gate와 owning semantic/storage/daemon 결과가 모두 필요하다.
- 미세한 개선 또는 불확실한 CI만 있으면 기본값을 유지하고 raw frontier를 남겨 종료한다. 반복 latency 샘플은 독립 질의로 취급하지 않는다. `100 -> 10` 즉시 변경은 범위 밖이다.
