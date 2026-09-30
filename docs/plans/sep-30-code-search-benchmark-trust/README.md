# Sep 30 — code-search benchmark trust execution plan

Status: `PLANNED`. This plan records work to execute; it does not claim new
labels, live captures, product quality, speed, or release qualification.
Planning baseline: `quanta-index@0d21914e53b85e13b8e3c2ec644a9a0112faf5be`
on 2026-09-30. The shared checkout had pre-existing dirty code changes outside
this documentation path when this plan was authored; their ownership was not
assessed here. Recheck source and ownership before implementation.

[Tickets](tickets/INDEX.md) are execution slices of the existing
[CS-BENCH-01](../sep-27-code-search-remediation/rfcs/CS-BENCH-01-corpus-gold-and-holdout.md)–[04](../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md)
and [CS-INT-01](../sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md).
Those RFCs and the benchmark ADRs retain contract authority. Do not create a
second corpus manager, runner, scorer, registry, or evidence format for this plan.

## Decision and claim boundaries

Use four separate scoreboards: exact declaration/file lookup, identifier
robustness, semantic/architecture/symbol relevance, and repository workflow
retrieval. Report execution completion and performance on separate axes. A
single combined five-product quality or speed ranking is not an output.

| Lane | Input | Permitted claim |
| --- | --- | --- |
| Existing exact names | Frozen gin `code_only` 99-file, 1,196-query suite | Source-oracle exact-name regression on one known repository; not independent human relevance or a fresh holdout |
| Semble gin 20 | Pinned upstream annotations and the separately verified gin source universe | A small reviewed case series: semantic 11, architecture 6, symbol 3; no product superiority claim from category counts this small |
| Gin identifier robustness | Preregistered prefix, infix, split, typo and no-answer cases | Failure modes by query transformation; a gin-only diagnostic, not evidence that the engine generalizes across repositories |
| Agent Retrieval Bench gin 88 | ARB-provided base-commit snapshots and `all_files` candidates | Positive repository-workflow file/context retrieval; the gin subset contains no no-gold cases |
| Fresh multi-repository release | Independent corpus and unseen query families under BENCH-01 | Generalization or product-default claims only after BENCH-01/03/04 and decision-policy admission |

The public 1,196, Semble 20, and ARB examples are visible to implementers.
They may be regression or external diagnostic inputs. Reshuffling them, or
generating variations of already tested gin names, does not create an unseen
product holdout. A later engine change requires an independently frozen release
for a new generalization claim.

## Pinned starting facts, not new benchmark receipts

- Gin source: `d3ffc9985281dcf4d3bef604cce4e662b1a327a6`, `code_only` 99 files,
  file-universe SHA-256
  `d4e1ea025c067f344af640568b5bfd0835ed09c2bfbcf9668b8e9782bc68bd67`.
- Existing 1,196-query suite at
  `/Users/songmin/Documents/code-new/qi-gin-lexical-oracle-20260929-3abc98c9/suite-declaration-file-1196.json`:
  SHA-256 `bb49c90ecd3706d153c16f01ff336a44b42a2e23d160126107554dcf0e281ef3`.
  All tasks are `eval`, exact bare names and answerable; 1,118 have exactly one
  gold file. These facts were read from that suite, not inferred from a report.
