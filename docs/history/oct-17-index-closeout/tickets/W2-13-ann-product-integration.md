# W2-13 — Native ANN 제품 경로 연결

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


상태는 [SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-13)만 갱신한다. 기존 ANN private helper 뒤에 **실제 파일/Catalog/reader caller**를 연결한다. [기존 상세 설계](../../../plans/oct-13-native-incremental-wave2/tickets/W2-13-semantic-ann.md)는 참고이며 작업 배정/상태표가 아니다.

## Confirmed boundary

[Semantic mount](../../../../crates/quanta-index-semantic/src/native/mod.rs)는 `ann_lance`와 bounded `ann_store`/`ann_session`/`ann_files`를 제품 모듈로 등록한다. `NativeSemantic::with_ann`의 `prepare_searchable`은 `embedding/ann_materialization.rs`를 호출하고, Kernel source 변경·projection advance·owner materialization은 같은 AttemptContext와 기존 단일 publication CAS로 directory를 coverage에 채택한다. `validate_coverage`는 기존 owner-map 형식을 provider 호출 전에 `RebuildRequired`로 거절한다. Runtime의 실제 provider 등록이 owner와 reader의 ANN 경로를 선택한다. 컴파일·회귀 수용 여부는 [SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-13)만 판정한다.

`NativeSemanticReader::query`는 selected coverage의 object/child refs·recipe·owner-root를 검증한 뒤 `ann_query`로 들어간다. directory는 COW region map과 bounded delta member map을 가진다. 64-owner 또는 256-row delta는 bounded size-tiered COW 병합 후 기존 Lance helper로 한 번 봉인하며, 지역당 8,192행·1,048,576 cells와 파일 총량32MiB를 넘길 수 없다. 빈 벡터 member는 owner graph에만 보존하여 delta 슬롯을 소모하지 않는다. 활성 지역은 최대4,096개, query가 여는 지역은 최대8개다. format2는 재사용하지 않는 단조 지역 ID와 활성 수를 분리한다. obsolete 지역은 새 selected owner map에 그 member 참조를 유지하는 owner가 하나도 없을 때만 동일 CAS에서 참조 해제한다. pending이어도 동일 searchable member를 유지하면 회수하지 않는다. query는 sparse COW region map의 실제 key를 순회한다. sealed-region 결과는 항상 `Approximate`; no-region·selected owners≤64를 별도 확인한 경우만 기존 exact oracle를 사용한다.

선택한 region 후보는 기존 `SelectedOwner`·`current_member`의 source/projection 인증과 member/input stamp·chunk/vector ID 검증을 통과해야 한다. mask·교체·삭제는 top-k/refill 전 accept에서 처리한다. delta와 지역은 같은 exact ranking/chunk 조립 함수를 재사용한다. Runtime과 SDK는 opaque directory 참조를 허용하고 owner가 있을 때 searchable 참조를 요구한다. directory 전체 권한 검증의 소유자는 Semantic이다.

Lance staging은 지역마다 별도 bounded memory ObjectStore와 private Session을 쓰며 별도 전역 캐시를 만들지 않는다. 봉인된 실제 파일 바이트를 ≤1MiB managed objects로 설치하고 canonical file-set manifest의 정확한 child refs로 연결한다. descriptor→파일 청크는 기존 `ImmutableObjectPort`/Catalog attempt·pin·GC 경로를 재사용한다. 외부 파일 경로만 저장하거나 별도 물리 namespace·ledger를 추가하지 않는다. staging/read quota와 memory grant는 원 WorkBudget에서 선예약하며, 파일 복원은 인증된 실제 길이로 제한한다. 이 상한은 Lance 내부 RSS·물리 비용이나 Large 성능 수용의 증거가 아니다.

## Publication and file authority

현재 연결된 게시 경로가 보존해야 하는 계약이다. 새 구현 배정이나 별도 TODO가 아니다.

- admitted immutable file-set install/open/checked-close/reclaim, attempt/read-ticket/pin/GC 수명. Lance 타입을 Core/Contract에 노출하지 않는다.
- selected owner-map/input stamp·recipe와 bounded directory·region/delta/tombstone·file refs의 결합.
- 기존 owner completion/publication CAS의 단일 게시 지점에서 관련 입력 재검증과 directory/coverage 원자 채택. Semantic·Runtime·overlay·SDK validator를 같은 계약으로 변경한다.
- source coverage Complete와 ANN exhaustive/approximate/capped/incomplete 상태를 구분한다. 동일성 검사만 삭제해 완료 처리하지 않는다.
- immutable 파일의 flush/checked-close·필요 barrier와 설치를 완료한 뒤 Catalog 참조를 게시한다. SQLite transaction은 외부 파일 설치까지 원자화하지 않으므로 stage/install/CAS crash 경계와 orphan 정리를 별도로 검증한다. root CAS의 충돌 검사는 유지하고 관련 입력 재검증/안전한 재사용으로 불필요한 재준비를 줄인다. 무관한 변경을 덮거나 CAS를 완화해서 stable-owner 진행성을 만들지 않는다.

## Architecture direction and reference limits

2026-10-17 조사 및 2026-10-18 논문 4건의 arXiv 원문·버전/범위 재확인과 현재 vendor source를 기준으로 한 **설계 판단**이다. 이 layout이 업계 최고라는 증거, 모든 2026 연구의 포괄적 조사, 우리 구현/성능 수용을 뜻하지 않는다. correctness/snapshot·bounded work와 workload별 recall/latency/update 비용을 분리하는 방향을 유지한다.

