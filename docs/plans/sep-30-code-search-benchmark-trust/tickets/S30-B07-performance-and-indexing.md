# S30-B07 — equal-boundary performance and indexing measurement

Status: `ACTIVE` (2026-10-04): instrumentation and focused validation in progress;
qualified performance measurement remains `NOT_RUN`. Priority: P1. Depends on B04's
correct, complete capture contract and a quiet admitted host. Parent:
[Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md)
and [MISC-06](../../sep-27-misc/tickets/INDEX.md).

## Work and boundaries

Measure correctness before time. A Quanta SDK/IPC request and a Semble
in-process BM25 function call are different timers; report both as different
profiles, never as one speed ratio. Build the pinned Quanta release profile
required by the selected deployment/benchmark recipe, and bind the actual
binary SHA, Semble package/model assets, compiler options and product versions.

For the full admitted 1,196-query exact-name workload and any newly admitted
lanes, record the following. The 20-query Semble set is too small for a stable
p95 comparison and remains descriptive:

- Client request construction through **complete decoded required output**,
  including transport/process startup where that is the actual product flow.
  Report server-internal stages only where instrumented; absent telemetry is
  `unavailable`, not zero. Time-to-first-result and completed top-k are distinct.
- Per-query p50/p95, distributions, completed/error/timeout/partial counts,
  output bytes and native units. Repeat randomized paired blocks with a frozen
  seed and warmup policy; record host load and resource envelope throughout.
- Fresh-root index construction, parse/chunk/embed/index/publish/activate phases,
  source files/bytes, indexed units, CPU, peak process-tree RSS and disk. Quanta
  and Semble may have different chunk counts and operations; report the
  boundaries rather than a bare indexing-speed multiple.
- Cold process/model/page-cache and warm query states separately. A fresh
  directory alone does not prove a cold OS cache.

Use the same task set and user-visible output unit when making a matched
workflow performance claim. Native-product workflows may differ, but their
requests, work and result limits must be shown. Sourcegraph/OpenGrok remote or
unattested indexes cannot silently share local index-build denominators.

## Verification and deliverable

- Query order/status and required output are equivalent within the chosen
  comparison mode before timing analysis. No survivor-only latency after
  dropping failed requests.
- Timing source, clock boundary, warm/cold state, repetition count, compiler
  profile, model assets, indexed source and product topology are in the raw
  receipt. A busy host or concurrent build makes the performance claim
  `NOT_RUN`/diagnostic rather than `PERF_QUALIFIED`.
- Publish a performance table separate from B05 quality, including every
  product's actual query and indexing boundary. Existing gin release timings
  remain diagnostics and are not reused as equal-work proof.

Extend the current registered capture and resource owners only for an observed
missing boundary; do not add a second benchmark harness.

## 2026-10-04 parallel execution plan

This section supersedes speculative optimization proposals. Instrumentation is
implemented work; reduced latency is a separate claim requiring measurements.
Use one frozen baseline and a distinct external output root for each experiment.

| Owner | Existing implementation boundary | Next action | Acceptance |
| --- | --- | --- | --- |
| Benchmark/integration | `tools/benchmark/retrieval/run.py`, canonical `tools/benchmark/host_monitor.py`, required test inventory | Finish positive/negative monitor replay, phase contracts and public daemon roundtrip; admit a quiet host for repeated captures | Complete required output and every measured response checked; missing observations and zero-request schedules cannot qualify; canonical raw transcript and phase bytes bound |
| Ordinary and typo search | `crates/quanta-index-lexical/src/searcher/code_search.rs`, core lexical outbound stats, plane lexical route and response budget | Attribute posting probes, source verification, OSA comparisons, row creation, sorting and preview costs by execution mode | Independent byte/OSA oracle; result IDs, scores, order, spans, exact count, cursor, budget/cancel and status preserved |
| Indexing | lexical ingest/writer/authority/seal, SDK lexical publication and retrieval runner | Measure full and delta builds with the existing stage inclusion tree; optimize only the repeatedly dominant stage | Fresh versus delta update/delete equality; immutable identity/digest, replay/restart and durability preserved; no double-counted child durations |
| Scale and load | registered harness `scale.rs`, `tail.rs`, `open_loop.rs` and their existing binaries | Extend actual runners beyond small tier using typed source repository identity; reuse existing arrival scheduler | File/byte/digest and per-repository identity oracle; every timed response validated after its timer; nonzero offered work; repeatable capacity and refusal evidence |

### Search decisions after attribution

- Posting/source verification dominates: improve lossless candidate intersection
  or eliminate repeated verification. Keep independent exhaustive byte-scan
  fixtures, source spans and case semantics.
- Sorting/row creation dominates: delay row materialization and assess bounded
  top-k selection. Full match verification required by exact totals/cursors
  still runs; finding the first ten matches is not a stopping rule.
- Preview dominates: reduce copies/normalization for selected rows without
  changing required output bytes or source span.
- Typo comparisons dominate: assess generation-bound token/posting reuse or
  conservative candidate filtering against an independent exhaustive OSA1
  oracle. Preserve exact-first policy, Unicode/case, no-answer and admission.
