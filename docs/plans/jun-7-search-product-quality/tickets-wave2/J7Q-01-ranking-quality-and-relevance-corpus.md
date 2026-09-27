# J7Q-01 — Relevance and external lexical comparison

Status: `ACTIVE_RESIDUAL`
Parent: [quality index](INDEX.md)
Owner: lexical/symbol/structural/history ranking and benchmark evaluator

The judged seeded fixture, per-route MRR@10/NDCG@10/Recall@20 and top-1,
top-k/hard-negative invariants are implemented in
`crates/quanta-index-searchd-harness/src/relevance`. Recreating a relevance
producer is not the remaining task. The current external overlap output is
explicitly `unprovisioned`; it is not a competitive-quality result.

## Remaining acceptance

- Independently judge stable query IDs and graded labels; include near-duplicate
  distractors and hard negatives. Keep deterministic tie-break checks separate.
- Measure lexical, symbol, structural and history-backed families independently;
  retain per-query rankings, top-1, top-k containment and forbidden high ranks.
- Execute a real Sourcegraph lexical comparison on overlapping keyword, phrase,
  regex, constrained-content, repo-metadata and symbol-name surfaces. Bind exact
  queries, corpus/revision, native observations, order and explicit gaps.
- Keep macro/route denominators visible; a stable but poor ranking or a blended
  average cannot hide a route regression. Fixture floors are not external gold.

Output owner: `relevance_matrix`, registered under `quality-core/quality-full`.
The retained `summary.json`, `query_judgments.json` and `sourcegraph-overlap.json`
are interpreted under current artifact admission, not old README status labels.
Independent corpus/gold/holdout and native response acceptance are owned by
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md)
and [CS-BENCH-02](../../sep-27-code-search-remediation/rfcs/CS-BENCH-02-native-response-validation.md).
No semantic/hybrid or comparative SOTA claim is closed by this ticket alone.
