# May 25 Lexical Enhancement Closeout

Status: `partial-execution-live`
Date: `2026-05-26`
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
- history surfaces stay typed fail-closed until producer data exists
- structural surfaces execute only against materialized parse-tree/chunk
  authority already present in the readiness ledger
- E2E tests persist real index data, reopen it, query it, and assert results

## 1.5 Verification refresh (2026-05-27)

- Current live-source closeout rerun stayed green on:
  - `cargo check -p quanta-index-contract`
  - `cargo check -p quanta-index-sdk`
  - `cargo test -p quanta-index-searchd-runtime --test repo_map_end_to_end`
  - `cargo test -p quanta-index-sdk --lib`
  - `cargo test -p quanta-index-searchd-runtime`
- Additional owner-local proof rails on the same current tree are also green:
  - `cargo test -p quanta-index-searchd-runtime --test explain -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test dsl_scenarios -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_perf_chaos -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_restart_replay_determinism -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_full_corpus -- --nocapture`
  - `cargo test -p quanta-index-searchd-runtime --test e2e_matrix_inventory -- --nocapture`
  - `just rust-test-full-corpus`
- That rerun covers the active lexical/Sourcegraph/structural daemon rails now
  living under `searchd-runtime`, including `e2e_lexical_full_fidelity`,
  `e2e_dual_syntax_lowering_parity`, `e2e_perf_chaos`, `sdk_frontdoor`, and
  `repo_map_end_to_end`.
- The narrower `may-26-indexing-residue-tasks` structural/bridge pack is
  separately closed on the same current tree. The remaining status here is the
  broader whole-program queue, not a May-26 residue-pack reopen.
- Program status remains `partial-execution-live`: `E2E-07` and the residual
  `LXE-10` metrics surface are not closed by this rerun.
- This is a current live-source proof refresh, not a frozen-tree release claim.

## 1.6 Public-surface closure refresh (2026-05-27)

Landed in the same current-tree window:

- `searchctl` now sends production lexical/semantic/hybrid/explain/repo-map
  queries through `quanta-index-sdk`; raw query IPC assembly is no longer the
  non-test consumer path
- `sdk_frontdoor` owns the public SDK happy-path proof, including lexical,
  semantic, hybrid, explain, repo-map, history, runtime, and structural rows
- `repo_map_end_to_end` is narrowed to raw IPC transport/persistence
  invariants instead of public happy-path authority
- history authority now fails closed with exact codes
  `HISTORY_GENERATION_NOT_READY`, `HISTORY_PRODUCER_UNAVAILABLE`,
  `HISTORY_SHARD_UNAVAILABLE`

This refresh closes public-surface residue around the SDK front door and
history taxonomy. It does not close the broader engine/program residue tracked
below.

## 2. Current truth to freeze first

`LXE-00` produced the current executable matrix; any remaining ticket work must
still be re-frozen against live source before new completion claims are made.

Engine residue on the latest local review:

- `crates/quanta-index-lexical/src/lib.rs` has filters that are ignored or
  returned as `NotImplemented`.
- regex no longer escapes through a generic query string; the remaining regex
  risk is observability and large-corpus proof, not route correctness.
- `crates/quanta-index-search-plane/src/lowering.rs` accepts a wider syntax
  surface than the executor can currently prove.
- `crates/quanta-index-search-plane/src/query_dispatcher.rs` now lowers one
  top-level structural leaf plus the executable `repo:` / `file:` / `lang:`
  filter subset into the live structural domain path instead of hard-closing
  the happy path.
- `crates/quanta-index-lq-structural/src/matcher.rs` and
  `crates/quanta-index-searchd/src/app/runtime.rs` expose a truthful structural
  subset over materialized parse-tree/chunk authority: root-kind exact,
  root capture, root-kind plus capture, ordered direct-child tree-walk,
  variadic sibling capture / wildcard skip, and `where` / `inside` /
  `outside` constraints.
- structural execution inside `quanta-index-core` / `searchd` now uses
  internal `StructuralMatchBinding` / `StructuralMatchCandidate` carriers;
  public `StructuralBinding` / `StructuralCandidate` projection happens only at
  the search-plane response boundary.
- structural shapes outside that subset, and structural queries with filters
  outside `repo:` / `file:` / `lang:`, remain typed `STR_INVALID_REQUEST`.
- existing hard-case tests are useful but are not sufficient if they do not
  write records into the real storage/index path and query the reopened index.

Public-surface residue that remains intentionally open:

- semantic/hybrid still expose the current vector/handle request contract;
  text-only semantic ownership is deferred to the separate `SEM-OWN` follow-on
- internal `searchd-runtime` composition still uses legacy channel
  publisher/subscriber wiring and mirror paths even though the public SDK front
  door is closed

Facts above are current guardrails, not blanket completion claims. Remaining
tickets still need line-backed source and runtime proof before they can be
marked implemented.

## 3. Execution waves

| Wave | Tickets | Goal |
| --- | --- | --- |
| 0 | `LXE-00` | freeze current truth and executable capability matrix |
| 1 | `LXE-01`, `LXE-02` | finish active contract cleanup and planner authority split |
| 2 | `LXE-03`..`LXE-06` | close lexical execution gaps across filters, regex, phrase, symbols |
| 3 | `LXE-07`..`LXE-10` | wire semantic/hybrid/history/structural/bridge around the new lexical authority |
| 4 | `E2E-00`..`E2E-07` | prove real storage/query/restart/perf behavior |

Within a wave, tickets may run in parallel only when their owner files do not
overlap. E2E harness work can start after `LXE-00`; when a new surface is
intentionally behind implementation, its scenario row must stay
expected-failing until the owning ticket lands. The current live lexical
matrices no longer rely on expected-failing rows.

## 4. Non-negotiable rules

- Breaking-first. Remove legacy public query fields instead of keeping shims.
- One lexical text request with explicit syntax enum. No syntax guessing.
- No silent fallback. Unsupported syntax is a typed rejection at intake or
  lowering.
- Planner owns engine selection. `search-plane` may dispatch but must not own
  lexical engine semantics.
- Structural live success is allowed only against materialized parse-tree/chunk
  authority already present in the readiness ledger.
- No structural text/regex fallback. Unsupported structural shapes stay typed
  `STR_INVALID_REQUEST`.
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
11. history endpoints return typed not-ready/unavailable when producer data is
    absent, and structural endpoints either execute against materialized
    parse-tree/chunk authority or return typed fail-closed codes
    (`STR_LANG_NOT_SUPPORTED`, `STR_GENERATION_NOT_READY`,
    `STR_SHARD_UNAVAILABLE`, `STR_INVALID_REQUEST`).
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
