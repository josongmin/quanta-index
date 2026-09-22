# Real-repository retrieval and context-yield benchmark

This evaluator scores an externally recorded runner output. It does not query
the index, generate candidates, or infer missing evidence. Python 3.9 and the
standard library are sufficient for the evaluator; the focused tests use
pytest. The two versioned JSON schemas document the exact wire contracts.
Runtime validation additionally checks Git, file contents, hashes, split
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

# Supply query-pack.json to a separate retrieval runner. The runner must emit
# a recorded JSON file conforming to runner.schema.json. No gold is in the pack.

python3 -m tools.benchmark.retrieval evaluate \
  --repo /absolute/clean/repository --suite /absolute/suite.json \
  --runner /absolute/recorded-run.json \
  --baseline-route lexical --candidate-route hybrid \
  --output /absolute/report.json
```

The runner computes `query_pack_sha256` as SHA-256 of UTF-8 JSON serialized
with sorted keys, no whitespace, and `ensure_ascii=False`. A producer can use
the same implementation as `evaluator.digest(evaluator.canonical(pack))`.
The evaluator re-derives the pack from the frozen suite and rejects a different
hash. The pack includes suite commitment, repository commit, tokenizer, route
names and only eval task IDs, query text and query hashes. It contains no labels,
answerability bit or train tasks. `freeze` is the only query-pack producer.

## Schema versions

Current contract is v2 (`schema_version: 2`, see `suite.schema.json` and
`runner.schema.json`). V1 suites and runner records remain readable by the
same single evaluator for migration; v1 still requires at least two routes
and both answerable and no-gold eval tasks. V2 permits an all-answerable
external suite: no no-answer tasks are invented, and the absent stratum
reports `not_applicable`. Suite and runner versions must match.

## Frozen suite v2

V2 suite: `schema_version: 2`, `suite_id`, `repository_commit`, at least one
route, `tasks`, and optional `file_universe` (canonical path+SHA allowlist).
When present, every gold label and candidate must come from the allowlist;
a tracked-but-excluded file is rejected. Each task adds optional `category`;
each gold block adds optional `grade` (0-3). Grades enable NDCG@10 only when
every eval answerable gold label carries one. Eval needs at least one task.

## Runner record v2

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

V2 adds span-aware Recall@1/5/10/20, MRR@10, graded NDCG@10 (only when every
eval answerable gold label carries a reviewed grade, else `not_applicable`),
file-only recall as a secondary view, and both chunk-level and deterministic
same-file-collapsed rankings (best chunk per file). Primary metric is
`ndcg_at_10` when graded, else `recall_at_10`. Reports include per-query rows,
per-route status counts and mean latency, sample counts, and a 95% normal
confidence interval on paired deltas when the sample reaches 20, else an
explicit insufficient-sample marker. Re-scoring immutable records is
deterministic under row order; scores depend only on recorded spans and
statuses, never on runner identity strings.

The report is evidence only for the supplied frozen suite, pinned repository
and runner record. No arbitrary pass threshold or claim of production retrieval
quality is inferred.
