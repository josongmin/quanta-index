# E2E-02 - Sourcegraph Parity E2E

Status: `executed`
Priority: `P0`
Depends on: [E2E-00](E2E-00-live-dsl-matrix-harness.md), [LXE-03](LXE-03-lexical-filter-execution.md), [LXE-04](LXE-04-regex-trigram-real-execution.md), [LXE-06](LXE-06-symbol-select-type-execution.md)

## Purpose

Prove Sourcegraph-compatible syntax enters through `TextQueryRequest`,
translates into canonical active query, and returns the same runtime result as
equivalent LQ where supported.

## Current code-backed status (2026-05-27)

- Owner proof lives in
  `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`.
- Current live-source proof is green on both:
  - `cargo test -p quanta-index-searchd-runtime --test e2e_dual_syntax_lowering_parity`
  - `cargo test -p quanta-index-searchd-runtime`

## Owner files

- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`
- `crates/quanta-index-lq-bridge/src/translator.rs`
- `crates/quanta-index-lq-bridge/src/syntax.rs`
- `crates/quanta-index-search-plane/src/lowering.rs`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`:
  own SG query to LQ query parity rows with exact ordered result checks.
- `crates/quanta-index-lq-bridge/src/{translator.rs,syntax.rs}`: document and
  exercise which SG features are translated versus typed rejected.
- `crates/quanta-index-search-plane/src/lowering.rs`: ensure SG and LQ both
  enter the same active lexical request lowering path.
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`: record
  parity ownership and rejection ownership per SG feature.

## Required scenarios

- `repo:` parity with equivalent LQ repo filter.
- `file:` parity with equivalent LQ file/path filter.
- `lang:` parity.
- `case:` parity.
- `count:` parity.
- `type:file` and `type:symbol` parity.
- `select:file`, `select:content`, `select:symbol` parity.
- `patterntype:literal` raw substring parity.
- `patterntype:regexp` regex parity.
- boolean `or` parity where supported.
- negation parity where supported.
- unsupported Sourcegraph feature returns typed bridge/translator error.

## Test plan

- table with SG query, equivalent LQ query, expected IDs, expected engines.
- parity assertion compares ordered result IDs and candidate kinds.
- unsupported rows assert stable typed code and no execution side effects.
- translator unit tests remain separate and cannot satisfy E2E rows alone.

## DoD

- Sourcegraph syntax does not use a separate runtime request type.
- every supported Sourcegraph row proves parity against persisted data.
- unsupported SG syntax fails before planner execution.
- `SearchExplanation` includes syntax translation and planner evidence.

## Failure modes

- proving translation AST equality without executing the query.
- accepting unsupported Sourcegraph syntax and returning empty success.
- treating CodeQL bridge packet export as Sourcegraph query support.
