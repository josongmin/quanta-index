# LXE-01 - Active Contract and Dead-route Cleanup

Status: `completed`
Priority: `P0`
Depends on: [LXE-00](LXE-00-truth-freeze-and-executable-matrix.md)

## Purpose

Make the public query and result contract match the active runtime surface.
Remove legacy routes instead of keeping compatibility shims.

## Current live truth (2026-05-27)

- semantic request wire shape is `SemanticQueryRequest { query_text,
  generation, generation_selector, lexical_scope, top_k }`
- hybrid request wire shape is `HybridQueryRequest { text_query,
  semantic_query_text, generation, generation_selector, top_k }`
- contract decode rejects deleted `scope`, `query_vector_ref`, and
  `semantic_vector_ref` fields
- public SDK and `searchctl` surfaces now expose text-based semantic/hybrid
  query carriers instead of the removed vector/handle request contract
- proof rails:
  - `cargo test -p quanta-index-contract --test lxe_unified_surface -- --nocapture`
  - `cargo test -p quanta-index-contract --test ipc_query_result_v2_contract -- --nocapture`
  - `cargo test -p quanta-index-sdk --lib`
  - `cargo test -p quanta-index-searchctl --tests`

## Owner files

- `crates/quanta-index-contract/src/query/mod.rs`
- `crates/quanta-index-contract/src/query/requests.rs`
- `crates/quanta-index-contract/src/results/mod.rs`
- `crates/quanta-index-contract/src/results/**`
- `crates/quanta-index-contract-base/src/query/**`
- `crates/quanta-index-contract-base/src/results/**`
- `crates/quanta-index-sdk/src/search.rs`
- `crates/quanta-index-sdk/src/lexical.rs`
- `crates/quanta-index-sdk/src/semantic.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`

## File-level work breakdown

- `crates/quanta-index-contract/src/query/requests.rs`: remove legacy lexical,
  semantic, and hybrid request shapes and keep only the unified text-based
  intake.
- `crates/quanta-index-contract/src/query/mod.rs` and
  `crates/quanta-index-contract-base/src/query/**`: delete legacy query
  variants and keep only lossless active carriers. Decode must reject unknown
  variants/fields instead of silently dropping them.
- `crates/quanta-index-contract/src/results/**` and
  `crates/quanta-index-contract-base/src/results/**`: augment `SearchExplanation`
  with the planner-trace fields below and install the active carrier set for
  lexical, bridge, history, diff, and structural results.
- `crates/quanta-index-sdk/src/{search,lexical,semantic}.rs`: expose only the
  unified text request surface (`TextQueryRequest`) and remove direct `LqQuery`
  call paths.
- `crates/quanta-index-search-plane/src/{lowering,query_dispatcher}.rs`: delete
  dead compat branches and compile only against the active contract.

## Work items

- Ensure `TextQuerySyntax { Native, Sourcegraph }` is the only public lexical
  syntax selector.
- Ensure lexical request intake is `TextQueryRequest { syntax, query_text,
  generation, generation_selector, top_k }`.
- Ensure semantic request uses `lexical_scope: Option<TextQueryRequest>`.
- Ensure hybrid request uses `text_query: TextQueryRequest`.
- Remove public `LqQuery` direct request fields from lexical/semantic/hybrid
  request structs.
- Remove active legacy leaves and bypasses (any equivalent of `Custom` /
  `MatchAll` / `Raw` / direct-AST passthrough) from the active `LqQuery` /
  `TextQueryAst` hierarchy and from request structs.
- Augment `SearchExplanation` with:
  - `planner_trace`
  - `engines_touched`
  - `early_stop_reason`
  - `summary`
- Keep producer/channel carriers under `lex::*`.
- Keep active query/result carriers under `query::*`, `results::*`, and `ipc::*`.

## Test plan

- CBOR round-trip for every active query leaf/filter/option/directive.
- CBOR round-trip for lexical, semantic, hybrid, history, structural, and
  bridge IPC requests/responses.
- decode rejection for unknown variants/fields (fail-closed on schema drift).
- reflection test that legacy variants and old request fields are absent.
- SDK compile tests that only `TextQueryRequest` is exposed as the lexical
  text surface.

## E2E plan

- `E2E-00` must consume only `TextQueryRequest` as the lexical text intake.
- `E2E-02` must prove Sourcegraph and LQ enter through the same request shape.
- `E2E-03` must prove semantic/hybrid lexical scope references the same
  sub-struct.

## DoD

- no public stable request accepts direct `LqQuery`.
- no active contract type can represent `Custom`, `Raw`, or `MatchAll`.
- all active response carriers compile from `results::*`.
- all removed shapes are covered by negative compile/decode tests.
- contract, SDK, and search-plane compile together without compatibility shims.

## Failure modes

- keeping an internal old route that can still be called by SDK tests.
- accepting Sourcegraph syntax through a separate request path.
- preserving v1 explanation and only filling v2 fields opportunistically.
