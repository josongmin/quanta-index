> HISTORICAL — pre-migration document from `docs/plans/oct-17-index-closeout/tickets/WRITE-OWNERSHIP.md`. Current work: [Oct-22 SSOT](../../../../plans/oct-22-finalizing/README.md). Old TODO/status/counts do not create work or qualify current source.

# Oct-17 공용 파일·인계 규칙

상태·담당·다음 조건은 [SSOT](README.md)7행만 갱신한다. 이 문서는 고정 수정 경계다. 과거 child/채팅 배정·claim 번호·인계 chronology는 [보존 이력](../../../oct-17-index-closeout/tickets/WRITE-OWNERSHIP.md)이며 현재 수정권이나 실행 증거가 아니다.

## Exclusive boundaries

| 계약 경계 | 유일한 구현/소비 경계 | 재사용·금지 |
| --- | --- | --- |
| W2-05 A | Root의 Chunk plan private lane | 기존 query·declaration·TextBasis·canonical recipe/IR를 소비. 아래 공용 preplan/dispatch seam은 통합자만 수정 |
| W2-05 B | Root의 Chunk 문서 private lane | A와 plan/unit·owner 계약을 먼저 고정하고 paid producer를 구현. dispatch/parent/export는 통합자가 결합 |
| W2-05 C / W2-02 | Root의 내부 publication upper caller·pending/V9/ACK 통합 | P writer/generation/vector/root·I Parent/reconcile/receipt·M SearchPlan·W corpus writer를 소비. lower engine 재개발 금지 |
| W2-05 C의 writer bank | 기존 P의 stage/generation·FinalDocs/vector·FullSearchPlan/root 준비·commit/CAS bank | Root는 이 bank를 호출하는 상위 경로를 구현. P는 Root의 consumer/terminal이나 PUB SDK를 별도 구현하지 않음 |
| W2-05 D | Root의 Wire canonical raw physical/header custody | private bank와 기존 admitter 경계를 구현. 실제 caller DATA/finisher 및 필요한 Core seam은 원 owner와 직렬 결합 |
| W2-05 E | PUB의 Facts/recompute 실패 입력·SDK whole-error 변환·공개 게시/install/recovery | 기존 target clone owner 재사용. PTA/Product/Source/Store lower body는 해당 canonical owner, 내부 publisher는 Root. PUB는 반환 계약과 SDK parent를 한 번 결합 |
| W2-13 공용·query/directory | 기존 SEARCH의 Contract/Kernel/Runtime composition과 ANN 제품 경로 | 해당 shared parent는 exact hunk로 결합. 두 번째 directory/seal/query/worker 금지 |
| W2-13/09 physical | 기존 Catalog/PHYSICAL의 Common file/admission/FD·capacity/retirement | SEARCH가 물리 owner/ledger를 복제하지 않음 |
| W2-16 | Root 실제 Source 인계·PUB SDK composition·기존 AST/SQL/apply | 외부 source-owner caller·fresh command Original을 실제로 결합 |
| W2-18 | Root/PUB 실제 FULL evidence·기존 BOOT/launcher consumer | 새 target issuer/launcher 금지. default 전환은 A 뒤 |
| W2-09/19 | 원 oracle owner의 운영/영향 결과를 Root가 집계 | 같은 테스트/부하 실행을 다른 이름으로 복제하지 않음 |

## Collision resolution

### Parallel implementation schedule

