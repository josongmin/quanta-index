# ADV-02 Sourcegraph Structural Mixed-Domain Widening

Parent packet: [../README.md](../README.md)

Status: `landed` (legality matrix frozen; `RawString` and `Predicate` siblings
both widened and parity-proven)

Increment log:

- `legality-matrix` (done, behavior-preserving) — lifted the scattered leaf
  branch comments in `rewrite_sourcegraph_structural_expr` into one central,
  enforced table: `StructuralLeafVerdict` + `structural_leaf_verdict(&LqLeaf)`
  in `crates/quanta-index-search-plane/src/lowering.rs`, guarded by
  `sourcegraph_structural_leaf_verdict_matrix_is_frozen`. The boolean context
  is uniform (All/Any/Not recurse), so the matrix is one-dimensional over leaf
  kind.
- `raw-string-sibling` (done, widened + proven) — `RawString` leaves are now
  preserved as lexical siblings of a structural body (verdict
  `PreserveLexical`), mirroring native `'...' OR match { ... }`. On the SG side
  the reachable raw leaf is `file:contains('...')` (executable bridge
  predicate). Proof: owner-local lowering test
  `sourcegraph_structural_route_preserves_raw_string_in_mixed_boolean_or`;
  cross-route parity row
  `structural_sourcegraph_native_mixed_raw_string_or_parity` in
  `e2e_dual_syntax_lowering_parity` (green); bridge golden rail unchanged (21).
