# J7-07 — Perf Hellgate And Compare Policy

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
