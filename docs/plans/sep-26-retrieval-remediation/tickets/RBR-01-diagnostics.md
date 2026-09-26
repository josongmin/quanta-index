# RBR-01 — SDK 응답과 단계별 실행 정보 보존

## 현행 판정 — 2026-09-26, [중앙 코드 감사](CURRENT-AUDIT.md)의 공유 dirty source

현재 runner는 diagnostic6/protocol4다. query stage selector와 transient V2 ingest binding에 canonical hybrid floor identity·actual initial-fetch trace 검증이 추가됐다. 최신 installed local SDK18/18 exact collection·terminal은 observation-off 동등성과 floor25/50/100×k1/10/100 initial-probe를 포함해 통과했다. `/private/tmp/qi-rbr-sdk-symbol-final.TEINH2/actual-terminals.json`, SHA `713a3cfd070b573ce27af094393712ada341a396285062e93508f7f4e46f6fdd`. 두 설치 binary SHA 전후 동일·source drift local이지 clean-source proof가 아니다. actual diagnostic6/floor50/record5의3route replay와10변조 거부는 `live-replay-current.log`, SHA `3bd82d3955e111796bbeab766be73742bb5f16d407c60acde70b851018f3930e`. 실제 quiet-host overhead/clean-source qualification은 NOT_RUN이다. 아래425/config38 및 이전 diagnostic4/5 기록은 해당 snapshot의 소유 local 증거이며 최신 전체 qualification이 아니다.

