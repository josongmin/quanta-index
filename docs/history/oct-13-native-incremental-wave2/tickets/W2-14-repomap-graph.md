# W2-14 — 동일 Catalog view의 provenance graph와 전체 랭킹

> HISTORICAL REFERENCE — 2026-10-18 retirement. Current work/status: [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md). Old TODOs, owners, commands and counts below do not authorize new work or qualify current source.


> `HISTORICAL_DESIGN_REFERENCE`. 현재 잔여/상태는 [Oct-17 SSOT](../../../plans/oct-17-index-closeout/tickets/README.md#w2-14)만 따른다. 아래 과거 TODO·진척·실행 순서는 신규 작업을 만들지 않는다. 이 파일에는 상태/담당을 갱신하지 않는다.

기존 요구/큐: R24 / NI-06. 요구는 [RFC §9.3](../../../rfc/oct-10-native-incremental-vnext.md#93-repomap-and-future-adapters), 결과는 [VALIDATION](../../../plans/oct-10-index-closeout/VALIDATION.md)이 소유한다.

## Reuse

- [x] `DONE_IMPL`: graph contribution/support/retraction, Kernel group root, pinned adjacency, SDK/daemon의 양방향 adjacency, 요청 anchor 검증과 데이터/grant 수명. adjacency 검증은 전체 랭킹의 완료 증거가 아니다.
- [x] `DONE_IMPL`: 기존 snapshot query에 연결된 `query::ranking`의 bounded selector. Native scanner도 이 선택기를 사용한다. 두 번째 ranker/materializer/producer를 만들지 않는다.

## Final scope and contract

**필수 parity는 후보 집합·focus 거절·folded substring·점수/동점 순서·token budget 확장/floor·drop/degraded 이유다.** RFC가 legacy `RepoMapQueryResponse`의 모든 전역 문자열 필드까지 같은 wire로 요구한다고 해석하지 않는다.

- Native full-rank API는 `ViewSelection`/instance와 query text/focus/top-k/token budget을 받아, 선택된 `ViewRef`/instance·정책 결합과 ranked rows/drop/degraded 결과를 반환하는 typed 경로로 정한다. 이는 adjacency와 별개 연산이며 `NativeGraphResult`만으로 전체 query를 완료 처리하지 않는다. 행에는 typed subject·owner path·점수·rank 등 실제 제공 가능한 계약값을 노출한다. 없는 repo/revision/generation/snapshot metadata를 만들어 넣지 않는다. 현재 graph 전수 순회가 끝났다는 사실과 producer frontier의 완결성을 구분하고, 기존 group/certificate coverage가 증명한 범위에서만 complete를 반환한다.
- 기존 legacy 응답을 그대로 반환하는 bridge는 해당 endpoint가 그 wire를 약속할 때만 범위에 포함한다. 그때 필요한 전역 metadata는 선택된 projection의 인증 header 한 곳에 둔다. 이 header를 Native 랭킹 자체의 필수 선행조건으로 삼거나 owner마다 복제하지 않는다.
- 조회 입력은 기존 `CompiledRepoMapProjectionEntryV1`의 subject/doc_type/owner_path/search_text/final_score 계약과 그 closed manual codec을 재사용한다. 큰 검색문은 projection leaf에만 보유한다. 같은 의미의 `NativeRankRowV2`나 내부 IR twin을 추가하지 않는다. Native profile admission은 검색문 1MiB·path 4096 bytes·subject 2048 bytes이며 framed map key의 기존 한도도 적용한다. 이 admission 바깥의 legacy 입력까지 호환된다는 주장은 하지 않는다.
- 점수·토큰 상수는 인증된 원본 사실과 구분한다. token hint 64, score 변환·tie·floor/drop·Unicode fold 버전을 포함한 단일 인식 가능한 rank-policy descriptor/digest를 정의하고 기존 `InstanceSpec.policy`/format에 결합한다. profile·build·query 모두 그 정책을 검증한다. `ProjectionBinding.producer_policy`와는 동등성 강제가 아니라 명시적 호환 규칙을 검사한다. 임의 digest를 응답에 싣는 것만으로 정책이 증명되지는 않는다. 최초 Native profile의 허용 쌍은 producer와 instance 모두 이 인식된 rank descriptor digest를 인증한 경우 하나이며, 다른 producer 정책은 명시적 호환 매핑을 결합하기 전 거절한다. 실제 producer가 이 profile을 공급한다는 증거와 query의 정책 검사는 별도다. producer에 새 query 알고리즘을 구현시키지 않는다. 이미 계산된 projection을 인계받는 경우 producer certificate가 그 값과 입력/정책을 결합해야 한다. 현 compiled profile의 점수 범위 0..1000 위반·subject/type 불일치는 거절하며 clamp나 추정값으로 보정하지 않는다. 실제 symbol kind 등 새 필드는 API가 약속할 때만 요구한다.

## Representation and bounded execution

- 기존 `GraphOwnerOutput`에 subject순 정렬·중복 금지 projection leaf 참조를 결합한다. wire의 projection 필드는 필수이며 adjacency-only output은 명시적인 빈 배열을 사용한다. 누락 필드를 기본값으로 채우는 reader나 구형/신형 IR을 병행하지 않는다. 공개 `GraphNode`에 큰 검색문을 넣어 adjacency 응답까지 비대하게 만들지 않는다. nodes/edges/projection은 하나의 인증 output이며 별도 전체 bundle을 재구성하지 않는다.
- projection leaf는 위 canonical compiled entry codec으로 저장하고 output의 `(subject, row_ref)`가 그 내용·정책과 일치해야 한다. output의 child set은 이 leaf refs와 정확히 일치한다. 현재 `load_output`과 Kernel `stage_projection_output`/`stage_projection_owner`의 leaf-only 검증을 단일 writer가 같이 바꾼다. 기존 object DAG 등록/무결성/GC를 재사용하고 새 direct-child 수·폭만 제한하며, query/open 때 전체 inherited DAG를 다시 순회하지 않는다. root의 기존 4MiB 한도를 키워 전체 텍스트를 넣는 방식으로 해결하지 않는다.
- Native transport는 `QueryRepoMap`(request tag 37)과 `RepoMapRanked`(response tag 30)를 사용한다. `StageProjectionOutput`의 `children` 필드는 필수이며 leaf는 명시적인 빈 배열이다. children은 decode 중 최대 4096개·strict ascending·unique를 강제한다. 구형 `{ingress,bytes}` 메시지를 기본값으로 수용하지 않는 breaking-first 배포 계약이다. Kernel은 ingress/Catalog child 존재·등록·수명 검증을 재사용하며 child payload 전체를 staging마다 재독하지 않는다. 실제 output refs와 leaf 내용은 adapter가 검증한다.
- 승인 뒤 adapter가 검출한 `CorruptObject`는 기존 Kernel의 `Unknown`/reconcile 계약을 따른다. 이를 terminal refusal로 바꾸지 않는다. malformed-child 회귀는 내부 typed cause와 같은 operation key의 `Unknown`·Pending 보존·게시 view 불변을 함께 검사한다. query의 응답 전 typed refusal과 publication의 승인 후 결과를 혼동하지 않는다.
- Runtime의 canonical borrowed wire는 기존 페이지의 hit/reason과 grant를 직렬화 완료까지 보유한다. 새 랭킹 목록을 복사하거나 lease 없는 DTO를 내보내지 않는다. 미해결 focus는 Core와 Native wire의 `FocusSubjectNotFound` 전용 코드이며 메시지 문자열 파싱으로 분류하지 않는다. 고정 정책의 drop/degraded reason은 허용 목록·strict ascending·unique로 닫는다.
- 기존 node-support tree의 작은 support에 row ref를 결합하고 정확한 child ref로 leaf를 보유한다. compiler의 현 row preview cap 1MiB·materialized cap 64MiB와 graph support metadata 16KiB·output root 4MiB를 서로 혼동하지 않는다. 프로필의 실제 행 수·행/총 bytes·node/edge cap 및 decode scratch·retained result를 birth 전에 검사한다. 큰 payload의 split/reference ingest와 실패 시 data-before-grant 해제까지 계약 동결 단계에서 결합하며, legacy보다 작은 admission을 몰래 적용해 parity를 주장하지 않는다. 반환 page는 hits/reason을 읽기 전용으로 빌려주어 데이터만 lease에서 분리해 이동하지 못하게 한다. hit 복사가 끝나면 큰 preview/folded 후보는 해제하고, 복사된 결과와 diagnostic의 grant만 응답 소비까지 보유한다.
- 같은 typed subject의 surviving supporters는 digest뿐 아니라 모든 projection 사실이 일치해야 한다. 한 supporter 삭제 후 행이 유지되고 마지막 삭제 후 제거돼야 한다. 전역 degree/score/text 의존성이 바뀌면 producer가 실제 affected closure를 인증한다. 전역 통계/정책 변경의 비국소 비용을 local delta로 숨기지 않는다.
- 최초 Native scanner는 같은 pinned node root를 page 단위로 순회하고, supporter를 identity별로 검증·그룹화한 뒤 공용 selector에 보낸다. File/Symbol 행 누락·충돌은 응답 전 typed refusal이다. Module/Chunk를 임의 File/Symbol로 승격하지 않는다. cross-variant raw-ID 충돌도 canonical compiler와 같은 전역 거절 계약을 유지한다. 같은 raw ID의 나머지 typed variants를 최종 node root의 bounded prefix probe로 확인해 새 O(N) 메모리 set 없이 검사한다. bootstrap과 변경 subject에 적용하고 retained predecessor의 정책/검증 경계를 확인한다. query의 malformed-view 검사도 동일한 root를 사용하며 이 추가 probe 비용을 계수한다. supporter 중복을 제거한 distinct subject당 한 후보만 내보내고, encoded map-key 순서를 의미상 동점 규칙으로 사용하지 않는다. selector의 raw identity comparator와 같은 pinned scan의 안정적인 ordinal을 독립 compiler/reference gold로 대조한다.
- focus는 모든 `(identity, doc_type)`를 먼저 해석하고 funded owner paths를 보유한다. 하나라도 없으면 `FocusSubjectNotFound`; global fallback은 없다. prefix를 넓힐 때 같은 root·안정적인 ordinal·같은 누적 budget을 사용한다. 작업/메모리 한도 소진은 `ResourceLimited`이며 exhaustive 빈 결과가 아니다.
- 범위가 제한된 complete scan은 최초 기능 경로로 허용한다. 전수 scan의 CPU/I/O는 N과 함께 증가하며 추가 prefix probes·heap 작업·재순회도 별도 계수한다. 이를 O(N) 상한이나 sparse/sublinear query로 부르지 않는다. 영속 subject/owner-path 인덱스·unranked counter·n-gram 가속은 최초 정확성의 필수조건이 아니다. W2-09에서 focus/전체 조회/예산 확장 비용이 목표를 넘는다고 입증되면 필요한 인덱스만 추가한다. 가속 후에도 substring exact verification·fanout 예산을 유지한다.

## Implementation order and exclusive writes

1. **공용 계약, 단일 writer:** Contract `native_protocol/graph.rs`, 기존 compiled projection codec, request/response tags·recognized policy, Kernel의 projection output staging/child-set 호출부를 함께 동결한다. 누락 행·범위·raw-ID 충돌 거절과 Native API 반환 필드를 먼저 고정한다. 실제 producer의 개발 완료를 codec·fixture 검증의 선행조건으로 삼지 않는다.
2. **계약 뒤 병렬:** SEARCH graph writer는 `contribution.rs`/`affected.rs`의 sparse row/child 수명을, 별도 SEARCH query writer는 private rank scanner와 shared selector adapter를 소유한다. `graph.rs` root/schema/parent 수정은 통합자에게만 둔다. 공용 부모는 통합자만 수정한다. 기존 SDK/INT producer writer는 같은 계약으로 인증 row와 affected closure를 공급한다. 같은 producer를 새로 만들지 않는다.
3. **통합:** 한 writer가 typed Kernel registry→Runtime→SDK 경로를 결합한다. 정적 call graph·codec·오류/drop 경로를 대조한 뒤 owner tests를 모아 실행한다. 원격 producer 미연결은 실제 E2E 수용만 막으며 local selected-view fixture를 막지 않는다.
4. **수용:** producer 연결 후 FULL/delta/delete/query/reopen parity와 비용을 W2-09/19가 같은 결과 원장에서 소비한다. legacy 전역 응답 bridge가 실제 요구되면 이 단계의 명시적 추가 계약으로 다룬다.

## Acceptance

- 독립 full-sort/reference와 동일한 후보·순서·rank·drop/degraded 결과. infix, Unicode normalization/case fold, CJK, 짧은 문자열, focus 미해결, 큰 owner fanout, token skip/prefix 확장/floor를 포함한다.
- 고정 producer projection으로 full-vs-delta support 2→1→0→1, replace/delete/rename/recreate/no-op, supporter 불일치·행 누락·잘못된 type/점수, old view·pin·재오픈을 검사한다. header-only 변경을 선택한 bridge는 owner row 재작성 없이 갱신돼야 한다.
- row decode/grant 거절 전 birth/scan0, scan 오류·취소·부분 이동·응답 저장소 실패의 data-before-grant 해제, page nonprogress/중복/순서 위반 거절을 검사한다. 양방향 adjacency의 원 anchor/fanout 거절은 기존 oracle를 재사용한다.
- Native Runtime/SDK의 view·instance·policy·요청 결합과 strict focus를 실제 UDS에서 검증한다. 단위 fixture 통과, owner-local UDS, 실제 외부 certified producer E2E를 별도 결과로 기록한다. endpoint 동결 시 Native full-rank semantic parity와 기존 공개 `RepoMapQueryResponse` wire 호환성의 약속 여부를 각각 적고 검증한다. adjacency나 Native subset 결과를 기존 SDK endpoint의 호환 완료로 보고하지 않는다. legacy bridge를 약속하지 않은 경우 그 호환 범위는 NOT_APPLICABLE이며 R24의 후보/랭킹 의미 요구는 그대로 남는다.
- W2-09는 broad/focus/prefix 확장의 실제 reads/bytes/CPU/peak와 고정 D/K에서 N/H/M 증가를 측정한다. 전수 scan 비용 수용과 sparse update 수용을 혼동하지 않는다. 수치 목표는 실행 전에 기존 profile에 동결한다.

## Primary references and limits

2026-10-15 확인. 아래는 설계 근거이며 이 저장소의 성능 수용 결과가 아니다.

- [SQLite FTS5 trigram](https://www.sqlite.org/fts5.html#the_trigram_tokenizer): substring과 token 검색의 차이, 짧은 문자열/Unicode 조건의 한계를 확인하는 구현 참고다. SQLite/FTS 도입 자체를 요구하지 않으며 현재 LQ fold 의미를 독립 gold로 보존한다.
- [RocksDB MANIFEST](https://github.com/facebook/rocksdb/wiki/MANIFEST): immutable 파일 집합의 논리 게시/복구를 참고한다. 별도 manifest/CURRENT·새 Catalog 권한을 만들지 않고 기존 Catalog·object child·pin/GC를 재사용한다.

## Source entry points

- [graph.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-repomap/src/native/graph.rs), [affected.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-repomap/src/native/graph/affected.rs)
- [ranking.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-repomap/src/query/ranking.rs), [compile.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/repomap/compile.rs)
- [materializer.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-repomap/src/materializer.rs), [native_protocol/graph.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-contract/src/native_protocol/graph.rs)
- [native_repomap_transport.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/src/native_repomap_transport.rs), [native_uds/repomap.rs](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-sdk/src/native_uds/repomap.rs), [Kernel staging](/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-kernel/src/update.rs)

공용 parent/export/contract/config/registry는 [단일 writer 규칙](../../../plans/oct-13-native-incremental-wave2/tickets/WRITE-OWNERSHIP.md)을 따른다. 문서의 예정 타입/동작은 구현 완료가 아니다. 실행 결과는 VERIFIED/FAILED/BLOCKED/NOT_RUN/NOT_APPLICABLE로 구분한다.
