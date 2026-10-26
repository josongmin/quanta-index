# W2-06 — 실제 변경 이벤트→delta/delete/rename/Resync 연결

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-06)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R37,R38 / NI-02. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: SourceAdvance/Resync IR·mutation staging·domain retraction ports. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

완전한 old/new SourceCut와 affected closure에 기반해 실제 변경 이벤트를 동일 native delta/retraction/V9 protocol로 전달한다.

old ACK/SourceCut와 old+new affected owner/support/output closure를 완성한 뒤 기존 FULL-only guard와 Resync/terminal receipt를 함께 변경한다. 부분 delta를 full Resync로 가장하지 않는다. 도메인 기능은 W2-11/12/14를 소비한다.

## Acceptance

- independent cold-vs-incremental gold로 edit/delete/rename/recreate/no-op, negative→exists 및 config-only change를 검사한다.
- old/new closure 누락·추가, 역순 N/N+1·duplicate event·late ABA·ACK loss/restart에서 stale publish를 거절한다.
- all-delete/zero-output/unsupported/required side-output 및 Resync full baseline을 P2I01–08 실제 producer/consumer에서 검사한다.

## Source entry points

- [prepared_aggregate_owner_pipeline_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_owner_pipeline_v1.rs)
- [native_source_publish_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_source_publish_v1.rs)
- [aggregate_prepare.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/aggregate_prepare.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
