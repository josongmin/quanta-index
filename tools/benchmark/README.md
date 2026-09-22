# Benchmark tooling

The registered evidence CLI is `python3 tools/benchmark/benchctl.py list`.
Its sole profile/family/path/baseline control plane is
`tools/benchmark/manifest.json`; producers remain the referenced Just recipes.
`run systems` executes the freshness and open-loop producers and then requires
their current-HEAD artifacts. `validate systems` checks existing artifacts
without rerunning them. `quality-full` additionally includes these rails.
`benchctl run` and `benchctl validate` also require the target checkout to be
clean; a HEAD-matching artifact captured before local source edits cannot be
requalified as evidence for the dirty tree.
`benchctl summarize <profile>` is read-only and labels observed files
`present_unvalidated`; it is deliberately not a qualification command.
The `dsl-authority` profile is canonical-Linux-only: it writes an
`unsupported_host` preflight receipt and refuses before producer execution on
any other OS. Local macOS runs remain available only for diagnostic families.
The open-loop qualification default uses seeded-Poisson arrivals; the prior
deterministic periodic schedule remains an explicit diagnostic mode only. The
correctness verdict requires a healthy first offered-load point
and no malformed response or unexpected typed error. Timeout, drop and socket
refusal above saturation are recorded as capacity loss with error-kind counts,
not hidden or interpreted as a passing latency SLO. A capacity threshold needs
reviewed measurements on the pinned Linux host.
The recorded retrieval and agent-outcome evaluators have separate CLIs and
strict input contracts in [retrieval/README.md](retrieval/README.md) and
[agent_outcome/README.md](agent_outcome/README.md); they do not invent runner
results when recordings are absent.

The sections below document the DSL Layer-3 latency gate.

The 3-layer DSL-benchmarking model is defined in
[`docs/plans/jun-2-dsl-hardening/RFC-DSL-Benchmarking.md`](../../docs/plans/jun-2-dsl-hardening/RFC-DSL-Benchmarking.md).
This directory holds the **Layer-3 (query latency)** tooling: capturing and
gating per-scenario warm and cold query latencies for the DSL query matrix.

Nothing here is "verified" until baselines are actually captured (Phase A)
on a quiet host at the head under test. There are no committed baselines
right now: the previous schema-1 baselines (short `git_rev`, no corpus /
config digest, host or resources, 200+ commits stale) were removed because
the gate below refuses them, and a stale baseline cannot be migrated into an
attributed one. `just rust-bench-dsl-compare` fails typed until
`--update-baseline` records one at `HEAD`.

`--update-baseline` is deliberately fail-closed: every scenario must have a
real latency row. It cannot turn an `early_stop_reason` fixture gap into a
committed ratchet reference.

## Artifact schema (the contract): `BenchArtifactV1`

Every benchmark and relevance artifact — the DSL warm/cold matrices, the
  ambiguity, snippet, scale, tail, ANN, concurrency, freshness, open-loop,
  ops, and UI rails, the relevance rail and its OpenAI A/B capture, and the
  scan-vs-index experiment — is one `BenchArtifactV1`
envelope, written by exactly one writer
(`crates/quanta-index-searchd-harness/src/artifact.rs`, QI-BB-010). A
measurement that cannot say which source, corpus, configuration, model and
host it came from is not evidence, so the envelope is:

```json
{
  "schema_version": 2,
  "dimension": "dsl-warm",
  "mode": "warm",
  "concurrency": 1,
  "provenance": {
    "git_head": "0123456789abcdef0123456789abcdef01234567",
    "corpus_digest": "sha256:…",
    "config_digest": "sha256:…",
    "model_revision": "search-owned-hash-text-v1@fnv1a64-slots-l2unit-v1:d16"
  },
  "host": {
    "os": "linux", "arch": "x86_64", "cpu_count": 8, "mem_bytes": 17179869184,
    "hostname_hash": "sha256:…"
  },
  "resources": { "peak_rss_bytes": 123456789 },
  "phases": { "build_ms": null, "update_ms": null, "gc_ms": null },
  "disk_amplification": null,
  "rows": [
    {
      "scenario_id": "lexical.keyword.native",
      "route_family": "lexical",
      "syntax": "native",
      "result_shape": "candidates",
      "latency": { "p50_ms": 0.42, "p95_ms": 0.55, "p99_ms": 0.61, "samples": 200 },
      "qps": null,
      "error_count": 0,
      "timeout_count": 0,
      "result_count": 3,
      "typed_error_code": null,
      "engine_touched": ["lexical"],
      "early_stop_reason": null
    }
  ],
  "detail": {}
}
```

