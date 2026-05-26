# LXE-02 - Planner Authority IR

Status: `completed`
Priority: `P0`
Depends on: [LXE-01](LXE-01-active-contract-and-dead-route-cleanup.md)

## Purpose

Move lexical engine selection into the lexical domain. `search-plane` may
parse, normalize, and dispatch, but it must not encode engine-specific lexical
semantics.

## Current live truth (2026-05-27)

- lexical engine selection is planner-owned on the current tree; live query
  execution goes through the lexical planner boundary rather than ad hoc
  search-plane engine branching
- `SearchExplanation` runtime traces and `engines_touched` now come from the
  executed plan surface that the runtime owner rails assert
- proof rails:
  - `cargo check -p quanta-index-search-plane --tests`
  - `cargo test -p quanta-index-searchd-runtime --test explain -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_lexical_full_fidelity -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_restart_replay_determinism -- --nocapture`

## Owner files

- `crates/quanta-index-core/src/domains/lexical/lowering.rs`
- `crates/quanta-index-core/src/domains/lexical/inbound.rs`
- `crates/quanta-index-core/src/domains/lexical/outbound.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- new `crates/quanta-index-lexical/src/planner.rs`
- new `crates/quanta-index-lexical/src/plan.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`

## File-level work breakdown

- `crates/quanta-index-core/src/domains/lexical/{inbound,outbound}.rs`: define
  the planner-owned IR boundary and execution ports.
- `crates/quanta-index-core/src/domains/lexical/lowering.rs`: map active query
  carriers into the planner IR without engine-specific shortcuts.
- `crates/quanta-index-lexical/src/{planner.rs,plan.rs}`: implement boolean,
  filter, engine, and trace planning as the lexical SSOT.
- `crates/quanta-index-lexical/src/lib.rs`: execute only planned nodes and stop
  accepting ad hoc AST inspection during execution.
- `crates/quanta-index-search-plane/src/{lowering,query_dispatcher}.rs`: call
  into the planner boundary and stop owning engine selection.

## Work items

- Introduce a planner-owned intermediate model:
  - normalized boolean tree
  - field predicates
  - engine requirements
  - candidate caps
  - cancellation checkpoints
  - explain trace nodes
- Convert active `LqQuery` into this IR at the lexical boundary.
- Make engine selection deterministic and explicit:
  - content
  - path
  - symbol
  - regex
  - phrase/position
  - trigram
  - history
  - structural
- Remove ad hoc execution decisions from `search-plane`.
- Ensure all unsupported IR nodes produce typed planner errors.
- Ensure planner trace is the source of `SearchExplanation` trace fields.

## Test plan

- unit tests for every leaf/filter mapping into planner IR.
- deterministic serialization/debug output for planner traces.
- rejection tests for unsupported OR/NOT scoped filters.
- no direct engine selection assertions in `search-plane` tests.
- property test: equivalent normalized boolean queries produce equivalent IR.

## E2E plan

- `E2E-01` validates that multi-engine lexical rows touch the expected engines.
- `E2E-05` validates planner trace stability after reopen/replay.

## DoD

- `search-plane` cannot execute a lexical query without calling the lexical
  planner boundary.
- `SearchExplanation` uses planner trace, not reconstructed raw expressions.
- every engine touched in an E2E response comes from planner IR.
- typed planner errors are asserted at the request boundary.

## Failure modes

- the IR becomes a second AST with no execution ownership.
- `search-plane` keeps special cases for repo/path/symbol/regex.
- explanations are generated from display strings rather than the plan.
