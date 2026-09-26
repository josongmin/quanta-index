# RBR-01 — SDK 응답과 단계별 실행 정보 보존

## 현행 판정 — 2026-09-26, `f16bad93` 기반 dirty overlay

- 구현: `SearchExplanation.stage_timings`의 typed server-monotonic elapsed/calls/returned-candidate DTO를 semantic/hybrid 실제 prepare/read-view/lexical/embed/dense/admission/fusion/project 경계에 추가했다. SDK가 해당 응답을 보존하고 diagnostic v4가 route별 필수·순서·중복·count·request ID를 검증한다. dense fetch는 refill의 중복 출력 누계이고 dense admission에 중첩되므로 합산 총시간이 아니다. lexical-only 응답과 ingest publish stage는 아직 내부 계측 공개 경로가 없다.
- 검증: contract `lxe_unified_surface` 19, search-plane lib 413, retrieval lib 82 passed; workspace all-targets compile exit 0(공유 dirty local diagnostic). 실제 SDK 통합 `sdk_roundtrip` 16/16 passed(exit 0, 136.99s). raw record capture가 검증한 바이너리 SHA-256은 searchd `e7f5ec6ba6cad409d2fdce18e72c6166a03548724e2b5f63b914ab0f4c312b20`, runner `045682aa5c47637ea52b0d6ccc0945f827c8744d1387a453db97cb9d7f7366ac`; 이후 공유 target의 searchd 바이너리가 다시 바뀌었으므로 현재 target bytes와 혼동하지 않는다. raw artifact: `/private/tmp/rbr01-stage-v4-final.edu3NK/{actual-runner-record,actual-runner-pack,actual-runner-diagnostic}.json`, SHA-256은 순서대로 `bfa213c163e97863acafd66dee2a4513f9e054c6d61757e776915408b98aebae`, `f30afc079e12b2851c411fb6a41b6c6ec3f7f75d7cc03c34af83e2227b412618`, `4c2793cc89ede4756cf4398fd3d8d6727b2d337257f228663fb90f4b38a98db5`. 별도 Python replay가 v4·3 route를 수용하고 실측 sidecar의 request ID 0, stage 누락/역순, 최종 count 위조, 외부 stage 이름의 5가지 변조를 거부했다. 실행 시 `main`은 `33924335cad664bc23f262a9ed1bbf8f791d7edc` + 공유 dirty overlay였으므로 clean-source receipt가 아니다. 진단 on/off overhead는 `NOT_RUN`.
- 잔여: lexical-only search stage 공개, RBR-10 ingest report 연결, 서버 계측 on/off overhead 측정, 그리고 현 dirty live/local 결과를 고정 clean revision의 정식 SDK receipt로 재발급한다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P0. semantic/hybrid 내부 stage timing은 구현·local live 관측, lexical-only/ingest·overhead 및 clean-source proof는 잔여. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-00 계약.
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