- `provenance.git_head` is the exact 40-character `git rev-parse HEAD` of a
  **clean** worktree, resolved by the rail binary itself. A dirty tree, a
  short SHA or an unresolvable head is a typed refusal
  (`BENCH_WORKTREE_DIRTY`, `BENCH_GIT_HEAD_NOT_FULL`, `BENCH_GIT_UNAVAILABLE`);
  there is no `"unknown"` and no env-supplied stamp.
- `corpus_digest` is a framed sha256 over the exact bytes the rail ingested;
  `config_digest` over the rail's parameters; `model_revision` names the
  embedder the fixture was built under (`null` only when no embedding model
  was exercised).
- `host` records the host the number came from (the hostname is hashed);
  `resources.peak_rss_bytes` is `getrusage(RUSAGE_SELF)` of the harness
  process, which drives the daemon in-process.
- `phases` carry build / one-file update / reclaim durations where the rail
  has that phase (the scale rail); `null` is "no such phase", never "not
  timed". `disk_amplification` is bytes written over changed bytes for a
  rail that wrote an index.
- `rows` carry p50/p95/p99, `qps` (the concurrency rail), and error / timeout
  counts. A row with `early_stop_reason` set was **not measured**: its
  `latency` is null. A baseline containing one is refused and a current one
  fails the comparison; absent measurement is never a zero-regression result.
- `detail` is the dimension's own shape (tier manifest, per-route budgets,
  judged queries, per-client-count tallies).

Route labels are semantic ownership labels, not result-shape aliases:
`lexical`, `semantic`, `hybrid`, `symbol`, `repomap`, `structural`, `history`,
and `runtime_catalog` remain distinct. A route with no qualified tail budget is
emitted without inheriting an unrelated lexical threshold.

### Stale-artifact gate

`python3 tools/ci/lint/check-bench-artifacts.py` (part of `just rust-policy`
and the CI policy job) walks every artifact family above and refuses one
that is not schema 2, whose head is not 40 lowercase hex, or — for a fresh
artifact under `artifacts/` — whose head is not the checkout's `HEAD`.
Committed baselines are held to the shape and a full head, not to head
equality. `compare_dsl_bench.py` additionally refuses a comparison whose
current side is not at `HEAD` or whose corpus, configuration, model revision,
or host identity differs from the baseline's. Absence is reported, not refused; `--require` (the Linux perf
evidence gate) fails when a family has no artifact.

## Producers

- **`warm-matrix.json`** is produced by the dedicated runner
  `dsl_warm_matrix` (bench-profile binary; each isolated pass boots a fresh
  runtime, the runner interleaves passes round-robin across scenarios, and the
  final row pools raw samples across those isolated passes).
- **`cold-matrix.json`** is produced by `run_dsl_cold_matrix.py` (true
  cold-start: a fresh OS process per sample, using a bench-profile prebuilt
  `dsl_cold_matrix` binary).
- warm measurements include the real daemon UDS front door. Because clients are
  one-shot, daemon accept-loop cadence is still visible in the tail metrics.
  The current adopted contract is
  `crates/quanta-index-searchd/src/app/searchd.rs`: query accept idle `1ms`,
  control/ingest accept idle `5ms`. Older artifacts captured under a uniform
  `50ms` accept poll are not comparable as-if they measured the same
  steady-state path.

The scenario authority lives in
`crates/quanta-index-searchd-harness/src/scenarios.rs` and currently covers all
four shipped families end-to-end plus an adversarial family (33 scenarios):
**lexical** (keyword / phrase / regex / `file.contains` / `repo:has.file`),
**history** (`since.time` / `since.commit` / `after` / `until` /
`diff.added|removed|touched`), **runtime catalog** (`dirty` / `changed` /
`stale` / `snapshot` / `meta.*` / `affected` / `invalidated_by`), **structural**
(boolean `OR` / `NOT` plus a genuine `match { … }` tree pattern), and
**adversarial** (malformed / unterminated / oversized-past-16 KiB /
nesting-past-depth-32 — exercising the *typed-error path latency*; fail-closed
must be fast). Several lexical/structural surfaces also have a sourcegraph twin
for native↔sourcegraph parity. Each is seeded by a deterministic fixture and
served through the real runtime — no mocked latencies.

