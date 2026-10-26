# W2-05 — 실제 FULL producer와 V9 초기·중간 상태 복구 완성

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-05)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R37,R38 / NI-02. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

`HELPER_IMPL`: committed RepoState용 public `publish_native_initial_full_for_committed_repo_v1`과 internal FULL/V9/ACK 본문을 재사용한다. 조사한 crates에서는 public 메서드 선언 외 호출이 없으므로 실제 product 진입점 연결 완료로 표시하지 않는다. public API를 새로 만들지 않는다.

## Scope

committed RepoState/Original source의 initial FULL부터 V9 phase recovery·terminal ACK까지 실제 제품 경로의 원 authority를 유지한다.

Preparing/Prepared/Ready/Committed/unbound Dispatching/Terminal 각각의 continuation을 기존 V9에서 완성한다. CleanupFencing/CleanupPending/Cleaned와 RecoveryRequired의 retirement/replacement 자격을 구분한다. 불확실한 원 요청을 reconcile 전에 폐기하거나 source 재수집·deadline/budget reset으로 성공을 만들지 않는다.

## Acceptance

- 고정 phase table로 persist/stage/submit/terminal/ACK 전후 crash·retry·ACK loss와 no-recollection을 검사한다.
- zero-output/unsupported/부분 Required와 성공 prefix 후 실패에서 false Complete를 금지한다.
- 실제 SDK product entrypoint→Original producer→daemon FULL→검색을 독립 lexical/semantic/owner gold로 검증한다. P2I01,02,04,05,06을 포함한다.

## Source entry points

- [native_initial_full_replay_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/sdk/search_builder/source_bound/native_initial_full_replay_v1.rs)
- [indexing_api.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/indexing_api.rs)
- [prepared_aggregate_owner_pipeline_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_owner_pipeline_v1.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
