# E2E-01 - Lexical Full-fidelity E2E

Status: `completed`
Priority: `P0`
Depends on: [E2E-00](E2E-00-live-dsl-matrix-harness.md), [LXE-03](LXE-03-lexical-filter-execution.md), [LXE-04](LXE-04-regex-trigram-real-execution.md)

## Purpose

Prove LQ DSL behavior against persisted lexical indexes.

## Current live truth (2026-05-27)

- `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`
  is live and green on the current tree
- the table-driven rail now covers content/path/repo/lang/boolean/case/count,
  phrase, regex, raw substring, and type/select lexical happy-path or typed
  error rows against persisted runtime data
- public `select:path` / `select:content.match` proof is additionally covered
  on the SDK front door
- proof rails:
  - `cargo test -p quanta-index-searchd-runtime --test e2e_lexical_full_fidelity -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test sdk_frontdoor -- --nocapture`

## Owner files

- new `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`
- `crates/quanta-index-searchd-runtime/tests/common/e2e_corpus.rs`
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`

## File-level work breakdown

- `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`:
  add table-driven lexical runtime rows with exact IDs, ordering, kinds, and
  explanation assertions.
- `crates/quanta-index-searchd-runtime/tests/common/e2e_harness.rs`: expose the
  helpers needed for per-row setup and reopen/query assertions.
- `crates/quanta-index-searchd-runtime/tests/common/e2e_corpus.rs`: encode
  corpora designed to catch filter leaks, phrase false positives, and trigram
  prefilter mistakes.
- `docs/plans/may-25-lexical-enhancement/lexical-capability-matrix.md`: map
  each green E2E row back to the syntax/operator matrix.

## Required scenarios

- content term:
  - matches content text
  - does not match path-only text
- path/file:
  - path query matches path
  - file filter narrows content hits by path
- repo:
  - same file path in two repos returns only requested repo
- lang:
  - same content in two languages returns only requested language
- boolean:
  - AND intersection
  - OR union with deterministic ordering
  - NOT exclusion
  - nested boolean typed rejection where unsupported
- case:
  - case-insensitive default
  - case-sensitive option changes result set
- count:
  - stable top N after deterministic merge
- phrase:
  - exact adjacent phrase
  - unordered token non-match
- regex:
  - regex-only match not reachable by token query
  - trigram false positive rejected by exact verify
- raw substring:
  - token-boundary crossing substring
- symbol/select/type:
  - select content
  - select path
  - select symbol
  - type file/content
  - type symbol

## Test plan

- one table-driven test row per matrix row.
- each row asserts expected candidate IDs and candidate kinds.
- each row asserts `engines_touched`.
- each row asserts stable typed error when expected unavailable.

## DoD

- all P0 lexical matrix rows are green or expected-failing with linked owner
  ticket.
- every green row writes records through the harness before querying.
- result ordering is asserted, not only set equality.
- `SearchExplanation` is asserted for successful and typed-failure rows.

## Failure modes

- testing parser/lowering only.
- using a corpus where false positives cannot be observed.
- asserting non-empty results instead of exact IDs and order.
