# Daemon test harness

Shared Native runtime fixtures, benchmark drivers, and pinned external old-Q
reference support. The runtime consumes this crate as a dev dependency; it is
not part of the production daemon graph.

Start with [Native scale](src/scale/native_index.rs), [scenario authority](src/scenarios.rs),
[external reference](src/legacy_reference.rs), and [benchmark support](src/bench_support.rs).
Test and benchmark evidence
boundaries are in the [JUN-08-001 ADR](../../docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md).
