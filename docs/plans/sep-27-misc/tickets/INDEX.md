# SEP-27 benchmark / retrieval / test-optimization — execution SSOT

Status: `ACTIVE`. Audited 2026-09-27 KST against
`main@63f09be92fca533bd18d1d71a6464ca30e8073d1` plus the existing dirty tree.
This document is the single current work/acceptance contract for this scope.
All required decisions, ticket bodies, negative tests, measurement rules and
handoff conditions are inline. No deleted plan or handoff is a prerequisite.
Code paths below are repository-relative and identify implementation owners,
not alternative planning authorities. Machine-readable schemas and test
inventories continue to define their executable contracts.

Implementation in the shared dirty checkout is allowed. Preserve unrelated
changes. Qualification requires an immutable source/input/environment identity;
do not manufacture a clean receipt from dirty console output. This document is
bound by both retrieval and benchmark source closures. Finish normative edits
before freezing; put post-freeze results in external digest-bound receipts.

## 1. Final audit and evidence boundary

### Current findings

| ID | Finding and evidence | Classification |
| --- | --- | --- |
| C1 | `benchctl.promote_profile_runs` publishes families separately; `validate_promoted_runs` selects newest runs per family instead of one complete capture. Controlled concurrent GC deleted published runs: promotion exit 0, validation exit 2. A second-family failure produced no complete profile pointer. | Invariant `FAILED`; structural repair in MISC-01 `NOT_RUN`. |
| C2 | Native recipes use direct `subprocess.run`; host envelope uses static `shared`/one sample. Criterion is honestly diagnostic (`none`/zero samples), not capture-time host proof. Shared execution and capture modules already exist. | Source-backed integration gap; MISC-02 `NOT_RUN`. No measured slowdown claim. |
| C3 | Raw verification reads whole files; pair packing materializes the tree and ZIP; unpacking reads complete entries; producer stdout/stderr accumulate in bytearrays. | Source-backed O(raw bytes) memory exposure; MISC-03 `NOT_RUN`. No actual OOM or measured bound claimed. |

Fresh diagnostic command, run before this documentation migration:

```sh
PYTHONDONTWRITEBYTECODE=1 /private/tmp/qi-rbr-guard-final.kmqdr8/venv/bin/python3 /private/tmp/qi-sep27-ssot-audit.YXFB7o/probe.py
```

- Driver SHA-256: `563936e97402fc6e639c83176d669542af66ac869903b8c6efe31f592fc38a7c`.
- Result: `/private/tmp/qi-sep27-ssot-audit.YXFB7o/result.json`, SHA-256
  `ce189af7e44e99a9540beb5c8cab4285f32102d65bd3fbbb37d54e778497d3cd`.
- Raw: `/private/tmp/qi-sep27-ssot-audit.YXFB7o/raw.log`, SHA-256
  `5b36665e2b5a12c8bbcc69ebcaad8c5273acc94a5fe3082d1047959e8e2249a3`.
- Six implementation/test inputs were unchanged before/after; their hashes
  and the dirty inventory are in the result. Python 3.13 private runtime;
  fixed native-artifact fixtures with injected second-family failure and GC.
  Driver exit 0 means the diagnostic executed, not that the invariant passed.
- Excludes actual producers, quiet-host measurements, full-suite execution,
  clean-source qualification and hostile same-UID filesystem attestation.
- `latest` is advisory. Its change alone is NOT a defect. The required
  invariant is complete-profile publication/custody, not advisory rollback.

### Completed frozen proof, not a pending rerun

Frozen HEAD `a66eb6b89f772ae71b871123456e654096beee7d`, tree
`3fa072de5f2fce5ed4beb17e53c33dfd9f3755ef`, clean at capture:

- CI: selected/executed/passed 1,655; zero failed/skipped; 15 dynamic subtests
  reported separately; 12 gates; eight optimized-Python terminal mutants refused.
- Native: SDK 18, Python 324, Rust 108 selected/executed/passed; terminal exit 0.
- Fresh validation plus two replays, both real receipt consumers and eight
  metadata refusals, five bound binary copies, independent source/runtime POST.
- Root receipt: `/private/tmp/qi-rbr-guard-final.kmqdr8/root-final-closeout.json`,
  SHA-256 `64f232c6298614636e25a3a4f96a50ba1e7cabb97280d2008e924a0f9a2bd77e`.
  All 76 referenced artifact hashes were rechecked during this final audit.
  The receipt contains exact commands, environments, raw paths and hashes.
- Native capture: `retrieval-contract-a9bb876c8b28437b9f09d3ce50e40dfa`.
- Scope `VERIFIED`: those frozen local rails only. This migration changes
  source/contract identity; using that proof for the new source is `BLOCKED`.
  New-source qualification is `NOT_RUN`, not a newly discovered code defect.
- Full Rust workspace, installed lifecycle matrix, Linux positive isolation,
  qualified pair/performance, hosted CI and deployment were not established.
  The prior hosted billing block was not refreshed; do not assert it is current.

The external paths are evidence, not documentation dependencies. If absent or
changed, affected historical proof is `BLOCKED`; use a new capture, not a
similar filename/count. They do not become release authority.

### Do not reopen as implementation work

- Exact Python inventory 319-to-324 repair, live-collection guard, and the
  consumer helper correction are present; derive identities, never freeze 324
  as a permanent expected count.
- Query-observation parity and macOS PID/peak/sample validation are present.
  Measurement and platform qualification remain separate.
- Python store reader/writer no-follow, linked-parent refusal, exclusive raw
  creation, duplicate staging and hardlink protections are present. Preserve
  them; do not import an older store implementation wholesale.
- The 18 test-optimization findings below are implemented-owner invariants to
  requalify, not 18 open bugs. Reopen only a reproduced reachable regression.
- Workspace placement and the single Python CLI are decided. No second Rust
  CLI, broad layout rewrite, default-ranker change or optional symbol-authority
  expansion is authorized by this packet.

