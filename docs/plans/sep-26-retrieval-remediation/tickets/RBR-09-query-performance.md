# RBR-09 — Hybrid fetch 비용 측정과 제한된 정책 최적화

## 현행 판정 — 2026-09-26, `2c08dccf` + 공유 dirty

- 구현 관측: production 기본값 100을 유지하며 `HybridFetchFloorPolicy::{Floor25,Floor50,Floor100}`의 bounded startup selector를 추가했다. strict `parse`/`as_str`/`get`, dispatcher/config getter·builder와 `QUANTA_INDEX_EXPERIMENTAL_HYBRID_FETCH_FLOOR=25|50|100`을 연결한다. unset은 100이고 empty/alias/whitespace/범위 밖 값은 거부한다. hybrid·hybrid-seed·explain 재도출이 같은 선택 정책을 사용한다. benchmark CLI/spec/diagnostic canonical binding은 직렬 통합 소유자의 후속 범위다.
- 잔여 결정: RBR-01의 source-bound stage timing 후 같은 질의·filter·k에서 유한 matrix를 실행한다. 개선/품질/CI 근거가 부족하면 floor 100 **유지**. 측정 전 floor 축소나 순차 실행의 비용 비율 추정 금지. [현재 전수 판정](CURRENT-AUDIT.md).
- 규칙 보존: 기존 `over_fetch_top_k`는 floor100에 위임하며 이전 기본값과 동등하다. explicit floor는 기존 top-k clamp와 `max(overfetch,k+1)` continuation probe를 유지한다. dense admission의 배증/refill/ceiling, constraints·read-view·deadline·fusion·force-empty 무호출 규칙은 바꾸지 않는다. 기존 trace의 `hybrid.internal_top_k`는 실제 초기 fetch를 출력한다. Enabled/Disabled stage rail과 fixed lexical reserve는 별개 정책이다.
- 검증(local diagnostic): core floor 2 tests, search-plane floor 2 tests와 전체 lib 427 tests, daemon config 39 tests가 각각 exit 0이었다. core/search-plane/searchd all-target Clippy와 scoped format/diff check도 exit 0이며 추가 suppression은 없다. raw `/private/tmp/qi-hybrid-floor.pkIB8Q/`의 source-final-before/after는 14개 owning source/config/test 입력이 동일함을 보존한다. 기존 search-plane 425/config38 결과는 floor selector 추가 전 source의 역사 기록이며 이번 변경 증거로 재사용하지 않는다. 공유 전체 dependency/source closure와 installed SDK floor 실행은 별도 통합 범위다. 실제 floor raw matrix·외부 ANN guard·quiet-host p95·최종 clean-source 자격은 `NOT_RUN`; selector 구현을 성능 개선으로 승격하지 않는다.
- source 경계: 이 lane 중 외부 commit으로 HEAD가 `e3c87234`에서 `2c08dccf`로 이동했다. 14개 owning 입력의 전후 동일성만 확인했으며 서로 다른 HEAD의 실행을 단일 frozen-repository receipt로 합성하지 않는다. 최종 통합 source/config/binary custody는 중앙 소유자가 재발급한다.
- 실행 도구 감사: 현재 floor25/50 selector는 core/dispatcher/daemon/runner/spec 어느 경로에도 없다. `MIN_INTERNAL_FETCH_K=100`·`hybrid_probe_top_k_v1=max(overfetch,k+1)`은 그대로다. 이를 단순 matrix `NOT_RUN`으로만 표시하지 않고 실제 bounded experimental policy 구현 공백으로 남긴다. existing top-k 변경은 floor 비교의 대체가 아니다. `query_timing_overhead.py`는 on/off 결과·입력·sample equality를 검증하는 `diagnostic_unqualified` replay이며 quiet-host p95 자격이 아니다.

- 우선순위: P2. fetch matrix·stage 비용·quiet-host 효과 측정 `NOT_RUN`; floor 100 유지. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-01/02/03/07.
- 확인된 사실: hybrid fetch floor 100, semantic-only top-10 probe 11. 과거 지연의 원인 비율은 미측정.

## 파일·함수