| 근거 | 적용할 판단 | 적용하지 않는 것 |
| --- | --- | --- |
| [Lance read/write·version cleanup](https://lance.org/guide/read_and_write/) | immutable version의 old reader와 파일 수명 보존, compaction과 실제 reclaim 구분 | 최신 문서의 auto-cleanup/default를 pinned Lance7 지원으로 가정하거나 Catalog pin 밖에서 시간 기준 파일 삭제 |
| [SQLite isolation](https://www.sqlite.org/isolation.html) | Catalog의 transaction/snapshot과 외부 파일 설치의 권한 경계 구분 | SQL snapshot만으로 ANN 파일 pin·durability까지 보장한다고 추론 |
| [LanceDB query 공식 API](https://lancedb.github.io/lancedb/js/classes/VectorQuery/)와 [pinned Rust query](../../../../vendor/lancedb-0.30.0/src/query.rs) | pre/post filter와 refine/probe의 recall/비용 차이를 구분; 실제 메서드/실행 지원은 pinned Rust source로 확인 | 최신 JS/Cloud API·기본값을 vendored Rust0.30 기능으로 승격 |
| [FANNS 비교 연구 v3, 2026-04-01](https://arxiv.org/abs/2507.21989v3) | 실제 filter 종류·selectivity·K·scale에 따라 방법을 비교 | 단일 알고리즘/합성 무필터 recall로 모든 제품 workload 우수성 선언 |
| [RACORN-1, 2026-07 preprint](https://arxiv.org/abs/2607.00768v1) | 낮은 selectivity·query/filter 상관과 선택 집합 크기를 별도 평가 | HNSW 연구 알고리즘을 Lance IVF backend에 곧바로 이식하거나 논문 수치를 우리 제품 SLO로 사용 |
| [IP-DiskANN, 2025](https://arxiv.org/abs/2502.13826), [Slipstream, 2026-06 preprint](https://arxiv.org/abs/2606.02992v1) | 긴 insert/delete stream의 recall·업데이트/유지보수 비용과 입력 순서 편향을 독립 비교 | in-place graph 교체·adaptive controller 도입을 이번 snapshot layout 연결의 선행 작업으로 추가 |

**최소 구현 순서:** 기존 region/file binder의 실제 Native caller 연결→sealed descriptor/단일 CAS와 exact-selected baseline→기존 ANN의 bounded routing/refill. 검증 가능한 routing 방식부터 구현하고 특정 partition tree/learned router·새 scheduler를 미리 고정하지 않는다. 성능 결함이 재현되기 전 엔진 교체·GPU·quantizer upgrade는 범위에 추가하지 않는다. 기존 수치 recall0.95/adoption latency는 프로젝트 제안 gate이며 논문/업계 보장 수치가 아니다.

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

The current product reader's `validate_request` explicitly rejects nonempty
`QueryConstraintSetV1` and `lexical_scope` before embedding. Its registered
query contract supports unconstrained text plus selected-view visibility and
overlay masking. The private region helper's `accept` callback is not a public
metadata-filter API. Record selected membership/replacement/delete/overlay
eligibility separately from user-supplied predicates. A synthetic callback
filter cannot qualify the currently rejected public request. Any acceptance
case requiring that request stays unsupported/unaccepted under the existing
contract; do not silently add a predicate engine or relabel the helper test as
product support. The original filtered-quality requirement remains unclosed.

Report the actual per-region index kind as well as global traversal. With one
vector per owner, the first64-owner seal contains only64 rows, below the
pinned256-row training floor; its local search is Flat. Later size-tiered
seals can include prior runs and reach the trained branch; inspect the actual
seal instead of inferring its index kind from the delta owner count. Global traversal still
reports Approximate because directory routing is bounded. The trained 256-row
helper does not prove that ordinary SourceUtf8 publications train an IVF index.
Quality acceptance must identify row/chunk counts and selected region seals;
do not change sealing limits or adopt another engine without measured need.

## Integration ownership — one ticket, exclusive writes

| 단위 | 담당·독점 수정 | 선행·합류 |
| --- | --- | --- |
| C 공용 계약/게시 | 공용 writer: Core ports·Contract coverage/result·Kernel CAS·Runtime composition·SDK validator·parent/export/Cargo | 서명/권한/Drop 계약 동결 뒤 D/Q/P 병렬. 실제 caller 결합은 한 writer |
| D directory/준비 | SEARCH private child: seal/directory·bounded exact delta/tombstone·채택 준비 | W2-05 인증 입력 소비. Q와 같은 `embedding.rs`/`ann_lance.rs` 본문 동시 수정 금지 |
| Q query/routing | SEARCH 별도 private child: visibility filter·bounded region probes/candidates/refill·결과 상태 | D와 같은 directory 계약 소비. 공용 wire/SDK 변환은 C에 인계 |
| P Native 파일 | C의 managed chunk/file-set 결합 + 기존 PHYSICAL의 object install/read/close/reclaim | 별도 외부 Lance namespace 없이 기존 Common/Catalog object 경로와 원 fault oracle 재사용 |

아래 경계는 기존 코드 수정권을 구분한다. 완료된 D/Q/P 구현을 새 작업으로 배정하지 않는다. 추가 변경 시 private child의 실제 path/hunk를 먼저 동결한다. 기존 PHYSICAL/공용 담당 인계 없이 다른 worker가 같은 본문을 수정하지 않는다. related owner/region 단위 재검증·재사용/rebase로 stable A가 무관한 B churn 때문에 무한 stale이 되지 않게 한다. 새 일반 scheduler를 선제 구현하지 않는다.

## Acceptance

- 고정 독립 vectors/exact gold에서 multi-vector/chunk bijection·ID 중복/ABA·replacement·recipe/model 변경·selected corpus 밖 행0, old view/delete/reopen을 검증한다.
- install/CAS 전후 crash/cancel·ENOSPC/short-write/fsync/close, original FD·staged/current/old file census·old pin/last-pin reclaim을 실제 caller에서 검사한다. 합성 EIO/OS fault/media 장애는 구분한다.
- 후보/refill/fanout·memory/disk/work 상한과 필터 shortfall을 보존한다. hidden tail brute force·전역 exact fallback·owner별 N개 table 전수 검색으로 Large를 닫지 않는다.
- bounded exact profile 통과와 Large ANN qualification은 별도다. pinned LanceDB0.30/Lance7 지원을 사용하고, recall@10>=0.95·cached adoption p95<=500ms/p99<=2s 제안 gate 및 Large query SLO/입력/hardware/budget/duration은 실행 전 기존 W2-09 profile에서 확정한다. 미정 SLO는 기능 구현을 막지 않지만 qualification PASS도 아니다.
- stable A under B churn의 adoption/drain, filtered recall·분포 이동/삭제·old pin·총 N/H/M+유지보수 비용을 수용한다. 원 물리/품질 oracle는 W2-19가 한 번 소비한다.

## Connected publication sequence

1. `NativeSemantic::prepare_owner/verify_owner`가 기존 인증 입력과 owner artifact를 준비한다.
2. Kernel의 원 `AttemptContext`가 `prepare_searchable`을 호출해 owner/delta/region child refs와 directory를 같은 게시 시도 안에서 준비한다.
3. `ann_materialization`은 bounded delta 봉인 전에 활성 capacity를 검사하고, 실제 Lance 행을 attestation한 managed file-set을 설치한다. source advance는 absent/superseded delta와 완전 obsolete region만 정리한다.
4. 기존 Kernel CAS가 source/member/coverage와 searchable directory를 함께 채택한다. 이전 pin은 이전 directory의 실제 object graph를 계속 소유한다.
5. Runtime이 ANN owner/reader를 구성하고, reader가 sparse directory→bounded routing/refill→selected 인증→공용 exact ranking으로 조회한다. SDK는 같은 traversal/coverage 계약을 검증한다.

이 경로의 source와 선정 수용 근거는 [README](../../../plans/oct-17-index-closeout/tickets/README.md#w2-13)와 기존 VALIDATION에서 확인한다. 미수용 항목은 아래 Acceptance의 실제 fault·filtered quality·비용·장기 churn 조건이며, 같은 포트/엔진/runner를 다시 만드는 작업이 아니다. PHYSICAL의 진행 중 물리/계측·부하 packet은 해당 기존 담당이 계속 소유한다.

## Existing refill packet boundary

이 채팅 SEARCH가 `crates/quanta-index-semantic/src/native/ann_lance.rs`의 `query_region/query_prefix`, shared candidate sorting, 해당 기존 test owner의 exact-region filter/cap/work/refund hunk를 수정한다. `mod.rs`/공용 Core·Kernel/Runtime/SDK parent와 PHYSICAL adapter는 이 packet의 수정 범위가 아니다. 초기 prefix 폭과 누적 candidate cap을 명시적으로 구분한다. 둘을 같게 주거나 첫 prefix가 region 전체를 덮으면 기존 단일 조회 경로로 반환해 추가 merge buffer/grant를 만들지 않는다. 작은 prefix를 강제하는 자동 optimizer는 추가하지 않는다. 원 budget 아래 반복 prefix의 실제 반환/검사 후보 행을 누적하고 immutable ID로 합친다. 이 상한은 materialized 후보 행이며 Lance 내부 방문 node·거리 계산·물리 read/RSS 상한의 증거는 아니다. 각 backend query는 원 work의 read/byte charge를 추가하며 실제 probe/거리/IO 비용은 제품 결합 후 별도 관측한다. 다른 지역 routing·directory/CAS/file-set 제품 등록은 위 Connected publication sequence의 현재 경로를 사용한다. 별도 미구현 항목으로 재배정하지 않는다. 실행 상태는 README 행만 갱신한다.


## Result contract packet — Oct-18

이 채팅 SEARCH가 공용 result 계약 합류를 직렬 소유한다. 정확한 hunk는
Contract `native_protocol.rs`의 semantic traversal/result와 기존 codec/test,
`native_protocol/headless.rs`의 overlay semantic result/codec, Semantic
`native/embedding.rs::SearchResult/search_exact/NativeSemanticReader::query`와
기존 테스트, `native/ann_lance.rs::RegionPage/query_region/query_prefix`,
Runtime `native_semantic_result.rs::convert`와 `overlay.rs::convert`,
SDK `native_uds.rs::validate_semantic_result/validate_overlay_semantic_result`와
해당 기존 fixture/test다. constructor 변경에 필요한 Kernel
`tests/native_query_session_real.rs::WrongStoredSemanticReader::query`와 Runtime
`tests/native_effort.rs`의 SearchResult 생성 hunk도 함께 결합한다.

기존 wire의 source coverage/global exhaustive와 별도로 exact exhaustion,
exact cap, approximate/capped를 하나의 canonical traversal 계약으로 표현한다.
현재 exact 제품 생산자와 Runtime/SDK 소비자를 동시에 변경하며 ANN helper도
같은 계약을 소비한다. 신규 독립 engine/IR twin이나 fallback을 추가하지 않는다.
Kernel 게시/CAS, Runtime composition, PHYSICAL file admission·ledger·reclaim,
INT/SDK producer와 native_uds feature guard는 이 packet의 수정 범위가 아니다.
물리/INT 채팅의 현재 작업과 source hunk를 대조한 뒤 정한 경계이며
별도 채팅 메시지 전송/소유권 인수 기록으로 사용하지 않는다.

Canonical result contract: `NativeSemanticTraversal::{ExactExhausted,
ExactCapped, Approximate { capped }}`. `certifies_exhaustive(coverage,
skipped_stale)` requires exact exhaustion, Complete source coverage,
covered==desired and no stale owner. A Partial/Pending source can have
ExactExhausted traversal while remaining globally nonexhaustive.

The actual exact producer tracks scan caps separately from source/stale
coverage. Runtime durable/overlay and SDK consumers reject incompatible
exhaustive claims. Native semantic replies require traversal; missing or
unknown values cannot default to exact or empty success. Producers and
consumers change together without a legacy decoder or a parallel IR.

The public API guard covers contract-base, Contract and SDK. Reconcile a
changed baseline only after checking its actual producer/consumer and
custody contract. The traversal type/fields are one part of that common
integration surface; unrelated producer or feature changes retain their
original writer. Current guard results belong only to README/VALIDATION.

## Selected owner authority packet — Oct-18

SEARCH owns `native/embedding/selected_owner.rs`, its registered
`tests/selected_owner_tests.rs`, and the exact `embedding.rs::search_exact`
loader call plus existing semantic fixture roots/edges. The product exact
reader authenticates every selected owner against the selected source or
producer map before stale/empty/overlay masking, then checks the full canonical
owner/member object references and GC edges before scoring. Projection metadata
uses the existing bounded Core decoder. Pending coverage includes its original
`previous_missing` edge in the existing selected-envelope check.

This is the common selected-input acceptance boundary used by current ANN
candidates. File/Catalog lifetime and ANN publication use the managed graph
and original CAS described above; remaining physical acceptance uses its
original owner and oracle.
Do not duplicate this logic in a region router or accept a sealed candidate's
self-consistent input stamp as proof of selected source membership. The existing
PHYSICAL Common binder/ledger, Kernel publisher and SDK producer are unchanged.
No additional backend, physical namespace, scheduler, unlimited fallback or wire
contract is introduced by this packet.

### Native file authority join constraints

The current `ImmutableObjectPort::install`/managed `FileObjectStore` installs
bounded wire objects through the original Catalog scope; its reclaim path
authenticates and unlinks those objects' actual physical incarnations. It does
not install/reclaim an arbitrary external Lance dataset merely because a JSON
descriptor mentions that dataset's path. `NativeObjectReadIo`/`NativeObjectWriteIo`
reuse Common capacity and checked FD retirement for the existing object store.
`PublicationLanceAdmission::for_work` instead requires an already-held legacy
publication allowance. Neither fact alone supplies Native attempt/read-ticket
authority for an external file set.

The current managed layout connects actual file bytes through ann_files
chunks and the descriptor/member child graph to those original Catalog
references. It retains old-view reachability and uses the original physical
reclaim path. A raw pathname, unrelated allowance or self-attested seal cannot
substitute for that connection. Keep fault/last-pin physical acceptance under
the existing PHYSICAL oracle; do not create a second storage backend/ledger
or silently stage an unbounded dataset. Source connection and physical fault
qualification remain separate claims.

### Managed chunk layout integration packet — Oct-18

Root C owns parent/Cargo/Core/Kernel/Runtime/SDK integration; workers own separate `ann_files.rs`, `ann_store.rs`/`ann_session.rs`, and `embedding/ann_directory.rs`/`ann_query.rs`. The original external-file join requirement is fulfilled in source by storing actual Lance bytes as existing managed immutable objects. Every descriptor retains its exact chunk/member map children. There is no new physical storage kind.

Acceptance consumes the same packet: grouped strict and fixed-vector tests; actual managed FileObjectStore/Catalog old-pin/reopen/reclaim and fault paths; actual daemon UDS seal/read/restart; filtered recall, bounded funding, churn/region debt and RSS/temporary peak. The README distinguishes executed acceptance from residual scope; do not repeat completed source/fixture work. The active4,096-region refusal remains a finite safety bound, not sustained-churn or density/SLO qualification. Format2 separates the monotonic region ID from active count, retains IDs even when owners become empty, and rejects older directory artifacts with RebuildRequired rather than an IR upcast. An ordinary prepare inspects one bounded region with a persistent cursor. At capacity, the existing attempt checks the entire bounded active keyset for a fully obsolete region before any Lance build; original budget refusal remains explicit. This retires fully superseded regions within the original CAS. The bounded small-run consolidation contract below additionally coalesces still-selected members of eligible small runs in that same attempt; it is not a generic/background compactor. Before file-set installation, the build reads the actual bounded Lance rows and checks the canonical `rows_digest` against the input seal. Selected opens verify immutable file hashes binding those attested bytes; they do not rescan every table row. Returned candidates are independently authenticated and recomputed against selected member artifacts, and sealed regions never certify global exhaustion.


When overlay documents consume the entire owner limit, the durable `seen`
set allocates no backing and requests no zero-byte reservation. Other paths
retain the global nonzero admission contract. Matching-provider mask-only
coverage uses an actual Core MemoryScope; empty-directory and65-zero-vector
reads alone do not reproduce this boundary.

Lance table creation can move Arrow input into a detached task. Each physical
data/child/validity buffer must retain the original admitted Retained
MemoryLease until its last backing reference drops, including outer-future abort. Normal request
cancellation waits for write terminal completion, then rejects publication
under the original request. This shares one original charge without a payload
copy. Retained escape capacity is admitted before allocation so a caller abort
does not violate the original execution ledger. This does not claim aggregate
vendor RSS coverage or a join-all boundary.
The regression packet covers pointer identity/last child release, detached
input after outer abort, and refusal only after controlled write completion.

Execution outcomes are recorded once in the existing [validation anchor](../../../plans/oct-10-index-closeout/VALIDATION.md#w2-13-managed-ann-layout-2026-10-18);
current status and remaining assignment are owned only by [README](../../../plans/oct-17-index-closeout/tickets/README.md#w2-13).

## Bounded run consolidation contract — 2026-10-18

A sequential 64-row publication layout can produce many unrelated random
regions. Centroid top8 routing cannot recover a gold row from an unopened
region, even with perfect regional retrieval. Keep the original recall gate;
legacy single-dataset ANN results do not qualify this Native layout.

Coalesce the delta and up to eight smaller-or-equal immutable runs, comparing
against the growing accumulated row count, before one final Lance build.
Inspect and cache at most64 source descriptors once per ordinary seal,
starting at the existing persistent maintenance cursor and wrapping once.
Reuse those candidates across all carry rounds; the cursor advances on
ordinary publications so a fixed low-ordinal prefix cannot starve later runs.
At the original active4096-region cap, inspect its full bounded set under the
original budget and allow one fitting larger run to free a slot. This slow
path remains explicitly fallible; do not rescan4096 descriptors per merge.
Reuse original owner artifacts, admitted work/memory, file-set limits and the
same directory/Catalog CAS. Member maps are scanned in ordered pages of at
most64, with total owners bounded by declared rows and rows/cells by the
existing region envelope. Authenticate original members and selected coverage;
retain pending members with the same searchable reference, deduplicate equal
references, and exclude only members no longer selected. Stage source-region
unlinks and the new descriptor in the same attempt; failed admission/build/CAS
cannot publish a partial replacement. Old views retain old child graphs.

The policy improves small-run layout; it does not guarantee at most8 active
regions or recall0.95 for arbitrary dimension, churn or corpus size. Existing
row/cell/file limits and query caps stay unchanged. Dimension4096 permits only
256 rows per region; a small-width4096-row result cannot qualify that profile.
Required checks include carry beyond128 rows, multi-page member retirement,
pending/replaced/deleted members, original refusal and old-root retention,
then independent Native quality acceptance. The pre-change workspace batch is
not evidence for these source changes.

### Native quality acceptance input contract

Use the existing `native_product_process` daemon/SDK fixture and the original
`AnnRailConfig::DEFAULT`: 4096 distinct source owners, 64 dimensions, 64 queries,
top10 and mean recall at least0.95. Reuse the harness generator and independent
exact oracle; the Legacy report remains a separate execution path. The HTTP
fixture serves deterministic vectors through the configured OpenAI transport,
checks the model/dimension, and rejects unknown corpus/query inputs. This proves
the product pipeline with a controlled provider, not an external model service.

The existing8D one-hit fixture keeps its original inputs. The separate quality
fixture declares `max_owners=1024` before execution, matching the original
regional candidate ceiling; its requested top10 cannot be accepted with the
one-hit fixture's eight-owner cap. Keep production route/candidate caps,
per-request funding, dispatch retry count and assignment deadline unchanged.
The external ingestion watchdog is orchestration, not a service SLO increase.

SourceAdvance admission must succeed before this quality case can measure ANN.
Keep byte/hash/child validation at consumption, bounded pending/active memory,
child-first installation and scratch-COMMIT-before-physical ordering. Remove
actual duplicate producer/traversal work rather than resetting budgets or
lowering fees independently of work. The original refusal remains visible
until the original case passes; static work bounds are diagnostic evidence.

Stage actual UTF-8 sources through bounded SDK pages, publish SourceAdvance,
and wait for original owned-work completions. Queries must authenticate the
exact selected view, instance, complete coverage, approximate traversal,
non-exhaustive result, unique known owners, whole-file vector identities, full
pages and independent exact scores. Match the oracle's string tie ordering
with the fixed owner identities. Repeat the query set after daemon restart;
record first-open and reopen recall separately. Do not publish unmeasured index
bytes/build time or promote small Semantic/Kernel fixtures to this acceptance.


### Full SourceAdvance cost floor — 2026-10-18

For frozen functional source `39cc4008194980d0031cf17a80cb706aa55607751196a25ae236c7cfa2cc5d88`, the original4K fixture starts with empty Source maps and a required NativeText member. This is a static logical WorkBudget lower bound for the current representation and fee paths; it is not physical IO, elapsed time, or an impossibility result for future implementations.

- N=4096 owner keys `owner\0 + producer + padded row-{i}` have819 distinct adjacent longest-common nibble prefixes; the corresponding `path\0src/{i:04}.rs` keys have456. Compressed source maps require V=9467 nodes and E=17657 direct edges, plus4096 distinct path-owner markers. The staged mutation-map leaves point to mutation records, not source-record values; they cannot substitute these source-map nodes.
- Current managed Attempt reservation28 + unique confirmation21 gives49 base reads per new object. Source nodes/markers therefore cost49(V+N)=664587; their edge fees add5E=88285. Grant even the best possible4-unit credit for every internal edge, including cross-batch edges that cannot actually get it:subtract4(V-2)=37860. External source-record children cost8 reads per reserve/confirm pass; path-marker children cost7 per pass, adding30N=122880. Catalog Source subtotal is at least837892.
- The original `text_spec()` is CoreRequired/SourceLocal. Every supplied record is valid UTF-8 under the configured TextRecipe policy. NativeText's ContributionWrites emits a distinct contribution carrying each distinct source-record reference; no prior text contribution exists. Its N objects add at least49N=200704, even ignoring all their edges and every posting/inventory/map object.
- Full-request Catalog-only lower bound:664587+88285-37860+122880+200704=**1038596 >1000000**. Scratch reads, record/body verification, map traversal, frontier/member/publication work and all remaining Text objects are omitted. Owner/path branch counts were independently recomputed from the fixture key bytes. Actual charge paths were traced through AttemptContext→FileObjectStore→managed Catalog, Source path/map producers and NativeText ContributionWrites.

The existing original1M refusal remains FAILED. An indexed path seek improves scan work but leaves its one logical read per returned row unchanged. A further implementation must remove enough actual work across the **entire SourceAdvance including required Text**, with the bound recomputed after the change; moving the refusal beyond Source map installation is insufficient. Do not tune fee constants without eliminating their work, reset budgets, weaken child/checksum/global-fence checks, or count an alternative quality profile as the original cost gate. The separate functional-versus-cost contract question is pending; preserve the original funding until an explicit decision. No packing/storage-stack/counter-authority replacement is authorized by this arithmetic.


### Transaction-bound fence sequence — 2026-10-18

Reuse `native_counter_v1/attempt_fence` and one shared scalar read/advance implementation. Managed Attempt reservation finishes every external-child adoption before constructing a sequence borrowed from that exact Transaction. Its at-most256 inputs may reuse one authenticated fixed-size counter snapshot; every fence still performs its own update and returns the same monotonically increasing unique number as the scalar path. Each update compares both prior number and checksum, refusing an unexpected intervening writer before overwriting it. No second counter, range/number reuse, callback-spanning permission cache, wire/schema change or new public authority.

The sequence must be bounded by the admitted input count and unusable after refusal. Generic/scalar calls retain fresh counter reads. Ingress/Bootstrap, confirmations and existing-slot collision validation stay on their existing flow. First needed counter read is charged before SQL; subsequent omitted SELECTs alone receive a1-unit read credit. Live rows that allocate no fence retain the old reservation envelope. All other fees/limits, global nonreuse, rollback and reserve→physical sync/barrier→confirm remain.

Required gold:real256-input Attempt executes one counter SELECT, assigns the exact256 successive global fences and advances the durable counter by256; all-live replay reads no counter and receives no new credit. Counter CRC/type/length/positive-domain/overflow checks, same-transaction stale snapshot refusal, refusal poisoning and whole-prefix rollback/reopen must hold. Recompute the entire Source+required Text floor after the code change; this bounded optimization alone cannot close the original1M gate, so do not repeat that product run merely to observe the same predictable failure. Execution status belongs only to README/VALIDATION.


At the counter-sequence checkpoint, before the single-row metadata reader below, the optimistic lower bound changed without changing the historical39cc receipt:with M=17659 mandatory Source/path-marker/Text-contribution objects and private batches bounded256, at least G=ceil(M/256)=69 batches perform a first counter read. Fresh Attempt base27 + unique confirmation21 gives48 per object; first reads add G. The new Catalog-only floor is48M+G+5E−4(V−2)+30N=**1021006 >1000000**. This still excludes all extra scratch/Text/frontier/member/publication work. The17590 difference between the two lower bounds is not measured request savings; actual smaller batches increase G. The executed256-object gold instead establishes the exact local counter SELECT reduction256→1. Do not resubmit unchanged4K merely to confirm this remaining static contradiction.


### Single-row authenticated metadata — 2026-10-18

Registered/provisional metadata uses one canonical bounded reader in `Catalog/object_install`. A keyed SELECT guards payload BLOB type and1..1048576 length and checksum BLOB type/32 length in SQL. Rust borrows the guarded cells, admits charged bytes before decoding, checks the original domain checksum, and decodes the same RegisteredObject. A present malformed row cannot masquerade as absence. Root, external-child, registered-match and provisional-match callers retain their original identity/canonical-bound and complete edge-ledger validation. There is no intermediate payload/checksum Vec copy, second shape query, new schema, cached permission or alternate decoder.

Managed unique confirmation credits exactly one removed shape SELECT (Attempt21→20;Ingress17→16). A successful external-child metadata read uses one SELECT instead of two; byte/edge allowances and every other fee stay unchanged. Generic/duplicate paths keep conservative envelopes. Required gold covers real256-input first confirmation and replay statement counts, both ledgers under SQLite result-length limits with an unguarded TooBig control, BLOB/TEXT/empty/oversized shapes, checksum refusal, admission before JSON decoding, and unchanged corrupt fields. Existing full-batch/edge/rollback/reopen gold remains required.

For the original4096 fixture the earlier minimal counted subset now yields986963, but that subset omits the mandatory Text owner-map. `ContributionWrites` emits an owner→contribution edit for each admitted source and `apply_postings` installs `root.owners` through the same map engine. Its raw32-byte owner keys have the same819 distinct adjacent common nibble prefixes as the Source owner keys, hence L=4915 additional map nodes and T=L−1+N=9010 edges. Source, staging and Text owner-map values differ; these nodes cannot substitute their different committed child graphs.

Count M=V+2N+L=22574 mandatory objects, G=ceil(M/256)=89 first counter reads, fresh base27+20=47, known edges E+T=26667, and optimistically credit every internal edge across batches:4(V−2+L−1)=57516. The two Source leaf-value classes alone still require external-child reads26N=106496 after metadata fusion. At this pre-terminal-inlining checkpoint the stronger Catalog-only lower bound was **47M+G+5(E+T)−4(V−2+L−1)+26N=1243382 >1000000**. All other Text inventory/posting/term objects and edges, scratch, record/body, frontier, member and publication work remain omitted. This is a stronger count of required work, not a performance regression, measured savings or an impossibility claim about future representations. Original4K runtime acceptance therefore remains open; do not repeat unchanged runtime solely to confirm this contradiction.


### Bounded radix terminal children — implementation packet 2026-10-18

C integration owns the existing `storage/map.rs` Node/codec/bootstrap/update/lookup/scan hunks and `map/scratch.rs` node-read envelope for this repair. No new physical adapter/file-set/capacity/retirement path or worker is introduced. Existing Source/Text callers retain PersistentMapPort and its original budgets. The selected repair embeds short terminal-child key tails and their value refs in the parent radix node, eliminating those intermediate immutable map objects. An embedded value remains an authenticated direct RegisteredObject child, never a fabricated ObjectRef or hidden GC edge. Each inline tail is bounded256 nibbles and there are at most16 branch slots; long tails stay indirect. Public roots always reference real immutable objects. Bootstrap and incremental editing must produce the same canonical roots; retained old roots keep their own direct value graph. The current persisted node magic changes once and old maps fail closed with RebuildRequired; no alternate IR/upcaster/legacy reader.

Required gold:independent BTreeMap lookup/scan/CAS/prefix/delete laws, exact16-leaf object-count/direct-edge oracle, inline/indirect boundary and maximum-key handling, corrupted masks/tails/children/trailing bytes, explicit old-format rejection, cold reopen, changed-path rewrite bounds and existing commit-before-physical/custody refusals. Run storage strict/lib and the existing Source/Text affected scopes together after static integration. Recompute the full mandatory Source+Text object count before submitting any expensive original4K batch. This is a producer representation repair inside the same engine, not budget enlargement, fee retuning or a separate packing/storage stack.


The terminal-child codec uses QIMAP002 inside unchanged QIOBJ001 immutable objects. Native state15 and its capture-format digest bind the new map semantics before Catalog initialization/recovery; Native14 markers and old Catalog stamps refuse before rewrite. The legacy daemon also refuses Native15. Current marker/SDK fixture consumers and the wire-surface inventory move together; source-capture2 and public map methods remain unchanged. Header15 + prefix≤8192 +16(length2+tail≤256) limits payload12335 bytes and existing at-most17 physical children; scratch read bound is12895 wire bytes. Data buffers keep original WorkBudget memory grants. A point lookup decodes the bounded parent including sibling tails; no latency/RSS improvement is inferred from file-count reduction.

For the identical fixed Source/path/Text-owner subset, every terminal tail is below256 nibbles. Source map objects fall9467→1275;Text owners4915→819. Markers and contributions remain4096 each, so M22574→10286. Known edges become9465+4914=14379;optimistic internal credit becomes4(1273+818)=8364;first counter reads G≥41;Source external-child reads remain106496. This subset's floor is **47×10286+41+5×14379−8364+106496=653510**, compared with1243382 for the old representation. These are comparable static subsets, not measured request savings.

The original `row-{i}` body also has a distinct indexable numeric token per owner. The producer indexes both sensitive and folded surfaces; grant perfect content-addressed sharing between those surfaces. Even then `apply_postings` builds at least N distinct one-owner map roots and N wrappers, each with one physical child. Their owner key or child root distinguishes every object from this subset and from each other. Ignoring all added first-counter reads and generously granting4-unit internal credit on every added edge (including actually external ones), these add at least47(2N)+5(2N)−4(2N)=393216. At the Native15 checkpoint the full-request lower bound was therefore **1046726 >1000000**, still omitting every Text inventory/term-map object,all scratch/frontier/member work and all extra checks. Original4K is still statically excluded;no repeat execution merely to rediscover it. At that checkpoint the next representation work was in the actual small Text inventory and singleton posting producers;the later Native16 packet implements that repair. It was not a Catalog fee adjustment.


### Bounded Text entry-map packet — 2026-10-18

C owns the actual NativeText inventory/posting producer/reader/private codec hunks, their existing disposable SQL cursors, overlay inventory lookup and common Native format binding. One private EntryMap represents either up to8 sorted fixed keys of at most32 bytes plus real occurrence refs, or the existing persistent MapRoot. It serves both file-local inventories and term posting memberships. Inline occurrence refs remain direct immutable-object children;no synthetic ObjectRef,second engine/authority/cache or public port is added. Large keys/maps retain the existing COW engine. Choose the representation from the resulting entries,including small↔large updates/deletes;initial and incremental construction use the same canonical policy.

Resolve small candidates before physical installation using the original immutable prior and complete CAS delta. Bind each candidate to its original edit-stream digest and revalidate the complete global stream before any map installation or metadata publication. Large-tree patches keep changed-path COW;an inline base may seed only its at-most8 old entries into the fresh map builder. Reject missing/reordered/changed/extra/foreign edits,wrong final cardinality and late cursor errors;do not treat a successful prefix as a complete index. Preserve64-root batching,original memory admission/cancel/attempt custody,complete child refs,token totals/DF/selected-source validation and reserve→sync→confirm. Gate the new private artifact encoding through the existing Native state format;no old-format reader or upcast.

Gold must independently cover inline bounds/codec/canonical padding/order/direct children,mapped delegation,one initial entry without a map object,8→9→8 and long-key transitions,base CAS failure,changed replay digest and complete EOF,query/phrase/overlay equivalence,old pin/reopen/GC and unchanged unrelated-owner work. Group strict and affected existing producer/consumer tests;no original4K rerun until the whole request's cost contradiction has been removed.


#### Native16 implementation and remaining child-authentication cost — 2026-10-18

The one EntryMap is implemented across contribution inventory, posting memberships, prepare/reuse, point/page/query/overlay readers and exact physical child lists. Inline metadata has fixed8 slots/key32;tree metadata requires empty inline slots. The shared EntryMap patcher admits plans/bases/results before use, checks complete original-base CAS and final small cardinality, compares independent per-root replay digests, rejects incomplete/foreign/global reordered input and retains existing64-root COW batching. A known empty inline inventory validates and ends without allocating a shared empty page. Native16/current marker16/Catalog stamp16 gate the private encoding;authenticated old15 and marker15 require rebuild before mutation. Public ports,source-capture2,QIMAP002 and original budgets stay.

Posting production now identifies exact payload+children and reuses data refs only from earlier confirmed top(surface0) rows in this disposable prepare. A partial SQLite index on top(n) supplies a keyed seek;the same original attempt retains confirmed custody. No pending ref enters that index and it supplies no registration authority. Final parent-map installation authenticates registered children again. Additional index/primary-key bytes are included in scratch write demand. Failed install/late stream/refused memory cannot turn a prefix into publication. Within-page object dedup remains the existing store's responsibility.

This is not yet original4K acceptance. Recompute the whole request rather than subtracting removed object counts alone. The earlier identical10286-object Source/marker/contribution/Text-owner subset has minimum653510 reads,including106496 Source external-child reads. Conservatively OMIT all those external Source reads:547014. A row-{i} inventory has four occurrence entries(two tokens × two case surfaces) plus its SourceRecord child. The existing Catalog producer validates/adopts each of these external children independently on reservation and confirmation. Additional contribution edge allowance is25 per owner;external child reads are2×(7+4×6)=62 per owner,so add87N without recounting the base47 already in547014. Even granting perfect confirmed sensitive/folded wrapper reuse,the N distinct numeric singleton wrappers add at least47+5+2×6=64 reads each;their children were confirmed in earlier calls and are not earlier inputs of that wrapper batch. At the pre-window checkpoint the unchanged Catalog path required at least **547014+(87+64)×4096=1165510 >1000000**,before all Source external reads,term-map work,scratch,frontier/member/publication. This is a stronger counted subset,not a speed regression or measured result. No original4K rerun until that contradiction changes.

The following owned repair targets same-transaction repeated external-child authentication/adoption in the existing Catalog managed reservation/confirmation. Preserve complete parent edge ledgers,registered metadata checks,GC phase refusal and rollback. Reuse must be bounded and borrowed from the actual transaction;never a cross-transaction authorization cache or fee-only discount. Ingress/bootstrap/duplicate behavior and original memory/work admission need independent adversarial checks before promotion. Other producer/pending/AST/BOOT owner boundaries remain unchanged.


#### Bounded transaction-local child preflight repair — implementation 2026-10-18

The existing managed installer now borrows the actual locked Transaction in AdoptedChildrenInTransaction. A fixed64-slot/direct-mapped window remembers only complete registered-metadata+edge-ledger+GC-adoption checks that succeeded in that transaction. Full ObjectRef equality is required;collisions cause ordinary revalidation. No metadata/payload/FD persists,failed/provisional/frozen children never enter,and a new transaction starts empty. Parent-specific canonical registrations,full edge ledgers,scope intents,child-first checks and final registration/CAS remain. This covers Attempt reservation and existing managed Ingress/Attempt confirmation;Ingress reservation/bootstrap retain their old path.

The data-only reservation billing prediction uses a separate fixed window and is destroyed before authentication. It credits the existing four-unit omitted-adoption envelope only for actually skipped earlier-input or same-window reservation adoptions. require_child_charged still bills every reached complete authentication;misses/collisions retain the old work. No WorkBudget reset/refund or limit increase. Each fixed window admits original Temporary memory before birth;zero-child operations allocate none,and billing/auth windows do not overlap. Cancellation is checked even on hits.

New independent Catalog checks cover reached keyed SQL counts,all duplicate physical child entries retained with unique edge ledger rows,new-transaction corruption refusal,late colliding corrupt-child rollback,reservation read parity for1 vs4 repeated edges,original memory/cancel/refund boundaries. These are implementation/verification scope,not yet accepted results. The1165510 pre-window floor cannot be reused as the current floor or claimed as measured savings;recompute all affected contribution/posting/map/batch work after gold. Original4K/quality/operational/whole-workspace gates stay open until their actual runs.


#### Native16 post-window full-request counting boundary — static 2026-10-18

Give the new bounded window ideal collision-free retention and ideal256-object groups,even though actual limits/collisions can require more work. N4096 contributions have5N physical child slots. Their N SourceRecords are distinct and two occurrence refs are common. At least16 groups give N+32 external authentications per reservation/confirmation phase. Additional contribution reads are therefore5×5N−4(4N−32)+14N+384=94720. The N distinct numeric wrappers have one common occurrence ref;their object/edge/adoption/authentication minimum is52N−4(N−16)+192=196864. These terms do not repeat the contribution object base already in653510.

The omitted final Text term map also matters. Independently built a symbolic nibble trie over the actual keys {0,1}×({decimal0..4095}∪{row}),with one unique numeric-wrapper symbol per owner and one common-row-wrapper symbol. Applied canonical unary compression and bounded terminal inlining,then deduplicated identical case subtrees. There are409 numeric prefixes that are also values;the QIMAP002 boundary retains a value node and a following digit-branch node for each. The minimum graph is821 distinct map objects,4918 direct physical child slots,821 internal slots(including the duplicate root-to-shared-case-child slot),4097 distinct wrapper values. All tails fit256. This is a pure structural model of the existing IR,not a second product engine or actual timing measurement.

Even allowing4-unit internal credit on every internal slot and omitting all first-counter reads for the added objects,the term-map reads alone are at least47×821+5×4918−4×821+14×4097=117251. Each external wrapper has at least one child,and these wrappers were installed in earlier calls,so both later Catalog transactions must authenticate it. Together the conservative Native16 known subset is **653510+94720+196864+117251=1062345 >1000000**. It still omits common-row posting-map/wrapper objects,scratch,record/body/frontier/member/publication and nonideal batches/collisions. No original4K rerun is justified by the child-window repair alone.

The remaining blocker is the existing persistent map's small-object granularity through the actual Source/Text producers. Another prefix/authentication cache or a larger window cannot remove this ideal-sharing contradiction. The next repair must reduce real map object/edge/installation work under the same public map contract and managed store;retain canonical initial/incremental roots,complete original-base CAS,point/page/prefix semantics,old-root pins/reopen/GC and bounded payload/memory. Select a bounded page/leaf grouping in that existing engine only after auditing its current Node/codec/bootstrap/update boundary;do not introduce a parallel map engine,physical binder,runner,compactor,relaxed fees or larger request caps. This is remaining code,not an unrun check that can be marked complete by another unchanged benchmark.


#### Final structural audit: value wrappers precede page grouping — 2026-10-18

The preceding page/leaf suggestion is insufficient by itself. The1062345 subset also omitted external authentication of the N distinct Text-owner contributions,each with five physical children;that adds22N=90112 reads. More decisively,give the current producer/port contract the impossible best case of zero map-object base costs and zero internal edges. Still retain the necessary four classes of map value edges(SourceRecord,path marker,Text contribution,numeric wrapper),one common-row wrapper edge,the3N distinct marker/contribution/numeric-wrapper object bases and ideal child-window sharing. The minimum is47(3N)+5(4N+1)+(62N+14)+94720+[5N−4(N−16)+192]=**1012499 >1M**. All counter reads and other work remain omitted. Therefore page grouping alone cannot close the unchanged ObjectRef-only value contract.

The Native16 audit confirmed source::install_path_markers serializes an OwnerKey into a separately installed immutable object per owner only because MapEdit/MapPage/PersistentMapPort values are ObjectRef. The producer and exact-path readers then load that object to recover the small datum. Numeric posting wrappers have the same unnecessary small-wrapper shape. These are real registered objects,not fake refs,but their per-object work dominates. Encoding owner bytes as ObjectRef is forbidden. Appending owner bytes to path keys would also break the existing4091-byte path/4096-byte map-key bounds;raising those limits or introducing hash-only path identity is not the repair.

The next C-owned contract-first repair is one canonical bounded map value:existing real object references,or small inline metadata with its explicit real child refs. Inline data must never expose a fabricated RegisteredObject/ObjectRef. The existing map node owns every inline child's physical GC edge;scalar owner metadata has none. Preserve full CAS/value-kind identity,canonical initial/incremental roots,complete key/value/child decoding,point/page/prefix semantics,all current key/request limits and source path identity. Fund escaping inline bytes/refs/shared backing through original retained memory before birth;temporary decode backing cannot escape. The selected fixed-size contract is ≤32 inline data bytes and ≤1 explicit real child. Source owners fit exactly32 bytes with no child; singleton postings use owner32 plus the occurrence child; tree postings use DF8 plus the map-root child. Small multi-owner postings retain the current bounded wrapper. Canonical scratch values are ≤67 bytes; map-node payload is ≤12930 bytes, wire ≤13490 bytes and physical children ≤17. Retained page/edit accounting uses the actual value size. ObjectRef values remain for original source objects and larger metadata;the actual producer chooses the representation deterministically. No second map engine,physical stack,issuer,raw FD,compatibility reader or uninstalled identity.

Freeze MapValue/MapEdit/MapPage/cursor/port signatures in Core first;then update the existing Storage codec/scratch/install producer and all actual Kernel/lexical/RepoMap/semantic consumers. Keep parent/export/format changes under C. Source path markers and eligible Text posting wrappers should become inline values under that same contract;preserve one apply_source path for advance/resync and body-only path-map stability. Scalar-vs-reference values with identical32 bytes must remain distinct,all inline children must authenticate/reclaim,and malformed/missing/extra child data/changed input/cancellation must fail closed. Cross-kind CAS,initial↔delta equivalence,pinned history/reopen/GC and original memory/refusal gold precede one original4K run. Recompute actual whole-request cost before deciding whether additional page grouping is still needed.

Read-only routing search found41 Quanta Rust files referencing PersistentMapPort and no Rust reference in the searched Semantica checkout. This is discovery,not compilation or an ownership transfer. Enumerate exact implementing/calling hunks before edits and keep the existing external producer/pending/AST/target lanes independent. A new persisted value encoding must co-cut Native format/marker/Catalog stamp and refuse the previous current format before initialization/recovery. Native16 results remain their own accepted historical focused checkpoint. The Native17 contract, QIMAP003/scratch producers, Source path values and singleton/tree posting values are now implemented with all actual workspace consumers; workspace all-features/all-targets compilation and affected strict Clippy passed. Independent raw-byte/CAS/child/history/reopen and managed Text assertions are in the grouped functional batch. Current execution status remains in README and VALIDATION; compilation does not accept original4K/GC/operations or external producer/cutover work.


Native17 historical boundary: Core/Storage/Scratch/Contract/Catalog gold and selected actual Source/Text/RepoMap/ANN/Runtime caller, old pin/reopen and format guards were exercised (executions in VALIDATION). The unchanged original4K gate failed at SourceAdvance with Unknown and an immediately observed Pending operation before quality/reopen. A subsequent bounded capture identifies cancellation at the original30s deadline after Source application succeeded; it does not establish read-budget exhaustion or an accepted publication. Native17's single-child contract above describes that historical checkpoint, not the current Native18 representation.

### Native18 compact metadata contract

User confirmation on2026-10-18: the original4K input must satisfy the per-request1M-read and30s product performance conditions. A separately enlarged accuracy budget does not meet this acceptance condition. Preserve the remaining original work/byte/memory/IO caps as well.

The measured two-token corpus still installed one JSON contribution per owner during Text preparation, then separate source-pending coverage objects. Repair those producers together in the existing map engine and the existing logical Contribution/OwnerCoverage IRs. Keep data32 and extend the fixed Copy value to at most five real children: one source plus four inventory slots, retaining order and duplicates. Larger inventories and covered/projected/contextual coverage retain their object representation. No CID or registered object is invented for inline data.

The canonical value wire is at most195 bytes; QIMAP004 payload remains at most12930 bytes with at most85 physical child slots and15666 wire bytes. QIMAP001/002/003 are refused. Native state/marker/Catalog stamp co-cut to18 and refuse17 before initialization. Allocate and fund the actual physical child capacity before birth; do not allocate85 slots for an object-only node. Derive scratch read, retained page/edit and pending traversal bounds from the actual representation. Branch prefixes retain their existing bounded backing beside one reference/value selector, without heap-indirected value variants.

Text owner lookup, universe/overlay scans, semantic selected-owner/ANN maintenance, coverage retirement/materialization and the Kernel edit spool consume the same MapValue contract. Source/member/projection/occurrence maps that only contain objects remain strict object projections. Pending coverage stores policy32 and one real source edge only when covered/searchable/projection are absent and producer context is zero. Every other state uses the original authenticated object form. CAS includes kind, complete data and ordered child list; map nodes retain all inline GC edges.

Independent wire/edge/order/count/malformed tests, a full85-edge node, old-root history/reopen, actual managed two-token contribution transition, pending-to-covered materialization, and memory denial/cancel/refund laws precede another original4K run. Keep all original request/work/byte/memory/IO/deadline caps, corpus and quality/reopen oracles unchanged. Source compilation and functional results remain separate from original4K and operational acceptance in the SSOT and execution ledger.