Concurrent edits observed during consolidation include SIGCHLD-driven completion
in `producer_execution.py`, pure bootstrap caching in `retrieval/evaluator.py`,
diagnostic command timings in `retrieval/portable_proof.py`, and corpus/lexical
tests. They belong to other writers and were not imported, reverted or qualified
by this documentation task. The execution change does not itself replace the
native direct-subprocess path or bytearray log accumulation. Reconcile their
latest implementations before editing overlapping owners; do not overwrite
event-driven completion with an older polling implementation.

## 2. Architecture and common contracts

```text
benchctl + registry: select declared profile and route commands
  adapter: prepare inputs, run producers, invoke existing domain validator/scorer
    producer_execution: process/session/terminal/log/cleanup custody
    host_monitor: declared capture interval observations and local reservation
  profile_capture: complete-profile publication owner
    evidence_bridge: typed run construction, not metric-policy authority
    RunStore: safe raw files and immutable runs
    custody: publication versus destructive GC exclusion
```

- `tools/benchmark/registry.toml` is the only registration authority;
  `registry.py` validates it and `manifest.py` is a read-only projection.
- `tools/benchmark/benchctl.py` is the only current orchestration CLI. Just,
  Cargo and Python producers and domain comparators remain their existing
  owners. Do not recompute domain verdicts in the common wrapper.
- `benchmarks/bench-protocol` owns typed evidence and independent Rust
  validation; Python emits identical canonical bytes. Product crates must not
  acquire normal dependencies on benchmark-only packages.
- Keep one current internal API at each boundary. A coordinated API cutover
  updates all live callers; no versioned twins, fallback readers or repair
  wrappers. Preserve strict persisted-artifact identities and explicit refusal.
- Proposed internal values: `PreparedRun` carries family/case/source/input/
  build/host/command/payload/raw references; `ExecutionResult` carries actual
  terminal state and bounded file-backed stdout/stderr references. Names may
  change; there must not be competing semantic representations.
- A run is one case. A capture is the exact declared family/case inventory
  from one execution. Only the profile pointer is the profile commit point.
  `latest` remains advisory; baselines use explicit admitted ID and digest.
- Profile/input/source/registry identity is fixed before execution and checked
  before commit. Same-source runs alone do not prove the same capture.
- Host reservation and evidence-store custody are different locks. Do not hold
  store custody across long producer execution. Work/raw outputs stay external.
- Corruption, unknown fields, missing inputs, duplicates, stale identities,
  reordered records, partial/timeout/interrupted output and forged digests
  must fail closed. Missing is never zero, empty, skipped-pass or success.

## 3. Ticket map and dependencies

| Ticket | Owner lane | Depends on | Remaining outcome |
| --- | --- | --- | --- |
| MISC-01 | Publication/integration | Shared internal contract agreed | Complete native profile transaction and exact capture validation |
| MISC-02 | Execution/host | Shared execution/raw contract agreed | Owned producer lifecycle and capture-time diagnostic host evidence |
| MISC-03 | Evidence/data path | Shared execution/raw contract agreed | Bounded raw/archive/log I/O without weakening custody |
| MISC-04 | Serial integration | 01-03; final normative edits | Same-source affected gates, actual capture/replay, closure and CI reconciliation |
| MISC-05 | Functional qualification | Stable integrated source | TOPT invariants, Rust/daemon, installed ingest and platform checks |
| MISC-06 | Measurement | 02/04/05 and admitted host/inputs | Distinct test-cost/query/ingest/micro/system measurements |
| MISC-07 | Profile/product coverage | 04; available external inputs | Complete execution inventory, live comparison, conditional outcome claims |

All implementation/qualification tickets are `NOT_RUN` on their proposed final
source. C1's executed negative diagnostic is `FAILED`; missing prerequisites
block only dependent claim scopes. User-owned inputs in section 11 are not
automatic coding subtasks. A result satisfying multiple tickets is referenced
by one capture ID; do not schedule duplicate execution.

## 4. MISC-01 — atomic complete-profile publication

### RCA and file-level changes

`tools/benchmark/benchctl.py::promote_profile_runs` bypasses the complete-capture
owner; `validate_promoted_runs` scans newest family runs. Existing
`tools/benchmark/profile_capture.py::commit_capture/load_capture` already has
the exact-inventory and atomic-pointer abstraction.

1. Extend the existing profile-capture module with one publication entrypoint
   accepting prepared runs, capture ID, profile, registry digest and expected
   inventory. Do not create another transaction module.
2. In `benchctl.py`, prepare every family and invoke that entrypoint. Derive
   expectations from registry plus independent producer listing, not observed
   successful output. Current native aggregate families use `case_id=None`;
   Criterion case IDs come from the binary listing.
3. Under `tools/benchmark/custody.py::custody`, establish the capture-store
   marker before promoting any run, promote immutable runs, verify exact
   inventory/source/registry, then use `commit_capture` to publish once.
   The marker preserves the Rust collector's refusal of orchestration-owned
   capture stores; Python GC uses the same custody lock.
4. Validate one `load_capture` result. Retain source, lockfile, host-policy,
   preflight, payload-kind and raw-derived domain checks. Do not weaken them
   when removing newest-family selection. Missing pointer means no complete
   capture; do not synthesize one by scanning runs.
5. Move repeated publication orchestration from capture adapters into this
   owner only where it is genuinely shared. Keep domain preparation/replay
   in the adapters. `evidence_bridge.py::promote_native_run` remains one-run
   materialization, not another profile transaction.

### Commit and failure semantics

```text
PREPARED -> PRODUCERS_COMPLETE -> RAW_VALIDATED
  -> [custody: RUNS_STORED -> CAPTURE_STORED -> PROFILE_POINTER_COMMITTED]
  -> fresh-process validation/replay
```

- Before pointer commit, any failure preserves the prior complete pointer.
  Orphan immutable runs are not a completed profile. Preserve failed epochs;
  subsequent explicit GC may remove only unreferenced data.
