# J7Q-01 — Ranking Quality And Relevance Corpus

Parent RFC: [../rfc.md](../rfc.md)

Status: `planned`

## Goal

Add a relevance corpus and quality gate so ordering regressions fail before
broad daemon reruns.

## Current Code Fact

- result execution correctness is strong
- explanation and score wires exist
- no blocking live relevance metric gate is present on the current tree

## Owner Seam

- lexical ranking behavior
- symbol and route-level ordering behavior
- scenario authority
- runtime quality rail

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-harness/src/scenarios.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- `/Users/songmin/Documents/code-new/quanta-index/docs/plans/jun-7-search-product-quality/COMMAND_AND_ARTIFACT_CONTRACT.md`

## Preferred Implementation Direction

- use a judged relevance corpus with stable query IDs and per-route labels
- support graded judgments, not only binary hit/miss, so `NDCG@10` remains
  meaningful
- keep deterministic tie-break evaluation separate from relevance quality so a
  stable but bad ranking does not look healthy
- measure lexical, symbol, structural, and history-backed families
  independently
- include hard negatives and near-duplicate distractors
- maintain one overlap-scoped external comparison sheet against Sourcegraph
  lexical shipped query families

## Layer Boundary Clarification

- this ticket owns ordering quality, not candidate-set correctness
- score wires and explanation wires are inputs, not proof by themselves

## Concrete Work Items

1. Add a relevance golden corpus with per-query intent labels.
2. Add expected:
   - top-1
   - top-k containment
   - forbidden-high-rank rows
3. Compute blocking metrics:
   - `MRR@10`
   - `NDCG@10`
   - `Recall@20`
4. Split lexical / symbol / structural / history-backed route reports.
5. Add a Sourcegraph lexical overlap subset for shipped overlapping query
   families.
6. Keep deterministic tie-break proof separate from relevance quality proof.

## Required Outputs

- stable command:
  - `just rust-verify-quality-relevance`
- canonical artifacts:
  - `artifacts/search-quality/relevance/latest/summary.json`
  - `artifacts/search-quality/relevance/latest/sourcegraph-overlap.json`
  - `artifacts/search-quality/relevance/latest/query_judgments.json`

## First Increment

- add one lexical relevance subset and one symbol or structural relevance
  subset

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-searchd-runtime --test dsl_scenarios -- --nocapture
```

## Worker First Commands

```bash
rg -n "score|explanation|symbol|structural|dsl_scenarios|scenario" crates -S
sed -n '1,260p' crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs
sed -n '1,260p' crates/quanta-index-searchd-harness/src/scenarios.rs
```

## No-Go

- do not treat score presence as relevance proof
- do not reuse latency bench artifacts as relevance evidence
- do not widen this ticket into semantic retrieval or hybrid fusion quality
- do not declare competitive lexical quality without an overlap-scoped external
  comparison

## Reviewer Rejection Checklist

- reject if the corpus is only a hand-picked list of “looks right” queries
- reject if one blended metric hides route-family divergence
- reject if Sourcegraph lexical is cited without an explicit overlapping query
  subset
- reject if a regression can demote top-1 quality while leaving all blocking
  gates green

## DoD

- ranking regressions fail on a dedicated relevance rail
- overlapping lexical query families clear the Sourcegraph lexical floor or the
  gap is explicitly called out

## Not Done If

- top result drift can still pass all blocking gates
- lexical and symbol or structural relevance are not measured separately
- the overlap subset exists only in prose or chat and not in persisted artifacts
