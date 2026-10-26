> HISTORICAL REFERENCE — pre-cleanup ownership and handoff text. Current paths, owners and next conditions come only from [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old task names and interrupted child assignments below do not authorize new work.

# Oct-17 공용 파일·인계 규칙

상태/담당/순서는 [README](../../../plans/oct-17-index-closeout/tickets/README.md)만 소유한다. 이 문서는 코드 수정 경계다. 기존 claim은 잠금이나 현재 실행 증거가 아니며 인계 전 source/dirty hunk를 대조한다.

## Exclusive boundaries

| 티켓/단위 | 독점 경계 | 공용 합류 |
| --- | --- | --- |
| W2-02 INT issuer | pending root·jobs/delta·SQL/claim·startup inventory | snapshot: taxonomy finite enum/dirty-unit/read-requirement, cost: semantic plan/unit nested seed. 기존 seed/serializer 재사용; parent/Contract/COORD는 INT 통합자 |
| W2-05 SDK producer | `client/indexing_api.rs`의 실제 FULL caller/target 및 old/new closure·mutation/PTA producer | INT는 Required/AA/V9 접속; SEARCH는 인증 결과 수신; BOOT는 같은 target 소비. 같은 producer 별도 구현 금지 |
| W2-13 공용 | Contract coverage/result·Core ports·Kernel completion/publication CAS·Runtime composition·SDK validators | 해당 repo 공용 writer 한 명이 producer/consumer를 함께 결합. Cargo/lock·parent/export·registry 포함 |
| W2-13 directory | private seal/directory·bounded exact delta/tombstone·채택 준비 | `embedding.rs`/`ann_lance.rs` 공유 본문을 query worker와 동시 수정하지 않음. private child 경로를 계약 때 동결 |
| W2-13 query | private directory reader·visibility·routing/probes/refill | 공용 result/SDK/overlay 변환은 공용 writer에 인계 |
| W2-13 PHYSICAL | 기존 Common binder·Native file-set adapter·close/reclaim | `publication_lance_io`·capacity/retirement는 기존 PHYSICAL 담당. SEARCH가 물리 스택을 복제하지 않음 |
| W2-16 AST | 실제 언어 authority adapter·인증된 Stub 공급 | Runtime parent 등록은 공용 writer. external apply CAS는 실제 source-owner repo/caller를 명시해 인계 |
| W2-18 BOOT | target→selection·launcher Legacy→Native hunk | SDK/INT 발급 target만 소비. 새 target issuer/producer 금지. default는 A 후 |
| W2-09 계측 | 기존 harness와 source owner별 실제 phase hunk | 이미 제출된 부하 실행 재사용. daemon/producer 공용 parent는 해당 기존 writer |
| W2-19 통합 | 공식 selector·공용 정합과 실패 원인에 한정한 수리 | 각 원 oracle/gold/실행 결과를 소비하고 테스트/runner/issuer를 복제하지 않음 |

## Collision resolution

### W2-13 WAL policy join

W2-13의 필수 잔여는 수정 소스의 원4K 제품 수용이다. WAL 유지보수의 처리 가능한
상한·압력·정책 합류는 새 실패 근거로 선택하는 조건부 수리이며 필수 신규 기능이 아니다.
SEARCH는 resource 입력/정책 계약, Catalog call site·Runtime composition과 원4K
oracle을 맡는다. 기존 Catalog checkpoint charging/transaction·VFS
`Adapter::before_mutation`/`after_sync`·Common capacity/retirement 본문은
기존 Catalog/PHYSICAL writer와 exact hunk로 결합하며 계획만으로 소유권이 이전되지 않는다.
생성자/admission/public export는 producer와 consumer를 대조한 단일 writer가 수정한다.

`native_pump::run_reclaim_due`는 이미 빈 object 큐에서도 WAL checkpoint를 먼저 호출한다.
이 경로를 새 demand 큐·flag·poller·worker로 복제하지 않는다. 기존 reader-deferral,
WAL truncate/funding·fault gold도 원 oracle owner가 변경된 정책의 부족한 assertion만
보완한다. SEARCH는 같은 테스트 본문을 별도 작성하지 않는다. W2-09의 정식
Off/Record·운영 배치와 W2-19의 full-suite/CI/release는 원 담당 결과를 소비한다.
이 경계 명시는 채팅 전송·새 작업 배정·공용 본문 수정권 인수를 뜻하지 않는다.

### W2-02/05 cross-repository hunk map

고정 실행 범위는 [W2-05 A/B](../../../plans/oct-17-index-closeout/tickets/W2-05-producer-full-delta.md#native-chunk-plan)·[C](../../../plans/oct-17-index-closeout/tickets/W2-05-producer-full-delta.md#native-publication-caller)·[D](../../../plans/oct-17-index-closeout/tickets/W2-05-producer-full-delta.md#wire-canonical-custody)·[PUB E](../../../plans/oct-17-index-closeout/tickets/W2-05-producer-full-delta.md#sdk-producer-custody)와 [W2-02 terminal](../../../plans/oct-17-index-closeout/tickets/W2-02-pending-recovery.md#terminal-publication-join)을 따른다. Root A–D/Runtime upper caller와 PUB E/SDK parent는 서로의 body를 중복 구현하지 않는다. PUB의 현재 recompute claim `OCT21-PUB-RECOMPUTE-CUSTODY-V163`와 P cutover claim을 실제 수정 전 대조한다. 이 링크는 상태 원장이 아니며 current state/next condition은 README7행만 소유한다.

현재 배정/상태는 [SSOT 담당](../../../plans/oct-17-index-closeout/tickets/README.md#owners)과 위 7개 티켓 행만 갱신한다. 아래 경로는 S의 `packages/analysis/quanta-v2/crates` 아래이며 수정권 경계다. 인계 후보는 최신 live preimage와 대조한 뒤 원 writer가 결합한다. 후보 경로 수나 apply-check는 실제 합류를 뜻하지 않는다.

2026-10-20 exact-hunk 인계: archived W의 후속 scheduler/Runtime consumer는 Root가 인수한다. M의 SourceLocator/generation 조합, P의 structured-store/5-root 상위 generation은 원 담당을 유지한다. M이 넘긴 `scv2_orchestration/graph_facts.rs`와 children은 Root Graph worker, `executor/native_symbol_v3.rs`·`native_graph_v3.rs`와 children은 Root 문서 worker, `native_chunk_v3.rs`·canonical Chunk paid recipe는 Root Chunk worker가 독점한다. 공용 dispatch/types/Runtime pending/consumer는 Root만 수정한다. P는 ImportGraph source-range 및 canonical producer의 derived quality field/init/finish/getter hunk를 Root 문서 lane에 넘겼으며 기존 encoder/recipe를 보존한다. ItemIndex checked accessor는 old owner archived·M/P2 교집합 없음과 stale claim을 대조한 뒤 Root Chunk lane이 인수했다. `tests.rs::sample_symbol_doc_v1`은 P 인계 후 visibility만 Root가 넓혀 M fixture를 재사용한다. 이것은 파일 전체 수정권·새 wire/issuer·동적 수용을 뜻하지 않는다. 후속 Graph empty producer는 Root 문서 lane이 `executor/native_empty_v3*`와 `job_validation/outcomes.rs`의 기존 digest tuple 공유 hunk를 맡고, Root는 `NativePreparedIndexProjectionV3`와 ports/dispatch/reexport만 직렬 결합한다. P는 paid `AuthoritativeEmptyProjectionScopeV1`의 writer transaction/삭제 소비를 맡는다. Symbol zero의 ordinary InvalidBatch와 FULL 전체 surface 교체 권한은 per-file Graph empty와 합치지 않는다. Public Stage3의 Original lender 부재는 actual caller 연결 잔여이며 free accessor나 retained group을 issuer로 사용하지 않는다.

| 경계 | 독점 private body / 기존 owner | 직렬로 결합할 공용 hunk |
| --- | --- | --- |
| M metadata / initial ingress | `quanta-sdk-session-backend-owner/src/repository_indexing_v1/original_inventory_read_birth_v3/retained_json_birth_v3/binary_metadata_v3.rs`·`src/snapshot_port.rs`와 실제 query Source/metadata 소비; `quanta-sdk-runtime-composition-owner/src/bootstrap_inputs_v1/initial_original_source_ingress_v3.rs` | M의 Source/metadata/projection 인계와 PUB의 최종 게시를 `quanta-sdk-runtime-executor/src/client/indexing_api.rs`에서 한 통합자가 결합. 구현된 `original_initial_ingress_v3.rs`·constructor/caller를 중복 생성하지 않음 |
| P checked projection / RepoMap | `quanta-runtime/src/retrieval/port_impls/index_projection_writer`의 `graph_projection.rs`, Native stage/`structured_store.rs`, `repo_map_handoff_dispatch/native_emit_v3` | canonical persistent owner 변경은 원 owner와 결합. `repo_map_export_bundle/native_bundle_v3.rs`·`source_bound_final_assembly/import_graph_access_v3.rs`의 live carried export를 유지하며 C 후보의 해당 consumer hunk만 대조 |
| W initial corpus writer | `authority_assembly/import_graph_publication/native_full_v3.rs`와 동명 child directory의 인계된5개 private 파일 | `authority_assembly/import_graph_publication.rs`, `batch_prepare/commit_request_v3.rs`, `indexing_import_graph_payload.rs`와 Ingress/parent/export/Cargo는 통합자. 진짜 module/inventory/Q2 lender와 sole frame/ref를 함께 연결 |
| C opaque durable consumers | `committed_import_graph_publication_guard_v1.rs`, `import_graph_carry_v1.rs`와 retained full-origin/pin export/회귀 | 이미 adopted된 C16/Root13/P5·Runtime2는 재적용하지 않음. C는 validated_carry_for_publication_v1의 NativeCarried successor 검증·반환 contract만 W와 exact hunk로 결합. source/record move·borrow 수리는 재개발하지 않음. 신규 paid canonical plan/receipt·immutable pending custody는 Root 담당이며 C consumer 범위를 덮지 않음 |
| I signed SQL / INT | `quanta-sqlite-native-reader/src/{protocol_v1,reader_v1,worker_v1}.rs`, 기존 patched Unix VFS·pending/checkpoint private body | `quanta-runtime/src/indexing_machine_v2/store/{mod,connection,native_replay_connection_v3}.rs` 의 기존 body는 재사용하고 coordinator/V9 상위 caller는 Root가 실제 게시 artifact에 결합한다. I 인계를 기다리는 새 작업을 만들지 않음. AST/SDK/계측이 별도 SQL engine·VFS·CAS를 만들지 않음 |
| PUB public SDK / install | `quanta-sdk-session-backend-owner/src/repository_indexing_v1/original_source_publication_v3/materialize_v3.rs`, `quanta-sdk-runtime-executor/src/client/indexing_api/original_initial_publication_birth_v3.rs`의 source-index/final install 계약 | Root가 반환한 실제 publication artifact와 SDK parent를 직렬 결합. Runtime 내부 Chunk/manifest/root orchestration은 Root이며 같은 본문을 PUB가 별도로 구현하지 않음. 적용된 lower/upper packet 재적용 금지. Java/Parser/type-world/normalized projector·fresh Native journal/registry authority는 해당 canonical producer의 genuine 입력만 소비 |

추가 직렬 접합:

- **최종 caller 통합 경계:** Root는 `consumer_v3/{chunk_preplan_v3,plan_v3,entry_handoff_v130,native_manifest_handoff_v165}.rs`와 실제 Chunk 문서/dispatch, 내부 publication의 upper caller·필수 export 인수를 소유한다. P는 기존 writer/stage/generation/FinalDocs/vector/root/CAS bank body, PUB는 `indexing_api.rs` 및 공개 SDK/install/recovery와 `clone_for_target_generation_v1`·Facts/recompute 실패 custody를 소유한다. Root와 PUB의 접점은 실제 게시 artifact이며 selected/staged receipt로 대체하지 않는다. 실제 Source/target 결과의 AST/BOOT 인계는 Root가 통합하고 SDK parent는 PUB만 한 번 바꾼다.
- **완료 지원 코드의 소비:** M FullSearchPlan·I Parent/reconciliation/receipt/preplan query·W corpus writer의 기존 인계를 Root가 직접 소비한다. 이들의 archived/idle 상태를 새 구현 blocker나 동일 leaf 재개발 사유로 사용하지 않는다. Wire `original_canonical_admission_v3/native_memory_v3.rs`의 raw physical/header custody는 Root의 별도 hunk이며 Source·Args·snapshot의 일반23파일 완료와 혼동하지 않는다.
- **claim·검증 경계:** 현재 P의 `QIDX-P-NATIVE-PUBLISH-CUTOVER-20261020` claim이 writer 공용 경로를 포함한다. Root는 변경 직전 fresh preimage/dirty와 해당 invariant를 확인하고 기존 caller 통합 인계에 맞는 정확 hunk로 reconcile한다. claim은 파일 전체 lock이 아니며 P body를 덮거나 전체 파일을 재적용하지 않는다. Runtime owning은 P의 현재 profile/결과를 소비하고 SDK/CP는 기존 PUB/PY 큐에서 별도 수용한다. Root가 동일 Runtime 실행을 복제하지 않는다.

- 2026-10-20 후속: Root query 비용 lane는 `executor/native_v3/path_birth_v3.rs`·Symbol/Chunk Args 생성과 기존 Core digest 재사용을 마감했다. Root 하위 scheduler enqueue lane는 `native_transaction_v3/{prepare_v3,edits_v3}.rs`와 Contract blocked-reason recipe hunk, drain lane는 `native_drain_v3.rs`·private readiness child를 소유한다. P는 writer high-level Native readiness와 shared borrowed predicate를 소유하고 Root control-plane port·typed snapshot-absent observation을 소비한다. Native manifest pin parent/children의 exterior DATA·RCU 조회·counted-token 수명 수리는 Root pin lane만 맡는다. `depends_on`의 실제 terminal witness 없이 성공을 열지 않는다.
- P가 인계한 `structured_store/native_stage_transaction_v3.rs`의 const-N/N=1 entry와 `generation_build_v3/native_replacement_plan_v3.rs::try_resolve_from_stage_v3` signature는 Root가 한 번 결합한다. 기존 FULL4·단일 delta1이 같은 엔진/예산/순서를 사용하며 원 `fixtures_v3.rs::prepare_v3`는 서명만 N으로 확장한다. P empty/4-batch 회귀·공통 fixture 본문을 복제하지 않는다. Chunk full-file provider/substrate/operation plan 공급은 기존00→11에서 원 producer owner를 지정하기 전 수정권을 추정하지 않는다.
- 위 query/pin/readiness seam·const-N source 합류 후 Root의 완료 claim5개는 해제했다. 이후 실제 paid Batch/Empty→writer 수락/coverage/shards/verdict 연결은 P의 기존 writer 경계, signed Store/worker 설정→SDK 생성자 연결은 PUB의 기존 경계, 그 실제 결과→scheduler completion→pending/V9/ACK→outer 반환은 Root 경계다. native_stage finish의 `Result<()>`나 selected handle만으로 writer 성공 증거를 만들지 않는다. Core digest 원인 이동 API와 Symbol/Graph O(1) 원인 반환은 세 독점 source hunk로 합류했으며 새 allocator/codec/receipt owner를 추가하지 않는다.
- Root는 NativeInitialImportProducerV3와 public inventory loan callback을 initial facade→collector/context→projection/final ingress에 전달한다. M은 같은 callback에 genuine Product epoch를 terminal→RepoState→SDK caller로 인계한다. alias만 붙이거나 없는 epoch를 재발급하지 않는다.
- W는 PreparedImportGraphRequestV3 및 기존 generic prepaid failure engine을 소유한다. Root 하위 오류 lane은 ordinary_aggregate_prepare_v2.rs의 Failed/execute와 authority_finalize.rs의 consuming constructor·후속 bundle finalization을 연결해 typed first/abort/reconcile 원인과 whole request/corpus/prepared authority를 함께 보존한다. W의 materialize_acked_source_bound_publish_batch_v2는 실패 시 FailedSourceBoundMaterializationV3{cause_v3,prepared_batch_v3}를 반환하고 Root caller가 이를 기존 prepaid bank에 이동한다. NativeProducerBirthV3의 Arc input 보유는 기존 allocator의 exterior-slot seam으로만 확장하며 P stage/generation·W publish_commit·I SQL 본문과 구분한다.
- Root query-preparation은 borrowed manifest/generation으로 fallible port 생성을 먼저 끝내고 `complete_source_bound_search_plane_lane_bundle_v3`에서 request를 infallible move한다. Native lexical child constructor는 W가 소유하고 actual finalizer·paid binding-map copy·packing/literal 소비는 Root 오류 lane이 소유한다. C는 graph-pin exterior slots만 변경한다. validation/repair reader와 escaped shipping-authority retained alias는 Root 검토 lane이 연결하며 같은 bundle 파일의 C graph hunk를 보존한다.
- P canonical engine/removal/common stage/generation은 원 P가 유지한다. Root private stage-value lane의 quota 중단 후 Root가 actual Output key/list slots·retained shallow list만 마감했다. P가 그 소스와 나머지 File/Symbol/Chunk/Callsite paid document copy를 원 common-stage consumer 범위에서 이어받는다. NativeProducerBirth-only 중복 convenience는 제거했다. 기존 GraphEdge copier·새 map/IR·P tree 본체를 중복 작성하지 않는다. canonical imbl의 fallible ordinary copy와 guarded/admitted point lookup 두 API만 Root가 P와 hunk를 조율해 확장한다. 같은 insert/remove/get 순회와 node 표현을 유지하고 P stage/generation consumer는 건드리지 않는다.
- Root 하위 async lane이 적용한 ordinary queue/receipt/state move·borrow는 유지한다. W는 `incremental_rail.rs` Native producer 분기와 `core_invalidator`의 Native 선택/인수 전달, `change_path_policy.rs` canonical dirty builder의 admitted hook만 확장한다. W는 인계 완료한 Source capsule 다음으로 control-plane `planning.rs`/`planning/{native_specs_v3,native_body_v3,native_entry_v3}`와 Contract `indexing_pipeline/native_plan_parts_birth_v3` closed builder 및 concrete entry의 exact mount를 맡는다. M은 기존 metadata source를 보존하고 `coordinator.rs`/private children·Native lease/candidate/closeout 및 해당 owner 회귀만 맡는다. W는 planner source 마감 뒤 scheduler.rs와 scheduler private transaction/resident의 인계된 exact hunk를 맡고, M은 coordinator를 유지한다. Root는 Contract의 scoped predecessor metadata/outcome·receipt owner 분리, Native pipeline·pending-closeout·상위 Source bank/실제 caller와 Contract/control-plane lib exports를 맡는다. `indexing_pipeline_ports.rs`의 W module mount와 Root retained lender/옛 by-value API 제거 hunk는 구분해 합류한다. 중단된 하위 lane의 적용 source만 인수하며 미구현 상태를 보존한다. 같은 dirty-family/sort/validation·IR을 복제하지 않는다. Root는 공용 Runtime assembly/contract/caller를 직렬 합류한다. I는 SQL statement/CAS/row wire와 canonical serializer를 유지한다. 원본 loader는 collector에 이미 존재하므로 재구현하지 않는다. Native pin/Source residency와 restart 재획득 계약이 닫히기 전 omission gate를 열지 않는다.

`tools/rust-crate-boundary-guard/{check_retrieval_error_integrity.py,tests/test_retrieval_error_integrity.py}`는 P/기존 guard owner가 한 번 결합한다. RepoMap canonical child 추적, 별도 read-host 음성 회귀, C의 guard 민감도 후보를 구분한다. 같은 파일이라는 이유로 read-host 수리를 P/C가 각각 구현하지 않는다.

- 현재 Native 검증 합류의 독점 경계: Root Contract lane는 공통 Doc/Batch/Evidence/scalar/typed-error 정책·private 구조만, Root query owner lane는 FileText/ImportGraph 실제 checked-read의 after-Source owned-bank 전달과 같은 생성자 borrowed-input만, Root producer lane는 File/Symbol/Graph 실제 typed doc 생성·admitted 검증 및 그 failure DATA 반환을 소유한다. File/Graph와 Symbol checked owner를 서로 중복 수정하지 않는다. Source 아래 callback의 실패도 exterior slot에 먼저 저장하고 raw Serde retainer 부재를 성공으로 삼지 않는다.
- 후속 query 실패 수명은 Root query-consumer lane가 File/Symbol/Graph query 함수의 input/output/checked-birth partial 반환 hunk를 맡고, Root producer lane는 공통 NativePreparedProjectionFailureV3·projection bank·path_birth_v3.rs 최초 paid path 실패 반환을 맡는다. Root test lane는 독립 failure-lifetime child와 기존 Graph birth test constructor8개만 co-cut했다. P owned-stage/shard extraction과 별도로 Root helper가 native_projection_shards_v3.rs 기존 prepare_rows/prepare_group_outputs 두 private body의 GOD 구조 수리만 맡는다. 알고리즘·정렬·digest·copy·진단 순서·예산은 원 owner를 재사용하며 두 lane의 DATA/finisher와 중복 편집하지 않는다.
- Root의 neutral Replay adapter 추출·13 caller 인수 합류와 signed pending joint15/plan-ID whole-terminal 합류는 적용 후 claim을 해제했다. old v107/v115 후보의 whole-file 재적용은 금지한다. 현재 getter4개 대신 existing Frozen borrow를 사용하는9 read co-cut은 wire/SQL/CAS 본문 변경이 아니다.
- 실제 Native prepare의 owned failure DATA→기존 prepaid outer 오류 bank→같은 pending custody Source-last 연결은 Runtime producer와 PUB outer caller가 함께 co-cut한다. PUB는 기존 drain/queue/core 알고리즘을 다시 구현하지 않는다. P는 실제 writer acceptance와 discard/doc/root/pin/lease/GC 및 게시 artifact를, I는 그 actual artifact/BoundV2 attempt/epoch·Native preparation과 same signed Store를 받는 기존 V9/ACK를 소유한다. stage receipt를 publish/V9 증거로 만들지 않는다.
- Core에는 기존 Option-funded header/birth/strong/weak 엔진이 이미 있다. 정확한 no-overlap source/claim 확인 뒤 Root가 기존 public_node_funding_v3.rs에 Option<NativePublicNodeFunding>의 immutable Deref 하나와 기존 owner test2개만 적용·검토하고 claim을 해제했다. 이 hunk는 재구현하지 않는다. 나머지 Core는 기존 담당 유지, P는 같은 pointer의 Ordinary None/Native actual-grant Some을 소비한다. unit Arc를 Native grant로 위장하거나 별도 root store/Source를 추가하지 않는다. sole PY resource/QBC 실행은 공식 외장 target/snapshot에서 재개됐으며, 다른 lane는 중복 compile/capture/GC를 하지 않는다.

### Other shared boundaries

아래는 중복 가능성이 남는 shared boundary다. 단일 writer 원칙이 선언돼 있다는 이유로 실제 hunk 충돌이 해소됐다고 판정하지 않는다. 착수 전 기존 owner와 함수/hunk 인계가 확인돼야 한다.

| 겹치는 lane | 유일한 수정 권한과 소비 경계 |
| --- | --- |
| R pending ↔ F producer | S coordinator/V9/공용 Contract 함수는 INT 통합자. R은 pending Store/private seed, F는 SDK entry/target·closure의 인계된 private hunk만 소유. F가 pending serializer/claim을 다시 구현하지 않음 |
| F producer ↔ D ANN | source/member/vector identity·chunk manifest·vector allocation custody의 원 생산자는 F/기존 semantic owner. D는 같은 인증 입력의 물리 search layout만 준비. chunking/embedding·policy hash 재생산 금지 |
| D directory ↔ Q query | canonical directory codec/seal writer는 D, query는 동일 descriptor를 읽음. 두 번째 seal/codec·selection map 금지. 파일 helper 공유 본문 변경은 C가 한 번 결합 |
| D directory ↔ P 파일 | D는 logical refs/descriptor, P는 실제 파일/admission/FD/reclaim. C는 port/Catalog 채택. P가 검색 directory를 별도 게시하거나 D가 자체 FD/GC ledger를 만들지 않음 |
| C ANN ↔ S Stub ↔ O 계측 ↔ B BOOT | Q Runtime의 composition/registry/생성자·native_product parent는 해당 repo 공용 writer. 각 lane은 exact adapter/private hunk와 인수 변경을 인계. SDK public validator/parent도 같은 방식으로 직렬 결합 |
| F/INT ↔ O 계측 | S producer/COORD의 실제 함수 body는 기존 producer/INT writer. O는 기존 trace 계약·필요 phase/counter 인계. 계측을 이유로 producer 알고리즘이나 JSON/serializer를 다시 작성하지 않음 |
| B BOOT ↔ release target | launcher/selection·Legacy→Native 연결은 B만 구현. W2-19는 실제 target/observer 입력과 실행 수용을 추적하며 두 번째 launcher/action executor를 만들지 않음 |
| 각 기능 oracle ↔ W2-19 | 기능 담당이 독립 gold/회귀와 actual caller를 소유. W2-19는 같은 결과를 소비하고 missing release process scope만 보완. 이름만 다른 동일 테스트/실행 배치 복제 금지 |

W2-19는 G0에서 `ticket/lane → existing owner → exact path::function/hunk → caller contract → required integration writer`를 해당 티켓의 packet에 확정한다. private 신규 파일 경로는 제안만으로 write 권한이 생기지 않는다. 동일 path의 disjoint hunk도 parent/export/formatter 영향을 확인한다. 확인되지 않은 작업은 정적 설계·독립 gold 준비까지만 진행한다. 별도 lock service/작업 registry 파일을 추가하지 않는다.

## Handoff

- 인계에 ticket ID, exact path와 함수/hunk, 서명/authority, 재사용 코드, 독립 oracle, 실패/drop 순서, 미실행 검사를 포함한다. whole-file overwrite/formatter로 다른 hunk를 덮지 않는다.
- 한 파일의 private child는 병렬 가능하지만 parent/export/schema는 한 writer다. 공용 caller 서명을 동결하고 실제 생산자/소비자를 함께 변경한다. definition-only API는 연결 완료가 아니다.
- worker는 통합자에게만 반환한다. 다른 worker·채팅에 직접 요청/보고하지 않는다. 이 문서 자체는 다른 채팅 메시지나 소유권 인수를 승인한 기록이 아니다.
- 기존 Semantica claim/세부 hunk는 [과거 인계 참고](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)를 탐색 자료로만 사용한다. 현재 배정·실행·상태는 그 문서에 갱신하지 않는다.

## Verification queue

정적 call/ABI·binding·취소/실패·backing-before-grant Drop를 먼저 검토하고 같은 source의 영향 검사를 통합한다. Q는 `./scripts/cargow`/Justfile의 공유 resource admission을 사용하고 S는 owning QBC를 사용한다. 같은 blocker의 필터 변형 재실행·잠금 우회·타 담당 프로세스 종료를 하지 않는다. 실제 source/원인이 바뀐 재검증은 차이를 기록한다. 컴파일·선정 테스트·제품 E2E·qualification·remote CI/release를 각각 판정한다.


## 2026-10-20 Native dependency and funded outer join

- CP lane: scheduler/native_drain_v3 readiness/completed-batch/complete and existing native_transaction topology owner. Paid topology order, actual accepted dependency witness, existing ordinal index and paired final order only. No Runtime fixture or PUB drain edits.
- Contract lane: indexing_pipeline/native_outcomes_v3 paid in-place sort and partial-sort poison only. Does not supply completeness, writer acceptance, scheduler state or a second sort engine.
- Runtime test lane: native_planner_v3/scheduler_v3.rs caller migration and new same-Original writer acceptance/refusal regressions. Existing paid P slots are test-only bridge; no fake success or new writer.
- PUB: live v125 selected_drain/executed_outcome/owned_drain caller successor. Consumes CP prepare/next/accepted tuple and combined final sort, retains whole failure/custody. No duplicate CP lookup or ordinary fallback. Root serially applies a fresh exact-preimage candidate.
- Root: Native Chunk preplan full-file producer/custody/recipe/planner seam after notifying existing00/11; P store/writer and PUB outer SDK remain their current owners. Core shared parent baseline findings stay with the existing Core owner.