- Crash after capture creation but before pointer replacement must not make a
  partial profile current. A valid immutable capture may remain retained.
- Crash/response loss after pointer replacement is an ambiguous response, not
  permission to rewrite history. Resolve the exact capture ID and digest.
- GC validates all custody references before deletion. Malformed references
  stop collection. Never delete unknown references to recover apparent success.
- Do not add advisory `latest` rollback or use `latest` as a baseline.

### Acceptance and tests

Owners: `tools/ci/tests/test_benchctl.py`,
`test_benchmark_profile_capture.py`, `test_benchmark_evidence_bridge.py` and
`test_bench_protocol_conformance.py` under `tools/ci/tests/`.

- Positive multi-family profile roundtrip and fresh replay.
- Failure on second family, missing/extra/duplicate case, wrong profile,
  registry drift, same-source mixed capture and payload/preflight mismatch.
- Cross-process publication/GC interleaving, not only a same-thread mock.
- Failure/crash before and after each commit boundary; old complete pointer
  remains usable or exact new complete pointer is recoverable.
- Existing symlink/linked-parent/hardlink/staging refusals remain intact.
- DoD: the current C1 counterexample no longer loses a complete native profile
  to GC; complete publication semantics hold for every adapter using the owner.

## 5. MISC-02 — producer and host lifecycle

### RCA and owners

- `tools/benchmark/benchctl.py`: replace direct native-recipe subprocess calls
  with `producer_execution.execute`; remove invented one-sample host evidence.
- `tools/benchmark/producer_execution.py`: retain private session, parent
  lifeline, actual terminal record, unreaped group identity, bounded cleanup
  and signal handling. Exit zero alone does not establish descendants exited.
- `tools/benchmark/criterion_capture.py`: bind observations to declared build,
  warmup and measurement phases rather than a static host dictionary.
- New `tools/benchmark/host_monitor.py`: own capture-time observations and
  cooperative local reservation; `evidence_bridge.py` derives typed host facts
  from the validated observations instead of caller-supplied summary constants.
- `tools/ci/source_closure.py`: enroll new module/tests in the affected closures.

### Logic

Build/provision outside timed measurement where the protocol permits; bind
binary bytes. Reserve host, record start, record periodic samples with sequence
and monotonic time, execute declared phases, clean up producer resources,
record end and release reservation. Preserve actual phase boundaries. Host
monitoring does not silently redefine cold/warm or correctness boundaries.

Observations bind capture ID, host identity, reservation identity, start/end,
sequence, maximum unobserved gap, clock discontinuity and interference facts.
Hash process commands where needed; do not dump secrets from ambient command
lines or environments. Unknown observations fail, not default to a quiet host.

Timeout, interrupt, spawn failure, malformed terminal or monitor failure leaves
explicit failure and retained raw logs. Preserve primary and cleanup errors.
Do not report completion while pipe drain/reap is incomplete. No broad process
kills, foreign Cargo termination or PID-name ownership heuristics.

A cooperative lock excludes participating controllers only. It cannot prove
thermal/power/scheduler isolation or external process absence. Its successful
capture is diagnostic; qualified performance requires section 10 host admission.
If performance was requested and prerequisites fail, refuse that claim; never
silently downgrade the request and return performance success.

The isolated implementation at
`/private/tmp/qi-bm-host-monitor.c3ah44/checkout` is a candidate, not authority.
Compare exact owner hunks with current main. Do not copy its older retrieval,
conditional proof, query-parity, macOS resource or test-authority changes.

### Acceptance

- Real timeout/interrupt/parent-death/nested-child tests; no surviving owned
  processes; finite cleanup; malformed or absent terminal cannot pass.
- Monitor tests: missing/reordered/duplicate samples, missing end, stale
  capture, changed reservation, excessive gap, clock discontinuity,
  observation failure and forged host-envelope fields are rejected.
- An otherwise valid cooperative observation cannot issue a performance verdict.
- Correctness on/off observation parity is preserved; monitor/log overhead is
  measured separately, never subtracted from one product without disclosure.
- Existing execution tests in `tools/ci/tests/test_criterion_capture.py` remain
  effective. A dedicated execution-test module may own them after a single
  relocation with exact test-authority/selector updates, not duplicate coverage.

## 6. MISC-03 — bounded evidence, archive and log I/O

### RCA and files

| Owner | Required change |
| --- | --- |
| `tools/benchmark/evidence.py::_read_regular_file/_verify_raw` | Share a no-follow descriptor/epoch guard; compute digest and size in bounded chunks. Small control documents use explicit size limits. |
| `tools/ci/lint/handoff_validation.py::_consume_repo_regular_file` | Reuse the complete-read descriptor primitive; change only if required, preserving all existing callers' complete-read/identity semantics. |
| `tools/benchmark/evidence.py::StagingRun.write_raw` | Add one canonical streamed-file staging path with exclusive creation, incremental digest/size, flush/fsync, safe parent descriptors and typed failure. |
| `tools/benchmark/evidence_bridge.py::promote_native_run` | Accept validated file-backed raw references rather than forcing large `bytes` values; maintain one internal contract. |
| `tools/benchmark/pair_capture.py::pack_native/unpack_native/tree_bytes/capture/replay_run` | File-backed ZIP generation and entry streaming; remove whole-tree and whole-archive memory copies. |
| `tools/benchmark/producer_execution.py::_wait_for_terminal/execute` | Drain stdout/stderr to owned files with bounded buffers; preserve full raw bytes and limited error tails. |

Coordinated callers: `criterion_capture.py`, `retrieval_capture.py`,
`lexical_capture.py`, `pair_capture.py`, `recorded_capture.py`, `corpus_release.py`
under `tools/benchmark/`, and `tools/benchmark/retrieval/portable_proof.py`.
Update every affected read/parse/replay path in the same API cutover. Stream
Cargo JSONL and other unbounded line protocols; bounded control-output reads
must reject oversize, not truncate into valid-looking input.

