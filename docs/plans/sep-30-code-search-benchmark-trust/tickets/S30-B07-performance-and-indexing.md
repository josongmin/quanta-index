# S30-B07 — equal-boundary performance and indexing measurement

Status: `NOT_RUN` (2026-09-30); see receipt below. Priority: P1. Depends on B04's
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
