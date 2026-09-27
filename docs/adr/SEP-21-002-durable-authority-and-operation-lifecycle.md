# SEP-21-002 — Durable Authority and Operation Lifecycle

Status: `Accepted`

Decided: 2026-09-21

Amended: 2026-09-21 — generic global-event and quarantine crash ownership were made explicit before product cutover.

Amended: 2026-09-24 — prerelease breaking cutover removes the P10 legacy
importer. Old roots are refused; current-format backup/restore is not a format
converter.

Gate owner: S21-00; blocks S21-02, S21-03, S21-04, S21-05 and S21-11

## Durable ownership

- SQLite `repomap_candidate_v1` and `repomap_activation_v1` are the only RepoMap lifecycle and visibility authority.
- The object store owns immutable bytes only. Runtime `activations/` files are deleted.
- P03 deletes activation-file code paths, not legacy bytes: a V1 root is
  refused before mutation. No importer reads or transforms a legacy
  `activations/` tree.
- Memory registries are catalog-derived caches and never win recovery disagreement.
- The state root has one process lease and one global `MutationCoordinatorV1`. Ingest, control and background durable
  mutations all pass through it. Per-socket serialization is not authority.

## Candidate and activation protocol

Publish:

1. validate and compile the entire input under an immutable resource envelope;
2. write immutable object, fsync, atomically publish and verify digest;
3. commit the candidate row and terminal operation result;
4. never change active query truth.

Activation uses a separate `BEGIN IMMEDIATE` transaction:

1. verify exact sealed candidate commitment;
2. compare expected `{activation_epoch, candidate_commitment}`;
3. append activation event and update active row with a positive durable sequence;
4. commit;
5. publish the in-memory cache.

Every activation and rollback increments the epoch. Generation decrease is allowed only through the rollback opcode,
and only to a retained, verified candidate. Missing/corrupt active objects produce durable invalidation and never
reactivate merely because bytes reappear.

## Operation journal

Canonical flow:

```text
authorize
 -> inspect terminal by operation key/body digest
 -> replay or conflict
 -> immutable prepare at recorded epoch
 -> record deterministic refusal OR claim prepared plan with owner/lease/fence
 -> apply only the prepared plan
 -> fenced terminal commit
 -> recover expired/uncertain owner
```

Persisted terminal schema is `OperationTerminalResultV2`:

- `Committed(BatchPublishReceiptV2)`;
- `Refused(OperationRefusalV1)`.

It is canonical-CBOR version 2 and byte-immutable across replay. `AppliedNow` versus `Replayed` is delivery metadata,
not persisted result content. Sequence zero is invalid.

Only deterministic refusal based on exact body plus frozen policy/schema digest is persisted. Authorization,
deadline, cancellation, busy and provider transport failures are not terminally cached.

## Sequence, retention and recovery

The current persistence layout, exact digest domains and closed event codes are
owned by [the catalog sequence owner](../../crates/quanta-index-catalog/src/sequence.rs),
its enum/DDL parity and installed-schema verifier. The initial ADR's hand-written
DDL/digest layout and root-lifetime-only retention rule are superseded by the
completed [SEP-27-005](SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)
contract; they are not an alternate decoder or migration target.

- Terminal sequences are positive and unique in one state-root-global stream,
  with issued domain `1..=i64::MAX`. Exhaustion refuses without wrapping,
  operation mutation or a new event.
- `catalog_sequence_v2` owns event sequencing. Event and domain writes allocate
  in the same transaction. This is distinct from the root-global positive fence
  allocator, which owns claim/prepare/lease fencing, not event numbers.
- `SequenceEventKindV1` currently admits 1..12: committed, refused, aborted,
  candidate seal, activation, rollback, retry supersession, quarantine record,
  quarantine discard, RepoMap invalidation, candidate quarantine and generation
  GC invalidation. Schema/enum parity is checked; old installed definitions refuse.
- Startup reconciles the allocator from the generic ledger before allocating
  recovery events. The contiguous ledger and domain references are verified
  both ways for exact kind/identity/payload/commitment/state. Missing, duplicate
  or mismatched references refuse; startup failure rolls back recovery writes.
- Replay returns exact retained terminal bytes. Generation GC may remove journal
  rows only with target-bound invalidation and the transactionally retained,
  self-digested GC-floor record checked in both directions. Retry supersession
  does not raise that floor; a retired retry cannot silently become fresh.
- Stale owners cannot commit after ownership replacement. Fence history survives
  release/recovery within the root; restored older-root/external fencing remains
  a separate operational boundary.

## Quarantine crash protocol

P03 allocates the observation time and global sequence once and commits the generic event, quarantine domain row and
any required activation invalidation in one transaction. The row owns the exact canonical incident bytes. After
commit, readable payload bytes and the incident envelope are projected with create-new, exact-byte replay, file fsync
and directory fsync. Only after both required projections are durable may the original be unlinked and its source
directory fsynced. Restart resumes from the catalog row with the same time, sequence and bytes. Projection failure is
not ignored as successful open; a durable invalidation remains non-servable. Online discard appends a tombstone event
and may reclaim payload bytes only after commit; incident/event evidence is retained.

## Compatibility

The daemon is V2-only. Legacy candidate, activation, journal and receipt readers are importer-only. A runtime
fallback or dual-write is forbidden.

## Rejected alternatives

- activation file plus catalog reconciliation;
- mutable preflight before terminal replay;
- re-applying an ambiguous in-progress operation without a prepared plan/fence;
- bounded receipt pruning without a producer watermark protocol.