### Invariants

- Source and destination roots remain disjoint; corpora/models/indexes/results
  stay outside Git. No implicit temp archive in the checkout.
- Pin regular-file descriptors with no-follow ancestor traversal and verify
  size, inode/device/mode and change epoch before/after consumption. Never
  replace this with a check-then-open pathname or only an mtime check.
- Streaming digest binds the exact copied bytes and count. Seek-based ZIP
  replay must preserve pinned-descriptor identity and complete hash accounting;
  do not disable the helper's complete-read check to accommodate seeking.
- Deterministic ZIP: sorted unique canonical names, fixed metadata policy,
  regular entries and declared compression; reject traversal, links, encryption,
  duplicate aliases, corrupt/truncated entries and undeclared inventory.
- Bound archive entry count, total bytes and control-document sizes explicitly;
  record/refuse exceeded limits. Metadata cost may scale with file count but
  payload memory must not scale with total raw bytes.
- Partial writes and disk-full preserve a failed epoch and never publish a
  complete profile. Do not silently retry against a different input or location.
- Do not introduce CAS/deduplication or alter corpus-retention policy merely to
  fix streaming. Measure disk/RSS first; shared-blob retention would require
  independently tested references/GC and a separately justified change.

### Acceptance

Owners: `tools/ci/tests/test_bench_protocol_conformance.py`,
`test_benchmark_evidence_bridge.py`, `test_pair_capture.py`,
`test_criterion_capture.py` and affected adapter/portable-proof tests.

1. A deterministic reader oracle rejects unbounded `read()`/whole-buffer use;
   verify maximum chunk size, full digest and exact byte count.
2. Growth, truncation, same-bytes link swap, restored mtime, parent replacement,
   duplicate staging, disk-full and interrupted output cannot become success.
3. Large raw, large entry and many-entry archive roundtrip; byte/case identity
   and domain verdict match the fixed independent oracle.
4. Actual subprocess peak-RSS measurements at increasing input sizes support
   the claimed bound. Declare platform/sampling tolerance; no OOM claim from
   code shape alone and no bound claim from a mocked read-size test alone.
5. Python/Rust canonical evidence fixtures stay equal. No wire-schema expansion
   is needed for file-backed internal I/O; if one becomes necessary, stop and
   revise producers/readers/goldens together rather than add compatibility twins.

## 7. MISC-04 — serial integration and source-bound closeout

- One integration owner controls `benchctl.py`, `evidence_bridge.py`, shared
  internal APIs, source closures, test authority and final documentation.
  Storage and execution lanes may work separately after that contract is fixed.
- Preserve all unrelated dirty edits. The preexisting inventory repair and
  `test_required_python_inventory_matches_live_collection` must remain together.
- Import only independently relevant isolated hunks; no whole-checkout copy.
- Finish normative changes, then freeze HEAD/tree/dirty overlay, Cargo/Python
  dependencies, features, registry, selectors, toolchain, target root, binaries,
  inputs, host and runtime identity. Use a clean isolated qualification source;
  source changes require a new affected run, not an edited passing receipt.
- Run cheap targeted tests first, then affected Python control-plane and Rust
  contract tests; actual SDK/contract capture; fresh validation/replay and both
  actual consumers with metadata refusal controls; independent pre/post identity.
- Required-test inventory is exact identity equality with actual collection,
  command/package/target/features and terminal events. Omitted, duplicate,
  skipped, extra-substituted or zero-selected proof is refused. Test additions
  update the appropriate authority in the same change; counts are derived.
- Recheck local CLI/Just/registry/CI selector parity and same-raw old/new domain
  validator parity where both express the metric. No default values for metrics
  one side cannot represent. Hosted jobs require actual job terminals and their
  source identity; local proof is not hosted CI or release activation.
- DoD: all requested scopes have `VERIFIED`, `FAILED`, `BLOCKED`, `NOT_RUN` or
  `NOT_APPLICABLE`, exact command, raw results/counts, source/input/environment,
  artifact path/SHA-256, covered/excluded scope and next condition.

## 8. MISC-05 — functional, installed and platform qualification

### Test-optimization invariant census

These are coverage obligations, not presumed remaining implementation defects.
Resolve current selectors from `tools/ci/test-authority.toml` and actual
collection, then record one owner/oracle/terminal result per row.

| ID | Current owner surface | Independent invariant |
| --- | --- | --- |
| D1 | searchd runtime end-to-end/repomap fixtures and searchd harness | Long-path configuration covers all three sockets, including ingest. |
| D2 | searchd runtime `tests/common/searchd_binary_process.rs` | Already-exited child cleanup cannot signal an unrelated reused process. |
| R1 | runtime lifecycle/lease fixtures | Explicit acknowledged release; no mandatory three-second happy-path sleep. |
| R2 | embed OpenAI retry/sleeper seam | Injected test delay; production retry bounds/jitter unchanged. |
| R3 | embed concurrency fixtures | Structural barrier proves four-way overlap, not a 25 ms timing guess. |
| R4 | IPC `PeerWatch` | Explicit wake/disarm/join; cancellation needs no compensating sleeps. |
| R5 | runtime ingest-resource envelope and lower core owners | Boundary assertions conserved below E2E; one wiring proof retained; daemon boots counted. |
| WA-1 | lexical trigram property tests | Unexpected error makes the property fail, never a skipped success. |
| WA-2 | runtime matrix smoke | Exact expected candidate identity/set, not any in-corpus row. |
| WA-3 | runtime state migration | Manifest/object/digest completeness; missing/corrupt data fails. |
| TH-1 | SDK-frontdoor observation waits | Never-ready input returns typed timeout, not stale `Ok`. |
| TH-2 | runtime process-envelope scrape waits | Never-true predicate returns typed timeout. |
| TH-3 | SDK binding fixtures | RAII temporary path custody; no pid-only persistent socket directory. |
| TH-4 | runtime filter-execution cases | Immutable family fixture reuse with per-case context and no shared mutable daemon. |
| PO-1 | core `timeref.rs` | Injected exact clock and fixed boundary matrix; convenience edge samples once. |
| PO-2 | SDK `config.rs` | Injected environment precedence/errors; no process-global mutation seam. |
| PO-3 | catalog connection/idempotency | One clock sample per transition; less/equal/greater deadline matrix. |
| PO-4 | search-plane `single_flight.rs` | Outcome-or-cancellation wake; no correctness dependence on 20 ms polling. |

