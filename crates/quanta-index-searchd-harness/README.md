# Daemon test harness

Shared runtime E2E fixtures and benchmark drivers. The runtime consumes this
crate as a dev dependency; it is not part of the production daemon graph.

Start with [harness](src/harness.rs), [scenario authority](src/scenarios.rs),
and [benchmark support](src/bench_support.rs). Test and benchmark evidence
boundaries are in the [JUN-08-001 ADR](../../docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md).
