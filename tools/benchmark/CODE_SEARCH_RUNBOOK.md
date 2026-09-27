# Code-search benchmark runbook

Run commands from the repository root. This is the operator guide; the
[benchmark README](README.md) owns the common evidence contract, and the
[retrieval README](retrieval/README.md) owns the native runner and metric
contracts. Neither an old report nor a successful `plan` is a new benchmark.

## Choose the rail first

| Goal | Command/owner | What actually runs | Maximum claim |
| --- | --- | --- | --- |
| Check retrieval code while editing | `just retrieval-contract-local` | Focused Python/Rust contract tests | Dirty-checkout diagnostic; no search quality or speed |
| Capture current-source SDK/contract proof | `benchctl run retrieval-contract` | Real daemon SDK proof and contract tests | Typed correctness proof; no relevance or speed |
| Compare Quanta with Semble | `benchctl run retrieval-diagnostic --pair-spec ...` | Both products search the frozen repo/query pack | Exploratory paired diagnostic; not qualified quality or speed |
| Score five lexical products | `benchctl run lexical-diagnostic --lexical-spec ...` | Re-scores **recorded** Quanta, Semble, Sourcegraph, OpenGrok and cs observations | Corpus-bound recorded file-recall diagnostic; **no live product search** |

There is currently no registered one-command **live five-product** producer.
`lexical-diagnostic` never starts or calls Sourcegraph, OpenGrok or cs. A fresh
five-product comparison requires separately collecting their raw rows first;
do not describe a re-score of old rows as a new search run.

## Shared preparation

1. Record `git rev-parse HEAD` and `git status --porcelain=v1`. `benchctl run`
   and `validate` require a clean source checkout. During concurrent/dirty
   edits, use `list`, `plan` or `just retrieval-contract-local` only; do not
   attribute a historical artifact to the current tree.
2. Keep corpus checkouts, release, suite, query pack, model assets, specs,
   native outputs and evidence **outside this checkout**. Give every capture
   a fresh output root. Do not overwrite an earlier run to retry it.
3. If using a common corpus release, validate it before choosing one repository
   and view. Both products must use its same commit, ordered file/path/SHA
   universe and frozen queries. A release is `frozen_not_admitted`, not proof
   that each product indexed every file.

```sh
python3 tools/benchmark/benchctl.py list
python3 tools/benchmark/benchctl.py plan retrieval-diagnostic
python3 tools/benchmark/benchctl.py plan lexical-diagnostic
python3 tools/benchmark/benchctl.py corpus validate \
  --release /absolute/external/corpus-releases/release-name
```

To create a new release, use `benchctl corpus create --spec ... --checkouts ...
--release ...` as documented in the [corpus section](README.md#shared-external-corpus-releases).
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

First capture one complete raw query row per task for local/remote
Sourcegraph, OpenGrok and cs against the **same selected view** and blind
query pack. Record indexed-universe evidence, submitted query, top-10
repository-relative paths, terminal HTTP/process result, errors and per-query
elapsed time. Keep Quanta/Semble's native pair report, lock, records and
verdict. The checked-in scorer requires the exact row contract in
[`lexical_file_comparison.py`](retrieval/lexical_file_comparison.py); historical
external scripts or an unverified indexed-universe assertion are not a
registered live collector.

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
