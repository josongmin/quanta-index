# RBR-01 — SDK 응답과 단계별 실행 정보 보존

## 현행 판정 — 2026-09-26, 관측 HEAD `8c0c1242` + 공유 dirty overlay

- 구현: `SearchExplanation.stage_timings`의 typed server-monotonic elapsed/calls/returned-candidate DTO를 lexical/semantic/hybrid의 실제 prepare/read-view/search/embed/admission/fusion/project 경계에 추가했다. lexical `TextQueryResponse`도 공통 explanation을 실어 SDK→diagnostic v4로 전달한다. diagnostic replay는 route별 필수·순서·중복·count·request ID를 검증한다. dense fetch는 refill의 중복 출력 누계이고 dense admission에 중첩되므로 합산 총시간이 아니다. ingest publish stage는 아직 공개 경로가 없다.
- lexical `project` 시간은 response-budget fit 이전에 멈추고 후보 수만 최종 fitted page에 맞춘다. 따라서 이 stage 시간은 직렬화·cursor budget 비용을 포함한다고 해석하지 않는다. force-empty plan은 backend 호출 없이 prepare/project만 기록한다.
- 후속 validator 보강(공유 dirty local): diagnostic v4에서 route stage의 실제 호출·후보 수를 `engines_executed`, `engines_touched`, `strategy`와 대조한다. lexical의 budget-fit 후 기여, semantic의 optional lexical scope/zero-hit, hybrid의 lexical/dense admission 조합을 구분한다. 누락·위조된 engine/strategy가 구조적으로 정상인 timing 배열만으로 통과하지 못한다. 합성 fixture와 보존된 실제 3-route v4 sidecar replay는 통과했고, 그 실제 sidecar의 3 route × 3 모순 필드 변조 9건은 모두 거부했다. 새 daemon capture·clean-source receipt는 `NOT_RUN`이다.
- 검증: lexical 추가 후 contract IPC 55, `lxe_unified_surface` 19, search-plane lib 413, retrieval lib 83, searchctl lib 42 passed. 실제 SDK 통합 `sdk_roundtrip` 16/16 passed(exit 0, 168.55s). raw artifact `/private/tmp/rbr01-lexical-final.B3GlSv/{actual-runner-record,actual-runner-pack,actual-runner-diagnostic}.json`의 SHA-256은 순서대로 `e5ee86153502970b82984858e2d204ff8d9ce1fc9f835175e131e4969c08167c`, `de8c9f9466c835e0f1a2070aeff0edf820f5b3a7c9d1930eca19471dcf64a102`, `bd91fb510b831abe2a238d02851f591ad1111497d62b37e6b4add2c0e495515d`. capture/현재 target 바이너리 SHA-256은 searchd `7716b0b0393379cb7e561b668158065e0ab9d707be694febfa845ada5b270449`, runner `15d131b9ea12b312e324356053ac30686d74605ca6ff364eac3267d60edb1f57`. 별도 replay는 v4·3 route를 수용하고 실제 lexical sidecar의 request ID 0/재사용, stage 누락/역순/타 route, 최종 후보 수 위조 6종을 거부했다. 실행 중 HEAD·dirty 및 lexical 소스 whitespace가 움직여 **dirty local proof**이며 clean-source receipt가 아니다. Python 전체는 두 번 273/274·32 subtests였고 유일한 실패는 각각 동시 변경된 canonical receipt schema의 inventory 필드, 이후 `main` tier drift였다. 두 embedded schema를 맞춘 뒤 해당 집중 테스트는 통과했으나 현 bytes의 전체 274건 재실행은 `NOT_RUN`. 계측 on/off overhead도 `NOT_RUN`.
- 최신 보완의 전체 Python 275 passed/32 subtests(exit 0)는 실행 중 HEAD 및 test 파일이 이동해 source-stable full proof가 아니다. 현 bytes에서 diagnostic 포함 관련 6 tests와 required inventory 275/275 exact, 보존된 실제 sidecar 3-route replay·추가 엔진/전략 변조 9건 거부를 확인했다. 새 live daemon capture와 clean-source SDK receipt는 여전히 `NOT_RUN`이다.
- 잔여: RBR-10 ingest report 공개 연결, 서버 계측 on/off overhead 측정, 현 dirty live/local 결과를 고정 clean revision의 정식 SDK receipt로 재발급한다. 동시 writer의 `ProcessRequestEventsV1` control variant가 미완성이던 중 SDK compile이 한 번 실패했으나 이후 contract+SDK all-targets check와 `sdk_binding_owner_v1` 13/13 재실행은 통과했다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P0. 세 query route의 stage 공개는 코드 반영, ingest·overhead 및 fresh live/clean-source proof는 잔여. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-00 계약.
- 성격: 확정 관측성 공백. [TEST-PLAN](TEST-PLAN.md) 적용.

