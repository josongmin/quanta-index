# W2-19 — 통합 테스트·CI 선택과 운영/성능/UX 수용

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-19)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R38,R39,R40; 기존 IMPLEMENTED 요구의 회귀 / NI-02,NI-03,NI-07,NI-08,NI-09. 설계 계약은 [RFC](../../../rfc/oct-10-native-incremental-vnext.md), 실행 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

[x] `DONE_IMPL` 재사용 경계: existing QBC/Justfile/CI·owner oracles·단일 VALIDATION. 이 표시의 범위는 명시한 하위 구현이며 티켓 전체 검증 완료를 뜻하지 않는다. 새로운 결함이 확인되지 않으면 재구현·재배정하지 않는다.

- [x] `DONE_IMPL`(issuer ownership): 기존 generic repository Original factory와 policy/deadline/error helper 8개를 `original_execution_keeper_v3/repository_issuer_v3.rs`로 이동했다. IFDS/public facade는 같은 정의를 re-export하고 writer genuine fixture가 canonical owner를 import한다. 기존 policy/deadline test 2개도 같은 child로 이동했다. 새 issuer·budget policy·production feature를 만들지 않았다. 해당 새 Runtime 테스트 실행 완료 표시는 아니다.
- [x] `DONE_IMPL`(test fixture mount): cfg-disabled writer `tests`의 기존 helper9개를 `original_native_test_support_v3/prepared_artifact_fixture_v3.rs`로 본문 동일하게 이동하고 기존 caller/re-export를 유지했다. 새 snapshot copy/issued drop·unwind5개를32c에서 mount하기 위한 정합 수리다. issuer/fixture를 다시 복사하거나 production feature를 넓히지 않는다. issued test constructor의 drop/unwind는 실제 writer abort/V9 proof를 대신하지 않는다.

## Scope

기존 requirement/oracle/Justfile/CI와 단일 결과 원장에서 코드·제품·운영·비용·default/release 수용을 구분한다.

Runtime Native owner tests must use a real Original issuer and the actual test
feature closure. The generic repository issuer and shared policy/deadline/error
helpers now live in the canonical Original owner; IFDS consumes those same
definitions. The writer fixture no longer imports a cfg-disabled IFDS module.
Preserve this ownership repair instead of repeating the move or widening
production features to make a test mount compile.
The same issuer also has production SDK indexing/mutation and public resume
facade consumers. Preserve their exact definition and refusal/budget semantics
when moving it; do not solve the writer test mount by forking an issuer or
changing the SDK producer in parallel with its existing owner.
The existing source-bound-replay-owner-test-support integration target compiles
normal Runtime lib and its owner cases. Its registered a3591548dd6a class
reaches index-sdk-ingress and both Native admission gates through search-runtime.
Use the actual required feature closure; a lib(test) name filter still compiles
the entire package's test graph. Historical32c/9c cfg/API failures and source
bindings belong to VALIDATION. Do not repeat the same failed closure with a new
filter or treat normal-library compilation as behavior success.
Native INSERT/JSON birth/exact-row tests are mounted in the existing integration
target. The independent response-owner regression has one body shared by
lib(test) and that target, using the same genuine Original fixture. This
placement preserves assertions and authority without a new profile or feature.
The same integration target now also mounts the canonical Store transfer-marker
transaction regression. It checks occupied digest/attempt, orphan Retired edge,
callback error preservation/rollback, actual marker-file creation and two
file-backed SQLite connections with zero busy timeout. It requires an observed
DatabaseBusy and callback0 while the first transaction is held, then successful
retry; thread scheduling alone cannot satisfy the oracle. Product writer/GC
interleaving and crash/restart qualification remain separate.
The latest Q impact packet passed41/41 across Contract/SDK libraries, covering
opaque receipt custody, before-send classification, control negatives, remote
message/repair retention and diagnostic frame sizing. This is scripted-UDS owner
proof. SDK-only strict still failed; neither local formatting nor later two
inline error-style repairs close that gate. Native direct Frozen/V9 and retry
code requires its own latest-source checks and direct coordinator fixture.
A production-only check or a selector that filters out the new tests cannot
close the genuine Original behavior requirement. Preserve unrelated fixture/API
compile failures until their owning cause is repaired.