Do not shorten sleeps, disable production jitter, weaken errors, share mutable
global fixtures or delete lower-layer assertions to make timing look better.
Only reproduce-and-fix a current regression; otherwise retain the implemented
owner and execute its qualification.

### Installed ingest and process/resource checks

- Use actual separately launched searchd and installed/public SDK for publish,
  seal, CAS activation and query. In-process harness/direct IPC is not SDK proof.
- Exercise fresh, replace, delete, edit/rename where claimed, reopen/replay,
  tombstones, real fault injection and restart. Compare full expected row sets,
  unchanged owners, generation, activation and disappearance of old hits.
- Transient ingest observation binds request/repo/revision/batch/generation/
  receipt/activation; reject mixed, missing, partial or stale observations.
  Do not add transient stage timings to durable receipts. Old V2-incompatible
  wire requests must be rejected before dispatch; body digest is preserved.
- macOS v1: reject emitted PID duplicates, per-process peaks above tree peaks
  and per-process samples above global samples. A live zero-RSS root may be
  absent from positive-RSS metric rows; those rows do not prove PID start identity.
- Linux: actual delegated-cgroup and Landlock positive execution under an
  available supported host. Fake-owner/process-group tests are not that proof.
- Whole-process-tree resources include daemon/provider children. Record index
  bytes separately from shared model-cache bytes. Windows native pair and new
  canonical symbol-text authority are `NOT_APPLICABLE` absent new product scope.

DoD: focused invariant terminals plus same-source full Rust/daemon rails and
installed/platform evidence for the claims selected. Remaining host access
blocks only that platform; no generic all-platform success.

### Retained retrieval implementation invariants

These implemented contracts remain required regression coverage; they are not
additional feature tickets. Reopen a code change only for a demonstrated failure.

| Surface | Required invariant / negative control |
| --- | --- |
| Query policy | Exactly one of native/literal/natural_language; preserve native DSL AND and literal escaping. Natural-language lexical token-OR and semantic text have distinct bound identities. Gold/category/holdout data never drives planning. |
| Observation | Executed versus contributed lanes are distinct; OFF omits query-stage collection/DTO data but preserves operational/deadline clocks. Compare exact enabled/disabled startup policy and config digest; server/SDK/sidecar timings remain separate. |
| Semble mode dispatch | native-default, hybrid-no-rerank, lexical-only and semantic-only use the same pinned function/profile in cold, warmup and measurement. Bind requested/actual alpha, rerank, lane counts, depth and upstream source; alpha endpoints do not prove only one lane executed. |
| Symbol publication | Chunks and symbols replace together; symbol-only changes alter scope digest. Reject duplicate/cross-kind IDs. Parser/grammar/lockfile/capability identities and per-path unsupported SHA/reason are explicit; supported parse failure is coverage failure. |
| Symbol ownership | Use AST ownership, not delimiter/lexical guesses. Rust generic impl owners and direct method scope, JS/TS function declarations versus explicit methods, and Python nearest named scope have independent fixtures. |
| Result authority | Published typed unit registry, generation, path and byte span authorize hits. Reject forged/stale/unanchored hits. Unsupported symbol Phrase/RawString/Regex/regexp-keyword/content-filter combinations remain typed refusals, never silent chunk fallback or empty exhaustive success. |
| Span accounting | Indexed identity drives rank; returned bytes/tokens drive context cost; independent source spans drive exact recall. Union overlapping spans; hand-check Unicode/CRLF/long-line cases and line-expanded context. |
| Semantic parity / ANN | Full vectors with pinned model/tokenizer/config, canonical adversarial inputs, norms and pairwise directional checks. Reject omitted/subset/reordered/forged vectors and nonfinite/scalar-type substitutions. Independently exhaustive-scan the same rows; cover 255/256, short/full result, filter/page/churn boundaries. |
| Fetch experiment | Only typed integer 25/50/100; default 100. Requested policy, daemon config and actual initial-fetch trace agree. Preserve ceiling/refill/generation pinning/force-empty; reject bool/float/alias/unknown/missing/duplicate settings. No omitted/duplicate hits across pages. |

Minimum owner checks are the retrieval Python contract, Rust chunking/library,
actual SDK process, relevant storage/semantic integration and asset-backed
model tests when claimed. Asset-free validators cannot substitute for actual
model execution or production-served ANN checks.

## 9. MISC-06 — measurement, with separate denominators

| Track | Required workload and reporting |
| --- | --- |
| TOPT R1-R4, TH-4 | At least five warm paired samples per selector; median/p95/min/max, selected/executed/failures. Cold build separate; same features/toolchain/cache/target policy. R5 measures assertions/cases/daemon boots, not invented speedup. |
| Query observation | Same source/binaries/inputs on/off; roomy and tight deadlines; k=1/10/100 plus public cap boundaries; filters and explicit fetch floors 25/50/100. Compare result/order/page/cursor/failure, coverage, planner/lane/candidates. Normalize only request IDs and stage timings. Keep default hybrid floor 100. |
| Installed ingest | Fresh-root time-to-searchable separately from replace/delete/fault/restart latency; full row-set/activation correctness is prerequisite. Fresh root is the statistical unit. |
| DSL authority | Canonical Linux, warm/cold separate, current registered floors 200 warm / 20 cold. Actual admitted baseline and host required; a plan listing is not capture. |
| Micro | Both `quanta-index-lq-norm/pipeline` and `quanta-index-searchd-runtime/dsl_query_matrix`, every binary-listed case, smoke, raw samples/estimates and immutable binary identity. Criterion diagnostic minimum 10 samples and 1,000 resamples is not performance admission. |
| Systems | Freshness truth and offered-load producer; offered/accepted/completed/dropped/error/timeout counts, generator saturation/health, latency, RSS and disk. Closed-loop QPS does not establish open-loop capacity. |

