# W2-03 — V5 snapshot과 검증 scratch의 실제 backing admission

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-03)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R11,R12,R37 / NI-01. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

- [x] `DONE_IMPL`: snapshot admitted seed/holder·same encoder compare·actual unit-set scratch·clear-surface borrowed slice의 명시된 source 결합을 재사용한다. Runtime 수용 완료는 아니다.
- [x] `DONE_IMPL; STRUCTURE_VERIFIED`: Contract의 port trait을 제거하고 borrowed sort/comparison-admission callback으로 같은 canonical comparator/hash와 Foundation fallible sorter를 연결했다. `just rust-hexagonal` 통과. Contract→Core 역 의존·lint 예외 없음. S 호출부의 새 Runtime 검증은 별도다.
- [x] `DONE_IMPL`: snapshot current-scope의 clear-surface 검사에서 BTreeSet을 고정 4종 비트마스크로 바꿨다. clear membership은 O(C+S) 시간/O(1) 추가 공간이며 오류 순서는 유지한다. 추가 동작 테스트의 실행은 아직 미완료다.
- [x] `DONE_IMPL`(Lexical receiving 호출부): Contract의 `CorpusSurfaceOrderScratchV3`/`validate_surface_mutations_with_borrowed_scratch_v3`와 S의 `admitted_lexical_decode_v3/surface_scratch_v3.rs`가 mutation/candidate/semantic 중복·교집합 검증을 funded backing으로 연결한다. 이 구현을 다시 만들지 않는다. snapshot의 별도 재검증과 producer 파생 출력은 아래 범위다.
- [x] `DONE_IMPL`(Native validator caller, 실행 미검증): Native snapshot scope/source 중복·cluster membership은 `native_validation_v3.rs`의 Original-backed sorted index/seen scratch를 사용한다. `validate_lexical_native_surface_v3`는 기존 shape 검사 뒤 공유 surface-only 검증을 재사용하도록 추출됐다. 공용 re-export 2곳은 결합됐으며 실제 Runtime 수용은 아래 범위다.
- [x] `DONE_IMPL`(Native successor caller, 실행 미검증): `native_successor_v3.rs`는 prior/current scope를 빌려 Original-backed scratch와 같은 canonical encoder로 `AdmittedCanonicalOutputV3`를 반환한다. `aggregate_prepare`의 `SemanticOwnerSnapshotOutputV3`는 grant를 함께 보유한다. 이 반환값의 pure issuer 인수는 아래 admitted copy capability로 연결됐다. 기존 prior/shadow 할당 전체를 제거한 표시는 아니다.

- [x] `DONE_IMPL`(snapshot output 인수): pure issuer의 private `snapshot_copy_v3.rs`가 기존 NativeTemporaryVecBacking/Original group으로 canonical bytes를 사전 예약·복사하고 issued artifact의 마지막 backing 필드로 수명을 보유한다. Foundation의 opaque `AdmittedCanonicalCborBytesV1`는 Vec detach API가 없으므로 raw escape 대신 명시적 O(output bytes) copy를 선택했다. encoder+copy의 동시 peak와 work는 실제 비용에 포함한다. 타입/Runtime 실행 및 full issuer abort/V9/unwind 검증은 별도다.
- [x] `DONE_IMPL`(Native prior caller): `aggregate_prepare`는 Native일 때 `NativePriorSnapshotIndexV3`와 borrowed view를 공유 delta planner에 전달하며 ordinary `prior_state_v1()` clone을 호출하지 않는다. unchanged hierarchy도 필요한 parent/module을 조회한다. full ordinal index O(N log N)·전체 snapshot canonical encode/copy O(S)는 남는다.
- [x] `DONE_IMPL`(실제 producer 연결, 실행 미검증): aggregate Native caller가 `Some(birth)`를 lexical/semantic 공용 builder에 전달하고 `from_native_produced_v3(birth.into_produced_v3(batch), prepared)`로 최종 DTO/group을 인수한다. coverage/identity/language/source digest·symbol/raw/module/cluster 출력, borrowed current digest view·module ordinal·transition Vec를 같은 bank에 연결했다. ordinary branch만 Owned다. 기존 helper-only/None 판정을 폐기하며 다시 연결하지 않는다. generation pin/policy·Degraded/error·초기 metadata와 work/lifetime 검증은 아래 잔여라 fully Original-funded 완료는 아니다.
- [x] `DONE_IMPL`(공용 helper): Native canonical digest는 기존 Original encoder/hash body를 재사용해 고정 Hash를 반환하고 funded String으로 변환한다. symbol-kind ordinary/Native는 11-byte stack classifier를 공유하며 source slice는 borrowed view를 제공한다. 별도 serializer·taxonomy·renderer를 만들지 않는다. 실제 Native producer 전체 연결은 아래 범위다.

