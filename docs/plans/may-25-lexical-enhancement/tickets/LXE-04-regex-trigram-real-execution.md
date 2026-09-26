# LXE-04 - Regex and Trigram Real Execution

> Archive status: `Historical execution record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) and [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md). Live capability truth: [Lexical Capability Matrix](../lexical-capability-matrix.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: `completed`
Priority: `P0`
Depends on: [LXE-02](LXE-02-planner-authority-ir.md)

## Purpose

Route raw substring and regex leaves through the real trigram and regex
verification engines. Escaping regex into a generic query string is not an
acceptable execution path.

## Current code-backed status (2026-05-26)

- Landed:
  - regex planner/dialect filter/typed `LEX_REGEX_*` rejection path
  - materialized trigram prefilter postings wired into the live lexical adapter
  - generation-local `DocId <-> candidate_id <-> authoritative indexed_text` seam inside the adapter
  - raw substring live execution via trigram prefilter + exact verify over authoritative indexed text
  - whole-document regex live execution via trigram prefilter + exact verify over authoritative indexed text
  - full-fidelity/parity rows `regex_only_match_not_reachable_by_token`, `regex_trigram_false_positive_rejected`, `patterntype_regexp_parity`
- Remaining follow-up:
  - explain-grade prefilter candidate counts and engine trace still belong to the observability wave

## Owner files

- `crates/quanta-index-lexical/src/lib.rs`
- new `crates/quanta-index-lexical/src/regex.rs`
- new `crates/quanta-index-lexical/src/trigram_plan.rs`
- `crates/quanta-index-lq-trigram/src/**`
- `crates/quanta-index-lq-regex/src/**`
- `crates/quanta-index-core/src/domains/lexical/**`
- new `crates/quanta-index-searchd-runtime/tests/e2e_lexical_full_fidelity.rs`
- `crates/quanta-index-searchd-runtime/tests/e2e_dual_syntax_lowering_parity.rs`

## File-level work breakdown

- `crates/quanta-index-lexical/src/{regex.rs,trigram_plan.rs}`: plan raw
  substring and regex execution with candidate caps, trace nodes, and typed
  failures.
- `crates/quanta-index-lexical/src/lib.rs`: remove any escaped query-string
  regex path and execute regex/raw through the planned engine path only.
- `crates/quanta-index-lq-trigram/src/**`: provide mandatory trigram extraction,
  candidate intersection, and bounded prefilter APIs.
- `crates/quanta-index-lq-regex/src/**`: provide exact regex verification and
  unsupported-syntax classification.
- `crates/quanta-index-searchd-runtime/tests/*.rs`: add persisted-runtime rows
  for raw substring, regex positive hits, trigram false positives, and plan
  limit behavior.

## Work items

- Add a planner node for raw substring:
  - mandatory trigram extraction
  - candidate cap
  - exact byte verification
- Add a planner node for regex:
  - parse regex into regex/HIR representation
  - extract mandatory literals/trigrams when possible
  - prefilter through trigram postings when bounded
  - verify exact regex against candidate content
- Define typed rejection for:
  - regex parse errors
  - unsupported regex features
  - unbounded candidate plans over the configured budget
  - trigram shard unavailable
- Record explain trace:
  - extracted trigrams
  - prefilter candidate count
  - verify count
  - early stop reason
- Remove generic query-string regex execution from active runtime.

## Test plan

- unit tests for mandatory literal/trigram extraction.
- property tests for trigram candidate superset behavior.
- exact verification tests that reject trigram false positives.
- budget tests for degenerate regex.
- typed error tests for unsupported syntax.
- no active call site may build a Tantivy query string from `/.../`.

## E2E plan

Covered by `E2E-01`, `E2E-02`, and `E2E-07`:

- raw substring finds content spanning token boundaries.
- regex finds matches that token search cannot find.
- regex does not return trigram false positives.
- Sourcegraph `patterntype:regexp` uses the same execution path.
- pathological regex is bounded and typed rejected or capped.

## DoD

- raw substring and regex rows in the matrix have real engine E2E proof.
- the old escaped-query path is removed or unreachable.
- candidate caps are deterministic and tested.
- regex/raw verify source is authoritative `indexed_text`, not preview snippet text.

## Failure modes

- calling trigram a success without exact regex verification.
- silently falling back to full scan for unbounded regex.
- conflating regex parser success with index execution success.
