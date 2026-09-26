# J7-07 — Perf Hellgate And Compare Policy

> Archive status: `Historical program record`. Current architecture: [JUN-08-001](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `landed`

Goal:

- keep correctness and latency gates separate

Owner seam:

- `Justfile`
- `tools/benchmark/README.md`

Delivered targets:

- `just rust-verify-hellgate-fast`
- `just rust-verify-hellgate-broad`
- `just rust-verify-hellgate-all`
- `just rust-bench-dsl-compare`

Rule:

- warm/cold compare is a perf gate
- `rust-bench-dsl-truth` is correctness smoke
- `rust-verify-hellgate-all` is the only aggregate target that mixes them
