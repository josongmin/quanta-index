# S30-B04 — native-bound five-product capture

Status: `ACTIVE_RESIDUAL`; historical executions are diagnostic. Priority: P1. Exact-name lane depends
on S30-B01; the 20-query and robustness lanes also depend on S30-B02/B03 input
freezes. Parent: [Sep 30 plan](../README.md). Contract owners:
[CS-BENCH-02](../../sep-27-code-search-remediation/rfcs/CS-BENCH-02-native-response-validation.md)
and [CS-BENCH-04](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md).

## Matrix and work

Capture Quanta, Semble, Sourcegraph, cs and OpenGrok where each has an actual
supported mode. Do the **full 1,196 exact-name set**, not only the historical
300 or a selected success subset. A lane whose input or product is unavailable
remains visibly `BLOCKED` or unsupported; other admitted lanes may proceed.

For each lane, freeze one of two modes before capture:

- Matched semantics: same case, normalization, literal/identifier meaning,
  path scope, candidate universe, result unit, cutoff and completion rule.
- Native workflow: product's documented user-facing mode, with its own actual
  query transformation and output semantics. Semantic/hybrid vs lexical-only
  belongs here unless an exact common contract is proved.

For a distinct-file exact-name comparison, use Quanta's `keyword_file` policy
(scored, `select:file case:yes name`) for a ranked comparison. `literal_file`
(quoted content phrase) and `substring_file` (raw substring) are constant-score
restrictions returned in path order, so their top 10 is an observed path-ordered
prefix, not a relevance ranking (correction 2026-10-01). Semble or any other chunk-native
product needs an explicit bounded collect-to-ten-unique-files policy and source
rank preservation; if unsupported, retain its ten-chunk observed-prefix result
as a separate diagnostic. Never call a deduplicated ten-chunk prefix file top-10.
Keep declaration/symbol requests separate from bare content search.

Capture raw response bytes, endpoint/process exit, native errors, partial/cap
signals, ordered result identities, source/index generation, request and
effective-query digests, model/index config, timers and output bytes. Replay
with the same pinned native decoder; normalized rows do not substitute for raw.
Prove each product's actual indexed universe and source freshness for the
selected claim. OpenGrok disk/file-view observations keep the narrower scope in
[OCT-05-002](../../../adr/OCT-05-002-native-capture-clock-and-index-scope.md);
they do not attest the service's loaded reader or whole posting universe.

## Verification and deliverable

- Every expected task has one complete native-bound row or a typed
  unsupported/error/timeout/incomplete status. HTTP 200 with native error,
  truncated stream, stale source, duplicate ID, path escape and changed
  normalized hit/order all refuse in focused decoder fixtures.
- Product-specific index inventory and before/after source observations are
  attached or the comparison remains diagnostic with an exact blocker.
- Each capture uses a fresh external root and never overwrites the 300/1,196
  historical inputs, `/private/tmp/g3`, or another product's raw evidence.
- Same input/host captures are repeatable and replayable; a successful capture
  alone is neither relevance nor performance qualification.

Use the existing [live external adapter](../../../../tools/benchmark/retrieval/live_lexical_external.py),
[lexical scorer](../../../../tools/benchmark/retrieval/lexical_file_comparison.py),
[runbook](../../../../tools/benchmark/CODE_SEARCH_RUNBOOK.md) and
[registry](../../../../tools/benchmark/registry.toml). Repair only a reproduced
missing adapter boundary, with focused native fixtures under its current owner.


## Execution ownership

Current cross-ticket execution is owned once by the
[OCT-04 residual ledger](../../oct-4-parallel-closure/tickets/INDEX.md).
Retain the acceptance above for any new claim; reuse compatible captures.
Past counts, binaries, failures and commands are recoverable from [historical bodies](../../ARCHIVE-INDEX.md#oct-05-benchmark-and-quality-ledger-compaction).
They do not qualify current source.
