# OCT-10-002 — Shared paged lexical authorities

Status: `Integrated; local regression and SDK acceptance verified; XL and operational acceptance pending`

Decided: 2026-10-10

Supersedes the storage and admission details of
[OCT-10-001](OCT-10-001-seal-owned-lexical-serving-metadata.md) for the new format.
Historical F16 measurements remain evidence for F16 only.
Current execution and exclusions are recorded in the
[closeout validation](../plans/oct-10-index-closeout/VALIDATION.md).

## Decision

1. One current contract couples sealed manifest 19, compact File authority 18,
   Native authority 6 and activation repository envelope 4. Older artifacts
   require a producer rebuild; no parallel legacy backend remains.
   Native bodies persist only the NFC surface and reconstruct folding through
   the existing normalizer. The prior two-surface admission bound remains in
   force; the reader already computed folding to check its stored duplicate.
   Map pages use the V2 canonical inline/dictionary leaf encoding; dependency
   authentication and the referenced object closure remain unchanged.
2. A generation commits small File/Native roots. Authenticated immutable objects,
   bounded body blocks, posting pages and COW maps live in one track-wide pool.
   Logical document IDs come from durable source-event ranges in the existing
   activation catalog, independently for File and Native namespaces. Physical
   segment compaction preserves logical IDs.
3. Open authenticates the compact control commitments, not every pooled leaf's
   existence or length. Missing, truncated, redirected and same-length corrupt
   shared leaves are detected when selected or scrubbed. Invalid control roots,
   identities and unsupported formats still refuse at open. A missing map key
   is a proved absence; a missing referenced object is an integrity error.
4. Publication checks the original mutation and ID reservation. Independent
   source-byte/tokenizer oracles validate changed derived content. Inherited
   COW roots are authenticated base provenance, not fresh whole-corpus hashing.
   Selected reads hash the exact owned buffers used for decoding and matching.
5. An attempt adopts inherited and newly written objects durably. Header-last
   initialization, pending journal heads and retirement markers make crashes
   recoverable. Seal identity publication precedes lease retirement. Retry and
   maintenance settle an abandoned lease only after independently authenticating
   the immutable target, source event and committed roots under its writer fence.
6. Scrub uses an identity-bound in-process cursor and performs bounded object and
   semantic steps. Shared corruption names every retained referencing generation;
   all known resident/admitting references are fenced before any shared-fault
   receipt is published, including the discovering generation. Receipt attempts
   continue for the other known references after one fails; the first error
   remains observable. Partial receipt failure keeps the current process's admission
   closed. A complete clean proof can
   settle integrity proof fences, while deletion owners retain their fences.
7. GC keeps an exact disk-backed mark set and in-process phase cursors. Complete
   marking and object inventory precede deletion. Epoch changes invalidate the
   pass; every sweep batch is fenced and contains at most 64 objects. The existing
   maintenance timer advances one port by at most 256 work units per tick.
   Restart drops scratch and restarts marking. Retained, quarantined and held-reader
   generation roots and outstanding adoption journals remain protected.
8. Search work is request-scoped. Metadata, postings, selected source bodies and
   previews must share cumulative physical-read accounting. Exhaustion refuses
   typed; it cannot silently turn unexamined candidates into no matches.
   The physical object-read ceiling includes the admitted source count and map
   height for the current bounded metadata walks. Posting/route work retains its
   separate 16,384-read limit; cumulative authenticated bytes remain capped at
   512MiB. Adding another full metadata walk requires revisiting that bound.
9. Pair retention charges private generation files plus the unique reachable
   shared-object closure of the requested generations. Root disk admission uses
   actual whole-track bytes once, including quarantine, unfinished publications
   and garbage until physical deletion. Shared objects cannot be omitted or
   charged repeatedly through a sum of overlapping pair closures. Offline
   backup/restore validates the copied lexical closure before publishing staging.
10. Producers use a scoped durable adoption batch, bounded by the existing object
    codec's 128 dependency references and 2MiB of new encoded data objects plus
    the batch manifest. The writer reserves 5,351 bytes for the maximum-width
    manifest; pending-head recovery enforces the same reference bound. Small epoch/head
    control records are additional. A pending manifest binds the original journal
    head and exact references; recovery completes a valid partial append and refuses
    corrupt chains. Successful scope exit flushes before root publication. Error or
    unwind closes the view and releases its gate. Map root adoption uses the same
    scoped protocol rather than reacquiring an already-held file lock. Producers
    flush before bounded tokenization and compaction preparation; byte/reference
    limits are not wall-clock lock bounds.
11. On macOS, scoped pool-data writes may use POSIX file and shard-directory
    fsync within a group. A successful full-drive barrier must complete before
    any adoption head names that group. Batch manifests and all control records
    keep full synchronization. Existing digest and inherited-root reuse repairs
    file/directory synchronization after an interrupted rename. A shard on another
    device is refused. This changes barrier grouping, not the publication
    durability contract. Non-macOS writes retain the existing synchronization path.
12. FD-relative root writes hold an independently opened directory's exclusive
    OS lock across stale-temporary recovery, write, rename and directory sync.
    Returned pre-rename errors clean their own temporary. Post-rename errors
    remain uncertain publication outcomes; retry authenticates durable state.

