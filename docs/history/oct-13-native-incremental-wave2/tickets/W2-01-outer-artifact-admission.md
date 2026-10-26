# W2-01 — 최초 aggregate artifact 읽기·디코딩의 Original admission

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-01)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R11,R12,R37 / NI-01. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: same Original group의 path backing·raw IO DATA·FD retirement·outer loader. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

## Scope

기존 V3/V4/V5 aggregate artifact의 최초 read/decode·path/FD·payload backing을 같은 Original authority와 물리수명에 묶는다.

## Acceptance

- 고정 V3/V4/V5 bytes로 truncated/oversize/noncanonical/foreign ref·kind·receipt를 거절한다.
- pre-birth denial에서 artifact backing birth는 0이며 partial read/decode·cancel·panic에서 첫 원인을 보존한다.
- Path/FD/payload는 grant보다 먼저 정리하고 실제 cold reopen/crash에서 동일 receipt와 canonical bytes를 복원한다.

## Source entry points

- [prepared_commit_artifact_v2.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_commit_artifact_v2.rs)
- [prepared_artifact_readback_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_artifact_readback_v3.rs)
- [prepared_aggregate_owner_pipeline_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_owner_pipeline_v1.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
