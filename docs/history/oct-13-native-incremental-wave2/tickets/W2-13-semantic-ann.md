# W2-13 — 기존 Semantic ports 뒤 admitted incremental ANN backend

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-13)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R21,R22 / NI-05. 요구는 [RFC §9.2/9.2.1](../../../rfc/oct-10-native-incremental-vnext.md#921-dynamic-ann-evidence-and-independent-selection-gate), 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse and proven gaps

- `PRIVATE_CODE_PRESENT`: `ann_lance.rs`의 bounded region·streaming·grant·checked sizing을 재사용한다. 테스트 전용 parent mount는 private helper를 검증 대상으로 포함하며 제품 등록·대규모 수용을 뜻하지 않는다. 기존 초안을 보완하고 새 ANN 엔진을 쓰지 않는다.
- 기존 per-owner vector artifact/chunk membership·recipe·prior reuse는 W2-12의 권한을 소비한다. 임베딩·chunker·논리 ID를 ANN에서 다시 만들지 않는다.
- **행 식별 수리 범위:** `RegionRow`/`RegionCandidate`·Arrow schema·v2 row digest·정렬·중복·선택 입력 대조에 `(owner, member, vector_id)`를 결합한다. 기존 1024-byte vector ID 한도와 variable-length ID의 birth/result grant를 재사용한다. 한 owner의 여러 vector와 selected input ABA·재오픈 fixture는 private helper 증거이며 실제 chunk artifact/physical file-set 연결은 별도 수용이다. 실행 결과는 SSOT/VALIDATION을 따른다.
- **확정된 제품 결합 부재:** Kernel source/projection/completion은 `coverage.searchable=owners.0`을 게시한다. Semantic reader뿐 아니라 Runtime result/overlay와 SDK도 그 동일성을 검사한다. `searchable: ObjectRef`와 `coverage_children`은 directory를 보유할 수 있지만 원자 채택 caller와 Native file resolver는 아직 없다. 이 공용 결합은 해당 writer의 작업이다.

## Fixed architecture and minimal contracts

1. 논리 identity는 기존 `(owner, member, vector_id)`와 owner input stamp/recipe에 결합한다. vector ID는 기존 artifact/manifest의 bounded 논리 ID이며 Lance row offset이 아니다. 기존 `MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES` 한도를 재사용하고 variable-length ID의 decode/copy/result memory까지 grant sizing에 포함한다. row/schema/row digest/seal/candidate/tie/중복 검사·반환 경로에 같은 ID를 사용한다. chunked 입력은 selected artifact와 chunk 위치의 일대일 대응을, whole-file 입력은 기존 canonical `b"whole-file"` ID와 artifact 대응을 검증한다. 재배치·압축 후에도 ID는 유지한다.
2. exact vector 원본은 기존 immutable owner artifacts를 재사용한다. 검색 레이아웃은 bounded sealed exact delta/tombstones, immutable bounded ANN regions, persistent directory와 visibility-aware routing이다. owner마다 Lance 테이블을 열어 N개를 검색하는 방식이나 mutable global head를 도입하지 않는다.
3. Core/Kernel 경계는 vendor 중립인 admitted immutable file-set 설치·열기·폐기와 attempt/read-ticket/pin/GC 수명만 전달한다. Lance table/session/index 타입은 Semantic/물리 adapter 안에 둔다. Runtime의 기존 Common publication capacity/binder·FileAdmission driver를 확장해 사용하며 두 번째 resource ledger·reaper·storage stack을 만들지 않는다. 새 port는 실제 install/open/close/reclaim caller까지 같은 변경에 결합한다.
4. 준비된 directory는 정확한 owner-map stamp·recipe·sealed delta/region/file refs를 결합한다. 기존 owner completion/publication CAS의 단일 게시 지점에서 관련 owner/입력을 다시 검증하고 directory와 coverage를 원자 채택한다. Semantic·Runtime·overlay·SDK의 `searchable==owners` 가정과 child 검증도 함께 수정한다. reference 필드만 바꾼 상태를 완료로 보지 않는다.
5. 무관한 B 갱신 때문에 stable A의 준비 결과가 계속 stale이 돼서는 안 된다. 관련 owner/region 단위의 검증·안전한 재사용/rebase와 기존 owned-work 유지보수로 진행성을 보장한다. stale 전체 root를 deadline까지 반복 준비하는 것, 새 일반 scheduler/compactor 선제 구현은 해결책이 아니다.
6. 물리 파일은 attempt-bound 생성/설치·실패 정리와 selected-ticket 열기·checked close·last-pin reclaim에 결합한다. admission은 create/grow/open보다 먼저다. 불확실한 close/unlink는 custody/charge를 유지한다. Scratch는 disposable 준비 공간으로만 쓰며 legacy `PublicationAllowance`를 Native authority로 재해석하지 않는다. Catalog가 visibility의 단일 권한이며 Lance 내부 latest/version은 이를 대체하지 않는다.

## Query and profile boundaries

- 작은 exact region과 bounded exact delta는 명시된 합법적 profile이다. 현재 `vector_index.rs`의 256행 floor는 프로젝트 정책이며 Lance 라이브러리 최소치가 아니다. 행·dimension·cell cap의 교차 조건을 검사한다(현재 1,048,576 cells에서 dimension 4096은 최대 256행). 이 결과는 Large ANN qualification이 아니다.
- ANN 모드는 distance metric·index recipe·query effort·full-vector refine와 indexed coverage를 seal에 결합한다. 최신 웹 문서의 API/기본값을 pinned LanceDB 0.30.0/Lance 7.0.0의 지원 기능으로 간주하지 않는다. upgrade·RaBitQ·GPU 도입을 선행 작업에 넣지 않는다.
- hidden unindexed-tail brute-force를 허용하지 않는다. indexed coverage와 sealed exact delta를 합쳐 current membership을 보장한다. `fast_search`로 tail을 빼서 누락을 숨기거나 자동 fallback을 켜서 비용을 숨기지 않는다. pinned 실행 계획/카운터로 실제 경로를 확인한다.
- 현재 `query_region`의 1회 bounded overfetch만으로 filtered top-k나 전역 recall을 보장할 수 없다. routing fanout·region probes·누적 candidates·distance/refill budget을 고정하고 selected owner/input/vector visibility를 확인한다. 필터/삭제에 따른 shortfall은 bounded refill 후에도 `approximate/capped/incomplete`를 정직하게 노출한다. 정확한 거리 재계산은 후보 집합의 exact top-k 증명이 아니다. source/owner coverage의 Complete와 ANN 후보 탐색의 approximate/exhaustive 상태도 별개다. current membership 인증은 탐색 누락이 없다는 증거가 아니다.
- Large profile은 반드시 구체적인 routing/refill과 유지보수를 함께 수용한다. hash region 전수 검색·row count 전수 확인·무제한 exact fallback은 Large 비용 계약을 충족하지 못한다. 작은 profile 기능은 진행하되 Large gate를 지우거나 fallback 성공으로 대신하지 않는다.

## Implementation order and exclusive writes

1. **즉시 가능한 SEARCH private 작업:** vector identity 결함, row/seal/decode 검증과 grant sizing을 고치고 테스트 전용 mount로 기존 private module을 compile한다. multi-vector·삭제/필터·독립 exact gold를 검사한다. 이 mount는 production 등록/물리 qualification이 아니다. 별도 producer나 ANN 엔진을 만들지 않는다.
2. **병렬 계약 결합:** 단일 공용 writer는 Core/Kernel publication/coverage와 모든 reader 가정을, 기존 PHYSICAL writer는 Native 파일 capability의 실제 Common/Lance adapter를 소유한다. SEARCH는 이 계약으로 directory·routing·refill을 개발한다. parent/export/Cargo/Runtime 조합은 통합자 한 명만 수정한다. private exact/quality 검사는 외부 producer나 파일 port 완료를 기다리지 않는다.
3. **합류:** 파일 capability→sealed directory→owner completion CAS→Semantic reader→Runtime/SDK를 한 source에서 결합한다. staged file의 실패 정리·child refs·old-pin/재오픈·cancel/drop 경로를 정적으로 대조한 뒤 영향 테스트/strict를 모아 실행한다. W2-07의 물리 oracle와 W2-12의 논리 membership gold를 재사용한다.
4. **선택/수용:** W2-09의 기존 harness/profile에서 현 Lance를 먼저 검증한다. 계약 불일치 또는 목표 초과가 확인되면 같은 port 뒤 호환 대안 하나만 비교한다. custom HNSW, 모든 논문 구현, 라이브러리 교체 자체를 합격 조건으로 삼지 않는다. profile을 만족하지 못하면 Large gate를 닫아두고 원인·대안을 기록한다.

## Acceptance and stop conditions

- **정확성:** 독립 고정 vectors/exact gold에서 owner당 여러 vector·chunk bijection·ID 중복·ABA·same-ID replacement·recipe/model 변경·old view/delete/reopen을 검사한다. selected corpus 밖 행은 0개여야 한다. 이 correctness 조건은 recall 허용오차와 별개다.
- **수명/원자성:** install 전후·CAS 전후 crash/cancel, long pin, ENOSPC/short-write/fsync/close에서 staged/current/old file census와 custody를 확인한다. W2-07의 기존 fault owner가 담당하며 ANN integration은 동일 원인을 실제 caller에 주입한다. 합성 EIO와 실제 OS fault를 구분한다.
- **품질/비용:** RFC §12.3의 제안 gate인 Large recall@10 >= 0.95, cached-input adoption p95 <= 500ms/p99 <= 2s를 해당 qualification profile에서 검사한다. Large query latency SLO·hardware·vectors/query/filter/update trace·duration·candidate/refill/fanout/memory/disk cap은 실행 전에 기존 W2-09 profile에 동결한다. 미정 SLO는 기능 시험을 막지 않지만 제품/비교 qualification 통과를 주장할 수 없다.
- **스케일/지속성:** 고정 D/K에서 N/H/M과 region 수를 독립 증가한다. 필터 selectivity/embedding correlation·distribution shift·집중 삭제·provider 유무·hot read/write·장기 pin·유지보수를 포함해 recall, p95/p99, refused/timeout, IO/CPU, scratch/retained bytes와 debt/drain을 보고한다. 동일 region의 stable A와 연속 B 갱신에서 A adoption과 drain의 진행성을 증명한다.
- private compile/exact, Native physical integration, 실제 external producer, Large qualification은 별도 verdict다. 결과·source·명령은 VALIDATION에 한 번 기록하고 W2-19가 소비한다. 성공 표본만 골라 recall/latency를 보고하지 않는다.

## Primary references and limits

2026-10-15 확인. 논문 수치나 현재 upstream 기능을 pinned 구현의 성능/지원 증거로 승격하지 않는다.

| 1차 자료 | 채택할 근거 | 적용 한계 |
| --- | --- | --- |
| [LanceDB vector search](https://docs.lancedb.com/search/vector-search), [pinned query.rs](/Users/songmin/Documents/code-new/quanta-index/vendor/lancedb-0.30.0/src/query.rs) | unindexed tail, pre/postfilter, metric/refine/query effort를 명시적으로 통제 | 웹 문서는 변하므로 0.30.0 소스와 실제 실행 계획을 확인. refine의 정확한 거리가 exact 후보 집합을 뜻하지 않음 |
| [SPFresh, SOSP 2023](https://www.microsoft.com/en-us/research/publication/spfresh-incremental-in-place-update-for-billion-scale-vector-search/) | local partition repair·분포 변화 실험 | in-place 갱신은 우리 retained immutable view를 보장하지 않음 |
| [ACORN, SIGMOD 2024](https://arxiv.org/abs/2403.04871) | embedding과 필터 조건을 함께 바꾼 recall/cost 평가 | filtered search 결과가 update·crash·GC 증거는 아님 |
| [IP-DiskANN, 2025](https://arxiv.org/abs/2502.13826), [공식 DiskANN](https://github.com/microsoft/DiskANN) | 긴 insert/delete stream의 recall·유지보수 비교 후보 | 구현 채택 또는 우리 snapshot/file 권한 호환을 전제하지 않음. backend 교체 결정은 같은 입력·포트의 gate 뒤 |

## Source entry points

- [ann_lance.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-semantic/src/native/ann_lance.rs), [embedding.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-semantic/src/native/embedding.rs)
- [vector_index.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-semantic/src/vector_index.rs), [qualified_fs.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-semantic/src/qualified_fs.rs)
- [Kernel update.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-kernel/src/update.rs), [native_semantic_result.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_semantic_result.rs), [native_uds.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/native_uds.rs)

공용 parent/export/contract/config/registry는 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 문서의 예정 타입/동작은 구현 완료가 아니다. 실행 결과는 VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE로 구분한다.
