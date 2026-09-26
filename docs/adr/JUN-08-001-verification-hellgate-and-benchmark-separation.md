# JUN-08-001 — Verification Hellgate and Benchmark Separation

Status: `Accepted`

Decided: 2026-06-08

Consolidated: 2026-09-27

Source programs: Jun-2 DSL benchmarking and Jun-7 verification hellgates

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

## Consequences

- Fast green does not imply broad, cross-repository or performance green.
- A red external boundary does not invalidate the existence of the gate, but
  it blocks the corresponding qualification claim.
- Historical component results are evidence for their recorded snapshot only;
  current status belongs in fresh receipts.

## Historical record

The hellgate implementation packet is indexed in
[the completed-plan archive](../plans/ARCHIVE-INDEX.md). The detailed adopted
measurement contract remains in
[the DSL benchmarking RFC](../plans/jun-2-dsl-hardening/RFC-DSL-Benchmarking.md).