## 파일·함수

- [sdk.rs](../../../../benchmarks/retrieval/src/sdk.rs): `QueryOutcome`, `query_route`, `PinGuard::with_hits`.
- [diagnostics.rs](../../../../benchmarks/retrieval/src/diagnostics.rs): `diagnostic_value`.
- [main.rs](../../../../benchmarks/retrieval/src/main.rs): capture/phase metrics와 diagnostic 호출.
- [run.py](../../../../tools/benchmark/retrieval/run.py): `validate_retrieval_diagnostic`, frozen sidecar/replay.
- [semantic_query.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs): `build_hybrid_response_explanation`.
- 단계 trace가 필요할 때만 `query_dispatcher/routes/{hybrid,semantic}.rs`, `semantic_derive.rs`, semantic `build.rs`를 확장한다.

## 작업

1. 기존 응답의 request/generation, explanation, window completeness, early stop, executed engines를 버리지 않고 보존한다. contributions는 실행 여부와 별개다.
2. dense lane의 실제 exact/approximate identity, internal fetch, admitted/examined/fused counts를 기존 응답에서 먼저 가져온다. free-text trace는 raw로 보존하며 임의 파싱 결과를 독립 authority로 만들지 않는다.
3. 최종 top-k 진단과 pre-fusion membership을 구분한다. 더 큰 반환 창으로도 원인 분리가 안 될 때만 opt-in bounded stage trace를 추가한다. request/generation/config 바인딩과 상한·truncation 표시가 필수다.
4. query 단계별 elapsed/call counts와 publish 내부 embedding/delete/append/seal/activate timings를 실제 서버 경계에서 수집한다. runner 전체 wall time에서 추정 분배하지 않는다. timing은 식별자로 연결하고 diagnostic on/off overhead를 측정한다.
5. 현재 dirty의 record-normalized span/rank 및 task-route/status 정합성 검증을 보존한다. 같은 수정을 중복 구현하지 않는다.

## 테스트·완료

- lexical을 실행했지만 결과 0인 hybrid fixture에서 executed와 contributed가 다르게 기록된다.
- error/timeout/partial/empty-exhausted/empty-capped를 구분하고 unknown count는 missing으로 남긴다.
- request ID 혼합, stale pin, 잘못된 record/pack hash, duplicate task-route, 순서 변경, 잘린 trace를 거부/명시한다.
- SDK 응답→record→sidecar를 실제 daemon roundtrip으로 대조한다. 기존 unanchored span 회귀 테스트도 실행한다.
- 새 baseline에서 적어도 입력/검색/필터/반환 단계의 **관측 가능 범위**가 명확하다. 모든 내부 후보가 보인다고 과장하지 않는다.

## 제외

rank/fetch/model 기본값 변경은 이 티켓에 넣지 않는다. public DTO 확장은 기존 정보와 bounded trace가 부족하다는 근거가 있을 때만 수행하고 별도 API/fuzz 검증을 추가한다.
