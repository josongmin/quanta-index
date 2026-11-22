# Benchmark usage

Run commands from the repository root. Keep corpora, gold, models, recordings,
outputs and evidence outside the checkout. Captures need clean source and fresh
output roots. Use `list` and `plan` while editing.

- [Code-search runbook](CODE_SEARCH_RUNBOOK.md): the complete live five-product
  lexical plus Quanta–Semble lexical/semantic/hybrid matrix, [one result table
  and reading guide](CODE_SEARCH_RUNBOOK.md#required-result-table) with mode
  input scope and execution coverage.
- [Retrieval usage](retrieval/README.md): native runner, pair options and replay.
- [Agent recording usage](agent_outcome/README.md): A/B/C JSONL inputs.
- [Architecture decisions](../../docs/adr/README.md): contracts and measurement policy.

The registered `retrieval-diagnostic` and code-search pair use the Legacy
generation SDK/`serve` path. In these guides, a local or native runner means a
Rust producer, not automatic selection of `serve-native`. Explicit Native file
capture uses the [Native file profile](retrieval/README.md#explicit-native-file-profile).

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
| `quality-core`, `quality-full`, `systems` | Registered native Just producers and artifact checks. Inspect `plan` for prerequisites. |
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
Relevance, tail, concurrency, freshness, open-loop, operator and UI runs require
`QUANTA_INDEX_LEGACY_SEARCHD_BIN` and `QUANTA_INDEX_LEGACY_SEARCHD_SHA256` for their
pinned Legacy reference. The relevance, tail, load and freshness reports record the
executable digest and daemon peak resident bytes. Native relevance acceptance
uses the separate SDK holdout on the Native candidate.
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

The `dsl-diagnostic` profile runs the shared warm fixture against a pinned
Legacy daemon over SDK IPC. Set `QUANTA_INDEX_LEGACY_SEARCHD_BIN` and
`QUANTA_INDEX_LEGACY_SEARCHD_SHA256` to the approved executable and digest.
Its optional matrix is stored as `dsl-diagnostic.json` in the capture. It
measures an SDK round trip and carries a distinct configuration digest from
older in-process Criterion runs. `dsl-authority` has its own producers and
baseline admission.

## Archived OpenAI A/B diagnostic

Use the reviewed A/B executable, its build receipt, and the matching clean
source checkout. `benchctl.py capture-frozen-openai-ab` requires `--runner`,
`--runner-sha256` with a `sha256:` prefix, `--source-checkout`, `--source-head`,
and a fresh absolute `--out` directory outside that checkout. It runs on Linux
and macOS with atomic no-replacement rename support on the output filesystem.
The command publishes `summary.json`, `cases.json`, and `provider-stats.json`
together after validation. A path created concurrently stays intact and the
capture fails. Successful capture prints the sidecar digests and the source
and runner identity.

For the external-daemon runner, set `QUANTA_INDEX_LEGACY_SEARCHD_BIN` to the
approved absolute executable path and `QUANTA_INDEX_LEGACY_SEARCHD_SHA256` to its
64-character lowercase digest. Set `QUANTA_INDEX_PROVIDER_ENDPOINT` to
`https://api.openai.com/v1/embeddings` and set
`QUANTA_INDEX_PROVIDER_OPENAI_MAX_REQUEST_COST_MICROS` to the approved
worst-case charge for one request under the selected model and input limits,
including oversized single inputs. Set
`QUANTA_INDEX_PROVIDER_COST_CEILING_MICROS` to the approved total in the
same units. The daemon debits
that bound before each outbound attempt, including retries and attempts whose
outcome is ambiguous. Source and query HTTP attempts share the process cost,
concurrency, and input-byte limits.

With the intended provider environment set, run the reviewed executable as
`QUANTA_INDEX_EMBEDDER=openai /absolute/path/to/relevance_openai_ab --print-inputs`.
This prints the resolved configuration including defaults and the digests of
that configuration, the exact Source batch, and the compiled judgment roster.
It runs without launching either daemon or calling OpenAI.
After review, set `QUANTA_INDEX_OPENAI_AB_APPROVED_INPUT_DIGEST` to the printed
`approval_digest`. The capture runs a credential-free `--print-inputs` on the
pinned runner and compares the compiled judgments with the committed
`semantic_judgments.tsv` before starting the paid run. Settings, Source and
judgments are bound by the same approval digest. The adapter validates this
digest before giving the runner its credential and before publishing results.
The runner acquires a new staging directory before starting either arm and
writes sidecars with exclusive creation. The adapter publishes them together
after validation.

The runner checks the daemon's installed limits before publishing Source. The
capture verifies the executable bytes, the two activation roots, query-phase
provider observations, the supplied provider settings and grant, and the cases
from the approved checkout.

For preserved results, use
`benchctl.py validate-frozen-openai-ab --capture DIR --source-checkout CHECKOUT --source-head HEAD`.
Supply the original non-secret provider grant settings. Historical in-process
captures use their original Rust judgment roster. External-daemon captures
also require their daemon SHA-256 and original input approval digest. Archived
external-daemon results can be checked with the original approval format.
Validation
reads the three sidecars and prints their digests. It requires neither an API
key nor an executable.

`just rust-capture-quality-relevance-openai-ab RUNNER SHA SOURCE_CHECKOUT
SOURCE_HEAD OUT` calls the same capture command. The former `benchctl run
semantic-ab` profile is retired. Review preserved outputs through the archived
capture contract and retain their original source revision.

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
python3 tools/benchmark/benchctl.py run dsl-authority --dsl-native-plan "$QUANTA_DSL_NATIVE_PLAN" --admit-baseline
python3 tools/benchmark/benchctl.py run dsl-authority --dsl-native-plan "$QUANTA_DSL_NATIVE_PLAN"
python3 tools/benchmark/benchctl.py compare dsl-authority
just rust-bench-dsl-refresh 20
```

Set `QUANTA_DSL_NATIVE_PLAN` to a JSON control file for the approved candidate.
Its exact fields are `schema` (`native-dsl-authority-plan/v1`), `git_head`
(the full frozen checkout commit), `warm_runner`, `cold_runner`, `daemon`,
and `history_fixture`. Each file entry contains an absolute `path` and a
`sha256` string in `sha256:<lowercase-hex>` form. Use the two NoFeatures
Linux runners and the approved Native daemon from the same candidate build,
and the Published History fixture for that daemon.

Prepare the History fixture from that frozen checkout using the existing
Runtime selector. Supply the approved Linux old-Q path and its lowercase
SHA-256 through `QUANTA_INDEX_LEGACY_SEARCHD_BIN` and
`QUANTA_INDEX_LEGACY_SEARCHD_SHA256`. Both daemon SHA arguments for this
preparation use raw lowercase hex.

```sh
just rust-prepare-native-dsl-history "$history_export_root" "$native_daemon" "$native_daemon_sha256"
```

Use a short, new absolute export directory outside the checkout. The exporter
publishes `cold-fixture.json` after verifying its seven History scenarios through
warm queries and fresh daemon processes. Use that file's path and SHA-256 in
the plan and keep its referenced state and config in place for both modes.
Both collectors require the fixture's recorded `source_git_head` to match the
frozen checkout and use the exact Published view, cut and instance.

The first guarded run captures both baseline candidates on the same host.
The profile publishes the warm and cold pair after validating both staged
artifacts and their preflight and input pins. Review metrics and scenario
behavior before committing the baselines. Subsequent runs require compatible
Native baselines.

Warm controls: `DSL_BENCH_WARM_SAMPLES=100`, `DSL_BENCH_WARM_REPEATS=5`,
`DSL_BENCH_WARM_COOLDOWN_MS=10`, `DSL_BENCH_WARM_PASS_SETTLE_MS=5`, and
`DSL_BENCH_WARM_PRIME_QUERIES=5`. Comparison requires at least 200 warm samples
and 20 cold samples per row.

To rerun a Legacy producer from a frozen release, supply the checkout commit
and the SHA-256 of its prebuilt executable. Select `--bin-mode cold` and the
cold executable for fresh-process measurements. Use a distinct output for
each mode.

```sh
python3 tools/benchmark/run_dsl_cold_matrix.py --bin-mode warm \
  --bin-cmd /archive/release/bin/dsl_warm_matrix --bin-cwd /archive/release/source \
  --bin-head "$release_commit" --bin-sha256 "sha256:$warm_sha256" \
  --out /archive/results/warm-matrix.json
```

Use the original release build receipt to verify that the executable was
built from the supplied commit. Each capture runs a private executable copy
checked against the supplied digest. The runner verifies the checkout, original
executable, and output artifact before publication. Baseline admission still
uses the guarded authority run.

For manual artifact inspection:

```sh
python3 tools/ci/lint/check-bench-artifacts.py --profile dsl-authority --require --skip-baselines
python3 tools/benchmark/compare_dsl_bench.py /external/baseline.json /external/current.json
```

To run the Native Text and History SDK matrix, use `just rust-bench-dsl-native
<warm|cold> <runner> <runner-sha> <daemon> <daemon-sha> <history-fixture>
<fixture-sha> <out>`. Supply the matching warm or cold runner, absolute input
paths, `sha256:`-prefixed digests and an output path outside the checkout. Both
modes reopen the producer's Published History fixture, so its socket must be
stopped. The runner obtains the case list from the pinned Rust binary. A
complete result uses `native-dsl-matrix-v1` and is diagnostic. The existing
`dsl-authority` profile owns Linux comparison baseline admission.

History samples use `native-dsl-history-samples-v2` and carry a typed page
from a timed query. Collection binds the page to the pinned Published view,
History cut and instance. Assembly verifies the exact fixture rows and rejects
a capture that mixes published members or cuts.

Add `--native-artifact --bin-cwd <frozen-checkout> --bin-head <full-commit>`
after `<out>` to assemble the same collected bytes through the pinned Rust
`BenchArtifactV1` writer. Assembly requires a clean checkout and a matching
runner. The common artifact preserves the raw matrix and all 33 registry rows.
The default keeps diagnostic qualification. The official profile requests
authority from its pinned producer and requires Linux, the configured sample
floors, and measured daemon RSS on daemon-backed reports. Each mode writes its
staged output after input and artifact validation completes.

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
```

`rust-verify-hellgate-all` runs the fast and broad functional checks before the
DSL authority refresh. The refresh validates the committed baseline pair and
checkout before starting the timed producers.

Sourcegraph coverage output is [the generated reference](../../docs/reference/sourcegraph-filter-parity.md).
The recorded scan-vs-index experiment can be reproduced using the binary
from a frozen Legacy checkout. The runner invokes it with that checkout as
its working directory so the binary records the Legacy source revision.

```sh
python3 tools/benchmark/run_scan_vs_index.py \
  --bin-cmd /path/to/legacy/scan_vs_index --bin-cwd /path/to/legacy-checkout
```

The runner writes a Markdown report and one JSON artifact per scale under
`artifacts/experiments/`. Recorded capture keeps this experiment diagnostic.

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
