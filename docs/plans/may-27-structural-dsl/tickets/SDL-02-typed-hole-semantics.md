# SDL-02 - Typed Hole Semantics

Status: `planned`
Priority: `P0`
Depends on: [SDL-01](SDL-01-structural-boolean-composition.md)

## Purpose

Turn the reserved typed-hole namespace into executable structural semantics over
the shipped language set.

This ticket ships a closed common-kind family first. It does not promise an
arbitrary per-language grammar taxonomy in one pass.

## Owner files

- `crates/quanta-index-contract/src/lex/error_code.rs`
- `crates/quanta-index-lq-norm/src/ast.rs`
- `crates/quanta-index-lq-norm/src/parser/implementation.rs`
- `crates/quanta-index-lq-structural/src/types.rs`
- `crates/quanta-index-lq-structural/src/pattern.rs`
- `crates/quanta-index-lq-structural/src/matcher.rs`
- `crates/quanta-index-lq-structural/src/errors.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-lq-structural/tests/**`

## File-level work breakdown

- `error_code.rs`, `errors.rs`
  - add a dedicated structural typed-hole error instead of collapsing back to a
    generic invalid request when the hole kind is first-class syntax
- `ast.rs`, `parser/implementation.rs`
  - preserve typed-hole syntax in typed AST/IR
- `types.rs`, `pattern.rs`
  - define the closed kind taxonomy and lower it into matcher IR
- `matcher.rs`
  - validate node-kind admissibility per language against producer parse-tree
    kinds
- `query_dispatcher.rs`
  - preserve typed failure and avoid silent downgrade to untyped holes

## Work items

- ship the common cross-language kind family first:
  - `expr`
  - `stmt`
  - `item`
  - `type`
- represent typed holes explicitly in structural IR
- map producer parse-tree node kinds into the common family per shipped
  language
- reject unsupported kind-language pairs with a dedicated typed structural
  error
- keep the current untyped hole behavior unchanged
- do not interpret typed holes via string matching on raw source text

## Test plan

- parser tests for typed-hole syntax acceptance and malformed variants
- IR lowering tests proving the kind survives into structural matcher input
- matcher tests per shipped language for positive and negative kind matches
- tests for unsupported kind-language pairs
- public route tests showing typed-hole failure is explicit and stable

## E2E plan

Owned by [SDL-E2E-01](SDL-E2E-01-structural-proof-and-observability.md):

- `match { :[X.expr] }`
- `match { :[S.stmt] }`
- `match { :[T.type] }`
- unsupported kind row on a shipped language
- unsupported language row stays `STR_LANG_NOT_SUPPORTED`

## DoD

- typed holes are no longer parser-only reserved syntax
- kind matching is defined on producer parse-tree authority
- unsupported kind-language pairs fail typed without fallback to untyped holes

## Failure modes

- parser accepts typed holes but matcher erases the kind constraint
- language-specific node kinds leak directly into public syntax without a
  stable common family
- unsupported typed holes silently behave like `:[X]`
