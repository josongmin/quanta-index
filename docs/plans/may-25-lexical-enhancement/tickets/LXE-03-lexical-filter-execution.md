# LXE-03 - Lexical Filter Execution

Status: `completed`
Priority: `P0`
Depends on: [LXE-02](LXE-02-planner-authority-ir.md)

## Purpose

Make every accepted lexical filter either executable or typed rejected before
query execution. No filter may be silently ignored.

## Current code-backed status (2026-05-27)

- Executed or typed-rejected on the live lexical rail:
  - `repo`, `file`, `lang`, `rev`
  - `case:yes/no` for keyword/path-term execution
  - `count:` with deterministic tie ordering after full recall
  - `select:repo`, `select:file`, `select:path`, `select:content`,
    `select:content.match`
  - type/select routing into symbol/history surfaces where owned by adjacent
    tickets, with the lexical rail itself staying typed fail-closed outside
    its authority
  - producer-dependent typed-unavailable or typed-not-ready truth for `fork`,
    `archived`, `visibility`, and `context`
- Code-backed proof:
  - `cargo test -p quanta-index-lexical --test tantivy_smoke`
  - `cargo test -p quanta-index-core --test lexical_policy`
  - `cargo check -p quanta-index-search-plane --tests`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_lexical_full_fidelity -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture`

## Owner files

- `crates/quanta-index-lexical/src/lib.rs`
- new `crates/quanta-index-lexical/src/filters.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `crates/quanta-index-lq-norm/src/**`
- `crates/quanta-index-contract/src/query/**`
- `crates/quanta-index-searchd-runtime/tests/dsl_scenarios.rs`
- new `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`

## File-level work breakdown

- `crates/quanta-index-lexical/src/filters.rs`: define `FilterPlan`, typed
  unavailable codes, and field ownership for each accepted filter.
- `crates/quanta-index-lexical/src/lib.rs`: remove `Ok(None)` success branches
  and execute filters through planned constraints only.
- `crates/quanta-index-search-plane/src/lowering.rs`: lower LQ and Sourcegraph
  filters into the same canonical filter representation.
- `crates/quanta-index-lq-norm/src/**`: normalize filter operands and reject
  ambiguous or unsupported shapes before execution.
- `crates/quanta-index-searchd-runtime/tests/*.rs`: add exact-result rows for
  repo, file, lang, case, count, select, and typed unavailable filters.

## Work items

- Implement or reject with typed errors:
  - `repo`
  - `file`
  - `lang`
  - `rev`
  - `type`
  - `select`
  - `case`
  - `count`
  - `fork`
  - `archived`
  - `visibility`
  - `context`
- Replace no-op filter handling with an explicit `FilterPlan`.
- Define field ownership:
  - repo/path/lang filters are pre-candidate field constraints
  - case affects text/regex/phrase matching
  - count is a result cap with deterministic tie handling
  - type/select route to content/path/symbol/history/structural surfaces
  - fork/archive/visibility/context are typed unavailable unless indexed
- Make boolean placement rules explicit:
  - filter under top-level AND is executable when supported
  - OR/NOT-scoped filters are either planned correctly or typed rejected
- Add engine-level metadata needed to execute filters without inspecting raw
  query text.

## Test plan

- unit tests for each filter lowering into `FilterPlan`.
- negative tests proving no filter returns `Ok(None)` as a success path.
- parser/lowering tests for OR/NOT scoped filters.
- deterministic count/tie-ordering tests.
- source-level test that unsupported filters return typed unavailable codes.

## E2E plan

Covered by `E2E-01` and `E2E-02`:

- repo filter excludes same-path files in another repo.
- file filter matches path and does not match content-only hits.
- lang filter uses indexed language metadata.
- case filter changes result set for mixed-case content.
- count cap returns stable top N.
- unsupported producer-dependent filters return typed unavailable.

## DoD

- no accepted filter can be ignored.
- every filter row in the capability matrix is `executed` or `typed-rejected`.
- Sourcegraph and LQ filter semantics share the same planner/executor path.
- E2E proves filters against persisted records, not synthetic in-memory lists.

## Failure modes

- using string matching over file paths in `search-plane`.
- treating unavailable producer metadata as match-all.
- applying count before deterministic merge.
