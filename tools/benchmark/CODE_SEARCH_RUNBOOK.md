# Code-search benchmark runbook

Run commands from the repository root. Use the [benchmark guide](README.md)
for other profiles and the [retrieval guide](retrieval/README.md) for native
runner options. Evidence and scoring policy lives in
[SEP-26-003](../../docs/adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md)
and [SEP-27-004](../../docs/adr/SEP-27-004-benchmark-capture-and-resource-custody.md).

## Choose the rail first

| Goal | Command/owner | What actually runs | Maximum claim |
| --- | --- | --- | --- |
| One-repository 100-query smoke | One five-product lexical workflow plus semantic and hybrid pairs on the same frozen repository | All five lexical products; Quanta and Semble in all three modes; native setup/index and query timings | One-repository diagnostic only |
| Complete code-search comparison (default) | Every repository × query-family cell in [Complete comparison](#complete-comparison-is-the-default) | Five live lexical products plus Quanta--Semble lexical, semantic and hybrid pairs | Full diagnostic matrix only after all cells validate and replay |
| Check retrieval code while editing | `just retrieval-contract-local` | Focused Python/Rust contract tests | Dirty-checkout diagnostic; no search quality or speed |
| Capture current-source SDK/contract proof | `benchctl run retrieval-contract` | Real daemon SDK proof and contract tests | Typed correctness proof; no relevance or speed |
| Compare Quanta with Semble | `benchctl run retrieval-diagnostic --pair-spec ...` | Both products search the frozen repo/query pack | Exploratory paired diagnostic; not qualified quality or speed |
| Run all five lexical products | `benchctl code-search run --spec ...` | External live capture, live SDK pair, five-product score, validation and replay | Fresh five-product diagnostic; no qualified quality or speed |
| Verify the declared complete matrix | `benchctl code-search matrix-verify --spec ...` | Revalidates the release and every declared pair/workflow capture, then checks all repository × family × mode cells | Complete declared diagnostic matrix; no independent gold, speed or indexed-universe claim |
| Capture the three external lexical products | `benchctl code-search external --spec ...` | Live Sourcegraph/OpenGrok HTTP requests and cs processes; retains native responses | Fresh external recorded diagnostic; indexed-universe attestation remains absent |
| Score five lexical products | `benchctl run lexical-diagnostic --lexical-spec ...` | Re-scores **recorded** Quanta, Semble, Sourcegraph, OpenGrok and cs observations | Corpus-bound recorded file-recall diagnostic; **no live product search** |

The `code-search` workflow composes the existing registered pair and lexical
profiles with live external collection over one frozen corpus view and query pack.
`lexical-diagnostic` itself never starts or calls Sourcegraph, OpenGrok or cs.
The workflow validates one lexical input per invocation. `matrix-verify` checks
the complete Cartesian product of the release repository inventory and the
matrix spec's declared query-family inventory. The family list is an input
declaration, not an independently discovered benchmark population. Freeze and
review it before capture; a verifier cannot discover families omitted from its
own declaration.

## One-repository smoke: 100 queries

For a smoke request, select **one** frozen repository/view and one 100-task
bare-symbol query set. Use the same 100 task IDs, query bytes and gold-file
projection in lexical, semantic and hybrid packs. Run exactly one
`code-search run` for the five-product lexical row and one
`retrieval-diagnostic` pair each for semantic and hybrid. Validate and replay
all three captures. Do not run the full repository matrix for a smoke request.
In the report, name the selected repository and exact commit once for all five
products. Check that the native pair and external spec bind to the same release
view, manifest, suite and query pack. Distinguish this declared input binding
from proof of the files actually indexed by Sourcegraph and OpenGrok: when
backend index-universe attestation is absent, label a fair five-product
comparison `BLOCKED` and keep the observed rows explicitly diagnostic.

Use `repetitions: 1`, `query_warmup_passes: 1` and
`query_repetitions_per_root: 1` for the basic 100-query smoke. Each native
mode currently creates its own fresh Quanta and Semble index over the **same
repository**; the 100 measured queries in that mode reuse the loaded index.
If repeated warm samples are requested, increase only
`query_repetitions_per_root` before capture. A separate invocation cannot
reuse the exited native workers.

Prepare three pair specs and one lexical workflow spec outside the checkout.
The workflow spec points to the lexical pair and external specs. Use fresh
output and evidence roots for each command:

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py code-search run \
  --spec /absolute/external/smoke/workflow.json
uv run --frozen --extra dev python tools/benchmark/benchctl.py run retrieval-diagnostic \
  --pair-spec /absolute/external/smoke/semantic-pair.json \
  --evidence-root /absolute/external/smoke/semantic-evidence
uv run --frozen --extra dev python tools/benchmark/benchctl.py run retrieval-diagnostic \
  --pair-spec /absolute/external/smoke/hybrid-pair.json \
  --evidence-root /absolute/external/smoke/hybrid-evidence
```

The lexical workflow performs its own validation and replay. Run
`validate retrieval-diagnostic` and `replay --family retrieval-pair` on each
of the semantic and hybrid evidence roots as shown below in section A.

Report **one table** with Quanta, Semble, Sourcegraph, cs and OpenGrok as
columns. Under each mode, use separate rows for (1) gold files found in the
top ten with the within-mode rank, (2) index-build seconds, (3) query p50/p95
milliseconds, and (4) chunk NDCG@10 where measured. When each task has one
gold file, show recall as an intuitive count such as `98/100`; otherwise show
the file-recall fraction and denominator. Explain that p50 is the median and
p95 is the value at or below which 95% of measured calls fall. `NOT_RUN` means
an applicable measurement was not made; `N/A` means the product has no route
or persistent index for that row. Do not replace either with zero.
Label recall ranks as observed diagnostic ranks: Quanta/Semble rank the first
ten chunks, while the three external products return up to ten distinct files.
The scorer explicitly excludes native rank equivalence. A shared corpus and
query pack do not remove this result-unit difference. Chunks versus files are
different result units, not different input repositories.
For Quanta, report `chunk + embed_publish_seal_activate` from phase metrics;
also retain discovery, symbol preflight and daemon boot separately so the
index number is not confused with full readiness. For Semble, report its
native `index` phase; retain discovery and model preparation separately.
Sourcegraph/OpenGrok use pre-existing indexes in this workflow, so their
index-build time is `NOT_RUN`; `cs` has no persistent index here (`N/A`).
Do not substitute workflow wall time or an old service uptime for an external
index-build measurement. External products remain `N/A` in semantic/hybrid.
The timing boundaries differ by product, so do not rank index or query speed
across products from this diagnostic smoke.

## Complete comparison is the default

When asked to run a code-search benchmark, run every applicable cell below. Do
not stop after a Quanta--Semble pair or after rescoring recorded lexical rows.
For each repository and mode query family, freeze the same commit,
manifest/file universe, task IDs, query bytes, gold labels and comparison
contract across the Quanta--Semble modes. Route-specific suite and pack bytes
may differ because each declares its own routes; compare their common task
projection rather than requiring equal suite/pack digests across modes.
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

### Index setup versus repeated queries

The native `run` path builds one Quanta index and one in-memory Semble index
per repository, mode and fresh root. After indexing, it sends the entire
warmup and measurement schedule to those loaded indexes. It does **not**
reindex for every query. Sourcegraph and OpenGrok search their already-running
indexes; the external collector does not rebuild them. `cs` starts a search
process per query, so its wall time includes process startup.

For repeated-query timing on the **same loaded index**, set
`repetitions: 1`, `query_warmup_passes: 1`, and
`query_repetitions_per_root: 10` in each pair spec before running the batch.
With 100 tasks this yields 100 warmup calls and 1,000 measured calls per
route after one index build per product/root. The randomized schedule and
per-query timings are recorded; report index/setup time separately from
query p50/p95. Repeated observations are not new distinct queries. Qualified
speed still requires the separate multi-root protocol in the retrieval guide.

A **new** `benchctl code-search run` or `retrieval-diagnostic` invocation
currently creates fresh Quanta and Semble indexes. The runner explicitly
rejects nonempty state roots, and the Semble worker holds its index only in
memory until it exits. Saved capture files are evidence, not a reusable live
search session. Therefore do not rerun the full five-product workflow merely
to obtain more timing samples from the same queries: choose the warm-query
repetition count before capture. A later query pack cannot be sent to those
terminated native workers without a separate persistent-session feature.

After capture, write an external matrix spec with `schema_version: 1`, the
absolute `release_path`, its validated `release_digest`, a nonempty unique
`query_families` list, and one `cells` entry per repository/family. Each cell
contains `repository`, `view`, absolute `suite` and `query_pack` paths, and a
`captures` object with `lexical-only`, `semantic-only` and `hybrid` keys. Each
mode contains an absolute `root` and `kind` (`workflow` for a supported
bare-symbol lexical cell, otherwise `pair`). Roots cannot be reused. The
verifier checks exact source, native input bytes, corpus/query binding,
exploratory claims, route and Semble execution mode, then replays each capture.

```sh
uv run --frozen --extra dev python tools/benchmark/benchctl.py code-search matrix-verify \
  --spec /absolute/external/code-search-matrix.json
```

The result is `diagnostic_unqualified`. It cannot qualify independent labels,
whole-product indexed scope, latency, or a default-policy change.

Within each repository/query family and mode, compare products only when commit,
manifest/file universe, suite and query pack match. Across modes, require the
same task/query/gold projection. Across repositories, report
the aggregation method and retain each run's universe binding. The five-product
lexical workflow and the mode pairs can each be internally matched while still
covering different corpus views; never merge their scores into one overall row.
External search products are `N/A` for semantic/hybrid
modes and for query families rejected by the bare-symbol scorer, with the reason
shown in the table.

### Required result table

After each run, show **one table** with the five products in this order:
Quanta, Semble, Sourcegraph, cs, OpenGrok. Group rows by corpus view, query
family and mode; use a separate row for each metric instead of packing several
abbreviations into one product cell. Compare products within a mode only when its repository
commits, manifests, suite and query pack match. Across repositories, declare
the weighting and retain the per-repository manifest digests. Use `N/A` for an
unsupported route or query form, `NOT_RUN` for an applicable missing capture,
and `FAILED` for an executed failure. Never render missing values as zero.
For every executed product, include file recall@10 with its within-mode rank,
query latency p50/p95 and index-build time when measured; also include
NDCG@10 where measured. Explain each metric in plain language immediately
under the table and name each product's timing layer. The current
five-product capture records runner SDK query calls for Quanta, worker search
dispatch for Semble, HTTP request wall time for Sourcegraph/OpenGrok, and
process spawn plus search wall time for cs. Those numbers are descriptive;
do not rank cross-product speed until a common timing boundary and qualified
host protocol are available. Report absent latency as `NOT_RUN`, not zero.
Native Quanta/Semble file recall uses paths present in ten returned chunks;
external recall uses up to ten distinct returned files. These observed ranks
must not be described as a fair product-level file-ranking comparison.

### Current 100-query diagnostic (2026-09-28)

Source HEAD: `c64f6af5d5e2817e346336ceee0e57f5fb25d74f` from a clean
worktree. Ten pinned repositories each supply 100 bare-symbol tasks: the prior
20 symbols plus 80 deterministic Tree-sitter function declarations. There are
1,000 task rows and 997 distinct query strings across repositories. The same
100 task/query/gold projection is used for lexical, semantic and hybrid modes
within each repository. Every completed lexical row is a fresh five-product
workflow with validation and replay. Values aggregate one query per repository
per task; every repository has equal weight. `R` is file recall@10; `N` is
chunk NDCG@10; the two times are query latency **p50/p95** in milliseconds.
Ranks in parentheses use `R` within the row. Speed is intentionally unranked.

| Input universe / cohort | Query | Mode | Quanta | Semble | Sourcegraph | cs | OpenGrok | Tasks/product |
| --- | --- | --- | --- | --- | --- | --- | --- | ---: |
| `code_only`, all 10 | bare symbol | lexical | R 0.981 (#4); N 0.0159; 4.89/16.73 ms | R 0.976 (#5); N 0.0830; 0.229/1.50 ms | R 0.989 (#2); 179.81/301.20 ms | R 0.990 (#1); 35.21/70.89 ms | R 0.985 (#3); 11.21/30.39 ms | 1000 |
| `code_only`, common 9 | bare symbol | lexical | R 0.983 (#4); N 0.0153; 4.76/15.99 ms | R 0.978 (#5); N 0.0814; 0.200/1.45 ms | R 0.992 (#1); 178.51/234.79 ms | R 0.991 (#2); 34.38/64.66 ms | R 0.989 (#3); 10.73/30.51 ms | 900 |
| `code_only`, common 9 | bare symbol | semantic | R 0.748 (#2); N 0.0092; 3.73/5.35 ms | R 0.831 (#1); N 0.0536; 0.095/0.395 ms | `N/A` | `N/A` | `N/A` | 900 |
| `code_only`, common 9 | bare symbol | hybrid | R 0.980 (#2); N 0.0155; 9.75/33.76 ms | R 0.989 (#1); N 0.0926; 8.75/53.82 ms | `N/A` | `N/A` | `N/A` | 900 |
| `code_only`, trpc | bare symbol | semantic | `FAILED` | `FAILED` | `N/A` | `N/A` | `N/A` | 100 attempted; 0 scored |
| `code_only`, trpc | bare symbol | hybrid | `FAILED` | `FAILED` | `N/A` | `N/A` | `N/A` | 100 attempted; 0 scored |

All-ten lexical uses 10/10 validated five-product workflows (1,000 queries
per product). The common-nine rows exclude trpc so the three modes use the
same repository cohort. Semantic and hybrid each have 9/10 validated and
replayed pairs; trpc failed at task S04 with duplicate candidate byte spans at
4 KiB and also failed on a fresh 16 KiB retry. Both trpc mode cells remain
`FAILED` rather than zero. External products have no semantic/hybrid route in
this protocol, so their mode cells are `N/A`.

OpenGrok's original ten projects indexed only 1,492 of 6,477 admitted files.
The displayed lexical score uses reindexed projects
whose path inventories match every admitted manifest. A `full` OpenGrok probe
also checked served bytes for axios, black and fastapi. For tokio and vite,
the `/file/content` API returned 404 for an indexed/searchable file; for
eslint it returned 400 for a path containing braces. Those three workflows
were rerun without the full probe after exact path-inventory checks. The
remaining four projects have exact path-inventory checks but no full served-byte
probe. Thus the OpenGrok input byte universe is not fully attested across all
ten. Sourcegraph indexed-universe attestation is also absent. Gold labels are
mechanically generated and not independently adjudicated. The snapshot is
`diagnostic_unqualified` and does not support a qualified product ranking.

Latency boundaries differ: Quanta is runner SDK query-call time; Semble is
worker search-dispatch time; Sourcegraph and OpenGrok are loopback HTTP request
wall time; cs includes process spawn and search. The p50/p95 values describe
those captured calls, with differing setup and protocol overhead. They are
not a cross-product speed benchmark or speed rank.
The current 100-query specs used one warmup pass and one measured pass per
fresh root. Their displayed query timings exclude Quanta/Semble indexing, but
the full matrix runtime includes fresh index builds for each native mode run.

The following **2026-09-28 20-query diagnostic snapshot** is historical and
has no captured speed column. Use the current 100-query snapshot above for the
latest result table.
Source HEAD: `071c6fee987b41a9aa66a0a0928b0e22476b3177`. Each repository
has 20 tasks per query family; aggregate values give every repository equal
weight. Quanta and Semble cells are **chunk NDCG@10 / file recall@10**; the
three external cells are **file recall@10** only. `common 9` excludes trpc,
whose `code_only` semantic capture was rejected by the evaluator. The all-ten
lexical row still includes trpc's five live lexical products.

| Input file universe / cohort | Query | Mode | Quanta | Semble | Sourcegraph | cs | OpenGrok | Tasks per product |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `code_only`, all 10 | bare symbol | lexical | 0.0193 / 0.985 (#4) | 0.0837 / 0.970 (#5) | 0.995 (joint #1) | 0.990 (#3) | 0.995 (joint #1) | 200 |
| `code_only`, common 9 | bare symbol | lexical | 0.0188 / 0.9889 (joint #3) | 0.0793 / 0.9722 (#5) | 0.9944 (joint #1) | 0.9889 (joint #3) | 0.9944 (joint #1) | 180 |
| `code_only`, common 9 | bare symbol | semantic | 0.0122 / 0.7222 (#2) | 0.0521 / 0.8056 (#1) | `N/A` | `N/A` | `N/A` | 180 |
| `code_only`, common 9 | bare symbol | hybrid | 0.0196 / 0.9778 (#2) | 0.0987 / 0.9944 (#1) | `N/A` | `N/A` | `N/A` | 180 |
| `code_only`, trpc | bare symbol | lexical | 0.0247 / 0.950 (joint #4) | 0.1238 / 0.950 (joint #4) | 1.000 (joint #1) | 1.000 (joint #1) | 1.000 (joint #1) | 20 |
| `code_only`, trpc | bare symbol | semantic | `FAILED` | `FAILED` | `N/A` | `N/A` | `N/A` | 20 attempted; 0 scored |
| `code_only`, trpc | bare symbol | hybrid (16 KiB) | 0.0243 / 0.950 (joint #1) | 0.0834 / 0.950 (joint #1) | `N/A` | `N/A` | `N/A` | 20 |
| candidate root, all 10 | bare symbol | lexical | 0.0421 / 0.985 (joint #1) | 0.0881 / 0.985 (joint #1) | `NOT_RUN` | `NOT_RUN` | `NOT_RUN` | 200 |
| candidate root, all 10 | bare symbol | semantic | 0.0295 / 0.815 (#2) | 0.0636 / 0.870 (#1) | `N/A` | `N/A` | `N/A` | 200 |
| candidate root, all 10 | bare symbol | hybrid | 0.0455 / 0.990 (#1) | 0.0968 / 0.985 (#2) | `N/A` | `N/A` | `N/A` | 200 |
| candidate root, all 10 | natural language | lexical | 0.0092 / 0.370 (#2) | 0.0863 / 0.960 (#1) | `N/A` | `N/A` | `N/A` | 200 |
| candidate root, all 10 | natural language | semantic | 0.0222 / 0.670 (#2) | 0.0536 / 0.755 (#1) | `N/A` | `N/A` | `N/A` | 200 |
| candidate root, all 10 | natural language | hybrid | 0.0235 / 0.710 (#2) | 0.0721 / 0.975 (#1) | `N/A` | `N/A` | `N/A` | 200 |

**Read the table:** Higher scores are better. File recall@10 is the fraction of
gold files found in the first ten results; NDCG@10 also rewards ranking of
gold chunks. Parenthesized ranks use file recall@10 among executed products in
the **same row**; equal scores share a rank and the next rank skips places.
Ranks do not use NDCG or compare different cohorts, input universes or modes.
Compare all five file-recall values only in the `code_only` lexical rows.
External-product NDCG was not measured. `NOT_RUN` means the three external
products were not queried on that candidate-root manifest; their `code_only`
scores cannot fill those cells. `N/A` means the current external protocol has
no semantic/hybrid route or does not admit natural-language lexical queries.

**Mode input scope:** `code_only` is the broad code-file view used by all five
live lexical products and the new Quanta--Semble semantic/hybrid pairs.
Candidate root is a smaller frozen benchmark-file subset used by the earlier
Quanta--Semble three-mode matrix. Bare-symbol queries use identifier text;
natural-language queries use prose for the same gold target. Both families
contain 20 tasks per repository. The two file universes have different
manifests even at the same repository commits, so their scores are separate
tracks. Lexical, semantic and hybrid select different retrieval routes; they
do not change a row's file universe. `code_only` uses 4096-byte strict windows
except the trpc hybrid retry, which used 16384 bytes after 4096-byte chunks
projected to duplicate byte spans. trpc semantic still failed because two
native Semble hits projected to the same line span. Do not treat the trpc
lexical/hybrid scores as a pure mode ablation or include trpc in a three-mode
aggregate until that failure is fixed and rerun.

Execution coverage: 10/10 live five-product `code_only` lexical workflows;
20 Quanta--Semble `code_only` semantic/hybrid cells attempted, 18 passed at
4096 bytes, one trpc hybrid cell passed on a fresh 16384-byte retry, and trpc
semantic remains `FAILED` after both settings. The earlier candidate-root
matrix completed 60/60 pairs. Every passing new pair completed `run`,
`validate` and `replay` with 40/40 selected/executed/passed verdict cases.
The snapshot remains diagnostic: gold labels were mechanically generated and
not independently adjudicated; external indexed-universe attestation and
qualified cross-product latency are absent. A bare-symbol lexical row is
complete only when all five products ran on the same manifest, suite and
query pack.

Include a compact execution summary with expected/completed workflows, paired
results, live task rows per external product, validation/replay counts, source
HEAD and the diagnostic/qualification boundary. Keep latency out of cross-product
rankings unless a separate qualified speed protocol passes.

## Lexical workflow command (repeat for every repository × supported bare-symbol family)

Use the [workflow spec example](retrieval/examples/code-search-workflow.json)
after preparing the pair and external specs below. The workflow refuses unequal
suite/pack bytes, a different release manifest, non-lexical profiles, dirty source
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
Set OpenGrok `indexed_view_probe` to `full` for a five-product comparison. This
checks the project's indexed path inventory against the release manifest and
checks every served file's bytes before accepting the capture. A result from a
partial OpenGrok project may be retained as a diagnostic, but must not be
ranked as an equal-indexed-universe product comparison. The probe does not
attest Lucene term freshness; retain that qualification limit.
Sourcegraph's Git origin must contain the selected original commit; a new
synthetic snapshot commit is refused. Its request uses a short file-extension
filter to stay below the request-target limit, then validates and filters the
full native stream against the selected-view manifest while preserving hit
order. It records out-of-manifest matches separately. An 8 KiB request-target
preflight runs before any live product request. cs must be an executable
binary. The collector retains each raw HTTP response and cs stdout/stderr,
checks terminal results, and emits exactly one `symbol_only` row per task and
product. Failed or partial captures stay in `.staging` and do not publish the
final output root:

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