13. Authenticated reuse is request-local and bounded: Native segment descriptors
    and shared map-page buffers may be reused within one search. File Record,
    Segment and FilePostings objects share the existing total 4MiB request cache
    with map pages, keyed by the complete ObjectRef. Blob length and digest checks
    still run on reuse. A new request
    creates fresh authentication state. Cache hits still check cancellation;
    physical read meters count misses, while candidate/work limits remain active.
    Body payloads are not retained as a whole-corpus cache. Cache capacity does
    not increase the existing 128MiB Native or 512MiB File physical-read caps.
    Pure gram conjunctions intersect segment-local masks before checking surviving
    current locators. A parent-restricted candidate set retains the existing
    locator-directed path instead of rescanning global anchor segments.
14. Seal source-admission counters describe successfully verified in-memory
    Upsert inputs. They count logical sources and original bytes, independently
    of physical pooled-object reads. Obsolete staged-file/replay read counters
    are removed rather than filled with synthetic values.
15. Authority root names and their byte bound are a lower shared contract.
    Durable quarantine/scrub receipts own their codec and I/O independently of
    the scrub walker. Publication, verification and GC depend in one direction;
    no module-cycle exception is added for this storage path.
16. A fresh independent File seal verification pass uses the same fixed-capacity
    map-page reuse mechanism. It never inherits producer authentication state.
    This avoids repeated physical authentication of the same map path while
    retaining the source-derived oracle and a constant scratch bound.
17. History restore does not turn inactive content damage into a daemon-wide
    integrity admission decision. For the two explicit content-integrity refusal
    codes, a physical backend may prove a conservative pair byte bound using the
    complete state-root index bytes. Restore preserves every existing record of
    the affected pair and validates its pins, generation count, pair byte ceiling
    and root byte ceiling. Healthy pairs retain their existing restore policy.
    If that physical bound is unavailable or exceeds a ceiling, restore refuses;
    ordinary I/O and unsupported-contract errors still propagate. Normal restore
    and new publication retain exact closure measurement. Active compact controls
    are proved by the activation owner; inactive controls and pooled leaves are
    proved by their own doors, selected reads and scrub.

## Limits and tradeoffs

The seal/identity constructor remains the trust boundary; these local hashes
are not an external signature against an actor rewriting every commitment.
Deferring pooled-leaf checks makes normal open cheaper and moves some corruption
detection to selected use or background scrub. A metadata-only open is therefore
not a complete healthy-content proof.

Shared-fault fanout depends on a complete generation inventory. A reachability
failure after inventory fences the complete inventoried set conservatively.
Failure to enumerate the inventory itself is an infrastructure error; this
change does not establish a track-global persistent admission fence for an
unknown set. That boundary remains outside the qualified fanout claim.
An unwritten receipt is not durable admission state. If a receipt write fails,
the process-local fence is retained, but restart-wide quarantine for that
unrecorded generation is not established. A later selected read still authenticates
its own content. Extending this guarantee requires a separate durable recovery
contract; incidental quota hashing is not such a contract.

Native foreground compaction can retokenize inherited documents in bounded
segments. Fresh input and compaction work have separate counters. Compact roots
do not prove whole ingest is O(delta): Tantivy commit/merge, coverage, catalog
history and other publication consumers retain their own costs. Pair quota
measurement currently authenticates the selected object closure using disk-backed
deduplication; whole-track admission walks physical file metadata. These quota
rails are corpus-dependent and require separate performance measurement. No
durable size counter or unproved O(delta) accounting cache is introduced.
On content-damaged restore, the root-wide byte bound can exceed a pair's actual
closure. A large multi-pair root can therefore refuse restoration even when
that pair's exact bytes might fit. This is an explicit degraded-state tradeoff
for proving the configured cap without trusting corrupt dependencies or
introducing another persistent accounting authority.
Coverage pages inherited by hard link are rehashed at seal to detect an in-place
change after preparation. This is an explicit corpus-dependent integrity cost;
request-local reuse of pooled pages does not remove it.

The macOS barrier grouping follows Apple's documented distinction between
[POSIX fsync and F_FULLFSYNC](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html).
The installed Rust 1.92 implementation uses F_FULLFSYNC for both `sync_all` and
`sync_data`; swapping these methods does not remove per-object drive flushes.
Fault injection and SIGKILL tests exercise refusal and restart ordering. They
do not establish physical power-loss behavior or qualify every filesystem/device.

Collection work units bound the incremental object traversal, not wall-clock
latency or OS I/O. The bounded attempt-control recovery and generation identity
inventory are control-plane startup work. No additional worker, service, counter
registry or live-working-tree fallback is introduced.

## Acceptance

Required validation covers fixed search gold, independent derived-content
oracles, delta/no-op/tombstone/reopen, original range retry, real SIGKILL recovery,
shared corruption fanout, partial receipt fences, held-reader retention, GC
progress/staleness, offline backup/restore and consumer integration. Compilation
and historical F16 benchmarks do not satisfy these requirements. Execution status
belongs in the [implementation ledger](../plans/oct-10-index-closeout/LEXICAL-QUERY-PLAN.md).
