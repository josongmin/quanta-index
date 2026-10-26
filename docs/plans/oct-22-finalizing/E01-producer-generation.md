# E01 — 실제 producer·Chunk·RepoMap generation

상태/담당/다음 조건은 [SSOT E01](README.md#e01)만 갱신한다. 원 요구 R03/R04/R23/R24/R37/R38, 종전 W2-05/06/12/14를 소유한다. 게시·복구 수명은 [E02](E02-publication-recovery.md), ANN 검색은 [E04](E04-semantic-ann.md)의 결과를 소비한다.

## Reuse and exclusive writes

- A: 기존 `consumer_v3/chunk_preplan_v3.rs`·`plan_v3.rs`와 private child. 파일별 single-ordinal query·paid TextBasis·canonical recipe·plan 전달을 재사용.
- B: `native_chunk_v3/paid_projection_v3.rs`와 private child. 기존 direct/merge/split paid producer·digest/gold를 재사용.
- 기존 M/I/W·graph/RepoMap/corpus/inventory·Facts producer/seed는 해당 source owner가 한 번 생산한다.
- PUB는 공개 producer/후반 Facts 오류·SDK 인계를 소유한다. PTA/Product/Source/Store lower body는 기존 canonical owner, 공용 plan/member/token/export/dispatch는 Root다. [단일 owner map](OWNERSHIP.md#compiled-handoff-owner-map)을 따른다.

<a id="implementation-packet"></a>
<a id="native-chunk-plan"></a>
## Whole-file plan

현재 여러 파일 plan 코드를 다시 만들지 않는다. 모든 대상 upsert의 canonical identity/span·정책·순서·Original owner를 유지하고 삭제에는 HIR query를 강제하지 않는다. 성공 zero-output/all-delete·명시 unsupported·미구현/누락 거절을 구분한다.

<a id="native-query-closeout"></a>
## Native query → planner 실행 계약

이 절은 이 채팅의 Native Chunk 잔여를 기존 E01 안에서 자른 구현/수용 계약이다. 현재 판정은 [README E01](README.md#e01), 실행 증거는 [VALIDATION](VALIDATION.md)만 소유한다. 아래 절단은 새 에픽·진척 분모가 아니며, E01의 filesystem/RepoMap·공개 FULL/delta와 전체 [DoD](DOD.md#final-dod)를 면제하지 않는다. 수정 경계는 [독점 파일표](OWNERSHIP.md#native-query-closeout-writes), 실행 순서는 [다음 배치](WAVES.md#next-execution-batch)를 따른다.

### C0 — 병렬 구현 전 계약 확정

Root가 다음 다섯 항목을 현재 producer/caller/consumer에서 확정한다. K/H/T는 그동안 자기 파일의 기존 owner·회귀·고정 oracle를 준비할 수 있다. 공통 서명에 의존하는 수정은 계약 확정 뒤 시작한다.

1. 실제 SDK dispatch가 설치한 Original delegate와 `QueryOrchestrator`까지 추적한다. wrapper에 direct override가 없다는 사실만으로 실제 경로의 결함을 추론하지 않는다. Native FileText/HIR/ItemIndex 및 공통 경로를 쓰는 Symbol/ImportGraph의 `call_query` 아래까지 읽어 필요한 context field를 표로 고정한다.
2. 입력은 실제 `OriginalExecutionKeeperV3`의 session+rail, 별도 Source B의 snapshot/session/provider, canonical Original workspace `SourceSpec`·published binding·정책/lineage/framework·언어 payload다. 각 값의 발급자·paid retain·검증자·최종 drop owner를 기존 source에서 지정한다. path/default request만으로 Original workspace를 합성하지 않는다.
3. 기존 `RequestExecutionSessionV3::Original` slot과 admitted `prepare_original_runtime_base_v3`를 우선 비교한다. Native에서 기존 owner를 열거나 공유 operation을 추출하는 선택을 R-ENG-05에 따라 정한다. 다른 context surface가 필요하다면 의미·입력·거절 계약의 차이와 기존 owner 재사용을 먼저 증명한다. optional A·두 context issuer·ordinary diagnostic copy를 추가하지 않는다.
4. request slot, supplied keeper axes, paid request/base, semantic FULL provider가 함께 열리는 feature/의존성 집합을 확정한다. 과거 class8304에는 `original-execution-policy-digest-v3`가 없다. 현재 Python FULL Runtime cache의 mount는 `live-native-java-generation`과 `original-python-frontend-payload-v3`, Parser portable birth는 `parser-java`와 `semantic-module-python-min`을 요구한다. 실제 생산 경로에 맞는 closure를 고정하고 이전 policy ID를 새 실행에 그대로 적용하지 않는다. Cargo/lock/공용 export는 한 통합자가 처리한다.
5. K/H가 소비할 정확한 인수/반환/오류형, checked lender→alias retain의 실패 소유권, Source 종료/최종 drop 순서를 고정한다. 특히 현재 `IndexProjectionWriteError`에는 Runtime-private `OriginalPythonFullCustodyFailureV125`의 partial/Source/funding 운반형이 없다. H의 canonical 오류를 기존 Native query/exterior failure bank에 보유하고 `NativeChunkPreplanQueryFailureV3`·`NativePreparedProjectionFailureV3`까지 인계하는 계약을 정한다. generic RuntimeError 한 개로 평탄화하거나 contract-retrieval에서 Runtime에 역의존하지 않는다. full-policy guard의 미준비 field는 생산자별로 나누며, Native query가 그 값을 요구하는지 확인하기 전에는 admission guard를 열지 않는다.

### Native index context의 재사용 결정

이 계약은 전체 분석 context와 Native index query가 실제로 소비하는 field를 구분한다. Native 진입은 `FILE_TEXT`·`HIR`·`ITEM_INDEX`·`IMPORT_GRAPH` 네 query ID만 허용한다. 실제 Output A의 closed keeper/rail, pinned workspace Source B와 published authority binding, 해당 query의 실제 provider를 기존 owner에서 받는다. `ReopenScope::Global`은 이 pinned workspace 범위이며 analysis session resume를 뜻하지 않는다.

기존 canonical paid RequestSpec의 두 Box와 `prepare_original_runtime_base_v3`를 재사용한다. Native 전용 owned admission은 ordinary diagnostic copy나 `canonicalized_source_scope_v1` 복사를 거치지 않는다. 네 query의 실행자에서 필수로 읽지 않는 full-analysis projection/schema·IFDS resume guard를 전체 Original 진입에서 제거하지 않는다. `PlannerOptions`의 Open 저장값도 Python SourceClosed 분석의 증거로 쓰지 않는다. SourceClosed 정책이 필요한 실제 consumer에는 `ProductPythonBootstrapAuthorityV1`의 genuine 정책을 인계해야 하며 evidence census에서 추론하지 않는다.

Python은 기존 SDK 유료 evidence ref → Source B의 generic paid parts/handle → SDK capability → V61/V60 → `QueryOrchestrator`의 typed provider → 기존 Runtime FULL cache/birth → canonical `PortCommittedFullPortableHirV3`를 사용한다. Kernel의 Module/Full은 하나의 generic header retain/bind 알고리즘을 공유한다. 초기와 successor 조립은 기존 `prepare_repo_state_assembly_v166`를 공유한다. reader getter나 구조체 필드의 존재만으로 이 경로의 실행 성공을 판정하지 않는다.

### 구현 절단과 수용 기준

| 절단 / 우선순위 | 실제 작업·입출력 | 완료 기준 / 거절 조건 |
| --- | --- | --- |
| K / P0 — Output keeper 운반 | `run_chunk_preplan_v3`의 각 upsert와 accepted projection에서 실제 closed keeper를 Native query/prepare port·query birth·retained sources까지 운반한다. 기존 `try_retain_same_original_v3`와 `retain_native_session_v123`의 checked lender/current·partial failure bank를 재사용한다. 입력 A의 rail을 버린 뒤 동일 정책 숫자로 재구성하지 않는다 | FileText/Chunk/Symbol/ImportGraph가 같은 물리 A/rail을 소비. Source B와 구별, query outputs/partial이 owner보다 먼저 종료. retain 실패의 원 Current/outer/inner 원인 보존. Delete/MarkStale의 no-HIR-query와 기존 writer acceptance 유지 |
| C1 / P0 — born-under-A Original context | 실제 A를 context birth 전에 canonical request/axes owner에 공급. 정식 Original SourceSpec·request/base·trace·paid metadata를 생산하고 마지막 Source B bind 뒤 기존 text/semantic provider를 보존/설치. `runtime_ctx_for_native_snapshot_v3`는 결과를 재인증해 소비 | **실제 context factory 양성**에서 A/session/rail·Source B/provider·정책/binding/digest가 일치. missing/foreign/stale/policy/binding/cancel은 읽기/쓰기 전 typed 거절. birth 후 session 교체·무제한 정책 setter·Ordinary fallback 금지. helper transport 양성만으로 C1을 닫지 않음 |
| C1 생산자 하위 절단 / P0 | 기존 paid axes producer와 authority input owner가 snapshot/environment/language/framework raw→presence 검증→lineage/published binding의 실제 산출물을 인계한다. `refuse_unprepared_language_framework_and_consumers_v3`의 final binding·lineage·resume·port consumers 및 Kotlin common mode는 각각 실제 producer/consumer로 대조 | 이미 구현된 paid 복사/검증을 다시 만들지 않음. 필요한 field의 최종 DATA 이전/검증/실패 보유를 완료한 admission 범위만 허용. 빈 payload/default·guard 삭제로 통과 금지. 기존 PUB/lower owner와 같은 hunk는 중복 배정하지 않음 |
| H / P0 — Python committed FULL HIR | canonical Python frontend/member loan에서 기존 `OriginalPythonPortableHirBirthV111`·Runtime Python FULL birth/finisher를 재사용해 **동일 `PortCommittedFullPortableHirV3`**와 whole failure를 Native에 공급. typed imported-provider 운반과 기존 paid Python kernel handle 확장 중 C0에서 선택한 하나만 구현 | Java issuer가 없어도 genuine Python FULL→HirQuery 양성. 언어/member/path/foreign Current와 늦은 Source·output funding 실패 각각에서 exact typed cause/partial·최종 환급 확인. `OriginalPythonFullCustodyFailureV125`·first/secondary cause를 기존 bank에 끝까지 보유. 새 HIR/캐시/serializer/issuer, Runtime→SDK-backend 역의존, ordinary parse fallback 금지 |
| T / P0 — 실제 다중 파일 회귀 | 실제 Source pin/Parser FULL/Runtime query를 사용하는 최소 2 upsert+1 delete fixture. 기존 merge/split/single gold에 대응하는 실제 FULL HIR/corpus 입력을 C0에서 매핑하고 기대값을 실행 전에 고정. 역순 입력·path/span/text/ID/unit 순서·기존 canonical digest로 all-file aggregate→planner 검증 | 각 upsert의 FileText/HIR/ItemIndex 실제 실행 횟수 확인, delete query0. 두 번째 파일의 늦은 실패·foreign/missing/stale owner·policy/binding mismatch·cancel에서 planner 결과 없음, 앞선 partial/전체 오류를 두 번 관측해 정체성 보존, 최종 예산0. query output·opaque success/failure를 fixture에서 제조하지 않음 |
| I / P0 — 현행 source 합류·owning 검증 | K/C1/H/T와 필요한 기존 producer 산출물을 한 의존성/feature 집합에 결합. 정적 경계·test honesty·scoped safety 후 실제 owning QBC selector 실행 | compile/link와 실제 본문 수·실패·ignored·owner exit를 따로 확인. source가 바뀌면 영향 결과만 재검증. A17과 Source-binding2의 과거 서로 다른 Source 결과를 합산하거나 현행 전체 Runtime GREEN으로 승격하지 않음 |
| D / P1 — 실제 writer·게시 인계 | genuine plan을 기존 paid Chunk producer의 1→1/N→1/1→N에 전달. 실제 Native receipt→writer replay/full-plan/root serial을 수용하고 같은 결과를 기존 E02/공개 producer가 소비 | provider 출력 shape·stub receipt가 아닌 실제 writer artifact/receipt와 cold-target gold를 대조. E02 actual DELTA authority/member·V9/ACK·terminal/복구, FULL/delta/P2I는 각 기존 owner/gate로 이어짐. T/I 통과만으로 D 또는 E01 전체를 닫지 않음 |

### 남은 작업 packet — 기존 절단의 하위 작업

아래 packet은 C0/K/C1/H/T/I/D의 실제 연결점과 수용 조건을 더 잘게 자른 것이다. 새 진척 분모나 별도 상태표가 아니다. 파일별 유일 writer는 [OWNERSHIP](OWNERSHIP.md#native-query-closeout-writes), 현재 판정은 [README E01](README.md#e01)을 따른다.

| 기존 절단 / 작업 | 실제 수정·검증 대상 | 선행 입력 / 완료 조건 |
| --- | --- | --- |
| I — 공용 feature·API·fixture 접합 | Runtime `original-python-frontend-payload-v3`에서 Kernel `original-python-semantic-module-provider-v3`까지 실제 dependency edge, public exports와 세 kernel mirror, mock의 명시 Original context retain, `real_multifile_query_tests_v3` parent mount, production cohort의 등록 compile policy | 기존 paid request/context/provider 소스를 재사용. 현재 production feature closure를 resolve하며 Python-min에 Java/FULL 전체 기능을 무조건 확장하지 않음. boundary 검사와 test-harness 컴파일·실제 선택 본문을 구분 |
| C1/K — 원 context 오류·partial 보유(F) | `runtime_ctx_for_native_snapshot_v3`의 즉시 SDK RuntimeError 변환을 기존 Native birth/exterior bank의 원 `OrchestratorError` 보유로 교체. File/Chunk/Symbol/Graph와 `NativePreparedProjectionFailureV3` 관측자까지 전달 | Root가 공용 failure slot/API를 먼저 고정. 원 request birth·partial·first/secondary cause·funding을 두 번 관측해 보존하고 payload-before-keeper 및 마지막 환급을 확인. contract-retrieval에 Runtime 역의존을 추가하지 않음 |
| C1 — SDK 실제 context 생산 경로 | 실제 `Arc<SdkOrchestrator>`의 `QueryContextPort`에서 설치된 Original delegate와 typed Python provider를 전달. Facts target의 ingress Arc를 rail이 캡처하기 전에 기존 V62 bank로 유료 text provider/delegate를 설치하며, genuine target evidence를 같은 composition FULL bind owner에 재결속 | 같은 sealed provider의 단일 birth·동일 Source/Output·설치 성공 후 오류의 delegate/provider 보유. Java 분석 envelope 경계는 유지. effective feature closure를 확인하며 직접 Cargo edge 부재를 기능 부재로 추론하지 않음 |
| C1 — request 출생 후 admission 실패 | 기존 Request/Product/Ingress/Hydrated/Resolved/Admitted owner의 canonical 연산을 into-slot 또는 borrowed preflight로 확장. 단일 exterior bank가 실제 stage 입력과 typed cause를 `OrchestratorError`→F observer까지 보유 | 각 fallible 경계의 양성·거절·반복 관측·마지막 환급. 입력을 소비한 뒤 `to_string`으로 원인/유료 DATA를 버리지 않음. 새 request IR·policy·ordinary admission 복제 금지. context-base/port 설치/trace 이후 실패도 같은 소유권 경계로 대조 |
| C1 — 일반 Product Original 후속 경계 | SDK ImportGraph가 도달하는 `build_executable_request_from_admitted_with_projection_hydrator_v1`의 enrichment/port/budget/trace/materialization에도 같은 admitted/ctx/partial bank를 유지하고 SDK 오류 lowering에서 보유를 끊지 않음 | Native fixed-four entry의 수리와 별도 수용. 기존 `PaidFiniteSourceFailureReadyV3`·`RetainedRuntimeErrorV4`를 후보로 사용하되 actual Original session의 Source Current에서 producer 이전 node prebirth/retain, 같은 Current의 canonical paid display와 실제 거절 원인, typed stage cause 관측 계약을 유지. canonical SDK 오류 owner에 whole bank를 운반하며 무단 사후 node 발행·새 contract 역의존·selected-cause-only carrier로 대체 금지 |
| H/K — Python Native Symbol 입력(S) | `native_symbol_v3::query_native_symbol_output_v3`의 bare ItemIndex 입력을 canonical committed FULL로 준비. Symbol과 Chunk는 같은 `native_v3` module에 include되므로 기존 FULL 준비 helper를 우선 직접 재사용하며 실제 반례가 있을 때만 최소 추출 | genuine Python provider/Source B/Output A와 F slot 계약. FileText의 인증된 language/member·path를 사용. Python 양성·foreign/missing/wrong-member/funding 실패·Java 회귀 및 최종 환급. 두 번째 FULL producer/cache를 만들지 않음 |
| C1 — ImportGraph 실제 본문(G) | 기존 Original traversal은 forward/reverse·depth cap과 별도 reachable-cycle walk를 이미 구현한다. 선행 `import_graph_filetext_closure.rs::collect_original_closure_reuse_digest_v3`의 FileText 하위 호출이 borrowed `call_query`를 사용해 Original ordinary-copy 거절에 도달한다. 이 실제 호출을 같은 A/B·outer context·receipt와 실패 DATA를 보존하는 canonical owned subquery 경계로 연결 | 고정 forward/reverse/depth1 파일·edge와 모든 경우의 cycle[a,b], actual FileText closure digest/alias 검증·마지막 환급을 유지. 기존 traversal/reader를 다시 만들거나 guard만 바꾸지 않는다. source-static 결손과 R41 실제 SDK Graph 결과를 구분하며 실행 전에는 양성 성공을 주장하지 않음 |
| H/T — 유료 provider 생산·회귀 | SDK paid Ref header→handle→초기/successor 조립→V61/V60→Runtime FULL cache 실제 witness. 기존 Kernel Module도 같은 generic header 알고리즘의 영향 회귀 실행 | source-only field/reader test와 구분. genuine paid Ref 양성, foreign Current/group/member·late Source·header/retain/funding 거절의 typed partial·최종 환급. generic read가 추가한 work/authentication의 tight-budget 영향 확인 |
| T — 실제 모든 파일 query·planner 양성 | 기존 genuine 2 upsert+1 delete corpus에서 FileText/HIR/ItemIndex를 실행해 all-file aggregate→canonical planner로 전달. 1→1/N→1/1→N의 고정 ID/span/text/order/digest gold | 두 파일 FileText helper와 Source/ordinal test를 재사용하되 이를 HIR/ItemIndex/planner 성공으로 세지 않음. upsert별 실제 query count, delete query0, 역순 입력과 세 topology의 실제 산출물 대조 |
| T — 실패·취소 수용 | 두 번째 파일 late failure, missing/foreign/stale owner, policy/binding 불일치, cancel/unwind, output/Source finisher 거절 | 앞선 genuine partial과 원인 bank를 반복 관측. 결과 planner/write0, 정확한 typed 원인, 마지막 drop 뒤 전체 ledger0. fabricated opaque Output/receipt 금지 |
| I — current cohort owning 실행 | Runtime Chunk/source-binding, SDK composition/Executor, Kernel Module, Symbol/ImportGraph 영향 owner를 각각 resolve·실행 | selector마다 별도 요청·실제 본문 분모·compile/link/owner exit 기록. 과거 A17/Source-binding2를 합치거나 새 source의 GREEN으로 사용하지 않음. 같은 host heavy 검증 한 큐 |
| D — 실제 paid producer·writer 수용 | 이미 연결된 dispatch→`project_paid_chunk_v3`→writer accept→accepted receipt를 그대로 사용. replay/full-plan/root serial과 cold-target artifact gold 실행 | T/I 결과와 실제 세 topology. 기존 B/P producer·bank 재작성 금지. Chunk module의 컴파일 포함을 writer 본문 실행으로 세지 않음 |
| D/E02 — genuine DELTA 입력 | `prepare_sdk_handoff_v3`의 실제 source/target과 sealed History/Lexical/RepoMap·source-closure witness를 `NativeReplayMetadataStageV159` 이후 기존 aggregate issuer에 운반 | source/target·member 발급은 기존 PUB/lower owner, 상위 carrier/접합은 Root. FULL 재수집·빈 member·새 issuer·합성 target으로 대체하지 않음 |
| D/E02 — 준비 authority의 실제 소비 | `take_prepared_delta_initial_authority_v3`의 실제 caller를 연결해 `NativeInitialPendingV3`에서 기존 pending→V9→terminal ACK로 이동 | genuine DELTA 입력 뒤 같은 operation/target·5 durable cut·ACK/refusal/cancel/최종 환급 수용. metadata pending 또는 helper 수락을 공개 게시 완료로 쓰지 않음 |
| E01 원 DoD — 나머지 기능 수용 | filesystem child-first/path/frontier CAS·inventory pagination·RepoMap parity, complete affected generation/required side output, FULL/delta와 P2I01–08 | 기존 canonical owner/요구 R03/R04/R23/R24/R37/R38의 실제 결과를 한 번 소비. Native Chunk 하위 packet 완료로 이 분모를 줄이지 않음 |

중복 배정 제외: `runtime_cache.rs::execute_item_index_query_v3`의 현재 유일 caller는 ordinary materialization reader다. 선행 `ordinary_materialization_query_context_v3`가 Original을 cache flight 생성 전에 명시 거절하고 ordinary role을 검사한다. 이 경로를 Native Symbol 결손과 같은 미구현으로 배정하지 않는다. 새 caller가 생기면 reachability를 다시 확인한다.

### 기존 owner 재사용 지점

경로 기준은 Semantica `packages/analysis/quanta-v2/crates/`다. C0 착수 시 실제 모듈 mount와 변경 hunk를 다시 확인한다.

- K: `quanta-runtime/src/orchestration/original_execution_keeper_v3.rs`의 closed pair/paid retain, 기존 `OriginalNativeMemoryScopeV1`, `consumer_v3/accepted_projection_v114.rs`의 checked alias failure bank. 새 keeper/root/메모리 scope를 만들지 않는다.
- C1: `quanta-runtime/src/orchestration/orchestrator/context_builder/original_context_entry_v3.rs`, `build_context_shared/original_request_execution_v3.rs`, `request_spec/request_execution_session_v3.rs`. `orchestration_impl/port_impls.rs::bind_direct_runtime_source_v3`의 기존 B/provider 결속은 재사용하며 Source binder의 provider invalidation 규칙은 유지한다.
- H: `quanta-adapters-parser/src/semantic_attribution_java_workspace_production_owner_v1/original_python_owner_seal_v3/portable_hir_v111.rs`; Runtime `orchestration/analysis_execution_input_factory_v1/original_incremental_seed_v3/full_hir_v3/python_v111.rs`; kernel `ports/original_python_semantic_module_provider_v3.rs`; SDK-backend `normal_source_attribution_generation_evidence_v1/shared_evidence_ref_v150/python_semantic_read_v3.rs`. Runtime의 기존 `OriginalCommittedFullCacheV3`와 post-Source finisher를 유지한다. kernel에 Parser 타입 의존을 역으로 밀어 넣지 않는다.
- T: 기존 `chunk_preplan_v3/{tests_v3,product_tests_v3,query_path_tests_v3,source_query_tests_v3,recipe_tests_v3}.rs`의 oracle/실패 본문은 보존한다. 거부 provider까지1회 도달한 기존 source test는 성공한 checked query의 대체물이 아니다.

### 검증 인계

I는 QBC의 `owner resolve`로 **통합 source의 실제 feature/compile policy**를 먼저 확인한다. 그 뒤 동일 통합 source/feature cohort의 정식 snapshot·lease에서 **selector마다 별도 owning 요청/receipt**로 실행하고 본문 분모도 따로 기록한다. host의 heavy build/제품 부하는 직렬화한다. raw Cargo 결과나 class8304의 고정1079 cohort는 새 producer 경로의 자격 증명이 아니다.

```text
package: quanta-runtime; execution-kind: test; target-kind: lib
selector-mode: module; max-test-threads: 1
selector 1: coordination::change_event_recompute_invalidator::incremental_rail::native_delta_v3::consumer_v3::chunk_preplan_v3
selector 2: orchestration::orchestrator::orchestration_impl::port_impls::source_binding_tests_v3
compile-policy: resolve the integrated production feature closure; do not guess a policy ID
```

기존 17본문 및 2본문의 기능/음성 단언을 유지하되 새 회귀가 추가되면 실제 선택된 분모를 기록한다. receipt/writer replay/full-plan/root serial의 selector는 owning 소스에서 확인 후 별도로 resolve한다. selector 1의 compile에 포함됐다는 이유로 그 본문이 실행된 것으로 세지 않는다. 필수 입력/기능 집합이 없으면 해당 실행은 BLOCKED, 미실행은 NOT_RUN, 실제 compile/test 실패는 FAILED로 남기고 원 절단만 수리한다.

<a id="native-chunk-dispatch"></a>
기존 retained-cause node를 덧씌우는 것만으로 Java FULL·generic request 실패를 닫지 않는다. Java의 canonical terminal prebirth는 group/operation/node/retain/display 단계에서 거절될 수 있고, 미완 partial은 local birth에 남는다. 현 provider의 `Result<Arc<FULL>, RuntimeError>`에는 이를 운반할 공간이 없다. 기존 prebirth whole-failure 또는 provider in-slot 계약을 실제 caller까지 확장해야 한다. request builder는 동일 canonical 생성 알고리즘에 caller-owned request birth와 supplied family workarea를 받아 두 Source round의 미완 partial을 실제 Native bank에 보유해야 한다. FileText/HIR/ItemIndex/ImportGraph는 기존 family copy owner를 사용하고, PTA·Taint·CallGraph의 기존 paid terminal override는 유지한다. 새 슬롯의 정적 존재는 실제 거절/환급 증거가 아니다. 후속 절단은 actual caller bank→request/prebirth/family partial→worker/receipt의 소유권 계약이며, semantic pre-key hook7sync/3async를 우회하거나 Query262개에 불필요한 필수 associated type을 추가하지 않는다. 현재 실제 consuming override는 동기 FileText·FileTextBatch·FileMeta·Parse·ImportGraph·ItemIndex·Callsite7개와 비동기 SemanticModule·FunctionSummary·PrecisePTA3개다. `query_ops::prepare_exact_recursive_query_input_v3`에 진입하기 전 ctx/Args를 exterior slot에서 take하므로 hook Err·await 취소·unwind는 request/dispatch birth로 보존되지 않는다. 기존 hook을 호출하는 기본 adapter만 추가해서는 닫히지 않는다. 각 canonical producer의 오류/partial in-slot 전환과 비동기 cancellation-safe bank를 함께 연결해야 하며 기존 semantic hook 의미를 유지한다.

supplied Native family workarea의 실제 dispatch copy 계약은 `query_ops.rs` → `OriginalQueryRequestBirthV3::try_copy_request_for_dispatch_with_workarea_into_birth_v3` → `ErasedExecutionRequestV1::try_copy_with_runtime_ctx_and_workarea_into_slots_v3` (`query_executor_port/request_input_storage_v3.rs`)다. 기존 Ordinary/None의 `try_copy_with_runtime_ctx_v3` 경계는 별도로 유지한다. 원 request/ctx 외부 보유와 별개로, Foundation `typed_input_v3.rs`의 callback-local `TypedInputRetainWorkareaV3`·`OriginalTypedQueryInputRetainBirthV3<T>`는 shared/pending/box/payload/leases/funding/typed cause를 보유한다. request recovery만 밖으로 옮기면 이 bank의 미완 DATA가 계속 파기된다. 같은 Foundation owner의 concrete retain+validation whole bank를 caller-owned custody로 넘기는 checked ABI와 Kernel actual copy caller를 함께 유지해야 한다. 실패 시 문자열/원인만 운반하거나 새 erased IR/issuer·사후 carrier 할당으로 대체하지 않는다.

Pre-key 후속의 실제 생산 호출자는 Kernel `query_ops`, Runtime `orchestrator/preflight`·`canonical_existing_query/pre_key_input_v3`, Analysis Callsite의 nested Parse다. SDK 직접 hook 호출은 기존 회귀 fixtures로 함께 갱신한다. 두 기존 Query hook 자체를 caller-owned ctx/Args 슬롯으로 전환하는 것이 공통 절단이며, consuming override를 우회하는 별도 hook이나 성공 wrapper는 만들지 않는다.

- FileText·Meta·Parse: canonical FileText selected-path into-birth를 재사용하고 성공에만 Args를 교체. Batch: 기존 rows birth와 현재 member의 FileText selection birth를 함께 외부화.
- Graph: 기존 `OriginalImportGraphArgsBirthV3`와 Source finisher를 exterior slot에서 소비. 별도 reader·issuer를 만들지 않음.
- ItemIndex: 기존 FULL binder into-slots는 재사용하되 selected-source·Java issuer의 FULL 이전 partial도 먼저 보존. binder만 이동해서 완료 처리하지 않음.
- Callsite: outer Args와 nested Parse 입력을 유지한 채 같은 canonical Parse hook을 한 번 호출.
- SemanticModule: 기존 path birth와 build projection을 먼저 exterior에 보유하고 성공에만 Args로 반환.
- FunctionSummary·PrecisePTA: 같은 Precise 준비 producer를 재사용. QueryExecutor port→owner forwarding→Runtime adapter→native cohort의 context/request/paid digest를 await 이전부터 exterior bank로 연결해야 취소가 보존됨.

기존 FileText/HIR/ItemIndex/Graph request validation phase는 한 번 사용한 뒤 재시도를 거절한다. dispatch는 같은 exterior workarea 컨테이너 안에 별도 fresh validation phase와 canonical typed-retain birth를 보유해야 하며, 소비된 request validator를 재사용하지 않는다. 원 RuntimeError는 기존 Native failure의 primary로 한 번 이동하고 같은 observer가 primary와 partial DATA를 함께 대여한다. nested bank에 같은 오류를 복제하거나 추가 paid cause-node를 발급하는 요구는 없다. stored typed owner와 exterior bank의 checked 연결은 실제 consuming ABI까지 구현·검증해야 하며, callback 선언만으로 완료 처리하지 않는다.

## Documents and dispatch

1→1·adjacent-import N→1·large-body 1→N을 같은 plan-owned unit으로 문서화해 기존 writer가 실제 수락해야 한다. merged candidate/source-members와 기존 token-count 정책/권한/export를 함께 맞춘다. 단순 match arm, Ordinary copy 후 Native 승격, 빈 FULL 대체는 수용이 아니다.

<a id="native-destructive-jobs"></a>
## Destructive jobs — existing tombstone contract

Native의 Delete/MarkStale도 기존 `IndexProjectionTombstoneBatchV1`의 reason·doc ID·epoch·backend/family·receipt 의미를 유지한다. `AuthoritativeEmptyProjectionScopeV1`은 provider의 zero-row 결과이며 삭제 사유와 GraphEdge 제거 범위가 다르므로 대체 IR로 사용하지 않는다. 현재 확인/미구현 상태는 [E01 행](README.md#e01)과 [검증 요약](VALIDATION.md)에만 기록한다.

| 절단 | 구현/합류 계약 |
| --- | --- |
| Actual Native producer | 실제 bound job의 operation과 matching dirty-unit destructive reason을 기존 selected-job read에서 확인. Delete/MarkStale을 doc_type upsert dispatch보다 먼저 분기하고 기존 canonical tombstone ID/reason/backend 정책과 Native field-copy owner를 재사용. HIR upsert query·일반 writer fallback·수제 zero-row receipt 금지 |
| Existing carrier/accept | 기존 `NativePreparedIndexProjectionV3`가 paid tombstone을 이동하고 기존 Native writer acceptance가 별도 destructive operation 검증을 수행. batch/empty의 Upsert/Repair-only validator를 잘못 재사용하지 않음. producer와 consumer의 공용 서명/enum은 Root 한 writer가 함께 변경 |
| Native stage/receipt | 기존 COW stage·supersession·serial·finisher·receipt owner를 tombstone 입력으로 확장. logical tombstone validation, GraphEdge replacement-family 제거, 실제 shard digest/doc_count는 기존 canonical ordinary 정책과 일치. 빈 provider 결과 정책을 복제하거나 live backend를 선행 변경하지 않음 |
| Downstream | 실제 발급 receipt→동일 pending/manifest→mutation snapshot→lexical/semantic delete→기존 publication/terminal을 소비. destructive reason은 frozen job에서 최종 복구까지 보존 |

Vector 삭제의 ordinary producer는 empty payload를 stage한 뒤 structured 함수를 호출하지만, 하위 함수는 Vector에서 즉시 반환하므로 **Vector structured mutation을 생성하지 않는다.** Native도 단일 retained/COW vector payload store에 수락하고 기존 structured whitelist를 넓히지 않는다. structured Vector tombstone을 추가하면 shard kind를 구분하지 않는 lexical planner가 lexical 삭제까지 생산할 수 있다. actual reason/full receipt를 인증하고 같은 serial에서 paid candidate 준비→highest Source 수락→checked apply를 수행한다. ordinary/Native reader·artifact hydrate·discard/cleanup/rollback을 같은 store에 연결한다. 명시적으로 영속화된 ordinary empty records는 유지하고 누락 row는 거절한다. `doc_count=0`·prefix·payload 미존재로 삭제를 추론하거나 marker ledger·V2 schema를 추가하지 않는다. 정상 파일 삭제의 Manifest/SearchPlane tombstone은 기존 lexical/semantic planner가 소비한다. Vector-only direct 경로의 로컬 vector 삭제를 외부 SearchPlane semantic 삭제로 확대해 주장하지 않는다. exact 수정 경계는 [Vector cutover](OWNERSHIP.md#vector-stage-cutover)를 따른다.

수용은 실제 paid File/Symbol producer→Native stage에서 생성된 TombstoneScope를 사용한다. same-path 서로 다른 semantic typed tombstone을 lexical path-preferred tombstone으로 합치지 않는다. Delete/MarkStale·rename/recreate·missing/foreign reason·stale epoch/backend·GraphEdge family·before-write refusal·cancel/late Source/final refund를 기존 owner gold로 확인한다. field-funded typed-IR 정책 테스트는 이 actual producer 경로의 성공 증거가 아니다.

<a id="sdk-producer-custody"></a>
## Public producer handoff

실제 complete affected-domain generation·durable outbox·required side output을 공개 FULL/delta caller로 전달한다. Facts 후반 recompute/seed 부착/seed 누락에서도 원 terminal/입력/부분 결과·first/secondary cause·keeper를 유지한다. lower에서 버린 입력을 wrapper·문자열·재수집으로 복구하지 않는다.

## DoD

- [ ] 실제 SDK producer→모든 writer 수락→E02 게시→public query의 FULL/delta 결과가 independent cold-target gold와 일치.
- [ ] edit/delete/rename/recreate/no-op/Resync·2→1→0→1 support·변경 없는 dependent까지 complete affected closure.
- [ ] 여러 파일/언어·세 topology의 고정 ID/span/text/순서·missing/duplicate/foreign/역순 회귀와 실제 executor 양성.
- [ ] Native filesystem capture·child-first batches·path/frontier CAS·pagination과 RepoMap provenance/ranking parity.
- [ ] [P2I01–08](../../rfc/oct-10-native-incremental-vnext.md#214-gates-attached-to-existing-work-not-a-second-execution-plan)의 실제 producer/consumer·required side output·partial refusal·old-view/recovery oracle를 한 번 실행/소비.
- [ ] 실패/cancel/unwind의 whole owner와 Source 종료/마지막 drop 환급. 생성한 실제 artifact를 E02/E05/E06에 한 번 인계.

고정 세부 계약은 [이전 producer 사양](../../history/oct-22-finalizing/oct-17-index-closeout/tickets/W2-05-producer-full-delta.md)에 보존한다. 과거 TODO/상태는 재작업 지시가 아니다.