- 최신 코드 보완: 세 query route에 startup-bound `QueryStageObservationPolicy::{Enabled,Disabled}`를 구현했다. 기본값은 Enabled이고 `SearchdConfig`의 `QUANTA_INDEX_QUERY_STAGE_OBSERVATION=enabled|disabled`만 canonical selector로 허용한다. Disabled는 per-stage `Instant`를 읽지 않고 stage vector를 할당하지 않으며 `stage_timings=None`을 반환한다. 기존 operational metric/deadline clocks는 계속 작동하므로 **stage-only overhead** 측정이다. runner sidecar 출력 전환과 별개다.
- lexical pagination의 observation-authority 누수도 제거했다. CBOR page budget은 실제 timing slot 길이를 제외하고 on/off 동일 512-byte reserve를 더한다. worst-case 네 stage shape가 reserve보다 작은지 검증한다. 첫 1024-byte 시안은 기존 2900-byte first-row/cursor guard를 실제 실패시켰으므로 해당 guard를 지우거나 완화하지 않고 reserve를 줄여 복원했다. 이 변경은 byte-cap 근처 page를 이전보다 보수적으로 자를 수 있으므로 enabled의 예전 모든 경계 row count까지 동일하다고 주장하지 않는다. ranking/window/cursor/cancellation의 on/off 동등성은 유지해야 한다.
- 잔여 통합/측정: daemon selector와 config/binary identity를 capture/replay에 바인딩하고 stage None/present 및 selector 위조/불일치 거부를 검증한다. 동일 query/corpus에서 서버 stage 계측 비용과 sidecar 직렬화 비용을 **별도** 측정한다. 실제 overhead·fresh SDK/clean-source receipt는 `NOT_RUN`; ingest transient observation은 RBR-10 소유다. 현재 source/lane 검증 receipt는 아래 최신 보완 기록으로 구분한다.
- 측정 경계: client wall-time on/off는 stage clock/storage뿐 아니라 Some-vs-None stage DTO의 IPC 직렬화·전송 차이도 포함한다. 이를 clock-only 또는 전체 observability 비용이라고 부르지 않는다. 기존 operational metric/deadline 비용은 양쪽 공통이고 runner sidecar 직렬화는 별도 축이다.
- 결과 동등성 fixture는 충분한/unbounded budget과 고정 cursor clock을 사용한다. Deadline/cancellation 검사는 off에서도 그대로 유지하지만 계측 비용이 매우 촉박한 absolute deadline을 넘기는 경우까지 성공/timeout 동등하다고 주장하지 않는다. 실제 비교에서 timeout/partial이 발생하면 정상 pair로 정규화하거나 누락하지 말고 판정을 거부한다.
- 최신 local 검증: `./scripts/cargow test -p quanta-index-searchd --lib --locked app::config::tests:: -- --nocapture`에서 **38 passed, 54 filtered**, exit 0. canonical selector/invalid values와 양 deployment entry-point의 env family wiring fence를 포함한다. raw `/private/tmp/qi-query-observation.3RQXR5/daemon-config-tests.log`; 당시 9개 query/config source digest는 before/after 동일했다. `./scripts/cargow test -p quanta-index-search-plane --lib --locked`의 512-byte reserve 재실행은 **425 passed, 0 failed**, exit 0 (`search-plane-lib-reserve512.log`). 9개 신설 observation tests와 기존 2900-byte first-row guard·모든 lexical pagination 회귀를 포함한다. cursor A/B는 test-only 고정 clock으로 issuance/expiry input도 동일하게 바인딩한다. final-source cached 재실행/Clippy는 별도 closeout log로 갱신한다. 이 결과를 SDK/live/clean qualification으로 확대하지 않는다.
- final collector 재검증: `StageTimings::record_elapsed`로 정리한 최종 bytes에서 같은 full-lib 명령 **425 passed, 0 failed**, exit 0, tests 5.30s (`search-plane-lib-collector.log`). 10개 owned query/config source digest before/after가 동일하다(`query-source-collector-{before,after}.sha256`), binary digest는 `search-plane-test-binary-final.sha256`. final all-target Clippy는 별도 결과로 갱신하며 source-stable owner-local proof를 clean-source/live qualification으로 승격하지 않는다.
- 최종 query all-target Clippy: `./scripts/cargow clippy -p quanta-index-search-plane --all-targets --locked -- -D warnings` exit 0 (`search-plane-clippy-final.log`); source digest는 final after까지 동일하다. 반면 searchd를 포함한 확대 명령은 별도 committed/shared daemon의 state_migration collapsible-if 2건, supervisor indexing 2건, request_events test indexing 3건으로 `FAILED`였다(`query-clippy-metadata-stable.log`). 이 7건은 query 정책 변경의 산물이 아니며 중앙 통합 소유자가 처리한다. query 성공을 daemon/all-workspace lint 성공으로 확대하지 않는다.
- 아래 실행 수치·artifact는 각 과거 dirty snapshot의 local 기록이다. 최신 inventory/검증은 [CURRENT-AUDIT](CURRENT-AUDIT.md)를 따르며 현재 live SDK/clean receipt와 실제 on/off overhead는 `NOT_RUN`이다.

### 이전 diagnostic4 snapshot 기록