Every scenario row also carries golden behavior truth:

- `expected_shape`
- `expected_count`
- `expected_typed_error_code`

The bench runners validate that truth before emitting latency artifacts. A
latency run that drifts in behavior now fails instead of quietly publishing
numbers for the wrong result shape.

## Convenience recipes

```
just rust-bench-dsl-truth       # small golden-truth smoke over the bench scenario table
just rust-verify-hellgate-fast  # fast correctness hellgate (bench truth + text + structural + guards)
just rust-verify-hellgate-broad # broad daemon lifecycle sweep
just rust-verify-hellgate-all   # fast + broad + warm/cold compare
just rust-bench-dsl-warm        # dedicated warm authority runner -> warm-matrix.json
just rust-bench-dsl-warm-criterion  # exploratory criterion view -> warm-matrix.criterion.json
just rust-bench-dsl-cold 20     # cold matrix (20 samples/scenario) -> cold-matrix.json
just rust-bench-dsl-refresh 20  # warm -> cold -> compare, serialized authority run
just rust-bench-dsl-compare     # gate both matrices against tools/benchmark/baselines/
python3 tools/ci/lint/check-bench-artifacts.py --profile dsl-authority --require --skip-baselines
python3 tools/benchmark/benchctl.py list  # list producer/validator authority profiles
python3 tools/benchmark/benchctl.py preflight dsl-authority --receipt artifacts/benchmark-receipts/dsl-authority/preflight.json
python3 tools/benchmark/benchctl.py run dsl-authority  # clean-host preflight, serial producer, validate, compare
python3 tools/benchmark/benchctl.py compare dsl-authority  # validate then run declared baseline comparators
python3 tools/benchmark/benchctl.py summarize systems  # observed artifacts only; never a pass claim
just rust-verify-quality-concurrency  # 1/8/32 clients + slow client -> concurrency/latest/summary-c*.json
```

The warm authority runner honours `$DSL_BENCH_WARM_SAMPLES` (default 100).
The dedicated warm runner also honours `$DSL_BENCH_WARM_REPEATS`
(default 5), `$DSL_BENCH_WARM_COOLDOWN_MS` (default 10),
`$DSL_BENCH_WARM_PASS_SETTLE_MS` (default 5), and
`$DSL_BENCH_WARM_PRIME_QUERIES` (default 5).
Authority artifacts must be produced **serially**. Do not run warm and cold
producers in parallel on the same machine and then treat the results as gate
authority; shared CPU/package-cache contention can distort warm tail advisories
and cold first-query latency.

## Golden-truth smoke rail

`just rust-bench-dsl-truth` runs the same bench `SCENARIOS` table without any
timing assertions:

- `warm_matrix_scenarios_match_golden_truth`
- `cold_matrix_scenarios_match_golden_truth`

This is the correctness companion to the latency tooling. Use it when the full
runtime E2E rails are too broad and you want a smaller fail-fast proof that the
bench scenario authority still executes the shipped behavior exactly.

## Hellgate split

Verification now uses four separate lanes:

- fast correctness
  - `just rust-verify-hellgate-fast`
  - bench-owned truth + small text-route + small structural-route + inventory
    guards
- broad daemon lifecycle
  - `just rust-verify-hellgate-broad`
  - real daemon boot, front-door, replay, restart, fail-closed, corpus sweep
- cross-repo ingress
  - `just rust-verify-hellgate-cross-repo`
  - external producer publish + ingress live roundtrip
- perf compare
  - `just rust-bench-dsl-compare`

Do not collapse them into one verdict. A green perf compare is not correctness.
A green fast hellgate is not restart/replay proof.

## Scripts

### `compare_dsl_bench.py` — regression gate

```
python3 tools/benchmark/compare_dsl_bench.py <baseline.json> <current.json> \
    [--update-baseline] [--rel-threshold F] [--abs-threshold-ms F] \
    [--p95-rel-threshold F] [--p95-abs-threshold-ms F]
```