- `predicate-sibling` (done, widened + parity-proven) — `Predicate` leaves are
  now preserved as lexical siblings (verdict `PreserveLexical`, flat — no guard,
  so the checker's flat-table invariant holds). The lexical executor gates a
  preserved predicate downstream exactly as on native: `repo.has.file` /
  `file.contains` / `file.has.content` execute; non-executable predicate names
  typed-fail with `LEX_PREDICATE_UNIMPLEMENTED` at the execution layer — same
  code and layer as native, so SG mirrors native for every predicate sibling
  (the old behavior typed-failed *all* predicate siblings at lowering, strictly
  narrower than native). Native-parity-first: the parity rail is the judge and
  is green. Proof: owner-local
  `sourcegraph_structural_route_preserves_predicate_sibling_in_mixed_boolean`;
  matrix-freeze cell `Predicate => PreserveLexical`; native↔SG parity
  `structural_sourcegraph_native_mixed_predicate_sibling_and_parity`
  (`repo:has.file(path:src/lib.rs) AND <structural body>` → `["alpha_rust"]` on
  both syntaxes). `StructuralBlock` remains the only `TypedFail` leaf.

## Objective

Widen SG structural mixed-domain lowering beyond the current `keyword +
structural body` subset so that more native-executable mixed shapes survive the
bridge unchanged.

## Current Source Truth

- native mixed lexical/structural boolean already executes
- SG structural legality is now code-owned in
  `crates/quanta-index-search-plane/src/lowering/structural_matrix.rs`
- `RawString` and `Predicate` siblings are now widened for mixed `AND` / `OR`
  and remain typed-fail in root / `NOT`

## Current Code Pointers

- SG structural authority:
  `crates/quanta-index-search-plane/src/lowering.rs`
  `lower_sourcegraph_structural_query_text`,
  `lower_sourcegraph_structural_shape`,
  `rewrite_sourcegraph_structural_expr`,
  `rewrite_sourcegraph_structural_children`
- runtime dispatch hook:
  `crates/quanta-index-search-plane/src/query_dispatcher.rs`
  `lower_sourcegraph_structural_query_text`
- bridge mirror:
  `crates/quanta-index-lq-bridge/src/translator.rs`
  `lower_sourcegraph_expr`, `lower_sourcegraph_filtered`
- current owner-local truth:
  `crates/quanta-index-search-plane/src/lowering.rs`
  `sourcegraph_structural_route_preserves_lexical_keyword_in_mixed_boolean_or`,
  `sourcegraph_structural_route_rejects_repo_scoped_filter_under_mixed_or`,
  `scoped_filters_under_or_and_not_fail_closed_with_typed_translate_errors`
- parity/runtime truth:
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
  `structural_sourcegraph_native_mixed_lexical_and_parity`,
  `structural_sourcegraph_native_mixed_lexical_or_parity`,
  `structural_sourcegraph_native_mixed_lexical_and_not_parity`
  plus `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- proof/doc truth:
  `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`,
  `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`,
  `docs/plans/jun-2-dsl-final-cut/tickets/JFC-05-sourcegraph-bridge-and-carrier-parity.md`

## 핵심 로직

- replace special-case widening with an explicit legality matrix:
  `leaf kind x boolean context x route`
- if a mixed lexical sibling is representable on the active LQ wire, preserve it
- if not representable, typed-fail with `BRIDGE_TRANSLATE_FAIL`
- native execution remains the source of truth; SG widening follows it

## 건드릴 파일

- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-lq-bridge/tests/golden_bridge.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`
- `docs/plans/jun-2-dsl-final-cut/tickets/JFC-05-sourcegraph-bridge-and-carrier-parity.md`

## 생성 가능 파일

- first preference: keep the legality table inside
  `crates/quanta-index-search-plane/src/lowering.rs`
- if the table stops fitting cleanly, extract one helper module under the same
  owner crate, for example
  `crates/quanta-index-search-plane/src/lowering/structural_matrix.rs`
- sibling-specific runtime fixtures, if `runtime_rows.toml` becomes too dense:
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/docs-structural-raw-string.toml`
  and `docs-structural-predicate.toml`

## 건드리지 말 것

- bridge-packet directives
- runtime corpus by forcing bridge-only carriers into search-result rows
- native semantics that are not already executable

## TODO

- [x] freeze the SG legality matrix for mixed-domain structural queries (`structural_leaf_verdict`)
- [x] widen `RawString` sibling support where canonical lowering exists
- [x] widen `Predicate` sibling support where canonical lowering exists (flat preserve; executor gates)
- [x] add sibling-specific lowering tests and SG/native parity rows
- [x] keep non-representable shapes on explicit typed-fail rails (`StructuralBlock` typed-fail; non-executable predicates fail at exec)

## Concrete First Increment

Ship in this order:

1. document and codify the legality matrix without changing behavior
2. widen `RawString` sibling only
3. widen `Predicate` sibling only

Do not widen both sibling families in one PR. If `RawString` reveals routing or
explanation drift, stop before touching `Predicate`.
Do not change `query_dispatcher.rs` unless the existing search-plane lowering
entrypoint truly needs a new helper call.

## Implementation Steps

1. red: pin the current narrow subset with owner-local lowering tests and keep
   the existing parity rows green
2. refactor: express legality in one central table or helper, not scattered
   branch comments inside `rewrite_sourcegraph_structural_expr`
3. widen: add `RawString` sibling support only where the active LQ wire can
   represent it canonically
4. proof/doc sync: add `RawString` owner rails, parity rows, and ledger/matrix updates
5. red again: prove the remaining `Predicate` sibling path is still typed-fail
   until widened intentionally
6. widen: add `Predicate` sibling support only after the `RawString` lane is green
7. proof/doc sync: update bridge goldens, parity rows, and packet truth in the
   same change

## Dependency / Import Constraints

- widening must not depend on runtime corpus inventing semantics the bridge
  layer cannot express directly
- `quanta-index-search-plane` owns legality; `quanta-index-lq-bridge` mirrors it
- do not add route-local fallback from SG structural into plain lexical text
- no new owner authority in runtime tests or docs; legality must stay in the
  search-plane crate
- do not widen scoped-filter-under-`OR` here; that remains a separate lane

## Red Rails First

- matrix freeze rail:
  `./scripts/cargow test -p quanta-index-search-plane --lib sourcegraph_structural_route_preserves_lexical_keyword_in_mixed_boolean_or -- --nocapture`
- typed-fail rail:
  `./scripts/cargow test -p quanta-index-search-plane --lib sourcegraph_structural_route_rejects_repo_scoped_filter_under_mixed_or -- --nocapture`
- bridge mirror rail:
  `./scripts/cargow test -p quanta-index-lq-bridge --test golden_bridge -- --nocapture`
- parity rail:
  `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`

## NOT TODO

- no lexical fallback for unsupported SG structural shapes
- no widening before native parity is defined
- no set-like parity oracle
- no widening of scoped-filter-under-`OR` here; that is `ADV-03`

## Test Plan

- `./scripts/cargow test -p quanta-index-lq-bridge --test golden_bridge -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`
- focused owner-local lowering tests under `crates/quanta-index-search-plane`

## DoD

- SG structural route supports the widened mixed-domain sibling set explicitly
- native and SG parity is proved for each widened sibling family
- typed-fail remains stable for the still-unrepresentable remainder
- docs describe the legality matrix, not an ambiguous “broader subset”

## Concrete Deliverables

1. one code-owned SG legality table for mixed-domain structural queries
2. one `RawString` sibling PR with owner-local tests, parity rows, and doc sync
3. one later `Predicate` sibling PR with the same proof shape
4. explicit typed-fail coverage for all still-illegal sibling/context cells

## Failure Modes

- bridge lowers a shape that native cannot execute
- widened SG route hides fallback through structural-body rewriting
- parity rails cover only hits and miss typed-fail or miss parity
- legality matrix is documented but not actually enforced by code
