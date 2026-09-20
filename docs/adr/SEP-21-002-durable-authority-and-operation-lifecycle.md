# SEP-21-002 — Durable Authority and Operation Lifecycle

Status: `Accepted`

Decided: 2026-09-21

Gate owner: S21-00; blocks S21-02, S21-03, S21-04, S21-05 and S21-11

## Durable ownership

- SQLite `repomap_candidate_v1` and `repomap_activation_v1` are the only RepoMap lifecycle and visibility authority.
- The object store owns immutable bytes only. Runtime `activations/` files are deleted.
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

- terminal sequence is positive and unique in its declared stream;
- open/restore reconciles `next > MAX(all terminal sequences)`;
- initial `replay_floor` is 1;
- committed/refused terminal records are retained for the state-root lifetime;
- online receipt GC is forbidden;
- replay-floor increase requires an offline migration plus producer cutoff receipt;
- stale fence owners cannot commit after ownership replacement.

## Compatibility

The daemon is V2-only. Legacy candidate, activation, journal and receipt readers are importer-only. A runtime
fallback or dual-write is forbidden.

## Rejected alternatives

- activation file plus catalog reconciliation;
- mutable preflight before terminal replay;
- re-applying an ambiguous in-progress operation without a prepared plan/fence;
- bounded receipt pruning without a producer watermark protocol.
