# RB-01 — Frozen Suite and Single Scoring Authority

Status: `planned`

Depends on: RB-00 stage A

Owner: retrieval evaluator and suite schema

## Goal

Extend the existing real-repo retrieval evaluator so one independent oracle scores Quanta strategies and Semble on exact pinned source, without granting labels to either runner.

## Work

1. Keep `freeze` as the only query-pack producer. Its output contains eval IDs, queries and route/config identifiers, never gold spans or grades. The record contract carries `blinding: isolated | attested` plus `isolation_method` and `access_block_log`; runner-side `gold_access: false` remains an attestation, not proof of process isolation. Authoritative blinded quality runs require `isolated` with separate suite access; otherwise the claim is explicitly attested-only.
2. Version the suite/result contract for optional graded relevance, per-system route provenance, typed non-success results and measured timings. Preserve v1 reading only if it is cheap and tested; otherwise perform an explicit documented schema cutover, not a second evaluator. Semble annotations may not meet v1's mandatory no-gold eval stratum, so do not invent no-gold tasks just to pass validation. A no-answer metric is `not_applicable` without a real labeled stratum.
3. Validate labels against pinned Git bytes and exact file/line hashes. Normalize candidate path and line spans in adapters, then independently recompute file/block hashes and token counts in the evaluator. Invalid/out-of-bounds results fail.
4. Compute span-aware Recall@1/5/10/20, MRR@10, graded NDCG@10 where grades are reviewed, existing BCY budgets and no-answer abstention only for actual no-answer tasks. Report file-only recall as a secondary view. Apply a deterministic same-file collapse policy and publish both chunk-level and collapsed behavior if relevant.
5. Produce per-query rows and paired win/loss/tie plus confidence interval or bootstrap interval on sufficient sample sizes. Report sample count; do not hide small-sample uncertainty. Freeze the primary metric and treatment of unanswerable, capped and partial results before the eval split is revealed.
6. Validate a canonical path+file SHA allowlist for both runners and reject results from an excluded file even when that file is tracked by Git. Exact checkout validity alone is insufficient for a same-universe comparison.

## Planned files

- `tools/benchmark/retrieval/evaluator.py`
- `tools/benchmark/retrieval/{suite,runner}.schema.json` or explicit v2 siblings
- `tools/benchmark/retrieval/suites/`
- `tools/ci/tests/test_retrieval_benchmark.py`

## Acceptance / verification

- Existing evaluator v1 tests remain green or have an explicit migration test with no duplicate scoring implementation.
- Mutants for altered commit, line bytes, candidate span, file universe, grade, missing task/route, duplicate rank, gold in query pack and mismatched tokenizer are rejected.
- The same recorded candidates yield identical scores regardless of which runner produced them.
- [TEST-PLAN.md](TEST-PLAN.md) T01–T04/T13 mutants, including all-answerable external suites and excluded-but-tracked candidate paths, pass/fail as specified.
- Run focused `python3 -m pytest tools/ci/tests/test_retrieval_benchmark.py -q`; report exact selected/passed count and any untested schema path.
