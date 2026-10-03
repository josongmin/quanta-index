# Scan versus index experiment

Exploratory executable comparing keyword scan and indexed lookup scaling.
Its output does not qualify daemon latency, LQ behavior, or a committed
benchmark baseline.

The entry point is [src/main.rs](src/main.rs). Benchmark evidence boundaries
are in the [JUN-08-001 ADR](../../docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md).
