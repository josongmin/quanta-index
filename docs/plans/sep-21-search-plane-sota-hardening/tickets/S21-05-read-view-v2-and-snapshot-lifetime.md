# S21-05 — QueryReadView V2 and Snapshot Lifetime

Status: `planned`

Depends on: S21-00, S21-02, S21-04

## Goal

모든 query route가 선언한 domain의 실제 immutable handle을 한 번 acquire하고 요청 종료까지 보유하게
한다. ambient store/ledger 재조회와 activation/GC TOCTOU를 제거한다.

## Root cause

현재 `QueryReadViewV1`은 lexical/semantic/auxiliary handle은 보유하지만 RepoMap handle이 없다.
RepoMap route는 domain을 선언한 뒤 독립 port call을 하며, store는 activation check와 snapshot lookup
사이 lock을 놓는다. read identity와 실제 resource lifetime이 분리돼 있다.

## Target design

- `QueryReadViewV2`는 declared domain마다 typed `PinnedDomainHandle`을 보유
- handle은 logical pin, physical artifact identity, activation epoch, auxiliary epoch, capability/profile을
  함께 기록
- acquisition critical section에서 active/retiring/quarantine 상태를 판정하고 reference 획득
- route는 view accessor로만 backend/searcher/snapshot에 접근
- GC/retire/compaction은 handle reference와 attach fence를 존중
- view drop이 reference를 반환하며 panic path도 RAII로 동일 처리

## Work items

1. `RepoMapSnapshotReadPort`를 snapshot acquisition과 execution으로 분리
2. RepoMap `Arc<RepoMapIndexedSnapshot>`을 view에 저장
3. lexical/semantic/auxiliary도 identity descriptor shape를 통일
4. undeclared-domain accessor는 typed refusal
5. route code에서 ambient registry/store lookup 제거
6. view identity를 response/cursor provenance로 전달
7. pin/retire/attach lock order와 deadlock rule 문서화
8. physical remap/compaction이 logical pin 아래 다른 artifact로 바뀌지 않도록 epoch 검증

## Concurrency scenarios

- query N active-check barrier 중 N+1 activate/retire
- old view 보유 중 GC/compaction/quarantine
- retire가 먼저 이긴 뒤 새 old-generation pin 요청
- query panic/cancel 중 reference release
- repo A churn 중 repo B view acquisition
- auxiliary epoch update와 composite query

## Owner files

- `crates/quanta-index-search-plane/src/query_dispatcher/read_view/`
- `crates/quanta-index-search-plane/src/query_dispatcher/routes/`
- `crates/quanta-index-repomap/src/{reader,store}.rs`
- snapshot registries and retention owners
- `crates/quanta-index-core/src/domains/*` read ports

## Acceptance

- declared domain마다 정확히 하나의 `DomainReadEvidenceV2`가 존재하고 evidence 없는 domain/미선언 handle이 없음
- physical handle은 domain 수가 아니라 실제 resource group당 하나이며 공유 관계가 evidence에 명시됨
- route가 view acquisition 후 ledger/store의 ambient latest를 읽지 않음
- activation/GC 중 query는 old pinned result를 끝까지 반환하거나 acquisition에서 typed refusal
- mixed physical identity/aux epoch response 0
- view release 후에만 retired artifact가 삭제 가능

## Verification

- core read-view declaration/accessor tests
- deterministic barrier-based concurrency tests
- daemon activation concurrency, physical GC, snapshot registry, read-view E2E
- loom 또는 equivalent가 적용 가능한 lock/state core에는 model test 검토
- `just rust-profile test-daemon`

## No patch-on-patch rule

RepoMap store 내부에서 lock 두 개를 동시에 잡는 국소 수정으로 끝내지 않는다. route-visible handle을
view에 올리고 GC/reference lifetime까지 같은 contract로 전환한다.

## Final file/symbol plan

| File / symbol | Change | DoD |
|---|---|---|
| `crates/quanta-index-core/src/domains/repomap/inbound.rs::RepoMapQueryPort` | ambient `query`를 `acquire` port와 pinned snapshot query interface로 분리 | core trait에 adapter concrete type 없음 |
| `query_dispatcher/read_view/view.rs::{LedgerParts,QueryReadViewV1,acquire_read_view}` | RepoMap pinned handle와 `DomainReadEvidenceV2` 추가; V2로 version bump | 선언 domain별 evidence exact one |
| `query_dispatcher/read_view/identity.rs` | pin, candidate commitment, activation epoch, artifact identity, aux epoch의 domain evidence map | response/cursor가 실제 handle identity를 운반 |
| `query_dispatcher/routes/repo_map.rs:28-39` | `self.repo_map_query.query(request)` 제거; `view.repo_map()?.query(...)`만 허용 | acquisition 뒤 ambient lookup 0 |
| `crates/quanta-index-repomap/src/store.rs` | active catalog identity와 `Arc<RepoMapIndexedSnapshot>`을 한 critical section에서 획득 | activate/retire TOCTOU 0 |
| `crates/quanta-index-repomap/src/reader.rs` | pinned snapshot executor 구현; store 재조회 금지 | query lifetime 동안 동일 artifact |

### Proof additions

- barrier test: old view acquire 뒤 activate/retire/GC를 진행해도 old view는 동일 commitment로 완료한다.
- retire가 먼저 이기면 새 old-generation acquire는 typed refusal이며 deleted artifact open을 시도하지 않는다.
- cancel과 panic unwind 뒤 pin/reference count가 baseline으로 돌아오고 GC가 진행된다.
- shared physical handle을 여러 declared domain이 참조하는 fixture에서 handle count가 아니라 evidence cardinality를 검증한다.
