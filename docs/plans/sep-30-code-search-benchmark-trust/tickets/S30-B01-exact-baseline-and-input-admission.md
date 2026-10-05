# S30-B01 — exact gin baseline and input admission

Status: `ACTIVE_RESIDUAL`; historical executions are diagnostic. Priority: P0.
Parent: [Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-01](../../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md)
and [CS-BENCH-02](../../sep-27-code-search-remediation/rfcs/CS-BENCH-02-native-response-validation.md).

## Input and purpose

Start with gin `d3ffc9985281dcf4d3bef604cce4e662b1a327a6`, its 99-file
`code_only` manifest and the 1,196-task suite SHA-256
`bb49c90ecd3706d153c16f01ff336a44b42a2e23d160126107554dcf0e281ef3`.
Resolve the exact source universe and suite from the retained input manifest;
historical paths are in [historical bodies](../../ARCHIVE-INDEX.md#oct-05-benchmark-and-quality-ledger-compaction).
This is an exposed, source-oracle exact-name regression set. Its all-positive,
single-repository distribution must stay visible: 1,118/1,196 tasks have one
gold file. Do not present this as representative of typo, no-answer, semantic
or multi-repository usage.

## Work

1. Recheck corpus HEAD, every manifest file hash, source view, suite/pack digest,
   task IDs, all answerability/gold/file judgments and query-family identities.
   Check the existing source oracle against actual declarations, including
   `Engine`, `Context` and `RouterGroup` identities.
2. Inventory existing 1,196 captures per product from **raw rows and native
   evidence**, not summaries. Record which products have a complete row set,
   original/effective request, native rank unit, status and indexed-universe
   evidence. A 300-query capture is not a 1,196-query capture.
3. Replay historical evidence only against its original frozen source, input
   and decoder. Mark absent or incompatible captures `NOT_RUN`/`BLOCKED` for
   the 1,196 scope; do not mix historical 300 results into it.
4. Freeze a prospective run manifest and fresh external output root for B04.
   Resolve current clean code revision, binaries, dependency lockfiles,
   corpus selection, request modes and scoring policies before search.

## Verification and deliverable

- Independent counts and digests match the source/suite; tampered file, duplicate
  task, invalid query, wrong source and stale pack are refused by existing
  validators and focused tests.
- One admission matrix lists each product's `READY`/`BLOCKED` reason for the
  exact 1,196 run, including index scope and native result unit.
- Historical immutable data remains unchanged. The external report cites exact
  source, suite, capture and decoder identities; it claims no fresh quality
  gain or live five-product comparison.

Use [source oracle suite](../../../../tools/benchmark/retrieval/source_oracle_suite.py),
[evaluator](../../../../tools/benchmark/retrieval/evaluator.py) and the existing
[query-pool guard](../../../../tools/benchmark/retrieval/query_pool_guard.py).
Create no new suite from report hits.


## Execution ownership

Current cross-ticket execution is owned once by the
[OCT-04 residual ledger](../../oct-4-parallel-closure/tickets/INDEX.md).
Retain the acceptance above for any new claim; reuse compatible captures.
Past counts, binaries, failures and commands are recoverable from [historical bodies](../../ARCHIVE-INDEX.md#oct-05-benchmark-and-quality-ledger-compaction).
They do not qualify current source.