- Semble annotations must be fetched at
  [`aa634b14dc81ba6925a130cb999081f3240e2a3c`](https://github.com/MinishLab/semble/blob/aa634b14dc81ba6925a130cb999081f3240e2a3c/benchmarks/annotations/gin.json),
  with the upstream repository pin separately checked. The source labels were
  generated and checked with the same model; they are not independent review.
- The overlapping strings `Engine`, `Context`, and `RouterGroup` are different
  tasks across suites: upstream symbol/type relevance is not the existing
  Go declaration oracle. In the existing suite, `Engine` has two gold paths:
  `gin.go` and `binding/default_validator.go`.

## Admission and execution order

1. [B01](tickets/S30-B01-exact-baseline-and-input-admission.md): freeze the
   execution manifest, inspect source and replay the existing 1,196 rows.
2. [B02](tickets/S30-B02-semble-gin-20-qrels.md) and
   [B03](tickets/S30-B03-identifier-robustness.md): independently prepare the
   reviewed 20-query and preregistered robustness inputs. B04's exact-name
   capture can proceed after B01 while these inputs are prepared.
3. [B04](tickets/S30-B04-five-product-capture.md): run the admitted live
   five-product matrix into fresh external roots. Preserve native evidence,
   exact indexed scope, effective requests, completion and rank units.
4. [B05](tickets/S30-B05-scoring-statistics-and-report.md): independently replay
   raw rows, score each lane, and publish exclusions and uncertainty.
5. [B06](tickets/S30-B06-arb-gin-workflow.md): run ARB gin using its own frozen
   snapshots; keep results separate from the 99-file gin release.
6. [B07](tickets/S30-B07-performance-and-indexing.md) is a separate quiet-host
   measurement. [B08](tickets/S30-B08-fresh-multirepo-holdout.md) owns fresh
   cross-repository qualification. Neither is implied by a successful B04/B05.

Use the current [retrieval benchmark guide](../../../tools/benchmark/retrieval/README.md)
and [code-search runbook](../../../tools/benchmark/CODE_SEARCH_RUNBOOK.md) to
resolve actual commands. Inputs, indexes, raw captures, gold review records,
logs and per-run manifests belong under a new external output root; historical
captures and `/private/tmp/g3` remain immutable. The source checkout contains
only reusable code, tests, schemas and this plan.

## Non-negotiable report fields

For each task and product: original and submitted query, mode, case/scope,
effective request identity, repository/source/index identity, native rank unit,
ordered paths and IDs, duplicates, result cap/continuation, completion/error,
and elapsed boundary. For each scored lane: qrel policy and provenance,
eligible/attempted/completed/unsupported/incomplete/error denominators,
per-stratum results, excluded IDs/reasons, and independent replay status.

Ten chunks cannot be silently promoted to ten files. A file-ranked comparison
requires a genuine ten-distinct-file request or an explicit bounded collection
policy; a first-occurrence projection of ten native chunks remains an
observed-prefix diagnostic. Matching bare query text does not make different
symbol or semantic intents interchangeable. `QUALITY_DELTA=pass` admits evidence;
the product-default decision additionally needs the frozen policy and
[`decision.py`](../../../tools/benchmark/retrieval/decision.py).

## Research basis and limits

| Source | Applied decision | Limit |
| --- | --- | --- |
| [TREC relevance judgments](https://trec.nist.gov/pubs/trec33/papers/overview_33.pdf), [CodeSearchNet annotations](https://github.com/github/CodeSearchNet) | Source-grounded graded qrels, pooled-result review, independent metric cross-check | Pooling can miss relevant files; unjudged is not automatically irrelevant |
| [Semble benchmark](https://github.com/MinishLab/semble/blob/aa634b14dc81ba6925a130cb999081f3240e2a3c/benchmarks/README.md) | Import its gin queries and annotation provenance | Vendor benchmark and 20 gin queries cannot independently rank products |
| [COBE](https://repositorio.pucrs.br/dspace/bitstream/10923/25612/2/COBE_A_Natural_Language_Code_Search_Robustness_Benchmark.pdf), [Memtrace](https://github.com/syncable-dev/memtrace-public/blob/main/benchmarks/README.md), [Zoekt design](https://github.com/sourcegraph/zoekt/blob/main/doc/design.md) | Measure query transformations separately; distinguish substring candidates from typo correction | COBE studies natural-language robustness; Memtrace is a vendor-run method example; trigram is not fuzzy matching |
| [ARB](https://github.com/eyuansu62/agent-retrieval-bench), [CORE-Bench](https://github.com/zhangfw123/CORE-Bench-Eval) | Frozen base commits, all-file workflow retrieval and token-budget context, then scale | Workflow relevance is distinct from exact lexical conformance |
| [BEIR](https://arxiv.org/abs/2104.08663), [CoIR](https://github.com/coir-team/coir) | Heterogeneous, repository-aware external validation | Neither supplies Quanta's exact symbol/prefix gold |

These are applicable precedents, not a claim that one composite procedure is
an industry standard or that a particular search algorithm will win.
