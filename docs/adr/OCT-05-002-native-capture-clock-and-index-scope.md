# OCT-05-002 — Native Capture, Clock and Index Scope

Status: `Accepted`

Decided: 2026-10-05

Consolidates implemented O4-E2 contracts under
[SEP-27-004](SEP-27-004-benchmark-capture-and-resource-custody.md).
It preserves diagnostic qualification boundaries. Fresh cells and remaining
index authority live in the [OCT-04 residual ledger](../plans/oct-22-finalizing/E08-integration-release.md#integration-gates).

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

External `09103820` captured and independently replayed CS/SG/OG
across all nine Ready9 roots, 540 requests. The selected-project OpenGrok
reader scope and original byte-matching Java producer path were required.
The archive-path replay was refused. Its all-project/global reader flags
remain false.

Native/Semble `6a3f6afc` completed pair capture/replay for Bat, lo, Mocha,
Uvicorn and Zustand (40/40 records each). CLI, Django, Nushell and TypeORM
initially failed the fixed 32 MiB term directory. Each later passed 40/40
on `b262925b` with the original inputs and budget. The two Native
checkpoints and external response clocks remain distinct. The adapter's
score-only replay preserves the no-answer/unjudged/excluded denominators and
does not issue completed-boundary latency. [Paged directory](OCT-05-004-cost-capacity-and-qualification-boundaries.md#paged-term-directory)

The five earlier common-eligible counts were 20/5/0/1/2 and the four follow-up
counts were 0/0/0/1. The combined 151 tasks and 742 unique unjudged pairs remain
`BLOCKED` on independent judgments. Blank review forms supply no labels.
The current product gates remain with [E08](../plans/oct-22-finalizing/E08-integration-release.md#integration-gates).
[Run details and original receipts](../ARCHIVE-INDEX.md#earlier-benchmark-diagnostics)

## Selected Semble repetition and warmup diagnostic

The Semble 0.6.0/Python 3.13.9 repetition control ran Bat twice and
Zustand at warmup zero/one with the original 20 tasks per run. All 20
succeeded in each run with identical independent response/order/score-bit
checks. The first Bat wrapper had refused under Python 3.9; the admitted
retry used the pinned interpreter. Checked source/input files were unchanged
and per-run HEAD was `NOT_RUN`. This was Bat A/A diagnostic evidence and
the selected Zustand warmup cell. Default warmup remains one; adoption and
other repositories retain their own acceptance. [Run details and original receipts](../ARCHIVE-INDEX.md#earlier-benchmark-diagnostics)

## Selected fresh release Bat pair diagnostic

Frozen `b9c058e1` Quanta and pinned Semble 0.6.0 used the original
79-file Bat corpus and 20-task pack. Quanta recorded 20 capped windows and
Semble 20 successes. The canonical pair verdict independently replayed
byte-for-byte with 20 common-eligible tasks. MRR@10 was 0.9375/0.8417 and the
candidate-minus-baseline nDCG@10 interval was approximately [-0.0527, 0.0416].
The preflight that rejected a 104-byte UDS path remains separate from the
successful retry.

The verdict remains `diagnostic_unqualified` with `PAIR_VALID=pass` and
unissued Contract/SDK qualification in this pair. The original labels and this
local pair do not close independent human review, operational portability or
admitted performance. [Run details and original receipts](../ARCHIVE-INDEX.md#earlier-benchmark-diagnostics)

## Selected Gin declaration and robustness execution

Frozen `f60609fe` captured Gin `d3ffc998` (99 files, 1,196 query
identities) with the separately bound `b9c058e1` SDK release. The unsupported
file oracle refused before capture. Canonical v3 recomputed its independent
source labels. Symbol execution recorded 1,192 success and four capped;
the four declaration controls belong to that population. The default file
pair recorded Quanta 1,109 success/87 capped and Semble 1,196 success. Its
nDCG@10 delta interval included zero, and the verdict replay matched.

The seven default robustness members cover 5,776 scoring memberships and
4,514 unique native queries per product. The union and all seven member
views independently replayed. Quanta abstained on 99/99 content-absence
queries, whereas Semble returned nonempty windows. Declaration absence can
still match content, and literal content absence can still permit an OSA1
match. Their denominators stay separate.

The explicit typo batch used its own positive 1,189 tasks and independently
proved 99 OSA1-negative cases. Quanta recorded 1,116 successes and 73 capped
positives, with Hit@10 of 1.0, plus 99/99 negative abstentions. Semble recorded
1,068 successes and 121 abstained positives, Hit@10 of 0.8469, and 99/99
nonempty negative windows. Both native records and both members replayed.
These are source-derived diagnostics. Independent holdout, speed and release
claims retain separate acceptance. [Original inputs and result details](../ARCHIVE-INDEX.md#benchmark-dispatch-and-diagnostic-preimages)

## Selected retained Gin20 hybrid pair

The retained original manifest recovered the missing path with
the same bytes. All 99 Gin `d3ffc998` files and the original 20-task
suite/query pack matched. The frozen `f60609fe` pair reused the
[closed scanner binaries](OCT-05-004-cost-capacity-and-qualification-boundaries.md#closed-scanner-build-and-capture-diagnostic)
with its declared model/profile, seed42, top-k10, one warmup and one
measured repetition. Quanta recorded 20 capped windows, Semble 20 successes,
and diagnostic file recall@10 was 0.85/0.975.

The first preflight refused a 108-byte socket path. After shortening the
output path the actual capture and independent verdict both exited 0 and
replayed byte-for-byte. `PAIR_VALID=pass` applies to this diagnostic only.
Original AI-reviewed labels and absent Contract/SDK receipts retain their
original status. [Original inputs and result details](../ARCHIVE-INDEX.md#benchmark-dispatch-and-diagnostic-preimages)

## Retained B09 scope reconciliation

Frozen `d1a1b709` holds 33 successful native cells, 11,272 rows
across overlapping profiles. On the dated recheck, returned-window/ingest
diagnostics passed 33/33. All 33 phase replays failed for missing symbol
producer evidence. This replay failure retains the original native results
and is distinct from the separate global12 4,363-task/21,815-response join.

Fresh `f60609fe` recapture passed 8/8 available-source cells with
independent phase/diagnostic/source and external-snippet replay. It covered
CLARC original/neutral-renamed and six CodeSearchNet languages, 1,350 tasks
and 3,798 files under the original explicit 64-token NL policy. Recapture
for the 24 OSA cells was `BLOCKED` on original checkouts. Retained top-k
responses supply no replacement source universe. [Original inputs and result details](../ARCHIVE-INDEX.md#benchmark-dispatch-and-diagnostic-preimages)

## Selected C3 AI review and issuance

The source-bound CLI 2.1.292 executed the original Opus 5.5/Sonnet 5.5/Fable 5.1
roles with unchanged frozen queries, rubrics and full source files. The bounded
80,000-source-character/32-pair partition preserves every candidate pair;
only independently validated original receipts can be reused. Failed original
CLI-path, context-window and version-admission attempts remain under
`/private/tmp/qi-c3-oct8-review*`; they do not issue labels.

SQLAlchemy's actual 615.537s attempt and one exact-input 347.302s retry remain
`FAILED`: an unresolved pair and an invalid source line prevent completion.
Tailscale's actual 744.631s attempt and one 14.829s retry remain `FAILED` because
the same pair remains unresolved. Neither issued labels or admission; uncertainty
was not converted to a grade or human gold.

Zellij's initial 1533.248s unresolved failure is preserved. Its subsequent
2986.116s attempt completed both blinded reviewers, then stopped at 286 newly
validated adjudicator pairs with a real 429 session-limit response. The raw error
reported a 07:00 Asia/Seoul reset, zero usage and no model result. After that
observed boundary, the same-input resume received normal responses and completed:
actual review 832.982s, independent cached replay 15.333s, canonical labels 5.770s
and canonical NL suite 2.294s, all exit 0. Existing 59 adjudicator pairs were reused
only after their source and complete independent-pass commitments matched;
95 shared batch receipts supply the other 417 pairs. All three roles cover the
same 20 tasks/476 pairs. The original failed/429 outputs were not rewritten.

Commands and stage/raw digests are under
`/private/tmp/qi-c3-oct8-review-v4/quota-resume-zellij/`. Canonical issued artifacts
are under `qi-b08-closeout-20261004-2i72kj91/ai-review-current/zellij/`; NL
`validation.json` SHA-256 is
`16cdf71d0dfb04f7ea42186dc104d132f51df97534c2c6544dc853e4293acc8b`.
The issuer uses pinned 36970d9f source, not a later-source runtime verdict.
The 20 unchanged query families, repository commit and full universe also match
the retained verified NL source-split manifest. That focused binding does not
reissue the old 22-repository split result or its historical issued-suite count.

Human provenance and qualification remain false. The new labels/NL directory
and split preparation contain no full admission packet. License/model bindings,
matching-source contract/SDK receipts and fresh product capture remain required;
old native records cannot be rebound to the newly issued suite. SQLAlchemy and
Tailscale keep their incomplete judgment scopes separately.

An input-dependency recheck at Quanta `6686c0d3` separately validated the existing
Zellij local-use AI license against the new NL suite and the original 422-file
corpus manifest (`cf5303ed123ba60f2087a5d71765956222389ba11aeb67d4e155a28992724a30`).
Repository commit, full universe and all 20 source-split families match. This is
retained authorization for local source-bound ingestion/search/internal metrics;
it supplies no human, redistribution or final qualification claim.

The old model-binding receipt points to an absent `/private/tmp/qi-c5-pair-cache-0bca6def-20261004-v3/cache/5/hf`.
Current `semble.resolve_model_revision` correctly refused that path. The retained
Gin20 capture still contains the exact `minishlab/potion-code-16M-v2` revision
`e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b` and asset digest
`ea909b7defe7804ce18bf003ef60a437b54782541819ab6b7004c36fd9eea5d0`.
Canonical `materialize_model_cache` restored all 6 members/33,521,569 bytes to the
fresh disjoint `/private/tmp/qi-c3-model-recovery-_9m96xlc/hf`; current resolution
reproduces the original revision/digest. Its manifest, original capture/model
receipt and adapter source hashes are under the recovery root. Original receipts
were preserved. Final-source Contract/SDK receipts, host/cache/lockfile bindings
and new-suite admission/capture remain incomplete; recovering model bytes does
not issue those claims.

## ARB current policy and official file scoring

The frozen88-case official population has three separately prepared arms under
the current32-token NL policy: original text13 accepted/75 refused, retained
adapter-v1 27/61 and canonical adapter-v2 88/0. The historical original17 arm
included four queries now over the token limit and is not the current13 arm.
All 128 accepted case/spec/source/gold bindings passed preflight and their actual
frozen f606 captures completed with exit 0. The fresh external copy-spec preparation is
`/private/tmp/qarb-14cw8d1e/preparation.json`; query/budget/gold bytes are unchanged.

The [read-only scorer](../../tools/benchmark/retrieval/arb_official_score.py)
binds official raw samples and `baseline.target_gold_files`, original/adapted
query identity, base commit, full file universe, suite/pack/spec and current
record run/profile/binary identity. Its pinned official Recall@20/MRR@20 uses
the first20 distinct paths in the top100 ranked chunks. Successful/capped
rankings and observed empty abstentions can score; missing, malformed, failed
or refused records retain distinct states and no invented zero-filled metrics.
The initial25 focused tests and real128-case empty-record binding preflight
passed; that preflight supplies no native results. The complete scorer/corpus
extension passed44 focused tests. Selective restoration verifies pinned official
archive and corpus-manifest digests, safe member types/paths, snapshot identity,
counts and shared-base byte equality. Official BCY parses the exact bytes hashed
from one pinned descriptor through a private spool; extracted bytes are rechecked
after scoring. The transitive official source closure requires its clean pinned
Git checkout and matching import origins, including refusal of ignored Python
source. No checkout text substitutes for official corpus text.

Three retained official archives were restored and rehashed for83 release-chunk
files representing57 unique snapshots. The original capture controller passed a
raw manifest where the evaluator requires `SourceSnapshot`; all 128 native
processes exited0 but its additional replay status remained `FAILED`. The raw
controller and results were not rewritten. Independent canonical typed-context
and route-projection replay verified 128/128 records, diagnostics, phases,
preflight/resource bindings, inputs, source and binary identities under
`/private/tmp/qarb-14cw8d1e/independent-replay.json`.

The first actual scorer run at e227 then exposed a separate product defect:
it compared a combined four-route pack directly against each three-route Quanta
capture, retaining all accepted outcomes as `BLOCKED` with pack-hash mismatch.
Those three score/replay pairs remain under `/private/tmp/qarb-14cw8d1e/`.
Main 6c5cda3c uses the existing canonical route projection, requires exactly
lexical/semantic/hybrid provenance, and validates the pinned record bytes
against the projected suite/pack without changing gold, source, query or budgets.
The regression independently accepts a three-route source fixture before
combining its suite and rejects full-pack digest and route-set substitution.
The scorer/corpus focused selection passed 45/45; Ruff check/format passed.

All three corrected official score/replay pairs completed at clean 6c5cda3c
under `/private/tmp/qarb-score-6c5c-8jjktvw6/`. Each pair is equal after excluding
only its fresh corpus work-root path. All 384 route outcomes completed, with
independently restored official corpus bytes and complete BCY budgets 4k/8k/16k/32k.
The scorer leaves native custody as `NOT_ATTESTED_BY_SCORER`; the separate
128-record replay above supplies that scope. Unsupported preflight cohorts are
reported separately, with no zero fill.

| Arm | Route | Completed denominator | Recall@20 | MRR@20 | BCY@8000 |
| --- | --- | ---: | ---: | ---: | ---: |
| original | hybrid | 13 | 0.230769 | 0.020488 | 0.000000 |
| original | lexical | 13 | 0.346154 | 0.053936 | 0.038462 |
| original | semantic | 13 | 0.000000 | 0.000000 | 0.000000 |
| retained_v1 | hybrid | 27 | 0.253086 | 0.040050 | 0.049383 |
| retained_v1 | lexical | 27 | 0.308642 | 0.055556 | 0.067901 |
| retained_v1 | semantic | 27 | 0.209877 | 0.037235 | 0.086420 |
| current_v2 | hybrid | 88 | 0.369318 | 0.070568 | 0.142045 |
| current_v2 | lexical | 88 | 0.484848 | 0.102854 | 0.204545 |
| current_v2 | semantic | 88 | 0.308712 | 0.057861 | 0.125000 |

These are distinct 13/27/88-case accepted cohorts with 75/61/0 unsupported cases
out of the requested 88. They cannot establish a paired adapter improvement,
independent holdout rank, later binary relevance or qualified performance.
File relevance, BCY, native custody and those qualification scopes remain separate.
