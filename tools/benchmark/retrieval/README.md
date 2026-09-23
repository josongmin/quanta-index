# Real-repository retrieval and context-yield benchmark

The Python evaluator scores recorded runner output; it does not query an
index, generate candidates, or infer missing evidence. The benchmark-only
Rust CLI in `benchmarks/retrieval` loads a pinned repository, chunks it,
publishes through the public SDK to a real daemon, and records route results.
The Rust CLI is the Quanta runner; the separate Python `semble.py` adapter
and `run.py` driver implement paired capture and comparison.
Python 3.9 and the standard library suffice for the evaluator; focused tests
use pytest. Runtime validation checks Git, file contents, hashes, split
isolation and complete route coverage.

## Run

Use a clean checkout pinned at the suite's `repository_commit`. Keep the suite,
query pack, runner record and report **outside** that checkout so they do not
change its Git status.

```sh
python3 -m tools.benchmark.retrieval freeze \
  --repo /absolute/clean/repository --suite /absolute/suite.json \
  --output /absolute/query-pack.json

# freeze prints the canonical query_pack_sha256 for the runner record.

# Build searchd and the benchmark CLI through scripts/cargow, then run the
# benchmark CLI against the blind pack. The manifest is JSON with exactly
# repository_commit and files [{path, file_sha256}] for the admitted universe.
# The state root and output must be outside the clean source checkout.
./scripts/cargow --lane bench-lane build -p quanta-index-searchd-runtime \
  --bin quanta-index-searchd --locked
./scripts/cargow --lane bench-lane run -p quanta-index-retrieval-bench \
  --bin quanta-index-retrieval-bench --locked -- run \
  --repo /absolute/clean/repository --manifest /absolute/manifest.json \
  --query-pack /absolute/query-pack.json --strategy brace_heuristic \
  --routes lexical,semantic,hybrid --top-k 20 \
  --repo-id benchmark-repo --revision-id pinned-revision --generation 1 \
  --state-root /absolute/fresh-state-root \
  --searchd-bin /absolute/quanta-index-searchd \
  --searchd-expected-sha256 <lowercase-sha256-of-searchd> \
  --runner-name quanta-sdk --runner-revision pinned-revision --run-id run-1 \
  --blinding attested --isolation-method query-pack-only \
  --access-block-log runner-was-not-given-gold \
  --out /absolute/recorded-run.json

python3 -m tools.benchmark.retrieval evaluate \
  --repo /absolute/clean/repository --suite /absolute/suite.json \
  --runner /absolute/recorded-run.json \
  --baseline-route lexical --candidate-route hybrid \
  --output /absolute/report.json
```

`run` defaults to the pinned `potion-code`/Model2Vec provider.
`--embedder hash-dev` is an explicit development-only correctness control. The CLI derives
model ID and revision from provider-owned constants; caller-supplied model
labels are not accepted. Other provider profiles need a separately verified
provenance path before they can emit benchmark records. The daemon verifies
the local PotionCode model assets on boot; missing or changed assets fail the
run. The CLI refuses dirty or wrong-HEAD repositories, untracked admitted
files, empty query-pack universes, stale non-empty state roots and existing
output files. This CLI only emits `attested` blinding: it cannot prove process
isolation. A separate externally enforced runner/proof path is required before
claiming an `isolated` blinded quality verdict.

The runner computes `query_pack_sha256` as SHA-256 of UTF-8 JSON serialized
with sorted keys, no whitespace, and `ensure_ascii=False`. A producer can use
the same implementation as `evaluator.digest(evaluator.canonical(pack))`.
The evaluator re-derives the pack from the frozen suite and rejects a different
hash. The pack includes suite commitment, repository commit, tokenizer, route
names and only eval task IDs, query text and query hashes. It contains no labels,
answerability bit or train tasks. `freeze` is the only query-pack producer.

## Schema versions

Current authoritative contract is v3 (`schema_version: 3`, see
`suite.schema.json` and `runner.schema.json`). V3 adds a byte-identical
`comparison_contract`, byte-span coverage, per-capture binary/model/generation/
receipt provenance, route-to-capture binding and nullable timing semantics.
V1 and v2 remain readable by the single evaluator only for migration and are
never accepted as v3 pair inputs. V1 still requires at least two routes and
both answerable and no-gold eval tasks. V2/v3 permit an all-answerable external
suite: no no-answer tasks are invented, and the absent stratum reports
`not_applicable`. Suite and runner versions must match.

## Frozen suite and runner v3