- [hybrid/service.rs](../../../../crates/quanta-index-core/src/domains/hybrid/service.rs): `MIN_INTERNAL_FETCH_K`, `over_fetch_top_k`, admission ceiling.
- [config.rs](../../../../crates/quanta-index-searchd/src/app/config.rs), [runtime.rs](../../../../crates/quanta-index-searchd/src/app/runtime.rs): bounded floor env family·두 startup 경로·dispatcher wiring.
- [routes/hybrid.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/routes/hybrid.rs): `execute_hybrid_fusion`.
- [semantic_query.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs): probe/window/explanation.
- [vector_index.rs](../../../../crates/quanta-index-semantic/src/vector_index.rs): `LoadedApproximateIndexV1::apply` effort/refine.

### Owning local verification

- `./scripts/cargow test -p quanta-index-core --lib --locked hybrid_fetch_floor`: 2 passed/78 filtered, exit 0; closed selector·default100 independent formula·ceiling/refill boundary.
- `./scripts/cargow test -p quanta-index-search-plane --lib --locked hybrid_fetch_floor`: 2 passed/425 filtered, exit 0; floor25/50/100 × k1/10/100 actual backend fetch, force-empty no backend call, continuation/public-max/invalid-k.
- `./scripts/cargow test -p quanta-index-search-plane --lib --locked`: 427 passed, 0 failed/ignored/filtered, exit 0; default ranking/window/admission/explain regressions included.
- `./scripts/cargow test -p quanta-index-searchd --lib --locked app::config::tests`: 39 passed/54 filtered, exit 0; unset/known/invalid floor env and both-startup family fence.
- `./scripts/cargow clippy -p quanta-index-core -p quanta-index-search-plane -p quanta-index-searchd --all-targets --locked -- -D warnings`: final exit 0. Initial new-test `panic_in_result_fn` failure was repaired with typed test errors, not suppressed; historical log retained.
- `./scripts/cargow fmt -p quanta-index-core -p quanta-index-search-plane -p quanta-index-searchd -- --check` and scoped `git diff --check`: exit 0. Build/cache lock waits are not performance measurements.

## 실험과 결정

1. RBR-01 trace로 lexical, embedding, dense/admission, fusion, projection/transport 비용과 호출/후보량을 분리한다. sequential 실행이라는 사실만으로 병렬화 이득을 계산하지 않는다.
2. 첫 fetch matrix는 baseline floor=100과 explicit experimental floor={25,50}. 구현된 bounded startup policy를 benchmark의 canonical config/source/binary hash에 바인딩한 뒤 실행한다. 유효 top-k/probe/ceiling 규칙을 그대로 지키고 `k={1,10,100}` 및 sparse constraints를 포함한다. 서로 다른 k의 입력은 각각 comparison contract/query pack에 고정한다. 제품 기본값은 유지한다.
3. 요청 fetch가 ANN ef/refine에 미치는 비용도 기록한다. ANN effort 변경과 fetch 변경은 별도 실험이다. RBR-07의 exact oracle로 후보 누락을 감시한다.
4. stage 데이터에서 lexical/dense 순차 실행이 지배적일 때만 bounded concurrency 한 후보를 추가한다. 동일 read view/pin/budget, cancellation, error precedence와 cleanup을 보존하고 무제한 spawn을 금지한다.
5. development의 quality·warm p95·resource로 한 후보만 선택해 실험 profile에 고정한다. 기본 정책 승격은 RBR-12에서 다른 변경과 묶인 단일 holdout의 quality guard와 warm p95 판정 후 결정한다. Semble의 native-default와 순수 lane 시간을 혼합하지 않는다.

## 회귀 테스트와 종료

- top-k 경계, `k+1` probe, exhaustive/capped window, dense admission refetch/ceiling, force-empty constraints.
- RRF/score/order fixture, duplicated lane IDs, one lane empty/error/timeout, deterministic ties.
- 같은 generation/read-view 사용, deadline/cancel 이후 진행/프로세스 누수 없음.
- 성능 기본값 변경은 [TEST-PLAN](TEST-PLAN.md) gate와 owning semantic/storage/daemon 결과가 모두 필요하다.
- 미세한 개선 또는 불확실한 CI만 있으면 기본값을 유지하고 raw frontier를 남겨 종료한다. 반복 latency 샘플은 독립 질의로 취급하지 않는다. `100 -> 10` 즉시 변경은 범위 밖이다.