- [x] `DONE_IMPL`(cluster Ready 경로, 실행 미검증): 단일 `Scv2PinnedClusterFactsV1`에 prepared-generation borrowed storage를 추가했고, 같은 ordinal/membership body가 ordinary/Native를 처리한다. Native aggregate는 borrowed facts→Original bank→공통 `SccWorkspaceV4::try_run_indexed_v4`→lexical builder→NativeProduced로 연결된다. 별도 Native SCC/planner는 없다. bank는 Original scope로 이동했고 옛 lexical child/consumer import는 제거했다.
- [x] `DONE_IMPL`(추가 scan work, 실행 미검증): Python chunk prefix/header, card comparable facts의 공유 scan, hierarchy subset 검증에 기존 work callback을 연결했다. callback 존재는 전체 루프/비교의 비용 ceiling 수용이 아니므로 거절/cancel·결과/오류 순서를 검증한다.

Runtime79713c6c의 실제 E0365/E0364/E0603 진단에 따라 canonical prior index/생성자와 successor 함수의 visibility를 실제 handoff parent까지 정합시켰다. successor reexport도 같은 범위로 축소했다. 정의/로직/IR 복제 없음; fmt/diff는 VERIFIED이고 후속 Rust 검증은 NOT_RUN이다.

- [x] `DONE_IMPL`(prior config copy, 실행 미검증): canonical `PersistedSearchPlanePayloadLoadSpecV1`가 `expected_revision_id: &str`를 빌리도록 했고 이전 generation 로드에서 `..config_v1.clone()`을 제거했다. bound artifact·decoded identity의 기존 검증 body는 같은 borrowed revision을 사용하며 모든19 producer/fixture literal을 함께 변경했다. 현재 config revision이 바뀌어도 과거 row revision으로 읽는 fixed 회귀를 추가했다. 초기 aggregate config의 비UTF8 `PathBuf`와4 String birth는 아래 후속 연결로 저장됐으며 이 prior-load 회귀와는 별도 수용이다.

## Scope

snapshot decode·canonical comparison·unit-set/surface 검증과 prior/successor 파생 자료의 실제 backing 및 work를 같은 admission 계약으로 다룬다.

`semantic_state.rs::validate_batch_native_v3`의 Native surface callback은 추출된 `validate_lexical_native_surface_v3`를 소비하도록 변경됐다. `prepared_member_revalidation_v3.rs`와 `admitted_lexical_decode_v3.rs`는 surface-only 함수를 같은 owner facade로 re-export한다. 실제 recovery/predecessor/issuer에서 exact Native target을 전달한다. shape/hash 검사는 한 번, surface predicate는 기존 구현 하나를 사용한다.

`aggregate_prepare.rs`의 Native prior clone 회피와 actual Some(birth)/NativeProduced 인수는 연결됐다. bank 내부 transition/index/cluster 출력은 source 연결을 유지하고 실제 parity/refusal/drop을 검증한다. 남은 생산 할당/work는 아래 실제 경계다. ordinary-only legacy loader 전체를 일괄 수리하지 않는다.

