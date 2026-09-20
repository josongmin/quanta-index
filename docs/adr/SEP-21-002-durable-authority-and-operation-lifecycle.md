# SEP-21-002 — Durable Authority and Operation Lifecycle

Status: `Accepted`

Decided: 2026-09-21

Amended: 2026-09-21 — generic global-event and quarantine crash ownership were made explicit before product cutover.

Gate owner: S21-00; blocks S21-02, S21-03, S21-04, S21-05 and S21-11

## Durable ownership

- SQLite `repomap_candidate_v1` and `repomap_activation_v1` are the only RepoMap lifecycle and visibility authority.
- The object store owns immutable bytes only. Runtime `activations/` files are deleted.
- P03 deletes activation-file code paths, not legacy bytes: a V1 root is refused before mutation. Only the P10
  offline importer may read, transform or remove a legacy `activations/` tree.
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

- terminal sequence is positive and unique in one state-root-global stream identified by the root UUID. Its exact
  issued domain is `1..=i64::MAX`, matching SQLite `INTEGER`. Issuing `i64::MAX` atomically changes the allocator to
  explicit exhausted state; every later allocation refuses `SEQUENCE_EXHAUSTED` without wrapping, mutation or a new
  event;
- `catalog_sequence_v2(id = 1, next)` is the sole allocator. Every terminal operation result and every durable
  domain event allocates from it inside the same SQLite transaction that inserts `catalog_sequence_event_v2` and
  its domain row. The allocator row is exactly
  `(id INTEGER PRIMARY KEY CHECK(id = 1), next INTEGER NULL, exhausted INTEGER NOT NULL CHECK(exhausted IN (0,1)),
  row_digest BLOB NOT NULL CHECK(length(row_digest) = 32), CHECK((exhausted = 0 AND next BETWEEN 1 AND
  9223372036854775807) OR (exhausted = 1 AND next IS NULL)))`. The event row is exactly
  `(sequence INTEGER PRIMARY KEY CHECK(sequence BETWEEN 1 AND 9223372036854775807), event_kind INTEGER NOT NULL
  CHECK(event_kind BETWEEN 1 AND 9), identity_digest BLOB NOT NULL CHECK(length(identity_digest) = 32),
  payload_digest BLOB NOT NULL CHECK(length(payload_digest) = 32), event_commitment BLOB NOT NULL
  CHECK(length(event_commitment) = 32), row_digest BLOB NOT NULL CHECK(length(row_digest) = 32))`;
- event kind is closed: `1 operation_committed`, `2 operation_refused`, `3 operation_aborted`, `4 candidate_sealed`,
  `5 activation`, `6 rollback`, `7 invalidation`, `8 quarantine_record`, `9 quarantine_discard`;
- `CatalogSequenceEventCommitmentV2` hashes domain `quanta-index/catalog-sequence-event/v2` over canonical CBOR map
  `{0: 2, 1: event_kind, 2: identity_digest, 3: payload_digest}`. The row digest hashes domain
  `quanta-index/catalog-sequence-row/v2` over canonical CBOR map
  `{0: 2, 1: sequence, 2: event_commitment}`. The allocator row digest hashes domain
  `quanta-index/catalog-sequence-allocator/v2` over canonical CBOR map
  `{0: 2, 1: 1, 2: next-or-null, 3: exhausted-bool}`. All use the SEP-21 general digest framing;
- there is no per-repository, per-plane or per-operation stream. Open/restore derives allocator state only from the
  generic ledger: empty requires `(next=1, exhausted=0)`; non-empty max below `i64::MAX` requires
  `(next=max+1, exhausted=0)`; max equal to `i64::MAX` requires `(next=NULL, exhausted=1)`.
  It then verifies that every generic event has exactly one kind-appropriate domain row and every sequenced domain
  row has the matching generic event kind/identity/payload/commitment. Domain-table maxima never advance or repair
  the allocator; missing, duplicate or mismatched pairs are corruption and fail closed;
- initial `replay_floor` is 1;
- committed/refused terminal records are retained for the state-root lifetime;
- online receipt GC is forbidden;
- replay-floor increase requires an offline migration plus producer cutoff receipt;
- stale fence owners cannot commit after ownership replacement.

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