INT가 먼저 닫을 검증 경계는 Q canonical borrowed wire/SDK/boxed LanguageCode,
S kernel proof/route와 W2-01/02/03/04의 genuine Original Runtime 회귀다.
실제 owner feature/fixture closure와 결합 source를 확인한 체크포인트에서 reuse/refusal,
Required7/seal, marker/foreign contract, Native recovery ordinary-read0,
late receipt/output custody, snapshot fixed digest/order/peak를 모아 실행한다.
이미 통과한 Quanta Runtime/Kernel57은 해당 UDS/effort/overlay/session scope의
증거이며 이 Semantica Original 실행을 대체하지 않는다. 실제 producer와 비용
수용은 W2-05/06/09 연결 뒤 판정하고 SEARCH/PHYSICAL/BOOT 구현을 재개발하지 않는다.

최신 결과는 VALIDATION의 기존 Native Required 체크포인트에서만 조회한다. 이전 session87365/sourcefcf8e279…와 session78928/source18432776…은 각 capture의 compile FAILED/tests0이며 현재 source의 오류 개수가 아니다. kernel unused import와 외부 Type output visibility는 후속 source에서 수리됐고, 최신 terminal/cluster/marker/cause 구현은 그 capture 이후다. 외부 compile 실패로 INT가 컴파일된다고 추론하거나 옛 진단을 중복 개발하지 않는다.

현재 체크포인트는 연결한 W2-02 Native record·Structural/Dirty·RepoMap output·초기 issued persistence와 W2-03 pin/policy/Degraded·3+owner 오류 수리의 실제 owner tests다. terminal immutable 저장/identity metadata·direct frozen/V9 metadata 연결과 pending/event/error 수명 잔여를 구분한다. Store의 Frozen/V9 owner 회귀는 아래20/20에 포함되며, 최신 direct coordinator/writer는 별도 미검증이다. terminal persisted4variant seed/predicate/output·exact marker restore와 Ready cluster의 borrowed facts/SCC·NativeProduced는 구현돼 있다. a35 후속 owner20/20 VERIFIED@4be3a12c를 기존 DTO 담당으로부터 소비했다. Native INSERT·semantic 거절·수명/환급과 새 marker 회귀를 포함한다. 과거 실패는 VALIDATION에서 보존하고 재개발하지 않는다. 직접 coordinator를 포함하는9c 검사는 source별 의존성 실패로 caller가 미검증이며 최신 판정/실행 순서는 SSOT·VALIDATION을 따른다. Contract3진단은 과거 source다. Kernel4variant의 독립 literal CBOR 파일과 ordinary/admitted 양성·tag 변조 거절 테스트는 exact 실행1/1 VERIFIED@ced20cc3…다. 그 뒤 immutable input hash 재사용·최종 canonical 비교 work·record 오류순서 수정은 별도 source다. Kernel 고정 fixture의 정상/forged prehash·비교 단계 work 거절·retained output retire는 exact1/1 VERIFIED@3142a3d5…다. Runtime hash pure fixture는32c의 ingress cfg(test)에 mount하고 기존 genuine Original fixture gate는 유지한다. 후속 Native exact retry named lookup은 Runtime79713c6c 이후 source이며 기존 outer-persist owner 회귀에 missing/foreign alias/present/corrupt/symlink/Original 거절을 결합했다. 실행 결과 없이 수용하지 않는다. 이 test adapter는 schema oracle이며 실제 Runtime Original backing/refund 증명과 분리한다. 기존 History wire fixture를 모든 variant의 증거로 승계하지 않는다.

Q public auxiliary SDK→scripted UDS12/12는 해당 source의 실제 SDK 결합 증거다. 이후 shared key/seed/hash 회귀103·2·4·1은 각각의 범위다. 최신 전체 SDK·S Original·실제 daemon/E2E·성능 완료는 아니다. root의 이번 enum 이름 해석/Required callback visibility 수리는 boundary pre/post18/18과 fmt/diff만 검증됐다. 전체 실행·strict/API/module 결과와 분리한다.

