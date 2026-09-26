# L2 implementation checkpoint — verification outstanding

Status: **NOT_RUN** for current runtime behavior. This is an integration checkpoint,
not remediation closure. Source and file hashes are in `L2_HANDOFF.source.json`.
The older `l2-preparation-receipt.json` is stale for this implementation.

## Implemented owner paths

- Canonical typed file replacement carries source revision, coverage and both text
  and symbol units. The adapter calls the shared validator before target creation.
  Raw operation lists are fully decoded and require one valid mode/base first.
- Replacement/tombstone deletion and text-authority retirement use the conjunction
  of source-repository and path terms. A same-path file from another source is
  retained. Source provenance is written from the admitted coverage record.
- Typed full/delta publication writes the complete coverage snapshot and source
  event into one generation-local artifact. A delta proves its base before copying
  unchanged coverage, including empty files and failed extraction states. The new
  generation does not inherit the base artifact's identity. Raw mutations cannot
  modify or inherit a coverage-bound index.
- Manifest format 7 commits that artifact. Both generation doors hash/decode it,
  require matching identity and sorted unique rows, and reject missing, unexpected
  or tampered coverage. Missing coverage stays unavailable.
- The core strict gate checks the entire effective source repo/path/language
  universe. L1 supplies the existing filter matcher; result predicates never
  determine capability. L0 connects it before lexical predicate preparation.
- Delta preflight queries the proved base index for replacement candidate IDs
  outside the entire retired source-file set and refuses inherited ID collisions.
  The materializer invokes the required builder preflight before either track or
  provider mutates, and repeats it under the operation lock.
- Existing ActivationCatalog now persists repository envelopes (format 2), each
  containing all revision roots plus source streams, reservations, event identity
  history and high-water. A single durable replacement publishes paired roots and
  source freshness together. Legacy root files are refused for offline rebuild.
- Source-event keys span containing revisions. Reservation precedes materialization;
  original journal reconciliation proves staging. Repackaged events replay the
  original retained receipt and sequence. Different payload/base conflicts;
  pending retargets refuse. Reclaim does not erase source identity history.
- Activation obtains the event from the proved lexical handle, reconciles the
  original journal and checks its semantic roots before CAS. Restart and rollback
  require an accepted event/target history. Rollback retains source high-water.

## Bounds and failure behavior

- Per repository: 16 MiB encoded envelope, 256 revision roots, 256 streams and
  8192 retained events; manifest tokens are bounded to 4096 printable ASCII bytes.
  Admission refuses capacity exhaustion without evicting replay identity.
- Decoding bounds bytes and sequence cardinalities before collecting rows;
  encoding counts against the byte cap before allocating its output buffer.
- Catalog writes currently serialize under the catalog write lock, including
  durable filesystem replacement. This limits simultaneous repository mutations;
  no throughput claim has been established.
- A rename followed by failed parent fsync freezes the process's catalog. Reopen
  reads one envelope, never a root/event mixture. Uncertain or missing journal
  evidence never releases a reservation based on elapsed time.
- Typed source publication requires `seal=true`. A nonsealed first receipt cannot
  later be promoted by changing transport flags under the same event.

## New regression surfaces, all NOT_RUN in this checkpoint

- Lexical `tests/l2_file_mutation.rs`: canonical aliases/conflicts, record path
  mismatch, cross-kind IDs, source-scoped deletion, old pins, empty/full/delta
  coverage, inherited failed-file strict gates, inherited candidate ID refusal,
  actual sealed-generation coverage tampering.
- Search-plane `ingest_dispatcher/tests/l2_file_mutation.rs`: storage-free
  preflight/direct-call refusals before either track or authority changes.
- Activation catalog tests: cross-revision replay/CAS, missing/wrong journal,
  concurrent revision activation, failure before/after replace, retained stream
  capacity refusal, legacy/duplicate/oversized persisted input rejection.
- Dispatcher tests: newer target replays original receipt/sequence and changed
  payload cannot reuse a source event.
- Core/contract leased files contain strict codec/gate/binding regressions. L0
  reported an earlier 299-test shared-foundation run; that historical result is
  not current proof for the complete integrated implementation.

## Remaining integration and required proof

1. L1 added the required `SearchCorpusBatchBuildPort::preflight_batch` declaration
   under its L0-approved outbound lease. Both discovered implementations and
   materializer calls are connected; compile/runtime checks remain outstanding.
2. Finish current DTO/fixture and root-composition migration; freeze all selected
   inputs. Search-plane owner fixtures now provide the new ports; runtime wiring
   remains L0-owned. Check renamed event identity in multi-generation fixtures.
3. Use L0's serialized Rust slot for lexical owner tests and search-plane owner
   tests. Diagnose compile failures as integration failures, never behavior RED.
4. Run affected package checks and the required `just rust-profile test-daemon`
   plus owning scenario proof for shared ingress/activation/state-root changes.
5. Independently verify retained-generation replay, rollback/high-water and
   crash recovery through the composed process. No installed-process, repository,
   release, performance or activation qualification is claimed here.

No commit, push, reset or subagent creation was performed by L2.
