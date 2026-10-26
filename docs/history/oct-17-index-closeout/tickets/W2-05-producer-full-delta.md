# W2-05 — 실제 FULL/delta producer와 도메인·target 인계

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


상태는 [SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-05)만 갱신한다. 이전 **W2-05/06/12/14의 생산 연결 잔여**를 합쳤다. 하나의 producer가 lexical/semantic/graph 결과와 BOOT target을 제공한다.

## Remaining scope and order

1. 기존 FULL helper를 실제 SDK product entrypoint와 authoritative operation/target issuer에 연결한다. 선언/생산 check와 실제 callable 진입을 구분한다.
2. old/new SourceCut와 affected owner/support/output closure를 완성한 뒤 같은 producer에서 edit/delete/rename/recreate/no-op·SourceResync와 기존 V9/ACK를 결합한다. initial-only guard를 부분 delta로 우회하지 않는다.
3. source/policy/recipe-bound certified semantic/chunk·graph 출력을 기존 Index 수신 경로에 인계한다. semantic vector payload copy→published factory의 실제 backing을 Native owner가 유지한다. opaque alias만으로 생산 allocation의 소유권을 닫지 않는다.
4. 같은 FULL 결과의 operation/target을 W2-18에 인계한다. INT는 기존 Required/AA/V9를 결합하고 SDK의 analysis/PTA/workspace producer를 별도로 만들지 않는다.

## Reuse boundary

FULL helper/Original/V9 replay, Native delta staging/retraction, certified chunk/reuse 엔진, RepoMap canonical projection·support children·rank scanner/Kernel/Runtime/SDK는 이미 있다. **새 chunker·ranker·graph IR·producer는 범위가 아니다.** 기존 RepoMap 선정51/51 및 별도 UDS6/6은 수신 범위의 이력이며 실제 Semantica producer 수용을 대신하지 않는다.

Native SDK connect의 Index DATA/opaque Shared 계약과 Semantica의 `native_connect_v3`, `native_index_sdk_connect_v3`·query connector receiver도 저장된 구현을 소비한다. 예전 ADR의 receiver 미연결 문구로 새 connector/allocator/reference counter를 만들지 않는다. 실제 production admission/alias 최종 drop의 수용은 W2-19에서 확인한다.

## Exclusive sources

- SDK [indexing_api.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/indexing_api.rs): `ensure_indexed_with_native_source_socket_v1`와 `publish_native_initial_full_for_committed_repo_v1`의 실제 호출/target 전달. 조사한 crates의 FULL helper 호출 검색은 선언만 발견했으며 외부 consumer 전체 부재를 증명한 것은 아니다.
- [prepared_aggregate_owner_pipeline_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_owner_pipeline_v1.rs): initial SourceAdvance/complete old-new join guard, certified handoff.
- [aggregate_prepare.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/aggregate_prepare.rs): initial-only target admission과 deferred guard.
- [native_source_publish_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_source_publish_v1.rs): 기존 delta staging 소비.

동일 SDK/pipeline public hunk는 한 writer다. INT pending(W2-02)·ANN private(W2-13)와 독립 진행하고 공용 계약 변경만 직렬 합류한다.

## Acceptance

- 실제 SDK→Original producer→daemon FULL 검색과 independent cold-vs-delta gold를 사용한다. edit/delete/rename/recreate/no-op·negative→exists·config-only·all-delete/zero-output/unsupported를 포함한다.
- old/new closure 누락/추가, 역순 N/N+1·duplicate·late ABA·same-ID replacement·model/recipe 변경에서 stale publish를 거절한다. 변경되지 않은 provider 재호출과 invalid 입력 before-hook 거절을 관측한다.
- 독립 expected semantic partition/omission oracle를 실제 producer dispatch 전에 적용한다. 비어 있는 semantic replacement가 합법적인 unchanged인지 누락인지 구분한다. R3의 fixed G1/G2/G3/parent-member·omission-control test oracle는 구현 이력이 있으므로 재작성하지 않고 해당 actual Runtime/paired 실행과 projection-store cold restart를 수용한다.
- initial FULL은 genuine deferred NotComputed/no-head·원 event ancestry와 실제 Native SourceCut/빈 target 조건을 사용한다. 이미 활성 Legacy head를 지우거나 Native FULL로 재라벨하지 않는다. Acked exact replay·이미 bound Dispatching resume·이전 phase pending을 구분하며 저장된 continuation을 재구현하지 않는다.
- lexical/semantic/graph의 selected membership·input/policy와 고정 hits/정렬·drop/degraded, old view/pin·재오픈을 검사한다. malformed child의 exact Unknown/Pending·게시 view 보존 계약을 유지한다.
- Preparing/Prepared/Ready/Committed/Dispatching/Terminal·ACK-loss/restart에서 원 identity/target/custody·원 typed cause를 보존한다. Required7·부분 성공 후 실패를 false Complete로 바꾸지 않는다.
- 실제 producer 증거·원 target 인계는 한 번 생성하고 W2-13/18/19가 소비한다. 해당 독립 oracle를 각각 복제하지 않는다.

## Implementation packet

- **착수:** SDK 담당이 실제 indexing entrypoint→FULL helper→원 target issuer→Original/V9/Submit 호출을 추적한다. 외부 caller 미확인을 helper 부재로 바꾸지 않는다.
- **1차 산출:** 기존 helper를 실제 FULL 경로에 연결하고 원 operation/target·source/policy·desired domain 결과의 인계 tuple을 동결한다. 이 tuple을 SEARCH·BOOT가 공통 소비한다. 공개 SDK parent는 이 담당 한 명만 수정한다.
- **2차 산출:** FULL target/old ACK 계약 확정 뒤 같은 담당이 old/new affected owner/support/output closure와 delta/delete/rename/recreate/no-op/Resync를 결합한다. FULL과 delta를 서로 다른 worker가 같은 pipeline에 동시에 쓰지 않는다.
- **도메인 합류:** semantic certified payload/backing과 RepoMap graph 출력을 기존 수신 경로로 인계한다. INT의 pending/Required, SEARCH ANN, BOOT selection은 별도 writer의 exact hunk로 연결한다.
- **정적 완료/반환물:** 실제 caller 경로·인계 tuple·old/new closure 근거·R3 재사용 oracle·불확실 결과/ACK loss의 상태 분류·영향 fixture 목록. 제품 실행은 통합 큐에 모은다.

## Identity and reconciliation contract

[AWS idempotent API 설계](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/)의 요청 identity·intent 구분을 참고하되 실제 replay/resume 허용 범위는 기존 V9/SDK 계약이 결정한다. same identity/different payload는 거절하고 ACK 유실을 새 event/operation 생성으로 해결하지 않는다. SDK producer가 identity·affected closure·chunk/source 의미를 한 번 생산하고 ANN/BOOT는 그 인증 결과를 소비한다.

W2-02는 최초 persist/claim/discovery backing을 소유하고 이 티켓은 SDK 실제 호출/target·변경 closure를 소유한다. 같은 coordinator/Contract 함수 변경은 INT 통합자가 단일 결합한다. W2-09의 harness submit1/status 관측 정책은 이 production API의 재전송/resume 계약을 임의로 제한하거나 대체하지 않는다. 측정 caller의 정책과 제품 recovery의 의미를 구분한다.