- SDK/IPC overhead dominates: attribute transport, serialization and active
  resolution first, then assess connection reuse or resolve/search integration
  with activation-race, generation-pin and deadline tests.

Automatic fallback currently includes both the failed ordinary pass and OSA
work in its candidate clock. Split those subspans only if that aggregate is the
measured hotspot. Source-surface bytes are a work proxy, not measured disk I/O.
Disabled stage observation still executes backend clock reads, so enabled versus
disabled captures do not measure all instrumentation overhead.

### Indexing decisions after attribution

Existing delta generation files are hard-linked; text authority already uses
touched shards, file authority writes missing digests and sealing reuses base
commitments. Validate these counters rather than implementing parallel reuse.

The inclusion tree is runner total -> discovery/preflight/chunk/daemon boot/
publish envelope/query. SDK publish and activate are children of that envelope;
server lexical build is inside publish. Lexical stages partition preparation,
writer mutation, text authority, file authority and seal. Writer commit, merge
wait and commitment are children of seal; file admission is inside commitment.
Display unmeasured residuals instead of summing nested durations twice.

If writer/commit/merge dominates, compare bounded writer/segment policies. If
authority/admission dominates, inspect changed-shard reuse, digest writes and
remaining normalization. Preserve seal validation, fsync and directory sync.
The current Quanta phase-v3 `daemon_boot_and_readiness` field describes the
actual `DaemonSession::boot` envelope; historic v1/v2 `model_provider_prepare`
must not be interpreted as an isolated model preparation measurement.

### Scale, workload and comparison schedule

1. Prove scoped fixture rows `(source_repo_id, relative_path, bytes)` and digest
   inventory, including equal relative paths in two distinct repositories.
2. Run 256, then 4,096, then 32,768 files with per-source-repository planted
   tokens and identity checks. Distinct source repositories under one serving
   owner are explicitly different from independent owner generations; the
   latter need SDK publish/CAS and per-owner generation pins.
3. Measure fresh build, one-file update/delete, activation, reopen, warm query,
   CPU, peak process-tree RSS and index bytes. Large tier constants alone are
   not executed scale evidence. Existing scale and open-loop binaries currently
   execute only the small fixture.
4. Keep closed-loop request latency separate from scheduled-arrival open-loop
   throughput, queue latency, timeouts and refusal rates. Reuse the scheduler.
5. Preserve current 8 MiB/file, 128 MiB/generation and posting/transport limits;
   report the exact capacity refusal rather than increasing limits to pass.
6. Re-run all five products using the admitted exact 1,196 tasks and separately
   admitted prefix/infix/components/typo/no-answer lanes. Report native units,
   timer boundaries, corpus binding and external-index uncertainty per product.

Implementation and owner-local tests can run in parallel. Resource-heavy Rust
rails use admitted lanes; performance captures run sequentially on one host.
Apply existing five-fresh-root and route-local warm-observation floors with
paired blocks and uncertainty reporting. Do not invent a numerical performance
SLO, improvement factor or product ranking before obtaining these observations.

### Validation state

Search worker reports lexical `l3_exact_source` 28/28 and two focused plane
tests passing. Indexing worker reports contract 171/171 and SDK 120/120 unit
tests passing, plus lexical test-target compilation. These are owner-local
checks, not whole-repository or performance qualification. Harness executable
tests and public daemon boundary checks are still being scheduled through the
shared resource admission queue.

Fresh stage attribution, optimization A/B, actual medium/large/XL execution,
quiet-host performance qualification and fresh five-product comparison remain
`NOT_RUN`. No performance improvement has been established by instrumentation.

Integration focused check: `uv run --frozen --extra dev python -m pytest
tools/ci/tests/test_retrieval_benchmark.py -q -k
'host_timeline_replays_complete_bound_monitor or
protocol_phase_metrics_bind_raw_warm_counts_and_cold_separately'` passed 2 tests
in 13.42 seconds. The new positive monitor replay first exposed a digest-prefix
mismatch; the corrected reader admits the valid canonical transcript and
rejects a changed reservation. The phase golden accepts the actual v3 daemon
boot label, rejects the misleading old label, and excludes nested SDK timings
from the outer partition. The canonical Python inventory collects 664
identities; collection is not execution. Ruff and `git diff --check` passed.

## Execution receipt (2026-09-30)

`NOT_RUN`: host not quiet (load ~26 on 16 cores, concurrent builds). Producer `quanta-index@0d21914e` (clean worktree);
results, digests and residuals: [qi-s30-bench-trust-20260930-0d21914e/RESULTS.md](/Users/songmin/Documents/code-new/qi-s30-bench-trust-20260930-0d21914e/RESULTS.md).

Reconfirmed `NOT_RUN` 2026-10-01 (v2): host load 25–29 and the data volume ran out of space.

