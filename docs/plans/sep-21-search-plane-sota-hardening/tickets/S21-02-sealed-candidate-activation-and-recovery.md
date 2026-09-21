# S21-02 — Sealed Candidate, Activation Transaction, and Recovery

Status: `planned`

Depends on: S21-00, S21-01 Phase A (P01A checkpoint), S21-03, S21-04; closes atomically with S21-01 Phase B in P03

## Goal

RepoMap의 publish/activate/recover를 content-bound durable state machine으로 교체한다. 동일 generation
overwrite, digest-unbound activation, stale pointer resurrection을 한 owner에서 제거한다.

## Root cause

- snapshot key가 repo/revision/generation뿐이라 content identity가 없음
- materialized snapshot이 producer manifest commitment를 잃음
- activation record가 generation만 저장하고 self-integrity가 없음
- open이 candidate/pointer를 독립 load한 뒤 memory map으로 authority를 재구성

## Target state machine

```text
Absent -> Preparing -> SealedCandidate -> Activated -> Retiring -> Retired
                    \-> Quarantined
Activated --candidate loss/corruption--> ActivationInvalidated
ActivationInvalidated --publish only--> SealedCandidate (never Activated)
```

모든 transition은 expected prior state/epoch/content commitment를 확인한다.

## Target records

- versioned/self-digested candidate record
- immutable snapshot content digest and typed graph commitment
- versioned/self-digested activation record containing exact candidate identity
- monotonic activation epoch and expected-active CAS
- terminal mutation receipt containing prior/new activation identity and durable sequence
- durable invalidation/tombstone reason

## Work items

1. materializer output에 manifest/authority/content commitments를 보존
2. same logical generation publish:
   - exact same commitment: stored receipt replay
   - different commitment: `CANDIDATE_COMMITMENT_CONFLICT`
3. activation은 candidate exact commitment와 request를 compare
4. durable activation commit 후에만 memory snapshot registry publish
5. candidate corruption/missing 발견 시 activation을 durable invalidation
6. open-time reconciler가 catalog candidate/activation/invalidation과 immutable object 조합을 state table에 따라 판정
7. retirement/GC는 pinned handle과 rollback reference를 확인
8. identity/content-bound SDK ACK 도입
9. candidate/activate/rollback/invalidate/quarantine-record/quarantine-discard는 P02B의 generic global event
   allocator를 사용하고 allocator/event/domain row를 한 transaction으로 commit
10. quarantine는 exact catalog envelope commit 후 payload+incident create-new/fsync, 그 뒤 source unlink+directory
    fsync 순서로 투영하며 crash retry는 original time/sequence/envelope를 재사용

## Crash/fault matrix

- candidate temp write 전/후, fsync 전/후, rename 전/후
- catalog activation commit 전/후, memory publish 전/후
- quarantine catalog commit, payload projection, incident projection, source unlink와 directory fsync 경계
- retirement file delete와 reference update 사이
- corruption -> restart -> repair publish -> restart
- same generation same/different content retry
- activation ACK loss와 exact replay

## Owner files

- `crates/quanta-index-repomap/src/{materializer,model,persistence,store}.rs`
- `crates/quanta-index-core/src/domains/repomap/`
- `crates/quanta-index-contract/src/repomap.rs`
- `crates/quanta-index-sdk/src/repomap.rs`
- runtime RepoMap composition/control route

## Acceptance

- publish alone never changes active query truth
- repair/import alone never restores active truth
- every active pointer names one exact verified candidate commitment
- same generation different content never overwrites memory or disk
- crash converges to old committed activation or new committed activation, never mixed/ambient state
- receipt replay does not reapply storage

## Verification

- RepoMap owner-local store/persistence tests
- real filesystem fault tests
- `repo_map_end_to_end`, restart, activation concurrency, boot quarantine scenarios
- `just rust-public-api`, `just rust-wire-inventory`, `just rust-fuzz-smoke`
- `just rust-profile test-daemon`

## No patch-on-patch rule

manifest digest compare만 추가하고 activation record/ACK/recovery를 그대로 두는 수정은 불가하다.
candidate commitment, pointer envelope, state machine, recovery oracle을 한 merge unit에서 전환한다.

## Final authority boundary

- SQLite catalog의 `repomap_candidate_v1`과 `repomap_activation_v1`만 candidate/activation visibility를
  판정한다. `persistence.rs`는 immutable candidate object bytes만 저장한다.
- runtime의 `activations/` file read/write와 boot-time file scan authority를 삭제한다. 두 durable authority를
  reconcile하는 코드는 만들지 않는다. P03는 V1 root를 mutation 전에 refuse하며 legacy directory의
  bytes/inode/mtime를 바꾸지 않는다. legacy parsing/transformation/deletion은 P10 offline importer만 소유한다.
- `repomap_candidate_v1`은 logical key `UNIQUE`, candidate commitment, object address, schema/profile,
  terminal sequence, row digest를 가진다.
- `repomap_activation_v1`은 exact candidate commitment, monotonic activation epoch, expected-active CAS,
  invalidation/tombstone, row digest를 가진다.
- catalog transaction commit 뒤에만 in-memory registry를 publish한다. registry는 cache이며 재시작
  authority가 아니다.

### File-level action list

| File / symbol | Logic | Completion proof |
|---|---|---|
| `crates/quanta-index-repomap/src/materializer.rs` | infallible `materialize`를 제거하고 S21-03의 `CompiledRepoMapCandidateV1`만 입력받아 seal | malformed bundle이 이 owner에 도달하지 않음 |
| `crates/quanta-index-repomap/src/persistence.rs` | immutable object write/fsync/verify만 유지; activation path 제거 | runtime activation file read/write 0 |
| `crates/quanta-index-repomap/src/store.rs` | catalog CAS → object verify → registry publish; acquire는 active identity와 `Arc`를 원자적으로 반환 | crash/restart에서 mixed authority 0 |
| `crates/quanta-index-searchd/src/app/runtime.rs:1023-1078` | catalog-backed ports만 composition; file pointer authority 제거 | boot/open route가 동일 ledger 사용 |
| `crates/quanta-index-search-plane/src/control_dispatcher.rs` | publish/activate/rollback ACK에 prior/new commitment, epoch, terminal sequence 결속 | ACK loss exact replay |
| `crates/quanta-index-search-plane/src/ingest_dispatcher/` | publish는 S21-04 journal을 거쳐 candidate seal만 수행 | publish-only active truth 변화 0 |
| `crates/quanta-index-contract/src/repomap.rs` 및 SDK | mandatory content-bound request/ACK와 typed conflict/invalidation | wrong commitment SDK rejection |
| catalog quarantine + `persistence.rs` | catalog exact envelope → immutable payload/incident projection → source unlink/fsync | crash retry가 새 sequence/time을 생성하지 않음 |

### DoD additions

- S21-03 compiler refusal이면 object/catalog/registry mutation이 모두 0이다.
- same logical generation/different commitment는 `CANDIDATE_COMMITMENT_CONFLICT`; same commitment는 원 receipt replay다.
- candidate object loss/corruption은 durable `ActivationInvalidated`가 되고 파일 재등장만으로 재활성화되지 않는다.
- fault matrix 각 지점에서 restart 후 old 또는 new committed activation 하나만 관찰된다.
- projection failure는 open success로 삼키지 않으며 durable invalidation 이후 자동 serve/activate가 0이다.
- runtime activation file read/write/path symbol은 0이고 P03가 legacy bytes를 수정한 횟수도 0이다.
