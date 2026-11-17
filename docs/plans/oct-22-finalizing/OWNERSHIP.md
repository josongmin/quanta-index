# 공용 파일·수정 경계·인계 규칙

현재 작업과 작성자는 [8개 에픽](README.md#epics), [Native 전환 인수](../nov-12-native-cutover/README.md#레인별-착수와-인계),
[C0/P09 B05](../nov-12-bugbash-c0-p09/tickets/B05-final-verification.md)가 소유한다.
이 문서는 고정 수정 규칙과 수용 경계만 유지한다. 10월 채팅·PID·claim·패킷 적용 순서는
[원문 복구 색인](../../ARCHIVE-INDEX.md#2026-11-13-ownership-and-ticket-consolidation)으로 접었다.
과거 배정, 부모 채팅의 idle/completed 상태와 source-only 결과는 현재 수정권이나 완료 판정이 아니다.

## Exclusive boundaries

| 계약 경계 | 고정 수정·소비 경계 |
| --- | --- |
| E01 producer·Chunk·Wire | 기존 query/declaration/TextBasis·canonical recipe/IR·paid planner/emitter를 재사용한다. private plan/문서/Wire bank를 분리하고 parent/export/dispatch는 통합 작성자 한 곳에서 결합한다. |
| E02 publication·terminal | 기존 writer/generation·FinalDocs/vector·SearchPlan/root bank와 Parent/reconcile/receipt를 소비한다. upper caller가 lower engine·collector·Source issuer를 중복 구현하지 않는다. |
| E01/E06 공개 SDK | Facts/recompute·whole-error·공개 게시/install/recovery를 기존 canonical producer와 결합한다. 실제 caller DATA·최고 Source finisher·winning/losing cause의 수명을 보존한다. |
| E04 Query·ANN | Contract/Kernel/Runtime composition을 exact hunk로 결합한다. 기존 resident builder와 child-first installer를 소비하며 두 번째 directory/seal/query/worker를 만들지 않는다. |
| E02/E07 physical | Catalog/Common이 file·admission·FD·capacity·retirement를 소유한다. Query/SEARCH가 물리 owner나 ledger를 복제하지 않는다. |
| E05/E06 AST·BOOT | 실제 Source·FULL evidence와 기존 AST/SQL/apply·BOOT/launcher 소비자를 연결한다. 새 target issuer/launcher나 조기 default 전환으로 대체하지 않는다. |
| E07/E08 수용 | 원 oracle·운영·영향 검증의 결과를 집계한다. 동일 테스트·부하를 다른 이름으로 중복 실행하거나 focused 결과로 전체 수용을 닫지 않는다. |

## Collision resolution

### Parallel implementation schedule

- 변경 전 HEAD·dirty hunk·실제 producer→caller→consumer와 기존 작성자를 확인한다.
- private 파일/함수, 공용 접점, 불변식, 영향 회귀, 인수 시점을 정한 뒤 나눈다. 같은 파일의 독립 hunk와 같은 불변식의 충돌을 구분한다.
- 공용 parent/export/Cargo·fixture mount는 한 작성자가 결합한다. 실제 충돌 hunk만 조율하고 다른 작성자의 변경을 덮어쓰지 않는다.
- 기존 source·완료 수리·oracle을 먼저 소비한다. 과거 패치 번호, 실패 수나 미구현 표현만으로 재적용·재구현을 발주하지 않는다.
- 외부 채팅 연락·새 작업 배정은 현재 사용자 승인과 상위 지시 범위를 따른다. 날짜별 문서의 이전 승인을 현재 권한으로 전용하지 않는다.

## Vector stage cutover

<a id="vector-stage-cutover"></a>

기존 `index_projection_writer/`의 고정 접합은 아래와 같다. 이는 과거 W1/W2a를
새 작업자로 재배정하는 표가 아니며 실제 writer·hunk는 변경 직전에 대조한다.

| Source 접점 | 유지할 결합 |
| --- | --- |
| `vector_stage_v3.rs`·전용 child/tests | 단일 retained map, prepared update/undo, admitted read와 exterior Native birth를 제공한다. |
| `mod.rs`·`structured_store/retained_doc_v3.rs` | vector field/constructor·stage/read/discard·payload seal을 같은 owner에 연결한다. parallel ledger나 ordinary retain을 Native authority로 대체하지 않는다. |
| `commit_prepare_vector_payloads.rs`·Native read·`prepared_commit_artifact_v2.rs` | 두 reader의 실제 authority, 명시 empty/missing 구분과 canonical bytes를 보존한다. hydrate는 전체 candidate 준비 후 적용하고 undo를 유지한다. |
| `native_job_acceptance_v3.rs`·`commit_finalize/publish_store_mutations.rs` | 같은 serial에서 full receipt·highest Source를 확인하고 visibility 전 fallible 준비→checked apply·rollback을 결합한다. publication 후 새 fallible COW 삭제를 하지 않는다. |

vector field 타입·publication undo·hydration 반환형은 파일이 달라도 하나의 접합 계약이다.
공용 parent/Cargo/lock은 통합 작성자가 직렬 결합한다. structured Vector early-return과
ordinary empty records, foreign receipt·cancel·late Source·hydrate conflict·activation rollback·환급의
고정 oracle은 [E01 Vector 계약](E01-producer-generation.md)에 보존하며 시나리오를 축소하지 않는다.

## Compiled handoff owner map

인계에는 실제 path/function·입출력과 typed failure, Source/Original·DATA·grant의 수명,
적용된 hunk와 필요한 공용 결합, 실행한 명령·결과·미검증 범위를 남긴다.
정적 확인, 컴파일, 테스트 본문, installed/Product 수용을 구분한다.
관련 입력이 유지된 결과는 재사용하고 변경된 proof closure만 재검증한다.
원 실행 레인·canonical 입장·동일 시도 복구를 유지하며 다른 owner의 실행이나 잠금을 중복·우회하지 않는다.

## E02 L1 and provider boundaries

<a id="e02-l1-final-owner-map-20261030"></a>
<a id="e02-l1-owner-boundary-20261030"></a>

L1 A/B/C의 private 의미 소유권과 D의 단일 통합·실행 큐를 구분한다.
P60 D의 source 소유권과 L1-D 실행권도 별개다. 실제 caller는
[E02 계약](E02-publication-recovery.md#code-residuals-20261105)을 따르며,
source별 판정은 [L1 검증](VALIDATION.md#e02-l1-d-closeout-20261031)을 따른다.
prepared/accepted selector는 실행 결과가 아니며 L1 통과를 SDK seed·cold·Graph·Watch·Product 수용으로 합산하지 않는다.

<a id="e02-provider-c-boundary-20261030"></a>

[Provider contract](E02-publication-recovery.md#configured-native-provider-status-20261030-c)의 adapter/codecs·provider DATA와
실제 configured HTTP transport·공용 SDK transport-attempt·무거운 QBC는 해당 기존 owner가 결합한다.
P05 prior identity/seed/installer/terminal은 DELTA producer 범위이며 Provider로 재배정하지 않는다.
Voyage HTTP, QE predecessor 재인증, ItemIndex L1과 Graph/L3/Watch는 서로 다른 소유 경계다.
미공급 transport는 typed refusal로 남기고 Ordinary 결과나 source-only 확인으로 대체하지 않는다.

## W205 acceptance boundary

<a id="w205-closeout-20261030"></a>

[E01](E01-producer-generation.md)·[원 DoD](DOD.md)는 다음 기존 여섯 목표를 보존한다.
아래는 새 티켓·진행률 분모나 현재 완료 판정이 아니다.

| 목표 | 유지할 실제 수용 |
| --- | --- |
| O1 same-Source SDK | Linux SDK7 = BC2/FT1/Python4. 실제 admitted Source와 유효 Mac predecessor·source ledger가 일치해야 한다. |
| O2 initial Java | 공개 Java5 = SourceOnly1 + Maven/Gradle2 + JAR positive/missing2. Store 미게시 precommit model2는 별도이며 공개 수용을 대신하지 않는다. |
| O3 public Strong | Strong2의 Complete/MayAlias/NoAlias·정확한 stale cause·독립 ready digest·retained terminal을 유지한다. wire17은 SDK/Product를 대신하지 않는다. |
| O4 alloc/copy consumer | Runtime Full-floor3와 실제 public consumer를 확인한다. 과거 DFG11·다른 Source 결과와 합산하지 않는다. |
| O5 installed provider | 실제 installed paid Product-port의 typed model/metadata failure1과 losing DATA를 확인한다. unit bank·missing-JAR 거절은 이 수용을 대신하지 않는다. |
| O6 aliases/refund | same-key A/B Runtime1의 7축 absolute zero·B/writer 생존과 실제 Strong/installed alias·환급을 각각 확인한다. 빈 shard A/B fixture는 public SDK/Product가 아니다. |

Chunk6·packet priority2·영향 C1 full-failure module5도 원 수용 범위에 남는다.
W205 source 작성과 L1-D 실행은 구분하고 source/dependency/feature/OS/installed 입력을 맞춘다.
해당 source의 결과는 [기존 검증](VALIDATION.md#w205-proof-20261030)을 따른다.
FNL410의 실행·수락은 [원 티켓](/Users/songmin/Documents/code-new/semantica-codegraph-v2/docs/plans/oct-22-finilizing-semantica/tickets/FNL-410-public-analysis-semantics/TICKET.md#current-remaining-work)과
[ADR-0091](/Users/songmin/Documents/code-new/semantica-codegraph-v2/docs/adr/ADR-0091-fnl410-single-execution-and-acceptance-owner.md)이 소유하며 이 문서에 별도 큐를 만들지 않는다.

## Historical section links

아래 이전 앵커는 링크 복구용이다. 당시 exact hunk·채팅·적용 순서·실행 수는
[검증 원장](VALIDATION.md)과 [압축 전 원문](../../ARCHIVE-INDEX.md#2026-11-13-ownership-and-ticket-consolidation)에서 확인한다.
현재 배정은 문서 첫머리의 활성 원장을 따른다.

<a id="2026-10-20-native-dependency-and-funded-outer-join"></a>
<a id="delta-final-parallel-boundaries"></a>
<a id="delta-integration-hunks"></a>
<a id="e02-b-final-write-boundary-20261030"></a>
<a id="e02-current-wave-hunks-20261026"></a>
<a id="e02-independent-chats-20261028"></a>
<a id="e02-independent-finish-dispatch-20261030-b"></a>
<a id="e02-owner-exclusion-20261029"></a>
<a id="e02-supplier-hunks"></a>
<a id="e04-source-data-collision-20261027"></a>
<a id="e04-stale-delta-batch-20261027"></a>
<a id="e07-catalog-confirm-replay-writer-20261026"></a>
<a id="e07-checkpoint-rca-writers-20261027"></a>
<a id="e07-closeout-ownership-20261030"></a>
<a id="e07-conditional-resource-hunks-20261031"></a>
<a id="e07-confirming-input-queue-writer-20261026"></a>
<a id="e07-entry-map-exclusive-packets-20261103"></a>
<a id="e07-fulltail-rca-20261028"></a>
<a id="e07-isolated-regression-rca-writers-20261026"></a>
<a id="e07-live-owner-exclusion-20261030"></a>
<a id="e07-native-retirement-race-writer-20261026"></a>
<a id="e07-native-stack-and-callback-rca-20261027"></a>
<a id="e07-nonoverlap-boundary-20261030"></a>
<a id="e07-owned-grant-trace-writer-20261026"></a>
<a id="e07-pager-counter-ownership-20261028"></a>
<a id="e07-renewal-followup-writer-20261026"></a>
<a id="e07-retirement-io-diagnostic-writer-20261026"></a>
<a id="e07-scoped-scan-reuse-20261027"></a>
<a id="e07-sync-rca-tools-writers-20261026"></a>
<a id="e07-wal-policy-writers-20261103"></a>
<a id="e07-write-boundary-cost-20261027"></a>
<a id="e07-write-boundary-readout-20261028"></a>
<a id="e07-writer-cache-policy-ownership-20261028"></a>
<a id="native-query-closeout-writes"></a>
<a id="pta-copy-cohort-next-cut"></a>
<a id="r136-current-owning-cocut"></a>
<a id="r165-six-item-goal"></a>
<a id="r76-installed-sdk-whole-product-pta-witness"></a>
<a id="r81-initial-java-co-cut"></a>
<a id="runtime-type-closeout"></a>
<a id="shared-files"></a>
<a id="w205-r288-deduplicated-current-ownership-20261027"></a>
