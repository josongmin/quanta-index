# Copy/paste prompt — P03 Live Layout, Quarantine, Candidate and Activation Cutover

당신은 S21-01B/S21-02 owner다. repo root 기준 prompts의 `COMMON-EXECUTION-CONTRACT.md`와 `README.md`를 먼저
읽고 적용한다. P02I integration commit/handoff와 같은 clean HEAD에서 생성한 P02A/P02B proofs가 모두 일치할 때만
시작한다. 하나라도 없으면 shim을 만들지 말고 `BLOCKED`다.

repo instructions, FINAL-AUDIT, INDEX, amended S21-01/S21-02와 SEP-21-001/002, P02I handoff를 prerequisite로 읽는다.
P01/P02A/P02B handoff는 reference일 뿐 직접 재검증하지 않고 transitive contract는 current tracked schema/API
digest로 소비한다.
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
   candidate/activation/invalidation/retirement의 full closed transition table과 illegal transition refusal을 고정한다.
2. `CompiledRepoMapCandidateV1`만 seal한다. raw bundle 재해석이나 commitment 재계산은 금지한다.
3. P01A address/security primitive로 immutable object store를 구현한다. raw address digest → strict canonical CBOR
   decode → exact re-encode → payload identity/commitment 순으로 verify하고 create-new/no-clobber/fsync를 사용한다.
   live create/open/reconcile 전 경로가 lstat/no-follow/fstat, inode/device, expected uid, exact mode, regular-file,
   `nlink == 1` 검증을 사용한다.
4. runtime `RepoMapActivationRecordV1`, `activations` field/directory creation, scan/codec/persist/reconciliation을 삭제한다.
   기존 legacy directory를 runtime에서 변환·삭제하지 않는다. V1 root는 mutation 전에
   `SearchPlaneErrorCodeV2::StateRootFormatUnsupported`; P10 offline importer만 legacy artifact를 처리한다.
5. catalog commit 뒤에만 memory registry를 publish한다. registry는 cache다.
6. same logical generation/same commitment는 original receipt replay, different commitment는
   `SearchPlaneErrorCodeV2::CandidateCommitmentConflict`다.
7. candidate loss/corruption은 durable invalidation하고 파일 재등장/repair publish로 자동 재활성화하지 않는다.
   boot/open은 catalog candidate/activation/invalidation과 object digest를 exact reconcile한다. P04 pin authority 전
   physical GC는 tombstone-only/disabled이며 pinned handle과 rollback reference 부재를 증명하지 못하면 삭제를 거부한다.
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
13. P01A accepted-code table에 없는 error code가 필요하면 이 lane에서 enum/string을 추가하지 말고
    `BLOCKED`로 종료해 P01A/P02I contract owner로 되돌린다.

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
- symlink/hardlink/wrong owner/wrong mode/`nlink`/address digest mismatch가 mutation 전 typed refusal
- typed CandidateCommitmentConflict/ActivationCasConflict/ActivationTargetNotSealed만 사용; raw code 0

owner-local proof와 Linux release proof를 분리한다. owner node expected tuple은
`id=p03-candidate-activation-owner`, `family=F`, `required_host=any`,
`dependencies=[p02a-repomap-compiler,p02b-operation-journal]`다. release node는
`id=p03-candidate-activation`, `family=F`, `required_host=linux-production-like`,
`dependencies=[p03-candidate-activation-owner]`다. owner-local
node는 live
layout/quarantine/activation/security/state-machine targets를 모두 선택하며 P04 implementation gate다. 기존
`p03-candidate-activation`은 Linux production-like/release-daemon binding release node로 유지한다. owner-local proof가
없으면 S21-01/S21-02 implementation closure도 아니며, Linux node가 없으면 status는 `RELEASE_PROOF_PENDING`이고 release
GREEN이 아니다.

최종 보고에는 source freeze, exact write set, state table, fault/sequence/quarantine matrix, removed legacy runtime symbol
inventory, proof command/counts/host/binary, NOT_RUN, S21-01B+S21-02 M1 closure와
`artifacts/sep-21/handoffs/P03.json`을 남긴다. explicit owner path만 checkpoint commit하고 current lane branch에
non-force push한다. handoff `ticket`은 정확히 `S21-01+S21-02`로 기록한다.
