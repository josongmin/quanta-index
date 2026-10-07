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
  later golden replay, telemetry and persistence retain their separate boundaries.
  Complete output/clock/status facts must agree throughout the measured schedule,
  not just its first response.

Owners: [batch membership](../../tools/benchmark/retrieval/execution_batch.py)
and [capture/replay](../../tools/benchmark/retrieval/run.py). Retain duplicate or
changed membership, stale source/cache, missing child and phase-byte mutants.

## Owners and regressions

- [External capture/verify](../../tools/benchmark/retrieval/live_lexical_external.py),
  [Sourcegraph scope](../../tools/benchmark/retrieval/sourcegraph_index_scope.py),
  [OpenGrok consumer](../../tools/benchmark/retrieval/opengrok_index_scope.py),
  [Java reader](../../tools/benchmark/retrieval/native/FullLiveDocuments.java),
  [query witness](../../tools/benchmark/retrieval/opengrok_query_witness.py),
  [pinned fixture builder](../../tools/benchmark/retrieval/opengrok_query_fixture.py).
- [Semble phases](../../tools/benchmark/retrieval/semble.py),
  [required-cell controller](../../tools/benchmark/retrieval/execution_batch.py).
- Keep impossible Lucene metadata, source/role/UID drift, before/after mutation,
  unsupported true-attestation flags and actual HTTP capture/offline replay
  controls in [native scope tests](../../tools/ci/tests/test_opengrok_index_scope.py),
  [capture tests](../../tools/ci/tests/test_live_lexical_external.py),
  [query witness tests](../../tools/ci/tests/test_opengrok_query_witness.py) and
  [fixture reproduction tests](../../tools/ci/tests/test_opengrok_query_fixture.py), with
  [clock tests](../../tools/ci/tests/test_completed_response_timing.py).

## Native matrix and incremental acceptance

Consolidated CS-BENCH-02/04 and S30-B04 acceptance preserves each selected
product's actual SDK/daemon/server/indexer/binary/config/model and indexed source
view. Sourcegraph, OpenGrok, cs and Semble use their native supported entrypoints;
ripgrep is an exact scan/cost reference. Direct Zoekt is a separate engine profile.
A generic BM25 runner or GitHub CLI cannot substitute for another native backend.

- Inventory every selected entrypoint/format and retain original native response,
  request/source/index bindings and terminal outcome. Independently mutate path,
  span, order, score and metadata; normalized-only equality is insufficient.
  Genuine complete zero results remain valid, while missing/capped/partial results
  keep their actual outcome. Existing decoders remain the single parser authority.
- Ranked file requests require ten distinct files. Ten chunks subsequently deduped
  do not issue file top-10; collect-to-files work discloses extra enumeration.
  Path-ordered constant-score prefixes are not relevance-ranked pools. Matched
  semantics and native workflows retain separate comparisons and budgets.
- A deterministic update sequence covers full readiness, add/edit, rename/move/
  delete, malformed-source lexical survival, syntax repair and declared interruption/
  restart cuts. Independent source hashes and expected state verify new bytes,
  retired paths/declarations, unchanged source and coherent old/new readers.
- Actual watcher events permit workspace-event latency; explicit ingest keeps its
  narrower start. ACK, activation and query visibility are distinct milestones.
  Await bounded observed state, not sleep-based presumed completion. Report lag,
  stale hits, reprocessed files/bytes and recovery under the product's actual policy.

## Consequences

