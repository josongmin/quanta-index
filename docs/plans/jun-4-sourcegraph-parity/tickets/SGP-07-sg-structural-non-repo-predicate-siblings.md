# SGP-07 SG Structural Non-Repo Predicate Siblings

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Objective

Close SG structural mixed predicate gaps outside the shipped repo-gate family with exact explicit unsupported verdicts per family.

Primary candidates:

- `file.contains(...)`
- `file.has.content(...)`
- `symbol.has.name(...)`

## Current Source Truth

- SG structural route originally preserved only repo gate predicates:
  - `repo.has.file`
  - `repo.has.path`
  - `repo.has.content`
  - `repo.contains.content`
- `file.contains(path|file:..., <scalar>)` sibling remains typed `BridgeTranslateFail`
- `file.has.content(path|file:..., <scalar>)` sibling remains typed `BridgeTranslateFail`
- `symbol.has.name(...)` sibling remains typed `BridgeTranslateFail`
- native structural evaluator still has a broader generic lexical-leaf path, but shared SG proof remains family-scoped

## Files Touched

- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `tools/benchmark/sourcegraph_parity.py`
- `tools/benchmark/SOURCEGRAPH_PARITY.md`
- `docs/analysis/jun-4-dsl-capabilty.md`

## Final Verdict

- explicit unsupported:
  - `file.contains(path|file:...)`
  - `file.has.content(path|file:...)`
  - `symbol.has.name(...)`

Boolean contexts proven:

- `AND`
- `OR`
- `AND NOT`

## DoD

- explicit unsupported families have:
  - owner-local lowering rail
  - runtime corpus typed-fail row
  - SDK/front-door typed-fail row
  - parity guard demotion inventory entry

## Landed Evidence

- owner-local:
  - `sourcegraph_structural_route_rejects_file_contains_predicate_sibling`
  - `sourcegraph_structural_route_rejects_file_contains_predicate_sibling_under_or`
  - `sourcegraph_structural_route_rejects_file_contains_predicate_sibling_under_and_not`
  - `sourcegraph_structural_route_rejects_file_has_content_predicate_sibling`
  - `sourcegraph_structural_route_rejects_file_has_content_predicate_sibling_under_or`
  - `sourcegraph_structural_route_rejects_file_has_content_predicate_sibling_under_and_not`
  - `sourcegraph_structural_route_rejects_non_repo_predicate_sibling`
  - `sourcegraph_structural_route_rejects_non_repo_predicate_sibling_under_or`
  - `sourcegraph_structural_route_rejects_non_repo_predicate_sibling_under_and_not`
- runtime/front-door:
  - `sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth`
  - `full_corpus_runtime_fixture_executes_real_rows_only`
