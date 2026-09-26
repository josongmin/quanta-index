# BM-05 — Retrieval benchmark registration and product adapters

Status: `PARTIAL / CORPUS_VIEWS_AND_CONTRACT_PAIR_LEXICAL_ADAPTERS_IMPLEMENTED`; frozen corpus/CLI integration passed (158 tests), paired/CLI contract verification passed (112 tests), historical canonical Python recipe passed (579 tests); newly expanded recipe and fresh paired/lexical search pilot remain `NOT_RUN`. Priority: P1. Depends on: BM-02/BM-03; coordinate with RBR-12. Common gates: [TEST-PLAN](TEST-PLAN.md).

Current implementation/verification boundaries are recorded in [CURRENT-AUDIT.md](CURRENT-AUDIT.md). Historical `git show eff53181:docs/plans/sep-26-bench-migration/tickets/CLOSEOUT.md` and [BM-07-MIGRATION-MATRIX.md](BM-07-MIGRATION-MATRIX.md) do not establish complete migration qualification.

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

## Current implementation boundary

`retrieval_capture.py` implements common `run/validate/replay/summarize` for
`retrieval-contract`, using both existing portable proof producers. SDK/contract
receipts are typed `proof` payloads rather than artificial relevance rows.
Immutable custody retains original command/path provenance, raw inventories,
source closure, tool identity and frozen SDK binaries. Complete-profile
publication remains separate from individually promoted runs.

Focused shared-custody tests exercise owner-derived replay, changed counts,
missing rail, partial/failed terminal counts, source mismatch and quoted
external Just parameters. These tests do not establish a real SDK execution,
paired comparison, gold quality or performance qualification. Current-source
terminal receipts are recorded in [CURRENT-AUDIT.md](CURRENT-AUDIT.md).

`lexical_capture.py` now implements common execution of the existing scorer
over frozen external observations, five product-specific immutable runs,
complete-profile validation and raw replay. It retains native timing layers and
exclusions, never treating scorer execution as live product search. Focused
tests use declared fixtures; actual clean-source CLI and pilot receipts are
separate. `pair_capture.py` now connects existing native `pair`/`verdict` execution
to common `run/validate/replay/summarize` without a second scorer. It preserves a
self-contained Git corpus bundle, exact input/executable identities and the
original native output tree. File/context/indexed-span cases remain distinct;
missing span observations are unsupported, not zero. The registry now names the
actual `retrieval-run-manifest:v2` contract, replacing the erroneous v5 label.
Focused producer fixtures do not establish real product execution.
`corpus_release.py` now provides common `benchctl corpus create/validate`,
complete Git-object inventory and materialized `code_only`/`developer_search`
views with explicit exclusions and retained self-contained bundles. Real
gin/ripgrep input release creation and fresh-process validation are verified
at the source-bound checkpoint in CURRENT-AUDIT; this is input generation,
not the required live comparison pilot. `corpus_binding.py` now binds common
lexical schema v2 to one release/repository/view and exact suite/query universe,
retaining Git bundles for reconstruction without mutable original paths.
Product index-universe attestation remains false; capsules above 256 MiB refuse.
Still open: view-bound inputs throughout live paired comparator execution, live comparator capture,
independent scorer cross-check, fresh two-repo paired/lexical pilot and
large paired-capture raw storage deduplication.

The registry now separates `retrieval-diagnostic` (pair execution) from
`lexical-diagnostic` (five-product scorer). The lexical family points to its
actual module/schema rather than the pair producer. Proof validation points
to `portable_proof.py`; pair validation points to `run.py verdict`, not a
JUnit-count checker. Adapters must refuse owner/schema/scorer registration
drift, not silently run their hardcoded implementation under another policy.

Lexical file recall is query-macro gold-file coverage, distinct from hit rate.
Scorer raw rows and per-query paired recall are cross-checked; missing current
per-query fields require re-scoring original records rather than manufacturing
old receipt upgrades. This code correction does not qualify historical results.
