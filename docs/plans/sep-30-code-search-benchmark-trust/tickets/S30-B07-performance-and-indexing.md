# S30-B07 — equal-boundary performance and indexing measurement

Status: `ACTIVE` (2026-10-04): instrumentation and focused validation in progress;
qualified performance measurement remains `NOT_RUN`. Priority: P1. Depends on B04's
correct, complete capture contract and a quiet admitted host. Parent:
[Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md)
and [MISC-06](../../sep-27-misc/tickets/INDEX.md).

## 2026-10-04 harness cost RCA and next actions

One exploratory attrs pair (one fresh root, debug Quanta binaries, 3.12 driver,
busy macOS host) passed `PAIR_VALID`. The diagnostic driver timer measured
96.43 s total: 54.55 s product envelope, 14.09 s source closure capture,
11.39 s protocol-lock phase containing a second closure verification, and
10.94 s final closure verification. The closure inventories 1,123 files /
53.2 MB; a standalone verification took 9.36 s. This establishes repeated
custody scanning as a local cost, not a qualified speed comparison.

The middle verification was removed. The captured closure digest remains in
the manifest and protocol lock; independent verdict replay and the final full
verification still precede atomic promotion. A second fresh attrs pair passed
`PAIR_VALID`; protocol-lock time fell from 11.39 s to 0.046 s. Total time rose
to 115.12 s because product and final verification times rose under concurrent
load. Do not infer an end-to-end speedup from these two uncontrolled samples.
The stage timer is diagnostic only and is not authority for the verdict.
Receipts: `/private/tmp/a42f990` at source `ab0641b6`, and
`/private/tmp/a727b65` at source `a7e44827`, using the same runner SHA
`29fd369c0e0ddb3e704c3ef62d6f8459a0b146a1c4b8a29cce666154e169c32b`
and searchd SHA
`f324ccd7c578213d4217556c3c31b2a68692d79688ebd2dd330098ed4326af13`.

| Priority | Owner and exact change | Acceptance |
| --- | --- | --- |
| P0 done | `run.py`: phase timer, v7 ingest stage contract in direct capture, one final closure verification, and Python >=3.10 admission before product execution | Focused positive/negative tests; one complete source-bound pair with `PAIR_VALID=pass`; final source-drift refusal retained |
| P1 | C5 quality batch in `holdout_c4.py`, `run.py`, Rust retrieval runner and `semble.py`: run compatible intent packs against one immutable index per repository, with separate sealed packs, records, reports and replay per intent | Same per-intent rows, statuses and judgments as fresh runs; a changed corpus/model/strategy/generation refuses reuse; indexing phase reported once, never charged to individual query latency |
| P1 partial | `source_closure.py` and pair driver now accept an external prior closure for exploratory captures without claims. Reuse checks revision, clean source and file inventory; every cell still fully verifies before promotion. The C5 batch caller is not connected yet | Focused refusal tests and a clean-HEAD source-closure A/B passed. Full pair and C5 batch proof remain open |
| P2 | Profile SymPy verifier under the current source. Optimize repeated parsing only inside one independent validation pass, keyed by source bytes and parser identity; keep verdict re-derivation independent | Report and verdict bytes unchanged; tampered source and parser identity rejected; representative large-cell wall and CPU reported |

Exploratory C5 specs had no qualified admission bundle, so admission was not
the observed bottleneck. Do not remove qualified admission or reuse indexes in
fresh-root speed samples. The old 48-cell ledger had repeated per-repository
Quanta/Semble indexing across four intents, but its summed wall times overlap
concurrent cells and are not an estimate of achievable batch savings.

The four observed C5 intents cannot be concatenated into one evaluator suite.
The explicit OSA1 typo tasks use a different request mode from exact/prefix/
infix; even the three default-mode suites contain cross-intent near-duplicate
queries that the suite validator correctly rejects. Batch execution must keep
each original suite and blind pack independently valid. A multi-pack product
session may share the immutable index, but must emit one native, pack-bound
record per intent and an explicit shared-index receipt; derived synthetic
per-intent native records are not acceptable evidence.

