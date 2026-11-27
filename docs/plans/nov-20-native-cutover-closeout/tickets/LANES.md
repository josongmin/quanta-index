# Native 전환 레인별 작업 계획

[01–08 티켓](README.md)의 착수·합류 순서다. 실행 전에 대상 HEAD·dirty hunk·기존 오너를 확인한다. 원 실행 상태는 [01](01-q-regressions.md)·[04](04-maintained-test-migration.md), 조건별 마감 책임은 [티켓별 종료 표](#티켓별-종료-표)에 있다. 이전 실행의 제어 수리와 결과는 [NC-F 원장](../../nov-12-native-cutover/tickets/NC-F-integration.md#root-단일-큐-현재-검증--2026-11-16)에 보존한다.

## 현행 구현과 수용 순서

### 코드별 소유와 수용

**Root의 Q ANN 코덱 합류와 derive 정리는 완료했다.** 새 출하의 RepoMap 작업은 [별도 제거 티켓](../../nov-23-repomap-retirement/README.md)을 따른다. 아래 RepoMap DELTA 작업과 실행 결과는 후속 설계 및 기존 동결 입력 이력이다. Q Graph/SDK는 기존 A가, V6의 독립 FULL·cold 복구와 memo CAS 회수는 기존 A/Store가 맡는다.

| 작업 | 현재 코드 | 기존 단일 작성자와 남은 코드 | 수용 |
| --- | --- | --- | --- |
| Q 정책 · 01 | `6f4744e51`의 `ann_quality.rs`·`ann_enospc.rs`와 `b766fa676`의 `ann_directory.rs`가 합류했다. 현행 작업트리의 `ann_quality.rs`·`ann_enospc.rs`는 수정 상태이므로 최종 후보에서 소스와 결과를 다시 결속한다. 기존 derive 검사 **위반 0건 · exit 0**. ANN JSON·훈련 하한 **2/2**, Catalog 저장 행 **2/2**, Runtime 물리 프로파일·설정·재열기 **3/3** 통과. | Root ANN 코덱 작성 완료. 제품 4K의 deadline과 worker는 기존 B가 소유한다. | 원 4K 재시작은 `SourceAdvance` 30초, 별도 Whole SDK 4K는 owner work 1,200초에서 실패했다. Linux FUSE checked-close는 승인 fault 장치가 필요하다. [01](01-q-regressions.md#남은-코드와-합류) · [07](07-product-qualification.md#ann과-hybrid) |
| RepoMap 출하 제거 · 기존 S/A | S ImportGraph 중복 순회 제거와 FNL410 커널 8/8, Q 봉인 재시도 1/1, Graph FULL·DELTA 상한 2/2는 이전 동결 결과다. 네 간선 본문은 동결 drift로 Cargo 전 종료했다. | 기존 S 생산자와 A의 Q Graph/SDK가 호출자·발행·공개 계약 제거를 맡고 D/E08이 운영 절체를 맡는다. | 이전 Required 복구와 새 후보의 무게시·유지 검색을 기존 큐에서 수용한다. DELTA 최적화는 보류한다. [새 출하 범위](../../nov-23-repomap-retirement/README.md) · [DELTA 설계·결과](06-tantivy-retirement.md#현행-코드-작업-순서) |
| V6와 회수 · 06 | 제품 V49 첫 퇴역과 exact Cleaned 이후 SQLite DAG·memo hold 정산·공유 Required writer unlink가 연결됐다. ReclaimMemo는 별도 기존 제품 경로다. | A/Store가 실제 V6 Resync의 cold 증인, signed DDL 원자 전환, release와 global Required ref 보호를 담당한다. | 기존 CAS Root 배치 **0/24**. 첫 제품 Cleaned의 새 PID 검증과 공유 파일 보존·삭제도 `NOT_RUN`이다. [06](06-tantivy-retirement.md) · [S 현행 원장](/Users/songmin/Documents/code-new/semantica-codegraph-v2/docs/plans/oct-26-replay-query-read-custody/tickets/CURRENT_REMAINING.md#a-native-non-repomap-audit-20261123) |
| H History · 03.7 | S production은 이전 Q pin에서 **exit 101**이다. 필요한 API와 S 수리를 함께 포함한 봉인 후보가 있다. | 기존 H 레인에서 같은 입력의 production → CLI → Original replay 2개 → Required DATA 2개를 확인한다. Q는 영향 변경만 재검증한다. | 서명 설치 보류. 이후 같은 승인 후보에서 Source export→ingest→FULL/DELTA·재시작과 ACK-loss를 인수한다. [NC-H](../../nov-12-native-cutover/tickets/NC-H-history.md) · [03.7](03-signed-published-cp2.md#history-03-7) |
| R25와 D3 · 02/03 | 승인 pin SHA `1d7fc7e6…`는 남아 있고 외장 pool과 QGL은 확인했다. pin의 정확한 S·Q 경로와 CURRENT intake는 부재한다. | 기존 P07 제어기에서 승인 입력을 복구하거나 검토된 후계 S·Q를 봉인한다. R25는 확보된 후보를 현행 Main·Cargo 및 v34 AArch64 guest 입력과 대조한다. [설치 입력 판정](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#현행-판정--2026-11-25) | D3 새 Native Q와 signed worker→Published/CP2/복구, R25 별도 AArch64 terminal·formal→8 MiB 10본문과 P07을 기존 큐에서 인수한다. [R25](02-semantica-runtime-sdk.md) · [D3](03-signed-published-cp2.md) |
| DSL·B 제품 · 05/07 | Native SDK/CLI·Harness가 연결됐다. B의 Kernel 4경로와 overlay 2경로는 원 요청 기한을 전달하며, 점유 리포트는 표본 0을 처리한다. | B의 Kernel 기한 경쟁과 인접 exact **3/3**, Runtime overlay **1/1**은 통과했다. 후기 Record→ANN4K·Product4K는 기존 B가 확인한다. MCP는 실제 유지 호출을 먼저 대조한다. 기존 Metadata/Structural MCP 계약이 확인될 때 해당 호출을 이관한다. | 공개 Native 단언, DSL warm/cold, hosted old-Q, SDK60/Large·ANN/4K·Hybrid/E07을 각 소유 입력에서 확인한다. [호출자](05-bench-callers-hosted-ci.md) · [제품](07-product-qualification.md) |
| 출하 퇴역 · 04/06/08 | Tantivy 삭제, Nextest 128 로스터 수집 및 서명 번들 검사가 구현됐다. E08의 후속 paired R5 제어 7파일은 **227/227 PASS** 검증 바이트로 Q `eebb7f2b3`에 합류했다. | Root/F가 같은 Q/S·Cargo의 clean 후보와 old-Q 공급을 결속한다. E08이 P09/P10/P11의 선행 권위와 승인 운영 타깃을 담당한다. | 128개, 최적화·서명 번들 Tantivy 0, 동일 P10 ELF의 Linux R5 2개 및 P11 운영 증거를 받는다. Linux R5는 **0/2 NOT_RUN**이다. [R5 원 영수증](08-release-activation.md) |

병렬 실행 범위는 서로 다른 작성 파일의 코드 수정·검토와 승인 입력 준비다. 동결 후보가 준비되면 Q/Catalog 및 이관→R25·D 공급 이후 실제 설치 호출→P09/P10/P11을 기존 Root 큐로 보낸다. 같은 package·target·feature·selector·동결 입력의 terminal을 확인해 이미 끝난 본문을 한 번 인수하고, 달라진 입력의 회귀만 재선택한다.

**Writer 삭제 복구의 기존 FNL410 등록.** 기존 `fnl410-writer-deletion-fence-owner`는 writer 모듈의 일반 3건과 Unix prepare 1건을 선택한다. Ordinary alias drain·거절 및 Native reader 유지 exact 3건도 같은 `quanta-runtime.no-default.4a50f709b1a0`와 기존 Just 배치에 연결했다. 등록·선택·호출 경로 **4/4 PASS**, Unix Rust 본문 **0/7 NOT_RUN**이다. [S suite registry](/Users/songmin/Documents/code-new/semantica-codegraph-v2/tools/testing/suite-registry-v1.yaml:700) · [기존 Just 배치](/Users/songmin/Documents/code-new/semantica-codegraph-v2/Justfile:135)

**현행 오너 인계.** H는 [NC-H 현행 입력](../../nov-12-native-cutover/tickets/NC-H-history.md)에서 로컬 검증을 인수한다. 서명 설치 History와 ACK-loss는 [03.7](03-signed-published-cp2.md#history-03-7)에 있다. Ancestry CLI 세 실명은 기존 레인에 등록됐고 본문은 **NOT_RUN**이다. [A 06 수용](06-tantivy-retirement.md)

**현행 수용 범위.** 기존 Runtime 105개 실명은 원 실행 증거를 인수했다. 정식 출하 로스터는 **128개·17타깃·9패키지**다. 최종 커밋의 CircleCI 승인 old-Q pin을 적용한 뒤 오너들의 변경이 포함된 clean Q를 고정한다. 기존 오너의 실행은 package·target·feature·host·동결 Source·실행 파일이 일치할 때 해당 실명에 한 번 인수한다.

| 작업 | 기존 작성 범위와 먼저 할 일 |
| --- | --- |
| RepoMap 출하 제거 | [별도 티켓](../../nov-23-repomap-retirement/README.md)의 Semantica·Quanta Index·저장 퇴역·통합 범위를 사용한다. Root가 기존 FNL410·IFX150·paired R5 큐에서 해당 입력의 결과를 한 번 수용한다. |
| V6 제품·수명 | S A는 V6 FULL·V5→V6 DELTA·새 PID G3를 기존 IFX150에 고정한다. Pending과 V9는 signed SQL 246–248로 exact ACK 부모를 결속하고 253–255로 genesis와 census를 처리한다. 256–269의 seal·정본 스키마, cleanup CAS, 최신 Product head 검증, fence 재열기, release census 발급·소비와 Deleting Store 코드가 합류했다. 270–274는 Required archive와 스키마 대조, 275/276은 Runtime consume 콜드 재개의 후계다. 후계 서명 worker에서 V45/V46 재열기, V6 콜드 복구·최초 FULL과 cleanup fencing을 검증한다. daemon 운영 명령의 내구 writer release 연결, 실제 재시작·drain 검증, 참조 퇴역, 물리 회수와 최종 Cleaned가 남았다. |
| Linux·서명 공급 | 기존 Runtime의 Q 검증은 외장 bind를 복구했다. R25 v34는 현행 S/Q/GQLang·Cargo를 동결한 뒤 승인 Linux AArch64 ELF, frame, 기존 8 MiB 본문을 공급한다. D의 VM scratch는 승인 외장 볼륨을 선택할 수 있다. Docker.raw 내부 용량은 설치 입장 때 실측한다. 현행 차단은 P07의 고정 S·Q와 CURRENT intake 부재다. [현행 입장](../../../bugbash/nov-20-native-delta-cost/tickets/INC-03-product-acceptance.md#현행-판정--2026-11-25) D가 현행 CURRENT Linux Q/S/SQL 서명 설치를 공급하면 Root/F가 실제 공개 SDK/CLI Published·CP2·복구를 기존 호출로 실행한다. |
| Tantivy·출하 | Tantivy 제거와 출하 수집기 코드는 메인에 있다. Root/F는 기존 큐에서 최종 128개를 수집한다. Lexical 12건과 Runtime 43건은 최종 Q의 CircleCI 승인 old-Q와 x86-64 Linux를 고정한다. Lexical 오너는 그중 기존 유지 두 실명의 일치하는 PASS만 유지 원장에 인수한다. Root/F가 무의존 바이너리를 확인하고, 기존 E08 오너가 P09→P10→같은 ELF의 P11 실제 caller/kernel·운영 증거를 공급한다. 실패한 실명은 첫 오류와 변경 입력을 해당 오너에게 반환한다. [Lexical 코드와 실행](04-maintained-test-migration.md#lexical-코드와-검증-범위) |

**16:19 KST Root 추가 합류.** Q `bd0e54ec0`에 Native 품질 판정의 정책·심사 입력 경로 고정과 판정 직전 설치·설정 결속 검사를 합류했다. Python **354/354 · exit 0**이다. 정식 이관과 07 제품 실측은 최종 Q/S에서 수용한다. [판정 검증](07-product-qualification.md) · [Runtime 실행](04-maintained-test-migration.md)

11월 22일 최종 감사의 관측점은 로컬 Q `bd0e54ec0b0`, S `1626c47e5ea`와 관련 작업 트리다. Q `f08990c96`의 Tantivy 삭제, `1277bc645`의 미사용 Lindera 제거, `c41c48bdb`의 퇴역·paired R5 증거 결속, `74f751c70`의 P11 게시 수리, `bd0e54ec0`의 품질 판정 수리는 합류했다. 이후 Q `8e8a0a36a`에 서명 콜백 전후 입력 재검증을, S `3111ed37b90`에 V5→V6 재기준화를 합류했다. 정식 후보는 아래 기존 오너의 결과를 받아 별도로 동결한다. LanceDB는 Native semantic 검색의 현행 의존성이다. [퇴역 범위](06-tantivy-retirement.md)

| 기존 오너 | 남은 구현과 합류 범위 | 종료 조건 |
| --- | --- | --- |
| **B · PRODUCT4K** | Worker 중복 상태 조회 제거와 Kernel 분기, Catalog 준비·저장·복구의 원 budget 적용은 B 작업 트리에 있다. Catalog 집중 7/7 PASS. immutable operation 조회 기한 회귀는 Catalog lib 1/1 PASS다. Kernel 취소 경쟁·인접 DELTA **3/3**과 Runtime overlay **1/1**은 통과했다. 후기 Record 재접수는 admission 대기 중 `ENOSPC`로 종료됐다. 마지막 제품 본문은 materialization 1,200초에서 FAIL이다. | 좌측 코드의 집중·통합 회귀, 후기 병목 실측과 구조 수리, 원 admission의 동일 입력 ANN4K·whole SDK, overlay·두 번째 재시작 및 07.5 reclaim·fallback 확인. [B 원장](../../nov-21-bugbash-source-advance-4k/04-product-acceptance.md#현재-코드와-실행-판정) |
| **A · Q Graph/SDK, Root · S RepoMap DELTA · 보류** | 11월 22일 설계는 양방향 predecessor 조회와 Q의 같은 view·instance 영향 범위에 결속된다. 완전 번들의 미변경 output 참조·교차 파일 간선·순위 보존과 방문·직렬화·stage 수를 기존 G2에서 비교하는 계획이었다. | 이번 출하에서는 [RepoMap 통합 수용](../../nov-23-repomap-retirement/04-integration-acceptance.md)을 적용한다. [S 원장](</Users/songmin/Documents/code-new/semantica-codegraph-v2/docs/plans/oct-26-replay-query-read-custody/tickets/CURRENT_REMAINING.md>) · [Q 계약](../../../../crates/quanta-index-contract/src/native_protocol/graph/prior_file.rs) |
| **A · Semantic V6, 기존 SQL/SCV2 담당 협업** | S `1626c47e5ea`의 FULL·영향 SCC DELTA, `3111ed37b90`의 V5→V6 FULL 재기준화, `94b36bb3c47`의 명시적 V6 SDK 입구가 들어갔다. 설치 호출의 승인 선택과 첫 FULL의 SCC roster·shadow 조건은 기존 A/C가 대조한다. ACK 뒤 참조 해제·회수는 기존 Store/V9 오너가 맡는다. | signed SQL 244/245·reader allowlist의 동일 설치에서 V45 유지·rollback, 첫 V6 커밋·네 root, V5 replay와 V6 FULL→DELTA→cold 조회를 확인한다. Store 회수는 reader pin·재생·crash에서 검증한다. [S 원장](</Users/songmin/Documents/code-new/semantica-codegraph-v2/docs/plans/oct-26-replay-query-read-custody/tickets/CURRENT_REMAINING.md>) · [V6·V46 수용](06-tantivy-retirement.md) |
| **D · 서명·restore-forward, 기존 03.6/07.1** | Q `8e8a0a36a` 서명 전후 검증, S `d2997df20a5` 실제 교체 직전 검증과 기존 guest readback 검증을 합류했다. 새 후보의 D3 Q postimage 3/3이 일치한다. 물리 active sibling 거절 Rust 회귀는 기존 작업 트리에 있다. | 기존 sibling 회귀가 정상 primary를 허용하고 추가 active head를 거절하는지 본문을 확인한다. 같은 D3 서명 설치의 rollback·restore-forward·첫 ACK 유실 재시도 **3/3**을 수용한다. [03 서명·복구](03-signed-published-cp2.md) |
| **TEXT · SDK60/Large** | 이전 경로 계측 동결본의 두 Record와 Large 두 OS 재시작을 인수한다. 기존 strict quiet Off는 입장 전 거절됐다. 최종 후보에서 실패하면 해당 원 SDK의 첫 거절 단계와 시간으로 수정 범위를 정한다. | 07.3/07.4의 원 한도와 strict host 입장으로 Caddy·etcd 각각 Off 2회/Record 1회, Large 완주와 두 OS 재시작을 확인한다. [TEXT 원장](../../nov-12-native-cutover/tickets/NC-TEXT-build-final.md#current-nc-text-acceptance) · [07 제품](07-product-qualification.md) |
| **E08/P11 · 운영 등록과 증거 발급** | 게시 중 출력 디렉터리 교체를 거절하는 P11 발급 수리와 회귀는 Q `74f751c70`으로 로컬 메인에 합류했다. Python **208/208 · exit 0** 결과의 수리 파일 SHA를 합류 커밋과 대조했다. 운영 오너는 승인 Linux 타깃과 action·독립 observer·서명 키·attested inventory를 등록한다. | 제어 회귀와 별도로 동일 Linux P10 ELF의 paired R5 caller/kernel, 승인 대상의 배포·선택·복구 관측을 원 등록에 결속한다. [08 활성화와 운영 증명](08-release-activation.md) |

1. **병렬 구현 후 한 번 합류.** A/B/D와 필요한 TEXT 후속은 위 범위와 기존 exact를 사용한다. 패킷은 기준 SHA, 소유 파일, 제품 호출자, 기존 selector, 실제 terminal을 남긴다. E08은 기존 운영 입력을 병렬 준비한다. Root/F는 공용 Cargo·등록부를 합류하고 필요한 소스를 받아 Q/S·Cargo·SQL 입력과 Linux 후보를 한 번 고정한다. 이미 접수한 요청은 원 terminal·본문·정확한 입력을 대조해 재사용한다.
2. **기존 단일 실행 큐.** 04/06 출하 수용은 대체 **126개**와 old-Q 유지 **2개**를 합친 **128개 실명·17개 타깃**이다. 패키지·기능·호스트·old-Q 입력이 같을 때 기존 수집기 호출을 묶는다. `d8a490565`의 105건 및 `f08990c96`의 fast 2건은 각 출처에 귀속한다. 마지막 퇴역 검사의 Q `c41c48bdb`에서 소스·Cargo는 통과했고 전체는 이관 증거 부족으로 **exit 1**이다. 동일 Q/S의 최적화 daemon·CLI/SDK 산출물과 서명 번들에서 Tantivy 0을 대조한다. [04 실행](04-maintained-test-migration.md) · [06 퇴역](06-tantivy-retirement.md)
3. **제품과 활성화.** 동일 후보로 기존 TEXT가 Caddy 590·etcd 1,421의 원 **60초·파일 30,000·512 MiB·R/W/M 각 100만**, 각 quiet Off 2회와 Record 1회, Large 4,096파일·두 OS 재시작을 실행한다. D의 동일 서명 설치에서 복구를 확인하고, F-EVAL이 같은 view의 SDK 결과와 독립 블라인드 qrels로 Hybrid 대 lexical의 파일 MRR@10 **+0.02 이상**, 저장소 단위 95% 하한 **>0**과 07.6의 품질 가드를 판정한다. 각 본문 수·terminal·입력과 ELF/SQL/설정 신원을 Root가 대조한다. T·통합 A 및 코드 자격을 인수한 뒤 기존 E06 선택 CAS, 통합 B·P11 운영 증명과 최종 aggregate 순으로 진행한다. [07 제품](07-product-qualification.md) · [08 운영](08-release-activation.md)

## 이전 코드 착수 기록 · 11월 22일

아래 착수 기록의 후속 판정은 [현행 구현과 수용 순서](#현행-구현과-수용-순서)를 따른다.

Q `d8a490565e6f`의 작업 트리와 S `48afeb27a96f` 이후의 작업 트리를 기준으로 대조했다. 실제 쓰기는 아래 기존 오너가 맡고, Root/F는 공용 등록·동결·증거 인수를 맡는다. 착수 직전 같은 함수의 진행 중 패치와 같은 selector의 기존 요청을 확인한다.

| 우선순위와 기존 오너 | 실제 수정 또는 합류 | 종료 조건 |
| --- | --- | --- |
| 1 · 04 Runtime/D | `offline_state_custody.rs`에서 고정 old-Q로 `backup-state`를 호출하는 실패 경로의 입력 생성만 수리한다. `legacy_offline_refusal.rs`가 이미 쓰는 old-Q 기동·Source 발행·정상 종료 흐름을 재사용한다. `offline_live_root::build_live_root`를 쓰는 현행 Catalog 직접 검사와 제품 거절 규칙은 각자의 계약을 유지한다. 03B/C/D는 같은 fixture를 다시 작성하지 않는다. | Root #11 실패 7개 실명에서 원래 backup/restore/corruption 단언이 모두 통과한다. 필요한 경우 fixture를 공유하는 실명도 영향 범위에 넣고 기존 NoFeatures 후속에 연결한다. [첫 실패](03-signed-published-cp2.md) · [기존 old-Q 발행 본문](../../../../crates/quanta-index-searchd-runtime/tests/legacy_offline_refusal.rs#L1486) |
| 1 · 02 R25 공급 | v34 공급 제어가 Source lease 안의 compiler 식별 바이트를 rustc 측정과 terminal에 공유한다. Root 독립 Python 63/63, 설치 manifest 29/29, 변경 3파일의 patch replay와 CLI pre-import를 검증했다. | 현행 S/Q/GQLang·Cargo 동결과 게스트 Cargo 설정 대조 후 기존 공급에서 v34를 사용한다. 실제 Linux terminal·ELF, formal 및 8 MiB SDK 본문을 확인한다. [v34 인수](02-semantica-runtime-sdk.md) |
| 1 · Root/F 공용 Cargo | 메인 lock `da450067aba4…`에 vendored LanceDB의 Lindera 제거가 반영됐다. 전체 기능의 잠긴 오프라인 해석과 `cargo deny`를 통과했다. | 이 lock을 최종 출하 입력에 포함하고 기존 IVF-HNSW-SQ 255/256 경계 본문을 재접수한다. [의존성 검증](06-tantivy-retirement.md) |
| 병렬 · 기존 S A 발행 | V46 DB 마이그레이션과 signed SQL adapter가 있다. 진행 중 store 변경을 먼저 인수한 뒤 기존 Native Pending 트랜잭션과 `PendingCarrier`에 V6 root를 원자 결속하고 같은 artifact의 V9 수명을 연결한다. 이어 버전 있는 V6 root wire, ACKed predecessor의 cold read, path·symbol owner·cluster 인덱스를 연결한다. Child 보존·회수는 같은 원장에 연결한다. | 기존 V5 역사 입력과 V6을 각각 읽고, 같은 V6 Q/SQL 서명 설치에서 FULL→G2→새 PID와 변경 경로만 읽고 쓰는 비용을 검증한다. B는 기존 R5 assembler 경계를 맡으며 V6 writer는 기존 A 오너에 둔다. [V6 SQL adapter](</Users/songmin/Documents/code-new/semantica-codegraph-v2/packages/analysis/quanta-v2/crates/quanta-runtime/src/indexing_machine_v2/store/native_semantic_tree_v6/sql.rs:63>) |
| 검토 먼저 · 03B/C/D | CURRENT 후보는 D3 Source 검토와 승인·공급 단계에 있다. 이미 결선된 Published SDK/CLI와 CP2 승인 경로를 같은 입력에서 인수한다. 확정된 실패가 나온 코드와 새 mixed Source가 필요한 경계만 원 소유자에게 넘긴다. | 승인된 D3 Q ELF·S·SQL을 한 설치에 고정하고 공개 SDK/CLI·CP2·복구의 기존 실명과 원 terminal을 인수한다. [CURRENT](03-signed-published-cp2.md) |

04.4의 90개 유지 ID는 기존 대체 선택자 71개에 매핑돼 있다. 원장 기준 19개 ID는 `BLOCKED`, 71개 ID는 `NOT_RUN`이다. 기존 Lexical/TEXT·Runtime 오너는 같은 동결 후보의 선택자·실행 바이너리·inventory·terminal을 인계하고, 정확한 계약을 실행할 수 없는 항목을 확인하면 해당 leaf만 수정한다. Root만 공용 원장에 반영한다. Q 49/49, FileBody 포함 19/19와 Linux P10 후보 1/1은 입력이 같은 범위에서 인수한다. 정식 P09/P10과 동일 ELF의 P11 두 실명은 E08 오너의 기존 경로에 둔다. R5 및 기존 IFX150, P07은 등록된 실명으로 먼저 확인하고 재현된 실패를 원 오너에 돌린다. [04 이관](04-maintained-test-migration.md) · [08 출하](08-release-activation.md)

코드·승인 입력이 준비된 항목부터 기존 Root 한 큐에 입장시킨다. Q 실패 7개, CURRENT 검토·서명 설치, R25 arm64 공급은 원 승인 경계와 소스가 각각 다르다. R25는 새 ELF의 frame을 먼저 측정하고 `8,388,608 B`에서 Native 2·TS/JS 1·Java 7을 실행한다. 실행을 합칠 때는 package·target·feature·실명·동결 소스·바이너리가 같은 결과만 공유한다.

<a id="root-합류-실행표--11월-22일-1527-kst"></a>
## Root 합류 실행표

기존 Runtime·A 발행·CURRENT·E08 채팅이 해당 코드를 소유한다. 이 채팅은 공용 Cargo·suite 등록과 실행·출하 영수증 인수를 유지한다. 서브에이전트의 독립 대조를 받았고 같은 소스의 작성자 또는 중복 실행 큐는 추가하지 않았다.

| 레인 | 확인한 결과와 다음 작업 |
| --- | --- |
| 04 Runtime | **04.2 제품·runner 코드와 선택 본문 VERIFIED.** 제품 Rust 2단위, 물리 ENOSPC helper, Linux guest runner, exact resume/ordered runner를 코드 단위로 대조했다. 재개 검증에서 전체 입장 명령과 단계별 본문·종료 순서를 대조한다. 제어 회귀 **51/51 PASS**와 기존 Root 8→11→12의 **105/105** 원로그 재검증을 완료했다. 원 v5는 104건이며 Linux metadata 1건은 Linux-only 테스트 타입 수정의 파생 소스에 귀속한다. 퇴역 원장의 43개 매핑과 exact oracle 4개는 출하용 observation bundle이 없어 `NOT_RUN`을 유지한다. 정식 **128개·17타깃·9패키지**가 누락 held-reader 후보 2개를 포함해 별도 04.4 clean 후보에서 이를 닫는다. [코드 단위 종결](04-maintained-test-migration.md#042-코드-단위-종결) |
| Root/F 품질·Cargo | 메인 `1277bc645f6c`에 품질 판정과 Lindera 제거를 합류했다. lock `da450067aba4…`의 `cargo deny --all-features --locked --offline` 네 범주와 품질 회귀 349건이 통과했다. 동일 clean Q의 퇴역 검사는 소스·의존성 통과, 이관 실행 증거 부족으로 exit 1이다. 벡터 경계 Rust 본문과 같은 Q/S 출하 증거가 필요하다. [퇴역 검사](06-tantivy-retirement.md) |
| Root/F 04.4 정식 이관 | 최종 Q의 기존 수집 묶음에 Source·Symbol fast 두 실명, SDK22와 겹치는 `lexical_full_fidelity_matrix`·`l1_real_daemon_sdk_domain_primitives_and_symbol_windows`, Native 파일 삭제 회귀를 포함한다. 같은 package·target·기능·platform의 정식 결과를 공유하고 JSONL·inventory·source manifest·실행 파일·old-Q 사본을 보존한다. held 공개 조회 유지와 interrupted reclaim의 실제 제품 재시작 경계는 기존 04 오너가 대조한다. [정식 수집 순서](04-maintained-test-migration.md#최종-후보-수집-순서) · [기존 수집기](../../../../tools/ci/retirement_nextest.py) |
| S A V6와 Root QBC | 일반 open은 V45를 유지하고 최초 V6 commit에서 schema를 승격한다. signed SQL 이미지는 244·245와 새 reader allowlist가 필요하다. 기존 FNL410의 Runtime/Contract 로스터는 cold artifact lease까지 **15개 · 0/15 NOT_RUN**이다. 실제 V6 FULL, V5→V6 checked DELTA와 ACKed G3 cold replay는 기존 두 IFX150 exact에 연결됐고 **0/2 NOT_RUN**이다. 기본 설치 V6 선택과 동일 signed 실행, durable cleanup은 남아 있다. [V46 티켓](06-tantivy-retirement.md) |
| CURRENT·R25 | D3의 이전 동결 Source가 유실됐다. 현행 S/Q 재캡처·검토, D3 ELF·서명 설치와 readback이 필요하다. 내부 여유는 재캡처 51.10 GiB 및 복구 VM 53.48 GiB 기준 미달이다. 기존 D/공급 오너가 진행하며 R25 v34는 검토용 제어를 인수했다. R25와 P07은 승인된 실행 입력을 기다린다. [현행 용량·서명](03-signed-published-cp2.md) · [R25](02-semantica-runtime-sdk.md) |
| E08/P11 | 기존 E08의 P12A 동결 제어 **595/595 PASS**를 원 JUnit·inventory·run binding과 대조했다. 당시 입력은 `1277bc645`와 검토된 dirty digest다. E08은 이후 발행 경계 변경의 영향을 재검증 중이다. 정식 P09/P10과 동일 ELF의 P11 caller/kernel 본문은 남아 있다. [08](08-release-activation.md) · [P12A 원 binding](/private/tmp/quanta-p12a-r5-hardening.qo2SSD/raw/p12a-run-binding.json) |

**15:26 KST 병렬 인계.** 기존 B 채팅에 PRODUCT4K 후기 worker RCA와 원 4,096×64/Q64/K10·1,200초 본문 재검증을 전달했다. 기존 A Native Source·발행 채팅에는 RepoMap DELTA의 전체 순회를 변경·삭제 영향 범위로 줄이는 G2 작업을 전달했다. 두 오너는 기존 선택자와 실행권을 사용한다. Root는 두 결과의 적용 소스와 lock을 대조한 뒤 동일 Q/S 후보의 증거를 인수한다.

15:30 KST 공용 원장의 대체 후보는 Lexical 72개, Runtime 9그룹·독립 oracle 4개의 55개다. 파일·실명 기준 합계는 126개이며 공통 SDK matrix 1개는 같은 실행 증거를 공유한다. 진행 중인 105건의 실행기는 당시 Runtime 54건과 경로 집합에 고정돼 있다. 현행 원장으로 다시 입장할 때는 최종 Q에서 동일 타깃·feature·환경의 실명을 수집기에서 묶고 새 로스터를 대조한다. [Lexical 원장](../../../../tools/ci/inventory/lexical-test-disposition.toml) · [Runtime 원장](../../../../tools/ci/inventory/runtime-legacy-migration.toml)

**11월 22일 Root 큐 이력.** 이전 동결 Q의 SDK22 **22/22**, Source·Symbol fast **2/2**, Runtime 이관 **105/105**는 각 원 입력에 귀속한다. 최종 출하의 **128개·17타깃·9패키지** 실행과 H 설치 History는 별도다. 당시 잠금·용량·컴파일 재시도와 수집기/old-Q 결속 수리는 [복구 원장](../../../ARCHIVE-INDEX.md#2026-11-23-native-supplier-and-runtime-history)에 보존한다.

## 지금 병렬로 준비할 작업

**이전 정적 판정.** 최초 Lexical 91개 삭제 ID의 퇴역 분류는 재감사에서 수정됐다. 현행 **1개 퇴역·90개 유지 이관**과 원본 유지 **2개**의 ID별 판정은 [04.4](04-maintained-test-migration.md#044-id별-유지퇴역-판정)에 둔다. 당시 컴파일·대기·분류 결과는 [복구 원장](../../../ARCHIVE-INDEX.md#2026-11-23-native-supplier-and-runtime-history)에 보존하며 실제 출하 본문을 대신하지 않는다.

| 기존 레인 | 지금 맡을 작업 | Root 인계와 종료 조건 |
| --- | --- | --- |
| **R5 S SDK·Runtime · 06** | 기존 IFX150 FULL과 설치 SCV2의 exact Q rank·V9 ACK 회귀는 이전 `Required` 입력에 귀속한다. FULL과 ACK 이후 재시작 exact 두 건의 기존 `--resolve-only`를 확인했다. 실제 이전 SDK 본문은 **NOT_RUN**이다. [S R5 감사](/Users/songmin/Documents/code-new/semantica-codegraph-v2/docs/plans/oct-26-replay-query-read-custody/tickets/CURRENT_REMAINING.md#native-repomap-ack-audit-20261122) | 새 후보는 같은 P10 ELF에서 실제 S→Q 호출·재시작, 유지 검색, RepoMap 보조 결과 0과 ACK 재시도 중 추가 게시·CAS 0을 확인한다. [통합 수용](../../nov-23-repomap-retirement/04-integration-acceptance.md#실제-본문과-통과-조건) |
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

삭제 전에 이전 호환 서명 Q/S 설치 쌍과 삭제 전 Native의 실행 파일·호출자·설정·입력을 각각 고정한다. 이전 설치 쌍은 실제 선택·복구 입력까지 확인한다. 현 checkout에는 정식 DSL warm/cold 비교 파일이 없다. Legacy 기준을 다시 측정할 수 있도록 고정 old-Q 생산자를 확보하고, 최종 Native 후보에서 기존 `benchctl.py run dsl-authority --admit-baseline`으로 호환 기준선을 준비한다. 설정·코퍼스 digest가 바뀐 경우 기존 비교기의 일치 조건을 유지하고, 엔진 간 공통 기능 비교는 별도 원 `AB`/`BA` 계약으로 검증한다. 03의 서명 계약과 fixture는 재사용하고, 각 본문 결과는 그 동결 후보에 귀속한다. [DSL 기준선 계약](../../../../tools/benchmark/README.md#dsl-warmcold-gate) · [원 비교 계약](07-product-qualification.md#출하-벤치와-비교-입력)

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
| 입장 전 | Q Cargo 동결 | 메인에서 검증한 lock `da450067aba4…`를 출하 입력에 포함한다. 기존 벡터 경계 본문은 앞선 Root 실행의 종료 뒤 같은 입력으로 이어 간다. [06 의존성](06-tantivy-retirement.md) |
| 1 | NoFeatures 26 → 최종 후보 Q Lexical 27 → UDS 3 → Catalog 19 | NoFeatures #12의 26/26과 기존 두 Q 후보의 49/49를 원 동결 입력으로 인수했다. 새 D3·Cargo 동결을 이전 본문과 대조하고 달라진 범위만 기존 Root에서 실행한다. Catalog는 같은 Source·캐시의 정확한 30 PASS를 받아 기존 `--catalog-after`로 연결한다. [원 종료](01-q-regressions.md) |
| 2 | Lexical DATA·Native UDS·고정 old-Q 집중 27, Runtime Perf·내부 oracle | Lexical 로컬 결과는 BOOT 5/5, lib 2/2, 고정 old-Q DATA 15/15다. 마지막 DATA 실명은 단언 수정 뒤 1/1 PASS했다. 출하 원장은 동결 소스와 원 실행 증거를 인수하고, Linux 전용 분기는 기존 Linux 레인에서 본문을 확인한다. 실제 Linux metadata 포트, FileBody 7개와 Catalog의 pending seed·다중 incident도 확인한다. 같은 동결 입력에 중복되는 실명은 결과를 공유한다. 원 ID별로 실제 exact PASS를 대조한다. [04.4](04-maintained-test-migration.md#044-id별-유지퇴역-판정) |
| 3 | Semantica R5·IFX150, 신규 ELF 8 MiB R25 | R5 Runtime exact와 실제 IFX150 재시작을 기존 S QBC에서 인수한다. 기존 `test-native-delta-cold-owner`의 두 Runtime owner 선택 및 signed cold DELTA를 같은 레인에서 실행한다. RepoMap 키 비용, semantic snapshot 3건(기존 병합 실명의 changed-scope 검증 비용 포함), V5 buffer 이관 5건, writer wire 및 completed V5 검증 재사용 1건, 선행 semantic readback 2건, 부팅 transfer marker·orphan inventory·Store 퇴역 3건은 기존 Runtime 발행 배치에서 인수한다. Orphan inventory는 단일 본문 검증과 marker 재대조를 보강한 현행 exact를 사용한다. Semantic G3 회귀는 정본 owner 범위, shadow-off 검증, 교차 corpus 중복 거절을 포함한 현행 소스를 동결한다. 현행 managed ItemIndex 및 V5/V9 readback 수리를 포함해 기존 FULL, 두 번째 DELTA terminal, Pending-before-writer-markers를 동일 Q/SQL 서명으로 동결한다. Required RepoMap의 V9 재조회는 같은 기존 checked DELTA terminal과 Q Published-before-ACK 두 실명에 포함한다. Fresh Native issuer의 snapshot 보유 변경은 같은 checked DELTA 비용 matrix와 기존 cold ACK·Q Published 복구 입력에 포함한다. RepoMap ACK marker Original 입장 변경은 기존 FULL same-target, FULL ACK cut과 G2 checked/cold 두 cut에 포함한다. 선행 Required V5 재사용과 WriterState 입장은 기존 G2 두 실명 및 기존 WriterState marker exact에 반영한다. Cold current Required 보존은 checked DELTA·Q Published-before-ACK·cold ACK의 기존 세 실명과 같은 signed 설치에 포함한다. V5 승격은 기존 pending-before-writer-markers signed 실명과 기존 Runtime 발행 배치의 inode exact로 확인한다. R25는 현행 S/Q/GQLang과 Cargo를 고정해 arm64 ELF에서 프레임을 먼저 측정하고 Native 2, TS/JS 1, Java 7을 실행한다. [IFX150](06-tantivy-retirement.md) · [R25](02-semantica-runtime-sdk.md) |
| 4 | 현재 서명 설치의 실제 공개 SDK·CLI Published, CP2 | D3 이후 Q와 승인 S/SQL을 같은 설치에서 사용한다. 공개 7건, Caller 3건, signed SDK 22건, CP2 정상·mutant 및 Host 11건, retained 2건은 기존 목록을 따른다. [03](03-signed-published-cp2.md) |
| 5 | P09 → P10 → P11 paired R5 | 승인 Linux daemon에서 P10 exact 실행과 4회 daemon 관측을 받은 뒤 같은 ELF·깨끗한 Q/S로 P11 caller/kernel을 실행한다. 운영 P11의 승인 입력은 08에 귀속한다. [08](08-release-activation.md) |

출하용 128개 이관 증거는 기존 Root 큐의 동결 Q checkout에서 `python -m tools.ci.retirement_nextest --capture-roster`로 모은다. 전체 `--revision`, 외부 `--evidence-dir`, 기존 `--lane`을 지정하면 커밋에서 9개 exact 배치를 파생한다. 같은 증거 디렉터리에서 재개할 경우 검증된 배치는 다시 실행하지 않는다. 개별 오너 회귀는 기존 `--package`, `--lib`·`--test`와 `--filterset`을 사용한다. 실행기는 한 번 빌드한 바이너리와 같은 feature의 Cargo metadata를 수집·실행에 재사용한다. `QUANTA_RETIREMENT_NEXTEST_EVIDENCE_DIR`에는 출력된 증거 ID의 `inventory.json`, `nextest.jsonl`, `source-manifest.json`, `binaries.json`, `cargo.json`과 `target`의 해당 실행 파일을 보존한다. 원장 `run_id`는 출력 파일명의 증거 ID다. manifest는 실제 명령과 package·타깃·feature·filter를 기록한다. old-Q가 사용된 실행은 `target/legacy-searchd`까지 같은 증거 폴더에서 보존한다. 출하 검사는 원장 2개를 제외한 Git tree, 본문, 보존 실행 파일의 SHA를 다시 대조한다. Runtime event는 현행 Cargo 타깃과 모듈 이름으로 인수한다. [증거 생산기](../../../../tools/ci/retirement_nextest.py#L164) · [Runtime event 검사](../../../../tools/ci/lint/check-runtime-legacy-migration.py#L171)

기존 Runtime04의 PID 50363과 앞선 요청은 Cargo 입장 전 종료돼 본문 **0건**이다. 후속 동결 후보의 NoFeatures **26/26**은 원 입력에만 귀속한다. 종료·용량·Docker 대기 이력은 [복구 원장](../../../ARCHIVE-INDEX.md#2026-11-23-native-supplier-and-runtime-history)에 보존한다.

| 순서 | 실행과 실제 의존성 |
| --- | --- |
| **설치 입력 및 집중 본문 인수 · 03/04/05/06** | 03의 정확한 Linux Source는 기존 preimage 확인으로 재인수한다. 원 입력이 없어졌으면 03.1의 기존 supersession으로 대체 Source를 검토·등록한다. R5 실제 게시자·kernel, F15 수정본, DSL 유지 caller는 기존 요청·selector의 실제 결과를 받는다. 실패는 최초 단계의 원 terminal로 확인하고 영향받은 본문만 재실행한다. [03.1 입력](03-signed-published-cp2.md#완료-조건) |
| **삭제 후보 수용 · 06** | 합류된 Q/S 소스·Cargo와 타깃별 daemon·SDK·CLI·설정의 SHA를 고정한다. 통과한 `just rust-check-tantivy-retirement`와 all-features 소스 검사를 인수하고, 실제 릴리스 컴파일·번들의 Tantivy 0을 확인한다. 서명 설치에서 R5, 기본 Native 호출과 구 요청 거절, History, FULL/DELTA·재시작·ACK 유실 재시도·CP2 및 첫 durable commit 전 rollback/이후 restore-forward를 원 기동으로 검증한다. |
| **기존 한도의 제품 실측 · 07** | TEXT가 quiet 호스트에서 Caddy 590·etcd 1,421의 첫 Off를 확인하고 같은 후보의 각 Off 2회·Record 1회와 Large 4,096·두 OS 재시작을 채운다. 원 30,000 물리 파일·512 MiB WorkBytes·읽기/쓰기/변경 각 100만·SDK I/O와 submit→FULL ACK 60초를 적용한다. 설치본에서 F-EVAL이 같은 view의 SDK 후보와 독립 qrels로 원 Hybrid 승격 지표를 판정한다. 원 E07·ANN·Hybrid960과 두 기준 각각 `AB`/`BA` 5쌍을 실행한다. submit→ACK와 조회 p95의 짝비 중앙값은 각각 1.05 이하다. |
| **기본 선택 · 08** | 동일 후보의 03 설치 복구, 05·06·07과 통합 A 및 `CODE_QUALIFIED`를 확인한다. 코드 자격에는 Q P10과 동일 ELF의 `p11-cross-repo-cutover`가 포함된다. 승인 타깃에서 배포를 관측하고 E06 fence·CAS를 수행한다. 통합 B 운영 증명까지 받은 뒤 최종 aggregate를 검증한다. quiet Off·기준선 채집은 다른 부하와 시간을 겹치지 않게 한다. |

입장 전 호스트 부하·잠금·서명·기준선 누락은 `BLOCKED`로 기록하고 독립 준비를 계속한다. 컴파일 실패·실행 중 한도/기한·ACK/재오픈/trace 오류·품질 수치 미달은 실패한 범위에서 `FAILED`로 기록한다. 최초 거절 단계와 원인을 해당 작성자가 수리하고 기존 selector부터 재검증한다. Hybrid 미달은 정답의 후보 풀 누락과 융합 순위 손실을 분리한다. 순위 조정은 DEV로 돌아가며 다음 승격은 새 독립 EVAL로 판단한다. 결과는 각 티켓에 Source·요청·정확한 본문·exit와 함께 인계한다.

## 티켓별 종료 표

완료된 04.1·04.2와 06.2·06.3은 ADR에서 인수한다. [Runtime 이관 ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#completed-runtime-migration) · [Native 퇴역 ADR](../../../adr/NOV-12-001-native-query-and-semantic-engine.md#engine-composition-and-retirement)

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
| [04](04-maintained-test-migration.md) `04.3`, `04.4` | Lexical 테스트 작성자 | adapter 의존을 끊은 게시 fixture와 boot/file authority, 공개·물리/Work 검증의 유지·퇴역 목록 및 DATA-only 보존. |
| [04](04-maintained-test-migration.md) `04.5` | Root/F Q 실행 | 정확한 NoFeatures target과 중복 없는 test authority 등록, 유지 본문별 실제 PASS. |
| [05](05-bench-callers-hosted-ci.md) `05.1`, `05.2`, `05.5` | Harness 작성자 → Root/F 등록·기존 벤치 실행 | 외부 프로세스로 이관한 Hash/OpenAI 두 arm, 기존 실측 입력·provider, SDK publish/query와 요청별 계측/결과. |
| [05](05-bench-callers-hosted-ci.md) `05.3`, `05.4`, `05.6` | S/SDK 실제 호출자 → Root/F DSL | 33행·실제 caller·독립 gold 및 집중 본문으로 warm/cold 생산자와 공식 registry/CI 변경을 삭제 후보에 합류. 동일 Linux의 원 warm **200**/cold **20** 비교는 최종 후보에서 수용. |
| [05](05-bench-callers-hosted-ci.md) `05.7` | CI 공급 오너 → Root/F hosted 검증 | 승인 old-Q의 hosted 위치·크기·SHA·TLS와 실제 PR/main·correctness 원 결과. 공급 확인과 hosted 실행을 따로 기록한다. |
| [06](06-tantivy-retirement.md) `06.1`, `06.4` | Root/F 단일 후보 조립 | 유지 입력과 두 비교 기준을 동결한다. 전체 workspace와 실제 Linux release·번들의 Tantivy 0을 최종 후보 해시에 결속한다. |
| [06](06-tantivy-retirement.md) `06.5` | Root/F signed 설치, D/H 복구 증거 공급 | 같은 후보의 실제 기본 Native 호출·구 요청 거절·보존 경로 복구 본문. 08.1은 이 결과를 인수한다. |
| [07](07-product-qualification.md) `07.1`, `07.2`, `07.6` | D 서명 계약 → F Hybrid 소비/F-EVAL → 독립 심사 결과 인수 | 실제 동일 설치·동일 view·Q ELF/선택/BOOT 결속과 typed 거절, 실제 blinded 후보·독립 qrels의 원 캡처/파일 universe 결속, 원 수치 기준을 만족한 EVAL 판정. |
| [07](07-product-qualification.md) `07.3`, `07.4` | 기존 TEXT → F 최종 후보 인수 | 원 Caddy·etcd 한도와 각 Off **2**/Record **1**, 실제 ACK/조회/reopen 및 완전한 trace와 Large **4,096**·두 OS 재시작 본문. |
| [07](07-product-qualification.md) `07.5` | 기존 B/ANN → F 출하 인수 | Native LanceDB ANN4K와 whole SDK의 같은 소스·resolver, 독립 exact와 층별 기준, old pin·overlay·두 번째 재시작·실제 reclaim 및 Legacy fallback 0. |
| [07](07-product-qualification.md) `07.7`, `07.8` | F 기존 성능·비교 실행 | signed Mixed/E07·Hybrid 960초와 부모/자식 terminal, 보존한 두 비교 기준 **각각** AB/BA 5쌍 및 원 1.05/절대 SLO 판정. |
| [08](08-release-activation.md) `08.1`, `08.2`, `08.3` | F T 후보 → 기존 E01–E07/IDE 공급 → E08 | 같은 격리 설치 후보 T 결과, R01–R40 원 요구와 적용 N/A, 실제 client/IntelliJ 포함 기존 필수 증거, 동일 출하 입력의 `rust-policy` 통과와 통합 A 승인. |
| [08](08-release-activation.md) `08.4`, `08.5`, `08.6` | 기존 P11 운영 입력 → E06 전환 → E08/P11, crate 오너 → Root 정책 게이트 | 승인 target/action/observer와 attestation, E06 실제 fence/선택 CAS/기본 호출, 통합 B 및 네 출하·활성화·복구 상태. 같은 출하 SHA의 hosted CI·API/module·workspace와 `rust-policy` 결과, 루트 보존 상태. |
| [08](08-release-activation.md) `08.7` | E08/F 마감 | 원 검증 결과와 01–09 현행 상태를 원장에 결속. 필수 실패·입력 차단·미실행의 잔여가 없을 때 기존 티켓을 닫는다. |
| [09](09-native-search-gaps.md) `09.1` | SDK·caller 결정 → TEXT planner → Root/F 등록 | Native/퇴역 결정 기록, Native 데몬에서 실행한 기본 `.text()` 대체 본문 또는 거절 계약, `rg` 독립 정답과 repo·lang 범위 결과. |
| [09](09-native-search-gaps.md) `09.2` | TEXT → 07 정책 기록 | A/B 결정과 목표 코퍼스·검증량 수치, 고정 4,097파일 fixture의 공개 SDK·CLI 결과, 갱신된 경계 회귀의 재통과. |
| [09](09-native-search-gaps.md) `09.3` | Bench → F 인수 | 게이트 여부 결정, 실행 전 고정한 Zoekt 입력·질의 대응표·비율 합격선, 정확성 선행 통과 질의의 AB/BA 5쌍 결과. |
| [09](09-native-search-gaps.md) `09.4` | TEXT·Runtime 세션 (P1) | 페이지별 posting·본문 방문 계측과, 골든을 유지한 커서 결속 재사용. |
| [10](10-native-ide-agent-gaps.md) `10.1`–`10.3` | TEXT·SDK·Runtime 오버레이, 10.2는 Semantica 심볼 생산자 (P1) | 매치 위치 커서 계약과 `rg --json` 바이트 대조, 심볼 대체·퇴역 결정과 수용, `(repo, path)` 오버레이 충돌 키와 수용. |
| [07](07-product-qualification.md) `07.9`, [02](02-semantica-runtime-sdk.md) `02.6` | Catalog·Kernel GC / Semantica Source 생산자 | pin 해제 연속 실패 뒤 재시작 없는 회수 결과 / 수집 범위·제외 사유 manifest의 독립 대조 결과. |

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
| 고정 macOS old-Q SDK 유지 회귀 | `01a1f991-2f97-7092-8d3c-6434d1dfa012` | SDK22 **22/22**, Source·Symbol **2/2 PASS** 원 실행 인수. [코드와 결과](04-maintained-test-migration.md#legacy-sdk-custody) |
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

PID 50363은 입장 전에 **exit 143 · 0/26**으로 종료됐고 후속 #12는 **26/26 · exit 0**이다. Q **27→UDS 3→Catalog 19**는 원 동결 후보의 **49/49**를 보존한다. 최종 입력에서 영향을 받는 실명은 기존 Root에서 실행하고, 새 Catalog는 같은 Source·캐시의 Q/UDS 30 PASS와 exit 0을 확인한 뒤 `--catalog-after`로 연결한다. R25, CURRENT Published·CP2, formal P10·P11은 각각의 승인 입력이 준비된 기존 레일에서 인수한다. [Q](01-q-regressions.md) · [R25](02-semantica-runtime-sdk.md) · [CURRENT](03-signed-published-cp2.md) · [P10/P11](08-release-activation.md)
