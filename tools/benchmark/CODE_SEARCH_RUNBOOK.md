# Code-search benchmark runbook

Run commands from the repository root. Use the [benchmark guide](README.md)
for other profiles and the [retrieval guide](retrieval/README.md) for native
runner options. Evidence and scoring policy lives in
[SEP-26-003](../../docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md)
and [SEP-27-004](../../docs/adr/SEP-27-004-benchmark-capture-and-resource-custody.md).

## Choose the rail first

| Goal | Command/owner | What actually runs | Maximum claim |
| --- | --- | --- | --- |
| Complete code-search comparison (default) | Every repository × query-family cell in [Complete comparison](#complete-comparison-is-the-default) | Five live lexical products plus Quanta--Semble lexical, semantic and hybrid pairs | Full diagnostic matrix only after all cells validate and replay |
| Check retrieval code while editing | `just retrieval-contract-local` | Focused Python/Rust contract tests | Dirty-checkout diagnostic; no search quality or speed |
| Capture current-source SDK/contract proof | `benchctl run retrieval-contract` | Real daemon SDK proof and contract tests | Typed correctness proof; no relevance or speed |
| Compare Quanta with Semble | `benchctl run retrieval-diagnostic --pair-spec ...` | Both products search the frozen repo/query pack | Exploratory paired diagnostic; not qualified quality or speed |
| Run all five lexical products | `benchctl code-search run --spec ...` | External live capture, live SDK pair, five-product score, validation and replay | Fresh five-product diagnostic; no qualified quality or speed |
| Capture the three external lexical products | `benchctl code-search external --spec ...` | Live Sourcegraph/OpenGrok HTTP requests and cs processes; retains native responses | Fresh external recorded diagnostic; indexed-universe attestation remains absent |
| Score five lexical products | `benchctl run lexical-diagnostic --lexical-spec ...` | Re-scores **recorded** Quanta, Semble, Sourcegraph, OpenGrok and cs observations | Corpus-bound recorded file-recall diagnostic; **no live product search** |

The `code-search` workflow composes the existing registered pair and lexical
profiles with live external collection over one frozen corpus view and query pack.
`lexical-diagnostic` itself never starts or calls Sourcegraph, OpenGrok or cs.
The current workflow validates one lexical input per invocation. No aggregate
runner yet enforces completeness of the three-mode matrix below; the execution
summary must explicitly reconcile expected and completed cells. A fail-closed
matrix inventory is tracked in
[CS-INT-01](../../docs/plans/sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md#current-source-audit-and-decision-order).

## Complete comparison is the default

When asked to run a code-search benchmark, run every applicable cell below. Do
not stop after a Quanta--Semble pair or after rescoring recorded lexical rows.
For each repository and mode query family, freeze the same commit,
manifest/file universe, suite and query pack across the Quanta--Semble modes.
The external five-product scorer currently accepts bare-symbol lexical queries;
do not label natural-language external cells as missing work.

| Mode | Required execution | Products with live search | Products not applicable |
| --- | --- | --- | --- |
| Lexical-only | Bare-symbol supported family: one `benchctl code-search run --spec ...` workflow. Other families: one `retrieval-diagnostic` pair, then validate and replay | Quanta, Semble; Sourcegraph, OpenGrok and cs only on admitted bare-symbol inputs | External products on unsupported query forms |
| Semantic-only | One `benchctl run retrieval-diagnostic --pair-spec ...`, then validate and replay | Quanta, Semble | Sourcegraph, OpenGrok, cs |
| Hybrid | One `benchctl run retrieval-diagnostic --pair-spec ...`, then validate and replay | Quanta, Semble | Sourcegraph, OpenGrok, cs |

`code-search run` already performs the live external captures, the Quanta--Semble
lexical pair, lexical scoring, validation and replay. Do not run a second lexical
pair for that same repository/query family. Add the semantic-only and hybrid
pairs, then move to the next family. The semantic/hybrid cells for the three
external search products are `N/A`; they are not silently omitted or filled
with lexical scores.

For `R` repositories and `F` mode query families, require `3 × R × F`
Quanta--Semble mode results. Let `B` be the number of bare-symbol query
families supported by the five-product scorer. Require `R × B` live five-product
workflows. If each workflow's commit, manifest, suite and query pack exactly
match its corresponding lexical mode cell, that cell counts toward `3 × R × F`
and `R × (3F - B)` additional pair runs remain. If any input differs, run the
mode cell separately and report the workflow as a separate corpus/query track.
Every workflow must list all five products and every pair must pass `run`,
`validate` and `replay`. For example, 10
repositories × 2 mode families (bare and natural) with one bare-symbol family
requires 60 paired mode results, 10 five-product workflows, 50 additional mode
pairs when each workflow exactly matches its lexical mode cell, and 200 live
queries per external product when each suite has 20 tasks. If those inputs do
not match, run all 60 mode pairs separately.
Use fresh output roots for every run. A missing, failed or unverified applicable
cell keeps the matrix incomplete.

Within each repository/query family, compare products only when commit,
manifest/file universe, suite and query pack match. Across repositories, report
the aggregation method and retain each run's universe binding. The five-product
lexical workflow and the mode pairs can each be internally matched while still
covering different corpus views; never merge their scores into one overall row.
External search products are `N/A` for semantic/hybrid
modes and for query families rejected by the bare-symbol scorer, with the reason
shown in the table.

### Required result table

After the matrix finishes, show **one table** containing all five product columns
for every row. Do not split external products from Quanta/Semble modes. Compare
products within each repository only under the exact same universe. Include the
corpus view/cohort in each row; do not combine two different manifests for the
same repository. Across repositories, aggregate per-repository metrics with a
declared weighting and retain each manifest digest. Populate values from current
validated reports. Use `N/A` only for unsupported external routes or query
forms, and `NOT_RUN` or `FAILED` for missing or failed applicable work. Never
render missing values as zero.

| Corpus view / universe | Query family | Mode | Quanta (NDCG@10 / file recall@10) | Semble (NDCG@10 / file recall@10) | Sourcegraph (file recall@10) | cs (file recall@10) | OpenGrok (file recall@10) | Δ NDCG@10 (95% CI; Quanta − Semble) | Tasks/provider | Coverage note |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `<same five-product universe>` | bare-symbol | lexical-only | ... | ... | ... | ... | ... | ... | ... | all five live |
| `<same common universe>` | bare | lexical-only | ... | ... | ... | ... | ... | ... | ... | all five live; same manifest |
| `<mode-matrix universe>` | bare | semantic-only | ... | ... | `N/A` | `N/A` | `N/A` | ... | ... | no external semantic route |
| `<mode-matrix universe>` | bare | hybrid | ... | ... | `N/A` | `N/A` | `N/A` | ... | ... | no external hybrid route |
| `<mode-matrix universe>` | natural | lexical-only | ... | ... | `N/A` | `N/A` | `N/A` | ... | ... | bare-symbol scorer does not accept natural queries |
| `<mode-matrix universe>` | natural | semantic-only | ... | ... | `N/A` | `N/A` | `N/A` | ... | ... | no external semantic route |
| `<mode-matrix universe>` | natural | hybrid | ... | ... | `N/A` | `N/A` | `N/A` | ... | ... | no external hybrid route |
| `<same corpus-view cohort>` | all | each mode | ... | ... | ... | ... | ... | ... | ... | per-repository matched inputs |

The bare-symbol lexical row is complete only when all five columns contain
results from the same manifest, suite and query pack. If a product was not
queried on that exact input, show `NOT_RUN` and mark the matrix incomplete.

Include a compact execution summary with expected/completed workflows, paired
results, live task rows per external product, validation/replay counts, source
HEAD and the diagnostic/qualification boundary. Keep latency out of cross-product
rankings unless a separate qualified speed protocol passes.

## Lexical workflow command (repeat for every repository × supported bare-symbol family)

Use the [workflow spec example](retrieval/examples/code-search-workflow.json)
after preparing the pair and external specs below. The workflow refuses unequal
suite/pack bytes, a different release manifest, hybrid profiles, dirty source
and an existing output root. It captures external products first so authentication,
revision and result-envelope failures are discovered before the SDK pair.

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py code-search run \
  --spec /absolute/external/code-search-workflow.json
uv run --frozen --extra dev python tools/benchmark/benchctl.py code-search verify \
  --capture /absolute/external/fresh-five-product-run
```

`workflow.json` is published only after all five products, both profile validations,
both replays and the native external replay succeed. A failed run retains
`failure.json` and per-stage execution logs; start a fresh root after repair.
Use `code-search verify` to replay retained native responses and profile inputs.
Inspect `workflow.json` for complete capture identities. Long socket paths use a
short native runtime directory recorded in `pair-spec.json`; the permanent
workflow root retains the native tree. Retry failures with a fresh root.

## Shared preparation

1. Record `git rev-parse HEAD` and `git status --porcelain=v1`. `benchctl run`
   and `validate` require a clean source checkout. During concurrent/dirty
   edits, use `list`, `plan` or `just retrieval-contract-local` only; do not
   attribute a historical artifact to the current tree.
2. Keep corpus checkouts, release, suite, query pack, model assets, specs,
   native outputs and evidence **outside this checkout**. Give every capture
   a fresh output root. Do not overwrite an earlier run to retry it.
   Use complete Git history with all reachable objects. Shallow/partial checkouts
   cannot produce a self-contained bundle and are refused before live capture.
3. If using a common corpus release, validate it before choosing one repository
   and view. Both products must use its same commit, ordered file/path/SHA
   universe and frozen queries. A release is `frozen_not_admitted`, not proof
   that each product indexed every file.

```sh
python3 tools/benchmark/benchctl.py list
python3 tools/benchmark/benchctl.py plan retrieval-diagnostic
python3 tools/benchmark/benchctl.py plan lexical-diagnostic
uv run --frozen --extra dev python tools/benchmark/benchctl.py corpus validate \
  --release /absolute/external/corpus-releases/release-name
```

To create a new release, use
`uv run --frozen --extra dev python tools/benchmark/benchctl.py corpus create`
with `--spec`, `--checkouts` and
`--release` as documented in the [corpus section](README.md#shared-external-corpus-releases).
The recipe and exact-commit clean checkouts must exist first. A new evaluation
suite must bind the selected view's complete file universe and record label
provenance. Independent judgments are required for a quality claim; mechanical
labels remain diagnostic. Freeze its blind query pack with:

```sh
python3 -m tools.benchmark.retrieval freeze \
  --repo /absolute/external/clean-git-checkout \
  --suite /absolute/external/suite.json \
  --output /absolute/external/query-pack.json
```

`freeze` is not a gold-label generator. The suite, manifest and pack must
agree on commit, file universe, query identities and comparison contract.
Replace every placeholder in the [exploratory pair spec example](retrieval/examples/pair-spec.exploratory.json),
[live external spec example](retrieval/examples/live-external-spec.json), and
[common lexical spec example](retrieval/examples/lexical-spec-v2.json). The
examples demonstrate current schema only: zero digests and `/external/...`
paths are deliberately unusable. Do not reuse a prior binary or input digest.

## A. Run a live Quanta--Semble pair

Prepare an external JSON spec satisfying
[`pair-spec.schema.json`](retrieval/pair-spec.schema.json). In particular, pin
`repo`, `manifest`, `suite`, `query_pack`, `runner_binary`, `searchd_binary`,
`searchd_expected_sha256`, `host_profile`, `semble_python`,
`semble_lockfile` and its SHA-256, model assets, execution profiles, routes,
strategy, and a **new** native `output_root`. For this common profile set
`scope: "exploratory"`; a qualified spec is refused. For lexical-only work,
declare a lexical route and compatible Quanta/Semble execution profiles; do
not label a hybrid/default route lexical. The native runner refuses a dirty or
wrong-HEAD corpus checkout. Build/pin actual binaries for the frozen source;
an older spec's binary paths or digests are not reusable by assumption. Keep
the native output, original corpus and evidence roots mutually disjoint.
Generate a host profile at a new external path and build both binaries through
the repository wrapper before replacing their example paths and digests:

```sh
uv run --frozen --extra dev python -m tools.benchmark.retrieval.run host-profile \
  --profile-id code-search-exploratory --out /absolute/external/host-profile.json
./scripts/cargow --lane bench-lane build -p quanta-index-searchd-runtime \
  --bin quanta-index-searchd --locked
./scripts/cargow --lane bench-lane build -p quanta-index-retrieval-bench \
  --bin quanta-index-retrieval-bench --locked
mkdir -p /absolute/external/bin
QUANTA_INDEX_BUILD_LANE=bench-lane bash -lc '
  source scripts/quanta-index-env.sh
  install -m 0755 "$CARGO_TARGET_DIR/debug/quanta-index-searchd" \
    /absolute/external/bin/quanta-index-searchd
  install -m 0755 "$CARGO_TARGET_DIR/debug/quanta-index-retrieval-bench" \
    /absolute/external/bin/quanta-index-retrieval-bench
'
shasum -a 256 /absolute/external/bin/quanta-index-searchd
shasum -a 256 /absolute/external/bin/quanta-index-retrieval-bench
```

The binary paths in the spec must name the binaries actually built from this
source. Check the pair spec shape without starting a search:

```sh
uv run --frozen --extra dev python -c \
  'from pathlib import Path; from tools.benchmark.retrieval.run import load_spec; load_spec(Path("/absolute/external/pair-spec.json"))'
```

This is only a structural check; `run` performs the corpus/binary preflight.
For a lexical suite derived from a different query form, use
[`prepare_lexical_pair.py`](retrieval/prepare_lexical_pair.py) to preserve task
labels while freezing new bare-symbol queries.

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py run retrieval-diagnostic \
  --pair-spec /absolute/external/pair-spec.json \
  --evidence-root /absolute/external/fresh-evidence \
  --producer-timeout 7200

uv run --frozen --extra dev python tools/benchmark/benchctl.py validate retrieval-diagnostic \
  --evidence-root /absolute/external/fresh-evidence

uv run --frozen --extra dev python tools/benchmark/benchctl.py replay \
  --family retrieval-pair --evidence-root /absolute/external/fresh-evidence
```

`validate` checks the captured current-source profile; `replay` re-derives a
promoted run from retained raw bytes. Neither starts a fresh pair. Inspect
`<evidence-root>/profiles/retrieval-diagnostic.json`, its referenced capture
and `runs/<run-id>/` raw evidence, plus the native pair report and verdict.
The controller's elapsed time includes setup/build work; use native per-query
timing for query latency, and keep failures/timeouts in the denominator.

## B. Score five recorded lexical products

Run the live external collector with the same selected release view, suite and
blind pack as the pair. Sourcegraph and OpenGrok must already serve that view;
the spec pins their HTTP(S) origins, repository/project names and
operator-supplied image digests. Put required credentials in external
`token_file` paths; the collector reads them without writing token bytes into
the capture. Sourcegraph uses its `token` scheme; OpenGrok uses `Bearer`.
Sourcegraph's Git origin must contain the selected original commit; a new
synthetic snapshot commit is refused. Its request includes an exact selected-view
file filter. cs must be an executable binary. The collector
retains each raw HTTP response and cs stdout/stderr, checks terminal results,
and emits exactly one `symbol_only` row per task and product. Failed or partial
captures stay in `.staging` and do not publish the final output root:

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py code-search external \
  --spec /absolute/external/live-external-spec.json
uv run --frozen --extra dev python tools/benchmark/benchctl.py code-search external-verify \
  --capture /absolute/external/fresh-external-capture
```

Inspect its `capture.json`, raw response files and three `*_rows.jsonl` files.
The release proves the admitted file bytes; an HTTP result or operator image
digest does **not** independently attest that a server indexed every file.
Keep Quanta/Semble's native pair report, lock, records and verdict. The scorer
requires the exact row contract in
[`lexical_file_comparison.py`](retrieval/lexical_file_comparison.py), including
bare-symbol queries and result paths inside the selected file universe.

Create the external `--lexical-spec` schema-v2 envelope with exactly:

- `corpus`: `release_path`, `release_digest` (`sha256:...`), `repository`,
  and `view` (`code_only` or `developer_search`).
- `inputs`: absolute paths for `suite`, `query_pack`, `pair_report`,
  `pair_lock`, `semble_native`, `pair_verdict`, `sourcegraph_rows`,
  `opengrok_rows`, and `cs_rows`.

The spec is checked against the release's commit and complete selected-view
file universe; see [`corpus_binding.py`](corpus_binding.py). Run only after all
nine inputs are complete:

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py run lexical-diagnostic \
  --lexical-spec /absolute/external/lexical-corpus-bound-spec-v2.json \
  --evidence-root /absolute/external/fresh-lexical-evidence

uv run --frozen --extra dev python tools/benchmark/benchctl.py validate lexical-diagnostic \
  --evidence-root /absolute/external/fresh-lexical-evidence

uv run --frozen --extra dev python tools/benchmark/benchctl.py replay \
  --family lexical-file-comparison \
  --evidence-root /absolute/external/fresh-lexical-evidence
```

Inspect `profiles/lexical-diagnostic.json` and all five product runs. Report
file hit@10 and file recall@10 separately. Recorded timing layers differ
(HTTP wall, process spawn/search, SDK call, worker dispatch); their means or
p95s are descriptive and **not** a cross-product speed ranking. Mechanical
symbol labels are not independent developer-relevance judgments.

## C. Correctness proof without a relevance benchmark

For an edit loop, run `just retrieval-contract-local`. On frozen clean source,
use a fresh external root for the registered SDK/contract proof:

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py run retrieval-contract \
  --evidence-root /absolute/external/fresh-contract-evidence \
  --producer-timeout 7200

uv run --frozen --extra dev python tools/benchmark/benchctl.py validate retrieval-contract \
  --evidence-root /absolute/external/fresh-contract-evidence
```

This proves only its selected tests and SDK roundtrip. It does not produce
`PAIR_VALID`, `QUALITY_DELTA` or `PERF_QUALIFIED`.

## Report and failure rules

For each reported result, retain the exact HEAD/dirty state, registry digest,
corpus release/view and universe digest, suite/query-pack digest, binary/model
identities, product versions, host/cache regime, exact command, terminal
status, capture/run IDs and raw paths/digests. State covered/excluded query
strata and denominators. Use `VERIFIED`, `FAILED`, `BLOCKED` or `NOT_RUN` for
the **specific claim**, not for the whole project. A successful recorded
re-score is not a verified fresh five-product capture; a successful live pair
is not independent-gold quality or quiet-host performance qualification.

Common refusals are meaningful: dirty source or corpus, wrong commit/universe,
missing or stale spec input, old binary/model pin, existing output root,
incomplete/duplicate query rows, failed HTTP/process result, timeout, or
changed evidence bytes. Fix the input and start a new capture; never fill
missing observations with zero or rename an old receipt as current evidence.