The 2026-10-01 source-bound 1,196-query correctness follow-up recorded one
unqualified timing sample per new file mode. Semble `lexical-file` indexed
99 files/1,171 chunks in 444.8 ms and summed 1,196 worker query calls to
576.5 ms; Quanta's debug-runner `keyword_file` summed SDK query calls to
5,765.4 ms. These modes collect different candidate depths and include
different process boundaries, with no controlled warmup, repeated roots or
quiet-host admission. They are diagnostic phase observations; B07 remains
`NOT_RUN` for its equal-boundary performance protocol. Raw timing fields are
in the respective [`Semble`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/semble/adapter-run/phase-metrics.json)
and [`Quanta`](/Users/songmin/Documents/code-new/qi-s30-file-modes-20261001-ExtGj6hb/quanta/metrics.json) artifacts.

2026-10-01 preflight at clean `112c6c7e`: `NOT_RUN` again. The 16-core host
had load averages 14.78–19.00, above the local timing admission ceiling of 8,
with concurrent Rust builds and 53–54 GiB free. Darwin `host-probe` reported
CPU frequency `unavailable`; current performance admission requires observed
`stable` or `bounded` frequency, so this host cannot produce a qualified B07
result even after load settles. Quanta's SDK `.execute()` and Semble's worker
BM25 dispatch timers exclude different work. A same-boundary claim requires
an admitted host, current-head release binaries, complete-output timer
instrumentation and the full 1,196-task repeated protocol. The new 99-query
NOC run is a correctness diagnostic, not B07 performance evidence.

2026-10-02 current-source audit at `53ca51e3` with the existing shared dirty
overlay: `NOT_RUN` for B07 timing/index/update qualification. Read-only
`python3 tools/benchmark/retrieval/run.py host-probe` observed 16 cores,
concurrent Cargo/Rust compiler processes and CPU frequency `unavailable`;
`uptime` observed load averages 10.21/14.04/15.18. Free disk was 97 GiB.

The audit confirmed that the original SDK response timer and Semble library
dispatch timer measured different work. The current producers now implement one
query boundary: `request_construction_to_normalized_response`, with
`capture_relative_monotonic_ns` observations. Quanta constructs its route
request inside the clock, performs SDK/IPC execution and decode, proves the
normalized row and status, and serializes the required response. Semble uses
one resident worker: its parent starts before constructing the request,
receives and decodes the worker's native response, runs the same canonical row
normalizer, resolves status and serializes the required response before ending
the parent clock. Neither producer includes timing telemetry in required
response bytes. Per-query duration keeps the existing `query_latency_ms` field;
no library-duration/completed-duration twin is emitted.

Both producers cache the completed first measured row for final record
assembly. Source/span normalization is performed once per actual request;
the batch provenance envelope is assembled after the per-query clocks end.
Startup, model preparation and index construction precede the resident query
boundary and retain separate phase accounting. This is a resident benchmark
workflow, not a CLI startup or cold OS-cache claim. Worker phase timestamps and
parent query timestamps are never subtracted across process clock domains.

`PERF_QUALIFIED` now requires matching canonical boundaries and output units,
complete cold/warmup/measured schedules, serial monotonic observations,
nonempty required output, completed statuses and exact own-clock sample
durations. Semble observations and samples must agree between its native and
phase artifacts. Missing, dispatched-only, partial or mismatched evidence
cannot qualify; there is no unconditional performance-disable branch.

Every pair run manifest now requires `artifacts.phase_metrics_digests`, an
exact map from its manifest-relative `phase_metrics` paths to captured-byte
SHA256 digests. Replay checks the byte binding before phase validation for
both exploratory and qualified scopes. Missing, extra, duplicate or malformed
bindings are rejected; even a whitespace-only phase-file mutation invalidates
the pair. This closes the Semble phase hash gap without a second manifest
schema or a weaker performance gate.

Focused verification: `./scripts/cargow test -p quanta-index-retrieval-bench --lib record::tests`
passed 17 tests; `./scripts/cargow check -p quanta-index-retrieval-bench --bin quanta-index-retrieval-bench`
passed. `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_completed_response_timing.py tools/ci/tests/test_retrieval_benchmark.py -k 'completed or worker_template or normalize_record or verdict_perf_frontier_and_gates or qualified_speed_replay_rejects_unalternated_system_order or darwin_thermal_limits_and_frequency_fail_closed'`
passed 17 tests (408 deselected), including paired positive/negative replay,
real worker normalization under the parent clock and partial-line deadline
handling. Ruff passed. These focused checks establish instrumentation and gate
behavior. The full admitted repeated workload, fresh index construction and
incremental-update costs remain `NOT_RUN` on this contended host.

Phase-byte binding verification: `uv run --frozen --extra dev python -m pytest -q tools/ci/tests/test_completed_response_timing.py tools/ci/tests/test_retrieval_benchmark.py -k 'completed or phase_digest or manifest or qualified_speed or verdict_perf'`
passed 36 tests (398 deselected). It covers canonical map inventory, paired
positive replay, both products' byte-only tampering in exploratory/qualified
scopes and the existing manifest/performance gates. Ruff and
`git diff --check` passed. No additional Rust source changes were needed for
this manifest binding.
