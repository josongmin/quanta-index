# JFC-04 Structural Set Algebra and Negative Root

Parent packet: [../README.md](../README.md)

Status: `closed`

## Objective

Land deterministic mixed lexical / structural boolean execution and pure-negative structural root semantics on top of the already-shipped structural truthful subset.

## Current Source Truth

- mixed lexical + structural boolean executes with generation-pinned candidate identity intersection (`JFC-04` landed)
- pure-negative structural root subtracts from an explicit structural universe before projection
- structural bindings remain exact after set algebra; chaos and restart rails cover widened surfaces

## 핵심 로직

- lexical and structural branches must lower into one canonical candidate universe keyed by generation-pinned candidate identity
- set algebra runs before result-surface projection
- pure-negative structural root must subtract from an explicit generation-pinned structural universe, not from an implicit filesystem/global scan
- structural bindings from surviving structural candidates must remain exact after boolean composition

## 건드릴 파일

- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-corpus-smoke/src/corpus/model.rs`
- `crates/quanta-index-corpus-smoke/src/corpus/loader.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
- `crates/quanta-index-searchd-runtime/tests/sdk_frontdoor.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_perf_chaos.rs`
- structural fixture files under `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 건드리지 말 것

- producer-authored parse-tree authority
- bridge translator semantics before native algebra lands
- heuristic path fallback or text reparsing on the search side
- result projection ordering rules already closed for pure lexical rows

## TODO

- [x] introduce a shared candidate-universe layer for mixed lexical / structural execution
- [x] execute `AND`, `OR`, and `NOT` across mixed domains deterministically
- [x] define pure-negative structural root semantics using an explicit pinned universe
- [x] preserve structural bindings after set algebra
- [x] add runtime rows and companion rails (mixed boolean, pure-negative, `e2e_perf_chaos`, `e2e_restart_replay_determinism`)

## NOT TODO

- no lexical fallback disguised as structural success
- no reparsing text into structural authority on the search side
- no path-based heuristic join between lexical and structural results
- no widening Sourcegraph translation before native semantics land

## Test Plan

- owner-local query-dispatcher tests for mixed-domain algebra
- `./scripts/cargow test -p quanta-index-searchd-runtime --test sdk_frontdoor --test e2e_perf_chaos -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`

## Public proof scope

- Public rails now cover mixed **AND**, **OR**, **AND NOT**, and pure-negative
  root (`runtime_rows.toml`, `e2e_dual_syntax_lowering_parity`,
  `e2e_perf_chaos`, `sdk_frontdoor`). SG structural route still fail-closes
  **repo-scoped filters under mixed OR** (see `lowering` scoped-filter tests).

## DoD

- mixed lexical / structural boolean no longer typed-fails if the semantics ship
- pure-negative structural root is explicitly executed or explicitly remains fail-closed with no stale doc claim
- structural binding assertions survive boolean composition
- deterministic ordering is verified end-to-end

## Failure Modes

- candidate identity differs across domains and creates false joins
- projection runs before set algebra and destroys binding truth
- pure-negative root scans an implicit global universe
- companion rails stay green while runtime closeout still lacks exact proof
