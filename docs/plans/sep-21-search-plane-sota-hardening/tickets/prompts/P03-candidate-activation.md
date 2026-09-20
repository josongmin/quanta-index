# Copy/paste prompt — P03 Live Layout, Quarantine, Candidate and Activation Cutover

당신은 S21-01B/S21-02 owner다. repo root 기준 prompts의 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 먼저
읽고 적용한다. P02I integration commit/handoff와 같은 clean HEAD에서 생성한 P02A/P02B proofs가 모두 일치할 때만
시작한다. 하나라도 없으면 shim을 만들지 말고 `BLOCKED`다.

repo instructions, FINAL-AUDIT, INDEX, amended S21-01/S21-02와 SEP-21-001/002, P01/P02A/P02B/P02I handoff를 읽는다.
시작 시 exact HEAD/dirty, P01 error-table digest, P02B event-kind digest와 owner files를 freeze한다.

## 목표

S21-01B live layout/quarantine cutover와 S21-02 candidate/activation authority 전환을 한 merge unit에서 끝낸다.
SQLite catalog를 visibility와 incident-event의 sole authority로 만들고 filesystem은 immutable derived projection으로만
둔다.

## owner scope

- `crates/quanta-index-repomap/src/{materializer,model,persistence,store}.rs`와 live layout/quarantine tests
- catalog candidate/activation/invalidation/quarantine schema와 transaction adapters
- `crates/quanta-index-searchd/src/app/runtime.rs`
- search-plane ingest/control RepoMap routes
- `crates/quanta-index-contract/src/repomap.rs`, `crates/quanta-index-sdk/src/repomap.rs`
- P03 owner-local process/fault tests, test/proof authority delta

## 구현 순서

1. `repomap_candidate_v1`, `repomap_activation_v1`, invalidation과 `repomap_quarantine_event_v1` schema, row digest,
   UNIQUE/CAS/state transitions를 구현한다.
2. `CompiledRepoMapCandidateV1`만 seal한다. raw bundle 재해석이나 commitment 재계산은 금지한다.
3. P01A address/security primitive로 immutable object store를 구현한다. raw address digest → strict canonical CBOR
   decode → exact re-encode → payload identity/commitment 순으로 verify하고 create-new/no-clobber/fsync를 사용한다.
4. runtime `RepoMapActivationRecordV1`, `activations` field/directory creation, scan/codec/persist/reconciliation을 삭제한다.
   기존 legacy directory를 runtime에서 변환·삭제하지 않는다. V1 root는 mutation 전에
   `SearchPlaneErrorCodeV2::StateRootFormatUnsupported`; P10 offline importer만 legacy artifact를 처리한다.
5. catalog commit 뒤에만 memory registry를 publish한다. registry는 cache다.
6. same logical generation/same commitment는 original receipt replay, different commitment는
   `SearchPlaneErrorCodeV2::CandidateCommitmentConflict`다.
7. candidate loss/corruption은 durable invalidation하고 파일 재등장/repair publish로 자동 재활성화하지 않는다.
8. publish/activate/rollback ACK에 prior/new commitment, epoch, global terminal sequence, replay status를 결속한다.
9. candidate terminal result, activate, rollback, invalidate, quarantine record/discard는 P02B allocator를 사용한다.
   allocator update + generic event + domain row + relevant activation invalidation을 한 transaction으로 commit한다.
10. quarantine catalog row가 canonical incident envelope bytes/digest/sequence를 보존한다. commit 뒤 readable raw bytes는
    `quarantine/payloads/sha256/aa/bb/<60hex>.bin`, envelope는
    `quarantine/incidents/sha256/aa/bb/<60hex>.cbor`에 create-new + exact-byte replay + file/dir fsync로 투영한다.
11. 두 projection이 durable한 뒤 original unlink + source-directory fsync한다. crash/retry는 catalog의 동일
    envelope/time/sequence를 재사용하고 새 sequence/time을 만들지 않는다.
12. projection failure를 open success로 삼키지 않는다. invalidation은 durable하므로 serve할 수 없다. online discard는
    incident/event를 삭제하지 않고 journaled tombstone 뒤 payload만 회수할 수 있다.
13. 새 error code가 필요하면 같은 commit에서 canonical enum, exhaustive class/repair/SDK/fixture/inventory를 갱신한다.
    ad-hoc const/string code는 금지한다.

## 금지

- file/catalog dual authority, open-time winner selection, publish implicit activation, repair implicit reactivation
- optional digest ACK, partial materializer input, raw candidate bundle reinterpretation
- basename quarantine, `.reason` sidecar, incident overwrite, retry 시 sequence/time 재생성
- runtime legacy directory cleanup/migration 또는 P10 importer 선점

## DoD/proof

- compiler refusal이면 object/catalog/registry mutation 0; publish-only active truth 변화 0
- temp write/fsync/rename/catalog commit/registry publish/projection/unlink 각 crash boundary가 old 또는 new committed
  authority 하나로 수렴
- missing/corrupt candidate → durable invalidation → restart/repair/restart에도 automatic activation 0
- ACK loss replay가 object build/write/activation/sequence allocation을 반복하지 않음
- operation/candidate/activate/rollback/invalidate/quarantine/discard interleave의 global positive monotonic unique sequence
- repeated basename incidents가 모두 보존되고 exact retry는 same sequence/envelope/address
- runtime activation file read/write/path access symbol 0
- V1 root와 legacy `activations/` directory의 bytes/inode/mtime를 바꾸지 않고 typed refusal
- typed CandidateCommitmentConflict/ActivationCasConflict/ActivationTargetNotSealed만 사용; raw code 0

proof node는 `p03-candidate-activation`이다. 이 proof가 live layout/quarantine/activation targets를 모두 선택하지 않으면
S21-01/S21-02 closure가 아니다. Linux production-like host/release daemon binding이 없으면 release GREEN이 아니다.

최종 보고에는 source freeze, exact write set, state table, fault/sequence/quarantine matrix, removed legacy runtime symbol
inventory, proof command/counts/host/binary, NOT_RUN, S21-01B+S21-02 M1 closure와
`artifacts/sep-21/handoffs/P03.json`을 남긴다. explicit owner path만 checkpoint commit하고 push는 별도 요청 시에만 한다.