The v3 suite requires `comparison_contract` with `top_k`, `tokenizer`,
`tokenizer_budget_version`, `output_unit_policy` and the byte-span unit. The
blind pack echoes that contract and contains no train rows, labels, grades or
answerability bits. The v3 runner record must echo the same contract exactly,
bind each route to one capture, and bind every capture to runner/searchd binary
digests, generation, receipt/activation digests, model identity and one frozen
chunk strategy. Candidate credit is decided by proven byte spans; line spans
are a checked projection.

## Frozen suite v2 (legacy migration)

V2 suite: `schema_version: 2`, `suite_id`, `repository_commit`, at least one
route, `tasks`, and optional `file_universe` (canonical path+SHA allowlist).
When present, every gold label and candidate must come from the allowlist;
a tracked-but-excluded file is rejected. Each task adds optional `category`;
each gold block adds optional `grade` (0-3). Grades enable NDCG@10 only when
every eval answerable gold label carries one. Eval needs at least one task.

## Runner record v2 (legacy migration)

V2 record: `schema_version: 2`, `query_pack_sha256`, `runner`,
`route_provenance`, `results`. `runner` adds `tokenizer_budget_version`
(`qb-v1`), `blinding: isolated | attested`, `isolation_method`, and
`access_block_log`; `gold_access: false` stays an attestation. Authoritative
blinded quality requires `isolated` with separate suite access, otherwise the
claim is attested-only. `route_provenance` carries per-route
`system`/`model`/`model_revision`. Each result has `status` (`success`,
`abstained`, `capped`, `error`, `timeout`, `unavailable`), ordered
`candidates` with 1-based sequential `rank`, `timings`
(`query_latency_ms`, finite, >= 0), and typed `error` (null except for
error/timeout/unavailable). Non-success results carry no candidates and
score zero; `capped` is scored but flagged, never treated as exhaustive.

## Frozen suite v1

Required top-level fields: `schema_version: 1`, `suite_id`, full lowercase
`repository_commit`, at least two distinct `routes`, and `tasks`. Each task has
`task_id`, `split` (`train` or `eval`), `query`, SHA-256 of the UTF-8 query,
`answerable`, and `gold`. Answerable tasks need at least one gold block;
no-gold tasks must have none. The eval split needs both kinds. A gold block
contains a repository-relative POSIX `path`, one-based inclusive `start_line`
and `end_line`, SHA-256 of the complete file bytes and SHA-256 of the selected
line bytes. Line bytes include original line terminators. Gold labels are
checked against actual tracked files at the pinned commit. Duplicate IDs,
queries, labels and train/eval label overlap fail.

## Runner record v1

Required fields: `schema_version: 1`, `query_pack_sha256`, `runner`, `results`.
`runner` contains nonempty `name`, `revision`, `run_id`, `tokenizer` set to
`qi-regex-v1`, and `gold_access: false`. There must be exactly one result for
every eval task and route. A result has `task_id`, `route`, `abstain`, and an
ordered `candidates` list. Abstention requires an empty list; a non-abstaining
result requires candidates. Each candidate has the five gold-block location
and hash fields plus `tokens`. The evaluator recomputes tokens from the exact
block text with regex `[A-Za-z0-9_]+|[^\x00-\x20]` (ASCII identifiers;
other visible code points individually). This is a fixed, auditable
benchmark token unit, **not** a claim that these budgets equal any model's
tokenizer. Calibrate separately before using the results for model-window
sizing. Unknown fields, including any runner-side `gold`, fail.

`gold_access: false` is a runner attestation. The evaluator can verify that
the supplied pack is blind and that the record contains no gold fields; it
cannot independently prove the runner did not open the suite outside this
protocol. Use isolated runner credentials or process sandboxing when that
assurance is needed.

## Metrics and comparison

For 2k, 4k, 8k and 16k benchmark tokens, candidates are packed in rank order.
Packing stops at the first candidate that exceeds the remaining budget.
`BCY@budget` is the fraction of answerable eval tasks whose **every** gold
block is fully covered by packed candidates. `file_recall` and `block_recall`
are task-macro averages. `no_gold_abstention` is the fraction of no-gold tasks
with explicit abstention, or `not_applicable` without a real no-answer
stratum. `mean_context_tokens` is averaged across all eval tasks. The report
gives these metrics for each route and the selected route pair's deltas and
paired win/loss/tie counts (where success means BCY for answerable tasks and
abstention for no-gold tasks). A candidate matching a gold file but missing
its gold line span cannot earn block credit.