최신 Native product 단계는 Preparing/Prepared/Ready→게시/Committed→lease/Required dispatch→Terminal/checkpoint/ACK→staged final reader로 연결 저장됐다. Runtime lib(test)의 앞선 완료 결과는 FAILED@4832ea6c/82진단/테스트0이다. 최신 Runtime 요청 결과와 저장된 수리 후 미검증 상태는 아래 source별로 구분한다. R01 lang-common 실패는 넘어섰으며, 누락 import/crate 경로와 canonical typed API 이동을 반영하지 못한 기존 test 소비자를 분담 수리했다. Root/coordination/executor/SDK 파일 경계를 분리했고, 기존 Source root issuer·음성 조건·고정 wire/관측 대상은 유지한다. SDK taxonomy data의 오래된 literal3개만 현재 typed 표면에 맞췄고 guard 본문은 변경하지 않았다(Python13/13). 현재 source 경계24/24 통과는 Rust 동작 통과가 아니다. 후속 b10d6ca1은 의존성의 ordered-map birth public export 누락10진단/테스트0으로 중단됐다. 실제 caller가 사용하는 기존 두 타입의 facade만 동일 feature 교집합으로 연결했고5/5 pre/post 경계를 통과했다. 이 수리 뒤 후속9ac30bc6는 Cargo.lock 정합 오류로 테스트0/FAILED다. 현재 live lock의 dependency1 edge 수정은 저장됐지만 후속 검증은 NOT_RUN이다. CP mixed high-water/rollback 생산자는 전체 lib(test) 컴파일과 기존 RCU43/43 VERIFIED@90bd7795다. Runtime pin/최종 materialization·startup·최초 publication 변경이 모인 source에서 같은9c를 다시 사용하며 CP 결과를 Runtime 수용으로 확대하지 않는다. retention6·receipt-copy3·Runtime diagnostic/first-cause·later CAS/checkpoint·Required7·실 writer/crash oracle의 실행과 제품 수용을 구분한다. 모든 수용 조건은 아래 기존 게이트로 판정하며 새 테스트/profile/성능 원장을 추가하지 않는다.

## Acceptance

- persisted4variant의 ordinary/admitted 고정 wire, Native record/terminal/restart, cluster Ready/Degraded/오류 순서·membership/digest, 초기 artifact/marker no-clobber·foreign 무변경을 기존 owner fixture에 연결한다. 새 helper/API 존재만으로 통과 처리하지 않는다.
- 각 owner의 독립 oracle를 기존 Justfile/CI authority/selector에 연결한다. 기존 구조 게이트(예: `just rust-hexagonal`)와 scoped compile/strict/unit·genuine Original behavior를 먼저 판정한다. 구조 실패 상태를 구현 완료/통합 green으로 승격하지 않는다.
- A(pre-switch): actual producer FULL/delta/delete/recovery E2E→fault/pressure/ANN/headless API→W2-18의 격리된 새-root rebuild/reselection·rollback 리허설→Native Large/XL/matched total cost를 opt-in candidate에서 수용한다. W2-18 리허설의 개발·실행은 A 전에 가능하고, 실제 default 전환만 A 뒤다.
- W2-18 switch 뒤 B(post-switch): 실제 default-route smoke/rollback→coherent full suite→remote CI/release를 각각 판정한다. 미래 release를 A의 선행 조건으로 만들지 않는다.
- R01–R40/P2I01–08/UX01–12/AUTO01–10을 기존 RFC 요구와 독립 gold에 연결한다. conditional AUTO subcase와 외부 GUI/consumer 수용은 실제 범위를 명시한다.
- 테스트의 expected 값은 고정 gold·reference/model·독립 관측에서 얻는다. 구현식을 그대로 재계산한 결과는 독립 oracle로 인정하지 않는다. cancel/실패/timeout/capped/unknown 결과를 누락하거나 성공·성능 표본으로 집계하지 않는다.
- 새 QSDK/API의 fixed wire·foreign digest/receipt 거절과 actual admitted SDK UDS 결합, S proof substitution/body-only mutation send0·정상 route당1 send·부분실패/ACK-loss 복구, Fresh/recovered Original 수명 및 W2-03 actual producer 연결을 포함한다. 새 terminal wire/Box refund 테스트는 actual writer abort/V9와 구분해 실행한다. post-attempt typed 원인은 leaf→ordinary aggregate 변환→incremental quarantine→최종 API까지 보존되는지 검사한다. 테스트 존재/selector 매칭과 실제 실행 통과를 구분한다.
- Contract/SDK 공개 API와 모듈 기준선은 해당 owner의 공식 renderer로 변경 범위만 리뷰한다. 미연결 helper나 외부 dirty API를 통째로 baseline에 수용해 구조 gate를 닫지 않는다.
- 정식 제품/성능 수용은 서로 맞는 source·dependencies·config·input/corpus·binary·host 조건을 바인딩한다. quiet-host 성능 비교와 운영 부하 결과는 구분하며, routine focused 검증에 불필요한 qualification 증거 수집을 강제하지 않는다.
- legacy LQ5 운영·정식 성능/trace Off-Record 비교, 기존 semantic/test-platform/installed-pair/provider/release 의무는 동등 Native 수용과 명시적 퇴역 또는 해당 기존 증거가 나오기 전까지 유지한다.

