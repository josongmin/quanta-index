# LXE-09 - Structural Live Integration

Status: `completed`
Priority: `P1`
Depends on: [LXE-01](LXE-01-active-contract-and-dead-route-cleanup.md), [LXE-02](LXE-02-planner-authority-ir.md)

## Purpose

Expose the structural query/result surface as a truthful in-repo live path that
executes only against materialized parse-tree/chunk authority already present in
the readiness ledger.

## Owner files

- `crates/quanta-index-contract-base/src/results/structural.rs`
- `crates/quanta-index-lq-structural/src/**`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-search-plane/src/channel_dispatcher.rs`
- `crates/quanta-index-core/src/domains/structural/**`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`

## File-level work breakdown

- `crates/quanta-index-contract-base/src/results/structural.rs`: keep
  structural result carriers explicit and separate from content candidates.
- `crates/quanta-index-core/src/domains/structural/**`: define structural
  planner and execution boundary with typed readiness errors plus the internal
  authority-side structural match carriers.
- `crates/quanta-index-lq-structural/src/**`: compile contract structural
  blocks into shared structural IR, lower the truthful authority subset, and
  execute authority-backed tree-walk matching without reparsing source.
- `crates/quanta-index-searchd/src/app/runtime.rs`: replace the
  `FailClosedStructuralProducer` production wiring with a ledger-backed
  adapter that reads only materialized parse trees and chunks.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: route structural
  requests through the structural domain path, project internal authority-side
  carriers onto public structural DTOs, and preserve typed invalid-request
  boundaries for unsupported public surface.
- `crates/quanta-index-search-plane/src/channel_dispatcher.rs`: record
  structural track seals only when structural authority is actually
  materialized for the generation.
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`: assert active
  and pinned live structural success on indexed fixture data plus typed
  fail-closed boundaries at the public SDK surface.
- `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`: assert typed
  runtime errors for unmaterialized or inconsistent structural authority.

## Work items

- Keep public response carriers:
  - `StructuralBinding`
  - `StructuralCandidate`
- Keep internal execution carriers:
  - `StructuralMatchBinding`
  - `StructuralMatchCandidate`
- Add structural IPC request/response variant.
- Route a single top-level `match { ... }` leaf plus executable `repo:` /
  `file:` / `lang:` filter subset through the structural planner node.
- Compile `LqStructuralBlock` into shared structural IR and lower it into the
  currently truthful authority subset.
- Execute only against materialized parse-tree/chunk authority already present
  in the readiness ledger.
- Land the current truthful live subset:
  - root-kind exact match
  - root capture and root-kind plus capture
  - ordered direct-child tree-walk
  - variadic sibling capture / wildcard skip
  - `where` / `inside` / `outside` constraints
- Preserve typed fail-closed behavior:
  - unsupported lang: `STR_LANG_NOT_SUPPORTED`
  - generation not materialized: `STR_GENERATION_NOT_READY`
  - parse-tree/chunk authority inconsistency: `STR_SHARD_UNAVAILABLE`
  - unsupported structural shape or filter outside `repo:` / `file:` / `lang:`:
    `STR_INVALID_REQUEST`
- Do not synthesize structural matches from regex/text and do not add producer
  RPC or git/source reparsing at query time.

## Test plan

- contract/result carrier tests for structural carriers.
- `quanta-index-lq-structural` unit tests for compile/lower and truthful
  authority matching.
- `search-plane` tests for structural route lowering and typed boundary
  mapping.
- runtime tests for missing generation and shard-unavailable authority states.

## E2E plan

Covered by `E2E-04`:

- active `match { :[x] }` returns a `StructuralCandidate` with one binding.
- pinned `match { :[x] }` returns the same candidate on the pinned generation.
- active `match { function_item }` returns a `StructuralCandidate` with no
  bindings.
- active `file:^src/.*\\.rs$ match { :[x] }` executes on the live structural
  path and returns the same candidate set as the unfiltered happy-path row.
- active `repo:^repo-a$ match { :[x] }` executes on the live structural path;
  a repo mismatch returns an empty success, not a dropped filter.
- `lang:java match { :[x] }` returns `STR_LANG_NOT_SUPPORTED`.
- structural queries with filters outside `repo:` / `file:` / `lang:` return
  `STR_INVALID_REQUEST`.
- unmaterialized structural generations return `STR_GENERATION_NOT_READY`.
- orphaned parse-tree/chunk authority returns `STR_SHARD_UNAVAILABLE`.

## DoD

- structural dispatcher no longer immediately returns
  `STR_PRODUCER_PARSE_TREE_UNAVAILABLE` on the supported happy path.
- production runtime wiring uses the ledger-backed structural adapter instead of
  `FailClosedStructuralProducer`.
- structural execution inside `quanta-index-core` / `searchd` no longer carries
  the public structural wire DTOs directly.
- the current truthful subset is live for both `active(...)` and `pinned(...)`
  queries.
- executable structural filters are exactly `repo:` / `file:` / `lang:` on the
  current live route.
- unsupported shapes and unsupported filters remain typed fail-closed.
- full `STR-01` semantics stay explicitly out of scope for this ticket.

## Failure modes

- implementing structural as regex over source text.
- returning empty success on missing parse-tree/chunk authority.
- documenting full structural semantics as shipped when only the current
  truthful subset is executable.
- accepting extra filters/composite shapes and silently dropping them.
- leaking public `StructuralCandidate` DTOs into the core/searchd execution
  path instead of projecting them at the response boundary.
