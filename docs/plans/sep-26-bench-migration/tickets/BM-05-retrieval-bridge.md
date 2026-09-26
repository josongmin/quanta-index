# BM-05 — Retrieval benchmark registration and product adapters

Status: `PLAN / NOT_RUN`. Priority: P1. Depends on: BM-02/BM-03; coordinate with RBR-12. Common gates: [TEST-PLAN](TEST-PLAN.md).

Implementation/verification/qualification verdicts for this ticket are recorded in [CLOSEOUT.md](CLOSEOUT.md) and, where relevant, [BM-00-INVENTORY.md](BM-00-INVENTORY.md), [BM-03-DECISION.md](BM-03-DECISION.md) and [BM-07-MIGRATION-MATRIX.md](BM-07-MIGRATION-MATRIX.md). The original `PLAN / NOT_RUN` status above is the plan-time state, not the closeout state.

## Purpose

Bring retrieval execution into the common corpus/capture/evidence lifecycle while preserving RB's independent relevance authority and distinguishing file, line, span, context, and agent outcomes.

## Work

1. Register the existing `benchmarks/retrieval` Rust SDK producer, `tools/benchmark/retrieval/{run.py,evaluator.py,semble.py}` and Sourcegraph/OpenGrok/cs comparators. Keep one Quanta runner and one relevance scorer authority; do not fork `lexical_file_comparison.py` into a third current scorer.
2. Define an external corpus release with repository commit, complete tracked-file inventory, file hashes, license/attribution, explicit `code_only` and `developer_search` materialized views, exclusions, and view digest. Predeclare symlink, submodule, Git LFS, generated/vendor, binary/encoding, size-limit and case-collision handling. Preserve the existing `frozen-v4` candidate immutably. Within each product comparison, every adapter must use the same declared view and prove or explicitly fail to prove its searchable universe; an operator assertion alone is not independent index proof.
3. Move comparator capture logic under repo-owned adapters. Record exact original and rendered query, product/binary/container/version/config, index snapshot, response order and supported rank semantics, raw response, normalized results, timeout/partial/error and client timing boundary. Sandbox adapter processes with declared network/secret access, resource limits and owned cleanup. No fabricated span, rank, hit, or completed response.
4. Extend the current suite/capture contract to typed `file`, `line`, and `span` judgments/results where needed. Maintain historical replay separately. Development/holdout custody and independent gold remain owned by RBR-12 and the RB test plan; consume their frozen artifacts rather than inventing new ones.
5. Expose separate tracks: developer lexical product search; engine exact-match/trigram diagnostics; RB hybrid span/context; query speed; build/freshness. Distinguish `native_default` product behavior from `controlled_mechanism` parity; do not combine scores. Share corpus/query/evidence identities, not incompatible metric denominators. Report per-repo, language, query-intent, answerability and product coverage before aggregates. An incomplete judgment pool reports `unjudged` and pool coverage, not absolute recall as if the corpus had exhaustive labels.

## DoD

- Same corpus release/view/query pack is used across compared products, with exact path/hash reconciliation and explicit per-product coverage. Missing index-universe or rank attestation marks the product diagnostic, not qualified.
- File ranking metrics are independently cross-checked from qrels/raw ordered hits; span/context metrics retain the RB independent byte-span oracle. Unjudged, irrelevant, no-answer, timeout, and unsupported query are distinct states.
- A two-repo pilot through the chosen common CLI reproduces available raw captures and scores with no unexplained difference from the existing diagnostic calculation. A missing or unsupported comparator is explicit, not a zero score. A broader qualified claim requires separately admitted holdout/gold and host evidence; pilot success alone does not satisfy it.
- RB `PAIR_VALID`, `QUALITY_DELTA`, and `PERF_QUALIFIED` remain separate verdicts. No Quanta-vs-Semble or five-product winner is inferred from migration completion.

## Verification / exclusions

Use corpus mutation fixtures, adapter query/rank/timeout negatives, scorer cross-checks, existing `retrieval-contract-local` and clean `retrieval-contract-proof` / `retrieval-sdk-proof` rails, then actual external pilot. Product algorithm fixes belong to RBR tickets, not this bridge.
