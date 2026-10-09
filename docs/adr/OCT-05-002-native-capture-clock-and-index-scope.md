# OCT-05-002 — Native Capture, Clock and Index Scope

Status: `Accepted`

Decided: 2026-10-05

Consolidates implemented O4-E2 contracts under
[SEP-27-004](SEP-27-004-benchmark-capture-and-resource-custody.md).
It preserves diagnostic qualification boundaries. Fresh cells and remaining
index authority live in the [OCT-04 residual ledger](../plans/oct-10-index-closeout/VALIDATION.md#quality-inputs).

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
[E2-04](../plans/oct-10-index-closeout/VALIDATION.md#quality-inputs) and E1.

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

## Selected fresh release Bat pair diagnostic

On 2026-10-08, a fresh exploratory Quanta/Semble lexical-file pair used the
unchanged Bat commit `4608fc959aa8abf80d32198836511a570b7ae9ea`, 79-file
manifest and original 20-task suite/query pack/AI-reviewed labels. Quanta used
the three-binary-bound fresh SDK release build at clean source
`b9c058e15d7349ed76adaa408abbd43f123aa65a`; runner SHA-256 is
`f19e81f584e0b81f8aae36f595bd8a4353fd9c1ec43bae3bacf0c27241f3a6bd`, daemon
SHA-256 is `c9ee82af6815481415b123102b4270186f3a1c035f0f00a4eb560c91e2b307aa`.
Semble retained pinned 0.6.0/Python 3.13.9, lock and cached model assets.

The original natural-language-file UCD17 profile and Semble lexical-file profile
were unchanged: seed7, top-k10, one fresh root, one warmup and one measured
schedule per system. Quanta returned ten distinct files for each task with
`capped` outcome20; Semble recorded `success`20. The verdict's accepted40/failed0
counts include capped windows and do not assert Quanta exhaustion. All 20 tasks
were common eligible. Diagnostic MRR@10 was 0.9375/0.8416666666666666;
nDCG@10 was 0.606856383265772/0.6143293951848603, candidate-minus-baseline
delta -0.007473011919088557 with bootstrap 95% interval
[-0.05268058916227, 0.04162469927667926]. These retained-label observations
do not establish a qualified quality improvement.

`run.py pair --spec` completed and `run.py verdict` independently replayed the
manifest/native records/scoring; original and replayed verdict bytes matched.
Corpus, suite, query pack, lock, source closure and binary hashes stayed fixed
before/after. Raw capture and replay remain at `/private/tmp/q9qa8kwhvg/`.
The first preflight refused a 104-byte UDS path before capture; its original
spec/failure remains at `/private/tmp/qib9-xyv4xf0n/`. The retry changed only
the external output/profile paths to fit the 103-byte limit.

The report remains `diagnostic_unqualified`, `PAIR_VALID=pass`; all qualification
claims are false. The pair manifest contains no Contract/SDK qualification
receipts, so its `CONTRACT_GREEN`/`SDK_PATH_GREEN` remain `not_run` despite the
separate fresh SDK proof. T01–T09 inputs, operational portability, independent
human review, quiet-host performance and other required lanes are not closed.

## Selected Gin declaration and robustness execution

On 2026-10-08, the clean `f60609fe2d2882f1193d4ba68deb8704e8972138`
driver used the frozen `b9c058e1` SDK release runner/daemon identified above.
The intervening changes were documentation only. Gin remained at
`d3ffc9985281dcf4d3bef604cce4e662b1a327a6`, with its original 99 source files
and 1,196 query identities. The original `go_exact_local_name_v1` file suite
was refused before capture because that oracle is unsupported. Its failed
`g.staging` output and original suite bytes are preserved. A separate canonical
`source_oracle_suite` preparation recomputed v3 file/symbol labels from the
same source and queries; this does not relabel the old attempt as successful.

- The four declaration/name-span controls passed with MRR@10 and recall@10
  both 1.0. The full v3 symbol population recorded 1,192 success and four
  capped outcomes. Both declaration and name-recovery MRR@10 were 1.0;
  recall@10 was 0.9985493335876968. The four controls are included in the
  1,196 population and are not additional unique tasks.
- The default `code_search_file` pair executed 1,196 tasks per product:
  Quanta 1,109 success/87 capped, Semble 1,196 success. File Hit@10 was
  0.9924749163879598/0.9949832775919732; nDCG@10 was
  0.9376536074540455/0.9355208052300636. The nDCG delta's 95% interval
  [-0.005404081719473451, 0.009541906847290399] does not establish a
  qualified improvement. Independent verdict replay reproduced identical bytes.
- Canonical `identifier_robustness_suite` preparation selected all 1,196
  original families. Prefix/infix/components/typo admitted
  1,121/1,147/982/1,189 tasks, retaining 75/49/214/7 ineligible records.
  Declaration absence admitted 100; content absence retained 99 and excluded
  one; typo content absence retained 1,138 and excluded 51 from typo.
  These new populations do not replace the historical four typo denominators.

`run.py quality-batch --spec` executed the seven default-file cohorts through
one native index per product. Their 5,776 scoring memberships correspond to
4,514 unique native queries per product. Quanta recorded 3,699 success,
716 capped and 99 abstained; Semble recorded 3,891 success and 623 abstained.
`quality-batch-verify` independently replayed all seven members and both
original union records. Derived cohort views are not separate native captures.
All 28 original/derived suite, pack and member-spec byte bindings still matched.

| Source-derived cohort | Tasks | Quanta file Hit@10 | Semble file Hit@10 |
| --- | ---: | ---: | ---: |
| Prefix | 1,121 | 0.9928635147190009 | 0.8046387154326494 |
| Infix | 1,147 | 0.994768962510898 | 0.5274629468177855 |
| Components | 982 | 0.9928716904276986 | 0.9959266802443992 |
| Typo under the default profile | 1,189 | 0.9941126997476871 | 0.8469301934398654 |

Content-absence queries abstained 99/99 in Quanta; Semble returned nonempty
windows 99/99. Declaration absence alone allowed one actual content match in
Quanta, so its 100-task scope is distinct. Typo content absence produced nonempty
windows 1,138/1,138 in Quanta and 1,035/1,138 in Semble: absence of a literal
content match does not rule out a permitted OSA1 match. This cohort is not
an OSA1-absence oracle or an explicit-typo acceptance result.

A separate canonical `ascii_identifier_osa1_absent_casefold_v1` preparation
proved 99 negative queries against the same admitted source. A second native
batch selected `code_search_typo_file` for Quanta and retained Semble's declared
lexical-file profile. The 1,189 positive typo tasks recorded Quanta 1,116
success/73 capped and Semble 1,068 success/121 abstained. File Hit@10 was
1.0/0.8469301934398654, nDCG@10 was 1.0/0.6856607398733592, and recall@10
was 0.9982878769674397/0.8433137089991589. The independently proved OSA1
absence cohort recorded Quanta 99 abstentions and Semble 99 nonempty results.
No negative query was removed based on a product response. Original union
records and both original member suites were independently replayed with
`quality-batch-verify`: two members and two native records `VERIFIED`.
The explicit typo profile and default profile are distinct configurations;
these source-derived results do not qualify independent holdout quality.

Commands used: `source_oracle_suite`, `identifier_robustness_suite`,
`run.py quanta`, `evaluator.py evaluate-diagnostic`, `run.py pair`,
`run.py verdict`, `run.py quality-batch` and `run.py quality-batch-verify`.
Capture, new inputs, original refusal and replay remain outside the checkout
at `/private/tmp/qna7vj2zq6/`. Results are `VERIFIED` for these executed
diagnostic scopes, with all quality/speed/incremental qualification claims false.

## Selected retained Gin20 hybrid pair

On 2026-10-08, the original Gin20 spec's manifest path was absent. Its retained
`raw/input-manifest` matched the original evidence's SHA-256
`46dd79074244ce3d4082a46cf081e40ff12302fe52bed8b038f1be0b99c108b4`.
All 99 current corpus files matched that manifest at Gin revision
`d3ffc9985281dcf4d3bef604cce4e662b1a327a6`. The original 20-task suite and
query pack also matched their recorded digests. An external copy recovered
those exact bytes; original evidence and missing-path spec were preserved.

A new pair used the frozen f606 ASCII fresh release binaries identified in
the [closed scanner diagnostic](OCT-05-004-cost-capacity-and-qualification-boundaries.md#closed-scanner-build-and-capture-diagnostic).
It retained Quanta's native hybrid and Semble's native-default hybrid profiles,
potion-code model inputs, seed42, top-k10, warmup1, one measured repetition and
`require-complete` symbol coverage. Quanta recorded 20 capped windows; Semble
recorded 20 success windows. Observed file recall@10 was 0.85/0.975; this is
distinct from span/block recall and source-independent quality acceptance.

`run.py pair --spec` and a fresh `run.py verdict --repo --suite --run-manifest
--out` both exited0. Independent verdict bytes matched SHA-256
`8270d01f6251b9f951440f28bc1bbeb3929c05bd3ee20288fec3597da70e8619`.
Original inputs, new specs/native records/report and replay remain at
`/private/tmp/qgi0_rrtq2r/`. The first preflight refused a 108-byte UDS path
before capture; `spec-overlong.json` preserves it. The retry shortened only
the external output path. Status is `VERIFIED` for this diagnostic pair/replay,
`PAIR_VALID=pass`, with all qualification claims false. Original AI-reviewed
labels are retained; they are not new independent human judgments. Contract/
SDK qualification receipts are absent from this pair and remain `not_run`.
They do not issue independent holdout judgments, external five-product recapture,
other-repository acceptance or current release qualification.

## Retained B09 scope reconciliation

The retained `d1a1b7097c5c915afd8fb24467c6d737acdb3b14` B09 ledger has 33
actual successful native cells, totaling 11,272 selected rows across overlapping
profiles. Original record, diagnostic, pack and manifest byte commitments were
rechecked on 2026-10-08. Current `validate_retrieval_diagnostic` accepted all
33 original returned-window/ingest diagnostics. Current phase validation refused
all 33 with `invalid symbol producer evidence`; it requires producer evidence
not supplied by those original phase artifacts. That replay scope is `FAILED`,
not a new native execution or a rewrite of the original completion.

Original captures and frozen-decoder completion remain at
`/Users/songmin/Documents/code-new/qi-b09-structural-fix-20261004-ji1PLR/`;
the current decoder results remain at
`/private/tmp/qna7vj2zq6/b09-retained-current-decoder-replay.json`.
The separate retained 4,363-task/21,815-response global12 join is not the same
population as these 33 profiles. None qualifies a later source or supplies
missing current producer/phase bindings. Do not list the original B09 native
inventory wholesale as `NOT_RUN` or promote its old phase bytes to a current
closed capture.

Frozen-driver recapture preparation independently matched the original bytes
for CLARC original/neutral-renamed and six CodeSearchNet languages: eight cells,
1,350 selected tasks and3,798 source files. The original explicit64-token NL
policy is preserved. The eight-cell execution is recorded below. Original checkout
directories for24 OSA cells are absent, so their new recapture is `BLOCKED`;
retained top-k candidates cannot reconstruct those source universes. Preparation
and the independent controller are under `/private/tmp/qb09-owmq5d8q/`.

The fresh B09 available-source lane completed 8/8 cells at frozen f606 with
exit 0 and independent phase/diagnostic/source plus external-snippet score replay:
CLARC neutral-renamed/original and CSN Go/Java/JavaScript/PHP/Python/Ruby.
The bound inputs retain 1,350 tasks and 3,798 files, including the original explicit
64-token query policy. Raw records and replayed scores are under
`/private/tmp/qb09-owmq5d8q/`. The 24 unavailable OSA source universes remain
`BLOCKED`. These new eight cells do not restamp the historical 33-cell receipt,
and their overlapping datasets are not a combined holdout/ranking claim.

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