- Both artifacts must be schema-2 `BenchArtifactV1` with a full `git_head`;
  the current artifact's head must be the checkout's `HEAD` and both must share
  the corpus/config digest, model revision and host identity. Any of these
  refuses the comparison with exit 2, as does a missing baseline (capture
  one with `--update-baseline`, which itself refuses a stale current).
- Both artifacts must share the same top-level `mode`; a mismatch exits 2.
- Matches scenarios by `scenario_id`.
- Blocking metrics on both warm and cold artifacts:
  - **p50**: steady-state / first-query central tendency
  - **p95**: agent-loop tail; retrieval calls compound inside one turn
- Only `p99` remains an `ADVISORY` line.
- New scenarios (in current, not baseline) fail until a reviewed baseline update.
- Scenarios missing from current fail.
- `--update-baseline --preflight-receipt <receipt.json>` atomically writes the
  current artifact only when the candidate is Linux, complete, and the receipt
  is clean for the same OS/architecture/CPU-count host class.
- Exit codes: `0` ok, `1` regression / missing-fail, `2` usage / mode-mismatch.

### `run_dsl_cold_matrix.py` — cold-matrix orchestrator

```
python3 tools/benchmark/run_dsl_cold_matrix.py --samples K \
    --out artifacts/dsl-bench/cold-matrix.json [--bin-cmd "..."]
```

Builds `dsl_cold_matrix` once on the `bench-lane`, then invokes the resulting
binary directly once per `(scenario, sample)` in a fresh process for a genuine
cold-start, and hands every sample to the same binary's `--assemble`, which
aggregates p50/p95/p99 (nearest-rank percentile) per scenario and writes the
`BenchArtifactV1` — the orchestrator never writes an artifact or stamps a
head. Default sample count is `20`; passing fewer samples is allowed for
ad-hoc local inspection, but the comparator refuses cold artifacts with fewer
than `20` measured samples per row. Warm artifacts require `200` samples per
row; the default warm runner pools `100` samples across `5` passes.

## Baseline admission and regression gate

The scheduled Linux job requires a pinned `self-hosted, linux, quanta-bench`
runner and is a blocking authority gate. Until its
reviewed canonical baselines are committed it fails typed; it is never silently
report-only. Capture warm/cold artifacts on that same canonical host class,
review the artifact and scenario contract, then commit them through
`--update-baseline`. A deliberate scenario or semantic change requires the
same review, not an automatic PR-side update.

## Ratchet rule (exact)

A scenario regresses iff **both** legs are exceeded on the mode's blocking metric:

- **warm:** `p50 rel > +10%` **AND** `abs > +1.0 ms`
- **cold:** `p50 rel > +10%` **AND** `abs > +5.0 ms`
- **warm:** `p95 rel > +20%` **AND** `abs > +5.0 ms`
- **cold:** `p95 rel > +20%` **AND** `abs > +10.0 ms`

`--rel-threshold` / `--abs-threshold-ms` override p50 defaults;
`--p95-rel-threshold` / `--p95-abs-threshold-ms` override p95 defaults.

## Appendix: scan-vs-index scaling experiment (NOT a gate)

`run_scan_vs_index.py` + the `scan_vs_index` binary are an **exploratory
experiment**, deliberately separate from the 3-layer model above. They exist
only to make the *scaling* argument concrete, because the RFC forbids reporting
DSL latency against a text-only engine as a benchmark — a daemon IPC round-trip
and a `grep` process answer different questions, and at toy corpus sizes the
plumbing (IPC vs process spawn) dominates, which inverts the real picture.

The experiment removes that confound: it measures the lexical index query
**in-process** (no daemon, no IPC) and times `rg` / `grep` over the identical
corpus bytes, across corpus sizes. The result it demonstrates: index query
latency is ~flat in corpus size while a full scan is linear, so there is a
crossover beyond which the index wins per query (the index's one-time build cost
is reported separately and amortizes over many queries).

```
python3 tools/benchmark/run_scan_vs_index.py --scales 2000,20000,100000
```

Output goes to `artifacts/experiments/scan-vs-index.md` plus one
`BenchArtifactV1` per scale under `artifacts/experiments/scan-vs-index/`
(gitignored). The binary resolves the head itself and refuses a dirty tree;
the runner keeps the artifacts verbatim and adds only the scan timings. This
is never compared, gated, or written to the committed baselines. Only the
lexical keyword surface is even comparable to grep; history /
runtime-catalog / structural-tree queries have no text-engine equivalent.
