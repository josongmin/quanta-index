# J7Q-03 — Measured scale acceptance

Status: `ACTIVE_RESIDUAL`. Parent: [quality index](INDEX.md).
Owner: seeded source generator, storage/runtime and existing `scale_matrix`.
Implemented lifecycle/resource/refusal decisions are in
[OCT-05-004](../../../adr/OCT-05-004-cost-capacity-and-qualification-boundaries.md).
Current execution is owned by [OCT-04 E4-05](../../oct-4-parallel-closure/tickets/INDEX.md#o4-e4-05).

## Remaining acceptance

- Execute current release medium 4 × 64 = 256, large 16 × 256 = 4,096 and XL
  64 × 512 = 32,768 files in fresh external roots. Each has distinct source
  repositories under one serving owner; independent owner generations need a
  separate public publish/CAS/pin concurrency test.
- Freeze tier/seed/repo identities, same-path source keys, file/byte/digest
  inventory, size distribution, hit/symbol density, route mix and model/index
  ownership. Validate all measured cold/warm and adapter/per-repository responses
  against planted source truth after their timer; an empty/short successful page
  cannot enter an aggregate.
- Measure ingest/seal, activation, adapter open, one-file update/deletion,
  same-process reopen/readiness, positive/negative query and recovery separately.
  Compare scoped delete/delta with a fresh rebuild, preserving a same-path file in
  another repository. OS-process restart and cold page-cache recovery need their
  own actual proof; activation may already open the query snapshot.
- Obtain current per-phase CPU/RSS evidence and coverage/gap checks. Whole-process
  CPU/RSS includes harness, daemon thread and retained fixture work; CPU may include
  sampler/probes, sampled maxima are not physical peaks. Logical directory-size
  changes may double-count hard links and do not measure physical write I/O.
  Daemon-only attribution, physical I/O and transient disk remain separate scopes.
- Bind source/IPC/posting/history limits and deadlines to the selected profile.
  Native scale uses `scale-supported-v1`: two generations, 1 GiB per pair,
  2 GiB total history, 600 s client timeout, 100,000 records, 128 MiB source/text,
  256 MiB vectors, 512 MiB staged body and explicit 4 GiB process memory ceiling.
  The small unit harness retains 16 MiB history and 30 s client timeout; ordinary
  product text admission remains 64 MiB and staged-body admission defaults to
  128 MiB under its unchanged 2 GiB process ceiling. Explicit timeout/history
  overrides remain separate diagnostic inputs. Fixed tier shapes are unchanged;
  changing the profile does not turn historical refusals into successes.
- Report each selected tier's success or typed failure/stage, primary and cleanup
  errors, original source binding and limit only where typed authority supplies it.
  Missing latency is unavailable, never zero. Retain default large-timeout/history
  and XL posting-cap failures from their original source; current-source success
  requires new matching binaries and execution.
- Qualified capacity/performance needs an admitted host; portability needs another
  admitted host with matching input/configuration and explicit platform exclusions.

## Large/XL repair boundary

The F15 Large failure at the inherited 16 MiB fixture retention cap and the XL
385,260,565-byte CBOR failure at ordinary 128 MiB request admission are distinct.
XL reached no daemon in that refusal. The product text envelope is a later
admission boundary, not evidence of an executed engine OOM.

SDK and harness now stream publications above 64 MiB into bounded 1 MiB parts
in the private state-root upload store, then commit the unchanged sealed batch.
Incomplete uploads reserve no event or generation. Complete length/SHA-256,
guarded CBOR decode and the original publication binding precede the existing
journal/CAS path. Disk quota, slot count, retry prefix checks, explicit discard
and admission-time idle cleanup bound staging. Commit still materializes a bounded
complete DTO; arbitrarily large single-publication streaming is outside this fix.

Matching-release Large/XL lifecycle execution and qualified quiet-host performance
are separate acceptance scopes. See the runtime profile and operator configuration
in [the native scale guide](../../../../tools/benchmark/retrieval/README.md#native-scale-capacity-profile).

## Output and history

Use registered `scale_matrix`, `summary.json`, `tier_manifest.json` and its
no-replace refusal writer. Non-default tiers require a new absolute external
output root; unselected declarations are not executed results. An all-tier failure
cannot publish an earlier partial success as the complete run.
[B07](../../sep-30-code-search-benchmark-trust/tickets/S30-B07-performance-and-indexing.md)
and [CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md)
own measurement/host acceptance. Historical medium successes, large failures,
XL refusals, binaries and local checks remain recoverable through
[the history index](../../ARCHIVE-INDEX.md#oct-05-benchmark-and-quality-ledger-compaction).
