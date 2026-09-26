# DH-03 Predicate Proof Symmetry

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-08-001](../../../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent packet: [../README.md](../README.md)

Status: `landed`

## Objective

Make the shipped predicate subset proof sibling-complete without widening the
predicate surface.

## Current Source Truth

- `repo.has.file(...)` executes for both `path:` and `name:` argument filters
- owner-local and runtime proof now exercise both `path:` and `name:` branches
- `file.contains(...)` is live on the native dotted predicate surface
- current proof now covers raw positive plus phrase hit/miss

## Current Code Pointers

- predicate execution:
  `crates/quanta-index-lexical/src/lib.rs`
  `repo_has_file_constraint`, `predicate_content_leaf`
- owner-local proof:
  `crates/quanta-index-lexical/tests/tantivy_smoke.rs`
- runtime proof:
  `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- parity companion rail:
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`

## 핵심 로직

- if the code ships a subset, the proof inventory must cover that exact subset
- this ticket is proof completion first; product code should move only if the
  missing sibling rails expose a real bug
- no widening of predicate names or argument shapes belongs here

## 건드릴 파일

- `crates/quanta-index-lexical/tests/tantivy_smoke.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- proof/docs after rails land:
  `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
  `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 건드리지 말 것

- predicate parser surface
- new predicate names or argument families
- runtime catalog semantics

## TODO

- [x] add owner-local `repo.has.file(name:...)` true-gate and miss rails
- [x] add runtime-corpus `repo.has.file(name:...)` true-gate and miss rows on
  `docs-multi-repo.toml`
- [x] add owner-local `file.contains("...")` phrase-positive rail
- [x] add runtime-corpus `file.contains("...")` phrase-positive row
- [x] rerun parity rails if any dotted/native lowering assumptions change
- [x] tighten docs so the claimed shipped subset matches the proven subset exactly

## Concrete First Increment

The first PR for this ticket should do only this:

1. add the missing owner-local rails
2. add the matching runtime rows
3. update the proof inventory if and only if the new rails are green

Do not touch parser or executor code first. Let the missing proof expose whether
there is a real product bug.

## Implementation Steps

1. red: add missing sibling rails in `tantivy_smoke`
2. red: add matching runtime rows in `runtime_rows.toml`
3. repair: only if the new rails fail, fix the narrowest owner seam
4. proof: rerun `e2e_full_corpus` and parity rails
5. docs: sync proof inventory wording to the exact proven subset

## Dependency / Import Constraints

- no new predicate syntax
- no broad runtime harness rewrite
- keep the multi-repo oracle non-vacuous; do not add single-repo coincidence rows

## Red Rails First

- owner-local predicate rail:
  `./scripts/cargow test -p quanta-index-lexical --test tantivy_smoke -- --nocapture`
- runtime corpus:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`

## NOT TODO

- no widening new predicate capabilities
- no proof by doc prose alone
- no vacuous positive rows that would stay green if the sibling branch were dead

## Test Plan

- owner-local sibling rails in `tantivy_smoke`
- runtime-corpus sibling rows in `e2e_full_corpus`
- parity rerun if any repair touches lowering assumptions

## DoD

- shipped predicate subset proof covers every executable sibling branch
- `repo.has.file(name:...)` and `file.contains(phrase positive)` have direct
  owner-local and runtime proof
- docs/proof inventory no longer overclaim beyond the proven sibling set

## Failure Modes

- docs claim executable subset coverage broader than the proof inventory
- multi-repo predicate rows become vacuous
- proof completion quietly widens the surface instead of testing what already ships
