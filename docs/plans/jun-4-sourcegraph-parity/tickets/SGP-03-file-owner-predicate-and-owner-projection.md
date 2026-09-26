# SGP-03 File Owner Predicate And Owner Projection

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Objective

Close the Sourcegraph comparison cells for:

- `file:has.owner(...)`
- `file:has.owner()`
- `select:file.owners`

with an honest final verdict backed by real ownership authority and an explicit
projection result contract.

## Current Source Truth

- query-side ownership predicate family now exists in current registry
- ownership result projection now exists in current `select:` contract
- Sourcegraph docs treat ownership as both query input and result projection
- query-side filter and projection-side result contract may land in different owner seams
- repo-map `owner_path` / `RepoMapNodeRef` surfaces are structural ownership, not people ownership
  - reusing repo-map ownership as if it were CODEOWNERS / person ownership is incorrect
- source-repo keyed file-ownership ingest batch and lexical authority track now exist in the current quanta-index tree
- `select:` contract currently stops at `repo|file|path|symbol|content|content.match`
- exact query-side proof now exists on runtime/front-door/parity/corpus rails
- semantica owner-map / CODEOWNERS substrate remains a separate integration seam, but current quanta-index tree already has repo-local ownership authority and select projection proof
- therefore this packet closes with a full supported verdict for both filter and projection

## Files To Touch

- `crates/quanta-index-contract/src/ipc/ingest.rs`
- `crates/quanta-index-sdk/src/history.rs`
- `crates/quanta-index-lexical/src/predicate_registry.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## Final Verdict

- `file:has.owner(...)`: supported
- `file:has.owner()`: supported
- `select:file.owners`: supported

Reason:

1. query-side owner gate has a real ingest/public contract plus lexical authority snapshot
2. explicit `TextQueryResponse.file_owner_rows` result contract exists
3. exact runtime/front-door/parity/corpus proof exists for both owner-filter and owner-projection execution

## Reopen Conditions

1. break `TextQueryResponse.file_owner_rows` wire contract
2. remove exact projection runtime/front-door/parity/corpus proof

## Proof Basis

- quanta-index tree now has `PublishFileOwnershipBatch`
- quanta-index tree now admits `file.has.owner` in `PREDICATE_REGISTRY`
- quanta-index tree now admits `file.owners` in `LqSelect`
- exact query-side proof exists on:
  - `e2e_filter_execution`
  - `e2e_dual_syntax_lowering_parity`
  - `sdk_frontdoor`
  - `e2e_full_corpus`
- translator `select:` closed set includes `file.owners`
- shared front-door and runtime corpus both execute `select:file.owners`

## DoD

- capability inventory says filter and projection supported
- parity report lists owner filter and owner projection as canonical supported surfaces
- packet does not leave ownership cells in planned/ambiguous state

## Not Done If

- ticket is still left in planned state
- docs imply projection is unsupported after the result contract and proof have landed
- projection is treated as supported without an explicit result contract or proof
