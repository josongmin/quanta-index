# LXE-09 - Structural Live Integration

Status: `proposed`
Priority: `P1`
Depends on: [LXE-01](LXE-01-active-contract-and-dead-route-cleanup.md), [LXE-02](LXE-02-planner-authority-ir.md)

## Purpose

Expose the structural query/result surface and enforce Option B: no parse-tree
producer means typed unavailable, not live structural success.

## Owner files

- `crates/quanta-index-contract/src/results/**`
- `crates/quanta-index-lq-structural/src/**`
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`
- `crates/quanta-index-core/src/domains/lexical/**`
- `crates/quanta-index-core/src/domains/structural/**`
- new `crates/quanta-index-searchd-runtime/tests/e2e_history_structural.rs`

## File-level work breakdown

- `crates/quanta-index-contract/src/results/**`: keep structural result carriers
  explicit and separate from content candidates.
- `crates/quanta-index-core/src/domains/structural/**`: define structural
  planner and execution boundary with Option B fail-closed semantics.
- `crates/quanta-index-lq-structural/src/**`: expose real structural matching
  only when parse-tree inputs exist; otherwise expose typed unavailable.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs`: route structural
  requests and preserve `STR_PRODUCER_PARSE_TREE_UNAVAILABLE`.
- `crates/quanta-index-searchd-runtime/tests/e2e_history_structural.rs`: assert
  no structural success from content-only fixtures.

## Work items

- Add or verify active response carriers:
  - `StructuralBinding`
  - `StructuralCandidate`
- Add structural IPC request/response variant.
- Route structural DSL leaves through a structural planner node.
- Return typed fail-closed code when parse-tree producer data is absent:
  `STR_PRODUCER_PARSE_TREE_UNAVAILABLE`.
- Do not synthesize structural matches from regex/text.
- If parse-tree ingest exists in this repo, add positive fixture path behind the
  structural shard readiness check. Otherwise leave positive path blocked.

## Test plan

- contract round-trip tests for structural carriers.
- unit tests for structural planner route and unavailable status.
- negative test proving no text/regex fallback is used.
- readiness test for missing parse-tree generation.

## E2E plan

Covered by `E2E-04`:

- structural request without parse-tree data returns
  `STR_PRODUCER_PARSE_TREE_UNAVAILABLE`.
- explanation identifies structural planner route and fail-closed reason.
- no successful structural candidate is returned from content-only data.

## DoD

- structural endpoint exists and is typed.
- no structural success is claimed without parse-tree producer input.
- fail-closed result is covered by E2E, not only a unit test.

## Failure modes

- implementing structural as regex over source text.
- returning empty success on missing parse-tree data.
- documenting structural as shipped when only the request type exists.
