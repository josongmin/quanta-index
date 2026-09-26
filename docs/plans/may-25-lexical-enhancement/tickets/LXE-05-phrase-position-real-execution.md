# LXE-05 - Phrase and Position Real Execution

> Archive status: `Historical execution record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) and [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Live capability truth: [Lexical Capability Matrix](../lexical-capability-matrix.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `completed`
Priority: `P1`
Depends on: [LXE-02](LXE-02-planner-authority-ir.md)

## Purpose

Make phrase, ordered-token, and proximity-style DSL behavior depend on a real
position authority rather than token coincidence.

## Current code-backed status (2026-05-26)

- Landed:
  - phrase planner scaffold
  - `quanta-index-lq-positions` authority integration on the live lexical adapter
  - shared query/build tokenization inside the lexical phrase authority seam
  - execution of phrase rows through position postings rather than query-parser fallback
  - runtime rows for exact-adjacent match and reversed-order miss are both green on the positions-backed rail
- Remaining follow-up:
  - richer normalizer unification and explain counters belong to later normalization/observability work

## Owner files

- `crates/quanta-index-lq-positions/src/**`
- `crates/quanta-index-lexical/src/lib.rs`
- new `crates/quanta-index-lexical/src/phrase.rs`
- `crates/quanta-index-core/src/domains/lexical/**`
- `crates/quanta-index-lq-norm/src/**`
- new `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`

## File-level work breakdown

- `crates/quanta-index-lq-positions/src/**`: expose exact phrase and ordered
  token position lookup primitives.
- `crates/quanta-index-lexical/src/phrase.rs`: plan phrase queries onto the
  position engine, including field and case handling.
- `crates/quanta-index-lexical/src/lib.rs`: execute planned phrase nodes and
  stop treating phrase as token conjunction.
- `crates/quanta-index-lq-norm/src/**`: keep phrase tokenization and case rules
  consistent between parsing and execution.
- `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`:
  assert adjacent phrase success and non-adjacent failure against persisted
  data.

## Work items

- Define phrase planner nodes:
  - exact phrase
  - ordered token sequence
  - optional proximity/slop if supported by active DSL
- Bind phrase nodes to position postings.
- Ensure phrase matching respects:
  - case option
  - token normalization policy
  - path/content field selection
  - result caps after deterministic merge
- Define typed rejection for phrase features not represented by the position
  index.
- Add explanation trace with token sequence, field, candidate count, and verify
  count.

## Test plan

- unit tests for token-position normalization.
- unit tests for exact phrase versus unordered token matches.
- negative tests for unsupported phrase/proximity shapes.
- deterministic result ordering tests when phrase and term hits tie.

## E2E plan

Covered by `E2E-01`:

- `"foo bar"` matches adjacent tokens only.
- `"foo bar"` does not match `foo ... bar` unless slop is explicitly supported.
- case-sensitive phrase query changes results.
- phrase under file/path select does not search content.

## DoD

- phrase rows in the matrix are backed by position E2E.
- unsupported phrase extensions fail typed before execution.
- case-sensitive phrase matching is exercised against the positions-backed authority.

## Failure modes

- token AND is mistaken for phrase.
- phrase behavior changes when Tantivy analyzer internals change.
- case option is applied after candidate selection instead of during matching.
