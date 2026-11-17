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
전체 합격선은 [전환 DoD](../nov-12-native-cutover/DOD.md#자체-ann-검증)를 따른다.

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

## Implemented source contract — reuse, no new ticket

Core `SearchablePreparation`, Kernel의 actual `prepare_searchable` caller,
Semantic directory/materialization은 기존 source 경로다.
실제 선언은 `crates/quanta-index-core/src/index/searchable.rs`,
`crates/quanta-index-kernel/src/update.rs`,
`crates/quanta-index-semantic/src/native/embedding/ann_directory.rs`에 있다.
이 문서의 C1/C2·metadata/COW·ResidentStage 절단을 미구현 TODO로 재발급하지 않는다.
현재 binary/source가 달라지면 해당 영향 본문만 다시 검증한다.

- 기존 root/default/non-ANN의 scalar path는 원 wire·children·identity를 유지한다.
  provisional root는 read, pin, Catalog 게시 권한을 갖지 않는다.
- ordinary ANN에서 directory와 CoverageBinding은 child-first 준비·설치를
  공유한다. actual caller는 반환 길이·ordinal·순서·identity를 확인한다.
  Source/Projection/reconfigure caller도 같은 계약을 소비한다.
- `stage_work`의 설치된 artifact frontier, same-operation artifact reuse,
  owner metadata와 owners/missing original-base, old/current roots를 보존한다.
  prepared result는 원 payload/wire allocation lease와 attempt identity를 소유한다.
- ResidentStage는 provisional ref에만 stage-aware canonical object/map 읽기를
  빌려준다. 기존 stored-wire decoder, radix lookup/scan/batch lookup 및
  selected member/recipe/children validator를 사용한다.
- non-seal의 실제 delta는 같은 준비 범위에서 descriptor·coverage와 연결한다.
  seal, projected reverse-map, retirement unlink처럼 먼저 읽거나 써야 하는 분기는
  원 canonical stage를 첫 실제 write 전에 flush한다.
- install 전의 failure/cancel/encoding/funding 거절은 새 root나 성공 prefix를
  게시하지 않는다. partial physical install의 비용·reclaim은 원 attempt custody다.
  token drop은 I/O를 수행하지 않는다.
- selected backend의 실패를 다른 engine으로 재시도하지 않는다.
  원 grant·deadline·cancel·error priority, commit_transition의 Catalog CAS,
  physical base·old pin·delete/ABA fencing을 유지한다.

설치 그룹 감소는 해당 source와 실제 분기의 reserve/confirm 계수로 판정한다.
이는 단일 COMMIT, wall-time 단축, 현재 자체 ANN 제품 PASS를 뜻하지 않는다. 원 설치 그룹·sync 측정과
source별 테스트 결과는 [VALIDATION](VALIDATION.md#e04-native-keyword-sequence-20261025)
및 위 Git preimage에서 확인한다.

<a id="non-seal-delta-and-coverage-installation-contract"></a>
### Non-seal delta and coverage installation contract

기존 ordinary non-seal 구현은 ANN delta COW와 searchable
descriptor/coverage를 같은 원 attempt에서 결합한다.
새 delta bytes를 설치 전 읽지 않는 경로에만 적용한다.
`ObjectMapBatchPort::prepare_one_map`의 consuming token은 backend,
ResidentStage, prepared root, AttemptRef와 allocation grant를 소유한다.
준비 단계는 original base를 검사하고 설치 없이 COW DATA를 계산한다.
provisional token은 child/coverage encoding 입력이며 publish authority가 아니다.

소비 시 같은 attempt·sink scope와 coverage-base precondition을
physical I/O 전에 재검사한다. 기존 child-first installer의 reserve →
physical sync → confirm, 반환 ordinal/reference 검증, 최종 Catalog CAS를 유지한다.
이 non-seal 결합은 원 최대 2 short-key roots/2 opaque DATA를 유지한다.
독립 separate-sequence oracle는 canonical bytes/identity/child closure/선택 결과,
logical usage·peak admission과 실제 reserve/confirm 계수를 대조한다.
잘못된 attempt·base·반환 순서와 funding/confirm/부분 실패·재오픈을 검사한다.
취소·drop은 새 storage intent를 만들지 않으며 partial install은
원 attempt 회수 경로에 남긴다. seal/readback 또는 backend 부재는
별도 원 경로를 사용하며 selected backend 오류를 fallback으로 숨기지 않는다.
[원 구현·소스 결과](VALIDATION.md#e04-native-sync-mutation-classes-20261024);
정확한 옛 코드 절단은 위 Git preimage의 같은 heading으로 복구한다.

<a id="deferred-owner-install-contract"></a>
### Deferred owner install contract — historical source boundary

기존 `FileObjectStore` ResidentStage와 canonical `ObjectMapBatchPort`는
same-attempt prepared owner scope를 사용한다. 초기 artifact stage는
설치된 상태로 남고 metadata DATA·owners/missing base edit는 provisional이다.
stage-aware read만 임시 참조를 소비한다. 일반 backend에 설치된 root인 것처럼
넘기지 않는다. 원 bounded packet은 최대 4 DATA/4 independent roots이고
실제 walker는 ≤256 objects/64 MiB split를 유지한다.
원 base 조건과 child→parent reachability를 첫 durable reserve 전에 확인한다.
prepared/transferred/installed를 구분하고 CAS 이전에 가짜 완료를 내지 않는다.

이 절의 당시 focused 139/139 및 후속 157/157, 4K 실패·host 차단은
[원 source 기록](VALIDATION.md#e04-native-keyword-sequence-20261025)에 귀속한다.
후행 5c6d0f8a의 원4K 통과가 역사적 실패 상태를 대체했지만
새 자체 ANN과 성능 수용은 별개다. 구현을 다시 시작할 근거로 사용하지 않는다.

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
[보존 Semantic 사양](../../history/oct-22-finalizing/oct-17-index-closeout/tickets/W2-13-ann-product-integration.md),
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
