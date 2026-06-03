# ADV-01 Predicate Capability Registry

Parent packet: [../README.md](../README.md)

Status: `landed` (registry-only increment + first widened predicate family both
landed and verified)

Increment log:

- `registry-only` (done, no behavior drift) — introduced
  `crates/quanta-index-lexical/src/predicate_registry.rs` as the predicate
  capability SSOT (`PredicateKind`, `PREDICATE_REGISTRY`, `kind_of`,
  `parse_repo_file_matchers`, `RepoFileMatcher`/`RepoFileConstraint`, canonical
  `LEX_PREDICATE_UNIMPLEMENTED` owner). All five hardcoded `match name` dispatch
  sites in `lib.rs` (`lower_predicate_for_boolean_scope`,
  `extract_predicate_plan`, `prepare_predicate_plan`, the `compile_leaf`
  predicate arm, `repo_has_file_constraint`) and both planner sites
  (`plan_predicate_leaf`, `validate_repo_has_file_args`) now dispatch on the
  registry instead of re-matching raw names. `symbol.has.name` stays on the
  symbol-route arm by design (not a registry member).
  Verified green: `tantivy_smoke` (24), planner unit
  `predicate_repo_has_file_plans_through_tantivy_route`, registry units (5),
  `e2e_full_corpus`, `e2e_dual_syntax_lowering_parity` (4).
- `first-widened-family` (done) — widened exactly one `repo.has.file` arg-shape
  family: a `lang:` matcher (`RepoFileMatcher::Language`) that gates by whether
  the repo contains a file in the given language, lowered to one canonical
  exact-term query on the indexed `language` field (no ambiguity; `path:`/
  `name:` remain regex matchers). `parse_repo_file_matchers` is the single
  owner of the now `path:`/`name:`/`lang:` contract; planner validation inherits
  it unchanged. Proof: registry unit `repo_file_matchers_accept_lang_filter`;
  owner-local `tantivy_executes_repo_has_file_predicate_with_lang_matcher`
  (hit + fail-closed miss); native↔SG parity
  `repo_has_file_lang_predicate_parity` in `e2e_dual_syntax_lowering_parity`
  (green). Unsupported filters (e.g. `size:`) still typed-fail
  `LEX_PREDICATE_UNIMPLEMENTED`.

## Objective

Replace hardcoded predicate widening logic with a code-owned capability registry,
then widen the shipped predicate subset structurally instead of adding more
one-off branches.

## Current Source Truth

- predicate capability is now code-owned in
  `crates/quanta-index-lexical/src/predicate_registry.rs`
- unsupported names / argument shapes still typed-fail with
  `LEX_PREDICATE_UNIMPLEMENTED`
- the shipped executable subset is registry-driven, and the first widened family
  `repo.has.file(lang:...)` is landed

## Current Code Pointers

- lowering truth:
  `crates/quanta-index-lexical/src/lib.rs`
  `predicate_content_leaf`, `repo_has_file_constraint`,
  `lower_predicate_for_boolean_scope`, `prepare_predicate_plan`,
  `RepoHasFileConstraint`, `RepoHasFileMatcher`
- planner truth:
  `crates/quanta-index-lexical/src/planner.rs`
  `plan_predicate_leaf`, `single_string_arg`, `validate_repo_has_file_args`
- parser / AST shape truth:
  `crates/quanta-index-lq-norm/src/parser/implementation.rs`
  `predicate_repo_has_file_with_filter_arg`,
  `predicate_top_level_file_contains_with_raw_arg`
- owner-local predicate rails:
  `crates/quanta-index-lexical/src/planner.rs`
  `predicate_repo_has_file_plans_through_tantivy_route`
  and `crates/quanta-index-lexical/tests/tantivy_smoke.rs`
  `tantivy_repo_has_file_true_gate_narrows_by_indexed_source_repo_id`,
  `tantivy_executes_repo_has_file_predicate_as_repo_gate`,
  `tantivy_executes_repo_has_file_predicate_under_or_and_not`,
  `tantivy_executes_file_has_content_predicate_phrase_and_regex`
- runtime/parity truth:
  `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`,
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  `file_contains_raw_substring_parity`,
  `repo_has_file_predicate_parity`,
  `repo_has_file_predicate_under_or_parity`,
  `repo_has_file_predicate_under_not_parity`
