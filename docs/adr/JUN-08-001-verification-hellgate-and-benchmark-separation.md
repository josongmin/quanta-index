# JUN-08-001 — Verification Hellgate and Benchmark Separation

Status: `Accepted`

Decided: 2026-06-08

Consolidated: 2026-09-27

Source programs: Jun-2 DSL benchmarking, Jun-7 verification hellgates and search product quality

## Context

DSL correctness, daemon lifecycle, cross-repository ingress and latency were
previously concentrated in broad suites. That made failures slow to localize
and encouraged performance numbers to be inferred from correctness or chaos
runs.

## Decision

### Gate layers

The verification surface is split into independent gates:

- scenario truth: validates the shared executable scenario inventory;
- fast correctness: text and structural route hellgates plus capability guards;
- broad lifecycle: daemon, restart, replay, full-corpus and chaos behavior;
- cross-repository ingress: explicit external producer/runtime boundary proof;
- performance: warm and cold benchmark capture plus baseline comparison;
- aggregate: invokes the required components but does not replace their
  individual artifacts or failure identities.

The stable command front doors are the corresponding `rust-bench-dsl-truth`,
`rust-verify-hellgate-*` and `rust-bench-dsl-*` Just recipes. Command presence
does not mean the gate passed on the current revision.

### Scenario authority

Correctness and benchmark producers consume an explicit scenario authority
containing scenario ID, route family, query, syntax, fixture, expected result
shape and latency class. A benchmark must not path-include test-only scenario
definitions or maintain an unreviewed duplicate inventory.

### Performance separation

Performance has three non-interchangeable layers:

1. compile timing against a matching build baseline;
2. pure tokenize/parse/normalize/hash pipeline cost;
3. query latency, with warm steady-state and cold first-query measured by
   separate harnesses.

Correctness, boundedness and chaos elapsed time are not latency benchmarks.
Cold and warm samples, route families and native/Sourcegraph syntax remain
separate. Cross-syntax comparison is allowed only where semantic parity is
already proven.

Baseline admission requires the declared source, host, configuration, sample
floor and complete artifacts. A stale or unattributed baseline cannot be
silently migrated into a current gate.

The DSL latency gate uses the bench-owned scenario authority in
`crates/quanta-index-searchd-harness/src/scenarios.rs`. The dedicated
`dsl_warm_matrix` runner is warm gate authority; the Criterion view is
diagnostic. Cold samples come from a fresh process per query. Warm and cold
authority captures run serially. The current comparator blocks on both p50
and p95 regressions; p99 is advisory. Exact thresholds, sample floors,
artifact schema and baseline admission follow the contract below, implemented
by `tools/benchmark/compare_dsl_bench.py`, `benchctl.py` and the native artifact
writer. The benchmark README contains operator usage only. Scan-vs-index is a separate exploratory scaling experiment,
not a DSL latency gate or semantic comparison against a text-only engine.

### Native artifacts and regression admission

`BenchArtifactV1` schema 2 is emitted by the native artifact writer in
`crates/quanta-index-searchd-harness/src/artifact.rs`. It binds a full clean Git
revision, framed corpus/config digests, exercised model revision, host identity,
phase/resource scope and complete scenario rows. A null unmeasured phase or
latency never becomes zero. Peak RSS is harness-self `getrusage`, not process-tree
RSS. Semantic route names remain separate. Scenario shape/count/typed-error
truth is checked before publishing timing rows; early-stop rows cannot enter a
baseline.

Baseline/current mode, corpus/config/model/host and scenario identities must
match. Current source must match the checkout. Missing/new scenarios refuse;
stale schema-1 artifacts require recapture. Warm rows need at least 200 measured
samples; cold rows need 20. A scenario regresses only when both relative and
absolute growth exceed the threshold:

| Mode | Metric | Relative | Absolute |
| --- | --- | --- | --- |
| warm | p50 | >10% | >1 ms |
| cold | p50 | >10% | >5 ms |
| warm | p95 | >20% | >5 ms |
| cold | p95 | >20% | >10 ms |

p99 is advisory. Explicit comparator overrides bind the comparison configuration.
Guarded admission captures both warm/cold artifacts on one clean canonical Linux
host and validates complete measured rows before publishing the pair. Standalone
baseline update is refused. A durable pending marker blocks reads after crash or
failed rollback until the owner reconciles both baseline files. Baseline candidates
require review before becoming committed authority.

### Dimension-specific quality producers

The former J7Q-00/05/06/07/08 implementation plans are superseded by current
producer/contract owners. `registry.toml` registers the quality families;
`benchctl run quality-full` routes the producers and validates required artifacts
before the integration summary. Commands are not an alternate registration list.

| Dimension / owner | Implemented boundary |
| --- | --- |
| Relevance / `harness/src/relevance` | Judged seeded per-route MRR/NDCG/recall and rank/hard-negative invariants; external Sourcegraph overlap remains explicitly unprovisioned |
| Snippet / `snippet.rs`, `ui.rs` | Grade emitted windows/highlights and planner/engine/contribution fields against fixed probes; do not post-process a bad result to make the oracle pass |
| Scale / `scale.rs` | Seeded tier manifests and small-tier execution; medium/large/XL declarations are advisory until measured |
| Tail / `tail.rs` | Golden behavior before representative route timing; local budgets advisory, DSL baseline rules above remain separate |
| Operator / `searchctl`, `ops.rs` | JSON doctor/readiness/generation status/explanation/metrics and typed remote refusals; harness runtime-surface snapshots are supplements, not actual CLI execution |
| Repair / typed error DTO, `ambiguity.rs` | Supported alternatives, route hints and docs anchors remain advisory metadata on typed failure; internal invariant errors carry no fabricated repair; classes and wire round trips stay distinct |
| Consumer / result DTO, `ui.rs` | Typed preview/highlight/provenance and explanation sections; no parsing free-form summaries to infer search semantics; current exact preview contract is SEP-27-003-owned |

Each dimension keeps its own native artifact and denominator. Required missing,
invalid or failed dimensions cannot publish a passing aggregate. Context/span,
file recall, relevance, latency, capacity and operator-contract proof are not
interchangeable. Hash fixtures and declared sample/budget settings do not qualify
learned relevance, large-corpus capacity, quiet-host speed or downstream UI.
External comparative claims need independent judged gold and the actual overlap
subset; learned ranking or new consumer semantics require their own decision.

Open acceptance stays in the
[quality residual index](../plans/jun-7-search-product-quality/tickets-wave2/INDEX.md),
not completed implementation tickets. The gate names/output paths remain
registry/code-owned; operator usage stays in benchmark and CLI READMEs.

## Consequences

- Fast green does not imply broad, cross-repository or performance green.
- A red external boundary does not invalidate the existence of the gate, but
  it blocks the corresponding qualification claim.
- Historical component results are evidence for their recorded snapshot only;
  current status belongs in fresh receipts.

## Historical record

The hellgate implementation packet and superseded Jun-2 measurement RFC are
indexed in [the completed-plan archive](../ARCHIVE-INDEX.md#historical-record-recovery). The RFC's
old p95-advisory and proposed file-path instructions are not current policy.
