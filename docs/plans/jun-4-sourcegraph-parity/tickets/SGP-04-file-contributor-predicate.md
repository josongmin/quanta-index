# SGP-04 File Contributor Predicate

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Objective

Promote the Sourcegraph comparison cell for:

- `file:has.contributor(...)`

onto an executable query-side predicate with exact authority-backed proof.

## Current Source Truth

- registry/bridge/runtime owner exists on the current tree
- contributor search is separate from ownership and does not piggyback on it
- a dedicated contributor authority batch exists on the current tree
- executor narrows candidate ids by `(source_repo_id, repo_relative_path)` contributor sets
- runtime/front-door/parity/corpus proof exists for the single textual contributor subset

## Files To Touch

- `crates/quanta-index-contract/src/ipc/ingest.rs`
- `crates/quanta-index-sdk/src/history.rs`
- `crates/quanta-index-lexical/src/predicate_registry.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
- `crates/quanta-index-searchd-runtime/tests/common/frontdoor_scenarios.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`

## Final Verdict

- `file:has.contributor(...)`: supported

Reason:

1. dedicated file-contributor authority batch exists on the public ingest contract
2. lexical executor has a shipped `file.has.contributor` predicate kind
3. runtime/front-door/parity/corpus rails prove positive and miss behavior without repo-level author fallback

## Reopen Conditions

1. widen beyond the current one-textual-argument subset
2. add external producer auto-emission for contributor authority if cross-repo integration proof is required
3. add richer contributor identity semantics beyond exact lowercase textual matching

## Proof Basis

- `PublishFileContributorBatch` exists on the public ingest contract
- SDK history namespace exposes `publish_file_contributor`
- predicate registry ships `file.has.contributor`
- shared runtime/front-door/parity/corpus rails pin the executable truth:
  - `file:has.contributor(alice)` admits contributor-owned docs
  - non-matching contributor filters return empty, not fallback

## DoD

- capability inventory says supported, not partial
- parity report lists contributor filter as canonical supported surface
- packet does not leave contributor support in planned/ambiguous state

## Not Done If

- ticket is still left in planned state
- docs imply repo-level author search is the shipped semantics
- runtime/front-door/parity/corpus evidence is missing
