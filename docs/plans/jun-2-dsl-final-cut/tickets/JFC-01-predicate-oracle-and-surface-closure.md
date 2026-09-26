# JFC-01 Predicate Oracle and Surface Closure

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent packet: [../README.md](../README.md)

Status: `closed`

## Objective

Close the remaining predicate and result-surface residue after `file.has.content(...)` proof landed, with focus on non-vacuous repo-gate oracle closure.

## Current Source Truth

- `repo.has.file(...)` is executable today only for `path:` / `name:` filter arguments
- Sourcegraph-facing `file:contains(...)` parity is live on the companion rail, and native `file.contains(...)` is now directly runtime-proved on the dotted predicate surface
- `file.has.content(...)` is now directly proved on the Sourcegraph alias rows plus the owner-local native predicate rail
- `repo:` positive allow-list and non-vacuous `repo.has.file(...)` true-gate are
  runtime-proved on `docs-multi-repo.toml` through `ChunkRecord::source_repo_id`
  (`runtime_*_repo_has_file_true_gate_multi_repo`, `tantivy_repo_has_file_true_gate_narrows_by_indexed_source_repo_id`)
- public predicate front-door proof now shares one scenario authority:
  `dsl_scenarios` closes `repo.has.file(...)` true-gate/miss on the live SG route,
  `sdk_frontdoor` closes native `file.contains(...)` hit/miss on the builder/transport
  path, and `e2e_perf_chaos` proves both predicate families do not poison follow-up queries

## 핵심 로직

- if a public predicate is executable in code, it must have direct proof or an explicit typed gate
- aliases sharing one substrate still need alias-specific proof or alias-specific de-scoping
- Sourcegraph spelling parity must not be misreported as native predicate proof
- strong positive repo/predicate proof requires a non-vacuous multi-repo universe
- runtime closeout cannot reuse base-query coincidence as a fake oracle

## 건드릴 파일

- `crates/quanta-index-lexical/src/lib.rs`
- `crates/quanta-index-lexical/src/planner.rs`
- `crates/quanta-index-lexical/tests/tantivy_smoke.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- new or revised multi-repo lexical/runtime fixture files under `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 건드리지 말 것

- history date/diff substrate
- runtime catalog substrate
- structural boolean algebra
- bridge translator widening beyond alias/parity bookkeeping

## TODO

- [x] promote native `file.contains(...)` to a direct runtime row without reintroducing tokenizer-dependent keyword coincidence
- [x] add a non-vacuous multi-repo fixture for `repo:` positive allow-list
- [x] add non-vacuous `repo.has.file(...)` oracle rows on multi-repo fixture
- [x] remove any remaining set-like or base-query-coincidence oracle from predicate closeout rows
- [x] add public front-door and chaos companion rails for the shipped predicate families

## NOT TODO

- no single-repo positive row that returns the same result as the base content query
- no request-time git lookup
- no widening to unsupported predicate names or arg shapes
- no hidden lexical fallback dressed up as repo-gate truth

## Test Plan

- `./scripts/cargow test -p quanta-index-lexical --test tantivy_smoke -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test dsl_scenarios -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_perf_chaos -- --nocapture`

## DoD

- `repo:` positive allow-list has a non-vacuous oracle or is explicitly left out of runtime claims
- `repo.has.file(...)` true-gate has a non-vacuous oracle or is explicitly left out of runtime claims
- `file.contains(...)` and `file.has.content(...)` stay proved without reintroducing alias ambiguity in docs
- shipped predicate families are not only runtime-proved, but also exercised on the public front-door and no-poison chaos rails
- predicate docs and proof ledger match the actual executable subset

## Failure Modes

- alias proof is inferred instead of executed
- single-repo fixtures make repo-gate positives vacuous
- predicate widening lands in planner docs but not in runtime closeout
- unsupported arg shapes silently coerce instead of typed-failing