V2 introduced, and v3 retains, span-aware Recall@1/5/10/20, MRR@10, graded NDCG@10 (only when every
eval answerable gold label carries a reviewed grade, else `not_applicable`),
file-only recall as a secondary view, and both chunk-level and deterministic
same-file-collapsed rankings (best chunk per file). NDCG credits each gold span
only on its first covering candidate: overlapping chunks cannot earn the same
gain twice or inflate NDCG above 1. The report labels this scoring contract
`rb-rank-v2-first-coverage`; older reports without that marker are not
comparable on overlapping-chunk suites. V2 retains repeated line spans from
distinct chunks in their original ranks; they consume rank and context budget
but earn no second relevance gain. Primary metric is
`ndcg_at_10` when graded, else `recall_at_10`. Reports include per-query rows,
per-route status counts and mean latency, sample counts, and a 95% normal
confidence interval on paired deltas when the sample reaches 20, else an
explicit insufficient-sample marker. Re-scoring immutable records is
deterministic under row order; scores depend only on recorded spans and
statuses, never on runner identity strings.

The report is evidence only for the supplied frozen suite, pinned repository
and runner record. No arbitrary pass threshold or claim of production retrieval
quality is inferred.

## Runners and paired orchestration

`semble.py` runs a pinned Semble install (an outside-the-checkout virtualenv)
against the admitted manifest and a projected query pack, then normalizes
native hits into a v2 record with proven spans. It always emits the
path-mapping proof (path map plus both-side path+SHA diff); any skipped or
extra file fails the common-universe pair with a typed reason. `run.py`
drives the Rust SDK runner per chunking strategy (`quanta`), runs both
systems sequentially from a pinned spec (`pair`), deterministically merges
per-system records (`merge`), re-scores immutable records into the TEST-PLAN
§8 verdict artifact (`verdict`), and records the host check (`host-probe`).

Each system consumes a projected pack (same tasks/universe/commit, narrowed
routes, rebound suite commitment); the merge re-derives and re-validates
every projection with this evaluator before scoring. Records, corpora,
caches and reports stay under an explicit output root outside the checkout.
See `docs/plans/sep-23-retrieval-bench/tickets/` for the protocol and the
`just retrieval-*` recipes for the registered entry points.

## Pair spec schema

`run.py pair --spec SPEC` (and `run.py quanta`) take a JSON spec. Required
keys: `repo`, `manifest`, `suite`, `query_pack`, `top_k`, `output_root`,
`runner_binary`. Optional keys and defaults:

| Key | Default | Meaning |
| --- | --- | --- |
| `routes` | `["lexical","semantic","hybrid"]` | Quanta routes (must be suite routes) |
| `strategies` | required for `quanta`/`pair` | e.g. `[{"name":"whole_file"},{"name":"brace_heuristic"}]` |
| `searchd_binary` | required | explicit daemon pin; unpinned capture is refused |
| `embedder` | `potion-code` | Rust runner embedder profile (`hash-dev` is an explicit diagnostic control) |
| `repo_id`/`revision_id`/`generation` | `bench-repo`/`bench-rev`/`7` | batch identity |
| `runner_name`/`run_id` | `quanta-sdk-runner`/`run` | runner identity; `runner_revision` is derived from the binary SHA-256 |
| `blinding` | `attested` | only `attested` is supported today |
| `isolation_method`/`access_block_log` | `attested-only…` | blinding evidence text |
| `semble_python` | required for `pair` | pinned Semble venv interpreter |
| `semble_lockfile` | required for `pair` | external hash-pinned lockfile path (frozen into the stage) |
| `semble_lockfile_sha256` | required for `pair` | SHA-256 of the external lockfile; env must carry every locked line plus `semble==0.6.0` |
| `semble_route` | `semble-hybrid` | Semble record route name |
| `semble_cache_root` | `<out>/semble-cache` | Semble + HF caches (outside checkout) |
| `semble_repetitions`/`seed` | `1`/`0` | worker query sampling (1 untimed warmup pass) |
| `semble_model_revision` | observed | pinned HF revision (drift fails) |
| `repetitions` | `1` | external reps on fresh state |
| `alternate_order` | `true` | alternate system order per rep |
| `order` | `["quanta","semble"]` | base system order |
| `baseline_route` | Semble route | report baseline |
| `scope` | `exploratory` | `exploratory` or `qualified` |
| `claims` | all `false` | `{quality,speed,same_model,incremental}` |
| `evidence` | `{}` | external contract/SDK receipts; cannot override driver-observed `pair` or `perf` |
| `timeout_secs` | `1800` | per-capture timeout |