Correctness precedes timing. Retain failure/timeout/partial samples and their
denominator; never remove failures and report a faster survivor distribution.
Predeclare clock/resolution, transport/serialization inclusion, warmup, cache,
power/thermal/governor controls, compiler profile and instrumentation mode.
Do not run competing Cargo gates or both products concurrently on a timing host.
`QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` cannot qualify performance.

Bind before/after source diff for test-cost attribution. Unrelated intervening
changes mean retrospective comparison, not causal proof of one optimization.
CI/p95 from five observations is descriptive; do not invent statistical power.
Baseline admission binds explicit immutable run ID/digest, compatibility,
margin and uncertainty method. Missing baseline is not zero regression.

## 10. MISC-07 — profile coverage and retrieval acceptance

### Execution inventory and product pilot

For every active profile/case, record registry digest, required input,
producer command, terminal count, raw inventory, validator/scorer, capture ID,
fresh replay, claim class and exclusions. `list`, `plan`, `runnable=true`,
compile, fixture and local replay are not interchangeable evidence.

Audited profile inventory (re-resolve registry and case listing at execution):

| Profile | Declared families |
| --- | --- |
| dsl-authority | dsl-warm, dsl-cold |
| dsl-diagnostic | micro-searchd-runtime-dsl-query-matrix |
| quality-core | relevance, ambiguity, snippet, scale, tail |
| quality-full | relevance, ambiguity, snippet, scale, tail, ann, concurrency, freshness, open-loop, ops, ui |
| systems | freshness, open-loop |
| semantic-ab | relevance-openai-ab; actual provider access required |
| micro | micro-lq-norm-pipeline, micro-searchd-runtime-dsl-query-matrix |
| retrieval-contract | retrieval-sdk, retrieval-contract |
| retrieval-diagnostic | retrieval-pair |
| lexical-diagnostic | lexical-file-comparison |
| recorded | agent-outcome, scan-vs-index |

Distinguish absent external inputs from an absent executable adapter. If an
active registered producer still lacks a supported capture/payload/replay path,
record that exact implementation gap under this ticket and repair its existing
adapter boundary; do not rename it an external-input block or fabricate a
generic latency payload. Do not count one profile's subset as quality-full.

- Run representative real Just/daemon, Criterion, Python adapter and recorded
  import through the common CLI. Unsupported external-input profiles refuse
  before publishing a supported subset as a whole profile.
- Execute a same-release/same-query-pack live pilot on at least two repositories
  for Quanta and accessible comparators: Semble, Sourcegraph, OpenGrok and
  codespelunker where capabilities permit. No obligation to fake support or
  access for all five. Record actual indexed/searchable universe, query
  transformation, native rank, response completion, freshness and failures.
- Separate lexical/semantic/hybrid/symbol strata and `native_default` versus
  `controlled_mechanism` lanes. A lexical file result is not semantic quality;
  file-only output is not a fabricated whole-file span. No combined leaderboard
  across incomparable units or native/emulated/remote execution boundaries.
- Literal/regex truth comes from pinned source bytes and an independent oracle.
  Developer relevance needs adjudicated qrels; keep unjudged distinct from
  irrelevant and report judging-pool coverage/sensitivity.
- Recorded agent imports stay unauthenticated diagnostics. Authenticated
  outcome requires actual frozen tasks, independent baseline tests,
  trajectories and raw executed-test receipts; aggregate booleans do not suffice.
- A two-repository or 20-query pilot proves machinery and diagnoses gaps, not
  all-language superiority. No product ranking/default change without an
  independently demonstrated defect and the declared evaluation below.

### Retrieval comparison, data and scoring contract

All systems use one pinned repository commit, canonical sorted path/SHA byte
universe and frozen query pack. The current pair runner operates one repository
per capture; aggregate real per-repository pairs, never invent a synthetic
multi-repository commit. Corpus generation excludes unsafe/ignored/binary/
oversize inputs with retained reasons and uses the declared language/file policy;
candidate corpus presence does not prove each product indexed that universe.

The suite, query pack and schema-v3 runner records share byte-equal
`comparison_contract`: `top_k`, tokenizer, tokenizer-budget version and
output-unit policy (`rank_prefix`). Route records reference exact capture
provenance: runner/searchd binaries, generation, seal/activation, model and
chunk strategy/config. Supported strategy names are `whole_file`,
`fixed_window_strict`, `fixed_window_line_aligned`, `brace_heuristic` and
`semble_native` (Semble-owned only). Semble 0.6.0 is the frozen comparison
revision, not a claim that it is the latest release. Pin actual package/commit,
dependency lockfile and model assets for every capture. Reject old/unknown
record contracts rather than auto-upcast.

Tiny fixed-oracle repositories prove mechanics. Real evaluation data is split
before results are seen: separate development/holdout queries, near-duplicate
paraphrases, answer spans, query families, files and definitions as declared.
Reject leakage except explicit reviewed both-sides rationale-backed exceptions.
Gold/grades/category labels are evaluator-only, never runner/planner inputs.
`isolated` quality requires enforced inability to read gold plus an access-block
log. `attested` is weaker and cannot issue isolated quality proof.

Independent reviewers assign grades 0-3 under a frozen rubric: sufficient,
substantial/incomplete, weak support, irrelevant. Two distinct annotation
authorities and adjudication are required for qualified gold. Engine output or
model-generated/model-checked labels alone are not independent holdout truth.

