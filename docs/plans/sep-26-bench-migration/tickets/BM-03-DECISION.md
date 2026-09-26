# BM-03 — Rust vs Python CLI go/no-go decision

Decision: **NO-GO on replacing the Python orchestrator with a Rust CLI.**
Python `tools/benchmark/benchctl.py` remains the **single current CLI**; the
Rust crate `benchmarks/bench-protocol` owns the typed evidence contract and an
independent validator.

This decision is recorded, not assumed. It is based on the vertical-slice
evidence below plus the structural cost of the losing path.

## 1. What was actually built and run

| Artifact | Path | Evidence |
| --- | --- | --- |
| Typed contract + reference validator + immutable run store (Rust) | `benchmarks/bench-protocol/` | `./scripts/cargow --lane bench-lane test -p quanta-index-bench-protocol` → 37 adversarial + 2 conformance + 7 round-trip tests pass; `clippy --all-targets -D warnings` and `fmt --check` clean |
| Same contract in Python (writer/reader/run store) | `tools/benchmark/evidence.py` | 27 tests in `tools/ci/tests/test_bench_protocol_conformance.py` |
| Cross-language canonical bytes and digest | `benchmarks/bench-protocol/fixtures/{canonical-json-vectors.json,sample-evidence.json}` | Rust and Python produce byte-identical canonical JSON, including the sealed sample digest |
| Single registry | `tools/benchmark/registry.toml` | `test_benchmark_manifest.py`, `test_benchmark_policy.py` |
| CLI vertical slice | `tools/benchmark/benchctl.py` `list/plan/run/validate/compare/replay/summarize/preflight` | `test_benchctl.py` (36 tests), incl. `plan` determinism and digest binding |
| Real producer slice, frozen-raw replay | `test_benchmark_evidence_bridge.py::test_promoted_real_artifact_fixture_replays_through_the_artifact_oracle` | A valid `BenchArtifactV1` `dsl-warm` artifact is promoted into an immutable run; a fresh `benchctl replay` process re-runs the independent artifact oracle (`artifact_oracle: pass`, `replay: re_derived`); tampering with the captured raw is refused with `digest mismatch` |

Raw/verdict and refusal comparison against the existing Python path:

- **Raw parity**: the bridge copies the native artifact bytes verbatim into
  `runs/<id>/raw/` and records their SHA-256; the payload rows are the native
  rows (p50/p95/p99 ms, sample counts, error/timeout counts, `early_stop_reason`).
  No number is recomputed or rescaled, so there is no shared-metric drift to
  reconcile (`test_latency_payload_preserves_rows_and_sums_failures`,
  `test_unmeasured_rows_keep_their_reason_and_no_percentile`).
- **Verdict parity**: the common envelope's verdict is *not* the domain verdict.
  A capture-only run is `scope=diagnostic, status=not_run` with the reason
  "the regression verdict is issued by the declared comparator"; the existing
  comparator remains the verdict owner for `dsl-warm`/`dsl-cold`. There is no
  second scorer and no silently upgraded verdict.
- **Refusal parity**: the Rust and Python implementations refuse the same
  classes — duplicate JSON key, unknown field, unknown protocol/version,
  malformed digest, tampered document, dirty source without a dirty digest,
  `canonical-linux` on a non-Linux host, performance scope without an exclusive
  lease, timeout/partial producer with a pass verdict, non-pass verdict without
  a reason, instruction count reported as wall latency, file-only labels
  promoted to span judgments, unjudged rows carrying a score, closed-loop
  throughput claiming an offered rate, empty/degenerate payload rows, raw path
  escape, duplicate raw path, missing/extra/tampered/reordered raw bytes,
  symlinked raw reference, repeated run id, and non-comparable baselines.

## 2. Why the Rust CLI lost

1. **No guarantee gain.** The guards that matter — clean-worktree freeze, HEAD
   re-check, host-contention preflight, DSL baseline pair admission with
   rollback and pending marker, artifact attribution, comparator host/corpus
   identity — already exist and are tested in Python. A Rust `run` would have to
   invoke those same tools (making it a thin subprocess wrapper) or reimplement
   them (duplicating a fail-closed surface that can then diverge).
2. **Divergence is the risk, not the cost.** The contract explicitly warns that
   rewriting the Python guards "without parity could reduce guarantees". A second
   implementation of baseline admission and preflight is exactly the divergence
   the packet forbids.
3. **Measured structural cost.** The workspace lints are deny-all
   (`unused_results`, `arithmetic_side_effects`, `as_conversions`,
   `indexing_slicing`, `panic`, `exit`, `print_stdout`, `print_stderr`,
   pedantic+nursery) with `clippy.toml` disallowing `Result::{ok,unwrap_or,
   unwrap_or_else,unwrap_or_default}`. The `bench-protocol` crate already needed
   explicit, reasoned `#[expect]` attributes and helper conversions to satisfy
   them; porting ~630 lines of Python guard logic (plus subprocess, TOML, YAML
   and filesystem-error handling in the CLI) is real, ongoing cost for zero
   measured user-visible benefit.
4. **The contract, not the language, was the gap.** Before this change there was
   no common typed envelope, no immutable run store and no single registry. Those
   are now in place and pinned by cross-language vectors. The orchestration
   language was never the blocker.

## 3. Losing path retirement

No Rust CLI crate was created, so there is exactly one live CLI and nothing to
retire. `check-benchmark-policy.py` fails policy if a CI workflow grows a direct
producer/comparator step, and it fails if any family is left in an ambiguous
`legacy` authority state.

**Re-open condition.** A future Rust CLI proposal must first produce a candidate
that, on a clean exact-source snapshot, preserves every refusal above and
matches raw/verdict output on the same frozen inputs, with a measured execution
cost. Until then, adding a second CLI is a policy violation.

## 4. Exclusions

- No timed CLI-overhead microbenchmark was run. The cost axis is the structural
  argument above plus the vertical-slice breadth, not a wall-clock comparison —
  a contended shared checkout cannot produce a defensible number.
- The vertical slice is contract-level (a fixed `BenchArtifactV1` fixture), not
  a canonical-host measurement. A real DSL authority capture remains
  `BLOCKED` on the quiet canonical Linux host and admitted baselines; see
  `CLOSEOUT.md`.