At clean `934eb012`, a single local closure-only A/B over 1,124 files measured
`capture` 5.744 s, `reuse` 0.377 s, and the mandatory final `verify` 5.495 s.
The first and reused payloads/digests were identical. The output is under
`/private/tmp/qi-closure-reuse-mpjzrq0j`. These are diagnostic single-run
times; they do not show whole-pair savings or batch product-index reuse.
`test_benchmark_source_closure.py` passed 69/69 and the selected pair-driver
tests passed 6/6 at this HEAD. The remaining high-cost work is one native
product index session per repository with separate pack-bound executions.

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
checks, not whole-repository or performance qualification. The public API
baseline and `just rust-public-api` passed. The real
`sdk_roundtrip::real_daemon_roundtrip_publishes_and_queries` passed 1/1 against
the same-lane daemon; it crosses publication, activation, query and the nested
stage response boundary. New scoped scale/open-loop tests and actual medium
execution are still pending shared resource admission.

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
from the outer partition. The canonical Python inventory now collects 673
identities; collection is not execution. Ruff and `git diff --check` passed.

### Concrete remediation and stopping rules

Code ownership remains three parallel workers plus one integration owner.
Ordinary search and typo changes share one `searcher/code_search.rs` owner;
common contracts and capture readers have one integration owner. Performance
runs are sequential; a cold release compilation is not corpus indexing time.

| Work item | Existing files and smallest change | Independent proof and stop condition |
| --- | --- | --- |
| Ordinary search | `crates/quanta-index-lexical/src/searcher/code_search.rs`, core lexical outbound stats, plane `query_dispatcher/routes/lexical.rs` and response budget: use the implemented counters/clocks before changing candidate intersection, row creation, sort/page or preview | Byte-scan/full-sort goldens, last-candidate winner, ties, all cursor pages, exact totals, budget/cancel and response truncation. Optimize the repeatedly dominant stage only; stop on changed identity, order, score, span or status |
| Typo search | Same lexical owner: compare ordinary miss plus fallback with explicit OSA1 on the same typo input. Existing exact-first behavior and bounded source admission remain authoritative | Independent exhaustive OSA1 insertion/deletion/substitution/transposition oracle, short names, Unicode/case, path filters and no-answer; generation/digest faults refuse. A candidate filter must not drop any valid oracle match. Report file recovery and declaration recovery separately |
| Indexing | `adapter_ingest.rs`, `writer_cache.rs`, `file_authority.rs`, `sealed_generation/seal.rs`, SDK `lexical.rs` and the registered retrieval runner: attribute full/delta stages already emitted, then change only the dominant stage | Fresh rebuild versus update/delete equality, untouched source-repository survival, replay/restart and interrupted seal behavior; preserve fsync/directory sync and immutable generation identity. Existing hard links, touched shards and commitment reuse are verified before proposing new caches |
| Scale/load | Existing harness `scale.rs`, `harness.rs`, `artifact.rs`, `open_loop.rs` and their binaries: typed `(source_repo_id, relative_path)` fixtures, source/IPC preflight, nonzero finite offered work and every measured response verified after its timer | Same path in two repositories plus one-repository delta, fixed framed digest, missing/duplicate/foreign identity rejection; capacity refusal records its source/stage instead of a zero latency. First execute medium 4 x 64 = 256 files, then large 16 x 256 = 4,096, then XL 64 x 512 = 32,768 without shrinking inputs or raising product limits |
| SDK/IPC | Existing SDK and IPC owners, conditional on measured complete-call cost beyond server search | Attribute request construction, active resolution, serialization and transport first. Any connection or resolve/search change preserves generation pin, activation races, deadlines and request identity. SDK minus server time is a residual, not an IPC attribution |
| Five-product integration | `live_lexical_external.py`, its replay validator, `lexical_file_comparison.py`, existing tests and schemas: product-scoped capture/replay within the existing collector, followed by a checked join on input/source/timer contracts | A missing product/row, changed pack, duplicate task, backend drift or failed probe cannot produce a complete comparison. Each product retains native raw rows and its before/after scope checks; sequential collection must not claim a simultaneous paired speed experiment |

The scoped scale tiers describe multiple source repositories under **one**
serving owner. They do not prove multi-owner publish/CAS concurrency. Seeded
synthetic scale and real multi-repository retrieval are separate workloads.

Current external preparation binds exact 1,196 and typo insertion 1,192,
deletion 1,178, substitution 1,192 and transposition 1,192 tasks to Gin's same
99 files. These are query expansions, not source-corpus expansion. Prefix,
infix, components and no-answer retain their independently admitted suites;
do not manufacture 1,000 independent tasks from insufficient unique sources.

