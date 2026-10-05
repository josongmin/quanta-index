# Benchmark trust — B01–B09 acceptance index

Status: `ACTIVE_RESIDUAL`.
[Parent](../README.md) · [Current execution ledger](../../oct-4-parallel-closure/tickets/INDEX.md)

This index routes stable acceptance scopes. The OCT-04 ledger owns live
judgment/admission/capture/optimization state; do not maintain another queue here.
Historical executions remain diagnostic and retain their original identities.

| ID | Acceptance owner | Prerequisite / remaining result |
| --- | --- | --- |
| [S30-B01](S30-B01-exact-baseline-and-input-admission.md) | Exact Gin baseline | Source/suite/pack/native-row admission on the selected source; historical regression is not fresh qualification |
| [S30-B02](S30-B02-semble-gin-20-qrels.md) | Semble Gin 20 | Actual independent judgments, complete blinded five-product pool, provenance and descriptive category reporting |
| [S30-B03](S30-B03-identifier-robustness.md) | Mechanical variants | Frozen sampling/families, exhaustive supported gold, ambiguity and no-answer; default/file/declaration requests remain separate |
| [S30-B04](S30-B04-five-product-capture.md) | Native matrix | B01/B02/B03 admitted inputs; native replay, exact indexed universe and explicit cell outcomes |
| [S30-B05](S30-B05-scoring-statistics-and-report.md) | Independent reporting | Final qrels plus B04 raw; same eligible units, exclusions, uncertainty and frozen decision policy |
| [S30-B06](S30-B06-arb-gin-workflow.md) | ARB workflow | Correct per-case base snapshot, original/adapted request populations, official file/token-budget semantics and separate no-gold track |
| [S30-B07](S30-B07-performance-and-indexing.md) | Query/index/resource measurement | Correct complete output, actual boundaries, repetitions and admitted host; perf remains unqualified |
| [S30-B08](S30-B08-fresh-multirepo-holdout.md) | Unseen holdout/default decision | C0–C5 source/license/split/gold/native/decision acceptance; exposed cohorts cannot fill unused holdout |
| [S30-B09](S30-B09-external-robustness-adoption.md) | Public external inputs | Pinned source/qrels/license, explicit admission/units and diagnostic populations; not whole-upstream or holdout qualification |

B02/B03 preparation can proceed after B01; B06 input validation is independent.
B05 waits for its final labels/native evidence. B07 uses one admitted quiet host;
B08 remains a distinct generalization gate. The [OCT-04 waves](../../oct-4-parallel-closure/WAVES.md)
own the current dependency schedule and integration handoffs.

Implemented contracts are in [the Accepted ADRs](../../../adr/README.md#oct-05-implemented-contracts).
Old counters, failures, commands and source-bound receipts are recoverable through
[the history index](../../ARCHIVE-INDEX.md#oct-05-benchmark-and-quality-ledger-compaction).
