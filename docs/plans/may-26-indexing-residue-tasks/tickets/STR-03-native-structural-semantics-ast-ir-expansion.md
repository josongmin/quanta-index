# STR-03 - Native Structural Semantics AST/IR Expansion

Status: `shipped`
Priority: `P0`
Depends on: none
Blocked by: none

## Purpose

Extend the native structural DSL so the next intended semantics can be
expressed in AST/IR rather than being impossible to represent.

## Landed truth

- native AST now carries `LqStructuralExpr::{Pattern, Where, Inside, Outside}`
- native node IR now carries single-capture `MetaVar`, variadic `Hole`, and
  anonymous `WildcardMany`
- `LqStructuralBlock` now preserves both the legacy `nodes` view and the richer
  `exprs` view so current wire/tests stay stable while executor support rolls
  forward
- richer syntax parses and round-trips structurally, and the current authority
  matcher executes the landed variadic / `where` / `inside` / `outside`
  subset; unsupported composition still fails closed

## Owner files

- `crates/quanta-index-lq-norm/src/ast.rs`
- `crates/quanta-index-lq-norm/src/parser/implementation.rs`
- `crates/quanta-index-lq-norm/src/tokenizer/implementation.rs`
- `crates/quanta-index-lq-structural/src/pattern.rs`
- `crates/quanta-index-lq-structural/src/types.rs`
- related parser/IR tests only

## File-level work breakdown

- `ast.rs`
  - add explicit structural node forms required for the next semantics
- `parser/implementation.rs`
  - parse the new native syntax into typed structural nodes
- `tokenizer/implementation.rs`
  - add token support only if the new syntax cannot be expressed with current
    raw structural block capture
- `pattern.rs`
  - mirror the new AST shape into structural IR
- `types.rs`
  - extend metavariable/sequence validation if variadic semantics need it

## Work items

- add AST/IR support for the next structural semantics wave
- first required surface: `variadic`
- then add the representation needed for `inside`, `outside`, and `where`
- keep the new syntax fail-closed when parsed shape exceeds current executor
  authority
- avoid silently mapping richer syntax back onto `Literal | MetaVar | Group`

## Test plan

- parser unit tests for each new syntax form
- IR lowering tests proving the new syntax is preserved structurally
- negative parser tests for malformed new forms

## E2E plan

- no direct E2E claim from this ticket alone
- runtime rows land only after `STR-02` or the relevant executor support exists

## DoD

- the native structural DSL can represent the next intended semantics in typed
  AST/IR
- malformed variants fail typed at parse time
- no new syntax is accepted and then erased during AST/IR lowering

Status: satisfied.

## Failure modes

- syntax parses but lowers back into the old three-node model
- new syntax is tokenized but not preserved semantically
- variadic surface is accepted with ad hoc string conventions instead of typed
  nodes