A fresh external attempt failed before query execution on an OpenGrok HTTP
401 probe. Read-only inspection also found the current C5 services contain a
12-repository holdout rather than Gin. Preserve that attempt and existing
services. Prepare new isolated Gin roots and validate served source/revision.
The Docker VM has about 8.2 GB total memory with existing comparator processes
using about 4.4 GB; do not start both additional servers concurrently merely
to satisfy the collector's current interleaved loop. Product-scoped collection
is a prerequisite for sequential isolated services.

The standalone external CLI import failure was reproduced from an external
working directory and repaired in the existing CLI bootstrap. The subprocess
test removes `PYTHONPATH` and invokes the actual script; its focused run passed
4 tests. Current diagnostic-v7 on/off comparison, native chunk/file projection,
nested indexing replay and independent host-binding fault cases also passed
focused checks. These do not qualify host performance or new product rankings.

For performance acceptance, freeze the source/binary/input tuple, finish warmup,
use the existing five-fresh-root and 1,000-warm-observation floors, randomize
paired order and report uncertainty. A quality/identity difference, unknown host
observation, competing build or inconclusive repeated improvement prevents an
optimization from being promoted as a performance win. Numeric product SLOs
remain an explicit operational decision rather than an invented test threshold.

### Joined external capture implementation update

`lexical_file_comparison.py` now has an `--external-spec` entry point within the
existing scorer. The closed spec contains `schema_version: 1`, `native_inputs`
(the existing native input roles without the three external row roles), and
`external_captures` (exactly `sourcegraph`, `opengrok`, `cs` mapped to retained
capture roots). A legacy complete root may be shared by all three products;
product-scoped roots must declare exactly the products assigned to that root.
Every root is independently replayed before and after the existing scorer.
Native/external suite and pack bytes, release binding and complete product
coverage must agree. The report retains capture hashes and explicitly limits
the result to descriptive independent observations, not a paired speed claim.
Latency summaries now include an observed-call `sum_ms`; a non-finite total is
rejected rather than emitted as a valid statistic.

Focused join guards passed 14 tests. The full scorer and common capture tests
(`test_lexical_file_comparison.py`, `test_lexical_capture.py`) passed 82 tests in
121.70 seconds. The product-scoped collector's complete owner-local test file
passed 78/78 in 179.57 seconds, including actual HTTP/process capture, replay
and a joined legacy capture. Selected-product inventories, raw mutations and
changed summary bindings are rejected. The join additionally checks that rows
consumed by scoring match the independently replayed row digests; its latest
join/latency-focused check passed 16 tests in 1.96 seconds. These fixtures do
not prove a new five-product capture.

The isolated Gin Sourcegraph projection's native path inventory and stored
source verification both matched 99/99 files, with unchanged before/after index
bytes. Its owned Sourcegraph/src containers were stopped and their data retained
before collector changes; existing C3/C5 services were not modified. Evidence
is outside the checkout under
`/private/tmp/qi-five-product-gin-services-20261004-6ZY8VW/`.

### Fresh execution update (2026-10-04)

The release search daemon and retrieval runner were built from clean
`e5701efda6eb8a6c8efe51af97b25812a8e12503` in 44 minutes 16 seconds. This is
cold compilation time, not corpus indexing time. The subsequent clean Python
driver is `e1cc8a8471ace43fea67a127c4d978ff9a73db60`; its source revision is
not evidence of the binaries' build source. Binary digests and the distinct
source identities are retained outside captures. Existing `source_sha` fields
bind the driver closure. Their description must not promote that closure to a
Rust binary/source attestation.

The exact lane's fresh pair `/private/tmp/qf4a` passed capture and independent
verdict replay: selected/executed/passed 2,392/2,392/2,392, failed 0. Its
1,196 ordinary requests measured aggregate candidate work of 132.33 ms,
selected preview work of 119.97 ms and sort/page work of 7.81 ms. These are
contended-host diagnostic observations. The earlier single `writeContentType`
sample cannot establish a general candidate bottleneck: across this lane,
preview and candidate costs are comparable, while sorting is small. Do not
implement a bounded-sort optimization based on these observations alone.
The complete SDK call minus server search remains an unattributed residual;
it is not a measured IPC cost.