- **완료된 metadata 수리의 검증:** generation pin/policy/axes·repo/revision 입력 복사와 inline Degraded 이유를 existing bank에 연결했다. ordinary/Native가 같은 owner cardinality 결정을 사용하며 3+ owner 후보도 전체 정렬 Vec로 보존한다. 기존 Ready/Degraded/parity·작은 budget/거절·drop·3-owner fixtures를 실행한다. 이 source 연결을 다시 개발하지 않는다.
- **초기 config·event 입력의 연결(동작 미검증):** canonical config producer가 optional 기존 `NativeProducerBirthV3`를 받아 동일 검증 뒤 PathBuf·4 String을 실제 Original group에서 birth한다. bank를 config/ingress IO 앞으로 이동했고 ordinary 환경/fixture caller는 None이다. 경로는 기존 `OriginalCanonicalAdmissionV3::try_copy_original_os_path_into_v3`와 exterior `OriginalOsPathCopyDataV3`를 소비해 비UTF8 OS units·typed cause를 보존한다. 새 path allocator/IR은 없다. Source stream/event와 completed predecessor event ID도 기존 canonical CBOR/hash와 bank의 stable digest renderer를 공유한다. aggregate는 event 결과 변환 전 Original first cause를 확인한다. config/context DATA를 출력 인수 전에 drop한다. 최종 plan digest는 Native copy_string 뒤 기존 generic PreparedBatchCustodyV3<String>/NativeProduced holder로 유지하고 into_parts→issuer에서는 holder가 유지된 borrowed str만 사용한다. lexical과 digest가 다른 순서로 해제돼도 실제 group을 유지한다. OS-path 거절의 checkpoint/DATA 원인에 실제 TryReserveError를 결합하고, sibling drop 및 map 성공·unwind에서 DTO-before-grant와 환급을 확인할 회귀를 추가했다. nonUTF8 path, 고정 config/작은 Original 거절·환급, event의 ordinary parity/attempt binding·수명/거절 회귀를 저장했다. 첫 retry/config 검사 capture 이후 event delta는 별도 source이며 Rust 수용은 아직 미완료다. R-ENG-05: Native bank·canonical OS path copy/exterior DATA·기존 ordinary config/CBOR identity body와 실제 aggregate/lexical caller를 대조해 같은 owner를 확장했다. 최종 batch event는 이미 funded였으므로 재작성하지 않았다.
- **cluster/semantic error의 외부 수명(구현 저장, Rust 미수용):** 실제 Native FULL에서 도달하는 cluster membership/pin/facts 오류는 할당 전에 같은 bank로 String·retained Arc backing을 확보하며, typed floor와 semantic rejection을 분리한다. Native validator와 aggregate RuntimeError Display 변환도 실제 owner를 보존한다. Contract의 retained CommitRejected/BackendUnavailable은 원래 분류·문구를 유지하고 소비자 분기도 함께 변경했다. canonical cluster ID의 거절 callback은 반드시 Err를 반환해 잘못된 정책이 성공 ID로 바뀌지 않는다. 불변식 literal은 retained static이고 checked overflow는 실제 Original 원인을 기록한다. 고정 Unicode 문구·ordinary parity·body/Arc clone 수명·거절/환급 fixture와 9c/32c에서 탑재되는 Native validator 음성2개를 저장했다. 기존 decoder8개 gate는 유지한다. 컴파일/실행 성공 주장은 아니다.
- **초기 FULL의 도달 경계:** successful initial FULL guard 뒤에서는 prior semantic delta와 completed predecessor row 추출을 실행하지 않는다. 이를 초기 FULL 미구현으로 다시 개발하지 않는다. guard 이전의 predecessor 부재 확인·V9/pending/SQL driver 비용과 aggregate prologue/SDK·IPC의 외부 diagnostic backing은 W2-02 경계다. plan digest는 위 holder로 연결됐으므로 잔여로 중복 등록하지 않는다.
- **work/lifetime:** chunk/card/subset 패치와 cluster sort/Vec 성장/SCC의 실제 scan·compare·copy를 대조한다. Ready·Degraded·거절·cancel·error precedence와 metadata 구조/임시 peak의 before-birth·환급을 검증한다. sort 비교만 예약하고 내부 node/string/allocator backing을 funded라고 주장하지 않는다.
- **초기 작은 metadata:** 남은 source config/event·pending/V9 metadata copies의 owner를 추적하고 Native 도달분을 기존 authority와 같은 grant에 결합한다. 이미 funded인 lexical/source helper·scope·card 출력을 재작성하지 않는다.
- **실행 검증:** actual Some(birth) fixture의 ordinary gold·digest/scopes parity·produced owner drop 환급·oversize refusal를 추가했다. 테스트 존재가 실행 성공은 아니다. root가 실제 owner lib(test) 컴파일 진단을 받아 동일 packet을 수리하고 genuine Original behavior를 닫는다.

최종 group은 DTO보다 오래 유지하고 prepare/issuer/dispatch까지 전달한다. Ready planner/Some(birth)/holder 연결만으로 Degraded/error/metadata를 포함한 전체 Native 회계까지 완료 처리하지 않는다. full snapshot representation을 유지하는 현재 경로는 출력 O(S) 비용을 가지며, prior clone 제거와 budget 연결만으로 delta-only 성능을 보장하지 않는다. W2-09에서 N/Delta/output 축을 분리해 측정한 뒤 W2-10의 정책·구조 변경 여부를 결정한다.