아래는 기존 티켓의 수정 배치이며 새 상태표/작업 분모가 아니다. 현재 상태와 다음 완료 조건은 [SSOT 7행](README.md#remaining-work)만 갱신한다. private 구현을 분리하고 actual caller 연결과 무거운 검증은 아래 합류점에서 직렬화한다.

작업 채팅끼리 메시지·질문·상태 조회·추가 배정·새 채팅/하위 에이전트 생성을 하지 않는다. 각 담당은 독점 코드와 회귀 소스를 끝내 본인 채팅에 최종 결과만 남긴다. 공용 접합 요구는 정확한 함수/인수/owner로 그 결과에 포함하며 다른 채팅 응답을 기다리는 dependency로 만들지 않는다. Root가 현재 소스와 결과를 직접 읽어 공용 caller를 결합한다. 기존 PUB·SEARCH는 원 채팅을 유지해 같은 범위의 새 작업자를 만들지 않는다.

| 병렬 코드 lane | 독점 수정 파일/함수 | 시작 전 계약과 반환물 |
| --- | --- | --- |
| Root A — 전체 파일 plan | `consumer_v3/chunk_preplan_v3.rs`·`plan_v3.rs`와 필요한 private child | 현재 source의 single-ordinal port 반복과 실제 plan 전달을 재사용. 모든 대상 파일의 plan/unit·Original owner·순서·실패 owner를 유지하고 확인된 private 잔여만 수리. 공용 계약은 통합자가 결합 |
| Root B — Chunk 문서 | `native_chunk_v3/paid_projection_v3.rs`와 필요한 private producer child | A와 같은 plan-owned unit을 소비해 1→1/N→1/1→N의 paid Batch/정당한 Empty·whole failure를 반환. A의 실제 결과 없이 제품 완료 처리하지 않음 |
| Root D — Wire exterior custody | Wire `original_canonical_admission_v3/native_memory_v3.rs`·sibling `original_canonical_admission_v3.rs`·기존 owner tests | raw reserve 원인과 header partial을 보유할 exterior bank/borrow 계약을 먼저 고정. 선택된 실제 caller DATA와 highest finisher 결합은 통합 hunk로 인계 |
| 기존 PUB — E·공개 SDK | 기존 target/Facts/recompute private child와 공개 Source/초기 게시 경로 | 이미 있는 clone failure owner·prepare/after-Source commit은 재사용. SDK `indexing_api.rs`와 composition parent는 PUB 한 writer가 terminal→AST→BOOT 소비 순서로 결합 |
| 기존 SEARCH — W2-13 | 기존 Source/Text/ANN phase의 확인된 원인 수리 hunk | 기존 코드/64-owner 결과를 유지하고 원4K SourceAdvance의 원인과 필요한 수리만 닫음. Catalog/VFS/Common 본문은 PHYSICAL exact hunk를 보존 |

Root 통합자는 `consumer_v3.rs`·`projection_ports_v3.rs`·`executor/execution.rs`·`dispatch_port_impl.rs`·공용 parent/enum/export의 유일한 결합자다. A가 preplan의 단일 ordinal 서명을 변경하면 B의 dispatch에도 영향이 있으므로 private 결과를 합치기 전에 같은 변경에서 producer/consumer를 함께 자른다. 기존 부모/포트와 나란한 새 IR·임시 adapter·Ordinary fallback을 두지 않는다.

Root C의 reconciliation→manifest→게시 upper composition은 기존 bank로 준비할 수 있으나 실제 호출 연결은 A/B의 모든 writer 수락 뒤 한 writer가 수행한다. 이어 같은 artifact를 W2-02 terminal로 이동하고 PUB의 기존 SDK parent에 인계한다. AST/BOOT의 독립 consumer 준비는 기존 담당 범위에서만 진행하며 실제 Source/FULL 결과가 필요한 결합은 그 뒤다. PHYSICAL은 기존 운영 fixture/trace·입력 준비만 병렬로 하고 새 근거 없는 기능이나 두 번째 벤치를 만들지 않는다.

코드 checkpoint에는 각 lane의 exact hunk·actual caller·typed failure·Source/backing 수명과 고정 expected oracle를 제출한다. private 코드 존재는 합류 완료가 아니다. Runtime은 P의 기존 owning 큐, SDK/CP는 기존 PUB/PY 큐를 사용하며 같은 호스트의 heavy compile/제품/4K·운영 실행은 resource admission 아래 겹치지 않게 순서를 잡는다.

### Compiled handoff owner map

Semantica의 [compiled Index 인계](/Users/songmin/Documents/code-new/semantica-codegraph-v2/docs/handoff/oct-21/compiled-quanta-index.md)는 기존 결손의 근거/소비 관계다. 별도 실행 큐로 펼치지 않는다. 현재 작업 상태는 [SSOT](README.md#remaining-work), A/B/D의 독점 private 수정은 위 표를 따른다. 같은 함수·shared ABI·oracle에는 아래 한 writer만 둔다.

| 인계에 반복된 경계 | 유일한 코드 수정 경계 | 합류·다른 담당의 제외 범위 |
| --- | --- | --- |
| writer full-plan·parent retention·root/CAS | P: 기존 `index_projection_writer`의 bank와 owner 회귀. 예: `native_projection_root_prepare_v3.rs`·`structured_store/stage_commit_v3/native_v3.rs`·`search_plane_emission_plan/native_exterior_v144/full_plan_v164.rs` | Root: `consumer_v3/entry_handoff_v130.rs`·`native_manifest_handoff_v165.rs` 및 실제 upper caller에서 bank를 한 번 소비. P는 이 upper caller/terminal을 추가 작성하지 않고 PUB는 내부 writer/CAS 본문을 수정하지 않음 |
| PUB 실패 terminal·Facts 후반 실패·공개 Source commit 소비 | PUB: `mutation_api.rs::run_facts_source_task_v11`·`mutation_api/native_sdk_plan_id_capture_v56.rs::from_terminal_v166`·기존 공개 Source/초기 게시/SDK parent | 원 whole terminal/residual과 실제 lower 반환물을 소비. Root는 SDK parent를 수정하지 않고 C8은 PUB producer나 실패 bank를 중복 수리하지 않음 |
| PTA hydration·Product 실행·Facts carrier/seed의 lower 실패 | 기존 Semantica PTA/Product/Source canonical owner: `durable_transfer_v1.rs`·`committed_interprocedural_resume_attach_v1`·`product_v4_complete_reusable_execution_v1.rs`·`core_orchestration.rs`·`facts_original_v11.rs`의 기존 함수/children | PUB는 도달 consumer의 반환 계약을 결합. 해당 lower body를 Index 신규 작업자에게 재배정하거나 같은 내용을 Semantica와 Index 양쪽에서 별도로 수정하지 않음 |
| 초기 Source-index prepared commit 실패 | 기존 Store owner: `input_cell_store/contract_port_impl_v3.rs::try_commit_prepared_initial_original_v3`의 prepared/partial·funding·visibility 반환 | PUB는 backend `materialize_v3.rs`와 SDK failure owner에서 같은 반환물을 소비. prepared token 재생성·별도 CAS를 만들지 않음 |
| TSJS 공개 producer와 lifecycle | PUB는 공개 Source/실패 carrier를 소유. 기존 C8은 실제 TS/JS consumer·callback/receipt·Linux owning fixture를 소유 | imports/TypeWorld/QRA 의미는 기존 언어 owner를 소비. C8이 PUB producer를 수정하거나 PUB가 동일 Linux fixture/요청을 복제하지 않음 |
| terminal·AST·BOOT | Root는 actual artifact→기존 pending/V9/ACK, committed Source/FULL evidence 인계. PUB는 SDK parent. 기존 AST/BOOT는 자신의 queue/apply/launcher consumer | W2-02/16/18의 같은 결과를 재사용. stage receipt·target·ACK로 실제 publication을 합성하거나 새 consumer/launcher를 만들지 않음 |

A/B 공용 합류에서는 기존 plan unit의 merged candidate/source-members와 token-count callback의 정책/권한/export를 한 번 맞춘다. A의 plan producer와 B의 문서 producer가 같은 canonical 계약을 서로 다르게 고치는 것을 금지한다. 이 접합과 D의 실제 caller DATA/finisher는 Root 통합 hunk이며 private bank/producer를 다시 만드는 작업이 아니다.

실행 순서는 **기존 private/lower 코드 병렬 마감 → Root의 공용 plan/dispatch·D caller 합류 → 실제 writer 게시 → 같은 artifact의 terminal/SDK·AST·BOOT 소비 → 영향 owning·제품/복구 → 운영·통합 A → 전환/B·CI/release**다. PUB와 Semantica lower owner는 실제로 도달하는 ABI별로 결합하고, 전체 Semantica campaign 완료나 채팅 회신을 다른 독립 lane의 선행 조건으로 만들지 않는다. Runtime은 기존 P 요청, SDK/CP는 기존 PUB/PY 요청, TSJS Linux는 기존 `e271055a2cf5cb1c5c7cc5669289eaf5` 요청을 각각 한 번 소비한다. SEARCH의 원4K와 PHYSICAL 운영도 기존 단일 큐를 유지한다.

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

| 실제 수정 범위 | canonical packet | 공용 합류 |
| --- | --- | --- |
| Native preplan·plan/planner와 native_chunk 문서/dispatch | [W2-05 A](W2-05-producer-full-delta.md#native-chunk-plan)·[B](W2-05-producer-full-delta.md#native-chunk-dispatch) | Root가 concrete producer/consumer와 parent/export를 같은 변경에서 결합 |
| entry_handoff/native_manifest_handoff와 writer upper composition | [W2-05 C](W2-05-producer-full-delta.md#native-publication-caller) | P의 bank body를 보존하고 같은 writer serial·reconcile→manifest 순서로 소비 |
| canonical native_memory raw/header 경계 | [W2-05 D](W2-05-producer-full-delta.md#wire-canonical-custody) | Core 공용 seam은 필요한 exact hunk만 기존 allocator owner와 결합 |
| SDK composition·Facts/recompute·공개 초기 게시/설치 | [W2-05 E](W2-05-producer-full-delta.md#sdk-producer-custody) | PUB가 SDK parent를 소유하며 Root의 실제 artifact/whole failure만 소비 |
| 기존 Store/coordinator의 actual artifact→terminal | [W2-02](W2-02-pending-recovery.md#terminal-publication-join) | Root runtime과 PUB SDK를 함께 변경. signed SQL/decoder/CAS/ACK는 기존 구현 유지 |

현재 claim은 매 수정 직전 도구로 확인한다. live claim은 파일 전체 lock이 아니지만 conflicting hunk·invariant·proof를 임의로 인수할 수 없다. 기존 caller 통합 인계와 맞는 fresh preimage/dirty/symbol을 대조하고 필요한 reconcile을 수행한다. 후보 path 수나 apply-check는 source 적용/동작 수용이 아니다. M/I/W 완료 인계는 Root가 실제 caller에서 소비하며 archived/idle 상태를 새 구현 blocker로 만들지 않는다.

### Other shared boundaries

- source/member/vector·chunk/policy 의미는 원 producer가 한 번 생산하고 ANN은 같은 인증 입력의 검색 layout만 준비한다.
- directory writer와 reader는 같은 canonical codec/seal을 공유한다. logical refs는 실제 physical file/admission/FD owner와 결합하고 별도 file/GC ledger를 추가하지 않는다.
- Q Runtime/Contract/Kernel composition·SDK validator·Cargo/lock/parent/export는 실제 producer/consumer를 읽은 한 writer가 결합한다. 같은 파일의 private disjoint hunk도 formatter/export 영향을 확인한다.
- trace 추가는 source owner의 실제 phase hunk만 결합한다. 계측을 이유로 producer/JSON/SQL 알고리즘을 다시 작성하지 않는다.
- AST/BOOT는 원 Source/target/result를 소비한다. 후속 명령 권한, selection CAS·원 predecessor·rollback과 actual publisher 권한을 합치지 않는다.
- 원 owner가 고정 gold/회귀를 소유하고 W2-19는 같은 결과를 소비한다. 미지원/부분/거절을 빈 성공이나 가짜 receipt로 바꾸지 않는다.

## Handoff

인계에는 ticket·exact path/function/hunk·input/output authority·재사용 코드·독립 oracle·Source/failure/drop 순서·실행/미실행 범위를 포함한다. whole-file 후보/formatter로 foreign hunk를 덮지 않는다. 이 문서나 다른 채팅의 메시지는 새 배정·채팅 전송·소유권 인수 승인 자체가 아니다. 원 public 서명은 actual producer와 consumer를 함께 변경하고 definition-only API를 완료로 보지 않는다.

## Verification queue

Q는 Justfile/`./scripts/cargow`, S는 기존 owning QBC를 사용한다. Runtime owning은 P의 현재 제출/결과를 소비하고 SDK/CP는 기존 PUB/PY 큐의 독립 분모로 검증한다. Root가 같은 Runtime capture/compile을 복제하지 않는다. actual input/source/원인이 바뀌지 않은 RED를 filter/feature 변형으로 반복하거나 타 담당 프로세스를 종료하지 않는다. 정적·컴파일·본문·제품·운영·full-suite·CI/release는 [W2-19](W2-19-integration-acceptance.md#combined-checkpoints)에서 각각 판정한다.

## Historical section links

<a id="2026-10-20-native-dependency-and-funded-outer-join"></a>
과거 CP/Contract/PUB/Root 하위 lane 배정과 task/claim chronology는 [같은 이력 절](../../../oct-17-index-closeout/tickets/WRITE-OWNERSHIP.md#2026-10-20-native-dependency-and-funded-outer-join)에 보존했다. 현재 작업은 SSOT와 위 packet만 따른다.
