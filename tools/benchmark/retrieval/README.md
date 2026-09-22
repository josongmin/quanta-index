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
answerability bit or train tasks.

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
with explicit abstention. `mean_context_tokens` is averaged across all eval
tasks. The report gives these metrics for each route and the selected route
pair's deltas and paired win/loss/tie counts (where success means BCY for
answerable tasks and abstention for no-gold tasks). A candidate matching a
gold file but missing its gold line span cannot earn block credit.

The report is evidence only for the supplied frozen suite, pinned repository
and runner record. No arbitrary pass threshold or claim of production retrieval
quality is inferred. The current contract has no query execution latency,
external task QA, human graded utility, or agent outcome measurement.