Qualified primary metric: graded density-aware NDCG@10, scorer identity
`rb-rank-context-density-first-coverage`. A candidate earns credit only for
fully containing a gold byte span. Each gold span is credited once at its first
covering rank; use the maximum grade among newly covered spans, not their sum.
Let U be their union byte length and C the candidate bytes. Gain is
`(2**grade - 1) * U/C`; use rank-discounted gain and exact-span ideal DCG sorted
by grade. Publish raw chunk ranks and deterministic best-chunk-per-file collapse
separately. Same-file chunks consume ranks/budget; duplicates cannot boost gain.
Without independent grades, span Recall/MRR/BCY are diagnostics, not substituted
qualified NDCG. Report exact-span Recall@10, rank MRR/Hit@1, context bytes,
BCY and no-answer false-positive/abstention as separate secondary metrics.

Top-k policy `declared_top_k_v1` requires top_k>=10 for the @10 primary; @k above
the captured window is `NOT_APPLICABLE`, not extrapolated. Freeze the candidate
matrix, primary/guards, margins, order/seed, repeats and failure policy before
development evaluation. Select one final combination before opening holdout.
Primary paired 95% CI lower bound must meet the predeclared noninferiority
margin (default zero); the claimed effect must meet its declared improvement
rule, with all strata guards. No post-hoc exclusions or holdout retuning.
Use query/repository clusters for quality and preserve query/root structure for
latency; report strata and leave-one-repository-out sensitivity. Repeated calls
are not independent queries. Report bundle effects as bundle effects unless
separate ablation and independent holdout establish individual contributions.

### T00-T17 blocking matrix

| ID | Positive contract | Required negative controls |
| --- | --- | --- |
| T00 | Both runners' exact tracked path+SHA universe | Dirty/wrong revision, ignored/extra/missing/binary/oversize/symlink input, divergent filter |
| T01 | Pinned label bytes/lines, safe paths, blind pack and universe digest | Wrong location/hash, absent required gold, leaked grade/gold, mismatched universe |
| T02 | Dev/holdout and query-family separation, reviewed categories/grades | Duplicate/normalized/shingle-near queries, cross-split family/span leakage, invented no-answer |
| T03 | Exactly one eligible task/route row, ordered ranks/spans, exact contract and capture pins | Missing/extra/duplicate row, unknown field, nonfinite time, null versus zero confusion, dangling capture, forged lock |
| T04 | Hand-calculated span Recall/MRR/density-NDCG/BCY and independent rubric | Wrong lines, partial span credit, duplicate boosting, budget overflow, whole-file full credit |
| T05 | Separate fresh daemon, public readiness before publish | Wrong socket/state root, stale daemon/index, no readiness |
| T06 | SDK publish/seal receipt and exact CAS activation acknowledgement | Direct-IPC substitute, partial seal, digest mismatch, conflict, query before activation |
| T07 | SDK lexical/semantic/hybrid real ranked spans at expected generation | Wrong route/generation, typed timeout/error turned into empty success, capped result called exhaustive |
| T08 | Original byte slices, line bounds and stable chunk IDs | UTF-8/CRLF/BOM/EOF errors, empty/overflow span, nondeterministic IDs |
| T09 | Supported parser boundaries and explicit fallback coverage | Unsupported grammar, parse/long-declaration failure, silent whole-file fallback, overlap inflation |
| T10 | Rebuild semantic sources per chunk strategy with pinned real model | Reused vectors from another strategy, hash-dev claimed as model quality |
| T11 | Real pinned Semble mapping to exact admitted bytes/spans | Truncated snippet as full chunk, path drift, skipped query, missing model, filter mismatch |
| T12 | Complete real pair with equal commit/files/queries/k/budget/host and rebound pins | One-sided/public-score substitute, partial samples, wrong host/model/cache, repeated few-task qualification |
| T13 | Same immutable raw gives identical aggregate/disagreement/digest after relocation | Path/order-dependent score, omitted errors, duration exceeding enclosing phase, invalid result marked qualified |
| T14 | Registered commands, external output, fresh final-path public replay | Implicit CI downloads, dirty output root, unselected test claim, stage-only replay |
| T15 | Same-model claim: pinned weights/tokenizer/normalization/precision and full raw vectors within declared tolerance | Same name/dimension alone, reordered/subset/forged vectors, wrong model/config/executable |
| T16 | Incremental claim: SDK operation, exact activation and full fresh/incremental row equality | Stale hit, mixed generation, no-op change, sentinel delete, mtime-only truth, rebuild called incremental |
| T17 | Closed qualified admission binds source/corpus/suite/license/two annotators/adjudication/assets/host/receipts | Exploratory promotion, duplicate authority, stale or tampered inputs/receipts, undeclared cache |

T15/T16 are `NOT_APPLICABLE` when same-model/incremental claims are false; do not
make them unconditional pair blockers. When enabled, rederive from raw vectors
or before/operation/fresh/incremental full rows plus actual execution/binary/
configuration/collection/source custody. T16 preserves unrelated owners,
requires meaningful typed operations and one final successful build terminal;
reject contradictory/duplicate/later terminals and bool/float scalar aliases.
Summary pass/count JSON plus a digest is not independent evidence. Fault/restart
needs actual fault execution; full-row comparison alone does not prove it.

Returned-window diagnostic is separate from scorer input. Bind its SHA and
record/pack identities, exact task/route inventory, candidate rank/path/span,
status and hybrid lane contributions. Reject extra/duplicate/nonfinite/forged
fields. Distinguish runner boot/readiness, opaque publish/activate, assembly,
corpus reverification and shutdown; do not invent embed/index/seal subphase
timings. Requested floor and actual initial-fetch trace must agree; preserve
current diagnostic/protocol versions and strict required fields.

### Qualification and sample floors

- Distinct claims: `CONTRACT_GREEN`, `SDK_PATH_GREEN`, `PAIR_VALID`,
  `QUALITY_DELTA`, `PERF_QUALIFIED`. Quality/performance require valid contract,
  SDK and pair proof on the same bound source/run. Preserve specific failures.