- proof/doc truth:
  `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`,
  `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 핵심 로직

- define `predicate -> argument schema -> lowering target -> typed reject contract`
  as data, not ad-hoc `match` branches
- generate or mechanically validate docs/proof rows from that registry
- keep unsupported shapes explicit until the registry admits them

## 건드릴 파일

- `crates/quanta-index-lexical/src/lib.rs`
- `crates/quanta-index-lexical/src/planner.rs`
- new `crates/quanta-index-lexical/src/predicate_registry.rs`
- `crates/quanta-index-lexical/tests/tantivy_smoke.rs`
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- predicate-specific fixture files under `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 생성 가능 파일

- `crates/quanta-index-lexical/src/predicate_registry.rs`
- predicate-specific runtime fixture only if `runtime_rows.toml` cannot express
  the widened shape without coincidence:
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/docs-predicate-*.toml`
- do not add a new cross-crate helper for the first PR; reuse
  `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`

## 건드리지 말 것

- bridge carrier semantics
- empty-query parser shape
- request-time fallback authorities

## TODO

- [x] introduce a first-class predicate capability registry (`predicate_registry.rs`)
- [x] move existing shipped predicates onto that registry without behavior drift (owner + runtime + parity rails green)
- [x] choose and land the first widened predicate/arg-shape subset (`repo.has.file(lang:…)`)
- [x] add direct runtime rows and owner-local rails for every widened predicate (owner-local + parity)
- [x] keep unsupported shapes on stable typed-fail rails (`LEX_PREDICATE_UNIMPLEMENTED` centralized in the registry)

## Concrete First Increment

Ship in this order:

1. registry only, no behavior widening
2. migrate shipped predicates:
   - `file.contains`
   - `file.has.content`
   - `repo.has.file(path|name)`
3. widen exactly one new arg-shape family behind direct rails

Recommended first widened family:

- allow an explicit scalar matcher variant on `repo.has.file(...)` only if it
  can be lowered to one canonical repo-file matcher without ambiguity

Do **not** widen multiple predicate families in the same first PR.
Do **not** combine step 1 and step 3 in the same PR.

## Implementation Steps

1. red: add or pin the failing/guard rails first:
   - planner unit: unsupported shape still returns `LEX_PREDICATE_UNIMPLEMENTED`
   - owner rail: shipped predicates remain behavior-identical in `tantivy_smoke`
   - parity rail: `e2e_dual_syntax_lowering_parity` remains green on the shipped subset
2. refactor: add a registry type that describes:
   - predicate name
   - allowed arg schemas
   - boolean-scope lowering mode
   - top-level lowering mode
   - stable typed reject code/message owner
3. refactor: make planner validation read that registry first
4. refactor: make lexical lowering read the same registry instead of separate hardcoded
   `match` branches
5. refactor: freeze the existing shipped semantics by migrating current predicates onto
   the registry before any widening
6. widen: add the first widened arg schema only after the registry migration is green
7. widen: add direct owner-local rails and runtime rows for the widened shape
8. proof/doc sync: update the proof ledger, capability matrix, and this ticket in the same change

## Dependency / Import Constraints

- registry must stay inside `quanta-index-lexical`; do not create a docs-owned or
  test-owned capability map
- planner and lowering may depend on the registry, but runtime tests must not
  become the source of truth
- avoid introducing cross-crate feature flags just to stage widening
- do not move predicate authority into `quanta-index-search-plane` or the bridge;
  this is a lexical-owner seam
- no fallback from unsupported arg shape to keyword/regex coincidence

## Red Rails First

- planner unit rail:
  `./scripts/cargow test -p quanta-index-lexical --lib predicate_repo_has_file_plans_through_tantivy_route -- --nocapture`
- owner rail:
  `./scripts/cargow test -p quanta-index-lexical --test tantivy_smoke -- --nocapture`
- runtime corpus rail:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
- explicit parity rail:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## NOT TODO

- no silent coercion of unsupported predicate arguments
- no widening that only exists in docs or planner comments
- no repo-gate coincidence oracle reuse
- no “registry” that still duplicates rules in planner and lowering

## Test Plan

- `./scripts/cargow test -p quanta-index-lexical --test tantivy_smoke -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## DoD

- predicate support is registry-driven
- widened predicate shapes have direct owner-local and runtime proof
- unsupported shapes still typed-fail with stable diagnostics
- docs/ledger can no longer drift from the code-owned predicate subset

## Concrete Deliverables

1. `crates/quanta-index-lexical/src/predicate_registry.rs` as the only predicate capability SSOT
2. shipped predicate migration with no behavior drift on owner rails
3. exactly one widened predicate family with direct owner-local, runtime, and parity proof
4. proof ledger and capability matrix rows updated from the same capability truth

## Failure Modes

- new predicate support lands only in one route
- filter-arg widening silently changes semantics of existing predicates
- docs claim widened support without runtime proof
- registry becomes metadata only and does not actually own validation/lowering