공용 통합에서는 `prepared_aggregate_owner_pipeline_v1.rs`가 output enum의 `bytes_v3()`를 validator에 빌려주고 `prepared_aggregate_issuer_v1.rs`가 Native canonical output을 admitted artifact bytes로 인수한다. pipeline은 `bytes_v3()`로 빌리고 pure issuer는 `PreparedSemanticOwnerSnapshotV3`로 인수하도록 연결됐다. persisted `Option<Vec<u8>>` wire를 유지하며 실제 copy 전 backing/work를 예약하고 artifact DTO가 해제될 때까지 passive grant를 보유한다. bare `to_vec()`·grant보다 오래 사는 detached Vec·두 번째 wire IR로 우회하지 않는다. issuer의 성공/오류/unwind/abort 소비자까지 함께 결합한다.

Contract는 canonical order/wire/digest와 중립적인 인자 계약을 유지한다. 실행 자원/전략 port의 소유는 허용된 상위 계층으로 옮기고 Contract에 Core 역 의존을 만들지 않는다. borrowed callback/adapter 경계를 호출자와 함께 결합한다. port 이름 변경·lint whitelist·비중단 sort fallback으로 우회하지 않는다.

## Acceptance

- 기존 `just rust-hexagonal`을 통과하고 S의 실제 unit-set caller/adapter가 같은 comparator·funded work/refusal을 소비하는지 검증한다.
- 고정 snapshot semantic/digest gold, ordinary duplicate/error precedence, sort order와 side-output 오류를 보존한다.
- source-event payload digest와 canonical batch digest를 각각 독립 변조해 거절을 확인한다. 같은 payload hash를 다시 계산하는 것으로 서로 다른 두 commitment 검증을 대체하지 않는다.
- scratch 부족·checked overflow·sort refusal·cancel·abort 뒤 재사용 거절 및 grant release 순서를 검사한다.
- 임시 bank row는 퇴역·환급 전 재사용하지 않으며, scratch/count/layout 및 sort 비교·이동 work 계산은 checked 연산을 사용한다.
- 실제 recovery/predecessor에서 before-birth backing과 prior/successor/derived-map/canonical-output peak를 검사한다. 논리 lease를 RSS 상한으로 해석하지 않는다.
- actual Native producer가 birth `Some`과 retained `NativeProduced`를 소비하는지 확인한다. 독립 cold→reopen→delta gold와 ordinary semantic/digest oracle로 prior clone 회피·fixed rendering/order를 검증한다. helper-only 테스트는 제품 연결을 대신하지 않는다.
- symbol/module/cluster renderer와 coverage/identity/digest의 거절·오류·unwind를 검사하고 DTO→output group 해제 순서를 보존한다. issued test constructor의 drop/unwind와 실제 writer abort/V9 인계의 범위는 구분한다.
- actual producer의 ordinary map/clone·cluster를 남긴 채 `Some(birth)`만 켜는 회귀를 막는다. current digest-state borrowed view의 입력 수명과 admitted index 정렬/중복·range lookup을 독립 fixture로 검증한다. 구조 guard18/18·관련 negative pytest4/4는 Rust compile/Original behavior 결과를 대체하지 않는다.

## Source entry points

- [aggregate_prepare.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/aggregate_prepare.rs)
- [surface_scratch_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/prepared_member_revalidation_v3/admitted_lexical_decode_v3/surface_scratch_v3.rs)
- [unit_set_scratch_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/source_publication/unit_set_scratch_v3.rs)
- [semantic_state.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/semantic_state.rs)
- [native_successor_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/semantic_state/native_successor_v3.rs)
- [native_validation_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/semantic_state/native_validation_v3.rs)
- [prepared_aggregate_issuer_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/prepared_aggregate_issuer_v1.rs)
- [source_coverage.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/source_coverage.rs)
- [validation.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/ipc/ingest/validation.rs)
- [native_prior_index_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/semantic_state/native_prior_index_v3.rs)
- [native_producer_birth_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/original_native_memory_scope_v1/native_producer_birth_v3.rs)
- [community_cluster.rs — 단일 facts/plan·metadata 잔여](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/scv2_orchestration/generation_index/community_cluster.rs)
- [ordinal_working_set_v3.rs — 공유 SCC·membership](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/scv2_orchestration/generation_index/community_cluster/ordinal_working_set_v3.rs)
- [source_publication.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/source_bound_projection_assembly/authority_assembly/search_plane_handoff_dispatch/source_publication.rs)
- [constraints.rs — boxed LanguageCode](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract-base/src/query/constraints.rs)

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
