# Native 전환 레인별 작업 계획

2026-11-21 KST. [01–08 티켓](README.md)의 착수·합류 순서다. 작업 직전 현재 HEAD·대상 작업 트리와 기존 오너를 다시 확인한다. Q 실행기의 현행 PID·용량·terminal은 [01](01-q-regressions.md)과 [Root 큐 원장](../../nov-12-native-cutover/tickets/NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16)에서 확인한다. 각 체크박스의 마감 담당과 결과는 [티켓별 종료 표](#티켓별-종료-표)에 있다.

## 현행 코드 착수 순서 · 11월 22일

Q `d8a490565e6f`의 작업 트리와 S `48afeb27a96f` 이후의 작업 트리를 기준으로 대조했다. 실제 쓰기는 아래 기존 오너가 맡고, Root/F는 공용 등록·동결·증거 인수를 맡는다. 착수 직전 같은 함수의 진행 중 패치와 같은 selector의 기존 요청을 확인한다.

| 우선순위와 기존 오너 | 실제 수정 또는 합류 | 종료 조건 |
| --- | --- | --- |
| 1 · 04 Runtime/D | `offline_state_custody.rs`에서 고정 old-Q로 `backup-state`를 호출하는 실패 경로의 입력 생성만 수리한다. `legacy_offline_refusal.rs`가 이미 쓰는 old-Q 기동·Source 발행·정상 종료 흐름을 재사용한다. `offline_live_root::build_live_root`를 쓰는 현행 Catalog 직접 검사와 제품 거절 규칙은 각자의 계약을 유지한다. 03B/C/D는 같은 fixture를 다시 작성하지 않는다. | Root #11 실패 7개 실명에서 원래 backup/restore/corruption 단언이 모두 통과한다. 필요한 경우 fixture를 공유하는 실명도 영향 범위에 넣고 기존 NoFeatures 후속에 연결한다. [첫 실패](03-signed-published-cp2.md) · [기존 old-Q 발행 본문](../../../../crates/quanta-index-searchd-runtime/tests/legacy_offline_refusal.rs#L1486) |
| 1 · 02 R25 공급 | 기존 공급 오너가 `sdk_r25_paired_source_v1.py`의 terminal 교체 후 실패 정리와 pre-import root·argv 경계를 v33에서 수리했다. Root 독립 Python 61/61과 manifest 설치 검사를 인수했다. | 현행 S/Q/GQLang·Cargo 동결과 게스트 Cargo 설정 대조 후 v33으로 기존 공급에 입장한다. 실제 Linux terminal·ELF를 받은 뒤 formal 및 8 MiB SDK 본문을 확인한다. [v33 인수](02-semantica-runtime-sdk.md) |
| 1 · Root/F 공용 Cargo | 현재 staged `Cargo.lock` 보유자의 변경을 인수한 뒤 실패가 남으면 vendored `lancedb`의 Lindera 옵션 제거 후보를 한 번 재적용한다. 벡터 경로의 IVF-HNSW-SQ와 Jieba 동작을 확인하고 같은 입력에서 lock을 다시 해석한다. | 후보의 전체 feature closure·`cargo deny`·Native release 의존성을 확인하고 그 lock으로 후속 동결을 갱신한다. [의존성 후보](06-tantivy-retirement.md) |
| 병렬 · 기존 S A 발행 | V46 DB 마이그레이션과 signed SQL adapter가 있다. 진행 중 store 변경을 먼저 인수한 뒤 기존 Native Pending 트랜잭션과 `PendingCarrier`에 V6 root를 원자 결속하고 같은 artifact의 V9 수명을 연결한다. 이어 버전 있는 V6 root wire, ACKed predecessor의 cold read, path·symbol owner·cluster 인덱스를 연결한다. Child 보존·회수는 같은 원장에 연결한다. | 기존 V5 역사 입력과 V6을 각각 읽고, 같은 V6 Q/SQL 서명 설치에서 FULL→G2→새 PID와 변경 경로만 읽고 쓰는 비용을 검증한다. B는 기존 R5 assembler 경계를 맡으며 V6 writer는 기존 A 오너에 둔다. [V6 SQL adapter](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/indexing_machine_v2/store/native_semantic_tree_v6/sql.rs:63>) |
| 검토 먼저 · 03B/C/D | CURRENT 후보는 D3 Source 검토와 승인·공급 단계에 있다. 이미 결선된 Published SDK/CLI와 CP2 승인 경로를 같은 입력에서 인수한다. 확정된 실패가 나온 코드와 새 mixed Source가 필요한 경계만 원 소유자에게 넘긴다. | 승인된 D3 Q ELF·S·SQL을 한 설치에 고정하고 공개 SDK/CLI·CP2·복구의 기존 실명과 원 terminal을 인수한다. [CURRENT](03-signed-published-cp2.md) |

04.4의 90개 유지 ID는 기존 대체 선택자 71개에 매핑돼 있다. 원장 기준 19개 ID는 `BLOCKED`, 71개 ID는 `NOT_RUN`이다. 기존 Lexical/TEXT·Runtime 오너는 같은 동결 후보의 선택자·실행 바이너리·inventory·terminal을 인계하고, 정확한 계약을 실행할 수 없는 항목을 확인하면 해당 leaf만 수정한다. Root만 공용 원장에 반영한다. Q 49/49, FileBody 포함 19/19와 Linux P10 후보 1/1은 입력이 같은 범위에서 인수한다. 정식 P09/P10과 동일 ELF의 P11 두 실명은 E08 오너의 기존 경로에 둔다. R5 및 기존 IFX150, P07은 등록된 실명으로 먼저 확인하고 재현된 실패를 원 오너에 돌린다. [04 이관](04-maintained-test-migration.md) · [08 출하](08-release-activation.md)

코드·승인 입력이 준비된 항목부터 기존 Root 한 큐에 입장시킨다. Q 실패 7개, CURRENT 검토·서명 설치, R25 arm64 공급은 원 승인 경계와 소스가 각각 다르다. R25는 새 ELF의 frame을 먼저 측정하고 `8,388,608 B`에서 Native 2·TS/JS 1·Java 7을 실행한다. 실행을 합칠 때는 package·target·feature·실명·동결 소스·바이너리가 같은 결과만 공유한다.

## 지금 병렬로 준비할 작업

**11월 22일 정적 수리 상태.** Lexical 삭제 원 ID 91개와 기존 Runtime 43개·내부 oracle 4개의 처분을 유지한다. DATA와 Native의 실제 유지 계약은 exact 대체 본문에 연결한다. 정적 검사와 실제 본문은 같은 입력의 기존 Root 일괄 실행에서 인수한다. [일괄 실행 순서](#정적-수리-후-일괄-실행--11월-22일) · [04.4](04-maintained-test-migration.md#044-id별-유지퇴역-판정)

**11월 22일 02:34 KST 초기 판정.** Lexical 삭제 원 ID 91개 중 8개를 구 adapter 표현으로 분류했다. 후속 감사에서 8개 모두에 유지할 DATA 무결성 단언을 확인해 원장을 이관으로 정정했다. 91개 중 실제 본문으로 인수한 10개와 남은 81개는 04에서 추적한다. [04.4 원 ID](04-maintained-test-migration.md#044-id별-유지퇴역-판정)

**11월 22일 01:48 KST 초기 판정.** Q `d8a490565` 작업 트리와 lock `3128c36a74df…`에서 `just rust-check-tantivy-retirement`와 `just rust-test-authority`가 **VERIFIED · exit 0**이다. Lexical 원 ID 93개 중 91개는 엔진 퇴역, 2개는 호환 비용 시험 유지로 기록했다. 기존 오너가 Native 유지 계약을 ID별로 재대조한다. [후속 배정](#11월-22일-ss-후속-배정) · [04.4 원 ID](04-maintained-test-migration.md#044-id별-유지퇴역-판정)

**11월 22일 01:42 KST 코드 검증.** Runtime·Lexical·Harness 전체 타깃/전체 feature 컴파일과 `just rust-check-tantivy-retirement`, test/ignored authority, wire inventory가 **VERIFIED · exit 0**이다. Cargo 그래프·lock·소스·vendor 잔여는 **0**, Lexical 91개 엔진 ID는 승인된 퇴역 판정이다. S R5와 실제 출하·서명 설치 본문은 별도 수용이다. Root PID **50363**은 01:45 KST 조회 시 살아 있으며 기존 상태 파일은 `WAITING_Q_ADMISSION`, 실행 단계 **0/26**이다. 입장 용량·잠금은 실행 직전 재확인한다. [04.4 판정](04-maintained-test-migration.md#삭제-후보-재확인--11월-22일) · [Root 상태](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/root-q-language-regression/runtime04-native-graph-followup-6/status.json>)

| 기존 레인 | 지금 맡을 작업 | Root 인계와 종료 조건 |
| --- | --- | --- |
| **R5 S SDK·Runtime · 06** | 현행 IFX150 FULL은 실제 응답 조립의 exact Q rank와 같은 root의 foreign V9 ACK 거절을 검사한다. 설치 SCV2 제품 readback은 Native rank·Source, 마커·디렉터리 유실 거절과 Legacy socket 호출 0을 검사한다. [응답 조립](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/ifx150_restart_recovery_test_support_v1/native_initial_full_recovery_v3/public_assembler_regression_v3.rs#L110) · [SCV2](/Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-sdk-runtime-executor/src/client/ifx150_restart_recovery_test_support_v1/native_initial_full_recovery_v3/scv2_consumer_regression_v3.rs#L131) | 기존 FULL과 ACK 이후 재시작 exact 등록 두 건을 `--resolve-only`에서 재확인했다. 현행 Linux 서명설치 SDK/SQL에서 같은 generation·Source/view·Q rank, 정확한 typed cause와 재시도 게시·CAS 0을 확인해야 한다. 실제 SDK 본문은 **NOT_RUN**이다. [S R5 감사](/Users/songmin/Documents/code-new/semantica-codegraph-v2/docs/plans/oct-26-replay-query-read-custody/tickets/CURRENT_REMAINING.md#native-repomap-ack-audit-20261122) |
| **Lexical·TEXT · 04.3–04.4** | 삭제 원 ID 91개 중 구 adapter의 point/full 비교 1개를 퇴역하고, 90개의 Native·DATA·old-Q 계약을 이관한다. `sealed_commitment_cost` 2개는 고정 old-Q에 남긴다. | 이전 DATA 본문에 연결한 원 ID 10개는 소스 변경 이후 다시 검증한다. 유지 원 ID 90개에 대해 기존 레인의 현행 결과를 인수한다. [현행 04.4](04-maintained-test-migration.md#044-id별-유지퇴역-판정) |
| **Runtime/D · 04.2·04.5** | NoFeatures cutover/supervisor/offline custody의 기존 Root 요청 **26건**을 인수한다. old-Q 프로세스 장애 4개는 승인 debug ELF에서 별도 실행한다. Perf 43개와 held reader·RSS·dense·공유 소켓 4개는 원 SHA/실명과 현행 후보 본문을 보존하며 퇴역하지 않는다. | Root에 정확한 실명·실제 입력·exit를 제공한다. P10의 Native 상태 재구축·offline verify·잘못된 루트 거절·재오픈과 04.2 후보 selector를 기존 단일 큐에서 실행한다. [유지 테스트](04-maintained-test-migration.md#runtime) · [이관 원장](../../../../tools/ci/inventory/runtime-legacy-migration.toml) · [P10 이관](06-tantivy-retirement.md#p10-오너-증명과-출하-증명-이관) |
| **S Composition·Java·Runtime · 02** | 기존 R25 후속 Q/S·Cargo·feature/toolchain·formal pin과 새 Linux AArch64 ELF를 공급한다. 이전 `prepare_complete_owner_v1` 프레임·8,693,920 B 체인은 원 ELF RCA로 보존한다. 후속 ELF 실측에서 재현된 프레임/fixture가 있을 때 해당 작성자가 수리한다. | 새 ELF SHA를 기존 원 worker prefix·세 경로 계측에 결속하고 **8,388,608 B**에서 Native **2**·TS/JS **1**·Java **7**. P07 Backend/Composition/SDK owning lib 세 요청도 원 selector를 인수한다. [02](02-semantica-runtime-sdk.md#완료-조건) |
| **D/H/S 공급 · 03** | D/설치 오너는 기존 CURRENT preimage/supersession에서 승인 Linux Q/S 실물과 D3 ELF, 양쪽 역할·SQL worker·서명 readback을 재공급한다. H는 기존 raw Source export와 head별 FULL/DELTA·History 재시작, S는 실제 Published/FD6 caller를 공급한다. | 후보 동일성 확인 후 03.3 CP2 Host **11**, 03.4 기존 SDK22·Caller3·공개7·retained2, 03.5 정상/변이의 typed cause, 03.6 rollback/restore-forward 및 03.7 History를 각 원 분모·terminal로 닫는다. [03](03-signed-published-cp2.md#완료-조건) |
| **Semantica DSL / Harness / Hosted · 05** | DSL caller 오너는 Metadata **10**·Structural **2**를 실제 SDK/API/MCP/CLI에 연결한 기존 코드를 인수하고 독립 gold·현재 focused body를 공급한다. Harness A/B 외부 프로세스 이관과 기존 20 PASS를 인수해 승인 provider·비용·고정 데몬 입력을 준비한다. Legacy scale **38**은 기존 Harness 오너가 유지·퇴역을 판정한다. Hosted 오너는 승인 old-Q ELF의 runner/HTTPS 공급을 준비한다. | Root가 기존 Native DSL **33행**의 실제 공개 본문과 warm **200**·cold **20**, 벤치 A/B 계측을 원 캡처로 인수한다. Hosted는 동일 SHA의 PR/main·coverage·heavy job 원 결과를 낸다. [05 실제 호출·벤치·CI](05-bench-callers-hosted-ci.md) |
| **TEXT / B ANN / F Hybrid · 07** | TEXT는 Caddy·etcd의 Off 입장과 Large 입력, B는 ANN4K·whole SDK의 실제 실패 원인과 exact oracle, D/F는 기존 서명 검증 코드에 승인 설치 증거, F-EVAL은 동일 view 후보와 독립 블라인드 qrels를 준비한다. | 같은 06.4 후보에서 Caddy·etcd 각각 **Off 2/Record 1**, **4,096파일/두 OS**, ANN Recall@10 **≥0.95**와 물리 reclaim·Legacy fallback 0, Hybrid 독립 수치·signed Mixed/E07·Hybrid **960초**, 두 기준선 각각 **AB/BA 5쌍**을 판정한다. [07 계약](07-product-qualification.md) |
| **E08/P11 · 08** | 기존 오너가 E01–E07 및 실제 client/IntelliJ의 원 증거를 T 후보에 매핑한다. 운영 오너는 승인 Linux 타깃, 기존 action·독립 observer·등록 서명 키와 attested inventory를 준비한다. | 통합 A/코드 자격 → 승인 배포와 E06 선택 CAS → 통합 B/P11 운영 증명 → 최종 aggregate. `staged` 항목의 증명과 원 네 판정을 확인한다. [08 등록](08-release-activation.md) · [P11 운영 계약](../../../operator/p11-operational-proof.md) |

### Root/F 공용 합류 및 실행

- 공용 Cargo.lock·workspace/crate manifest·test/bench authority·nextest·recipe/CI·proof registry와 기존 ID inventory는 Root/F 한 작성자가 합류한다. 각 leaf는 원 테스트명과 실제 증거를 같은 04–08 티켓에 넘긴다.
- 기존 Root 실행기의 상태와 terminal을 확인하고 중복 실행 없이 현행 Q 27+3·Catalog 19 및 Path 9/cold 2, Source 1→Runtime 8→seal 2, 공개 API·module 정책을 01의 등록에서 닫는다. 04와 입력·실명이 일치하는 결과는 한 번 인수한다. 종전 Q **49/49**·Runtime **60**은 원 동결본과 일치하는 입력에서만 인수한다.
- 실행 가능한 입력부터 같은 Root 큐에 넣는다. R25 AArch64 후속과 CURRENT x86 서명 입력 준비는 병렬이다. P10은 P09와 승인 Native release-daemon을 요구하고, P11 paired R5는 같은 P10 ELF와 실제 S/Q 본문을 요구한다. 따라서 03·02·P10은 각자의 전제 충족 시 큐에 넣되 같은 P11/출하 후보의 증거 결속을 유지한다.
- 각 실패는 최초 compiler 오류·typed cause·body count와 영향 Source/Cargo를 남기고 해당 오너가 수리한다. 변경한 입력으로 같은 실명을 재검증한다. 본문이 없는 결과는 `NOT_RUN`, 없거나 검증 불가한 입력은 `BLOCKED`다.

## 공용 파일과 합류 순서

이 절의 표는 **삭제 전 합류 순서의 이력**이다. 06.2·06.3은 완료됐고 현재 삭제 후 수용과 작성 시작점은 위 레인별 표를 따른다. 실명 테스트·증명·CI와 통합 A→E06→최종 판정 순서는 [06의 게이트와 테스트 교체 범위](06-tantivy-retirement.md#게이트와-테스트-교체-범위)를 따른다.

Root/F가 Q의 공용 접점을 한 곳에서 합류한다. 우선 `legacy_reference.rs`/`legacy_reference/{source,binary,process}.rs`와 Runtime의 `tests/common`·`process_readiness_owner_v1.rs` mount를 확인한다. 세 leaf 레인은 기존 helper를 소비하고 변경이 필요한 경우 호출자, 입력/출력, 영향을 받는 selector를 Root에 전달한다. A/B의 `relevance/process.rs`는 A/B 전용 변경 범위와 공용 helper 호출 경계를 구분한다.

| 순서 | Root/F의 작성·합류 | 선행 인수 |
| --- | --- | --- |
| 1. 유지 경로 | Runtime/Lexical/Harness leaf와 module mount를 합류한다. crate별 `Cargo.toml`, `tools/ci/test-authority.toml`, `.config/nextest.toml`의 target/feature·선택 변경을 필요한 범위에 한해 적용한다. | 각 leaf 패치, 원 테스트명과 유지/퇴역 목록. `legacy_reference` 계약 변경이 있으면 영향을 받는 leaf를 같은 합류에 넣는다. |
| 2. R5와 DSL 전환 | R5 생산과 기동을 인수한 뒤 `scripts/verify-repomap-cross-repo.sh`와 `tools/ci/paired_r5_result.py`에 Native release 산출 경로·검증된 ELF SHA·실제 witness를 결속한다. DSL은 `scenarios.rs`, warm/cold runner·collector, `tools/benchmark/registry.toml`, `Justfile`, CI를 같은 패치로 정리한다. | `rust-build-release-native-daemon`으로 만든 기존 NoFeatures 후보에서 R5·DSL 공개 호출의 집중 회귀를 실행한다. DSL 33행의 21행+12행 유지·퇴역표와 독립 gold를 대조한다. [05](05-bench-callers-hosted-ci.md#정식-dsl과-실제-제품-호출) · [06](06-tantivy-retirement.md) |
| 3. Tantivy 삭제 | Legacy `serve`·Lexical/History/FST mount, workspace와 crate manifest, `Cargo.lock`, vendor, bundle, wire inventory·deny·CI/recipe를 한 후보에서 걷어낸다. data-only reader/export와 승인 복구·비교용 old-Q를 보존한다. | R5·DSL의 집중 본문과 유지 테스트 이관, D/H의 엔진 독립 복구 경로·fixture, 재실행 가능한 두 비교 기준. 최종 signed·hosted·성능 검증은 이 후보를 만든 뒤 실행한다. [06](06-tantivy-retirement.md#작성-순서) |

삭제 전에 이전 호환 서명 Q/S 설치 쌍과 삭제 전 Native의 실행 파일·호출자·설정·입력을 각각 고정한다. 이전 설치 쌍은 실제 선택·복구 입력까지 확인한다. 현 checkout에는 정식 DSL warm/cold 비교 파일이 없다. Legacy 기준을 다시 측정할 수 있도록 고정 old-Q 생산자를 확보하고, 최종 Native 후보에서 기존 `benchctl.py run dsl-authority --admit-baseline`으로 호환 기준선을 준비한다. 설정·코퍼스 digest가 바뀐 경우 기존 비교기의 일치 조건을 유지하고, 엔진 간 공통 기능 비교는 별도 원 `AB`/`BA` 계약으로 검증한다. 03의 서명 계약과 fixture는 재사용하고, 각 본문 결과는 그 동결 후보에 귀속한다. [DSL 기준선 계약](../../../../tools/benchmark/README.md#dsl-warmcold-gate) · [원 비교 계약](../../nov-12-native-cutover/TANTIVY-RETIREMENT.md#벤치-실행과-합격선)

### 패치 인계와 충돌 판정

- 인계에 기준 HEAD·바뀐 파일과 함수·패치 또는 commit·해당 파일의 기존 변경, 사용한 공유 계약·기대 결과·정확한 selector를 적는다. 미추적 Rust 모듈이나 fixture도 변경 목록에 포함한다. 원 오너의 현재 패치로 이미 해결된 변경은 그 패치를 인수한다.
- Root가 적용 직전 `git status --short --untracked-files=all -- <대상 경로>`와 `git diff -- <대상 경로>`를 확인한다. 같은 함수나 mount, 또는 다른 파일에 걸친 동일 wire·receipt·fixture 계약이 겹치면 해당 오너가 한 패치로 조정한다. 동일 파일의 독립 hunk는 기존 작성자 변경을 보존해 선별 적용한다.
- 합류 뒤 변경 목록과 `git diff --check`를 확인한다. 삭제된 mount는 대체 유지 body 등록과 같이 대조한다. 이미 제출한 동결 입력은 그대로 보관하고, 관련 소스·Cargo·바이너리·설정이 변한 검증만 기존 절차에서 다시 결속한다. [기존 공용 수정 규칙](../../oct-22-finalizing/OWNERSHIP.md#collision-resolution)
- 코드는 Main에서 담당 경계별로 작성한다. 격리 검증이 필요하면 이 작업의 임시 체크아웃 한 곳을 재사용하고, 결과 인계와 실행권 반환을 확인한 뒤 그 체크아웃을 제거한다.

11월 22일 05:54 KST FileBody 합류에서 `NativeIndexMaintenance` 예약 클래스와 `maintenance_handle()`에 두 수정이 겹쳤다. 기존 `Add Native scrub recovery test`(`01a1fee8-f424-7232-b73b-63dc26f2bc62`)의 한 패치로 소유 경계를 수렴한다. Runtime 유지 테스트 오너는 같은 함수에 별도 선택 분기를 추가하는 대신 원 보존 정책, 단일 Work 한도, 26→74 큐의 현행 소스 결속을 검증한다. Root는 배경 reclamation과 요청 기반 scrub이 같은 예약을 두고 경쟁하지 않는지, 원 deadline과 실패 축이 보존되는지, 실제 후속 3건과 기존 no-auto maintenance가 같은 수정에서 통과하는지를 대조한다. [04 후속 원 결과](04-maintained-test-migration.md)

## Root 단일 실행 큐

### 정적 수리 후 일괄 실행 · 11월 22일

이번 단계는 소스, 선택자와 제어 검사까지다. PID 50363의 구 요청은 원 terminal에 보존했다. 신규 Rust·QBC·설치 실행은 일괄 단계에서 기존 Root에 접수한다.

| 순서 | 기존 실행 | 인수 조건 |
| --- | --- | --- |
| 입장 전 | Q Cargo 후보 인수 | 기존 `Cargo.lock` 보유 실행이 끝나면 Lindera 기능 제거 후보를 같은 소스에서 다시 해석하고 `cargo deny`를 확인한 뒤 출하 입력을 고정한다. [06 의존성](06-tantivy-retirement.md) |
| 1 | 현행 NoFeatures 26 → Q Lexical 27 → UDS 3 → Catalog 19 | 구 PID 50363의 입장 전 종료를 인수했다. 최종 현행 입력을 기존 Root 큐에 한 번 접수하고, Catalog는 같은 Source·캐시의 정확한 30 PASS를 받아 기존 `--catalog-after`로 연결한다. |
| 2 | Lexical DATA·Native UDS·고정 old-Q 집중 27, Runtime Perf·내부 oracle | Lexical 로컬 결과는 BOOT 5/5, lib 2/2, 고정 old-Q DATA 15/15다. 마지막 DATA 실명은 단언 수정 뒤 1/1 PASS했다. 출하 원장은 동결 소스와 원 실행 증거를 인수하고, Linux 전용 분기는 기존 Linux 레인에서 본문을 확인한다. 실제 Linux metadata 포트, FileBody 7개와 Catalog의 pending seed·다중 incident도 확인한다. 같은 동결 입력에 중복되는 실명은 결과를 공유한다. 원 ID별로 실제 exact PASS를 대조한다. [04.4](04-maintained-test-migration.md#044-id별-유지퇴역-판정) |
| 3 | Semantica R5·IFX150, 신규 ELF 8 MiB R25 | R5 Runtime exact와 실제 IFX150 재시작을 기존 S QBC에서 인수한다. 기존 `test-native-delta-cold-owner`의 두 Runtime owner 선택 및 signed cold DELTA를 같은 레인에서 실행한다. RepoMap 키 비용, semantic snapshot 3건(기존 병합 실명의 changed-scope 검증 비용 포함), V5 buffer 이관 5건, writer wire 및 completed V5 검증 재사용 1건, 선행 semantic readback 2건, 부팅 transfer marker·orphan inventory·Store 퇴역 3건은 기존 Runtime 발행 배치에서 인수한다. Orphan inventory는 단일 본문 검증과 marker 재대조를 보강한 현행 exact를 사용한다. Semantic G3 회귀는 정본 owner 범위, shadow-off 검증, 교차 corpus 중복 거절을 포함한 현행 소스를 동결한다. 현행 managed ItemIndex 및 V5/V9 readback 수리를 포함해 기존 FULL, 두 번째 DELTA terminal, Pending-before-writer-markers를 동일 Q/SQL 서명으로 동결한다. Required RepoMap의 V9 재조회는 같은 기존 checked DELTA terminal과 Q Published-before-ACK 두 실명에 포함한다. Fresh Native issuer의 snapshot 보유 변경은 같은 checked DELTA 비용 matrix와 기존 cold ACK·Q Published 복구 입력에 포함한다. RepoMap ACK marker Original 입장 변경은 기존 FULL same-target, FULL ACK cut과 G2 checked/cold 두 cut에 포함한다. 선행 Required V5 재사용과 WriterState 입장은 기존 G2 두 실명 및 기존 WriterState marker exact에 반영한다. Cold current Required 보존은 checked DELTA·Q Published-before-ACK·cold ACK의 기존 세 실명과 같은 signed 설치에 포함한다. V5 승격은 기존 pending-before-writer-markers signed 실명과 기존 Runtime 발행 배치의 inode exact로 확인한다. R25는 현행 S/Q/GQLang과 Cargo를 고정해 arm64 ELF에서 프레임을 먼저 측정하고 Native 2, TS/JS 1, Java 7을 실행한다. [IFX150](06-tantivy-retirement.md) · [R25](02-semantica-runtime-sdk.md) |
| 4 | 현재 서명 설치의 실제 공개 SDK·CLI Published, CP2 | D3 이후 Q와 승인 S/SQL을 같은 설치에서 사용한다. 공개 7건, Caller 3건, signed SDK 22건, CP2 정상·mutant 및 Host 11건, retained 2건은 기존 목록을 따른다. [03](03-signed-published-cp2.md) |
| 5 | P09 → P10 → P11 paired R5 | 승인 Linux daemon에서 P10 exact 실행과 4회 daemon 관측을 받은 뒤 같은 ELF·깨끗한 Q/S로 P11 caller/kernel을 실행한다. 운영 P11의 승인 입력은 08에 귀속한다. [08](08-release-activation.md) |

출하용 Lexical·Runtime 이관 증거는 기존 오너 실행의 동결 Q checkout에서 `python -m tools.ci.retirement_nextest`로 수집한다. `--revision`, 외부 `--evidence-dir`, 기존 `--lane`, `--package`, `--lib` 및 필요한 `--test`를 각각 지정하고 원래 `--filterset`을 사용한다. 실행기는 한 번 빌드한 바이너리와 같은 feature의 Cargo metadata를 수집·실행에 재사용한다. `QUANTA_RETIREMENT_NEXTEST_EVIDENCE_DIR`에는 출력된 증거 ID의 `inventory.json`, `nextest.jsonl`, `source-manifest.json`, `binaries.json`, `cargo.json`과 `target`의 해당 실행 파일을 보존한다. 원장 `run_id`는 출력 파일명의 증거 ID다. manifest는 실제 명령과 package·타깃·feature·filter를 기록한다. old-Q가 사용된 실행은 `target/legacy-searchd`까지 같은 증거 폴더에서 보존한다. 출하 검사는 원장 2개를 제외한 Git tree, 본문, 보존 실행 파일의 SHA를 다시 대조한다. Runtime event는 현행 Cargo 타깃과 모듈 이름으로 인수한다. [증거 생산기](../../../../tools/ci/retirement_nextest.py#L164) · [Runtime event 검사](../../../../tools/ci/lint/check-runtime-legacy-migration.py#L171)

11월 22일 04:07 KST 기존 Runtime04 PID **50363**은 입력 변경 중 첫 `cutover_sync` 입장 없이 **exit 143 · 0/26**으로 종료됐다. Q 공식 GC, Semantica target·QBC 및 Docker 비사용 빌드 캐시에서 회수 가능한 로컬 바이트를 찾지 못했다. 로컬 가용 약 19 GiB였으며 현행 서명 설치의 필요량은 51.10 GiB다. 새 런타임 요청은 일괄 단계에서만 접수한다. [원 terminal](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/root-q-language-regression/runtime04-native-graph-followup-6/status.json>)

**이전 Runtime04 입장 이력.** PID 13486은 첫 `cutover_sync`의 `docker ps` timeout으로 **exit 1 · 본문 0/27**, PID 50739는 용량 대기 후 입장 없이 **exit 143 · 본문 0/27**, PID 75708은 Docker 대기 중 **exit 143 · 본문 0/32**였다. 당시 Docker `/_ping`도 timeout이었으나 이후 API HTTP 200을 확인했다. 이 종료 기록은 현재 PID 50363의 결과가 아니다. [13486 terminal](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/root-q-language-regression/runtime04-static-repair-followup-3/status.json>) · [50739 terminal](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/root-q-language-regression/runtime04-static-repair-followup-4/status.json>) · [75708 terminal](</Users/songmin/Library/Application Support/semantica-codegraph-v2/recovery/p07-runtime-closeout-20261109-01a183c0/feature-on-retry-linux-main-f82f0ca721d3/sdk-structural/rr-internal-controller/root-q-language-regression/runtime04-static-repair-followup-5/status.json>)

| 순서 | 실행과 실제 의존성 |
| --- | --- |
| **설치 입력 및 집중 본문 인수 · 03/04/05/06** | 03의 정확한 Linux Source는 기존 preimage 확인으로 재인수한다. 원 입력이 없어졌으면 03.1의 기존 supersession으로 대체 Source를 검토·등록한다. R5 실제 게시자·kernel, F15 수정본, DSL 유지 caller는 기존 요청·selector의 실제 결과를 받는다. 실패는 최초 단계의 원 terminal로 확인하고 영향받은 본문만 재실행한다. [03.1 입력](03-signed-published-cp2.md#완료-조건) |
| **삭제 후보 수용 · 06** | 합류된 Q/S 소스·Cargo와 타깃별 daemon·SDK·CLI·설정의 SHA를 고정한다. 통과한 `just rust-check-tantivy-retirement`와 all-features 소스 검사를 인수하고, 실제 릴리스 컴파일·번들의 Tantivy 0을 확인한다. 서명 설치에서 R5, 기본 Native 호출과 구 요청 거절, History, FULL/DELTA·재시작·ACK 유실 재시도·CP2 및 첫 durable commit 전 rollback/이후 restore-forward를 원 기동으로 검증한다. |
| **기존 한도의 제품 실측 · 07** | TEXT가 quiet 호스트에서 Caddy 590·etcd 1,421의 첫 Off를 확인하고 같은 후보의 각 Off 2회·Record 1회와 Large 4,096·두 OS 재시작을 채운다. 원 30,000 물리 파일·512 MiB WorkBytes·읽기/쓰기/변경 각 100만·SDK I/O와 submit→FULL ACK 60초를 적용한다. 설치본에서 F-EVAL이 같은 view의 SDK 후보와 독립 qrels로 원 Hybrid 승격 지표를 판정한다. 원 E07·ANN·Hybrid960과 두 기준 각각 `AB`/`BA` 5쌍을 실행한다. submit→ACK와 조회 p95의 짝비 중앙값은 각각 1.05 이하다. |
| **기본 선택 · 08** | 동일 후보의 03 설치 복구, 05·06·07과 통합 A 및 `CODE_QUALIFIED`를 확인한다. 코드 자격에는 Q P10과 동일 ELF의 `p11-cross-repo-cutover`가 포함된다. 승인 타깃에서 배포를 관측하고 E06 fence·CAS를 수행한다. 통합 B 운영 증명까지 받은 뒤 최종 aggregate를 검증한다. quiet Off·기준선 채집은 다른 부하와 시간을 겹치지 않게 한다. |

입장 전 호스트 부하·잠금·서명·기준선 누락은 `BLOCKED`로 기록하고 독립 준비를 계속한다. 컴파일 실패·실행 중 한도/기한·ACK/재오픈/trace 오류·품질 수치 미달은 실패한 범위에서 `FAILED`로 기록한다. 최초 거절 단계와 원인을 해당 작성자가 수리하고 기존 selector부터 재검증한다. Hybrid 미달은 정답의 후보 풀 누락과 융합 순위 손실을 분리한다. 순위 조정은 DEV로 돌아가며 다음 승격은 새 독립 EVAL로 판단한다. 결과는 각 티켓에 Source·요청·정확한 본문·exit와 함께 인계한다.

## 티켓별 종료 표

각 번호는 개별 티켓의 같은 체크박스다. 원 검증기가 요구하는 Source·관련 overlay/Cargo·feature/target·빌드/실행 해시 및 실제 selector/receipt를 연결한다. 명령 실행 결과와 본문을 분리해 기록하고, 컴파일만 된 경우 해당 본문은 `NOT_RUN`이다. 하나의 원 본문이 여러 항목에 필요하면 동일 입력·정확한 witness 범위에서 원 영수증을 참조한다. [원 수용 기준](../../nov-12-native-cutover/DOD.md)

| 완료 조건 | 마감 책임 또는 공급자 → 소비자 | 닫을 때 남길 결과 |
| --- | --- | --- |
| [01](01-q-regressions.md) `01.1`, `01.4` | Root/F Q 실행 | 기존 controller 원 admission 및 Q snapshot, Lexical **27** → UDS **3** 명단·exit·원 terminal. 실제 실패 원인 수리의 재검증과 paired Q 인계. |
| [01](01-q-regressions.md) `01.2` | Root/F Path, 필요한 생산 지원은 TEXT | Path Lexical **9**, cold reopen **2**, `just rust-public-api`·`just rust-cargo-modules`, `SegmentedFullV1` 명시 거절 또는 승인된 유지 범위의 실제 공급. |
| [01](01-q-regressions.md) `01.3` | Root/F 이관 후속 | Source **1** → Runtime **8** → seal **2** 그 자체의 selector와 본문. 04와 중복되면 같은 Source/target/test 결과를 참조한다. |
| [02](02-semantica-runtime-sdk.md) `02.1`, `02.2`, `02.3` | S composition·SDK·Java → Root/F | 같은 승인 Source/Cargo/control/QBC pin, 새 ELF와 세 worker 경로의 프레임 검사, 원 **8 MiB** 사전검사 및 SDK10 **Native 2·TS/JS 1·Java 7** terminal. |
| [02](02-semantica-runtime-sdk.md) `02.4`, `02.5` | S Runtime·Backend·Composition·SDK → Root/F | Runtime6의 Work3 현행 결과와 Storage3·Runtime291의 영향 범위, 기존 Backend·Composition·SDK owning lib 3요청 compiler 및 source/cause/retained/drop·환급 본문. |
| [03](03-signed-published-cp2.md) `03.1`, `03.2` | Root/F intake, D/S Source·FD6 공급 | 기존 preimage 검사·필요한 원 supersession, exact paired S/Q와 두 role·S/SQL worker 서명 설치 신원. 실제 초기 Source/ACK의 원 V9/FD6/Q, 필요한 fresh Attach의 Store CAS/HEAD와 동일 Source 재읽기. |
| [03](03-signed-published-cp2.md) `03.3`, `03.5` | D CP2 → Root/F 실행 | Host **11**(3+3+2+1+2)의 실제 compiler/body와 signed CP2 정상·변이의 parent/mutant/sibling·request/attestation 및 지정 typed cause. |
| [03](03-signed-published-cp2.md) `03.4` | S signed x86 SDK 공급 → Root/F Caller·Published | S SDK11/Native2와 signed SDK **22본문**(3+3+3+11+2)의 원 등록 수용, 별도 Caller **3**, ordinary 공개 **7**(Published4+복구3)·retained **2**(History/Host)의 원 terminal·실제 Published readback. |
| [03](03-signed-published-cp2.md) `03.6`, `03.7` | D 복구·H History·S producer → Root/F | rollback/restore-forward·동일 operation ACK-loss retry·재게시0·새 PID와 선택/조회. H actual Source export→S/head별 FULL, signed FULL→DELTA→restart, cross-revision과 V9/typed History receipt·동일 Q view. public7/retained2에 해당하는 본문은 03.4 원 결과를 연결한다. |
| [04](04-maintained-test-migration.md) `04.1`, `04.2` | Runtime 테스트 작성자 | 실제 backup/restore 및 보존할 Fast/Risk/Extended·semantic-stream의 ID별 대체 경로·fault oracle·기존 등록 대응. |
| [04](04-maintained-test-migration.md) `04.3`, `04.4` | Lexical 테스트 작성자 | adapter 의존을 끊은 게시 fixture와 boot/file authority, 공개·물리/Work 검증의 유지·퇴역 목록 및 DATA-only 보존. |
| [04](04-maintained-test-migration.md) `04.5` | Root/F Q 실행 | 정확한 NoFeatures target과 중복 없는 test authority 등록, 유지 본문별 실제 PASS. |
| [05](05-bench-callers-hosted-ci.md) `05.1`, `05.2`, `05.5` | Harness 작성자 → Root/F 등록·기존 벤치 실행 | 외부 프로세스로 이관한 Hash/OpenAI 두 arm, 기존 실측 입력·provider, SDK publish/query와 요청별 계측/결과. |
| [05](05-bench-callers-hosted-ci.md) `05.3`, `05.4`, `05.6` | S/SDK 실제 호출자 → Root/F DSL | 33행·실제 caller·독립 gold 및 집중 본문으로 warm/cold 생산자와 공식 registry/CI 변경을 삭제 후보에 합류. 동일 Linux의 원 warm **200**/cold **20** 비교는 최종 후보에서 수용. |
| [05](05-bench-callers-hosted-ci.md) `05.7` | CI 공급 오너 → Root/F hosted 검증 | 승인 old-Q의 hosted 위치·크기·SHA·TLS와 실제 PR/main·correctness 원 결과. 공급 확인과 hosted 실행을 따로 기록한다. |
| [06](06-tantivy-retirement.md) `06.1`, `06.2`, `06.3`, `06.4` | Root/F 단일 작성·후보 조립 | 유지 입력 및 두 비교 기준 동결, Legacy `serve`/engine/FST·feature·Cargo/lock/vendor/등록 정리, 전체 및 실제 release·번들 **Tantivy 0** 후보의 해시. |
| [06](06-tantivy-retirement.md) `06.5` | Root/F signed 설치, D/H 복구 증거 공급 | 같은 후보의 실제 기본 Native 호출·구 요청 거절·보존 경로 복구 본문. 08.1은 이 결과를 인수한다. |
| [07](07-product-qualification.md) `07.1`, `07.2`, `07.6` | D 서명 계약 → F Hybrid 소비/F-EVAL → 독립 심사 결과 인수 | 실제 동일 설치·동일 view·Q ELF/선택/BOOT 결속과 typed 거절, 실제 blinded 후보·독립 qrels의 원 캡처/파일 universe 결속, 원 수치 기준을 만족한 EVAL 판정. |
| [07](07-product-qualification.md) `07.3`, `07.4` | 기존 TEXT → F 최종 후보 인수 | 원 Caddy·etcd 한도와 각 Off **2**/Record **1**, 실제 ACK/조회/reopen 및 완전한 trace와 Large **4,096**·두 OS 재시작 본문. |
| [07](07-product-qualification.md) `07.5` | 기존 B/ANN → F 출하 인수 | Native LanceDB ANN4K와 whole SDK의 같은 소스·resolver, 독립 exact와 층별 기준, old pin·overlay·두 번째 재시작·실제 reclaim 및 Legacy fallback 0. |
| [07](07-product-qualification.md) `07.7`, `07.8` | F 기존 성능·비교 실행 | signed Mixed/E07·Hybrid 960초와 부모/자식 terminal, 보존한 두 비교 기준 **각각** AB/BA 5쌍 및 원 1.05/절대 SLO 판정. |
| [08](08-release-activation.md) `08.1`, `08.2`, `08.3` | F T 후보 → 기존 E01–E07/IDE 공급 → E08 | 같은 격리 설치 후보 T 결과, R01–R40 원 요구와 적용 N/A, 실제 client/IntelliJ 포함 기존 필수 증거, 동일 출하 입력의 `rust-policy` 통과와 통합 A 승인. |
| [08](08-release-activation.md) `08.4`, `08.5`, `08.6` | 기존 P11 운영 입력 → E06 전환 → E08/P11, crate 오너 → Root 정책 게이트 | 승인 target/action/observer와 attestation, E06 실제 fence/선택 CAS/기본 호출, 통합 B 및 네 출하·활성화·복구 상태. 같은 출하 SHA의 hosted CI·API/module·workspace와 `rust-policy` 결과, 루트 보존 상태. |
| [08](08-release-activation.md) `08.7` | E08/F 마감 | 원 검증 결과와 01–08 현행 상태를 원장에 결속. 필수 실패·입력 차단·미실행의 잔여가 없을 때 기존 티켓을 닫는다. |

용량/락은 Root가 기존 controller·terminal로 해소한다. R25 승인 pin은 S SDK, CURRENT 서명 Q/S와 설치·복구 입력은 D/H/S, provider·hosted 공급은 Bench/CI, 실제 target·observer는 기존 P11 오너가 해소한다. 해당 입력이 빠지면 해당 번호와 해소 담당을 티켓에 기록하고 독립 레인을 계속 진행한다. 원 계약상 허용되는 `NOT_APPLICABLE`에는 적용 범위와 근거를 남긴다.

## 11월 21일 작업 채팅 배정

기존 담당 채팅 16곳에 후속 범위와 종료 조건을 전달했다. 보관 상태였던 Hosted CI·Lexical·SDK22는 기존 채팅을 복원했다. 전달 뒤 16곳 모두 진행 상태를 확인했다. 실제 본문·exit와 합류 판정은 위 표의 해당 번호에서 인수한다.

| 레인 | 작업 채팅 | 담당 조건 |
| --- | --- | --- |
| Runtime 유지 테스트 | `01a1f99e-d06d-75f2-b82b-2d9e72382f99` | 04.1–04.2 leaf와 04.5 등록 인계 |
| Harness A/B | `01a1f99e-d6a5-7a00-8c13-147723cab6f3` | 05.1·05.2·05.5 |
| Semantica 실제 호출자 | `01a1f99e-db8d-70b3-8f79-8321a3a2749a` | 05.3의 12행·gold, 05.4·05.6은 Root 통합 |
| Semantica R25 | `01a1f99e-e34b-7b31-9d14-2410c0eed497` | 02.1–02.5, 기존 S 오너의 후보·제어·terminal 인수 |
| Hosted CI 공급 | `01a1f99f-8dd1-7011-ad45-45973aeaae13` | 05.7 승인 입력과 실제 hosted 결과 인수 |
| Hybrid 서명 소비 | `01a1f99f-937d-72f1-ac16-374ac4c1dd2b` | 07.1·07.2, 공급은 D, 품질 수용은 기존 F |
| E08/P11 출하 준비 | `01a1f99f-9b86-7fb2-8b06-b2bae90b3669` | 08.2·08.4와 08.7에 필요한 원 증거 인수 |
| Semantica 실제 R5 | `01a1ce51-c832-7dd0-9379-04f91696af07` | 06 실제 Required RepoMap 게시자·Native kernel terminal·공개 RepoMap reader. 실제 SDK cold/restart 본문과 paired 선택자는 기존 Root 등록을 재사용 |
| Lexical 유지 테스트 | `01a1f991-3e84-7273-ab17-23a7a4554bba` | 04.3·04.4 fixture·oracle·유지/퇴역 실명, F15 실행 결과 인수 |
| F15 현행 실행 | `01a1fcaa-faac-7e31-bc40-66ba70a81aff` | [F15 NoFeatures](04-maintained-test-migration.md#f15-nofeatures-filebody-restart) **3/3**와 [Native G1](04-maintained-test-migration.md#f15-native-g1-nextest) **1/1 PASS · exit 0** 인수 |
| D BOOT·복구 공급 | `01a1cbf6-5250-7fc0-ab65-d04da96c87f1` | BOOT/Host·FD6·Source 및 하위 거절/복구 증거, formal P10 등록은 Root |
| 03B/C/D 설치 인수 | `01a1f991-380f-71d2-805a-b4e089c17bf3` | 기존 서명 Q/S 설치·D3·정상 복구·CP2 실행과 원 terminal |
| History 원본 공급 | `01a1d915-2ffe-7ed3-b1be-807378c043cb` | 03.7 Source/History 보존·replay 입력, 실행은 03 원 결과 공유 |
| 고정 old-Q SDK22 | `01a1f991-2f97-7092-8d3c-6434d1dfa012` | 승인 macOS old-Q 22실명 원 영수증 |
| TEXT SDK60·Large | `01a1d915-3719-76b0-8d1d-dcf3213cba72` | 07.3·07.4 원 한도와 최종 후보 제품 본문 |
| ANN4K·whole SDK | `01a1cbf6-46b6-7590-8550-c818539e9f2d` | 07.5 기존 B 실행과 독립 exact oracle |

11월 22일 00:57 KST 기존 S R5 public facade·ACK 바인딩·frozen metadata를 인수했다. 별도 설치 RepoMap 루트 선택과 missing Native route 거절은 이 채팅의 R5 후속이다. Root의 원 paired 등록과 SDK 콜드 재시작 실명을 공유한다. [06 현행 제품 경로](06-tantivy-retirement.md)

01·06의 공용 Cargo/lock·레시피·proof 합류, Q formal P10과 paired R5 등록 및 무거운 검증은 원 Root/F 큐가 맡는다. F-EVAL 독립 홀드아웃은 기존 Root/F 수용 범위에 둔다. 07.1·07.2 채팅은 D의 설치 증거를 소비한다.

### 11월 22일 ss 후속 배정

기존 채팅 8곳에 후속 배정을 전달했다. 02와 03은 보관된 채팅을 복원했다. Runtime 04의 PID 50363과 Lexical Boot의 기존 실행권을 인수한다. 새 채팅·큐 추가는 0건이다.

| 기존 채팅 | 이번 배정 | Root가 인수할 결과 |
| --- | --- | --- |
| [SCG 아키텍처 감사] B · Runtime·공통 계약 · `01a1fe3b-4786-7b13-ad17-f7a6cb3ba6be` | 기존 assembler의 정상 rank·예산, 동일 root에서 다른 V9 provenance 거절 | 기존 설치 fixture와 selector·feature·workset 단언 |
| [SCG 아키텍처 감사] A · Native Source·발행 · `01a1fe3b-5170-7f73-a6c2-c38f874220ac` | IFX150 FULL/G2와 typed SDK SCV2, 마커·브리지 유실에서 typed 거절·Legacy 호출 0 | 기존 IFX150 selectors·실행 수·동결 입력 |
| Tantivy 퇴역 · Lexical 유지 회귀 이관 · `01a1f991-3e84-7273-ab17-23a7a4554bba` | 43+7+41개 원 ID의 Native 유지 계약 대조와 Boot freshness 인수 | 원 ID별 판정과 유지할 실제 대체 본문 |
| Tantivy 퇴역 · Native Text SDK60 TDD · `01a1d915-3719-76b0-8d1d-dcf3213cba72` | 기존 27개 동결 실행과 같은 repo/path chunk 축약·scoped 검색·case/Source의 열린 단언 | 신규 selector와 유지 계약별 oracle, 고정 old-Q 결과 |
| 04 Runtime · 백업·복구 유지 회귀 이관 · `01a1f99e-d06d-75f2-b82b-2d9e72382f99` | 기존 EFBIG/ENOSPC leaf와 Perf 32·동일 프로세스 4개의 원 실명 보존 | selector·원 소스 SHA·P10 등록 변경분을 Root에 인계 |
| 02 Semantica · R25 프레임·SDK 결과 인수 · `01a1f99e-e34b-7b31-9d14-2410c0eed497` | 새 arm64 ELF 및 기존 8 MiB 계측·선택자 공급 | ELF SHA·formal controls·Native 2 / TS/JS 1 / Java 7 |
| 03B/C/D · 서명 설치와 정상 복구 인수 · `01a1f991-380f-71d2-805a-b4e089c17bf3` | CURRENT 서명 설치, D3 ELF, 공개 SDK/CLI·CP2 입력 | 동일 설치의 실제 caller·typed 거절·복구 본문 |
| 08 E08/P11 · 출하 증거·승인 입력 인수 · `01a1f99f-9b86-7fb2-8b06-b2bae90b3669` | 기존 P09/P10 Linux ELF와 paired R5 입력 추적 | P09/P10 terminal, 동일 ELF SHA, Q/S resolver 결속 |

PID 50363은 입장 전에 **exit 143 · 0/26**으로 종료됐다. Root는 현행 입력을 동결한 기존 레인에서 NoFeatures **26→Q 27→UDS 3**을 실행한다. Catalog **19**는 같은 Source·캐시의 정확한 Q/UDS 30 PASS와 exit 0을 확인한 뒤 기존 `--catalog-after`로 연결한다. R25, CURRENT Published·CP2, formal P10·P11은 승인 입력이 준비된 기존 레일의 결과를 순서대로 인수한다. [Q](01-q-regressions.md) · [R25](02-semantica-runtime-sdk.md) · [CURRENT](03-signed-published-cp2.md) · [P10/P11](08-release-activation.md)