- 당시 구현: `SearchExplanation.stage_timings`의 typed server-monotonic elapsed/calls/returned-candidate DTO를 lexical/semantic/hybrid의 실제 prepare/read-view/search/embed/admission/fusion/project 경계에 추가했다. lexical `TextQueryResponse`도 공통 explanation을 실어 SDK→diagnostic v4로 전달했다. diagnostic replay는 route별 필수·순서·중복·count·request ID를 검증했다. dense fetch는 refill의 중복 출력 누계이고 dense admission에 중첩되므로 합산 총시간이 아니다. 당시 ingest 공개 경로 미구현 판정은 현재 RBR-10의 transient V2 구현으로 해소됐다.
- lexical `project` 시간은 response-budget fit 이전에 멈추고 후보 수만 최종 fitted page에 맞춘다. 따라서 이 stage 시간은 직렬화·cursor budget 비용을 포함한다고 해석하지 않는다. force-empty plan은 backend 호출 없이 prepare/project만 기록한다.
- 후속 validator 보강(공유 dirty local): diagnostic v4에서 route stage의 실제 호출·후보 수를 `engines_executed`, `engines_touched`, `strategy`와 대조한다. lexical의 budget-fit 후 기여, semantic의 optional lexical scope/zero-hit, hybrid의 lexical/dense admission 조합을 구분한다. 누락·위조된 engine/strategy가 구조적으로 정상인 timing 배열만으로 통과하지 못한다. 합성 fixture와 보존된 실제 3-route v4 sidecar replay는 통과했고, 그 실제 sidecar의 3 route × 3 모순 필드 변조 9건은 모두 거부했다. 새 daemon capture·clean-source receipt는 `NOT_RUN`이다.
- 검증: lexical 추가 후 contract IPC 55, `lxe_unified_surface` 19, search-plane lib 413, retrieval lib 83, searchctl lib 42 passed. 실제 SDK 통합 `sdk_roundtrip` 16/16 passed(exit 0, 168.55s). raw artifact `/private/tmp/rbr01-lexical-final.B3GlSv/{actual-runner-record,actual-runner-pack,actual-runner-diagnostic}.json`의 SHA-256은 순서대로 `e5ee86153502970b82984858e2d204ff8d9ce1fc9f835175e131e4969c08167c`, `de8c9f9466c835e0f1a2070aeff0edf820f5b3a7c9d1930eca19471dcf64a102`, `bd91fb510b831abe2a238d02851f591ad1111497d62b37e6b4add2c0e495515d`. capture/현재 target 바이너리 SHA-256은 searchd `7716b0b0393379cb7e561b668158065e0ab9d707be694febfa845ada5b270449`, runner `15d131b9ea12b312e324356053ac30686d74605ca6ff364eac3267d60edb1f57`. 별도 replay는 v4·3 route를 수용하고 실제 lexical sidecar의 request ID 0/재사용, stage 누락/역순/타 route, 최종 후보 수 위조 6종을 거부했다. 실행 중 HEAD·dirty 및 lexical 소스 whitespace가 움직여 **dirty local proof**이며 clean-source receipt가 아니다. Python 전체는 두 번 273/274·32 subtests였고 유일한 실패는 각각 동시 변경된 canonical receipt schema의 inventory 필드, 이후 `main` tier drift였다. 두 embedded schema를 맞춘 뒤 해당 집중 테스트는 통과했으나 현 bytes의 전체 274건 재실행은 `NOT_RUN`. 계측 on/off overhead도 `NOT_RUN`.
- 최신 보완의 전체 Python 275 passed/32 subtests(exit 0)는 실행 중 HEAD 및 test 파일이 이동해 source-stable full proof가 아니다. 현 bytes에서 diagnostic 포함 관련 6 tests와 required inventory 275/275 exact, 보존된 실제 sidecar 3-route replay·추가 엔진/전략 변조 9건 거부를 확인했다. 새 live daemon capture와 clean-source SDK receipt는 여전히 `NOT_RUN`이다.
- 잔여: RBR-10 ingest report 공개 연결, 서버 계측 on/off overhead 측정, 현 dirty live/local 결과를 고정 clean revision의 정식 SDK receipt로 재발급한다. 동시 writer의 `ProcessRequestEventsV1` control variant가 미완성이던 중 SDK compile이 한 번 실패했으나 이후 contract+SDK all-targets check와 `sdk_binding_owner_v1` 13/13 재실행은 통과했다. [현재 전수 판정](CURRENT-AUDIT.md).

- 우선순위: P1. 세 query route의 stage 공개와 실제 startup stage on/off rail은 코드 반영; config-bound benchmark/replay, overhead 및 fresh live/clean-source proof는 잔여. ingest 공개 경로는 RBR-10의 최신 판정을 따르며 아래 historical 기록을 현행 공백으로 재사용하지 않는다. [현재 전수 판정](CURRENT-AUDIT.md). 선행: RBR-00 계약.
- 성격: 확정 관측성 공백. [TEST-PLAN](TEST-PLAN.md) 적용.

## 파일·함수

