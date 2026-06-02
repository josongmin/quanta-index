# JFC-06 Observability and Chaos Closeout

Parent packet: [../README.md](../README.md)

Status: `closed`

## Objective

Close bounded observability, perf, and chaos behavior for every newly widened DSL surface.

## Current Source Truth

- bounded-label observability already exists and the widened families now have explicit chaos breadth:
  predicate (`file.contains`, `repo.has.file`),
  history (`before`, `after`, `since.time`, `since.commit`, `until`, `diff.*`),
  runtime catalog (`dirty:no`, `changed`, `stale`, `snapshot`, `meta.*`, `affected`, `invalidated_by:`),
  and structural (`mixed AND`, `mixed OR`, `mixed AND NOT`, pure-negative root, typed-hole rejection)
- structural fail-closed chaos rails remain in place and now sit beside the widened predicate/history/runtime rails
- widened history/runtime/predicate surfaces use the same bounded-dimension discipline and explicit no-poison follow-up queries

## 핵심 로직

- every widened surface must emit bounded labels only
- typed rejection, timeout, restart, and recovery behavior must be proved after new surface expansion
- this ticket only closes surfaces that are already natively executable from `JFC-01` through `JFC-05`; it must not green-light hypothetical future coverage
- perf and chaos rails must describe what is actually measured, not what the code aspires to support

## 건드릴 파일

- `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-lq-obs/src/lib.rs`
- `crates/quanta-index-lq-obs/src/dim.rs`
- `crates/quanta-index-lq-obs/src/cardinality_guard.rs`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- active packet docs that describe perf/chaos coverage

## 건드리지 말 것

- metric dimension schema without a bounded-cardinality rationale
- raw query text or file-path labels
- benchmark claims without measured evidence
- unrelated product metrics

## TODO

- [x] extend chaos/recovery rails for each newly widened surface family
- [x] close `JFC-03` and `JFC-04` executable deltas before promoting their chaos/obs coverage here
- [x] prove typed rejects do not poison follow-up requests
- [x] prove timeout/restart behavior where relevant for widened surfaces
- [x] keep observability labels route-stable and bounded
- [x] update docs so perf/chaos claims match actual measured or executed rails only
- [x] make the chaos rail sibling-complete for the newly widened predicate/history/runtime families

## NOT TODO

- no free-text dimensions
- no benchmark screenshot claims without repeatable commands
- no ambient repo-wide green claim from owner-local rails alone
- no mixing of proof inventory closeout with performance aspirations

## Test Plan

- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_perf_chaos -- --nocapture`
- rerun the widened owner/runtime rails that the new chaos assertions depend on

## DoD

- newly widened predicate/history/runtime/structural families have explicit chaos/recovery proof where relevant
- observability stays bounded and route-stable
- doc claims about perf/chaos match real executed rails
- no raw-query or raw-path leakage is introduced by widened surfaces

## Failure Modes

- widened surfaces add unbounded labels
- follow-up query behavior after typed reject or timeout regresses silently
- perf docs claim coverage the repo does not measure
- chaos proof stays on old surfaces while new ones remain unverified
