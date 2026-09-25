# RBR-01 — SDK 응답과 단계별 실행 정보 보존

- 우선순위: P0. 구현/검증: `NOT_RUN`. 선행: RBR-00 계약.
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
