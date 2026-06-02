# DSL Benchmark Tooling (Layer 3: query latency)

The 3-layer DSL-benchmarking model is defined in
[`docs/plans/jun-2-dsl-hardening/RFC-DSL-Benchmarking.md`](../../docs/plans/jun-2-dsl-hardening/RFC-DSL-Benchmarking.md).
This directory holds the **Layer-3 (query latency)** tooling: capturing and
gating per-scenario warm and cold query latencies for the DSL query matrix.

Nothing here is "verified" until the baselines are actually captured (Phase A).
The seed baselines ship with empty `rows` and `git_rev: "unbaselined"`.

## Artifact schema (the contract)

Both scripts read and write a single artifact shape:

```json
{
  "schema_version": 1,
  "mode": "warm",
  "git_rev": "abc1234",
  "rows": [
    {
      "scenario_id": "lexical.keyword.native",
      "route_family": "lexical",
      "syntax": "native",
      "mode": "warm",
      "result_shape": "candidates",
      "latency_p50_ms": 0.42,
      "latency_p95_ms": 0.55,
      "latency_p99_ms": 0.61,
      "samples": 200,
      "result_count": 3,
      "typed_error_code": null,
      "engine_touched": ["lexical"],
      "early_stop_reason": null,
      "git_rev": "abc1234"
    }
  ]
}
```

- `mode` — `"warm"` or `"cold"` (top-level and per-row; they match).
- `route_family` — `lexical | history | runtime_catalog | structural`.
- `syntax` — `native | sourcegraph`.
- Required row keys: `scenario_id`, `route_family`, `syntax`, `mode`,
  `result_shape`, `latency_p50_ms`, `latency_p95_ms`, `latency_p99_ms`,
  `samples`.
- Optional row keys (may be null/absent): `result_count`, `typed_error_code`,
  `engine_touched`, `early_stop_reason`.
- A row with `early_stop_reason` set (e.g. `"fixture_not_seeded"`) was **not
  measured**: its latency fields are null. The comparator skips such rows
  entirely — never compares, never fails on them.

## Producers

- **`warm-matrix.json`** is produced by the criterion bench `dsl_query_matrix`
  (amortized hot-path latency; the runtime is booted once and reused).
- **`cold-matrix.json`** is produced by `run_dsl_cold_matrix.py` (true
  cold-start: a fresh OS process per sample).

## Scripts

### `compare_dsl_bench.py` — regression gate

```
python3 tools/benchmark/compare_dsl_bench.py <baseline.json> <current.json> \
    [--update-baseline] [--rel-threshold F] [--abs-threshold-ms F] [--allow-missing]
```

- Both artifacts must share the same top-level `mode`; a mismatch exits 2.
- Matches scenarios by `scenario_id` and compares the **p95** latency.
- New scenarios (in current, not baseline) print as `NEW` and never fail.
- Scenarios missing from current (and measured in baseline) **FAIL** by default;
  `--allow-missing` downgrades them to a warning.
- `--update-baseline` writes current over the baseline verbatim and exits 0.
- Exit codes: `0` ok, `1` regression / missing-fail, `2` usage / mode-mismatch.

### `run_dsl_cold_matrix.py` — cold-matrix orchestrator

```
python3 tools/benchmark/run_dsl_cold_matrix.py --samples K \
    --out artifacts/dsl-bench/cold-matrix.json [--bin-cmd "..."] [--git-rev REV]
```

Invokes the harness binary once per `(scenario, sample)` in a fresh process for
a genuine cold-start, then aggregates p50/p95/p99 (nearest-rank percentile) per
scenario. Default `--bin-cmd` is
`cargo run --quiet -p quanta-index-searchd-harness --bin dsl_cold_matrix --`.

## Rollout: Phase A then Phase B

- **Phase A — baseline capture (report-only).** Run the producers, eyeball the
  numbers, and commit captured baselines with `--update-baseline`. No gating;
  the comparator is informational only. The seed baselines here are empty
  placeholders until this phase runs.
- **Phase B — relative regression gate.** Once baselines are trusted, wire
  `compare_dsl_bench.py` into CI as a blocking gate against the committed
  baselines. Regressions fail the build; deliberate changes are accepted by
  re-running with `--update-baseline` in the same PR.

## Ratchet rule (exact)

A scenario regresses iff **both** legs are exceeded on p95:

- **warm:** `rel > +10%` **AND** `abs > +1.0 ms`
- **cold:** `rel > +10%` **AND** `abs > +5.0 ms`

Explicit `--rel-threshold` / `--abs-threshold-ms` override the mode defaults.
