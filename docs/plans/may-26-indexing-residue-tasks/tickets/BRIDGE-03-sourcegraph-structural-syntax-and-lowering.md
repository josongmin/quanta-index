# BRIDGE-03 - Sourcegraph Structural Syntax and Lowering

Status: `shipped`
Priority: `P1`
Depends on: [BRIDGE-02](BRIDGE-02-sourcegraph-structural-honesty-gate.md)
Blocked by: none

## Purpose

Add actual Sourcegraph-side structural syntax and lower it onto the structural
query route instead of the lexical bridge surface.

## Landed truth

- the supported Sourcegraph structural surface is now:
  `patterntype:structural` plus exactly one quoted/keyword structural body and
  executable `repo:` / `file:` / `lang:` filters
- that subset lowers onto native structural `match { ... }` execution on the
  structural route
- unsupported Sourcegraph structural extensions such as regex bodies and
  boolean pattern composition fail typed before execution
- this landed at the search-plane lowering boundary; the bridge surface is kept
  honest without lexical fallback
- public proof now exists on both the symbol-independent frontdoor and the
  structural parity harness:
  - `sdk_frontdoor.rs` proves supported positive rows plus typed rejection
  - `e2e_dual_syntax_lowering_parity.rs` proves SG/native structural parity

## Owner files

- `crates/quanta-index-lq-bridge/src/syntax.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- related bridge tests only

## File-level work breakdown

- `syntax.rs`
  - extend SG parser surface to recognize structural query shape
- `translator.rs`
  - lower SG structural AST into the correct structural directive/request path
- `lowering.rs`
  - route SG structural lowering onto structural query construction, not lexical
    text lowering
- runtime tests
  - add SG structural public proof once the route exists

## Work items

- define the supported SG structural surface explicitly
- parse it into a dedicated SG AST shape instead of overloading `PatternKind`
- lower it onto structural requests with typed failure for unsupported SG
  structural extensions
- preserve SG lexical subset behavior unchanged

## Test plan

- parser tests for SG structural syntax
- translator tests proving SG structural AST does not lower as lexical pattern
- lowering tests for route selection

## E2E proof

- SG/native parity rows for structural live in:
  - `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- public frontdoor proof lives in:
  - `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`

## DoD

- SG structural syntax exists as a real frontdoor surface
- SG structural queries route to structural execution, not lexical execution
- unsupported SG structural extensions fail typed before execution

Status: satisfied for the supported subset.

## Failure modes

- SG structural syntax is accepted but translated as plain lexical text
- translator reuses lexical pattern slots and loses structural semantics
- SG structural frontdoor diverges from native structural truth without parity
  proof