Before a paired run, author a hash-pinned external lockfile for the exact
Semble virtualenv, put its path in `semble_lockfile` and its SHA-256 in
`semble_lockfile_sha256`, then preflight the env (the observed `pip freeze`
must carry every locked line; the recorded `installed_distribution`
RECORD/direct_url digests identify the installed bytes):

```sh
python3 -m tools.benchmark.retrieval.semble check \
  --python /absolute/semble-venv/bin/python
```

This pins the installed package inventory; it does not prove which model
weights Semble loaded. The adapter sets Semble's documented
`SEMBLE_MODEL_NAME` to the requested model and verifies the worker-reported
setting. It also requires an observed Hugging Face
cache revision and rejects a supplied revision that disagrees with it.
`same_model` remains an external claim needing its own evidence. The paired
driver records `attested` blinding, so its output alone cannot qualify an
isolated-blind quality or phase-qualified speed verdict.

Notes: the first Semble index includes the model download (later runs reuse
the cache; `index_stats` and `semble_index_ms` always record what ran).
Partial output is never resumed — rerun from a fresh output root. For
`just retrieval-verdict`, pass space-separated record paths as one quoted
`records` argument.

## T00–T16 evidence map

Blocking IDs map to a test target, command and artifact. `NOT_RUN` means no
evidence exists yet; conditional IDs apply only when the claim is made.

| ID | Target | Command / artifact |
| --- | --- | --- |
| T00 | `benchmarks/retrieval` corpus loader + `semble.py` mapping proof | `just retrieval-contract-proof <fresh-output-root>`; `mapping-proof.json` (path map + both-side path+SHA diff) |
| T01 | `tools/ci/tests/test_retrieval_benchmark.py` (runner independence) | `just retrieval-contract-proof <fresh-output-root>`; tampered-record mutants must fail |
| T02 | split-leakage custody: duplicate/near-duplicate queries, query families and answer-span overlap | `just retrieval-contract-proof <fresh-output-root>`; explicit both-sides allowlist mutants |
| T03 | same (schema rejection: fields/routes/timings/status) | `just retrieval-contract-proof <fresh-output-root>` |
| T04 | byte-span Recall/MRR/NDCG/BCY scoring and deterministic same-file collapse | `just retrieval-contract-proof <fresh-output-root>`; hand-calculated coverage/budget mutants |
| T05 | `sdk_roundtrip.rs` process/frontdoor | `just retrieval-sdk-proof <fresh-output-root>`; pinned actual runner + separate daemon, readiness and empty-state checks |
| T06 | same, SDK write authority | sealed receipt + exact composite activation ACK; direct IPC/fixture helpers refused by static guard |
| T07 | same, SDK read authority | lexical/semantic/hybrid SDK reads, generation pin and typed failure behavior |
| T08 | `benchmarks/retrieval/tests/chunking_contract.rs` | `just benchmark-prep-local` (chunking contract, no daemon) |
| T09 | same (oracle cases + fallback accounting) | `just benchmark-prep-local` |
| T10 | per-strategy generation/capture/model binding plus deterministic replay | `just retrieval-sdk-proof <fresh-output-root>`; `just benchmark-prep-local`; real ablation evidence remains unrun |
| T11 | `semble.py` mapping proof + adapter tests | `mapping-proof.json`; `just benchmark-prep-local` |
| T12 | `run.py pair` (same universe/host) + `host.json` | `just retrieval-pair <spec>`; NOT_RUN until a frozen pilot |
| T13 | `run.py verdict` (deterministic re-score) | command implemented and fixture-tested; no real-pair `verdict.json` issued |
| T14 | registered commands and external artifact root | `just benchmark-prep-local`, `just retrieval-contract-proof <fresh-output-root>` and `just retrieval-sdk-proof <fresh-output-root>`; none downloads Semble/model assets implicitly |
| T15 | model parity (conditional on a same-model claim) | NOT_RUN (no same-model claim; `model_revision` recorded per run) |
| T16 | incremental capture (conditional on an incremental claim) | NOT_RUN (no incremental claim) |

Current closeout blockers are explicit: the contract/SDK receipt recipes must
be run and frozen from one clean source; the generic workspace rail must record
a frozen-source GREEN after its new explicit searchd-pin setup;
Quanta capture is attested-only; and `run.py` hard-fails every speed claim as
`phases_unimplemented` because phase fragments and process-tree peak RSS are
not implemented. The commands above prove code paths, not W0-B, `PAIR_VALID`,
`PERF_QUALIFIED` or `QUALITY_DELTA`.
