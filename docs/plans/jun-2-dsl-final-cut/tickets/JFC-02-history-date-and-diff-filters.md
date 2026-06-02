# JFC-02 History Date and Diff Filters

Parent packet: [../README.md](../README.md)

Status: `closed`

## Objective

Hold the landed history date/window and diff-field substrate to current-source truth, including the newly executable qualified `since.time:` / `since.commit:` shapes.

## Current Source Truth

- current AST/parser/normalizer ship `before:`, `after:`, `since:`, `until:`, `diff.added:`, `diff.removed:`, `diff.touched:`
- current history execution requires explicit `type:commit` or `type:diff`
- `type:commit` executes `rev:`, `author:`, `committer:`, `message:`, and keyword/phrase/raw `content:` against commit metadata
- `type:diff` owns `file:` plus `diff.added:` / `diff.removed:` / `diff.touched:`; `type:commit file:...` and `type:commit diff.*:...` now typed-fail `INVALID_REQUEST`
- unsupported predicate/regex/structural leaves are rejected at validation time on the history/runtime executable-text planes
- current runtime rows prove positive, miss, and typed-invalid behavior for the landed history surface
- qualified `since.time:` / `since.commit:` now execute on the same history authority as bare `since:`, with unknown refs/tags typed-failing `HISTORY_INVALID_TIMEREF`
- front-door breadth is now explicit:
  `sdk_frontdoor` owns `since.time:` / `since.commit:` builder and transport proof plus the
  history invalid-request / predicate-leaf typed rejects,
  `end_to_end` owns raw IPC proof for `after:` / `until:` / `diff.*` and the same fail-closed admission contract,
  and `e2e_perf_chaos` closes no-poison / bounded-metric behavior across the widened history siblings

## 핵심 로직

- time authority is `committer_time_ms`
- history route is single-kind by contract: `type:commit` and `type:diff` are distinct execution surfaces, not a mixed implicit union
- bare `since:` / `until:` semantics must be explicit and singular; qualified `since.*` forms carry their own proof and must not survive as undocumented parser aliases
- date/window filters must narrow against commit metadata, not message/content fallbacks
- diff-field filters must narrow against added/removed/touched hunk text, not generic diff payload coincidence
- unsupported leaf shapes must fail closed during validation, not late inside the executor
- fixtures must have at least two commits and differentiated diff hunks so filter narrowing is observable

## 건드릴 파일

- `crates/quanta-index-lq-norm/src/ast.rs`
- `crates/quanta-index-lq-norm/src/parser/implementation.rs`
- `crates/quanta-index-lq-norm/src/normalizer/implementation.rs`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_full_corpus.rs`
- history fixture files under `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/`
- `crates/quanta-index-searchd-runtime/tests/fixtures/lexical_corpus/runtime_rows.toml`
- `docs/plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## 건드리지 말 것

- runtime catalog substrate beyond current history execution
- Sourcegraph / bridge widening before native semantics land
- structural boolean algebra
- semantic track contracts

## TODO

- [x] land qualified `since.time:` / `since.commit:` on the same `committer_time_ms` authority with direct runtime rows and chaos coverage
- [x] keep the landed multi-commit and differentiated diff-hunk fixtures non-vacuous
- [x] preserve typed invalid timeref behavior and exact diff-field narrowing semantics
- [x] extend chaos rails for history widened surfaces (`e2e_perf_chaos`)
- [x] add public front-door rails for qualified `since.*` and raw IPC rails for `after:` / `until:` / `diff.*`

## NOT TODO

- no parser-only acceptance with executor TODOs hidden behind generic errors
- no single-commit fixture where every filter returns the base query result
- no hidden parser alias that revives `since.time:` / `since.commit:` without owner-local and runtime proof
- no bridge widening in this ticket
- no fallback to generic `content:` or message filtering when diff/date substrate is missing

## Test Plan

- parser / normalizer owner-local rails for new filters
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test end_to_end -- --nocapture`
- `./scripts/cargow test -p quanta-index-searchd-runtime --test e2e_perf_chaos -- --nocapture`

## DoD

- all history date/window and diff-field filters, including qualified `since.time:` / `since.commit:`, stay live in AST/parser/executor
- `since:` semantics are frozen to one current-source meaning, and qualified `since.*` branches are explicitly proved rather than floating ambiguously in active docs
- runtime proof rows for those filters stay non-vacuous
- public front-door and chaos rails cover the shipped history families instead of leaving them runtime-corpus-only
- invalid values typed-fail with stable diagnostics
- missing `type:` and `type:commit + file:/diff.*` combinations typed-fail instead of returning empty-success
- docs no longer describe those surfaces as parser-only or stale fail-closed

## Failure Modes

- `since:` ships while qualified `since.time:` / `since.commit:` drift back into parser-only or undocumented alias state
- `since:` / `until:` are parsed as loose aliases with ambiguous time authority
- diff-field filters accidentally read generic content leafs
- multi-commit ordering is unstable or unverified
- bridge docs widen before native semantics are real