- `scope=qualified` requires schema-closed W0-B admission (T17), binding license,
  independent gold, source, repository, corpus/suite/pack, model/tokenizer,
  Semble lockfile, host/cache and exact contract/SDK receipts. Recheck before
  capture and from frozen artifacts. Exploratory scope cannot accept admission
  to manufacture qualification; its quality/performance are not applicable.
- Same exact query text; declare internal transformation. Explicit Semble-to-
  canonical path mapping and both-side path/SHA diff digest are mandatory.
  Same file count is insufficient. Keep warm API latency separate from process
  startup; include each product's embedding/indexing cost consistently.
- Quiet same host: CPU/power/cache configuration, no competing builds/benchmarks,
  thermal/frequency checks, capture-time observations and actual reservation.
  No load-average-only or preflight-only performance approval.
- Cold/time-to-searchable: at least five fresh state roots per repo/system,
  alternate product order, prebuild/provision outside timing. Include discovery,
  chunking, embedding, publish/seal/activation and first successful query.
- Warm: at least 20 distinct admitted tasks, exactly one eligible Quanta route,
  at least one warmup pass/root, five fresh roots and 1,000 valid observations
  per eligible route. Each floor is independent. Example: 20 x 10 x 5.
  Both products follow the same driver-issued randomized schedule.
- Raw monotonic query durations must fit declared enclosing windows/tolerance;
  preserve cold/first-query evidence. Unknown duration is null, not zero.
  Missing eligible timing blocks speed. Do not gate on underpowered p99.
- Freshness/update/rename/delete and concurrency are separate scenarios.
  Primary pair is serial warm latency; concurrency capacity needs equivalent
  client/arrival contracts and full offered/completed/error accounting.
- `verdict.json` uses the existing schema-v2 owner, rederived provenance and
  per-comparison digests. Missing and not-applicable T IDs are disjoint. Every
  verified comparison is reported; no empty comparison success. Fresh public
  replay at the promoted path must reproduce canonical identities and states.

## 11. User-owned inputs and explicit exclusions

These are prerequisites/decisions, not coding steps:

- License/attribution approval; two independent annotations and adjudication;
  frozen development/holdout corpus/query packs; exact model/tokenizer assets
  and Semble lockfile; actual quiet host and supported Linux access.
- Historical prospective TOPT admission cannot be recreated. User chooses
  whether the original requirement remains unsatisfied or receives a permanent
  explicit exclusion while retrospective measurements are judged separately.
- Hosted CI billing/access needs a fresh check before assigning current status.
  Release/deployment/activation requires separate authorization and evidence.
- New symbol-text authority, native Windows pair and expanded product support
  are out of scope unless explicitly reopened. Keep typed refusals.

Do not create approvals, gold, quiet-host claims, authentication or process
attestation in code. No external input blocks unrelated implementation closure.

## 12. Verification commands and stop rules

Commands are run from the chosen source root. Replace angle-bracket placeholders
with fresh external paths or actual immutable IDs. Start with affected owners;
do not launch all expensive commands concurrently.

```sh
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_benchctl.py tools/ci/tests/test_benchmark_profile_capture.py tools/ci/tests/test_benchmark_evidence_bridge.py tools/ci/tests/test_bench_protocol_conformance.py -q -p no:cacheprovider
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_criterion_capture.py tools/ci/tests/test_pair_capture.py tools/ci/tests/test_retrieval_capture.py tools/ci/tests/test_lexical_capture.py tools/ci/tests/test_recorded_capture.py tools/ci/tests/test_portable_proof.py -q -p no:cacheprovider
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_benchmark_source_closure.py tools/ci/tests/test_write_verification_receipt.py tools/ci/tests/test_retrieval_contract_proof.py tools/ci/tests/test_retrieval_sdk_proof.py -q -p no:cacheprovider
python3 tools/ci/lint/check-benchmark-policy.py
python3 tools/ci/lint/check-test-authority.py
python3 tools/prompt-manager/pm.py lint
git diff --check
./scripts/cargow --lane test-daemon-lane test -p quanta-index-bench-protocol --locked
just benchmark-prep-local
just retrieval-contract-local
uv run --frozen --extra dev python tools/benchmark/benchctl.py run retrieval-contract --evidence-root <fresh-external-root> --producer-timeout 7200
uv run --frozen --extra dev python tools/benchmark/benchctl.py validate retrieval-contract --evidence-root <same-external-root>
uv run --frozen --extra dev python tools/benchmark/benchctl.py replay <actual-run-id> --evidence-root <same-external-root>
just rust-profile verify-rust
QUANTA_INDEX_TEST_THREADS=1 just rust-profile test-daemon-all
just retrieval-pair <frozen-external-spec>
just retrieval-verdict <repo> <suite> <run-manifest> <fresh-external-verdict>
```

SDK/contract-only proofs can use `just retrieval-sdk-proof <fresh-root>` and
`just retrieval-contract-proof <fresh-root>`; the common profile already
executes both, so do not repeat them for the same closure without cause.
Public SDK changes add `just rust-public-api`; IPC decode/wire changes add
`just rust-fuzz-smoke`; module-boundary changes add `just rust-hexagonal` and
`just rust-cargo-modules`. Mark unneeded gates `NOT_APPLICABLE` with diff reason.

Use the canonical target-root policy. A preserved target directory is valid
only in an explicitly frozen rail with recorded binary/source/build identity;
never share an unqualified target across competing writers. Resolve selectors
and actual case counts before expensive execution. Do not inject ambient
`PYTHONPATH`, `PYTHONHOME`, `PYTEST_ADDOPTS` or `PYTEST_PLUGINS` into proof rails.

Stop the affected rail when source/input/binary/config/host identity changes,
ownership overlaps, a required oracle is absent, selectors execute zero cases,
or cleanup/evidence becomes partial. Keep failed epochs. Finish code and docs
before one serial qualification boundary; never edit a frozen checkout to add
a passing status. Local, installed, hosted, performance and deployment proof
remain separate even when the same receipt serves more than one ticket.
