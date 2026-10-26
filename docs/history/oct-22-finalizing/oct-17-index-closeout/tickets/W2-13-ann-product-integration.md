> HISTORICAL — pre-migration document from `docs/plans/oct-17-index-closeout/tickets/W2-13-ann-product-integration.md`. Current work: [Oct-22 SSOT](../../../../plans/oct-22-finalizing/README.md). Old TODO/status/counts do not create work or qualify current source.

# W2-13 — Native18 제품 게시·ANN 품질 수용

상태·담당·다음 완료 조건은 [SSOT](README.md#w2-13)만 갱신한다. 실제 구현 경계와 합격 조건만 유지한다. Native15–17 비용 산술·실행 제안·후속 후보는 [이력](../../../oct-17-index-closeout/tickets/W2-13-ann-product-integration.md)에 보존하며 현재 재개발 목록으로 사용하지 않는다.

## Confirmed boundary

[Semantic mount](../../../../../crates/quanta-index-semantic/src/native/mod.rs)는 `ann_lance`와 bounded `ann_store`/`ann_session`/`ann_files`를 제품 모듈로 등록한다. `NativeSemantic::with_ann`의 `prepare_searchable`은 `embedding/ann_materialization.rs`를 호출하고, Kernel source 변경·projection advance·owner materialization은 같은 AttemptContext와 기존 단일 publication CAS로 directory를 coverage에 채택한다. `validate_coverage`는 기존 owner-map 형식을 provider 호출 전에 `RebuildRequired`로 거절한다. Runtime의 실제 provider 등록이 owner와 reader의 ANN 경로를 선택한다. 컴파일·회귀 수용 여부는 [SSOT](README.md#w2-13)만 판정한다.

`NativeSemanticReader::query`는 selected coverage의 object/child refs·recipe·owner-root를 검증한 뒤 `ann_query`로 들어간다. directory는 COW region map과 bounded delta member map을 가진다. 64-owner 또는 256-row delta는 bounded size-tiered COW 병합 후 기존 Lance helper로 한 번 봉인하며, 지역당 8,192행·1,048,576 cells와 파일 총량32MiB를 넘길 수 없다. 빈 벡터 member는 owner graph에만 보존하여 delta 슬롯을 소모하지 않는다. 활성 지역은 최대4,096개, query가 여는 지역은 최대8개다. 현재 directory format3은 재사용하지 않는 단조 지역 ID·활성 수와 selected source→projection 역방향 membership을 보존한다. 이전 format2는 RebuildRequired로 거절한다. obsolete 지역은 새 selected owner map에 그 member 참조를 유지하는 owner가 하나도 없을 때만 동일 CAS에서 참조 해제한다. pending이어도 동일 searchable member를 유지하면 회수하지 않는다. query는 sparse COW region map의 실제 key를 순회한다. sealed-region ANN 탐색 결과는 `Approximate`로 유지한다. no-region·selected owners≤64를 별도 확인한 경우와, complete SourceLocal의 exact-path를 canonical path root로0/1 owner까지 확정한 경우에는 기존 bounded exact 랭킹을 사용한다.

선택한 region 후보는 기존 `SelectedOwner`·`current_member`의 source/projection 인증과 member/input stamp·chunk/vector ID 검증을 통과해야 한다. mask·교체·삭제는 top-k/refill 전 accept에서 처리한다. delta와 지역은 같은 exact ranking/chunk 조립 함수를 재사용한다. Runtime과 SDK는 opaque directory 참조를 허용하고 owner가 있을 때 searchable 참조를 요구한다. directory 전체 권한 검증의 소유자는 Semantic이다.

Lance staging은 지역마다 별도 bounded memory ObjectStore와 private Session을 쓰며 별도 전역 캐시를 만들지 않는다. 봉인된 실제 파일 바이트를 ≤1MiB managed objects로 설치하고 canonical file-set manifest의 정확한 child refs로 연결한다. descriptor→파일 청크는 기존 `ImmutableObjectPort`/Catalog attempt·pin·GC 경로를 재사용한다. 외부 파일 경로만 저장하거나 별도 물리 namespace·ledger를 추가하지 않는다. staging/read quota와 memory grant는 원 WorkBudget에서 선예약하며, 파일 복원은 인증된 실제 길이로 제한한다. 이 상한은 Lance 내부 RSS·물리 비용이나 Large 성능 수용의 증거가 아니다.

## Publication and file authority

Consolidation authenticates every scanned source member/artifact, including
stale and duplicate rows, before applying selected membership. Exact duplicate
owner/reference pairs contribute rows once; conflicting selected references
refuse. The bounded attempt retains authenticated selected artifacts with their
temporary memory leases and sorts them by canonical owner order. Source owner
counts and exact selected-reference overlap choose a reusable immutable member
root only when a conservative saving over an empty-root rebuild is positive;
vector row counts do not determine the map base. The chosen base receives only
its exact sorted insert/update/delete difference, with original-reference CAS
and stale member removal. No difference reuses the base root. Empty-root
reconstruction remains available when no useful base exists. This avoids both
page-by-page intermediate maps and unnecessary edits of unchanged members;
artifacts are not loaded again for final sealing. Final map key/reference
agreement, exact row/owner counts, region seals, old-view custody and the
original publication CAS remain mandatory. These source changes do not prove
that the original 4K latency/10-second worker deadline now passes.

Ordinary non-seal delta insertion carries one private canonical validation of
the new member/reference/children and its artifact into the delta edit. Its
decode lease ends before old-member reads and map staging. Empty output releases
that lease before removal; pre-seal releases it before the larger carry and
reauthenticates afterward. A raw row count cannot replace this validation.
Kernel Materialize applies the independent owners/missing edits in one existing
two-root map stream with both original-base preconditions, original key grants
and exact ordinal outputs. Directory/final coverage still follow their real
dependencies and the same publication CAS. These remove real repeated work;
their complete 4K throughput effect must still be measured.

Materialize encodes the owner binding and completed OwnerCoverage into one
existing bounded child-first object batch. Canonical `identify` supplies the
binding reference used by the coverage child; returned count, order and both
identities must match before map preparation. Preparation backing remains
admitted through installation. Managed reserve, physical synchronization and
confirm retain their existing ordering and durability. The existing `Finalize`
trace stage covers this metadata batch and paired owners/missing map preparation;
searchable preparation and publication retain their separate stages.

The immutable map owner uses bounded resident staging for `apply` with exactly
one edit and a key of at most64 bytes,and for a fully consumed two-root stream
with exactly one such edit per root. It shares the canonical radix
update, codec and child-first durable installer with disposable SQLite staging.
The full physical node bound applies even when a short edit splits a long
existing prefix. Prepared wire, registration and array backing retain original
memory custody. Original-base CAS and no-op semantics remain mandatory;the pair
validates EOF and prepares both DAGs before installation through one shared
installer. Other streams,multiple edits per root and longer keys retain the
existing SQLite path. This
removes temporary scratch creation/retirement rather than replacing the map
engine, rebating work or weakening immutable-object durability.

The existing 64-owner product region/reopen regression optionally accepts the
same trace-directory environment option as the original 4K fixture. It retains
its corpus, limits, assertions and normal daemon shutdown. A complete capture
can diagnose materialization-operation phase cost; 64x8 trace observations do
not replace the original 4096x64 quality/performance gate or an Off/Record
overhead comparison. Reject partial capture totals and overlapping-span sums.

The existing bounded WorkBudget first-denial diagnostic retains the first
deadline/cancel checkpoint caller without replacing the original error, usage
or deadline. A caller identifies where expiration was observed, not which
earlier phase consumed the time. ANN session work-cap precharges likewise do
not measure actual physical IO calls or assign a terminal phase.

현재 연결된 게시 경로가 보존해야 하는 계약이다. 새 구현 배정이나 별도 TODO가 아니다.

- admitted immutable file-set install/open/checked-close/reclaim, attempt/read-ticket/pin/GC 수명. Lance 타입을 Core/Contract에 노출하지 않는다.
- selected owner-map/input stamp·recipe와 bounded directory·region/delta/tombstone·file refs의 결합.
- 기존 owner completion/publication CAS의 단일 게시 지점에서 관련 입력 재검증과 directory/coverage 원자 채택. Semantic·Runtime·overlay·SDK validator를 같은 계약으로 변경한다.
- source coverage Complete와 ANN exhaustive/approximate/capped/incomplete 상태를 구분한다. 동일성 검사만 삭제해 완료 처리하지 않는다.
- immutable 파일의 flush/checked-close·필요 barrier와 설치를 완료한 뒤 Catalog 참조를 게시한다. SQLite transaction은 외부 파일 설치까지 원자화하지 않으므로 stage/install/CAS crash 경계와 orphan 정리를 별도로 검증한다. root CAS의 충돌 검사는 유지하고 관련 입력 재검증/안전한 재사용으로 불필요한 재준비를 줄인다. 무관한 변경을 덮거나 CAS를 완화해서 stable-owner 진행성을 만들지 않는다.

## Code path map

아래는 현재 구현의 책임/호출 경계다. 실행 상태와 다음 작업은 [SSOT](README.md#w2-13), source별 검증은 VALIDATION만 소유한다.

| 경계 | 실제 구현과 보존 조건 |
| --- | --- |
| SourceAdvance | [Kernel update](../../../../../crates/quanta-index-kernel/src/update.rs) → [source](../../../../../crates/quanta-index-kernel/src/source.rs)의 apply_source → build_members → WorkSpool::seal_lazy → Catalog commit_transition. 최종 inline path 값을 scratch에 한 번 기록하고, 전체 경로/frontier 검증 뒤 3-root apply_stream을 수행한다. |
| Work/Edit staging | [spool](../../../../../crates/quanta-index-kernel/src/spool.rs)의 첫 페이지/strict successor SQL을 분리한다. 128-row 경계, typed decode, 전체 count/digest/EOF와 취소 검증을 유지한다. seal과 Catalog replay는 서로 다른 검증 책임이며 단순 중복으로 삭제하지 않는다. |
| NativeText | [text](../../../../../crates/quanta-index-lexical/src/native/text.rs)의 InventoryBatch·InventoryEdits·PostingEdits·OccurrenceWrites가 원 SourceChangeCursor와 예산을 소비한다. 매 owner마다 별도 엔진을 만들거나 전체 corpus를 재구축하는 경로는 확인되지 않았다. |
| ANN owner/seal | [ann_materialization](../../../../../crates/quanta-index-semantic/src/native/embedding/ann_materialization.rs)은 일반 owner 갱신에 Some(materialized)를 받아 retire_delta를 생략한다. seal 조건에서만 [consolidation](../../../../../crates/quanta-index-semantic/src/native/embedding/ann_directory/consolidation.rs)이 후보를 인증하고 유익한 기존 root에 정확 diff를 적용한다. 선택 점수는 편집 수의 보수적 절감 기준이며 전역 최소 IO/시간 보장이 아니다. |
| Persistent map | [map](../../../../../crates/quanta-index-storage/src/map.rs)의 resident staging은 단일 edit·key≤64 또는 EOF까지 검증된 두 root의 각 단일 edit·key≤64에 적용한다. 원 base CAS·canonical radix·child-first installer를 공유한다. pair는 두 DAG를 설치 전에 준비하며 다른 stream·root당 다중 edit·긴 key는 기존 SQLite 경로다. |
| Product fixture | [native_product_process](../../../../../crates/quanta-index-searchd-runtime/tests/native_product_process.rs)의 canonical Daemon → [ann_quality](../../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs). 임시 stderr 파일/64KiB 출력 prefix는 진단 수단이며 원4K 입력·요청/worker 예산·quality oracle을 변경하지 않는다. |

## Architecture direction and reference limits

2026-10-17 조사 및 2026-10-18 논문 4건의 arXiv 원문·버전/범위 재확인과 현재 vendor source를 기준으로 한 **설계 판단**이다. 이 layout이 업계 최고라는 증거, 모든 2026 연구의 포괄적 조사, 우리 구현/성능 수용을 뜻하지 않는다. correctness/snapshot·bounded work와 workload별 recall/latency/update 비용을 분리하는 방향을 유지한다.

| 근거 | 적용할 판단 | 적용하지 않는 것 |
| --- | --- | --- |
| [Lance read/write·version cleanup](https://lance.org/guide/read_and_write/) | immutable version의 old reader와 파일 수명 보존, compaction과 실제 reclaim 구분 | 최신 문서의 auto-cleanup/default를 pinned Lance7 지원으로 가정하거나 Catalog pin 밖에서 시간 기준 파일 삭제 |
| [SQLite isolation](https://www.sqlite.org/isolation.html) | Catalog의 transaction/snapshot과 외부 파일 설치의 권한 경계 구분 | SQL snapshot만으로 ANN 파일 pin·durability까지 보장한다고 추론 |
| [LanceDB query 공식 API](https://lancedb.github.io/lancedb/js/classes/VectorQuery/)와 [pinned Rust query](../../../../../vendor/lancedb-0.30.0/src/query.rs) | pre/post filter와 refine/probe의 recall/비용 차이를 구분; 실제 메서드/실행 지원은 pinned Rust source로 확인 | 최신 JS/Cloud API·기본값을 vendored Rust0.30 기능으로 승격 |
| [FANNS 비교 연구 v3, 2026-04-01](https://arxiv.org/abs/2507.21989v3) | 실제 filter 종류·selectivity·K·scale에 따라 방법을 비교 | 단일 알고리즘/합성 무필터 recall로 모든 제품 workload 우수성 선언 |
| [RACORN-1, 2026-07 preprint](https://arxiv.org/abs/2607.00768v1) | 낮은 selectivity·query/filter 상관과 선택 집합 크기를 별도 평가 | HNSW 연구 알고리즘을 Lance IVF backend에 곧바로 이식하거나 논문 수치를 우리 제품 SLO로 사용 |
| [IP-DiskANN, 2025](https://arxiv.org/abs/2502.13826), [Slipstream, 2026-06 preprint](https://arxiv.org/abs/2606.02992v1) | 긴 insert/delete stream의 recall·업데이트/유지보수 비용과 입력 순서 편향을 독립 비교 | in-place graph 교체·adaptive controller 도입을 이번 snapshot layout 연결의 선행 작업으로 추가 |

**유지할 구조:** 연결된 region/file binder·sealed descriptor/단일 CAS·exact-selected baseline과 bounded routing/refill을 재사용한다. 이 연결 순서를 새 구현 목록으로 재발급하거나 특정 partition tree/learned router·새 scheduler를 추가하지 않는다. 성능 결함이 재현되기 전 엔진 교체·GPU·quantizer upgrade는 범위에 추가하지 않는다. 기존 수치 recall0.95/adoption latency는 프로젝트 제안 gate이며 논문/업계 보장 수치가 아니다.

## Filtered search and bounded exact policy

- 가능한 predicate pushdown은 selected immutable input의 의미와 pinned backend 지원이 일치할 때만 사용한다. pushdown 여부와 무관하게 반환 행의 owner/member/input visibility를 최종 검증한다.
- region helper는 원 candidate cap 안에서 prefix refill과 후행 accept를 수행한다. 이 helper만으로 여러 region의 filtered top-k/전역 recall을 보장하지 않는다. routing/refill은 unique vector ID로 합치고 **모든 재시도/probe/중복 후보의 실제 작업**을 누적 budget에 청구한다. 라운드마다 budget을 초기화하지 않는다.
- 반환 hits가 K보다 적다는 사실은 집합 소진 증거가 아니다. 선택 집합/region의 exhaustion·상한 도달·ANN 누락 가능성을 각각 표현하고 approximate를 exact로 표시하지 않는다. 전체 precision/recall의 ground truth는 selected eligible corpus에 동일 predicate를 적용한 독립 exact 결과다.
- 작은 exact region·bounded sealed delta 외에, **증명된 작은 selected eligible 집합의 bounded exact**도 명시 profile로 허용한다. 상한/실제 읽기·거리계산/메모리·선택 근거를 드러내고 모든 eligible vectors를 처리했을 때만 exhaustive를 주장한다. 자동 적응 optimizer 신규 구현은 요구하지 않는다. 무제한 전역 fallback이나 전체 scan을 숨겨 Large ANN 성능을 통과시키는 것은 금지한다.
- 고정 평가 축은 narrow/broad filter·query/filter 상관·embedding/ingest 분포 변화·삭제율·selectivity·K·scale다. exact/refine은 후보의 점수를 교정할 수 있으나 탐색하지 않은 이웃의 누락을 없앴다는 증거는 아니다.


### Quality rail binding

The existing harness `ann::run_ann_report` and `ann_matrix` use legacy
`SemanticAdapter::build/open_proven`; their 4096×64 unfiltered quality output
does not execute Native directory/CAS/routing. Do not use that rail's pass as
Native filtered recall or latency acceptance. The Native acceptance must name
the actual selected view, eligible corpus/filter, bounded region/candidate
policy and independent exact top-k. The existing 256-row trained-branch
regression covers one fixed vector/filter case; the real Kernel/daemon cases
cover publication/reopen/old-pin behavior. Neither is the Large quality or
steady-state SLO. Keep any needed Native observer on the existing harness and
original product entry point, without a second runner or storage fixture.

The product reader admits exact source-path equality through the authenticated
SourceRecord of each selected candidate, before exact top-k ranking and ANN
accept/refill. Projection candidates use the contribution's certified source,
not their semantic owner ID. Source-free certified projections have no file-path
membership; inconsistent source/member/registration/child bindings fail.
Active overlays carry the admitted original snapshot path; an unknown active
path refuses before embedding, while a tombstone still masks its durable owner.
NativeText indexability is not semantic eligibility and is not reused as such.

For complete SourceLocal exact-path requests, use the selected SourceCut.paths
canonical inline owner lookup before candidate generation. Kernel publication
and Semantic reads share the same admitted SourcePathKey codec. Authenticate
the reverse source owner/SourceRecord path and selected coverage/member before
ranking all vectors of that one owner through the existing rank/overlay path.
Absent paths have no durable candidates; owner/path root-presence mismatch
refuses before the embedding provider. No whole-corpus scan, new path index,
new funding or increased candidate limit is involved. Exhaustion still requires
complete coverage, no skipped stale owner and no eligible owner left unprocessed
at the original owner cap.
Partial coverage retains truthful bounded traversal. Projected narrow scope uses
the selected directory's complete reverse membership before candidate generation;
a contribution's forward source_owner alone never substitutes for that index.

Language predicates and generation selectors still refuse before embedding.
The Runtime lexical route provides authenticated same-view eligibility; a direct
semantic request without that carrier refuses. The shared Core
`NativeTextHitSource` distinguishes durable source objects from captured editor
revisions. QueryEffort reuses the emitted Fast universe only when the whole Text
request equals the lexical scope. A distinct scope executes the existing Text
reader under the same selected session, captured corpus and original budget.
This optional preparation follows Deep policy intention; capacity denial retains
the delivered Fast result, while corruption, cancellation and invalid authority
remain typed failures. The complete captured overlay still masks durable rows;
only lexical-selected live revisions may be embedded or ranked. Native SourceRecord has no certified language; extension
inference or a private region callback cannot supply that authority. A lexical
scope must run the existing bounded lexical plan under the same selected ticket
and original budget, with equal inner/outer constraints and authenticated
owner/source eligibility passed before semantic ranking. Separate Latest queries
or filtering a completed top-k cannot qualify it. The original general
filtered-quality requirement therefore remains unclosed; public exact-path,
selected replacement/delete/overlay and language/lexical scope have distinct
acceptance scopes recorded only in the SSOT.

The producer/Contract owner must supply authenticated language metadata before
SEARCH can consume it. Native generation predicates require the existing
Runtime/Kernel selection authority to bind the requested generation to the same
selected view; a legacy generation resolver is not that binding. W2-13 SEARCH must compose the existing
Runtime/Kernel and lexical reader under one ticket to produce bounded eligibility
before semantic ranking. Existing lexical reader implementation and W2-09
retained-read infrastructure are reused; neither owns this within-request join.
Language and generation remain producer/selection joins, not SEARCH metadata
inference or parallel readers. Projected source-owner links alone cannot prove
reverse completeness. Current ANN directory format3 therefore retains a fourth
physical child: a derived source-owner/projected-owner map whose values are the
original selected coverage MapValues. Every projected owner publication refreshes
it from a bounded structural diff inside the same AttemptContext and final CAS;
SourceLocal skips this work. Prior directory format2 requires rebuild through the
existing artifact gate; no parallel decoder or internal IR is added. Query
consumers must reauthenticate directory/coverage, each reverse value, contribution
and selected source before candidate ranking. The same prefix selection serves
lexical scope and direct projected exact-path lookup, with the existing owner cap
and truthful nonexhaustive results. General language/generation inputs remain open
separately from fixed SourceLocal4K; none of these seams relaxes that oracle.

The within-request lexical join belongs to W2-13 SEARCH; its current
implementation is in Runtime native_semantic_scope/native_lexical_scope and
native_effort. Do not recreate that implementation from this contract or
transfer its owner to W2-09 retained-read or W2-19 final qualification.
Existing `Kernel::begin_query_session` and `PinnedQuerySession::prepare_stage`
already provide one selected ticket and original budget for text then semantic
stages; a long-lived public read session is not required for this request.
The canonical scope is `SemanticQueryRequest.lexical_scope: Option<TextQueryRequest>`:
its ranked-hit universe is bounded by `top_k` (Native maximum 1024), not an
invented `max_docs` field. Preserve equal constraints and the existing overflow
and model gates. Retain the real lexical owner/source result and its funded
memory, reauthenticate it against the selected source/coverage, then rank the
finite eligible SourceLocal set before ANN candidate generation. Do not filter
an already completed semantic top-k.

The current `SearchSemantic` request has no text-instance binding and profiles
may contain several Text instances. A request-local selected-profile invariant
may support a single compatible Text instance only after validating uniqueness;
zero or ambiguous candidates must refuse, never select the first. This does not
restrict all profiles globally. Selecting a particular Text instance among
several needs an explicit canonical binding. SourceLocal, overlay and projected
joins must each keep their own fixed behavior oracles; source implementation is
not their acceptance. Finish the code joins and static review, then coalesce
affected behavior/strict checks and the original fixed SourceLocal4K on the final
joined source; freeze that source during execution. This avoids
an extra compile/qualification cycle after a provisional 4K result and changes
no product oracle, budget, owner order, profile or watchdog.

Report the actual per-region index kind as well as global traversal. With one
vector per owner, the first64-owner seal contains only64 rows, below the
pinned256-row training floor; its local search is Flat. Later size-tiered
seals can include prior runs and reach the trained branch; inspect the actual
seal instead of inferring its index kind from the delta owner count. Global traversal still
reports Approximate because directory routing is bounded. The trained 256-row
helper does not prove that ordinary SourceUtf8 publications train an IVF index.
Quality acceptance must identify row/chunk counts and selected region seals;
do not change sealing limits or adopt another engine without measured need.

The materialization wait observes exact WorkStatus receipts through the
bounded completion cursor before reading the final Latest view and probing
semantic coverage at the required publication sequence. Earlier ReadView
polling has been removed; its historical contention must not be described as
the current fixture. A readiness timeout cannot be attributed to the later
Q64, cold-reopen, delta or overlay checks without evidence. Those phases
qualify different selected views and durability/masking behavior and are not
redundant runs of one oracle. Provider input sets establish delivery,
not exactly-once calls: the initial coverage probe intentionally repeats query0.
The cold slice separately forbids corpus re-embedding, and the delta slice
requires only the one changed row input under the controlled healthy stub.
These checks do not qualify external-provider latency or arbitrary owner order.


## Native18 compact metadata contract

현재 source의 단일 MapValue는 실제 object reference 또는 data≤32 bytes·실제 child≤5의 고정 bounded inline value다. canonical value wire≤195 bytes, QIMAP004 node physical children≤85·wire≤15666 bytes다. Native state/marker/Catalog stamp18과 기존 포맷 거절을 함께 유지한다. 실제 child 수에 필요한 backing을 birth 전에 선예약하며 object-only node에 최악85슬롯을 무조건 할당하지 않는다.

Source path scalar, singleton/tree postings, compact Text contribution, source-only pending coverage 및 실제 lookup/page/universe/overlay/semantic/ANN/retirement/materialization·Kernel SQL spool은 같은 계약을 소비한다. 큰 inventory와 covered/projected/contextual coverage는 기존 authenticated object form을 사용한다. reference와 inline scalar의 동일32바이트를 같은 값으로 취급하지 않는다. CAS는 kind/data/ordered children 전체를 비교하며 inline GC edges를 map node가 실제로 보유한다. 위조 CID/RegisteredObject나 병렬 IR/엔진은 만들지 않는다.

inline-only packet에 object_count=0이면 존재하지 않는 object input-array의 memory grant만 생략한다. 실제 edit/key/inline/child backing은 원 grant를 유지한다. nonzero admission 검사를 완화하거나 혼합 object packet의 설치 순서를 바꾸지 않는다.

### Bounded mutation staging

Mutation map은 owner key와 기존 bounded MapValue를 함께 인증한다. 새 producer는 expected를 data0/32로 저장하고 next0/1개만 실제 child로 둔다. expected=None과32-byte zero digest는 서로 다른 값이다. 별도 JSON mutation 객체와 중간 packet은 만들지 않으며 실제 node/child의 reserve→sync→confirm은 기존 map installer가 소유한다. 기존 Object 값은 같은 MapValue 계약에서 CID/key/canonical payload/edges 검증을 유지한다.

논리 OwnerMutationSet의 count와 JSON stream digest는 물리 값·페이지 경계와 독립이다. canonical JSON은 owned 복사 없이 count/hash/compare하고 실제 logical byte 비용을 유지한다. EOF 전 descriptor를 봉인하지 않으며, reader의 최초 오류는 고정하여 같은 cursor에서 거절된 행을 건너뛰지 못하게 한다. 원128-row RPC·30s/worker10s·1M reads/512MiB와 Native18/QIMAP004는 유지한다.

### Bounded map and Text preparation

Radix branch는16-bit mask와 정렬된 실제 child 배열을 사용한다. leaf는 child 배열을 할당하지 않고 decode/grow는 실제 backing을 먼저 선예약한다. bootstrap/edit frame의 고정 크기와 동적 child backing은 각각 과금하며 wire/CAS/child 참조 권한은 그대로 유지한다.

TextSpool Inventory/Root/Posting/Top cursor와 MapScratch의 empty-root/sorted-existing-root replay는 공용 scratch-sqlite MapEditPage로 동일 OrdinalMapEdit를64행 keyset SQL page로 읽는다. 각 실제 SQL 실행에 기존 query fee(TextSpool4096-byte, MapScratch maximal-row bound)를 청구하며, 각 행의 read·canonical allocation·page backing을 유지한다. 고정 root의 첫 empty key도 포함하고 비-BLOB key를 숨기지 않는다. 반환 위치는 실제로 yield한 마지막 행이며 뒤따르는 손상 행은 반복 오류로 남긴다. 취소는 cached row/EOF 반환 전에도 원 요청에서 확인한다.

TextSpool inventory/posting/top과 MapScratch edit 쓰기는 같은 공용 writer로 최대64행을 하나의 parameterized INSERT에 바인딩한다. 예약만 묶은 뒤 N번 execute하는 방식으로 query 비용을 낮추지 않는다. 기존 query fee는 실제 INSERT마다 유지하며 logical writes=N, 모든 key/encoded value/header/secondary-index 작업과 SQL 문자열 backing을 별도로 청구한다. 물리 row reservation과 SQLite hard page cap은 그대로 유지한다. MapScratch의 bounded shared-key buffer는 원 global order·original-base CAS를 보존하며, 전체 입력 EOF 뒤에만 root assembly와 기존 child-first managed install로 진행한다. 오류는 disposable attempt 전체를 폐기하게 하며 같은 요청을 재시작하지 않는다.

Disposable sort page는 검색/게시 권한이 아니며, compact preview 뒤 원본 전체 스트림의 CAS/digest 재검증은 생략하지 않는다. query 수·값/순서·메모리 생존/거절/환급은 독립 gold로 검증한다. Batch writer gold는 실제 SQLite VM Run=1·64 logical writes·고정64KiB 예산과 field round-trip을 요구한다. 실행 결과는 SSOT/VALIDATION에서만 판정한다.

### Bounded source-root reconciliation

SourceChangeCursor reconciles the same sealed OwnerMutationCursor with existing PersistentMapPort::visit_diff over the entire owners root (empty prefix, exclusive continuation, at most64 borrowed changes per page). Both streams must agree on owner/expected/next; missing, extra, malformed, unsorted, duplicate or non-progressing diff rows fail. Every supplied value remains Object-only. Equal-value explicit mutations retain original prior/next point proofs because structural diff omits them. Identical immutable roots need no diff allocation or backend call. SourceRecord reads/path/edge validation and manifest count/digest/complete EOF remain mandatory; no accepted prefix or fresh request budget is introduced.

The cursor reserves exact64-entry retained backing before callbacks and checks cancellation before cached-row or EOF return. First failure stays sticky. Unsupported diff backends refuse rather than falling back to a full scan or new trusted metadata path. Production wrappers delegate to their existing real map authority; pure test maps use explicit configured golds, not a second map implementation. Bootstrap remains the existing SourceSnapshotCursor. These are source contracts; status and execution evidence belong only to README/VALIDATION.

### Frozen scratch work stream and atomic Catalog publication

WorkActionSet is the sole transient count/digest commitment over ordered WorkAction identities. It is not a durable replay root. A Kernel-owned WorkSpool is consumed when sealed: scan every bounded scratch page, canonical-decode every payload, match its stored key, require strict retire-before-create identity order, and reach complete EOF before returning the commitment. Rewind the same owned frozen scratch cursor and pass it through the existing WorkActionCursor port to Catalog. The cursor keeps the scratch connection/allocation and page grants alive through commit, checks cancellation at cached rows/EOF and preserves its first failure. No append-capable alias escapes sealing.

WorkSpool and EditSpool use distinct first-page and indexed strict-successor queries,with the existing128-row window. Continuations seek by key or the ordered(root,key) pair rather than scanning a nullable-OR prefix. Complete payload decoding,original count/digest,EOF,cancellation and logical work fees remain unchanged;query-plan improvements are not budget credits or a trusted-input shortcut.

Catalog retains attempt fence, expected view/source/events CAS, workspace/epoch and duplicate-work checks, Create input-root existence and atomic work-root adoption, complete count/digest/EOF verification, and one SQL transaction for view/work/operation publication. The previous attempt-owned intermediate action leaf/map guard is deliberately removed together with that intermediate artifact; no fabricated marker or trusted-boolean substitute is allowed. A late short/extra/corrupt/error/cancelled stream rolls back the entire publication. Precommit new inputs remain protected by their original attempt custody and old roots by the live read ticket; committed WorkRows and work-object edges are the durable authority. Pending recovery rebuilds from the original admitted request and does not consume scratch or the removed action map. Existing per-row canonical/logical work and managed scratch I/O custody remain charged; only eliminated object installs/map assembly/readback disappear. Before Catalog takes its lock, the full seal reads each Create dependency through existing ImmutableObjectPort::get and checks the returned reference. This preserves the removed leaf installer's physical-path refusal and adds normal hash/framing validation;its extra read/byte work remains charged. Retire has no current child reads. The direct cursor does not call the object port under Catalog lock.

The change is confined to the single internal producer/consumer contract. WorkAction payload semantics, public requests, Native18 persisted state stamps and source/pin/GC authority remain unchanged. Qualification includes existing late-cursor rollback/root-protection and pending-rebuild oracles, bounded scratch/page lifetime golds and an actual managed-VFS semantic SourceAdvance/materialization/query/reopen seam before the original4K gate. Source-only lock review does not qualify the managed-VFS seam.

## SourceAdvance cost RCA boundary

- Historical pre-fused product3 refused with cumulative WorkUsage.bytes536868628/536870912; next4096-byte edit-page query could not fit the2284-byte remainder. This is logical cumulative work, not process RSS, peak memory or measured disk bytes. That shared read call is the first recorded refusal, not attribution of the preceding512MiB to that reader. Reads455423/writes163038/mutations86122 stayed below their1M limits; quality/reopen was not reached.
- The pre-fused TextSpool inventory/posting/top code charged4096 per row and executed one INSERT per row despite shared batch reservation; pre-fused MapScratch charged its original4614-byte bound plus key per edit. Prepared-statement caching and a shared transaction do not combine these executions. NativeText prepare writes disposable inventory/posting/top edits; the common PersistentMap apply_stream stages those edits again for ordered COW construction, so multiple real scratch passes accumulate on the same AttemptContext budget. They are not a duplicate indexing engine or a reason to skip original-base CAS/digest validation.
- For one full64-row TextSpool group, the original statement-fee component alone was262144 bytes; one genuine INSERT retains4096, reducing that component by258048. This excludes all row/field/index/SQL/backing/physical work and says nothing about whole-product savings or latency. Real group occupancy can be smaller; single-row groups cannot obtain the full-group saving. The fused writer preserves those costs and reservations. Existing token_batch also executes per row, but bills actual term+64 fields rather than4096 per token; it is not evidence of the same fixed-fee defect.
- WorkBudget diagnostics retain cumulative axes,the first denied charge or memory reservation,and the first deadline/cancellation checkpoint caller. Existing trace spans provide partial phase durations;neither mechanism supplies a complete per-callsite cost inventory or SQL batch-occupancy census. Historical scalar INSERT amplification is source-confirmed,while its exact fraction of the512MiB failure and whether fusion is sufficient remain unproved. The decisive next evidence is corrected-source strict, independent actual-VM/field/refusal golds, affected replay/CAS/pin regressions, then the unchanged4K product oracle. If still refused, attribute remaining work by stage and actual SQL execution/row counts before selecting another structural repair; do not reduce fees without reducing real work or increase the frozen limits.
- Verification delay is independent: cargow uses one cooperative flock for the full Cargo leaf build/test/cleanup. Strict4 failed admission after1800.004483s before compiler start; another selected nextest retained the slot. The lock loop has no explicit FIFO queue, but the observation does not prove deadlock, stale lock or starvation. Current status belongs only to README/VALIDATION.

### Cancellation attribution boundary

Initial SourceAdvance uses the direct Submit path;the owned10s Materialize relay is a later boundary. SDK submit has its own30s absolute request budget,while the server serial admission budget is separate. Peer watch can cancel server work after a confirmed connection hangup,and cleanup can also initiate cancellation. WorkBudget checks cancellation before its own deadline. Therefore first_denial=cancelled,SDK Unknown,Pending status and a later clean daemon exit cannot alone identify the initiating event.

Keep the same named submit RequestBudgetV1 alive through the failure diagnostic and report its elapsed/remaining/interruption without replacing or refunding it. Preserve boxed std::error::Error source chains before string flattening. Correlate this with the existing request/capture identity,PeerCancelled result and NativeSourceApply/NativeSourceMapApply/BuildPrepare spans. SourceApply now records final inline path values in its first validated scratch pass;the separate scratch reread/rewrite and its NativeSourcePathInstall emission are removed. The diagnostic enum remains readable for historical captures. Complete path and frontier validation still precedes every immutable map installation. Missing/open/dropped events remain partial diagnostics;absence is not evidence that a phase was skipped. A trace-enabled diagnostic is not an Off/Record performance comparison.

The canonical fixture daemon writes stderr directly to its own temporary file,so optional failure logging cannot fill an unread pipe and block its worker. Startup and quality failure diagnostics reopen that same file with an independent cursor and return at most64KiB;the producer file itself has no byte-volume cap and is removed with the fixture. Directly constructed sibling daemons retain their existing stderr ownership. If failed fixture cleanup leaves its own daemon running, perform the existing Drop kill/wait before diagnostic recovery. Preserve the original failure and every cleanup error,and label forced exit with graceful drain and trace footer unconfirmed. This does not extend the five-second fixture drain,SDK/worker deadlines or product hard drain,and it does not turn Pending/Unknown into a terminal outcome.

Managed-install checkpoint attribution remains bounded. In [confirm_attempt](../../../../../crates/quanta-index-catalog/src/native/authority/managed_install.rs),the checkpoint before attempt protection follows registration/child preflight and registration mutation;expiration there does not identify the slowest preceding operation. In [ReservationSequence](../../../../../crates/quanta-index-catalog/src/native/authority/fence_counter.rs),next_inner checks before allocating the next global counter value;expiration there is not proof that counter allocation dominated execution. A timed worker refusal must be correlated with its owning operation and actual phase intervals before changing physical authority. Keep registration,child edges,fence,lifecycle,rollback and durable sync checks;do not infer a cost-axis cap failure from the generic ResourceLimited terminal alone.

ANN carry now uses the existing Compaction span around consolidated_members,including validation and error terminals. It separates carry preparation from the existing Lance SemanticBuild and file-set SemanticSeal intervals without adding a trace vocabulary or collector. Off creates no diagnostic clock calls;the focused regression compares exact selected root,refusal classes and work usage across Off/Record. This is diagnostic isolation,not measured performance parity. Region maintenance and carry consume the already selected region_page reference through the same descriptor/child validator instead of looking up that same map key again. Arbitrary ordinal readers retain region_at and its original point lookup. The removed lookup's real work charge disappears with that lookup;all remaining decoding,authentication and object work remain charged. This local reduction does not establish the cause or resolution of the4K timeout.

Before cross-owner buffering,NativeText owner-local inventory/posting tail flushes produced short SQL batches despite a64-row writer and64-root inventory window. The current InventoryEdits and PostingEdits retain rows across owners inside the bounded inventory window. The fixed fixture dispatches NativeText as the CoreRequired member during BuildPrepare;semantic is AsyncCoverage. This identifies the real builder path,but does not establish historical statement amplification as the cause of a later30s failure or prove that buffering is sufficient. Completed cross-page staging work is a separate request phase. Do not weaken durable file sync/reserve/confirm or move work to fresh-budget publications.

The implemented buffers retain mixed-root(u32,MapEdit) scratch rows and each key grant in the current InventoryBatch window,draining at64rows and before inventory cursor/root application or reset. OccurrenceWrites admits destination edit backing before durable put_batch and validates its complete ordinal output before transferring confirmed references. PostingEdits retains edits across owner diffs and drains before posting reads/application. Preserve primary(root,key)/(term,owner) uniqueness,original-base CAS,token totals,attempt fence and disposable error behavior. Logical rows/fields/index/physical work remain charged;only actually removed SQL executions lose their original statement fee. These buffers are existing implementation,not a new backlog item.

Trace interpretation: SqliteWork is mixed-scope in current producers. Kernel admission spans cover selected Catalog calls. Managed-install lane3 retains COMMIT-only meaning in both historical and current captures;current attempt lane4 covers reservation preflight,lane5 reservation mutation,lane6 confirmation intent/child preflight,lane7 registration/transfer and lane8 protection/lifecycle. Lane9 measures acquisition through the existing deadline/cancellation-aware Catalog lock,and lane10 the existing BEGIN call;neither changes the lock,transaction or checkpoint order. These disjoint inner regions still exclude parts of charging,physical file sync and other worker work;they must not be added to the enclosing reserve/confirm durations. Historical captures without a lane do not prove the corresponding work was absent;in particular73105 predates lane9/10. Whole-capture reserve/confirm totals include staging and cannot be assigned to a single submit. Keep producer,lane,request identity and interval bounds when interpreting a span.

The narrow detail stages retain the existing request budget: LexicalBuild covers NativeText prepare, NativeTextPostings covers apply_postings, and NativeTextRoot covers write_root. ManagedObjectReserve/ManagedObjectConfirm enclose the complete existing authority calls, including preflight and row work, rather than COMMIT alone. Storage uses the diagnostic context already carried by WorkBudget; these spans need not be descendants of the NativeText detail spans. Attribute them only within the same request identity and actual time interval, and do not add overlapping parent/child durations as independent wall time. Off/Record result and WorkUsage parity is functional evidence; it does not measure recording overhead.

Source path claim staging uses one conditional UPSERT per leaving/incoming claim. The conflict arm may fill only its own NULL column; it must preserve the opposite claim and already validated expected/next map values. A duplicate claimant remains an error even when it repeats the same OwnerKey. Reserve the private scratch capacity before the statement, retain original hard page limits, and preserve refusal before immutable map installation. One statement replaces the former INSERT-ignore plus UPDATE; no request budget is reset or discounted.

Catalog provisional reservation reuses the exact canonical metadata bytes that passed the existing size check. The private encoding result is not child/row authentication or installation authority: permanent/provisional collision checks, retry checksum/edge validation, intent binding, reserve/confirm separation and all work charges remain required. Sharing data across the external physical-write boundary cannot replace a same-transaction proof.

Kernel pending coverage stages both owner and missing edits through the shared bounded SQL writer in packets of at most64 rows. The existing64-owner leaf window admits at most128 canonical OrdinalMapEdits,including each key's original retained reservation,before copying. Retain every scalar logical mutation/byte charge;the shared SQL writer additionally charges its actual query/fields/physical reservation. Flush every full packet and its tail before map application. A short INSERT count is a disposable backend failure,never successful partial staging;the final root/CAS and old-view pin remain the publication authority. This batching neither splits SourceAdvance nor gives it fresh funding.

Persistent map multi-root build may reuse a previously built result only for identical bases and complete canonical edit streams. Root-local edit identities bind count,key length/key,Option presence and complete expected/next MapValue wires,including inline data and every real child. Candidate identity state is fixed at64 roots;only roots sharing a base peer incur edit fingerprint work. All original cursor ordering,CAS and EOF checks remain mandatory. Before reuse,both representative and current scratch cursors must decode every row through EOF and match their original identity/count. Different bases/values/no-ops retain their original results;late source failure or malformed secondary scratch cannot return roots. Reuse removes duplicate node construction,not child-first authentication,sync or publication authority. The existing SHA-256 content-identity assumption applies;missing runtime evidence is not a success claim.

MapScratch node reads use one type/length-guarded SELECT instead of separate
length and body SELECTs. The row's bounded borrowed BLOB supplies its length;
original logical byte admission and owned wire memory admission precede copying
and wire/hash decoding. Missing rows remain distinct from malformed present rows.
This removes one SQL execution/read-work unit per present staged node without changing immutable
installation,Catalog transactions or physical barriers. The child-first DAG
regression retains its16-read ceiling;it must not require spending exactly16
reads when the operation becomes cheaper. Independent SQLite VM-run,read/byte/
cancel refusal,malformed shape and retained-allocation golds cover the mechanism.

## Test proportionality and repair order

The existing4K product test is a final product oracle, not the inner edit loop. Keep the frozen corpus/request/worker/quality limits. SourceAdvance is one genuine4096-owner operation after128-row bounded input staging; splitting it into new publication requests to reset budgets would change acceptance. The payload contains31658 source-body bytes and45056 path bytes; raw4096×64 f32 vectors total1MiB. These are fixture sizes, not estimates of logical WorkUsage, SQLite/Catalog overhead or aggregate memory. The fixture is not a Large/stress throughput claim.

The fixed fixture streams the independently checked owner/source inventory in global owner-key order. SDK stage_mutations_fallible also permits cross-page key reordering and sorts only each128-row page. Kernel mutation_pages_allow_cross_page_key_reordering_with_canonical_seal independently checks reordered pages, canonical seal/replay and duplicate/CAS refusal; it is included by the all-Kernel lifecycle impact selector. Those functional checks do not establish the4K deadline/work bound for arbitrary page orders. Product success qualifies this declared sorted fixture, not every ordering of the same corpus; keep that distinction in acceptance reporting.

| Scope | Keep / correct | Decisive evidence |
| --- | --- | --- |
| Writer regression | Keep3 existing focused golds.64KiB bounds the writer's mechanism; it is not a new product SLO. Do not require an exact total cost equal to implementation arithmetic. | Actual SQLite VM Run=1 for64 rows, logical writes64, independent row/NULL/Object/inline/BLOB round-trip, original fee/write/memory/cancel refusal with no mutation. |
| Shared writer impact | Run one affected source/storage/native Text/Kernel/Runtime batch after owning strict and golds. Reuse earlier source-unaffected results only with their exact scope; avoid repeated full workspace or separate identical selectors. | Existing fixed order/CAS, malformed/cancel, old view/pin/GC and reopen oracles. |
| Fixed4K product | Keep4096×64/Q64/K10/recall≥.95, selected source/coverage/Approximate/bounded candidates and cold reopen. Run after the decisive cheaper checks, once for the repaired source. | Real UDS/SDK/provider-input binding and the independent exact top-k. Local deterministic HTTP stub does not qualify external provider/network latency or corpus relevance. |
| Timing/profile | Keep optimized bench profile for fixed-deadline product acceptance; ordinary debug execution is a separate functional diagnostic. Owned assignment10s/retry2 is not a deadline for the entire4096-owner corpus; SDK30s applies to each existing request. | Original limits enforced through actual request/assignment;1200s outer watchdog cannot extend either. max_in_flight1 makes this fixture serial and does not qualify production throughput. |
| Fixture failure reporting | The existing quality fixture records phase/last selected sequence/coverage, checks daemon exit while waiting, and shares bounded stderr diagnostics across stage/materialization/query/reopen failures. It skips graceful signalling after an observed exit. Canonical startup writes stderr to its owned temporary file;failure cleanup confirms exit,including explicitly labelled forced reap when needed,before reopening the bounded diagnostic reader. Directly constructed sibling daemons keep their own stderr handling. After failure,a bounded public WorkStatus snapshot can report blocked/pending/claimed/completed input classes and representative owners;it is non-atomic final-state evidence and cannot establish the first temporal cause. Do not infer terminal failure from inactivity thresholds. Execution status remains in README/VALIDATION. | Preserve original success checks and watch/request/assignment budgets. No live raw Catalog reads, new control endpoints, extra global poller or test-only admission bypass. |

Repair stays at the shared writer: one real bounded INSERT, all original logical row/field/index/physical work billed, original CAS/digest/EOF and child-first install maintained. Do not expand to a new engine, scheduler or vector router before evidence requires it. If the unchanged product oracle still refuses, first attribute source/map/Text/Catalog costs and actual SQL batch occupancy using bounded optional diagnostics; choose the next producer-side structural repair from that breakdown. Whole-workspace/CI/release acceptance remains W2-19 after the product gate, not a prerequisite for every writer edit.

### Structural repair decision and criterion proportionality

The30s SourceAdvance limit is a retained product requirement,not a deadline for
embedding or searching the whole4096-vector corpus. Required Source/Text and
pending coverage must precede the existing atomic publication;semantic workers
retain their separate10s assignment budget. No industry reference establishes
that this fixture must complete in30s on arbitrary hardware. Define the acceptance
host/storage class,optimized profile,existing admission guards and trace mode
before interpreting wall time. Contended Record diagnostics locate work but do
not qualify quiet-host latency or recording overhead. A deadline-truncated run
cannot estimate complete-operation cost or justify a replacement40s/60s limit.

Keep independent acceptance meanings:

| Criterion | Assessment and retained meaning |
| --- | --- |
|4096×64/Q64/K10/recall≥.95 and reopen/delta/path/overlay | Bounded product correctness/quality oracle;no reason found to weaken it. Run after affected golds,not for every private edit. |
|1M work axes/512MiB logical bytes | Per-request work bound,not RSS or physical disk traffic. A cancellation below these limits neither identifies a numeric-cap failure nor proves the complete operation fits. Retain original charges for real work. |
|30s request/10s assignment | Retain the user's product limits. Judge reproducibly under declared performance conditions;do not convert a contended diagnostic into acceptance or split one publication into fresh-budget requests. |
|Durable reserve/install/confirm and final CAS | Correctness boundary. Keep sync,fence,child closure,rollback/recovery and original selected-view/pin behavior. |
|Validation scope | Affected independent golds and owning strict first,one impact batch next,the unchanged4K after a relevant repair or newly available qualified environment. Full-suite/operations/release belong to W2-19/W2-09. |

Choose the next structural repair from the actual cost component:

1. Reuse existing request/phase spans and the admitted
   [Catalog VFS](../../../../../crates/quanta-index-searchd-runtime/src/publication_sqlite_vfs.rs)
   boundary to separate commit sync/checkpoint work from SQL/authority work and
   record actual bounded install-group occupancy when needed. Current whole-capture
   ObjectIo counters explicitly exclude Catalog/WAL;they cannot attribute its
   fsync cost or assign all staging objects to SourceAdvance. Existing lane3 is
   COMMIT-only for managed installs,not proof of which internal COMMIT operation
   dominates. Avoid a new collector,poller or per-object event flood.
   Each reached managed reserve/confirm span now emits one
   `managed_object_inputs` counter through the existing closed Core/recorder/
   timeline schema. It counts the actual input entries,including retries and
   refused calls;it does not count new files or successful registrations.
   Reserve and confirm describe separate calls and must not be added as unique
   objects. Keep request identity and interval containment when calculating
   occupancy;whole-capture totals include input staging. Off enters no span or
   diagnostic clock. This supplies batch occupancy,not WAL sync attribution.
   The original budget's diagnostic context now follows Catalog lock admission
   into the existing VFS callback. `SqliteWork` lanes11/12/13 measure native
   WAL/main-DB/other-file sync respectively,including the VFS post-call inode/size
   witness but excluding subsequent capacity settlement. They are neither device
   service time nor complete checkpoint time. Nested Off/verification scopes mask
   the parent;foreign VFS owners cannot inherit its spans. Sync failures retain
   their actual SQLite result,while a missing terminal remains Aborted. Match
   managed call and COMMIT by request identity and interval containment,then
   attribute to the enclosing phase;Text's postings child has a different parent
   span from its managed calls. Do not infer request attribution from whole-trace
   totals or sum nested durations.
2. If actual group occupancy/repeated preparation explains amplification,repair
   the producing NativeText/map boundary within the original AttemptContext.
   SQL batching and same-base/same-edit root reuse already exist. Consider only
   proved duplicate canonical build work across existing windows or avoidable
   underfilled physical groups;retain bounded memory/bytes,complete cursor/CAS/EOF
   validation and child-first installation. Wrapper identity deduplication after
   map construction does not prove that earlier construction was avoided. Keep
   reserve COMMIT→physical sync→confirm COMMIT ordering;holding one Catalog
   transaction across external file installation is not an equivalent repair.
3. If checkpoint work materially dominates,extend the existing bounded maintenance
   ownership rather than adding a second scheduler. Production
   [connection initialization](../../../../../crates/quanta-index-catalog/src/connection.rs)
   sets WAL/FULL/fullfsync but does not override auto-checkpoint. SQLite can run
   automatic checkpoint work in the threshold-crossing commit. The instrumented
   original4K diagnostic now observes13 main-DB syncs inside18 postings COMMITs;
   WAL/main-DB sync together occupy5.657s of7.952s COMMIT time. This establishes
   repeated checkpoint-related native IO inside the request,not the entire
   checkpoint cost or a quiet-host causal speedup. Existing SQLite's default WAL
   hook calls the checkpoint at its configured frame threshold;the application
   does not override that policy. The existing reclaim checkpoint uses
   the same Catalog lock and TRUNCATE,so merely calling it from another task does
   not isolate request latency. Any change must bound WAL growth,reader retention,
   disk reserve and maintenance work,with explicit pressure refusal and recovery.
   [SQLite WAL documentation](https://www.sqlite.org/wal.html#performance_considerations)
   describes these checkpoint/reader tradeoffs.

The same-transaction reservation→intent boundary now retains the exact bounded
canonical payload returned by a successful provisional insert. Attempt and
Ingress intents consume those bytes instead of encoding the same metadata again.
Collisions return no payload authority:the existing permanent/provisional row and
edge authentication still runs,then encodes the authenticated input. The private
payload does not escape its locked transaction. Logical charges,reserve→sync→
confirm ordering,fences and recovery remain unchanged. This removes a confirmed
duplicate;it does not establish that the30s product gate has passed.

### Renewal cadence after durable Catalog latency

`native_pump.rs::await_completion` must anchor each renewal attempt before
calling the existing Catalog renewal. Catalog `renew_claim` dates the durable
lease inside its locked transaction,then commits. A cadence anchored only when
that call returns adds the same call's latency to the next renewal interval.
With lease2000ms,period≈667ms and1688ms call latency,a successful renewal can
be followed by a2363ms start interval and a lost fence. The Off4K diagnostic
contains exactly that consecutive pair for one operation;it does not establish
that every loss or the final blocked owner's first failure has that cause.

Count renewal latency in the existing cadence and poll completion before an
overdue renewal. Keep original lease length,grant/worker deadline,Catalog
authentication and WorkFenceLost cancellation/drain semantics. Fast resource
denial still waits a positive period;slow calls themselves consume cadence time.
Do not add a second timer thread,connection,budget,lease extension or new retry
authority. A deterministic regression models a lease dated before slow COMMIT
and requires a second renewal before expiry without cancellation;existing
short-lease,resource-denial,fence-loss,stop and result-channel tests remain the
owning behavior oracles. Product progress/quiet-host overhead need their own
results after this repair and cannot be inferred from a control-loop gold.

First renewal must also include grant COMMIT/delivery latency. For an active
grant,Catalog begin_owned_work sets assignment.due_ms to the current lease
endpoint in the same clock sample used to derive execution_deadline. Runtime
translates that endpoint through their common assignment deadline and starts
its elapsed renewal clock at lease birth,not message receipt. An already-overdue
first period polls completion without waiting before attempting renewal. This
reuses existing grant fields;it adds no clock sample,serialized IR,lease duration
or worker budget. The virtual lease regression covers both immediate delivery
and delivery after70ms of a90ms lease,followed by slow70ms commits. It cannot
rescue a lease already expired before control can acquire Catalog.

### Failure RCA: request latency and managed-work recovery

The two contended Off4K failures have different proximate causes. Submit18587
published SourceAdvance,then observed row-1193 Blocked(FenceLost) with1298/4096
completed inputs. Submit98929 stopped at SourceAdvance30.001140541s,with reads
476301 and bytes405723287 below their original limits. Its cancellation
checkpoint at object.rs591 is an observation site,not a phase-cost attribution.
Neither failure reached the ANN recall oracle. The later runner ENOSPC is a
separate infrastructure failure;its causal contribution to request latency is
not established. Off mode also does not establish a tracing overhead cause.

The pre-repair recovery-path audit identified a second seam beyond the timer:

- Catalog `acquire_owned_work_impl` rotates an expired claim when its operation
  remains Pending. It preserves the original assignment deadline/dispatch cap.
- Kernel `drive` maps WorkFenceLost to terminal Refused(FenceLost). The existing
  lifecycle gold intentionally requires the old operation to remain terminal;
  it demonstrates recovery with a fresh claim and a different operation.
- Managed `finish_owned_work_impl` and acquisition instead blocked any already
  refused operation except Conflict. Persisted Refused(FenceLost) therefore
  blocked the work even if the pump reported Interrupted. This was a reachable
  distinction between terminating an old operation and recovering its work;
  it is not proof of which validation predicate failed for row-1193.
- WorkFenceLost also represents stale identity/status,lease expiry,inactive
  assignment and original assignment deadline expiry. It cannot authorize an
  unconditional retry or lease extension. Before changing recovery,establish
  the same current intent/input/epoch,old worker cancellation/quiescence and
  stale-write exclusion;reuse Catalog's existing fresh-fence/request rotation
  and original deadline/dispatch limits. Keep old terminal receipts immutable.

The repair reuses the existing Catalog rotation for terminal Conflict and
FenceLost in both finish and acquisition. Work-row status/current epoch and
assignment/grant identity checks remain;Superseded/Completed/Blocked work does
not reopen. Original deadlines,precharged dispatch counts,backoff and permanent
refusals remain authoritative. The old receipt remains terminal and the next
attempt has a fresh fence/key. Runtime waits for worker quiescence before finish
can rotate,retaining original completion and settlement errors. No new worker,
connection,allowance or serialized IR is introduced.

The focused Catalog/Kernel/Runtime batch passed29/29,including the expanded
Conflict/FenceLost finish/acquisition cases and eight cap/deadline/permanent
refusal cases. Canonical Kernel expired-operation recovery and existing
renewal/fault/quiescence tests remain independent golds. SourceAdvance phase
decomposition and original4K acceptance remain separate. Earlier Record evidence of
5.657s native sync within7.952s postings COMMIT is a candidate mechanism from
different source/contention,not the latest Off run's measured dominant cost.
A renewal callback or Catalog mutex/IO stall can itself consume a whole lease;
the cadence repair removes added waiting but does not guarantee renewal
serviceability. No deadline relaxation,blanket retry or mandatory checkpoint
rewrite follows from these observations. This remains the same W2-13 bundle.

The post-recovery Record diagnostic still expired at SourceAdvance30.004420s.
Its complete6889-event capture measures SourceApply12.420555s,including
SourceMap6.103306s,and interrupted Text17.514030s. Reached Text COMMIT5.143604s
contains only1.015769s native WAL/main-DB sync. This later sample does not support
promoting the earlier sync-heavy checkpoint hypothesis to a mandatory policy.
COMMIT's remaining time is not yet a separately measured CPU/settlement bucket.

The applied repair removes a source-confirmed duplicate at the existing
VFS consumer:after `reserve_end` authenticates root/publication/allowance on every
write,call `start` only when the same ticket is not already started. A first
resize still starts;every growth/overwrite,parent match,FD witness,sync and
settlement remains. Common authority methods keep their own validation for all
other callers. The existing root-identity gold explicitly exercises repeated
writes on an already-started,already-reserved ticket before root replacement.
This reduces real authority probes without new budgets,caches,queues or a change
to WAL policy. Its wall-time fraction and the new-source30s outcome must be
measured rather than inferred from this local reduction.

The combined final source passes the regional64 seal/search/reopen oracle in
27.841s with the original60s watchdog. Regional readiness now shares the4K
exact WorkStatus tracker instead of writing durable pins through ReadView
polling;the selected-view/coverage/query/reopen checks remain. The final4K Off
contended diagnostic nevertheless still expires at SourceAdvance30.002693s
(831285 reads,520854798 bytes;no numeric denial). Normal submissions were
refused by chained host preflights before any test body. This bundle remains
open for the original4K30s/full-oracle acceptance. Do not promote regional or
owning green to that gate,attribute all elapsed time to the final map checkpoint,
or manufacture a mandatory checkpoint-policy task from the older sync sample.

### SourceAdvance latency RCA: current-source observation

The later sampled diagnostic published the original4K SourceAdvance in22.639153s
under the retained30s/1M/512MiB limits. No production source changed. The fixed
corpus has31,658 total body bytes (`row-{seed}`),not4,096 ordinary code files;
ANN embeddings use a local stub and execute after this request. Source6.719868s,
required Text8.283078s,other BuildPrepare4.823732s and publish1.200984s are
disjoint components;admission/sealing/reply consume the remainder. Map3.578124s
is inside Source and postings6.039658s is inside Text.

Source map's1,277 installed inputs use6 bounded reserve/confirm groups per side;
managed Catalog time is2.146346s (~60% of map time). Postings'1,674 inputs use9
groups per side;managed time is4.379795s (~73% of postings). There is no evidence
of one Catalog commit per file or a per-edit full-tree rebuild. The same small
object storage path still authenticates and owns actual file I/O per object.
Source/Text/async coverage each read source records through their existing
independent validation cursor;Source and Text also read body objects. Catalog
per-object charges are conservative logical work,not a physical syscall count.
The async SourceLocal coverage loop consumes only owner/prior/next ObjectRefs,
yet the common cursor eagerly reads/decodes all4,096 source records again. This
is the first exact candidate in SEARCH's Kernel/core boundary. An identity-only
iteration must preserve root-diff/CAS/order/EOF and sticky failures,consume an
explicit same-attempt Source validation proof,and keep Bootstrap/reconfigure
corruption rejection. It cannot be replaced by an unverified raw mutation walk.
The4.823732s enclosing interval also includes work/coverage construction;its
whole duration is not an estimated saving from removing these reads.

Three3s OS sample windows show open/openat in57.1%,59.5% and42.0% of the active
worker's sampled stacks,against fsync3.1%,1.5% and5.1%. They locate a file/path
operation cost class;the stripped executable does not identify the exact Rust
caller or fcntl command. The Common root/allowance and VFS mutation paths must
remain live-authenticated. Do not remove validation,cache authority across
transactions,disable durability or reissue the already-fixed repeated start.

This changes the diagnosis from an allegedly impossible30s condition to a
measured synchronous object/metadata path with substantial environment-sensitive
latency and insufficient demonstrated tail headroom. It is not a controlled
Off/Record comparison or a host-only causal experiment. Exact callback ownership
and avoidable repetitions still need symbolized stacks or narrow timing before
selecting a code repair. Reuse the existing map/install/reader implementations;
do not create a packed-store migration or WAL-policy task from this evidence.
The conversational5–10s estimate is not an accepted new product threshold.

The diagnostic deliberately stopped later materialization with SIGTERM after
collecting Source data. Exit101/body226.31s and daemon143 are induced diagnostic
termination,not a natural full4K result;the final observed84/4096 does not qualify
ANN recall or throughput. Full4K remains open. The canonical parser validated
the108861-event capture,normal footer,health0 and no loss/open spans. Command,
source hashes,disjoint intervals and sample limitations are recorded in the
[execution RCA](../../oct-10-index-closeout/VALIDATION.md#sourceadvance-latency-rca--same-source-sampled-diagnostic).

### Latest original4K Off refusal: owned worker deadline

After the repairs below,the original fixed4K Off command passed normal
preflight and SourceAdvance's unchanged30s boundary. It nevertheless finished
FAILED(exit101,body654.49s,daemon0):owner `row-668` became
Blocked(ResourceLimited). The owned execution's first denial is `deadline`;
reads44,212,writes9,719,bytes23,414,732 and mutations9,312 remain below the
unchanged numeric limits. No denied numeric charge or memory was recorded.
This is the worker10s boundary,not a repeat of the SourceAdvance30s failure.
More precisely,`total_timeout_ms=10_000` starts at first assignment and is
preserved across dispatches. The grant receives only its remaining lifetime;
the Off refusal does not establish ten seconds of pure computation. The
fixture's batch_limit1/max_in_flight1 serializes owner work,so total4K body
elapsed time and one owner's budget are different quantities.

Expiry was noticed while installing reachable staged-map objects at
`map.rs:505`. The checkpoint alone does not identify the dominant preceding
phase or prove a particular merge size. Static follow-up identifies two
reachable repeated-read paths:consolidation authenticates selected coverage by
point lookup for every member,and generic sorted map edits independently walk
the same original root for every CAS predicate before construction. Scratch
node/mark writes also each reserve SQLite capacity. These are candidates to
discriminate,not measured wall-time shares. Preserve full source-row/artifact
authentication,original-base CAS,EOF,actual allocation admission and physical
custody. Do not weaken worker/SDK deadlines or numeric limits. ANN/Q64/reopen/
delta/path/overlay was not reached;the bounded cached890/4096 observation is
not full throughput. See the existing [execution record](../../oct-10-index-closeout/VALIDATION.md#source-validation--scalar-coverage-cow-repairs--search).

### W2-13 closeout implementation sequence

**Current repair checkpoint:**the RCA's first candidate is implemented,not an
open-ended cache task. Kernel `source::AppliedSource` privately binds completed
Source record/body validation to its exact AttemptRef and next SourceCut.
Async SourceLocal coverage consumes checked owner/reference mutations through
the existing Core SourceChangeCursor;the same cursor still authenticates the
sealed mutation/root diff,order,CAS and EOF,and errors remain sticky across both
access methods. No caller can manufacture the private Kernel proof. Snapshot
and reconfigure keep SourceSnapshotCursor's record/path/edge validation.
The initial Core/Kernel owning batch passed187/187 before the map changes below.

Further read of the existing sampled trace found271 completed materializations:
Finalize58.953s,SemanticPrepare27.626s and actual Lance SemanticBuild4calls/
0.106s. These are summed stage intervals over that diagnostic,not4K completion
or wall-time throughput. Materialize always submits two short-key coverage
edits to apply_stream;the prior stream path opened disposable SQLite scratch
even for that pair.

Storage now recognizes only a fully consumed two-root/two-short-key stream and
reuses the existing bounded ResidentStage/update_child/DAG installer. Three
bounded peek slots replay unchanged to the original stream algorithm for every
other shape;cursor-tail errors propagate before installation. Both DAGs and
their exact original-base CAS checks finish before physical reserve/confirm.
The two-root stage has at most twice the proved single-root node/wire bound;
all backing still uses the original memory grants. One shared stage and one
installer preserve DAG deduplication and existing bounded Catalog groups.
The first map/Kernel batch passed54/54,including independent scalar-map golds,
old roots,shared roots,long-key fallback,late CAS conflict and EOF refusal.

Finally,the short-key CAS is fused into the same radix walk that constructs
its disposable replacement. Exact-value and absent-key checks happen at the
terminal/split/missing branch before unwinding. Already-validated sorted stream
updates retain their original-base proof and existing path. No-op scalar edits
still check CAS and retain the original root. The final map/Kernel impact batch
passed54/54 with the fused CAS;the earlier54 result is retained separately. Original
4K Off/full-oracle acceptance follows the final owning and strict checkpoint.
No Catalog/VFS/Common physical body,wire format,worker,ledger or limit changed.

The follow-up bulk CAS repair retains one admitted decoded path of the immutable
original root inside apply_stream. Each frame matches the actual reference and
path offset;root or child divergence discards the obsolete suffix. Each edit
still compares its expected value against the original base,and complete
ordering/CAS/EOF validation precedes replacement installation. Original node
wire/digest checks run on every newly consumed node. Cache hits still check
cancellation/deadline and bill comparison work. Frame/node backing is admitted
by the original memory scope and released before replacement construction.
There is no process cache,mutable authority reuse or new public map interface.

The1028-key independent BTreeMap regression measured3076 uncached object reads
versus3 admitted path reads. This measures only the original-root lookup path,
not whole materialization latency. Storage map,Kernel materialization and
Semantic carry/retirement owning regressions passed64/64;Storage production
strict and hexagonal checks passed. Fixed4K product acceptance on this repair
still requires the original command under normal preflight. Its single602.1s
admission wait ended BLOCKED before Cargo/body because of foreign Rust/host
load;the final load22.92627 exceeded8. No live product run remains from that
submission and this repair's fixed4K body is NOT_RUN.

Current status and next action remain in [README](README.md#w2-13). This is the
implementation/acceptance contract for that single remaining bundle,not a new
ticket set. SEARCH owns its call-path integration and product oracle;existing
PHYSICAL owns the Common binder/capacity/retirement bodies and existing VFS
mutation/settlement hunks. This plan does not transfer those functions to SEARCH.
Before editing a shared function,recheck its live hunk and existing owner.
No other chat is dispatched or authorized to edit by this plan. A missing recent
chat entry or idle state does not release its code ownership.

**Target:**finish the Source/Text/ANN path under the original4096×64/Q64/K10/
recall≥.95,reopen/delta/path/overlay,30s request,10s assignment,1M work axes and
512MiB logical bytes. Code readiness,owning validation,product acceptance and
Off/Record overhead are distinct completion gates with their existing owners.
A quiet-host blocker cannot
close an unimplemented policy,and a code repair cannot close the product gate.

**Entry decision:**the recorded4K checkpoint costs precede the verified
reservation→intent payload repair and were measured in contended Record mode.
They establish an observed cost component,not that a new checkpoint policy is
mandatory on the repaired quiet Off path. First consume the repaired source's
owning result and run the unchanged original4K Off when the normal guard permits.
If it passes,do not add checkpoint policy work as a new W2-13 completion gate.
If it fails,select the next repair from that actual failure;the policy branch
below applies only when the evidence still identifies this boundary. While a
quiet host is unavailable,static serviceability analysis may continue,but no
performance policy is activated and no unchanged contended4K is repeated.
An independently proved correctness defect may be repaired without waiting for
performance acceptance. The earlier mandatory-policy wording was stronger than
the available evidence and is superseded by this entry decision.

Final duplication audit against current source:

| Already present / sole owner | Consequence for this bundle |
| --- | --- |
| Catalog `checkpoint_wal_for_reclaim`:threshold/IO/work checks,original request,TRUNCATE and typed busy refusal | Extend only the bounded policy/recovery gap;do not add a second checkpoint implementation or caller budget. |
| `run_reclaim_due` checks WAL before the first object step;`run_scheduled_round` invokes that step even with an empty object queue | Keep the existing cadence and empty-queue behavior. No new demand queue,dirty flag,poller or worker is justified by an empty reclaim queue. |
| Common binder/VFS already reserve physical growth before native mutation and retain actual inode/funding/retirement custody | Reuse that admission. Add a WAL serviceability predicate only if the existing bounds cannot express the accepted contract;do not mirror the byte/file ledger. |
| Catalog reader-deferral/work-cap tests and VFS `wal_truncate_checkpoint_retires_original_funding_before_next_writer` | Reuse these independent golds. Add only policy-specific boundary/recovery assertions;do not recreate a fault/long-reader matrix. |
| Completed Source/Text/map batching,reuse,payload encoding and diagnostic repairs | Not new implementation tasks. Revisit only a newly observed failure in an affected caller. |
| W2-09 formal Off/Record/Large/XL/mixed/physical pressure;W2-19 full-suite/CI/release | Consume their source-bound results. This chat does not submit a second campaign or require those gates to mark a focused writer test successful. |

Additional source constraints for any policy candidate:

| Current source fact | Required interpretation |
| --- | --- |
| VFS BeforeWrite target_size is offset+amount;its original-FD witness supplies current file bytes. Truncate has a different target meaning. | Identify WAL by authenticated native open flags/handle,not a suffix. Compute post-write extent as max(observed bytes,write end);distinguish added growth,overwrite and shrink. Never treat a header overwrite as reclaim or block sync/shrink solely because an old WAL is above the candidate cap. |
| `qiv_truncate` calls original-FD sync after WAL truncate0 to settle its mutation before the next writer;`after_sync` retains/settles original ticket custody. | Preserve that existing sync and funding handoff. Native WAL sync count greater than COMMIT count is not proof of redundant sync;pressure must not disable recovery/rollback settlement. |
| Boot Catalog recovery occurs before the optional maintenance pump starts. Automatic maintenance can be absent in an otherwise valid service. | Validate a requested deferred policy before activation,retain funded boot recovery,and activate only with its actual consumer. Do not silently disable auto-checkpoint during boot or reject every existing no-maintenance service merely to install this candidate. |
| Reclaim round timeout is checked at explicit checkpoints;it cannot preempt a native filesystem call. The Catalog connection currently has its configured SQLite busy timeout. | Work/IO bounds do not prove a hard maintenance duration or mutex hold ceiling. Distinguish mutex wait,SQLite busy wait and in-flight IO;do not claim that moving TRUNCATE to maintenance guarantees30s request latency. |
| The existing real-reader deferral gold uses zero SQLite busy timeout and an unbounded request. Maintenance physical growth uses the original held parent. | Reuse its snapshot-correctness result,not as finite production deadline/funding proof. Add only the missing finite-budget/cancellation/pressure case. DB growth is incremental retained-inode growth,not the entire already-charged database size. |
| Common `capacity_snapshot` reports logical regular-file charges;the native witness reports file width/inode identity. | A WAL extent bound is not a proof of aggregate allocated disk peak,filesystem metadata overhead or guaranteed ENOSPC-free operation. Retain fail-closed IO handling;W2-09 owns actual allocated-resource acceptance. Do not add a second OS sampler to this policy. |

The following1→5 sequence is a conditional policy-repair branch,not five
unconditionally missing components:

1. **Freeze the bounded WAL handoff contract before changing checkpoint policy.**
   Reuse Catalog's existing connection mutex and Runtime's publication allowance,
   MaintenanceControl reservation and reclaim driver. Derive one validated
   policy from those existing resource inputs,with a maintenance trigger,a hard
   physical WAL growth bound and enough
   headroom for the next admitted write/transaction and checkpoint DB growth.
   Use checked arithmetic and actual WAL extent/frame widths;do not confuse
   logical WorkUsage bytes,physical retained bytes and IO reservation.
   `checkpoint_wal_for_reclaim` currently charges4×observed WAL bytes and skips
   a WAL larger than max_io_bytes/4. The existing product control IO64MiB therefore
   permits at most16MiB observed WAL under that bound. This is the current
   checkpoint-call limit,not a newly accepted WAL product limit or default.
   The fixture's512KiB physical
   maintenance reserve is a separate limit,not16MiB of checkpoint growth credit.
   Derive a serviceable bound from all original IO/work/physical allowances and
   the next admitted native mutation. Do not introduce whole-corpus/transaction
   replanning or prepay a second copy of its resource authority. An infeasible
   configuration must fail explicitly;
   do not increase the fixture limits or invent a second allowance. Show that
   long-reader deferral and startup recovery cannot strand an oversized WAL
   forever behind the existing optional-skip branch. If that cannot be proved,
   do not activate deferred checkpointing;repair the bounded recovery seam first.

2. **Implement pressure/refusal and funded maintenance before deferring auto-checkpoint.**
   A selected WAL cap must be enforced before any growing native write/truncate
   side effect,using the same admitted parent and inode witness. Reuse existing
   physical growth admission;implement only the missing WAL-specific bound.
   The existing maintenance driver already checks WAL before object reclaim
   even when that queue is empty. Consume its original cadence and checkpoint
   result rather than adding a second demand mechanism. Preserve one finite
   budget per round and the
   original request for foreground work. Overwrite,shrink and original-FD sync
   retain their existing authentication/funding rules. A busy reader or unavailable allowance
   defers maintenance with a typed result;it neither becomes success nor starts
   an unbounded wait/retry. At the hard bound,refuse further growth while keeping
   rollback,close,shrink and recovery possible. Preserve settled/unsynced ticket
   custody,failed sync behavior and protected cleanup capacity. Any existing
   retained debt may be reduced only after an observed checkpoint/reclaim result;
   timers or
   nominal freed pages are not physical release witnesses.

3. **Join the producer and consumer,then change the checkpoint policy once.**
   Catalog connection initialization may defer automatic checkpoint work only
   after the bounded policy and actual maintenance caller are wired. Read back
   the installed policy;do not assume a pragma succeeded. Keep WAL/FULL/fullfsync,
   reserve COMMIT→external sync→confirm COMMIT,final CAS and cancellation checks.
   Startup with unavailable maintenance must not activate the deferred policy.
   Keep existing no-maintenance service semantics outside this selected policy.
   Do not hold the Catalog transaction across object file IO or introduce a
   second SQL connection,worker pool,ledger or public API for the same job.
   Merely moving TRUNCATE to a task using the same mutex does not isolate
   latency:check mutex wait,configured busy wait and reader-blocked behavior.
   Report in-flight native IO as non-preemptible rather than asserting that the
   round timeout guarantees a hard syscall/lock duration.

   Exact integration seams:

   | Owner / seam | Implementation boundary |
   | --- | --- |
   | SEARCH / Catalog integration | Proposed policy/readback call sites in `connection.rs::open_connection` and existing `checkpoint_wal_for_reclaim`;confirm the exact hunk with the Catalog/PHYSICAL owner before edits. Existing checkpoint charging/transaction/fault bodies are reused,not independently rewritten. Constructor/admission signature changes,if required,join all existing callers under one writer. |
   | SEARCH / Runtime composition | `native.rs` Catalog/service creation consumes the accepted policy. `native_pump.rs::run_reclaim_due` already supplies the maintenance caller;change only an actual missing producer/consumer seam,not its existing cadence,resource classes or shutdown/cancel/join. |
   | Existing PHYSICAL / VFS | `publication_sqlite_vfs.rs::Adapter::before_mutation` and `after_sync`;consume the same policy/parent,bind the actual native WAL file and refuse before side effects. Common `publication_file_capacity.rs` changes stay with its owner;SEARCH does not clone that engine. |
   | Existing oracle owners / SEARCH product | Catalog/PHYSICAL extend their existing checkpoint/VFS/pump fault golds only for changed behavior. SEARCH owns `native_product_process::ann_quality` and consumes those results in the single affected batch;no duplicate VFS test body or harness. |

4. **Finish source review,then execute one combined affected batch.**
   Inventory the existing independent golds first,then extend only uncovered
   policy deltas for:durable acknowledged rows after
   restart;reader snapshot preservation and resume after release;hard-bound
   refusal before growth;finite busy/lock behavior;insufficient original IO/work/
   physical reserve;preserved empty-reclaim-queue maintenance;cancel/stop/join;and failed
   write/sync/close custody. Reuse existing Catalog reader-deferral/work-cap and
   VFS WAL-retirement/fault tests;do not duplicate their full matrices. Off/Record
   must retain rows,outcomes,costs and Off diagnostic clock0. Complete call-site,
   failure/drop-order,format,hexagonal and affected module/API checks before
   combining the owning tests under `./scripts/cargow` admission. Keep current
   ReadSession lint13 and unrelated module drift separate;fix only owned hunks
   or take the existing owner's result,without blanket baseline rewrites.

5. **Run the unchanged product gate and close only on its actual result.**
   After the code/owning gate,declare the existing optimized profile,host/storage
   and normal timing guard;run original4K Off once. Check complete Source/Text/
   pending publication as well as later ANN/Q64/reopen/delta/path/overlay. A
   deadline below current numeric caps does not prove complete-operation work
   fits;inspect both on failure. Any further code change must follow the new
   first failure,not a raised deadline/cap or a smaller corpus. Once correctness
   passes,hand off the result/source to the existing W2-09 contract. Its owner
   alone runs the formal source-bound Off/Record comparison;this step adds no
   W2-13 performance campaign. Off/Record semantic parity tests in step4 remain
   focused correctness checks and cannot substitute for that comparison.
   Retain BLOCKED/NOT_RUN for an unavailable quiet environment. W2-13 closes
   after its owning code and unchanged product oracle pass;W2-09 owns overhead/
   operating-pressure acceptance,and W2-19 owns full-suite/CI/release.

   This specific `cargow test --profile bench` selector does not itself run
   `check_host_contention.py`:the wrapper supplies cooperative build/test
   admission,not quiet-host qualification. The caller must chain the explicit
   normal timing preflight with `&&` before submission. A preflight denial ends
   this submission without starting Cargo;do not assume that unsetting the
   contended-timing override installs a guard. Keep both trace-directory and
   contended override unset for Off. A successful preflight is a point-in-time
   observation,not a guarantee that the whole run remains quiet.

   ```sh
   python3 tools/ci/timing/check_host_contention.py &&
   env -u QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS -u QIDX_NATIVE_ANN_TRACE_DIR -u QUANTA_INDEX_NATIVE_WORK_DIAGNOSTICS_V1 QUANTA_INDEX_RESOURCE_WAIT_SECONDS=60 ./scripts/cargow --lane bench-lane test --profile bench --locked --offline --all-features -p quanta-index-searchd-runtime --test native_product_process ann_quality::product_native_ann_quality_4096x64_reopens_at_fixed_recall -- --exact --nocapture --test-threads=1
   ```

   Select the already declared storage class/TMPDIR before this command. Keep
   disposable output outside the checkout;do not introduce a parallel runner.

After an evidence-backed entry decision,the policy branch order is1→2→3→4→5.
Current repaired-source product acceptance comes before selecting that branch;
its success can close W2-13 without the candidate. Once step1's contract and exact shared hunk
ownership are fixed,the pure policy/growth checks,existing maintenance-driver
integration and independent oracle preparation can be separate work units.
Shared constructors/admission contracts and final code integration have one
writer;runtime execution remains one serial queue. This plan starts no parallel
workers and leaves no new registry,scheduler,ticket or evidence-file requirement.

FULL WAL commits retain a durability sync. Switching to NORMAL or removing file
barriers changes power-loss durability rather than removing duplicate work;it is
outside this repair contract.
[SQLite synchronous documentation](https://www.sqlite.org/pragma.html#pragma_synchronous).
Packed storage,new engines,global caches and asynchronous publication semantics
are not prerequisites. Consider physical layout changes only if request-attributed
object/barrier counts establish that bounded reuse/coalescing is insufficient.

### Readiness observer contention RCA

The quality fixture previously polled `ReadView(Latest)` every50ms during materialization. The actual Runtime route acquires,validates and releases a Catalog read pin;each step uses `BEGIN IMMEDIATE`,and pin creation/deletion and the durable clock create WAL work. The observer therefore competes with owner publication and renewal on the same Catalog mutex. This repeated writer path is source-confirmed;its share of the historical1789/4098 timeout is not measured,and that old-source timeout cannot prove the current producer is still slow. Current SourceLocal ANN materialization edits one owner,skips projected reverse construction and performs bounded region maintenance;it does not rebuild the entire4096-owner corpus per publication.

The bounded repair uses the existing public `WorkStatus` SELECT for readiness,at most64 unresolved exact staged owner/input intents per round and the same50ms wait. A fixed4096-entry completion bitmap follows round-robin order and skips only authenticated completed receipts;it does not assume that OwnerKey order matches the pump work-digest order. Initial readiness covers all4096 inputs;delta readiness includes only its newly staged replacement binding,so retained old work rows are not required. The fixed fixture has no terminal-work retention. Every status read checks the original watchdog;no status request resumes,claims,submits or funds work. Deadline-bearing source staging,selected inventory,readiness,exact-path,delta and overlay read/renew/stream RPCs use the existing request budget capped at that same absolute watchdog,so neither the SDK30s ceiling nor the outer limit can be extended. Final publication/page/coverage/query,initial/cold provider observations,and the final durable post-overlay checks reject success after expiry. The private overlay query reuses the existing QualitySelection instead of adding a parallel context or extending public APIs. Failure diagnostics remain separate bounded observations after failure. A status observation never accepts quality:the original final latest-view sequence floor,actual Complete/source-root/covered checks,Q64/K10/recall,cold parity and selected delta/overlay oracles remain mandatory. Runtime product publication,lease/fence/sync and all request limits remain unchanged. Current verification belongs to README/VALIDATION;do not infer speedup from removing observer writes alone.

## Original 4K acceptance contract

고정 제품 테스트는 [ann_quality.rs](../../../../../crates/quanta-index-searchd-runtime/tests/native_product_process/ann_quality.rs)의 product_native_ann_quality_4096x64_reopens_at_fixed_recall이다.

- 4096 rows ×64 dimensions,64 queries,K10,recall≥.95, 실제 UDS/SDK/provider input·selected source/member·exact score/coverage/traversal·cold reopen parity를 모두 요구한다.
- 원 per-request1M reads/512MiB·30s와 원 worker10s, 나머지 memory/IO/work/candidate/routing 상한을 유지한다. 외부 watchdog1200s는 요청 예산을 연장하지 않는다.
- 기존 bench profile/Cargo front door와 같은 profile의 CARGO_BIN_EXE daemon을 사용한다. legacy ann_matrix의 직접 SemanticAdapter 결과는 제품 staging/CAS/UDS/재시작 대체 증거가 아니다.
- 이전 Native17은 bench에서도 SourceAdvance 후 원30s 취소로 실패했다. debug-only 실패로 설명하거나 늘린 예산의 accuracy 결과로 닫지 않는다. 현재 Native18의 실제 결과는 README/VALIDATION에서만 판정한다.
- Unknown은 원 operation key로 조회하며 재제출하거나 실행 예산을 갱신하지 않는다. 현재 fixture는 실패 뒤 별도 budget()으로 status만 관측한다. 이는 만료한 submit을 연장하거나 성공으로 바꾸는 경로가 아니다. Pending·status 관측·완결된 error trace가 accepted publication/recall 통과는 아니다.

## Acceptance

이 목록은 제품 요구와 증거의 경계이며 현재 재실행 큐가 아니다. 남은 작업과 실행 상태는 [README의 W2-13 행](README.md#w2-13)만 따른다. 이미 구현·검증한 writer/pump/SDK 경로를 이 목록을 근거로 재개발하거나 동일 selector로 반복 검증하지 않는다. 현재 추가 코드 작업은 새 실행에서 같은 owner의 실제 실패가 확인될 때만 발급한다.

1. 수정 후 funded inline-only/mixed packet·kind/CAS·canonical wire/children85·malformed/old-format·memory/cancel/refund, actual managed Text pending→covered·Source/Resync·old-pin/reopen을 확인한다.
2. 동일 source의 실제 Runtime18/old17 거절·Small·selected boot/SDK fixture를 수용한 뒤 원4K를 실행한다. 서로 다른 source의 테스트 수를 합산하지 않는다.
3. actual caller에서 install/CAS crash/cancel·ENOSPC/short-write/fsync/checked-close·staged/current/old census·last-pin reclaim을 원 PHYSICAL oracle로 검증한다.
4. 제품이 지원하는 공개 query/filter/overlay의 selected eligible corpus에 독립 exact top-k를 적용한다. 명시 unsupported predicate는 지원으로 계산하지 않는다.
5. 원 profile의 N/H/M·memory/disk/work·routing/refill·stable-A-under-B-churn·수용을 W2-09/19가 한 번 소비한다. cached adoption p95≤500ms/p99≤2s는 별도 초기 제안 목표이며 현재 측정값/업계 보장이 아니다.

## Integration ownership — one ticket, exclusive writes

채팅 간 중복 제외 경계: 이 채팅 `Review Oct 10 index closeout`은 SEARCH 고정4K oracle과 그 실제 실패 수리만 맡는다. `Review Oct 10 index closeout --보완`의 session/settlement·운영 작업은 W2-09, `증분`/`증분 (2)`의 producer/pending·signed SQL·복원 합류는 W2-02/05 원 담당이 유지한다. BOOT 실제 선택 인계는 W2-18, 전체 통합/CI/release는 W2-19, 물리 파일/FD/capacity/fault는 PHYSICAL이다. 해당 구현이나 실행을 여기에서 복제하지 않으며 공용 파일 변경이 필요한 실제 실패가 나오면 기존 writer의 함수/hunk 경계를 먼저 확인한다. 채팅 상태는 라우팅 참고이며 제품 검증 증거로 사용하지 않는다.

[단일 writer/인계 규칙](WRITE-OWNERSHIP.md)을 따른다. 공용 Core/Contract/Kernel/Runtime/SDK·format/Cargo는 통합자, directory/query는 기존 SEARCH, 실제 파일/FD/capacity/fault는 기존 PHYSICAL이 소유한다. 기존 포트·엔진·fixture를 다른 티켓 이름으로 다시 구현하지 않는다.

## Historical section links

아래는 이전 deep link의 호환 anchor다. 원문은 [보존 이력](../../../oct-17-index-closeout/tickets/W2-13-ann-product-integration.md)에서만 확인하며 현재 할 일을 추가하지 않는다.

<a id="bounded-radix-terminal-children--implementation-packet-2026-10-18"></a>
<a id="bounded-run-consolidation-contract--2026-10-18"></a>
<a id="bounded-text-entry-map-packet--2026-10-18"></a>
<a id="bounded-transaction-local-child-preflight-repair--implementation-2026-10-18"></a>
<a id="connected-publication-sequence"></a>
<a id="existing-refill-packet-boundary"></a>
<a id="final-structural-audit-value-wrappers-precede-page-grouping--2026-10-18"></a>
<a id="full-sourceadvance-cost-floor--2026-10-18"></a>
<a id="managed-chunk-layout-integration-packet--oct-18"></a>
<a id="native-file-authority-join-constraints"></a>
<a id="native-quality-acceptance-input-contract"></a>
<a id="native16-implementation-and-remaining-child-authentication-cost--2026-10-18"></a>
<a id="native16-post-window-full-request-counting-boundary--static-2026-10-18"></a>
<a id="result-contract-packet--oct-18"></a>
<a id="selected-owner-authority-packet--oct-18"></a>
<a id="single-row-authenticated-metadata--2026-10-18"></a>
<a id="transaction-bound-fence-sequence--2026-10-18"></a>
<a id="w2-13--native-ann-제품-경로-연결"></a>
