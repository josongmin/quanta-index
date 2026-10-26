> HISTORICAL — pre-migration document from `docs/plans/oct-17-index-closeout/tickets/W2-05-producer-full-delta.md`. Current work: [Oct-22 SSOT](../../../../plans/oct-22-finalizing/README.md). Old TODO/status/counts do not create work or qualify current source.

# W2-05 — 실제 FULL/delta producer와 도메인·target 인계

상태는 [SSOT](README.md#w2-05)만 갱신한다. 이전 **W2-05/06/12/14의 생산 연결 잔여**를 합쳤다. 하나의 producer가 lexical/semantic/graph 결과와 BOOT target을 제공한다.

<a id="remaining-scope-and-order"></a>
## Product contracts

1. 제품 Rust 진입점은 `SdkRuntimeExecutor::ensure_native_indexed_v1(repo, socket, target)`로 고정한다. 기존 `QuantaClient::ensure_indexed_with_native_source_socket_v1`은 target을 선검증하고 cached/cold source 재검증 뒤 기존 FULL 게시를 호출해 opaque ACK를 이동 반환한다. ordinary EnsureIndexed command·daemon wire·target issuer는 추가/대체하지 않는다. 실제 target 발급 결과와 BOOT 인계는 이 진입점으로 수용한다.
2. 기존 facts-mode `prepare_facts_sdk_checkpoint_v11`의 committed target 승격·Original snapshot·path/semantic delta handoff·checkpoint를 재사용한다. general delta의 old/new SourceCut·affected owner/support/output closure와 edit/delete/rename/recreate/no-op·SourceResync 수용 잔여만 실제 caller별로 좁힌다. initial-only guard를 부분 delta로 우회하거나 facts producer를 별도로 만들지 않는다.
3. source/policy/recipe-bound certified semantic/chunk·graph 출력을 기존 Index 수신 경로에 인계한다. semantic vector payload copy→published factory의 실제 backing을 Native owner가 유지한다. opaque alias만으로 생산 allocation의 소유권을 닫지 않는다.
4. 같은 FULL 결과의 operation/target을 W2-18에 인계한다. INT는 기존 Required/AA/V9를 결합하고 SDK의 analysis/PTA/workspace producer를 별도로 만들지 않는다.

## Reuse boundary

FULL helper/Original/V9 replay, Native delta staging/retraction, certified chunk/reuse 엔진, RepoMap canonical projection·support children·rank scanner/Kernel/Runtime/SDK는 이미 있다. **새 chunker·ranker·graph IR·producer는 범위가 아니다.** 기존 RepoMap 수신 범위의 source별 이력은 실제 Semantica producer 수용을 대신하지 않는다.

Native SDK connect의 Index DATA/opaque Shared 계약과 Semantica의 `native_connect_v3`, `native_index_sdk_connect_v3`·query connector receiver도 저장된 구현을 소비한다. 예전 ADR의 receiver 미연결 문구로 새 connector/allocator/reference counter를 만들지 않는다. 실제 production admission/alias 최종 drop의 수용은 W2-19에서 확인한다.

## Exclusive sources

- SDK [indexing_api.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/indexing_api.rs): `ensure_indexed_with_native_source_socket_v1`와 `publish_native_initial_full_for_committed_repo_v1`의 실제 호출/target 전달. 실제 product entrypoint 호출과 authoritative target 전달을 확인한다. 선언 존재만으로 consumer 연결 완료/부재를 단정하지 않는다.
- [prepared_aggregate_owner_pipeline_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_owner_pipeline_v1.rs): initial SourceAdvance/complete old-new join guard, certified handoff.
- [aggregate_prepare.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/aggregate_prepare.rs): initial-only target admission과 deferred guard.
- [native_source_publish_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_source_publish_v1.rs): 기존 delta staging 소비.

동일 SDK/pipeline public hunk는 한 writer다. INT pending(W2-02)·ANN private(W2-13)와 독립 진행하고 공용 계약 변경만 직렬 합류한다.

## Source integration boundaries

The current state and exact assignments remain in [SSOT W2-05](README.md#w2-05). The following are reusable contracts and closure criteria, not another status ledger or additional tickets.

| Existing bundle | Reuse; no duplicate implementation | Required caller/consumer closure |
| --- | --- | --- |
| M metadata / initial ingress | Paid move-only binary owner, actual query Source B/genuine Output A, snapshot/group checks, Facts target map and existing Source text provider | Initial first-FD and cold committed FULL use their actual retained Source through the existing ingress packet and CFG/DFG pipeline. A binary miss stays within the same pinned text/deleted/not-indexed authority. Do not restore ordinary map copying or reuse the old FULL execution budget as a query budget. |
| P projection / RepoMap | Checked graph commands, paid document aliases, Native RepoMap bank, non-import edge collector and retained carried-origin export | Actual Native stage/shard metadata, canonical fallible persistent generation and exact member publication must consume all independent grants. Node/edge collection alone is not final emit. Extend the canonical persistent owner; no precharged ordinary clone or second map. |
| W full corpus writer | Existing sole full5 encoder, authenticated inventory/module/Q2 lenders, strict decoder and descriptor paid persist/readback | Consume the existing private writer through the real upper caller with its lenders, Native birth, request/failure custody and terminal ownership. Unmounted private source or an apply-check does not replace the actual initial writer. |
| C guard / carry | Existing opaque full-origin owner, manifest pin and paid retained-export alias | Join current carry/access/pin/caller APIs once with P/W; keep full origin, pin, Source and funding through export, async queue and final drop. A borrowed view is not an independently retained export. |
| PUB source index / final generation | Existing after-Source input-index finisher, prepared input CAS, candidate graph, durable slot and exterior outcome | Source-index completion is a prepare/commit substep. Require genuine Java joined pin/build floor, Original Python normalized Parser inventory/type-world/projector inputs, and fresh Native journal/registry authority before complete-generation visibility. An empty Java graph, provisional Python outcome or ordinary Baseline axes cannot replace these inputs. Never infer product success solely because the old unconditional `UnsupportedOperation` was replaced by a prepare/finish split. |

Common `materialize_v3.rs` and `indexing_api.rs` hunks are integrated serially. Root consumes existing Source/metadata, corpus/projection/carry and writer banks in the Runtime upper caller; PUB owns the public source-index/genesis/outcome and SDK parent contract. A helper or private patch is reused once, not reassigned to another worker as a missing implementation. Source/caller facts can change before handoff artifacts are refreshed; recheck the live symbol before replaying an old patch or TODO.

Initial FULL has no change-event pending plan. Its corpus writer must consume the genuine finalized inventory lender and actual Q2/module owners rather than return an absent binding when pending is absent. Preserve the already-issued Product epoch from terminal output through RepoState to that lender; do not reconstruct inventory or issue another authority for search publication. The same borrowed lender is forwarded through the collector and both ingress views. A retained RuntimeError must cross lane assembly intact, including its failure DATA and backing.

Successor publication must consume the collector's existing NativeCarried owner through the paid carry/successor-binding contract. An ordinary-only carry validator is not a supported Native consumer. Keep the canonical Native allocation gate until stage/generation consumers are paid and connected; removing a blanket refusal alone does not complete this contract.

## Acceptance

- 실제 SDK→Original producer→daemon FULL 검색과 independent cold-vs-delta gold를 사용한다. edit/delete/rename/recreate/no-op·negative→exists·config-only·all-delete/zero-output/unsupported를 포함한다.
- old/new closure 누락/추가, 역순 N/N+1·duplicate·late ABA·same-ID replacement·model/recipe 변경에서 stale publish를 거절한다. 변경되지 않은 provider 재호출과 invalid 입력 before-hook 거절을 관측한다.
- 독립 expected semantic partition/omission oracle를 실제 producer dispatch 전에 적용한다. 비어 있는 semantic replacement가 합법적인 unchanged인지 누락인지 구분한다. R3의 fixed G1/G2/G3/parent-member·omission-control test oracle는 구현 이력이 있으므로 재작성하지 않고 해당 actual Runtime/paired 실행과 projection-store cold restart를 수용한다.
- initial FULL은 genuine deferred NotComputed/no-head·원 event ancestry와 실제 Native SourceCut/빈 target 조건을 사용한다. 이미 활성 Legacy head를 지우거나 Native FULL로 재라벨하지 않는다. Acked exact replay·이미 bound Dispatching resume·이전 phase pending을 구분하며 저장된 continuation을 재구현하지 않는다.
- lexical/semantic/graph의 selected membership·input/policy와 고정 hits/정렬·drop/degraded, old view/pin·재오픈을 검사한다. malformed child의 exact Unknown/Pending·게시 view 보존 계약을 유지한다.
- Preparing/Prepared/Ready/Committed/Dispatching/Terminal·ACK-loss/restart에서 원 identity/target/custody·원 typed cause를 보존한다. Required7·부분 성공 후 실패를 false Complete로 바꾸지 않는다.
- pending 없는 최초 FULL이 실제 inventory loan/encoder를 한 번 호출해 full corpus binding을 만드는지 확인한다. 같은 epoch의 Source/world/Output을 빌리고 terminal→RepoState 보유권을 끊거나 foreign epoch로 바꾸는 음성 조건을 거절한다.
- writer 이후 prepare/abort/V9/reconcile/dispatch/ACK 실패와 reader Arc의 allocation 전·후 거절에서도 whole request·corpus·pin·prepared authority와 typed first/secondary 원인을 보존한다. 실패 뒤 새 예산으로 error backing을 할당하지 않으며 마지막 오류 owner drop에서만 환급한다.
- Native Chunk 양성 수용은 실제 executor의 여러 대상 파일 query→candidate/unit→planner→문서/dispatch를 통과해야 한다. 1→1·adjacent-import N→1·large-body 1→N의 고정 expected ID/span/text/순서와 언어별 정책을 검증한다. 삭제·all-delete/zero-output는 기존 delta/coverage oracle로 합법적 빈 결과를 확인하고 unsupported는 기존 거절/degraded 계약을 따른다. plan 누락·미구현을 빈 성공 FULL로 바꾸지 않는다. planner-only fixture와 executor 부재 음성 경로는 이 양성을 대신하지 않는다.
- 게시 join은 같은 writer serial 아래 원 paid request→기존 supersession reconcile→실제 highest/finisher→같은 request의 manifest 정규화/digest 순서를 검증한다. 고정 old/new shard와 replacement oracle로 superseded shard 제거·생존 순서·최종 manifest/receipt 일치를 확인하고 pending identity/재시작도 대조한다. manifest 선소비 뒤 request 재구성이나 reconciliation 생략으로 연결하지 않는다. 기존 reconciliation/manifest owner 회귀와 bank는 재사용하고 이 join만 추가한다.
- 실제 producer 증거·원 target 인계는 한 번 생성하고 W2-13/18/19가 소비한다. 해당 독립 oracle를 각각 복제하지 않는다.

## Implementation packet

A–E는 이 티켓 안의 고정 실행 계약이며 새 티켓/상태표가 아니다. 상태·담당·다음 조건은 [SSOT W2-05](README.md#w2-05), 실제 소스/실행 근거는 [VALIDATION](../../oct-10-index-closeout/VALIDATION.md#producerpendingastboot통합)만 갱신한다. 변경 직전 actual caller·fresh dirty hunk·claim을 대조하고 이미 충족된 항목은 기존 구현/결과를 소비한다. 새 채팅의 응답이나 다른 campaign 전체 완료를 착수 조건으로 만들지 않는다.

S source root는 `/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates`다. 아래 `quanta-runtime/src` 경로는 이 root 기준이다. 공용 parent/export/SDK hunk는 [WRITE-OWNERSHIP](WRITE-OWNERSHIP.md#collision-resolution)에 따라 한 번 결합한다.

<a id="native-chunk-plan"></a>
### A. Native Chunk 전체 파일 plan

- **수정 경계:** Root의 `consumer_v3/chunk_preplan_v3.rs::run_chunk_preplan_v3`·`first_chunk_dirty_ordinal_v3`와 `consumer_v3/plan_v3.rs::run_planner_phases_v3`. 전체 경로는 `quanta-runtime/src/coordination/change_event_recompute_invalidator/incremental_rail/native_delta_v3` 아래다. 실제 leaf는 기존 `retrieval/port_impls/index_job_executor/parts/executor/native_chunk_v3`에서 결합한다.
- **재사용:** 실제 async preplan query, checked HIR 언어/registry 선언, ItemIndex 선택, paid TextBasis·exterior phase slots, 기존 `hir_native_semantic_projection_provider/recipe_v3.rs`의 merge-before-split-before-single, canonical candidate/unit/plan IR. query/collector/codec/issuer를 새로 만들지 않는다.
- **반환물:** 첫 dirty ordinal 하나에 한정하지 않고 모든 대상 upsert 파일의 candidate/unit·full-file plan을 같은 Original의 paid owner로 생성해 planner의 `None`을 교체한다. 언어/정책 capability·file/identity/span·canonical 순서를 보존한다. 삭제에는 HIR query를 강제하지 않고 기존 dirty/retraction 의미를 사용한다.
- **병렬 접합:** 기존 preplan port는 `projection_ports_v3.rs`→`executor/execution.rs`→`dispatch_port_impl.rs`→`query_v3/preplan_execution_v3.rs`에서 단일 ordinal을 전달한다. 모든 파일 처리의 순서·paid plan/unit owner·실패 전달을 먼저 고정하고 B의 private 문서 생산과 병렬 구현한다. 이 서명을 변경할 경우 공용 trait/dispatch/parent는 통합자만 producer/consumer와 함께 수정한다. 파일별 Source 종료와 partial owner 보유를 생략하거나 별도 batch IR를 추가하지 않는다.
- **수용:** 실제 executor를 사용하는 0/1/여러 파일·다언어·순서 반전·duplicate/foreign identity·합법적 no-op/all-delete/zero-output와 미구현/누락 거절을 검사한다. independent fixed ID/span/text/순서를 사용한다. planner-only fixture나 executor 부재 음성만으로 완료하지 않는다. partial/failure/keeper는 highest 종료와 마지막 owner drop까지 유지한다.

<a id="native-chunk-dispatch"></a>
### B. Native Chunk 문서와 dispatch

- **수정 경계:** Root B의 `index_job_executor/parts/executor/native_chunk_v3/paid_projection_v3.rs`·해당 private 문서 producer. `index_job_executor/parts/dispatch_port_impl.rs::prepare_native_projection_job_v3`·공용 enum/exports·preplan port 인수는 Root 통합자만 A의 계약과 함께 직렬 결합한다.
- **재사용:** A의 exact plan-owned unit, 기존 canonical `chunk_projection` 및 materialization/lexical/semantic 정책, 기존 Native prepared Batch/Empty와 P의 writer acceptance. 기존 direct validator를 완화하거나 Ordinary 결과를 유상 copy해 Native로 승격하지 않는다.
- **반환물:** 실제 Chunk selected job이 paid 문서를 생산하고 같은 scheduler/writer에 전달한다. 1→1뿐 아니라 adjacent-import N→1 및 large-body 1→N을 materialize한다. 정당한 empty/retraction은 기존 authority로만 생성한다.
- **수용:** 세 topology의 고정 문서 수·ID/span/text/순서와 실제 dispatch→writer 수락을 확인한다. missing/duplicate plan unit, foreign Original, body/physical/late Source 거절·cancel/unwind에서 부분 문서/typed 원인/마지막 drop 환급을 검사한다. 단순 Chunk match arm 추가는 완료물이 아니다.

<a id="native-publication-caller"></a>
### C. 실제 내부 publication caller

- **수정 경계:** Root의 `consumer_v3/{entry_handoff_v130,native_manifest_handoff_v165}.rs` 및 이 entry가 실제 호출할 writer facade/upper composition. P는 기존 `index_projection_writer`의 bank body를 유지하며 Root는 필요한 공용 export/인수를 실제 caller와 같은 변경에서 결합한다.
- **재사용:** I Parent pin/reconciliation·manifest/receipt, P generation/FinalDocs/vector/root4phase/CAS, Root witness, M `NativeFullSearchPlaneBirthV164`. 기존 source-applied bank/algorithm/Store를 재작성하지 않는다.
- **순서/반환물:** 모든 writer 수락 완료→same-writer serial→parent pin→`NativeCommitReconciliationV3`의 actual highest/finisher→반환된 원 paid request→`NativeCommitManifestBirthV3` normalize/digest→manifest 순서 stage 선택→settled generation→FinalDocs/witness→generation0 receipt→같은 FinalDocs의 vector 검증→FullSearchPlan→paid root payload→root4phase→기존 CAS→실제 게시 artifact. v165가 request를 미리 소비하는 호출은 이 순서로 옮긴다. 같은 serial을 CAS까지 보유하고 각 Source가 종료된 뒤 다음 loan으로 이동한다.
- **수용:** fixed old/new shard·replacement oracle로 reconcile 후 membership/digest·생존 순서·receipt/generation을 검증한다. actual FullSearchPlan stage·root payload 값 생성·CAS 인계의 production caller가 도달해야 한다. foreign writer/parent/Original·stale CAS·late refusal·abort/uncertainty·pin/drop을 기존 oracle로 검사한다. manifest→request 복원·Clone·새 publisher·receipt/root 순환을 허용하지 않는다.
- **인계:** 성공 artifact와 실패 whole-owner를 [W2-02 terminal join](W2-02-pending-recovery.md#terminal-publication-join)에 한 번 이동한다. stage receipt·prepared manifest만으로 ACK를 만들지 않는다.

<a id="wire-canonical-custody"></a>
### D. Wire canonical raw/header custody

- **수정 경계:** Root D의 `quanta-runtime-source-authority-wire/src/original_source_control_v3/source_memory_lease_v3/original_canonical_admission_v3/native_memory_v3.rs`·sibling `original_canonical_admission_v3.rs`와 그 기존 owner tests. 실제 caller의 exterior DATA/finisher 인수는 통합자가 원 owner와 직렬 결합한다. private bank만 수정하고 caller가 Source 안에서 bank를 해제하면 완료가 아니다.
- **재사용:** 기존 Original ledger, Native allocation/header/shared-value 엔진과 같은 group. Source·Args·snapshot의 별도 일반 owner 완료를 이 경계의 수용으로 계산하지 않는다.
- **반환물:** `admit_native_try_reserve_v11`의 원 `TryReserveError`를 bool default adapter에서 지우지 않고 Source 밖 DATA에 보존한다. `take_native_custody_v3`의 payload/funding/pending/physical/complete header partial도 실제 highest Source가 끝날 때까지 외부 owner에 남긴다.
- **실제 consumer 접합:** `quanta-runtime/src/retrieval/port_impls/index_projection_writer/original_native_memory_scope_v1/admitted_canonical_output_v3.rs`의 `CanonicalMemberEncodeDataV3`와 `canonical_v3.memory_v3(group)`가 한 실제 Source/finisher seam이다. `indexing_machine_v2/store/pending_transaction/native_upsert_v3.rs`·`native_restore_v3.rs` 등 도달 caller도 대조하여 변경 계약의 실제 영향만 반영한다. 기존 Output exterior bank를 구조적 선례로 쓰되 별도 allocator를 복제하지 않는다. Core trait 변경은 기본 요구가 아니며 필요한 경우에만 원 owner가 같은 호출 계약으로 결합한다.
- **수용:** admission refusal의 physical callback0, raw reserve failure identity, header pre/post-birth·outer Source 취소/unwind, 중복 진입·성공 후 alias 보유를 검사한다. payload/header가 실제 해제되기 전 charge를 환급하지 않고 마지막 backing drop에서 환급한다. 별도 allocator/shared-owner 엔진·issuer·예산 확대는 만들지 않는다.

<a id="sdk-producer-custody"></a>
### E. PUB 공개 producer와 실패 입력 인계

- **수정 경계:** PUB의 `quanta-sdk-runtime-composition-owner/src/core/target_generation_clone_v72.rs::clone_for_target_generation_v1` 및 기존 target-generation callers, `quanta-sdk-runtime-executor/src/client/repo_state/authority/facts_public_source_preparation_v11.rs`·recompute child, 기존 `quanta-sdk-session-backend-owner/src/repository_indexing_v1/original_source_publication_v3/materialize_v3.rs`와 `quanta-sdk-runtime-executor/src/client/indexing_api/original_initial_publication_birth_v3.rs`/SDK parent. Root는 이 body를 중복 수정하지 않는다.
- **재사용/인계:** 기존 orchestrator·workspace/file-store owner·canonical Source/QRA/receipt와 공개 FULL helper를 소비한다. 기존 target clone failure owner와 prepare/after-Source commit은 재구현하지 않는다. 수정 직전 PUB의 recompute/Facts/공개 SDK fresh claim과 dirty hunk를 확인해 그 소유 경계를 보존한다. 내부 Runtime publication bank와 별도의 publisher를 만들지 않는다.
- **반환물:** target clone/Facts/recompute의 실패에서도 원 입력·최초/losing typed 원인·keeper를 보유하고 성공 때만 이동한다. Python/TSJS 등 선언된 공개 경로는 실제 Source·generation·receipt의 언어별 canonical 결과를 소비하며 Java 전용 조립을 성공으로 위장하지 않는다. 미지원 언어는 기존 명시 거절/degraded 계약을 따른다.
- **수용:** target validation/clone·bootstrap/generation·후반 recompute 실패와 성공 move-once·cancel/unwind/drop을 owning fixture로 검사한다. 정상 공개 caller→실제 artifact와 cached/cold exact retry를 확인한다. bare DTO·Ordinary decode·무권한 Clone·문자열만 남기는 오류 변환으로 원 custody를 대체하지 않는다. SDK terminal은 W2-02, 실제 결과의 AST/BOOT 소비는 W2-16/18에서 한 번 수용한다.

### Join and checkpoint

A/B private 구현은 [파일별 병렬 배치](WRITE-OWNERSHIP.md#parallel-implementation-schedule)로 진행하고 Root가 같은 plan/port/dispatch를 결합한 뒤 C→W2-02를 잇는다. D private bank와 PUB E는 독립 source hunk이며 실제 caller DATA·SDK parent 인수만 원 owner와 직렬 합류한다. P의 기존 Runtime owning 실행과 SDK/CP의 기존 PUB/PY 큐를 소비하며 같은 Runtime compile/capture를 Root가 복제하지 않는다. 공통 merge 기준은 actual caller·같은 Original/target·Source finisher·typed failure·backing-last다. 원 source/오류가 바뀌지 않은 동일 RED를 다른 filter/feature로 재실행하지 않는다. 코드가 연결되면 [W2-19 체크포인트](W2-19-integration-acceptance.md#combined-checkpoints)에서 영향 검증→실제 제품→운영 A→switch/B·CI/release를 각각 판정한다.

## Identity and reconciliation contract

[AWS idempotent API 설계](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/)의 요청 identity·intent 구분을 참고하되 실제 replay/resume 허용 범위는 기존 V9/SDK 계약이 결정한다. same identity/different payload는 거절하고 ACK 유실을 새 event/operation 생성으로 해결하지 않는다. SDK producer가 identity·affected closure·chunk/source 의미를 한 번 생산하고 ANN/BOOT는 그 인증 결과를 소비한다.

W2-02는 최초 persist/claim/discovery backing을 소유하고 이 티켓은 SDK 실제 호출/target·변경 closure를 소유한다. 같은 coordinator/Contract 함수 변경은 INT 통합자가 단일 결합한다. W2-09의 harness submit1/status 관측 정책은 이 production API의 재전송/resume 계약을 임의로 제한하거나 대체하지 않는다. 측정 caller의 정책과 제품 recovery의 의미를 구분한다.

## Product caller and BOOT evidence join

- 실제 명시 Native 제품 경로는 CLI `native_source_startup_v3`→`shared_execution_authority_adapter_v1`→daemon execution port→`attach_daemon_workspace_state_with_native_selection_v3`→`ensure_indexed_with_native_source_selection_v3`→기존 FULL target provider로 연결된다. fixed-target 진입점도 같은 publisher를 사용한다. 이 경로와 target 발급자를 재구현하지 않는다. 남은 수용은 reservation의 Original SQL 비용·수명/오류 custody, initial Original persist/epoch/import gate, 실제 FULL/delta 및 post-FULL evidence의 BOOT 인계다. 일반 EnsureIndexed의 암묵 Native 전환은 요구하지 않는다.
- 실제 BOOT 인계는 intended target.operation과 Native 후보 manifest_operation, owner가 검증한 post-FULL view/source를 교차 검증한다. target.expected_source는 게시 전 source이므로 후보의 게시 후 source와 등치시키지 않는다. opaque ACK의 `with_receipt_v3`가 주는 projection receipt만으로 Q operation/source가 증명되지는 않는다.
- [native_source_publish_v1](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-retrieval-kernel/src/index_sdk_ingress/native_source_publish_v1.rs)는 실제 Submit reply와 Published exact CatalogView를 얻고, [terminal_receipt_v1](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime-retrieval-kernel/src/index_sdk_ingress/terminal_receipt_v1.rs)는 operation·workspace/epoch/sequence·source frontier·view digest를 검증해 canonical custody한다. 이 기존 owner evidence에서 필요한 고정 크기 tuple을 scoped accessor로 인계하고 ACK 재시도/재시작도 같은 증거를 사용한다. 새로운 receipt issuer/store, bare DTO deep clone, ordinary JSON decode 우회를 만들지 않는다. accessor의 정확한 소유/반환형은 INT와 한 번 결합한다.
- SDK boundary의 과거4진단 수리는 기존 shared authority/orchestration bridge와 owner export를 사용한다. memory-bootstrap의 예전 `repo_id` 문자열2개는 실제 `repo_id_v2`·composed scoped writer·custody/revalidation을 검사하는 oracle와 구분한다. 현재 수용 상태는 README의 owning 결과를 소비하며, 이 과거 수리 설명을 새 미구현 목록으로 배정하지 않는다.

기존 negative tests는 잘못된 초기 target/sidecar 부재의 무수정 거절을 검증한다. 실제 ACK·cached/cold exact retry·FULL/delta 결과·BOOT 인계를 입증하지 않는다. 제품 facade는 기존 `runtime_executor_bootstrap_live.rs`를 사용하고 독립 process 복구는 [W2-02의 기존 IFX150 selector와 5 durable cut](W2-02-pending-recovery.md#process-proof-routing)을 소비한다. first-FULL 3개와 Facts pending-transfer 2개를 구분하며 같은 복구 fixture/가짜 Acked SQL을 SDK에 복제하지 않는다.

## Prepublication authority and command lifetime

- Native opt-in SDK facade 앞의 제품 composition은 실제 operation·expected ViewRef/SourceCut·profile·policy digest·source admission을 기존 deployment/Native authority에서 받아야 한다. C5 generation reservation과 사후 BOOT receipt는 이 초기 target을 발급하지 않는다. provider 계약과 actual caller를 함께 결합하고 일반 EnsureIndexed를 암묵적으로 Native로 전환하지 않는다.
- FULL Original은 해당 FULL·ACK/readback·고정 root/BOOT 증거 파생까지 소비한다. 후속 IDE 명령은 자체 genuine Current/Original과 절대 deadline으로 exact source·Q preflight·CAS를 승인한다. FULL keeper/deadline을 후속 편집 예산으로 재사용하지 않는다. immutable publication evidence와 이후 명령의 작업 권한을 구분한다.


## Committed import corpus boundary

The existing committed ImportGraph format owner remains the only writer/reader. Its input must include the ordered module corpus, rather than reconstruct it from Q2 edges: module identity, all eight edge axes and file witnesses, exports/reexports, degraded inputs, and zero-import modules. Initial parser production and target reissue are distinct; copying a prior cohort cannot create rows omitted by the initial producer.

- Reuse `OriginalImportGraphReadPortV3::with_module_graphs_original_v3` for the actual immutable cohort. The caller retains the admitted module/read owner, its Source DATA and typed failures through the enclosing finisher. A raw module slice is not a replacement for this source capability.
- Reuse the collector's existing verified Q2 typed/wire pair and retained RuntimeCtx through `VerifiedSharedQueryOutputV1::try_with_wire_v3`. This authenticates the existing canonical wire without another Q2 encoder or depth/cycle visitor. A CBOR byte-string member and an embedded CBOR value are different formats; the sole format owner must choose and validate one canonical shape while cutting producer, validator, reference version/domain and consumers together.
- Inventory rows already have a real Original read path through `CanonicalParseHirGenerationInventoryBatchV1::with_original_read_v3`, `visit_inventories_with_native_handoff_v3` and the member's `with_import_provider_rows_v3`. Preserve that authenticated CoreOutput/operation and the final request's Original custody. A diagnostic `C=()` inventory or decoded CBOR alone proves neither authority nor its absence: inspect the enclosing authenticated request owner and its actual publication handoff. Do not mint a new issuer or infer Source identity from a digest.
- The Native decoder must build graph/edge/module DATA directly from borrowed canonical input under the same Original, with paid traversal/allocation and retained partial backing. Ordinary Deserialize followed by funded copy does not establish this contract. The existing Core CBOR scanner checks framing/minimal headers/trailing input; the schema owner must additionally check tuple arity, field multiplicity, ordering, canonical map keys and applicable float representation.
- Existing module/edge births are the mutation boundary. Every public ingress, including a typed-copy input made by an ordinary fluent builder, must satisfy the same semantic validation before Native promotion. Failure drain or reentry cannot restore acceptance, detach backing or permit a prefix publication.
- A required committed corpus that exceeds the existing storage limit must fail explicitly before publication. Returning a successful absent binding is not recovery coverage. Preserve configured budgets; do not increase a ceiling or omit the corpus to make a product fixture pass.
- Keep one live internal representation and one committed format. Do not reinterpret the old inventory-plus-Q2 bytes as a full corpus or add a second store/parallel compatibility decoder. Pending replay JSON, its digest domain and existing SQL CAS remain unchanged by this corpus cut.

The code/verification state remains exclusively in README and VALIDATION; this section defines the shared producer/format/consumer contract and creates no additional ticket or completion ledger.