- [sdk.rs](../../../../benchmarks/retrieval/src/sdk.rs): `QueryOutcome`, `query_route`, `PinGuard::with_hits`.
- [diagnostics.rs](../../../../benchmarks/retrieval/src/diagnostics.rs): `diagnostic_value`.
- [main.rs](../../../../benchmarks/retrieval/src/main.rs): capture/phase metrics와 diagnostic 호출.
- [run.py](../../../../tools/benchmark/retrieval/run.py): `validate_retrieval_diagnostic`, frozen sidecar/replay.
- [semantic_query.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/semantic_query.rs): `build_hybrid_response_explanation`.
- [stage_timing.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/stage_timing.rs): canonical policy/`start`, guarded stateful `record_elapsed`, disabled no-storage collector, maximum CBOR reserve guard.
- [dispatcher.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/dispatcher.rs): startup-bound `with_query_stage_observation`/getter; lexical/semantic/hybrid routes consume it without changing execution authority.
- [response_budget.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/response_budget.rs): observation-independent byte accounting and `LEXICAL_STAGE_RESERVE_BYTES=512`.
- [config.rs](../../../../crates/quanta-index-searchd/src/app/config.rs), [runtime.rs](../../../../crates/quanta-index-searchd/src/app/runtime.rs): strict env family, both entry-point fence and dispatcher wiring.
- [stage_observation.rs](../../../../crates/quanta-index-search-plane/src/query_dispatcher/tests/stage_observation.rs): on/off full-outcome/cursor/correlation/cancellation/budget fixtures. `continuation.rs::with_clock_for_tests` is test-only input pinning, not a production clock change.
- Publish trace는 RBR-10 소유의 `semantic_derive.rs`/semantic `build.rs`와 별도 transient DTO/SDK 연결을 따른다.

## 작업

1. 기존 응답의 request/generation, explanation, window completeness, early stop, executed engines를 버리지 않고 보존한다. contributions는 실행 여부와 별개다.
2. dense lane의 실제 exact/approximate identity, internal fetch, admitted/examined/fused counts를 기존 응답에서 먼저 가져온다. free-text trace는 raw로 보존하며 임의 파싱 결과를 독립 authority로 만들지 않는다.
3. 최종 top-k 진단과 pre-fusion membership을 구분한다. 더 큰 반환 창으로도 원인 분리가 안 될 때만 opt-in bounded stage trace를 추가한다. request/generation/config 바인딩과 상한·truncation 표시가 필수다.
4. query 단계별 elapsed/call counts와 구현된 `QueryStageObservationPolicy` rail을 보존한다. publish embedding/delete/append/seal/activate transient observation은 RBR-10과 연결한다. runner 전체 wall time에서 추정 분배하지 않는다. request/generation/config identity를 실제 daemon selector에 바인딩한 capture/replay 후 overhead를 측정한다. sidecar on/off를 서버 계측 on/off로 취급하지 않는다.
5. 현재 dirty의 record-normalized span/rank 및 task-route/status 정합성 검증을 보존한다. 같은 수정을 중복 구현하지 않는다.

## 테스트·완료

- lexical을 실행했지만 결과 0인 hybrid fixture에서 executed와 contributed가 다르게 기록된다.
- error/timeout/partial/empty-exhausted/empty-capped를 구분하고 unknown count는 missing으로 남긴다.
- request ID 혼합, stale pin, 잘못된 record/pack hash, duplicate task-route, 순서 변경, 잘린 trace를 거부/명시한다.
- SDK 응답→record→sidecar를 실제 daemon roundtrip으로 대조한다. 기존 unanchored span 회귀 테스트도 실행한다.
- 새 baseline에서 적어도 입력/검색/필터/반환 단계의 **관측 가능 범위**가 명확하다. 모든 내부 후보가 보인다고 과장하지 않는다.

## 제외

rank/fetch/model 기본값 변경은 이 티켓에 넣지 않는다. public DTO 확장은 기존 정보와 bounded trace가 부족하다는 근거가 있을 때만 수행하고 별도 API/fuzz 검증을 추가한다.
