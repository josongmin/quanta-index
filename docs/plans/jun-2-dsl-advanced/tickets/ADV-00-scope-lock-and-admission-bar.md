# ADV-00 Scope Lock and Admission Bar

Parent packet: [../README.md](../README.md)

Status: `done`

Closeout: the packet-level admission table, frozen start order, claim-discipline
paragraph, and permanent exclusions are frozen in [../README.md](../README.md)
§9. Later lanes inherit that gate; this ticket changed no product behavior,
fixtures, or runtime rows.

## Objective

Freeze what qualifies as legitimate DSL widening after closeout, and block
scope creep that would blur runtime semantics, bridge carriers, and parser-only
shapes.

## Current Source Truth

- shipped closeout is complete
- widening candidates exist, but they are optional and must not be misreported
  as bug fixes
- the user-facing “advanced” bar is higher than “now executable”: it needs
  stable legality, proof, and benchmark/shadow evidence

## Current Code Pointers

- closeout authority:
  `docs/plans/jun-2-dsl-final-cut/README.md`
- proof inventory:
  `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
  `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`
- predicate widening seam this ticket governs:
  `crates/quanta-index-lexical/src/lib.rs`
  `predicate_content_leaf`, `repo_has_file_constraint`,
  `lower_predicate_for_boolean_scope`, `prepare_predicate_plan`
- predicate planner seam:
  `crates/quanta-index-lexical/src/planner.rs`
  `plan_predicate_leaf`, `validate_repo_has_file_args`
- SG mixed-domain seam this ticket governs:
  `crates/quanta-index-search-plane/src/lowering.rs`
  `lower_sourcegraph_structural_query_text`,
  `rewrite_sourcegraph_structural_expr`
- packet-truth and parity rails this ticket must not blur:
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`,
  `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`,
  `crates/quanta-index-lq-bridge/tests/bridge_directive_packet.rs`

## 핵심 로직

- widening must start from a closed baseline
- new surface must be owned by one authority and one proof rail
- bridge carrier, runtime search-result surface, and parser shape must stay
  separated
- no “advanced” claim before correctness and cost evidence both exist

## 건드릴 파일

- `docs/plans/jun-2-dsl-advanced/README.md`
- `docs/plans/jun-2-dsl-advanced/tickets/*.md`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 생성 가능 파일

- none required in the first increment
- if the admission checklist later becomes mechanical, place the checker under
  `tools/ci/lint/`; do not create product-code helpers from this ticket

## 건드리지 말 것

- shipped closeout packet verdict
- runtime corpus carrier split for bridge directives
- empty-query semantics

## TODO

- [x] freeze widening candidate list before code changes start (README §9 table)
- [x] define the benchmark/shadow bar for any “advanced” claim (README §9.2)
- [x] define the legality-matrix requirement for SG widening (README §9 owning-seam column + ADV-02/03)

## Concrete First Increment

The first PR for this ticket should do only this:

1. freeze one packet-level admission table:
   `surface`, `current state`, `owning seam`, `red rail first`, `claim gate`
2. freeze one recommended start order:
   `ADV-00` -> `ADV-01` registry only -> first widened predicate ->
   `ADV-02` raw sibling -> `ADV-02` predicate sibling -> `ADV-03` -> `ADV-04`
3. freeze one claim-discipline paragraph that forbids calling any widened lane
   “advanced” before correctness and cost evidence both exist

Do not change product behavior, fixtures, or runtime rows in this first PR.

## Implementation Steps

1. red: enumerate current packet/doc mismatches first and fail the review if any
   widening lane lacks an owning seam, red rail, or claim gate
2. refactor: normalize `README.md`, `INDEX.md`, and `ADV-*` ticket headings so
   every later lane has the same execution checklist surface
3. widen: none in this ticket; freeze only which future lanes may widen and
   which categories are permanently out of scope
4. proof/doc sync: update `HISTORICAL-MAP.md`, the proof inventory links, and
   packet wording so later tickets inherit the same admission language

## Dependency / Import Constraints

- no product-code imports or API moves inside this ticket
- no new runtime fixtures here; this ticket only defines the admission bar
- no docs-owned fallback authority; the ticket may point at owner seams only
- no new cross-crate helper modules from a docs-only lane

## Red Rails First

- packet truth spot-check:
  `rg -n "advanced|widen|proof|benchmark|shadow" docs/plans/jun-2-dsl-advanced docs/plans/jun-2-dsl-final-cut docs/plans/may-25-lexical-enhancement`
- seam sanity spot-check:
  `rg -n "predicate_content_leaf|repo_has_file_constraint|rewrite_sourcegraph_structural_expr" crates/quanta-index-lexical crates/quanta-index-search-plane`
- bridge-carrier boundary spot-check:
  `rg -n "into:codeql|scope:results|with:lexical" crates/quanta-index-lq-bridge/tests`

## NOT TODO

- no product-semantics widening inside this ticket
- no benchmark wording that outruns actual measurement
- no runtime-row additions to make the packet look more complete

## Test Plan

- docs-only spot check against live code and owning proof rails
- verify that every later `ADV-*` ticket links back to this admission bar

## DoD

- widening work has a stable admission bar
- packet/ticket language does not blur bugfix closeout with optional widening
- benchmark/shadow gate is explicit before implementation starts

## Concrete Deliverables

1. one packet-level admission table with owning seam and red rail columns
2. one frozen start-order list matching `README.md` and `INDEX.md`
3. one explicit claim-discipline paragraph for “advanced” labeling
4. one clear exclusion statement for bridge carriers, parser-only shapes, and
   empty-query semantics

## Failure Modes

- closeout packet is reopened without a real regression
- bridge-only carriers are reintroduced as fake runtime residue
- structural improvement is mislabeled as “advanced” without evidence
- widening tickets start coding before the claim/benchmark bar is frozen
