# Benchmark usage

Run commands from the repository root. Keep corpora, gold, models, recordings,
outputs and evidence outside the checkout. Captures need clean source and fresh
output roots. Use `list` and `plan` while editing.

- [Code-search runbook](CODE_SEARCH_RUNBOOK.md): the complete live five-product
  lexical plus Quanta–Semble lexical/semantic/hybrid matrix, required result
  table and execution-coverage summary.
- [Retrieval usage](retrieval/README.md): native runner, pair options and replay.
- [Agent recording usage](agent_outcome/README.md): A/B/C JSONL inputs.
- [Architecture decisions](../../docs/adr/README.md): contracts and measurement policy.

## Inspect, run and verify

```sh
python3 tools/benchmark/benchctl.py list
python3 tools/benchmark/benchctl.py plan systems
python3 tools/benchmark/benchctl.py run systems --evidence-root /external/bench
python3 tools/benchmark/benchctl.py validate systems --evidence-root /external/bench
python3 tools/benchmark/benchctl.py replay <run-id> --evidence-root /external/bench
python3 tools/benchmark/benchctl.py summarize systems
```

`plan` does not execute producers. `validate` checks existing evidence.
`replay` re-derives one run's verdict from retained raw bytes. `summarize`
reports observations (`present_unvalidated`), not a passing validation.
Use `--family FAMILY` with `replay` to replay that family's captured cases.
`QUANTA_BENCH_EVIDENCE_ROOT` can supply the external evidence root.

| Profile | Required input / execution |
| --- | --- |
| `dsl-authority` | Quiet canonical Linux host, warm/cold producers and admitted baselines; see below. |
| `quality-core`, `quality-full`, `systems`, `semantic-ab` | Registered native Just producers and artifact checks. Inspect `plan` for prerequisites. |
| `micro`, `dsl-diagnostic` | Criterion captures; diagnostic timings. |
| `retrieval-contract` | Contract and real-daemon SDK tests; correctness proof. |
| `retrieval-diagnostic` | External `--pair-spec`; live Quanta–Semble diagnostic. |
| `lexical-diagnostic` | External `--lexical-spec`; re-scores recorded observations from five products. **Does not run Sourcegraph, OpenGrok or cs searches.** |
| `recorded` | External `--agent-recording` and `--scan-recording`; unauthenticated imports. |

Check `list`/`plan` for current registration. An unsupported capture path refuses;
registration alone does not provide a runnable producer.

## Shared external corpus releases

Prepare a recipe and clean exact-commit checkouts with complete Git history:

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py corpus create \
  --spec /external/corpus-recipe.json --checkouts /external/checkouts \
  --release /external/corpus-releases/release-name
uv run --frozen --extra dev python tools/benchmark/benchctl.py corpus validate \
  --release /external/corpus-releases/release-name
```

Select `manifests/<repository>/<view>.json` and its matching
`views/<repository>/<view>` directory. Views are `code_only` and
`developer_search`. Freeze a new suite/query pack against that complete
commit/path/hash universe before searching. Existing destinations are refused;
retry at a fresh path. Validation restores retained Git bundles and checks view
bytes without requiring the original checkouts. Release status is
`frozen_not_admitted`; inspect exclusions in `release.json`.

## Search quality profiles

Inspect the current native producers and required artifacts before running:

```sh
python3 tools/benchmark/benchctl.py plan quality-core
python3 tools/benchmark/benchctl.py plan quality-full
python3 tools/benchmark/benchctl.py run quality-full --evidence-root /external/bench
python3 tools/benchmark/benchctl.py validate quality-full --evidence-root /external/bench
```

`quality-core` selects relevance, ambiguity, snippet, scale and tail. `quality-full`
adds ANN, concurrency, freshness, open-loop, operator and UI contract producers.
Use the plan's artifact paths to inspect each dimension independently. Local
fixtures and advisory timings do not supply external gold or large-tier capacity
measurements.

## Criterion diagnostics

```sh
python3 tools/benchmark/benchctl.py run micro --evidence-root /external/bench
python3 tools/benchmark/benchctl.py validate micro --evidence-root /external/bench
```

Options: `--criterion-samples` (default 100), `--criterion-warmup` (3 seconds),
`--criterion-measurement` (5 seconds), `--criterion-resamples` and
`--producer-timeout`. At least ten samples are required. Use fresh outputs;
interrupted or incomplete captures cannot be validated as complete profiles.

## Retrieval correctness proof

During edits:

```sh
just retrieval-contract-local
uv run --frozen --extra dev just benchmark-control-contract-local
```

On clean source:

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py run retrieval-contract \
  --evidence-root /external/bench --producer-timeout 7200
uv run --frozen --extra dev python tools/benchmark/benchctl.py validate retrieval-contract \
  --evidence-root /external/bench
uv run --frozen --extra dev python tools/benchmark/benchctl.py replay \
  --family retrieval-sdk --evidence-root /external/bench
```

