# STR-02 - Authority Matcher Tree-Walk Expansion

Status: `shipped`
Priority: `P0`
Depends on: none
Blocked by: none

## Purpose

Replace the current root-only truthful structural executor with a broader
parse-tree authority matcher that can execute non-trivial composite shapes
without query-time reparsing or text fallback.

## Landed truth

- live authority execution now supports ordered direct-child tree-walk in
  addition to root-kind exact, root capture, and root-kind plus capture
- variadic sibling capture / wildcard skip and `where` / `inside` /
  `outside` constraints execute on the same parse-tree authority surface
- unsupported ambiguous composite shapes still fail closed before execution
- runtime typed boundaries remain intact: `STR_LANG_NOT_SUPPORTED`,
  `STR_GENERATION_NOT_READY`, `STR_SHARD_UNAVAILABLE`, `STR_INVALID_REQUEST`

## Owner files

- `crates/quanta-index-lq-structural/src/matcher.rs`
- `crates/quanta-index-lq-structural/src/pattern.rs`
- `crates/quanta-index-lq-structural/src/binding.rs`
- `crates/quanta-index-lq-structural/src/types.rs`
- `crates/quanta-index-searchd/src/app/runtime.rs`
- `crates/quanta-index-lq-structural/tests/authority_match.rs`
- related structural runtime tests only

## File-level work breakdown

- `matcher.rs`
  - extend authoritative matcher beyond `RootKind` / `RootCapture`
  - define the truthful executable subset explicitly in code, not comments
- `pattern.rs`
  - support lowering of broader executable shape into authority matcher input
- `binding.rs`
  - extend binding projection if multi-node matches require richer span output
- `types.rs`
  - add helper types only if needed for tree-walk state
- `runtime.rs`
  - map the wider authority matcher surface onto existing typed structural
    errors without fallback
- `authority_match.rs`
  - add positive and negative proof for the new executable shapes

## Work items

- add authoritative tree-walk matching over producer parse-tree structure
- preserve parse-tree/chunk-only authority
- keep unsupported shapes fail-closed
- keep `STR_LANG_NOT_SUPPORTED`, `STR_GENERATION_NOT_READY`,
  `STR_SHARD_UNAVAILABLE`, and `STR_INVALID_REQUEST` semantics intact
- do not introduce producer RPC, git reads, or regex/text fallback at query
  time

## Test plan

- extend `cargo test -p quanta-index-lq-structural`
- add positive composite-shape authority tests
- add negative tests for partially-supported but still non-executable shapes

## E2E plan

- add or extend structural rows in:
  - `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
  - `crates/quanta-index-searchd-runtime/tests/end_to_end.rs`

## DoD

- live structural execution is no longer limited to root-kind exact and single
  root capture
- at least one non-root-only positive runtime proof passes
- unsupported shapes still fail typed instead of degrading silently

Status: satisfied.

## Failure modes

- parser accepts a shape but matcher still collapses it to root-only behavior
- runtime reports success by ignoring unmatched substructure
- tree-walk uses source text heuristics instead of parse-tree authority
- broader matching breaks typed fail-closed boundaries
