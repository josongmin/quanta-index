# J7Q-01 — Relevance and external lexical comparison

Status: `ACTIVE_RESIDUAL`
Parent: [quality index](INDEX.md)
Owner: lexical/symbol/structural/history ranking and benchmark evaluator

The judged seeded fixture, per-route MRR@10/NDCG@10/Recall@20 and top-1,
top-k/hard-negative invariants are implemented in
`crates/quanta-index-searchd-harness/src/relevance`. Recreating a relevance
producer is not the remaining task. The current external overlap output is
explicitly `unprovisioned`; it is not a competitive-quality result.

The frozen gin 300 bare-symbol lexical diagnostic is separate from that
unprovisioned qualified overlap: Quanta returned the generated gold file in
295/300 chunk-top-10 responses. All five missed gold texts were published in
the lexical generation. A two-query same-binary control placed `S040` at chunk
rank 12 (fourth distinct file) and `S273` at chunk rank 13 (twelfth distinct
file). This confirms top-k/rank-unit pressure in those cases; it does not prove
a lexical candidate-generation correctness defect or validate the generated
declaration-only labels as general content-search relevance.

At local `main@9c880476`, the exact-symbol searcher and dispatcher tests for
case-sensitive names plus a typed file anchor each pass. The earlier native
300/300 exact-definition-span result is bound to `a9a43529` and a mechanically
derived, unreviewed declaration suite. The latest HEAD has no new gin native
300-query result. Keep declaration lookup, file-ranked lexical search, and
chunk content retrieval as separate result units and acceptance tracks.

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
- For exact-name requests, verify the explicit symbol route through a current
  source-bound consumer capture and reviewed declaration identities; do not
  infer automatic bare-query routing from searcher/dispatcher unit tests.

Output owner: `relevance_matrix`, registered under `quality-core/quality-full`.
The retained `summary.json`, `query_judgments.json` and `sourcegraph-overlap.json`
are interpreted under current artifact admission, not old README status labels.
Independent corpus/gold/holdout and native response acceptance are owned by
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md)
and [CS-BENCH-02](../../sep-27-code-search-remediation/rfcs/CS-BENCH-02-native-response-validation.md).
No semantic/hybrid or comparative SOTA claim is closed by this ticket alone.