통합 수용은 [SSOT의 병렬 장벽/증거 원 담당](../../../plans/oct-13-native-incremental-wave2/tickets/README.md#병렬-실행과-통합-장벽)을 따른다. 물리 fault·chunk membership·ANN·rank·headless·boot의 기존 owner oracle를 소비하고 통합 전용 복제 구현/테스트 체계를 추가하지 않는다. 실제 producer 미연결과 local fixture/Index-only 검증 가능 범위를 구분한다. W2-09가 동결한 단일 profile·결과 원장을 사용하며 미정 SLO나 누락 비용으로 qualification을 통과시키지 않는다.

## Requirement routing

| 원 요구 | 사양 연결 |
| --- | --- |
| R10 | W2-07,10,19 |
| R11–R12 | W2-01–04,07,13,19 |
| R13 | W2-09,19 |
| R15 | W2-11,19 |
| R16 | W2-09,19 |
| R17 | W2-10,19 |
| R21 | W2-12,13,19 |
| R22 | W2-13,19 |
| R24 | W2-14,19 |
| R25–R26 | W2-15,19 |
| R27 | W2-16,19 |
| R28 | W2-15,16,17,19 |
| R30,R32 | W2-17,19 |
| R35 | W2-08,17,18,19 |
| R36 | W2-08,18,19 |
| R37 | W2-01–03,05,06,19 |
| R38 | W2-05,06,19 |
| R39 | W2-07,09,10,13,19 |
| R40 | W2-15–19 |
| P2I01–P2I08 | W2-05,06,19; 수명·예산은 W2-01–03,07 |
| UX01–UX12 | W2-15–17,19; 실제 trace·matched cost는 W2-09 |
| AUTO01–AUTO10 | W2-17,19 |


이 표는 기존 요구 ID의 사양 연결이며 별도 작업 상태표가 아니다. R01–09/R14/R18–20/R23/R29/R31/R33–34의 기존 구현은 재구현하지 않고 W2-19 영향 회귀로 보존한다. 각 요구의 실행 결과는 VALIDATION만 소유한다.

## Source entry points

- [original_execution_keeper_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/orchestration/original_execution_keeper_v3.rs)
- [repository_issuer_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/orchestration/original_execution_keeper_v3/repository_issuer_v3.rs)
- [ifds_materialization_operation_v1.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/orchestration/ifds_materialization_operation_v1.rs)
- [original_native_test_support_v3.rs](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/retrieval/port_impls/index_projection_writer/original_native_test_support_v3.rs)
- 변경 파일은 기존 담당의 실제 dirty hunk와 [공유 파일 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md#shared-files)을 확인한 뒤 한 writer가 확정한다.

공용 parent/export/contract/config/registry 변경은 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 검사 미실행을 기능 부재로 바꾸지 않는다. 결과에는 source·명령·범위·VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE을 구분한다.
