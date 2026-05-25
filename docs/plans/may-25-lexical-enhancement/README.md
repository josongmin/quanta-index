# May 25 Lexical Enhancement Closeout

Status: `proposed`
Date: `2026-05-25`
Scope: breaking-first closeout for lexical search, LQ DSL, Sourcegraph syntax,
real-engine execution, and hard E2E proof.

---

## 1. Objective

Close the remaining gap between the public lexical/search DSL claims and the
active repo implementation.

This is not a docs-only cleanup. The program is complete only when:

- every accepted LQ DSL and Sourcegraph expression is either executed by the
  owning engine path or rejected with a typed error before execution
- no active filter is silently dropped
- regex and raw substring paths use the trigram/regex engines rather than a
  query-string escape path
- semantic and hybrid search consume materialized lexical scope rather than a
  best-effort lexical side channel
- history and structural surfaces are wired fail-closed until producer data
  exists
- E2E tests persist real index data, reopen it, query it, and assert results

## 2. Current truth to freeze first

The implementation must be re-frozen by `LXE-00` before code work starts.
Known risk areas from the latest local review:

- `crates/quanta-index-lexical/src/lib.rs` has filters that are ignored or
  returned as `NotImplemented`.
- the regex leaf path still has a risk of going through an escaped query
  string instead of the trigram plus regex verification path.
- `crates/quanta-index-search-plane/src/lowering.rs` accepts a wider syntax
  surface than the executor can currently prove.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs` correctly
  fail-closes history and structural paths, but the response/runtime proof must
  be made executable.
- existing hard-case tests are useful but are not sufficient if they do not
  write records into the real storage/index path and query the reopened index.

Facts above are starting points, not completion claims. `LXE-00` must produce a
line-backed matrix before any ticket can be marked implemented.

## 3. Execution waves

| Wave | Tickets | Goal |
| --- | --- | --- |
| 0 | `LXE-00` | freeze current truth and executable capability matrix |
| 1 | `LXE-01`, `LXE-02` | finish active contract cleanup and planner authority split |
| 2 | `LXE-03`..`LXE-06` | close lexical execution gaps across filters, regex, phrase, symbols |
| 3 | `LXE-07`..`LXE-10` | wire semantic/hybrid/history/structural/bridge around the new lexical authority |
| 4 | `E2E-00`..`E2E-07` | prove real storage/query/restart/perf behavior |

Within a wave, tickets may run in parallel only when their owner files do not
overlap. E2E harness work can start after `LXE-00`, but individual scenario rows
must stay expected-failing until the owning implementation ticket lands.

## 4. Non-negotiable rules

- Breaking-first. Remove legacy public query fields instead of keeping shims.
- One lexical text request with explicit syntax enum. No syntax guessing.
- No silent fallback. Unsupported syntax is a typed rejection at intake or
  lowering.
- Planner owns engine selection. `search-plane` may dispatch but must not own
  lexical engine semantics.
- Structural live success is forbidden until parse-tree producer data exists.
- External producer gaps are explicit blockers, not green implementation claims.
- E2E proof must exercise persisted/indexed data, not parser-only fixtures.

## 5. Program exit criteria

The program is complete only when all are true:

1. `TextQuerySyntax { Native, Sourcegraph }` drives all lexical text intake.
2. semantic and hybrid requests reference the same `TextQueryRequest` sub-struct.
3. active `LqQuery` cannot encode removed legacy shapes such as raw passthrough,
   match-all bypasses, or custom leaves.
4. all accepted filters have an execution path or a typed fail-closed boundary.
5. `repo`, `file`, `lang`, `case`, `count`, `select`, and `type` are covered by
   live E2E scenarios.
6. regex and raw substring tests prove trigram prefilter plus exact verify.
7. phrase tests prove positional behavior instead of token coincidence.
8. Sourcegraph tests prove translated queries match equivalent LQ behavior.
9. semantic tests prove lexical scope materialization affects the candidate set.
10. hybrid tests prove lexical universe first, then semantic fusion.
11. history and structural endpoints return typed not-ready/unavailable when
    producer data is absent.
12. `SearchExplanation` carries planner trace, engines touched, early stop
    reason, and summary for real executed queries.
13. restart/replay E2E returns deterministic result IDs and ordering.
14. CI has at least one full real-engine corpus rail, separate from unit tests.

## 6. Ticket pack

- [tickets/INDEX.md](tickets/INDEX.md)
- [tickets/LXE-00-truth-freeze-and-executable-matrix.md](tickets/LXE-00-truth-freeze-and-executable-matrix.md)
- [tickets/LXE-01-active-contract-and-dead-route-cleanup.md](tickets/LXE-01-active-contract-and-dead-route-cleanup.md)
- [tickets/LXE-02-planner-authority-ir.md](tickets/LXE-02-planner-authority-ir.md)
- [tickets/LXE-03-lexical-filter-execution.md](tickets/LXE-03-lexical-filter-execution.md)
- [tickets/LXE-04-regex-trigram-real-execution.md](tickets/LXE-04-regex-trigram-real-execution.md)
- [tickets/LXE-05-phrase-position-real-execution.md](tickets/LXE-05-phrase-position-real-execution.md)
- [tickets/LXE-06-symbol-select-type-execution.md](tickets/LXE-06-symbol-select-type-execution.md)
- [tickets/LXE-07-semantic-hybrid-planner-provenance.md](tickets/LXE-07-semantic-hybrid-planner-provenance.md)
- [tickets/LXE-08-history-live-integration.md](tickets/LXE-08-history-live-integration.md)
- [tickets/LXE-09-structural-live-integration.md](tickets/LXE-09-structural-live-integration.md)
- [tickets/LXE-10-observability-and-bridge-sink.md](tickets/LXE-10-observability-and-bridge-sink.md)
- [tickets/E2E-00-live-dsl-matrix-harness.md](tickets/E2E-00-live-dsl-matrix-harness.md)
- [tickets/E2E-01-lexical-full-fidelity-e2e.md](tickets/E2E-01-lexical-full-fidelity-e2e.md)
- [tickets/E2E-02-sourcegraph-parity-e2e.md](tickets/E2E-02-sourcegraph-parity-e2e.md)
- [tickets/E2E-03-semantic-hybrid-e2e.md](tickets/E2E-03-semantic-hybrid-e2e.md)
- [tickets/E2E-04-history-structural-e2e.md](tickets/E2E-04-history-structural-e2e.md)
- [tickets/E2E-05-restart-replay-determinism-e2e.md](tickets/E2E-05-restart-replay-determinism-e2e.md)
- [tickets/E2E-06-full-corpus-real-engine-ci.md](tickets/E2E-06-full-corpus-real-engine-ci.md)
- [tickets/E2E-07-performance-and-chaos.md](tickets/E2E-07-performance-and-chaos.md)

## 7. Ticket contract

Every ticket in this pack must be specific enough to implement without a second
planning pass.

Required sections:

- `Purpose`: what correctness gap the ticket closes
- `Owner files`: the bounded file set the ticket is allowed to change
- `File-level work breakdown`: exact responsibility for each major file or file
  group
- `Work items`: behavior and interface changes
- `Test plan`: unit, contract, or integration rails
- `E2E plan`: real storage/query proof or explicit linkage to the owning E2E
  ticket
- `DoD`: completion gate, not progress wording
- `Failure modes`: how the ticket can appear done while still being wrong

If a ticket cannot name file ownership or exit evidence precisely, it is not
ready for implementation.