A replayable diagnostic can contain complete empty queries and incomplete
relevance. Disk/API proof is not loaded-reader or whole-universe qualification;
focused fake-process tests are not Java execution or a fresh service capture.
Historical exact bodies are in the [plan history index](../ARCHIVE-INDEX.md#historical-record-recovery).

## Completed clock scope

O4-E2-01's completed-response clock and output binding implementation/selected
actual scope are complete. Its ticket is retired. New selected runs verify the
same construction-to-normalized-output boundary and output identity; repeated
admitted performance belongs to O4-E4-06. Worker/transport/instrumented clocks
and invalid/failed observations cannot become pristine latency samples.

Native service cleanup preserves the first body/capture exception and original
cause while logging cleanup traceback separately, including Python 3.10. An
exception label does not claim which comparator ran. This avoids losing the
actual failure without turning failed cleanup into a successful capture.

## Completed Ready9 capture scope

### Semble admitted input identity

The adapter binds its record digest to the canonical query pack validated before
environment inspection and indexing. Each query must match its submitted-query
SHA-256; the pack's file universe and digest must exactly match the admitted
manifest. The worker's requests and record provenance use this same admitted
pack. After execution, the strict JSON reader rechecks the input and refuses
query/commitment drift or duplicate keys before publishing a record. Reformatting
the same JSON preserves its canonical identity.

The previous producer hashed a second, permissively parsed pack after execution.
A real parent/worker fixture reproduced successful publication after query or
suite-commitment replacement, duplicate-key insertion, and a pack/manifest
universe mismatch. Those cases now refuse; matching input still completes.
The selected Semble, bundle and completed-response regression scope passed
64 tests with 557 deselected using
`uv run --frozen --extra dev python -m pytest tools/ci/tests/test_retrieval_benchmark.py tools/ci/tests/test_completed_response_timing.py -q -k 'semble or load_query_pack or runner_bundle or completed'`.
The worker uses a fixture Semble index: this is input/capture contract verification,
not another actual product capture or repeated-cost/warmup qualification.

### Source-bound completed captures

Rechecked 2026-10-07 against the original result files. External source
`09103820` completed CS/SG/OG capture and independent replay for all nine roots:
180 requests per product, 540 total. Selected-project OpenGrok reader evidence
retains that scope; all-project/global loaded-reader flags remain false.
Original replay roots are
`/Users/songmin/.codex/task-evidence/ready9-native-recovery-20261006-01a10d0b-v3`
and `/Users/songmin/.codex/task-evidence/ready9-sgog-recovery-20261006-01a10d0b-v5`.
Replay retains the ten-role producer closure and Python3.13.9 executable;
SG/OG Java reader argv/cwd bind the original `p11-operation-authority` path.
The archived-source path was refused; replay at the original byte-matching
producer/reader path passed. Native raw bytes were not changed.

Quanta/Semble source `6a3f6afc` attempted every Ready9 repository and all 180
original NL file tasks. Bat, lo, Mocha, Uvicorn and Zustand completed pair capture
and independent `run.py verdict` replay, 40/40 product-task records per repository.
Their five-product file score projections also completed. Common eligible counts
are Bat20, lo5, Mocha0, Uvicorn1 and Zustand2; Mocha's common score is
`NOT_APPLICABLE`. These diagnostic populations cannot be pooled into qualified
whole-suite relevance or speed.

Main `49a02c15` repaired the common adapter's assumption that an NL file report
had top-level `per_query`. Canonical file judgments now preserve no-answer,
unjudged/null, excluded denominators and duplicate-observation comparisons.
The selected adapter regression run passed 92 tests; actual five-repository
payload conversion covered ten payloads and 200 rows. Bat's full canonical
five-product replay and score-only projections agree; projection does not
reissue completed-boundary latency or relabel external091 bytes as native6a3.

The separately recorded source49 workflow/matrix/five-product/fresh-join/default
decision fixture run passed 110 tests (6.70s):
`PYTHONDONTWRITEBYTECODE=1 uv run --frozen --extra dev python -m pytest tools/ci/tests/test_code_search_workflow.py tools/ci/tests/test_code_search_matrix.py tools/ci/tests/test_lexical_five_product_oracle.py tools/ci/tests/test_identifier_robustness_fresh_join.py tools/ci/tests/test_retrieval_default_decision.py -q -p no:cacheprovider`.
Missing/duplicate cells, source/query/profile drift and raw/report mismatch
refusals are fixture coverage, not additional native samples or qualification.

CLI, Django, Nushell and TypeORM capture failed at the fixed 32MiB term-directory
policy. No native record or promoted root was issued for those failures. Final
`completed-summary.json`, `capacity-failures.json` and
`unjudged-native-union.json` retain five completed pairs, four actual failures
and 72 tasks / 191 unique task-file pairs requiring independent judgments.
Original native input/commands/raw/results remain at
`/Users/songmin/.codex/task-evidence/ready9-native-pair-20261007-01a10d0b`.
The capacity repair is implemented in `b262925b`; its paged term-directory
contract and focused verification are retained in
[OCT-05-004](OCT-05-004-cost-capacity-and-qualification-boundaries.md#paged-term-directory).
Follow-up CLI, Django, Nushell and TypeORM capture and independent replay each
passed 40/40 at that fixed source, with both exit codes zero. Original corpora,
20-query packs and budgets were retained. Their five-product distinct-file projections reuse the original
CS/SG/OG bytes, preserving external091 versus nativeb262 identities. Their
result status is `diagnostic_unqualified`; no qualified latency or independent
gold is issued. Native and join result files are under
`/Users/songmin/.codex/task-evidence/ready9-native-paged-20261007-01a10d0b/{native-execution,reused-joins}/{cli,django,nushell,typeorm}/result.json`.

The original five completed repositories plus these four follow-ups complete
the nine-repository diagnostic inventory. `ready9-checkpoint-inventory.json`
retains the two native source checkpoints, original failed attempts and original
external response clocks; `uniform_native_source` is false. This is not
latest-main/release, native rank-equivalence or speed qualification. Final
source-closure verification passed for all 1,529 selected files, and both pinned
binaries plus all four original suite/query-pack hashes still matched.

Fresh common eligible counts are CLI0, Django0, Nushell0 and TypeORM1. The first
three common scores are `NOT_APPLICABLE`; neither these cohorts nor the prior
five-repository cohorts establish qualified whole-suite relevance.

`ready9-unjudged-checkpoint-union.json` binds 151 tasks / 742 unique task-file
pairs: the original 72-task/191-pair packet plus the new 79-task/551-pair packet.
Its status is `BLOCKED` because independent relevance judgments are absent.
The blind-review summary contains two blank source-bound forms for each of
CLI/Django/Nushell/TypeORM, with zero assigned reviewer identities and zero label
changes. Original 191-pair supplemental review inputs are retained separately;
blank forms are not completed review or human provenance. Independent
judgments, admission and remaining required inventory stay with
[E2-04](../plans/oct-4-parallel-closure/tickets/INDEX.md#o4-e2-04) and E1.

## Selected Semble repetition and warmup diagnostic

On 2026-10-07, four fresh Semble-only lexical-file captures used the original
complete Bat (20 tasks, 79 files) and Zustand (20 tasks, 50 files) inputs,
pinned Semble 0.6.0, Python 3.13.9, package lock and model assets. Seed 7,
top-k 10 and three measured schedules per run were fixed. Bat's two runs with
one warmup had 81 events each; Zustand's zero/one-warmup runs had 61/81 events
with the same cold probe and measurement schedules. All 20 tasks per run
succeeded. Independent native/normalized row,
status, ordering, candidate float64-bit and per-task output hash comparisons
agreed. Parent phase intervals, worker accounting and record/manifest hashes
were independently checked. Parent/worker totals were 3,005.551/2,009.560 ms
and 2,432.459/1,619.801 ms for Bat, and 1,295.933/573.024 ms and
1,140.424/532.682 ms for Zustand's zero/one-warmup runs. These timings are
shared-Mac diagnostics.

Original commands, protocols and outputs remain outside the checkout at
`/private/tmp/qi-e2-semble-20261007-xim9rl_i/{bat-a2,bat-b,zustand-warm0,zustand-warm1}`.
The first `bat-a` controller used Python 3.9 and failed during parent validation;
its partial worker output is preserved separately and not admitted. The retry
used the pinned Python 3.13.9 interpreter.

The wrapper's `source_head` is its preflight `5e15` checkpoint, not an observed
per-run HEAD. Concurrent docs-only main advancement was not sampled per run.
The five checked affected source files and all fixed corpus/query/lock/model
inputs stayed unchanged; exact per-run global HEAD binding is `NOT_RUN`.
Do not relabel these artifacts as clean `5e15` or current-main formal receipts.
Bat is an A/A repetition control, not an alternative-implementation A/B or reuse
adoption result. Zustand closes only the selected non-Bat warmup parity cell;
default warmup remains one and other repositories need their own selected checks.