The clean pair's one fresh Gin 99-file build has a lexical build observation of
7.038 s inside an 8.995 s SDK publish observation; SDK activation is a separate
0.410 s observation. The five non-overlapping lexical child stages are text
authority 2.510 s, preparation 1.814 s, file authority 1.436 s, writer
mutation 0.664 s, and seal 0.299 s. Their sum is 6.722 s; 0.315 s remains in
the outer lexical build clock. Seal's writer commit 0.016 s, merge wait 0.001 s,
and commitment 0.219 s are nested measurements, not extra elapsed time. These
are one contended-host diagnostic sample, not a stable profile or speedup.
Source: `/private/tmp/qf4a/rep-00/quanta/strategy-00-fw_strict/retrieval-diagnostic.json`
`observation.lexical_build_ns` and `observation.lexical_stages`, plus the same
cell's `phase-metrics.json` `phases_ms.sdk_publish` and `sdk_activate`.

The code boundary determines what can be optimized: `adapter_ingest.rs::build_batch`
times admission, coverage and base preparation before mutation; this fresh
generation has no base clone. `adapter_open.rs::commit_ops_under_lock` includes
file-delta planning and source digest/trigram admission inside **writer mutation**,
then runs the text-authority write and `file_authority::apply_plan` sequentially
under the generation writer lock. The text-authority stage includes a committed
Tantivy document scan and shard rebuild; the file-authority stage covers durable
source/manifest writes and obsolete-file cleanup, not the source trigram build.
The latter's serving index is materialized at open time. Existing text-authority
touched-shard updates and delta base carry-forward must be reused, not rebuilt.

Next indexing decision: repeat fresh-root and one-file-delta/no-op samples on
an admitted quiet host with the same pinned source, binaries and corpus. If text
authority remains dominant, split its scan, shard construction and durable
publish timings within `text_docs.rs`/`text_authority/writer.rs`; consider a
small change only after one component dominates repeated samples. If file
authority remains dominant, split source writes, manifest fsync and cleanup in
`file_authority.rs`; retain `index_store.rs::write_atomic_durable` and the
seal/identity durability order. If preparation remains dominant, split coverage
admission, staged coverage and base preparation in `adapter_ingest.rs` before
changing it. A candidate change must preserve the independently expected
file/text authority content, sealed identity and query results for a fixed
fresh corpus, a one-file delta and a no-op; corrupt source bytes or a mismatched
base identity must still be rejected. Compare bytes only where the canonical
format and generation identity require byte determinism.
Writer and seal changes need their own repeated evidence; no engine optimization
is justified by this single sample. Repeated indexing, substage attribution,
and incremental correctness/performance qualification are `NOT_RUN`.

Repeated typo attribution, capacity runs and equal-boundary five-product
comparison remain separate completion scopes.
Five diagnostic query lanes contain 1,196/1,192/1,178/1,192/1,192 tasks, or
5,950 total, over the unchanged Gin 99-file corpus. OpenGrok captured and
replayed every lane under clean external driver
`45dd36a492b882000f7063aa52fd583427808c05`; Sourcegraph and the native pair's
remaining lanes are running. cs is prepared but not executed. Native index
evidence proves Sourcegraph's 99 stored bodies and OpenGrok's served 99 bodies;
OpenGrok backend index attestation remains unavailable. Its zero typo hits are
successful empty HTTP results, not execution errors. Exact versus typo scores
and file versus declaration recovery must stay separate.

Actual medium execution at `06aac8cc` failed after publication because the
harness expected a first-query cold-open increment. Activation already proves,
opens and promotes the generation, so zero additional cold opens is valid.
The corrected harness represents absent cold-open/GC observations as missing,
not zero, and retains the first-route count check; `scale::` passed 19 tests.
The fresh medium rerun, large and XL executions are still pending. Another
thread's live release build holds the canonical build/test admission lock;
it was not killed or bypassed. Logical directory growth is not physical write
I/O, and harness-plus-daemon-thread RSS is not daemon-only RSS. CPU, explicit
reopen/restart and delete performance are not yet proven.

Evidence roots: `/private/tmp/qi-code-search-cost-20261004-guAWNESy/`,
`/private/tmp/qi-code-search-full-pair-20261004-ZQtbie/`,
`/private/tmp/qi-five-product-scoped-20261004-5ke6oh_h/` and the preserved
medium failure `/private/tmp/qi-scale-medium-20261004-06aac8cc-r1/refusal.json`.
No optimization speedup or qualified performance ranking is established.

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