Standalone source-bound producers are `just retrieval-contract-proof
<fresh-output-root>` and `just retrieval-sdk-proof <fresh-output-root>`.
For control-plane edits, `just benchmark-prep-local` runs its registered PREP
checks and build verification. PREP does not capture a timing benchmark.

## Recorded imports

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py run recorded \
  --evidence-root /external/bench \
  --agent-recording /external/recordings/agent.jsonl \
  --scan-recording /external/recordings/scan.json \
  --recorded-authenticity recorded_unauthenticated
uv run --frozen --extra dev python tools/benchmark/benchctl.py validate recorded \
  --evidence-root /external/bench
```

The scan file contains exactly `{"artifacts": [<native BenchArtifactV1>, ...]}`;
use distinct measured scales from one native source revision. Agent input format
is in the [recording guide](agent_outcome/README.md). Imports preserve recorded
observations; they do not authenticate the original producer. Requesting
`authenticated` refuses.

## DSL warm/cold gate

Run on the quiet canonical Linux host. macOS refuses `dsl-authority`;
use `dsl-diagnostic` for local diagnostic measurements.

```sh
just rust-bench-dsl-truth
python3 tools/benchmark/benchctl.py plan dsl-authority
python3 tools/benchmark/benchctl.py run dsl-authority --admit-baseline
python3 tools/benchmark/benchctl.py run dsl-authority
python3 tools/benchmark/benchctl.py compare dsl-authority
just rust-bench-dsl-refresh 20
```

The first guarded command captures both baseline candidates. Review metrics and
scenario behavior before committing them. Subsequent runs require admitted
compatible baselines. Do not run warm and cold producers concurrently.

Warm controls: `DSL_BENCH_WARM_SAMPLES=100`, `DSL_BENCH_WARM_REPEATS=5`,
`DSL_BENCH_WARM_COOLDOWN_MS=10`, `DSL_BENCH_WARM_PASS_SETTLE_MS=5`, and
`DSL_BENCH_WARM_PRIME_QUERIES=5`. Comparison requires at least 200 warm samples
and 20 cold samples per row.

For manual artifact inspection:

```sh
python3 tools/ci/lint/check-bench-artifacts.py --profile dsl-authority --require --skip-baselines
python3 tools/benchmark/compare_dsl_bench.py /external/baseline.json /external/current.json
```

Comparator options: `--rel-threshold`, `--abs-threshold-ms`,
`--p95-rel-threshold`, `--p95-abs-threshold-ms`. Exit codes: `0` accepted,
`1` regression or missing scenario, `2` invalid inputs/usage. The old
`--update-baseline` option refuses without writing. Threshold and native artifact
contracts are in [JUN-08-001](../../docs/adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md).

If `baselines/.dsl-admission-pending` exists, inspect both baseline files and
restore or recapture the pair before removing the marker. Retrying comparison
cannot repair an interrupted admission.

## Other checks and experiments

```sh
just rust-verify-hellgate-fast
just rust-verify-hellgate-broad
just rust-verify-hellgate-cross-repo
just rust-verify-hellgate-all
just benchmark-policy-local
python3 tools/benchmark/sourcegraph_parity.py --write --check
python3 tools/benchmark/run_scan_vs_index.py --scales 2000,20000,100000
```

Sourcegraph coverage output is [the generated reference](../../docs/reference/sourcegraph-filter-parity.md).
Scan-vs-index writes `artifacts/experiments/scan-vs-index.md` and native JSON
per scale; it is exploratory and is not admitted as a DSL baseline.

## Find outputs and handle failures

An external evidence root contains `runs/<run-id>/evidence.json`, native files
under each run's `raw/`, `captures/<capture-id>.json`, and
`profiles/<profile>.json`. Use the profile pointer to find a complete capture;
`latest` is advisory. Baselines name explicit runs under `baselines/`.

For source drift, re-freeze inputs and capture on clean source. For missing raw,
changed binaries or incomplete cases, retain the failed capture and rerun into
a fresh root. Check command logs and `work/<capture-id>/capture.json` or
`failures/<capture-id>.json`; missing terminal evidence cannot establish success.
Revalidation after code, input, dependency or normative ADR edits requires fresh
bound evidence. See [evidence policy](../../docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md).
