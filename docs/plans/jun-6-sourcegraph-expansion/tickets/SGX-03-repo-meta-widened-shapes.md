# SGX-03 — Repo Meta Widened Shapes

> Archive status: `Historical program record`. Current architecture: [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent RFC: [../rfc.md](../rfc.md)

Status: `landed`

## Goal

Close the remaining `repo:has.meta` regex family with real runtime semantics on
the live tree.

## Current Code Fact

- `repo:has.meta(key:value)` is supported
- `repo:has.meta(key)` is supported
- `repo:has.meta(tag:)` is supported
- regex key-only, regex-key exact-value, exact-key regex-value, and regex pair
  all execute on the live tree
- malformed regex still typed-fails instead of degrading to empty

## Official Sourcegraph Baseline

- Sourcegraph documents literal or slash-delimited regex key/value semantics
  for `repo:has.meta(...)`, and the live tree now matches that family

## Owner Seam

- `crates/quanta-index-lexical/src/predicate_registry.rs`
- `crates/quanta-index-lexical/src/lib.rs`
- query-time regex execution over existing exact metadata pairs

## Source Truth Anchors

- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/predicate_registry.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-lexical/src/lib.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs`
- `/Users/songmin/Documents/code-new/quanta-index/crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `/Users/songmin/Documents/code-new/quanta-index/tools/benchmark/sourcegraph_parity.py`

## Concrete Work Items

1. Kept current exact `key:value`, key-existence, and `tag:` support explicit
   in docs and guard inventory.
2. Promoted regex key-only, exact-key regex-value, regex-key exact-value, and
   regex pair shapes on the live tree.
3. Preserved same-pair semantics across key/value matching.
4. Kept malformed regex as typed-fail and moved the guard to shape-aware
   supported inventory.

## First Increment

- freeze exact/value, key-existence, and `tag:` green behavior
- add regex positive, miss, and invalid rails shape by shape

## Red Rail To Pin First

```bash
./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_filter_execution repo_has_meta -- --nocapture
```

## Worker First Commands

```bash
rg -n "repo:has\\.meta\\(|RepoMeta|key:value|tag:|regex" crates docs tools -S
sed -n '740,860p' crates/quanta-index-searchd-runtime/tests/e2e_filter_execution.rs
sed -n '900,980p' crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml
```

## No-Go

- do not match key and value across different metadata pairs
- do not demote malformed regex to empty success

## DoD

- regex family has a real query-time matching seam and proof
- already-supported exact/existence shapes do not regress
- neighboring unsupported shapes remain explicit

## Not Done If

- regex support still misses any of `/key/`, `/key/:`, `key:/value/`,
  `/key/:value`, `/key/:/value/`
- key/value matching can span different metadata pairs
