# E04 — Semantic native incremental·ANN 제품

상태·담당은 [E04 원장](README.md#e04)이 소유한다. 이 문서는 R18–R22,
원 ANN4K 수용 조건, 구현 재사용 계약과 남은 제품 경계를 보존한다.
2026-10-22~25의 전체 구현·실행 이력은
`git show ade5fadc1a084fbb81eba831861e996081d80867:docs/plans/oct-22-finalizing/E04-semantic-ann.md`
로 복구한다. source별 판정은 [VALIDATION](VALIDATION.md), 신규 자체 ANN
구현·출하 후보는 [NC-B](../nov-12-native-cutover/tickets/NC-B-semantic.md)와
[NC-F](../nov-12-native-cutover/tickets/NC-F-integration.md)가 소유한다.
[Native query·Semantic 결정](../../adr/NOV-12-001-native-query-and-semantic-engine.md)은
현재 설계 계약이다. 해당 ADR이나 집중 회귀를 설치·성능 수용으로 계산하지 않는다.

<a id="current-rca--2026-10-28"></a>
## Acceptance and RCA routing

원 ANN4K Off/Record는 고정 `5c6d0f8a` 코호트에서
[VERIFIED](VALIDATION.md#e07-wal-frame-product-20261031)다. Q64, cold reopen,
선택 경로, delta, overlay를 포함한다. 해당 바이너리·recipe·소스의 증거이며
11월 12일 자체 HNSW/파일 결과/출하 후보의 제품 수용으로 전이하지 않는다.
신규 소스는 owning 본문, 실제 SDK·daemon 제품, E07 성능·운영을 각각 판정한다.
새 구현의 현재 상태와 제한은 [NC-B](../nov-12-native-cutover/tickets/NC-B-semantic.md),
출하 결합은 [NC-F](../nov-12-native-cutover/tickets/NC-F-integration.md),
전체 합격선은 [전환 DoD](../nov-12-native-cutover/DOD.md#native-lancedb-ann-검증)를 따른다.

10월 SourceAdvance·worker throughput 실패와 단계별 원인·수리 결과는
[VALIDATION](VALIDATION.md#e07-fulltail-rca-20261028)에 남는다.
재현·원인·입력이 동일한 실패를 새 작업으로 발급하거나 기준을 올리지 않는다.
Source, worker, pinned query는 독립 비용 범위다.
focused test, 원 4K 제품, 비교 성능, 운영 수용을 합산하지 않는다.

## R18–R22 authority and reuse

| 요구 | 보존할 계약 | 판정 경로 |
| --- | --- | --- |
| R18 | semantic desired/covered/missing, certified projection delta, stale job/delete fencing | [원 DoD](DOD.md#requirements), 신규 owning source |
| R19 | model/recipe/input·owner/member/provenance, provider 호환성, unchanged-owner/ABA durable reuse | 기존 Kernel·Semantic 및 신규 NC-B |
| R20 | 작은 범위 bounded exact, cosine independent gold, 정확한 coverage | 기존 selected-view oracle 및 신규 파일 oracle |
| R21 | chunking·ANN feasibility와 증분 searchable membership | 기존 게시/CAS, 신규 자체 region |
| R22 | 지속 ANN recall·query·update·filter·history retention | 원4K와 신규 출하 후보를 별도 판정 |

기존 NativeSemantic directory, bounded region/delta/member COW,
embedding/provider, selected source/projection 인증, Kernel materialization/CAS,
SDK/query/overlay route는 소스별로 재사용한다. 구 Lance file-set 결과는
원 4K 코호트의 이력이다. 현행 자체 graph 경로는 NC-B의 source와
NC-F의 제품 수용에 결속한다. 원 source-path·lexical eligibility 권한을
확장자 또는 private callback으로 대체하지 않는다.

semantic desired/covered/searchable과 ANN exact/approximate/capped/incomplete를
구분한다. model/recipe/input/owner/member identity, selected view,
stale job/delete/ABA fencing과 held-old view pin을 유지한다.
mask/delete/replacement는 top-k/refill 전에 적용한다.
새 source-file identity는 인증된 source record와 같은 view에서 결정한다.
[NC-B 파일 계약](../nov-12-native-cutover/tickets/NC-B-semantic.md#shared_contracts).

선택된 vector의 논리 identity는 `(owner, member, vector_id)`와
owner input stamp·recipe에 묶는다. `vector_id`는 물리 row offset이 아니며
기존 1024B 상한·variable-length ID의 decode/copy/result grant를 지킨다.
chunk는 선택 artifact와 위치의 일대일 대응, whole-file은 canonical
`b"whole-file"` ID를 검증한다. row/schema/digest/seal·중복·ABA·재오픈에서도
같은 identity를 사용한다.

현재 view의 membership은 selected owner/input과 sealed exact delta·region을
함께 검사한다. unindexed tail을 숨기는 `fast_search`나 무제한 exact fallback은
수용 경로가 아니다. 고정 fanout/probe/candidate/refill 예산에서 필터·삭제 후
top-k 부족은 `approximate/capped/incomplete`로 보고한다. 후보의 exact 거리
재계산만으로 전역 exact top-k를 증명하지 않는다. 관련 없는 owner 변경이
안정된 owner의 준비를 무한히 stale로 만들지 않는 진행성도 별도 검증한다.

## Implemented source contract — reuse, no new ticket

Core SearchablePreparation, Kernel caller, Semantic directory와 Storage의
준비·설치 계약은 [구현 ADR](../../adr/NOV-07-001-native-index-execution-and-custody.md#prepared-searchable-installation)에 통합했다.
기존 C1/C2·metadata/COW·ResidentStage를 미구현 작업으로 발급하지 않는다.
현재 제품·성능 판정은 NC-B/NC-F와 아래 원4K gate를 따른다.

<a id="non-seal-delta-and-coverage-installation-contract"></a>
### Non-seal delta and coverage installation contract

[같은 attempt의 delta·descriptor·coverage 설치 계약](../../adr/NOV-07-001-native-index-execution-and-custody.md#prepared-searchable-installation)을 재사용한다.
2 DATA/2 short-key roots, original-base·sink·attempt 재검사와 독립
separate-sequence oracle를 유지한다. 당시 실행은
[원 source 기록](VALIDATION.md#e04-native-sync-mutation-classes-20261024)에 귀속한다.

<a id="deferred-owner-install-contract"></a>
### Deferred owner install contract — historical source boundary

[ResidentStage의 준비·이전·설치 구분과 4 DATA/4 roots 계약](../../adr/NOV-07-001-native-index-execution-and-custody.md#prepared-searchable-installation)을 재사용한다.
당시 focused 139/139·157/157과 4K 실패·호스트 차단은
[원 source 기록](VALIDATION.md#e04-native-keyword-sequence-20261025)에 귀속한다.
세부 코드 절단은 문서 상단의 Git preimage와
[복구 색인](../../ARCHIVE-INDEX.md#2026-11-17-upper-plan-and-retired-audit-consolidation)에서 회수한다.

<a id="original-4k-gate"></a>
## Original 4K gate

기존 `product_native_ann_quality_4096x64_reopens_at_fixed_recall`는
동일 bench daemon/SDK·원 recipe·원 입력에서 평가한다.

- 4096×64 vectors, Q64, K10, Recall@10 ≥ 0.95.
  실제 UDS/provider, selected membership, 독립 exact score/coverage/traversal,
  cold reopen, delta/path/overlay를 포함한다.
- 원 per-request 1M reads/512 MiB, SDK 30초, worker assignment
  lifetime 총 10초와 기존 memory/IO/work/candidate/routing 상한을 유지한다.
  claim/grant/retry는 원 deadline을 재시작하지 않는다.
  watchdog 1200초는 요청 예산 연장이 아니다.
- Pending/status·작은64-owner·legacy ann_matrix·focused graph PASS는
  원 4K 제품 양성 수용이 아니다. Unknown은 원 key에서 관측하며
  fresh key 재제출 또는 예산 갱신으로 성공시키지 않는다.
- 고정 5c6d0f8a Off/Record의 VERIFIED 범위와 현행 자체 ANN의
  제품·필터·파일 Recall 판정을 분리한다.
  [원 receipt](VALIDATION.md#e07-wal-frame-product-20261031),
  [새 source와 제품 잔여](../nov-12-native-cutover/tickets/NC-B-semantic.md).

## Shared provider admission

Source materialization과 query는 같은 provider·attempt registry를 사용한다.
registry의 physical inflight 상한은 검증된 service execution capacity의
`provider_units`와 일치한다. `provider.concurrency=1`은 각 owner/query의
fanout이지 service 총량이 아니다. 원 grant를 가진 Interactive/Enrichment
동시 호출을 fanout 값으로 부당하게 거절하지 않는다.

각 HTTP attempt의 grant custody, 취소 후 physical join,
shutdown drain을 기존 owner가 보장한다. typed `ServerOverloaded`와
`RequestCancelled`는 Native `ResourceLimited`와 `Cancelled`로 전파한다.
원 취소/deadline이 먼저 관측되면 그 결과가 우선한다.
무관한 provider 오류는 기존 Storage 진단을 보존한다.
Source·query bridge와 pool/custody/signals 영향 회귀를 source별로 실행한다.
fixture RED→GREEN은 원4K/Q64·quiet performance 수용이 아니다.

<a id="parallel-source-audit--2026-10-24"></a>
## Parallel source audit — 2026-10-24 (historical)

당시 read-only audit는 dirty HEAD `c32cd191…`의 Catalog/worker,
physical durability, quality oracle, Kernel/Semantic materialization을
분리했다. 그 결과를 현행 source 결함이나 새 구현 큐로 재발급하지 않는다.
원 finding·반례·소유 경계는
[VALIDATION 인바운드](VALIDATION.md#e04-successful-attempt-diagnostics-20261024)와
위 `ade5fadc1` Git preimage의 이 heading에서 복구한다.
이 anchor는 [기존 참조](VALIDATION.md#e04-filtered-last-pin-oracles-20261024)
호환을 위해 유지한다.

<a id="user-requested-current-source-original4k--2026-10-26"></a>
## Historical original 4K diagnosis

10월 26일 contended materialization 실패·capture는
[원 실행](VALIDATION.md#e07-user-requested-original4k-20261026)에,
후행 고정 코호트 PASS는 [별도 결과](VALIDATION.md#e07-wal-frame-product-20261031)에
귀속한다. 원 grant/claim과 실제 worker capture/span의 연결 수리는
[영향 결과](VALIDATION.md#e07-owned-grant-trace-correlation-20261026)를
따른다. 진단 계측은 성능 수리·현재 출하 수용이 아니다.

## DoD

아래 원 수용 계약은 삭제하거나 새 계획으로 분모를 축소하지 않는다.
5c6d0f8a의 original4K 일부 통과, 현행 자체 ANN 집중 본문 통과,
신규 출하 후보의 전체 수용은 각각 다른 판정이다.
상세 unchanged profile/layout/golden은
[W2-13 원문 복구 색인](../../ARCHIVE-INDEX.md#oct-22-validation-and-w2-preimages),
전체 R18–R22와 통합은 [원 DoD](DOD.md)를 따른다.

- [ ] genuine complete affected output의 desired/covered/missing,
  exact small-scope cosine gold, certified chunk/projection delta 일치.
- [ ] 원4K full publication → ANN/Q64 → reopen → delta/path/overlay를
  해당 source·binary·recipe에서 끝까지 통과.
- [ ] 실제 selected eligible corpus/filter의 independent exact top-k·recall,
  narrow/broad·상관·삭제/churn·history retention을 고정 profile에서 수용.
- [ ] full materialization/ANN seal·file installation/checked-close,
  same CAS·old pin/fault/cancel/last-release를 PHYSICAL 원 oracle로 수용.
- [ ] 지속 업데이트의 region bounds·maintenance/quality debt,
  실제 total resources·cached/uncached freshness를 E07에 전달.

기존 192/12 filtered exact와 managed last-pin/ENOSPC의 passing witness는
[해당 소스 검증](VALIDATION.md#e04-filtered-last-pin-oracles-20261024)에만 귀속한다.
원4K·churn·운영 전체 수용으로 확장하지 않는다.
RCA는 기존 trace와 필요한 좁은 실행으로 판단한다.
마지막 deadline checkpoint만으로 지배 비용을 단정하거나
새 engine/worker/ledger·예산 확대를 필수 해법으로 만들지 않는다.
