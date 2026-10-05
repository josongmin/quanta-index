# OCT-05-002 — Native Capture, Clock and Index Scope

Status: `Accepted`

Decided: 2026-10-05

Consolidates implemented O4-E2 contracts under
[SEP-27-004](SEP-27-004-benchmark-capture-and-resource-custody.md).
It preserves diagnostic qualification boundaries. Fresh cells and remaining
index authority live in the [OCT-04 residual ledger](../plans/oct-4-parallel-closure/tickets/INDEX.md#e2).

## Context

API inventories, served bytes, read-only Lucene disk observations and a service's
loaded reader establish different facts. Impossible field metadata and an
unsupported attestation flag can make a self-consistent capture overstate those
facts. Transport/worker clocks also differ from completed query response time.

## Decision

1. Canonical capture and offline verification decode retained native bytes through
   the same product owner. Bind source/query/unit/profile, runtime/config/binary,
   raw inventory and normalized rows. Recomputed derived digests cannot authorize
   rows that disagree with native responses. Empty completion, partial/capped,
   unsupported, timeout and missing result retain their actual meanings.
2. Sourcegraph native scope uses an owned execution and exact document/source
   inventory. Nonblocking pipe `EAGAIN` means wait and retry; it is not EOF or
   successful partial output. Retain bounded drains, actual terminal and cleanup.
3. OpenGrok's Java reader reports every live document and segment with deterministic
   JSON key order. The Python consumer strictly validates FieldInfo names/enums/
   booleans, stored-value shapes and declared indexed fields. Posting frequency
   availability must agree with IndexOptions; terms/counts and stored-value
   term digests must agree. Duplicate/unknown/impossible metadata refuses.
4. Keep source path/project/stored UID/indexed `u`, directory `d`/`dirpath`/LOC,
   and serialized settings `objuid` roles separate. Validate their deployed ABI
   and independent source/ancestor inventory. Root directory/project conventions
   cannot be inferred from file paths, and auxiliary objects are not source files.
5. Native disk before/after observations retain the scope
   `readonly_disk_live_documents_and_uid_postings`. Read-only mounts, Tomcat-only
   startup, WAR/config/source identity, indexed-project GET and denied PUT probes
   strengthen that observation. They do not attest the loaded service reader or
   all source bytes/postings. Keep `indexed_universe_attested`,
   `opengrok_indexed_universe_attested` and
   `opengrok_service_loaded_reader_attested` false for this mode, with the backend
   universe exclusion. A declared snapshot timestamp proves ordering only; it
   is not an independent seal authority. A stronger claim needs its own actual
   query-bound reader/source witness and consumer proof.
   The optional query-reader fixture pins the original/patched Java source,
   patch, compiler image and four compiled controller class digests. It reads
   the searcher's acquired subreader before release, then binds its commit
   generation/version/counts/file-name digest to native before/after observations
   and the request's capture-root/container/project/task/query nonce. Duplicate,
   malformed, stale or type-aliased witnesses refuse. Only selected instrumented
   requests acquire `opengrok_query_reader_scope.attested`; global flags stay
   false. Preserve pristine response-body equivalence and keep instrumented
   timing separate. Read-only service probes connect directly without ambient
   proxy routing.
6. Completed response time includes request construction, transport, complete
   decoding and required normalization/validation, excluding later persistence.
   Bind clock domain/boundary/duration and completed output size/hash. Retain
   transport and worker measurements under their original names. Semble parent
   phases and worker/process residual are separate domains; overlapping child
   intervals cannot be summed into a new total.
7. Required cells have explicit terminal/reuse/unsupported/failed/blocked/not-run
   outcomes. A failed repository cannot hold ready siblings indefinitely. Final
   publication requires the declared inventory and replay, not a live watcher.
8. Quality warmup zero requires actual task/status/score-bit/row parity against
   warmup one, each run's own protocol/schedule and retained phase ledger.
   Equal seeds do not imply equal measured order. The observed bat decision is
   scoped to its bound inputs; other cohorts retain one warmup until proved.
   Zero warmup cannot acquire qualified speed.

## Repository batching and retained phase bytes

- Compatible original suites and blind packs are independently validated before
  constructing a per-repository product-query union. Shared queries execute once;
  an explicit membership map projects native rows into each original scoring view.
  Keep native execution identity separate from derived per-intent views. Matrix
  verification re-derives membership, every child record and each report; publish
  only the complete declared batch. Do not concatenate incompatible intent suites.
- Retained phase paths bind exactly to captured SHA-256 bytes through
  `phase_metrics_digests` before semantic validation. Missing/extra/duplicate paths,
  malformed digests and changed bytes refuse. Parent/worker clock domains and
  nested stage inclusion remain explicit; rewriting phase JSON cannot reuse the
  old capture's authority.
- Qualified response timing uses the declared
  `request_construction_to_normalized_response` boundary and each capture's own
  monotonic domain. Required decode/normalization completes before timer stop;
  later golden replay, telemetry and persistence retain their separate boundaries. Complete output/clock/status facts must
  agree throughout the measured schedule, not just its first response.

Owners: [batch membership](../../tools/benchmark/retrieval/execution_batch.py)
and [capture/replay](../../tools/benchmark/retrieval/run.py). Retain duplicate or
changed membership, stale source/cache, missing child and phase-byte mutants.

## Owners and regressions

- [External capture/verify](../../tools/benchmark/retrieval/live_lexical_external.py),
  [Sourcegraph scope](../../tools/benchmark/retrieval/sourcegraph_index_scope.py),
  [OpenGrok consumer](../../tools/benchmark/retrieval/opengrok_index_scope.py),
  [Java reader](../../tools/benchmark/retrieval/native/FullLiveDocuments.java),
  [query witness](../../tools/benchmark/retrieval/opengrok_query_witness.py).
- [Semble phases](../../tools/benchmark/retrieval/semble.py),
  [required-cell controller](../../tools/benchmark/retrieval/execution_batch.py).
- Keep impossible Lucene metadata, source/role/UID drift, before/after mutation,
  unsupported true-attestation flags and actual HTTP capture/offline replay
  controls in [native scope tests](../../tools/ci/tests/test_opengrok_index_scope.py),
  [capture tests](../../tools/ci/tests/test_live_lexical_external.py),
  [query witness tests](../../tools/ci/tests/test_opengrok_query_witness.py) and
  [clock tests](../../tools/ci/tests/test_completed_response_timing.py).

## Consequences

A replayable diagnostic can contain complete empty queries and incomplete
relevance. Disk/API proof is not loaded-reader or whole-universe qualification;
focused fake-process tests are not Java execution or a fresh service capture.
Historical exact bodies are in the [plan history index](../plans/ARCHIVE-INDEX.md#oct-05-handoff-and-ticket-compaction).
