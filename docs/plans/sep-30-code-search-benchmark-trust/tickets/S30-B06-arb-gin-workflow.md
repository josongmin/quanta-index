# S30-B06 — ARB gin 88 workflow retrieval

Status: `ACTIVE_RESIDUAL`; historical executions are diagnostic. Priority: P1. ARB release validation
may start alongside B02/B03; live capture depends on B04's native/adapter
contract. Parent: [Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md)
and [CS-BENCH-03](../../sep-27-code-search-remediation/rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md).

## Scope

[Agent Retrieval Bench](https://github.com/eyuansu62/agent-retrieval-bench)
reports 88 positive gin cases: `code2test` 15, `trace2code` 56 and
`edit2ripple` 17. It supplies frozen repository/base-commit corpora and an
`all_files` candidate contract. Do not run those 88 queries against our one
gin `d3ffc998`/99-file corpus or transfer its file IDs into ARB qrels.

## Work

1. Pin the ARB repository/release IDs, download the relevant official bundles,
   validate archive/checksums and enumerate distinct gin base commits, files
   and per-case gold. Preserve release license and attribution metadata.
2. Build one candidate view per supplied base snapshot with `all_files`, not
   `code_only` or a task-aware candidate filter. Record full indexed-universe
   hashes, exclusions and build/activation identity before each case.
3. Translate each workflow signal through an explicit adapter preserving
   original text, any provided anchor file/trace and submitted request. Do not
   inject answer paths or use the known gold to choose a query rewrite.
4. Replay result identities and calculate ARB's file Recall@20, MRR and BCY at
   its 4k/8k/16k/32k token budgets. Compare a pinned official baseline on
   a tiny fixture before reporting the full subset.

## Verification and boundary

- All 88 have their correct base snapshot and task type; no post-fix file is
  substituted for a pre-fix view. Query/anchor/gold leakage controls reject.
- Official evaluator and local independently checked toy cases agree on file
  and token-budget treatment. Failure, unsupported and no returned files are
  separately counted.
- Report each workflow subset and repository snapshot. The gin 88 contains no
  no-gold cases and proves no abstention behavior. ARB's 50 natural and 32
  counterfactual no-gold examples belong to a later separate selective track.
- No combined mean or head-to-head rank merges ARB with the existing 1,196 or
  Semble 20. This pilot alone does not close BENCH-01's fresh multi-repo holdout.

Keep reusable adapters under the current retrieval benchmark owners. Store
downloaded corpora, indexes and captures outside the checkout.


## Execution ownership

Official-text and transformed-adapter arms keep separate populations and
request limits; a completed adapter arm does not close the official-text arm.
Current cross-ticket execution is owned once by the
[OCT-04 residual ledger](../../oct-4-parallel-closure/tickets/INDEX.md).
Retain the acceptance above for any new claim; reuse compatible captures.
Past counts, binaries, failures and commands are recoverable from [historical bodies](../../../ARCHIVE-INDEX.md#historical-record-recovery).
They do not qualify current source.
