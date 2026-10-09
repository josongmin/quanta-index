# SEP-27-004 — Benchmark Capture, Process and Resource Custody

Status: `Accepted`

Decided: 2026-09-27

Consolidates implemented MISC-01/02, IO-1–4 and C4/C5 decisions. It extends
[SEP-27-002](SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md)
and [SEP-26-003](SEP-26-003-retrieval-evidence-custody-and-qualification.md).
Remaining execution/resource acceptance lives in the
[MISC ledger](../plans/oct-10-index-closeout/VALIDATION.md#affected-checks).

## Decision

### One control plane and publication owner

- Registry selects declared profiles; benchctl routes existing producers and
  domain validators. Domain scorers keep metric mathematics. Python/Rust typed
  evidence shares canonical bytes; product crates have no normal benchmark-only
  dependency. Keep one current internal API and coordinated caller cutovers.
- One run is one case; one capture contains the exact declared family/case
  inventory. `profile_capture.publish_capture` owns complete-profile publication;
  `commit_capture` is its pointer commit. Missing/extra/duplicate cases refuse.
  Same-source runs do not alone prove the same capture. `latest` is advisory;
  baselines bind explicit compatible run IDs and digests.
- Validate registry/profile/source/input/build/command/raw identity, immutable
  staging and domain replay before committing one complete pointer. GC takes
  evidence-store custody before classifying capture membership. Host reservation
  and store custody are distinct; neither store custody nor a reacquired build
  lock is held across long producer execution.
- Keep existing prepared metadata and file-backed `ExecutionResult`. A command
  description is not an executed terminal; do not create a parallel manifest,
  payload-bytes compatibility API, umbrella CLI or run-store layout.

### Admitted capture epoch and sticky failure

- `CaptureEpoch` spans preparation, source checks, every producer, staging,
  promotion, domain replay, final source check and pointer commit. Native nested
  promotion reuses the same epoch; nested entrypoints require the same owner
  context. Pair/lexical routing and disjoint-root checks precede admission:
  unsafe pre-admission refusals intentionally perform no journal write.
- Mutable `work/<capture-id>/capture.json` diagnostics and exclusive bounded
  `failures/<capture-id>.json` are not immutable raw/success inventory.
  Preserve only observed source/input/log/terminal facts and real execution
  digests. Missing terminal never becomes exit zero.
- Nonzero returns, explicit refusals and exceptions are sticky across nested
  callers and callbacks. A caller cannot ignore failure and publish afterward.
  Preserve the primary refusal plus cleanup/journal/marker failures. Failed
  persistence is explicitly NOT_PERSISTED, never durable-evidence success.
- Failure before pointer commit preserves the old complete pointer. Failure
  after commit retains attempted/returned commit state and reconciles the actual
  pointer; do not rewrite immutable history or pretend commit did not occur.
  Success without a complete commit refuses.
- Abrupt death preserves the last journal and old/new complete pointer semantics;
  SIGKILL cannot promise Python cleanup or a finalized failure marker. GC does
  not erase retained diagnostics as if they were runs.

### Process execution and cooperative host observations

- `producer_execution.execute` owns process/session/lifeline, actual terminal,
  file-backed stdout/stderr, drains and bounded cleanup. Preserve nonzero/spawn/
  timeout/interrupt/parent-death/nested-child facts; partial cleanup is not success.
  Command-only native execution uses the same owner without publishing a pointer.
- Start the cooperative monitor at the first owned producer, including build
  work where selected, carry its descriptor through the process-group owner,
  and join/seal raw observations before publication. Keep original process logs
  if observation/sealing fails; no ambient argv/environment secrets enter metrics.
- Derive host fields from validated raw transcripts and bind capture/profile/
  input identity at publication, loading and relocated replay. Required missing
  raw, duplicate/reordered samples, missing end, excessive gaps, clock changes,
  stale identity and forged summaries refuse. Unmonitored imports report none/0.
  The host transcript has its own 64 MiB raw ceiling, separate from control JSON.
- Cooperative same-user reservation and observations are diagnostic. They do not
  establish hostile same-UID custody, isolated/quiet host or performance verdicts.
  Performance admission and actual native/Criterion producer acceptance remain
  separate. Leaf build/test locks cannot enclose recipes that reacquire them.

### Shared bounded file and archive contract

Pin no-follow regular-file descriptors and ancestor custody; check size, device/
inode/mode/link/change epoch before and after complete consumption. Stream hash
and byte count over the exact copied/read bytes, including seek-based archives.
Exclusive staging, flush/fsync and safe parent descriptors remain required.

| Owner / format | Retained limit and behavior |
| --- | --- |
| `evidence.RawFile/RawWriter` | 64 KiB streaming copy/hash; 16 MiB control JSON and per-JSONL-line bound; validate full input, including legal final JSON without LF. |
| Process logs | File-backed normal/failure/cleanup drains and bounded tails; no whole-log decode to produce an error. |
| `raw_archive` and pair replay | Canonical sorted regular entries/fixed metadata; reject duplicate aliases, traversal, links, encryption, corrupt/truncated/reordered/undeclared data. Bound the central directory before parsing, including ZIP64. |
| Pair archive/discovery | 64 GiB archive, 100,000 entries/fanout, 16 MiB central directory; rehash each case, no mtime-only cache; exact tree/mode/binary/source binding. |
| Corpus binding | 256 MiB capsule, exact bundle/metadata inventory and real Git reconstruction; source/output roots are disjoint and external. |
| Recorded/Criterion/native/lexical | Stream trajectories/observations; retain exact pair/artifact/feature/terminal facts. Bound retained metric metadata separately at 16 MiB. Reject malformed/duplicate/partial/nonfinite rows. Agent imports stay unauthenticated. |
| Portable proof and consumers | File-returning production, bounded control/collection metadata, streamed nextest/events/hash, exact execution/reuse identities, final raw/binary/source checks and relocated replay. No `dict[str, bytes]` raw cache. |
| Retrieval command bundle | At most 64 MiB aggregate payload plus 64 KiB ZIP envelope and 32 KiB directory; exact member roles/counts, same-raw metrics and final binary/context rehash. Noncanonical archives require refreeze, not a legacy decoder. |
| Runner bundle | Shared archive owner, 16 MiB envelope, 16 KiB directory, exact members/fixed bootstrap; a self-consistent forged manifest is not admission. |

Bound entry cardinality and retained metadata separately from raw bytes. Partial
write, disk-full, growth/truncation, restored mtime, link/parent swap, interruption
or corruption cannot publish success. File-backed code shape and serialized
limits do not prove physical heap/RSS bounds; IO-5 owns actual adapter acceptance.
No implicit corpus/model/run output in Git and no unsolicited CAS/dedup/retention
rewrite is part of this decision.

### External corpus and composed code-search capture

- A release binds clean exact-commit repositories, complete tracked inventory,
  self-contained Git bundles, streamed blob/object hashes, explicit exclusions
  and independently materialized `code_only`/`developer_search` views. Alias,
  unsupported object/path and byte-universe contradictions refuse. Views retain
  original tracked paths but do not invent a Git HEAD. Validation reconstructs
  retained bundles and view bytes without the original mutable checkouts.
  Releases remain `frozen_not_admitted`: neither gold nor each live index's
  exhaustive searchable universe is attested by the release file count.
- The live external producer executes Sourcegraph/OpenGrok HTTP requests and cs
  processes over the chosen view/pack, retaining complete native response bytes,
  execution identities and deterministic normalized rows. Replay re-derives rows
  and validates metadata/product inventories, exclusions and binary/version
  identity; retained raw and consistency checks do not authenticate remote servers.
- Capture and replay derive cs, Sourcegraph and OpenGrok rows through the same
  native-response decoders and refuse a normalized path or hit that disagrees
  with retained native bytes, even when its derived digest is recomputed. A bare
  normalized-row scorer remains diagnostic. The cs process owner bounds combined
  stdout/stderr and reaps its process group on interruption or I/O failure.
- OpenGrok's opt-in full indexed-view probe compares its native file inventory
  and served file bytes with the release before and after queries. It is a
  bounded observation, not proof of Lucene posting freshness, mutations between
  probes, or Sourcegraph/cs indexed scope. An incomplete or mismatched probe
  refuses that observation; no external product gets a qualified universe from
  a release manifest alone.
- `code-search run` composes external capture, the existing Quanta–Semble pair,
  five-product scorer, both profile validations and raw replays. Publication binds
  both complete capture identities and the external digest, cross-checking lexical
  input bytes against retained pair ZIP and external rows. Failure retains stage
  logs and observed failure state. Long Unix socket paths use a reserved short
  native runtime root; the permanent workflow retains the complete native tree.
- `lexical-diagnostic` consumes a corpus-bound v2 envelope with nine input roles;
  it performs no live product search. It retains per-product report/observations,
  checks per-query hit and recall aggregates independently and publishes only a
  complete five-case capture. File hit rate (any gold file) and macro file recall
  (all gold files) have distinct denominators. Unsupported/timeouts have no score.
  Old records lacking facts require native re-scoring, not relabeling.

### Executable and native measurement identity

Retrieval test execution-context v2 derives compiled executable roles from raw
nextest collection and retains their exact bytes (plus SDK runner/searchd).
Missing roles, contradictory paths, stale epochs or changed binaries refuse;
replay does not depend on the original target cache. A locally bound binary is
not compiler or remote execution attestation. Old contexts are not upcast.

Native payload replay preserves latency/load/freshness units and complete case
sets. Fast/slow aggregates are projected once, offered request accounting
reconciles served/errors/timeouts/drops, and freshness keeps actual transition
samples and observed stale-hit counts. Whole recipe wall time includes builds
and is not query latency. Shared-host diagnostic runs cannot acquire performance
qualification through replay alone.

### Cooperative build admission and native preparation

- `scripts/cargow`/`quanta-index-env.sh` give local leaf Cargo build/test work one
  shared cache-root slot. Metadata/fmt and prepared nextest lists with both bound
  binaries/Cargo metadata do not need a build slot. Unknown commands conservatively
  acquire it. Whole Python/Just orchestrators cannot hold the non-reentrant slot
  while invoking nested Cargo. Searchd pin preparation releases before the main rail.
- The process guard retains lock custody through command-group cleanup, including
  controller death; children do not inherit the descriptor and the lock file is
  never unlinked. Positive wait/command budgets refuse on timeout/interruption/setup
  failure. `wait_ns` and `held_ns` separate waiting from custody, include cleanup,
  and cannot supply success/performance proof. No-acquisition diagnostics report
  waiting only. Direct Cargo, other cache roots/services, unrelated processes and
  escaped descendants are outside this cooperative guarantee; host isolation is
  a separate admission condition. Operator defaults/options live in root usage.
- Cargo history uses process-independent `CLOCK_MONOTONIC`; missing/negative elapsed
  or exit fields refuse, including old rows. Recorded old durations cannot be repaired
  by inference. Shared targets require explicit source/build/binary reuse identity.
- Preserve each binary's feature graph: separate searchd preparation from runner
  preparation. Combining packages can unify different dependency features; unit-graph
  inspection is not binary equality or speed qualification. Prepare nextest once and
  bind/revalidate binaries plus Cargo metadata for collection/execution. The SDK
  `sdk_roundtrip` CARGO_BIN_EXE dependency builds its runner; an earlier duplicate
  runner build is unnecessary. Require its selected `non-test-binaries` metadata,
  hashes, actual runner record and nextest terminal, never an old target-path file.
  `portable_proof.py` owns both SDK producer paths; keep one sequence. Claimed cold
  preparation speed still requires serial quiet-host measurement on frozen inputs.

### Test selection and source closure

- The existing Just/CI rail, test-authority catalog and affected source closures
  own selection. Preserve C4 notification/bootstrap/timing and C5 pair-workspace/
  Cargo-preparation enrollment and guards; no second CI test list/job.
- Derive required identities from actual live collection, selected package/
  target/features and terminal events. Missing, extra-substituted, duplicate,
  skipped or zero-selected proof refuses. Historical numerical test totals are
  not an inventory contract. New tests update selector, owner/scope and closure.
- Moving normative decisions from tickets into ADRs moves their source bindings
  too. A changed adopted ADR must affect the qualifying capture, not silently
  leave its normative inputs outside the closure.

## Consequences and retained regression obligations

Retain negative tests for initial journal failure, every admitted capture phase,
real execution/parent death, nested sticky failure, replay/post-commit failure,
GC/pointer races, full-byte archives and independent host transcript controls.
The native positive GC/replay tail remains in its parameterized positive test;
failure retention is a separate test, with no lost positive coverage.

Owner tests and a generic large-raw subprocess probe were executed historically.
They are not actual-producer, adapter-wide resource, quiet-host, hosted CI or
product/performance qualification. The MISC ledger retains only those open
acceptance scopes and measurements. Native-derived scoring semantics remain
BENCH-02-owned; retaining/hash-binding raw alone does not reject a forged result.

Exact removed ticket/RCA/terminal bodies are recoverable from
`1419f3087f4f09a6ecab4ef39c30a2bf32544d5d`; see
[the plan archive](../ARCHIVE-INDEX.md#historical-record-recovery). This ADR carries no accumulated
test total or new release claim.

## Code-search external owner enrollment

The existing benchmark-control Python rail selects live external and workflow
owner modules. Opt-in test authority binds each to its actual producer and the
existing benchmark-control-capture scope. Retrieval and control-plane source
closures explicitly bind both tests and the workflow producer. Owner guards
check those relationships and collect each module through actual pytest, refusing
missing, malformed, duplicate or empty collections. This is local enrollment
coverage, not evidence that hosted CI or actual external backends executed.

Permanent live owner controls change normalized paths/hit flags, recompute row
digests and retain unchanged native bytes for cs, Sourcegraph and OpenGrok.
The diagnostic scorer observes the forged hit while canonical live verification
refuses it. Capture and replay use that same native decoding authority; a bare
normalized diagnostic score does not attest a backend universe.
