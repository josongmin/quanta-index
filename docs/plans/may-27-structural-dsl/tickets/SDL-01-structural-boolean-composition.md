# SDL-01 - Structural Boolean Composition

Status: `planned`
Priority: `P0`
Depends on: shipped [STR-02](../../may-26-indexing-residue-tasks/tickets/STR-02-authority-matcher-tree-walk-expansion.md), [STR-03](../../may-26-indexing-residue-tasks/tickets/STR-03-native-structural-semantics-ast-ir-expansion.md), [STR-04](../../may-26-indexing-residue-tasks/tickets/STR-04-structural-query-surface-expansion.md)

## Purpose

Remove the current one-top-level-structural-leaf fence and execute structural
`AND` / `OR` / bounded `NOT` over the live authority-backed route.

This ticket is about structural-only boolean composition. It does not open
mixed lexical/structural boolean semantics.

## Owner files

- `crates/quanta-index-lq-norm/src/ast.rs`
- `crates/quanta-index-lq-norm/src/parser/implementation.rs`
- `crates/quanta-index-lq-norm/src/normalizer/implementation.rs`
- `crates/quanta-index-lq-structural/src/pattern.rs`
- `crates/quanta-index-lq-structural/src/matcher.rs`
- `crates/quanta-index-lq-structural/src/binding.rs`
- `crates/quanta-index-core/src/domains/structural/types.rs`
- `crates/quanta-index-core/src/domains/structural/service.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`

## File-level work breakdown

- `ast.rs`, `parser/implementation.rs`, `normalizer/implementation.rs`
  - keep structural leaves inside the existing boolean AST instead of forcing a
    single top-level `StructuralBlock`
  - preserve canonical ordering and fan-out guards for structural-only boolean
    trees
- `pattern.rs`, `matcher.rs`, `binding.rs`
  - compile structural boolean branches into executable matcher plans
  - define deterministic candidate and binding merge behavior
- `core/domains/structural/**`, `runtime.rs`
  - execute set algebra on authority-backed candidate sets only
- `query_dispatcher.rs`
  - admit structural-only boolean trees
  - keep mixed lexical/structural boolean trees fail-closed until explicitly
    ticketed elsewhere
- runtime tests
  - prove candidate-set algebra and binding projection on the public route

## Work items

- replace `exactly one top-level structural block leaf` with a structural-only
  boolean lowering path
- define composition semantics on structural `candidate_id`
- lock merge behavior:
  - `AND` = intersect candidate ids
  - `OR` = stable union by candidate id
  - `NOT` = subtract from an already-positive structural seed; pure-negative
    structural queries remain invalid
- merge bindings deterministically:
  - disjoint metavariable names merge
  - same-name same-span bindings unify
  - same-name different-span bindings fail typed; no silent overwrite
- keep candidate ordering stable after set algebra
- reject mixed lexical/structural boolean trees with typed invalid-request
  behavior until a separate cross-engine plan exists

## Test plan

- parser/normalizer unit tests for structural-only `AND` / `OR` / `NOT`
- structural matcher tests for intersect/union/subtract semantics
- binding merge tests for disjoint names, identical names, and conflict rows
- dispatcher tests proving mixed lexical/structural boolean remains fail-closed
- runtime tests for public candidate/binding projection

## E2E plan

Owned by [SDL-E2E-01](SDL-E2E-01-structural-proof-and-observability.md):

- `match { fn $F() } AND match { impl $T { ... } }`
- `match { panic!(...) } OR match { unwrap() }`
- `match { panic!(...) } AND NOT match { #[test] ... }`
- duplicate-binding conflict row returns typed error
- pure-negative structural row returns typed invalid request

## DoD

- structural-only boolean trees no longer die at dispatcher intake
- composition runs entirely on authority-backed structural candidates
- binding merge behavior is specified and tested
- pure-negative and mixed-engine boolean trees still fail closed

## Failure modes

- intersecting/unioning doc hits while discarding structural candidate identity
- silently overwriting conflicting bindings
- treating top-level `NOT match { ... }` as a corpus-wide scan
- opening mixed lexical/structural boolean semantics by accident
