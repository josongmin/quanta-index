# CS-BENCH-04 — Local comparators, performance and incremental measurement

Status: acceptance **OPEN**. An exploratory external lexical producer is present;
equivalent-work performance and qualified incremental comparison are **NOT_RUN**
in the current remaining-work audit. Category: benchmark measurement. Findings:
F09; gaps: G01/G02.
Depends on BENCH-01/02/03 and shared MISC execution/custody prerequisites.

`live_lexical_external.py` explicitly labels its output diagnostic/unqualified,
retains raw responses and excludes backend indexed-universe attestation,
independent gold and qualified speed. Do not promote that producer's presence to
a completed comparison. Add L2 coverage/ranked-sidecar total update cost to the
measurement scope; index-only bytes cannot establish total update cost.
Actual workload/host admission and final source execution remain required:
[CS-INT-01](CS-INT-01-integration-and-qualification.md#serial-acceptance-boundary).

## Purpose and existing limitation

Keep local Sourcegraph, OpenGrok and cs in the comparison, with Quanta and Semble.
Freeze output unit, result limits/completion and process-spawn/worker/SDK/HTTP
timing boundaries before comparison. A truncated file view of an exhaustive
request is not equivalent to a chunk top-k request; differing timing layers
cannot establish an equal-work engine speedup.

Fixed-snapshot search proves neither watcher operation nor incremental updates.
Measure the real update pipeline separately rather than inferring it from a
successful rebuild or an ingest acknowledgement.

## Comparator profiles

| Product/reference | Role | Required qualification boundary |
| --- | --- | --- |
| Quanta | Lexical, definition and grouped-file policies separately | Actual SDK/daemon profile and source-bound units |
| Sourcegraph local | Developer code-search comparator | Pinned server/indexer images, local endpoint, index readiness and inventory |
| OpenGrok local | Native code-search comparator | Pinned runtime/indexer/config and native field/order semantics |
| cs local | Indexed lexical comparator | Exact binary, flags, indexed universe and CLI lifecycle |
| Semble local | Native hybrid comparator; lexical only if genuinely supported | Pinned lockfile/model/config, actual execution mode and work |
| ripgrep scan | Exact-match reference and scan-cost floor | Explicit manifest/options; not a learned/relevance oracle |

A direct Zoekt profile is optional engine isolation, not another mandatory
Sourcegraph product row. GitHub CLI cannot stand in for Blackbird because its
documented backend differs. Do not substitute generic document BM25 for a native
code-search comparator when claiming developer code-search outcomes.

Pin repository/image/package/binary and runtime identities. Record actual local
endpoint, worker topology, CPU/memory limits, model assets and indexing config.
Use supported readiness/manifest evidence; equal file count is insufficient.
If a product cannot prove part of its universe, label the dependent comparison
diagnostic. Unsupported capabilities remain visible, not zero-point failures.

## Equivalent work and timing boundaries

For matched file search, request ten distinct files with the same query semantics,
scope and timeout. Keep chunk/span tracks separate. If a product must enumerate
all matches to produce those files, disclose that work and compare end-to-end
workflow cost; do not call it equal internal engine work. Native-workflow profiles
may intentionally use different defaults but must retain that label.

Capture:

- Client end-to-end time: request construction boundary through complete decoded
  required output. Include process startup in the CLI workflow row; optionally
  add a resident-worker row when supported, not silently subtract it.
- Time to first result and time to stable completed top-k, with timeout/partial
  semantics. A streaming first result is not completed ranking latency.
- Server phases where available: planning, candidate retrieval, verification,
  ranking/grouping, source/snippet read and serialization. Missing instrumentation
  is unavailable, not zero. Do not subtract clocks across unrelated domains.
- Output bytes/units, candidates examined, verification count, duplicate groups
  and score components where measurable. Diagnostic tracing overhead is explicit.
- Construction wall time, CPU, peak RSS/process tree, final/temporary disk, indexed
  files/bytes/units, and actual parse/chunk/embed/index/publish phases.

Cold process, cold model, cold index/page cache and warm query runs are separate
states. A fresh directory is not proof of a cold OS cache. Cache eviction or host
controls that are unavailable yield a named diagnostic profile, not a cold claim.
Record VM/container overhead and host resource allocation; local does not mean
identical native execution topology.

## Sampling and load

Reuse MISC-02 host/process lifecycle and MISC-03 bounded I/O. Select a resource
envelope appropriate to the host and validate it throughout capture, not just at
startup. Final performance qualification excludes concurrent builds/indexers or
undeclared load. Functional diagnostics may run on a busy host with that label.

Use balanced randomized paired blocks across products and repeated independent
rounds; freeze seed/order and warmup policy. Report distributions, paired deltas,
sample counts and BENCH-03 uncertainty. Predeclare p50/p95 and only qualify p99
with adequate tail samples. Do not rerun until a favorable minimum appears.

For throughput/tails, extend existing registered open-loop/concurrency owners.
Report offered and achieved rate, failures, timeouts and queueing; closed-loop
latency alone can hide overload through coordinated omission. Do not add a second
load generator or claim every current product exposes server-phase timings.

## Incremental and recovery sequence

On a frozen initial release, execute a deterministic mutation workload:

1. Full build and source-bound readiness; establish baseline queries.
2. Add/edit files and query for new bytes/definitions while observing old readers.
3. Rename/move/delete; verify old-path and deleted-definition disappearance.
4. Introduce malformed source; verify lexical text and ENG-02 symbol capability.
5. Repair syntax; verify current symbols replace failed/old facts.
6. Interrupt/restart the relevant process at declared stages and reconcile state.

Bind every event to source hashes and publication identities. Measure mutation
visibility from an observed workspace event only when a watcher actually exists;
otherwise start from explicit ingest and label that narrower boundary. Record
ingest ACK, activation and query-visible milestones separately. Await bounded
observed state, not an arbitrary sleep followed by assumed success.

Measure stale-hit rate, visibility-lag distribution, bytes/files reprocessed per
changed byte/file, CPU/RSS/disk amplification and recovery time. Readers must see
coherent old/new snapshots, never mixed revisions. Products requiring full reindex
report that policy/cost; they are not credited with incremental behavior.

## Owners and DoD

Extend [registry](../../../../tools/benchmark/registry.toml), registered producers,
[Sourcegraph adapter](../../../../tools/benchmark/retrieval/sourcegraph.py),
[lexical capture](../../../../tools/benchmark/lexical_capture.py) and existing
[Justfile](../../../../Justfile) freshness/open-loop/concurrency rails. Live
collection belongs to a registered producer; the lexical diagnostic scorer
remains a replay-only reader. Port needed external one-off collectors into these
owners instead of depending on `/private/tmp` scripts.

- [ ] All required products have pinned local profiles and inventory/readiness
  evidence, or explicit claim-specific blockers/exclusions.
- [ ] Equivalent-work and native-workflow rows cannot be merged accidentally.
- [ ] Warm/cold, transport/engine and build/query timings have explicit boundaries.
- [ ] Resource identity, load, actual index/embedding work and output budgets are
  present; missing telemetry is not filled with zeros.
- [ ] Paired repeated capture replays through BENCH-02 and scores under BENCH-03.
- [ ] Mutation/restart workload proves source visibility and coherent snapshots;
  watcher claims are made only for an exercised watcher path.
- [ ] Performance admission is source/host/input-bound, with sufficient samples
  and predeclared budgets. A functional run alone cannot qualify speed.

User-owned approvals remain external inputs, not tasks to bypass. No product win
or numeric speedup is promised by this RFC. References: [S02/S03/S08/S09](../references.md).
