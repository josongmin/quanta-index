# CS-INT-01 — Remaining integration and qualification

Status: `ACTIVE`. Completed L1–L5 product decisions are in
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md);
capture and process custody is in
[SEP-27-004](../../../adr/SEP-27-004-benchmark-capture-and-resource-custody.md).
This ledger owns cross-owner code-search acceptance. [MISC](../../sep-27-misc/tickets/INDEX.md)
owns common execution, CI enrollment, producer/consumer replay and supported
platform qualification. A historical local test total is not current-source proof.

## Open integration boundary

| Scope | Remaining acceptance |
| --- | --- |
| [ENG-02](CS-ENG-02-capability-publication-and-freshness.md) | Measure total format-9 coverage pipeline work and physical heap across growing and mixed updates; qualify the combined public producer/daemon path. A same-build seal avoids one decode, but preflight/build/open still scan the base and seal re-hashes pages. |
| [ENG-04](CS-ENG-04-match-anchored-snippets.md) | Admit parser/compiler/retained regex engines and caches under one physical request allocation owner across indexed, manual and preview callers; prove lifetime, refusal and matcher equivalence. Per-engine limits and logical charges do not close this. |
| [BENCH-01](CS-BENCH-01-corpus-gold-and-holdout.md)–[BENCH-04](CS-BENCH-04-comparators-performance-and-incremental.md) | Admit independent gold/holdout, native-bound real captures across remaining entrypoints/formats, each external indexed universe, independent statistical units, equal-work measurements and quiet-host costs. Local fixed-native refusals and opt-in OpenGrok file-view probes do not qualify the comparison. |
| External producer and readers | Run actual Semantica issuer/QBC tests, paired SDK/consumer migration, installed process, crash/restart, supported-platform and activation/rollback checks on one selected source. Format 8 and earlier require a format-9 rebuild. Prior issuer inspection is not execution proof. |
| Release integration | Execute the selected full repository and hosted CI inventory on the final source/config/binaries, including explicit ignored cases when their public claim is selected. Local owner fixtures are narrower. |

## Current-source audit and decision order

Historical audit basis: the Sep-28 integrated source through `fb7f98b6`. Recheck these
call paths after source changes; concurrent documentation edits and historical
receipts do not update the implementation state.

- [`RegexExecutor::compile_prepared`](../../../../crates/quanta-index-lq-regex/src/executor.rs) sets 10 MiB NFA and 2 MiB DFA-cache limits
  per engine. Selected preview reserves estimated logical bytes, but indexed,
  manual, predicate, snippet and structural callers also compile executors.
  There is no demonstrated request-wide physical allocation ceiling or observed
  runtime overrun. ENG-04 is an implementation/design gap, not a proven outage.
- For a new delta, [`DirectSearchCorpusMaterializer::preflight_batch`](../../../../crates/quanta-index-search-plane/src/ingest_dispatcher/search_corpus.rs) checks the
  lexical builder before durable intent; `publish_batch` checks it again under
  its operation lock; lexical [`build_batch`](../../../../crates/quanta-index-lexical/src/adapter_ingest.rs) plans coverage again. Each delta
  plan walks the sealed base. Seal still hashes effective pages and independent
  open verifies the generation. ENG-02 is a measured-cost and resource-boundary
  gap; do not infer a stale-result defect or promise sublinear total ingest.
- [`code_search_workflow.py`](../../../../tools/benchmark/code_search_workflow.py) admits one exploratory lexical pair and external
  five-product capture per spec. [`code_search_matrix.py`](../../../../tools/benchmark/code_search_matrix.py)
  now replays every cell in the declared repository × family × mode inventory,
  but the family list is caller-declared and is not bound to an independently
  adjudicated task population. Its verdict is `diagnostic_unqualified`; external
  whole-product indexed-universe attestation remains false. The qualified
  [`QUALITY_DELTA=pass`](../../../../tools/benchmark/retrieval/run.py) checks evidence/uncertainty, not
  the sign or a minimum useful effect. None of these signals alone admits a
  product win or default change.

| Order | Work and owner | Exit condition / stop rule |
| --- | --- | --- |
| 0 | Record the source baseline, affected contracts/test authority and claim scope; keep unrelated dirty documentation outside implementation ownership. | One audited baseline and declared local, installed, hosted and benchmark claims. Re-open this audit when a relevant source path changes; freeze final source after implementation. |
| 1 | ENG-04: prove parser, compiler, first-search and retained-cache allocation ownership on the pinned regex stack. If its API cannot authorize every relevant allocation before it occurs, choose a controlled dependency change before promising a hard bound. Then apply one request-lifetime owner across all live executor constructors. | Typed resource refusal before expensive allocation, correct release on failure/cancel, independent differential truth/range fixtures and optional-preview hit stability. An estimate or post-allocation RSS sample is not a hard bound. |
| 2 | ENG-02: instrument outer preflight, lock-held preflight, build, seal and open on growing one-file and mixed deltas. Count bytes read/written, hash/page/row work, elapsed time and phase-specific temporary/retained heap. Remove repeated base work only through an authenticated pinned immutable handle that preserves the pre-intent refusal and lock-time ownership check. | Independent row/identity/tamper/retry/old-reader tests, measured end-to-end cost and declared physical claim. If reuse is unsafe or not material, retain verification and report the measured bound instead of weakening it. |
| 3 | BENCH-01/02/03/04: admit an independent immutable corpus/gold/holdout and review the query-family inventory before capture; enforce native-derived validation at every qualified entrypoint and format; prove each comparator's indexed view. Use the existing fail-closed declared repository × query-family × mode matrix verifier. Freeze primary effect, critical-stratum regression and resource limits before holdout. | Every applicable cell has a bound `run`/`validate`/`replay` result or explicit failure; unsupported cells are `N/A`. Keep evidence-valid `QUALITY_DELTA` separate from a threshold-based product decision and use repository-level inference for cross-repository claims. |
| 4 | MISC-03/04/05 and external owner: execute the selected owner tests, full Rust/Python/daemon rails, adapter-specific large success/failure I/O, real issuer and public SDK/installed daemon mutation/restart/rollback, and supported platform/hosted CI on the final source. | Report each requested scope `VERIFIED`, `FAILED`, `BLOCKED` or `NOT_RUN`; focused local fixtures cannot close external, installed or hosted scopes. |
| 5 | MISC-06/07: run qualified quality, equal-work latency/resource and incremental/recovery comparisons only with admitted inputs, product index scope and quiet-host controls. | Publish denominators, independent units, intervals, costs and exclusions; decide defaults against the frozen thresholds. Otherwise retain a diagnostic result with no win/speed claim. |

## Parallel execution lanes

The table above is the dependency order, not a requirement to serialize code
edits. Start from one recorded base and use separate checkouts for these disjoint
owners. Account for the existing dirty documentation before branch creation;
do not copy or overwrite another owner's working-tree edits.

| Lane | Exclusive edit ownership | Parallel deliverable | Handoff / dependency |
| --- | --- | --- | --- |
| R — regex | `quanta-index-lq-regex`; lexical `searcher/*`, `query_admission.rs` and `regex.rs`; structural matcher/universe and regex lowering call sites. The integrator alone changes `Cargo.lock` or shared request-budget contracts if the feasibility result requires it. | Prove allocator API feasibility, then implement one request-lifetime physical admission owner and typed refusal for every selected compile/search route. Run differential truth/range, cancellation/drop and focused SDK fixtures. | Supply the selected dependency/API change and exact affected tests to integration. Do not claim a hard heap ceiling if any parser/compiler/cache allocation bypasses pre-admission. |
| C — coverage | Lexical `adapter_ingest`, `adapter_open`, `sealed_generation/*`, search-plane `ingest_dispatcher/search_corpus` and their owning tests. No searcher/regex edits. | Instrument both preflights, build, seal and open; measure growing deltas, then implement a pinned authenticated base handle only if it preserves pre-intent and lock-held refusal. Verify tamper, lineage, retry and old readers. | Supply phase counters and bounded physical claim. Keep full verification if safe reuse cannot be established; cost optimization cannot weaken identity checks. |
| G — gold and corpus | `corpus_release.py`, `corpus_binding.py`, `retrieval/corpus_set.py`, oracle/recipe/label fixtures and their tests. No native capture, verdict or shared registry edits. | Produce independent immutable source/gold releases, query-family/repository splits and a sealed holdout with explicit unsupported/unjudged strata. | Give the release/pack identity and declared semantics to lane V. Generated product results cannot become gold; absent reviewed labels block qualified quality only. |
| V — validation and integration | One owner for `code_search_workflow.py`, `live_lexical_external.py`, retrieval evaluator/verdict, native capture adapters, IO-5 harness, shared schemas/registry/`Justfile`/`benchctl.py`/`evidence_bridge.py` and plan documents. | Close admitted native-entrypoint negative controls, add a fail-closed matrix cell inventory and a separate predeclared product-decision gate; prepare large-output/metadata adapter tests and real-product readiness checks. | Synthetic controls can run before G completes. Final capture/scoring waits for G's admitted release and the merged R/C source; no duplicate parser or second run-store authority. |

Wave 0 is a short serial baseline: inventory owned dirty paths, freeze the
cross-lane interface and required claim matrix, and check availability of
independent labels, native product/index access, installed producer/daemon and
supported Linux/hosted runners. Missing access is a scoped `BLOCKED` result,
not an invented passing fixture. Lanes R/C/G/V can then edit and run narrow
owner tests independently. Do not run competing heavy Cargo builds or any
timing qualification on the shared host.

Integration is serial: reconcile each lane against the latest selected source,
merge shared contracts once, then run affected owner tests and the selected
full Rust/Python/daemon gates. After that, exercise the actual issuer, public
SDK, installed process, restart/rollback, adapter I/O and supported platform
paths. Only the resulting frozen source, admitted G inputs and verified native
index scopes may enter quiet-host quality, latency, resource and incremental
measurements. Reuse compatible captures; rerun evidence only when its bound
source, inputs or configuration changed.

Closure is a per-claim matrix, not a single green label: implementation,
owner-local tests, installed producer/daemon, native five-product matrix,
qualified quality/speed, hosted CI and Linux each receive their own terminal
status and exclusion. A missing external product, reviewed label, host or
platform cannot be papered over by a diagnostic run; it blocks only the
dependent claim and leaves completed local code independently reviewable.

## Verification boundary

Use registered `Justfile` and `./scripts/cargow` selectors for the affected
owners. Routine focused checks need their observed terminal result and relevant
source scope. Formal replay, release and qualified benchmark claims bind the
selected source, dependencies, config, binaries, inputs and environment; missing
or stale evidence blocks only its dependent claim. Previously recorded local
counts, scratch logs and working-tree overlays are historical and recoverable
through [the plan archive](../../ARCHIVE-INDEX.md). No current release,
external-producer, physical regex/RSS or qualified benchmark claim is made here.

## Serial acceptance boundary

The decision order above is the single execution plan. Preserve these
invariants while integrating its stages:

- Preserve unrelated edits and one owner for shared schema, SDK, storage and
  selector changes. Keep existing legal domain/projection/empty/window semantics,
  canonical file mutation, lineage/coverage, exact-name/federated top-k,
  cancellation and matcher-aligned previews unless a current regression proves
  a change is needed.
- Format-9 seal/open/tamper/resident and unchanged-segment fixtures must pass on
  the selected source. The old private SSTable graph and format-7 receipts are
  historical; source digest alone cannot prove full-file bytes or parser coverage.
- Enroll affected tests and normative ADRs in existing rails. Resolve required
  test identities from live collection and execute ignored process cases when
  their public claim is selected. Refuse stale, missing or partial formal evidence.

## Required scope and exclusions

- Contract/engine: exact-reference legal/empty/count/cursor, strict storage decode,
  same-path federation, wrong-source and interruption/resource controls.
- Publication: failed-parse replacement/repair, source replay/conflict/reorder,
  Delta inheritance, read-while-write, activation/retention and real crash cuts.
- Preview: original NFC/folded/UTF-8/CRLF bytes, Boolean focus, oversize/overlap,
  optional refusal, restart and mutable-checkout drift/deletion.
- Producer: current grammar/parser/ownership inventory, retained malformed files,
  capability-enabled lexical and strict symbol Vite paths through actual daemon.
- Final-source full/installed/platform proof belongs to MISC-04/05. Real host /
  independent benchmark input admission belongs to BENCH-01/03/04 and MISC-06/07.
- ANN replacement, learned reranking, new default weights, embedding-free
  activation and expanded platform/product support are separate decisions.

Current contract details and commands are in the ADRs, registered `Justfile` /
`./scripts/cargow` rails and MISC acceptance. Historical handoffs, test totals and
RCA bodies are recoverable through the [plan archive](../../ARCHIVE-INDEX.md).

## Sep-28 integrated code audit

Historical Sep-28 integration branch: `codex/benchmark-validation`, based on
`d7d62b8d`. The R/C/G/V owner changes were integrated serially; use the final
commit printed by `git rev-parse HEAD` for subsequent source-bound runs. The
main checkout's unrelated dirty documentation was not copied into this branch.

| Scope | Current code result | Remaining boundary |
| --- | --- | --- |
| ENG-04 | Pre-parser 64 KiB gate covers the common executor and indexed scope. Shared indexed scope compilation preserves NFA-size/DFA-state refusals using the pinned external FST grammar, without vendoring or diagnostic-text matching. File path/name filters share one automaton; Sourcegraph lowering preserves typed refusal. | **OPEN:** pinned regex dependencies still allocate AST/HIR/compiler/cache data without request-scoped pre-allocation admission. A request-wide physical ceiling needs dependency hooks, shared leases and structural/searchd error mapping. Typed refusal is not an aggregate heap cap. |
| ENG-02 | Phase counters and growing/mixed lexical fixtures expose three base coverage walks. Mutation after either preflight is refused before target creation; old readers and repaired retry remain valid. | **OPEN:** at 2,048 files each walk read 256 pages, 2,048 rows and 883,542 encoded bytes. The current port supplies no immutable authenticated base token across preflight/build, and the internal lock cannot stop external disk changes. No full pipeline physical-heap claim. |
| BENCH-01 | Source-derived raw-literal/named-function gold capsule and blind family pack have independent fixed-span fixtures; all labels explicitly remain mechanical/unreviewed. | **OPEN:** human adjudication, a fresh valid corpus release and externally sealed holdout are absent. Capsule blind format is distinct from the existing evaluator query pack. |
| BENCH matrix | `code-search matrix-verify` checks the release repository inventory against every declared family and all three modes, replays each native capture, and rejects source/input/binding/route substitution. The owner is enrolled in source closure, test authority and the benchmark-control rail. | **OPEN:** the query-family declaration itself needs pre-capture external review; no valid complete external five-product capture or product indexed-universe attestation exists. Output remains `diagnostic_unqualified`. |
| IO-5 | External rows are written incrementally and large result files/binaries are hashed as payload. Archive names with portable case/Unicode aliases refuse before extraction; large failed stdout/stderr and many-entry controls are added. | **OPEN:** all actual adapter prepare/execute/publish/load/replay paths, heap and hosted resource limits remain to be measured. |

Do not collapse these rows into a single `VERIFIED` claim. The owner tests and
lint/format rail are local code checks; installed producer, native products,
independent labels, Linux/hosted CI, quiet-host benchmark and activation remain
separate execution gates.

## Sep-29 source remediation

The live searchd structural producer now compiles every repo/file regex through
the common regex executor before an empty candidate set or repo mismatch can
short-circuit. It reuses compiled file filters across chunks and preserves a
typed plan-limit error. The pure-negative search-plane universe also validates
all filters before returning an empty result. This closes the direct daemon
input-limit bypass and the malformed-filter empty-success paths; it does not
provide the request-wide **physical** regex allocation ceiling required by
ENG-04.

The code-search matrix now requires Quanta's frozen `native` execution profile
in every mode. Its bare-symbol five-product scorer admits explicitly judged
no-answer tasks without assigning them recall: answerable tasks alone form the
recall denominator, while no-answer tasks report an independent empty-result
rate. The capture adapter preserves those distinct typed observations on
replay. This is diagnostic scoring machinery, not independent gold, a complete
live five-product capture, indexed-universe attestation or a qualified product
comparison.

The raw archive reader now checks the writer's canonical ZIP metadata in both
central and local records before extraction, including streaming ZIP64 and
descriptors. Noncanonical historical ZIPs must be refrozen; matching payload
bytes alone do not make their envelope admissible. Adapter-specific IO-5
resource and actual-product acceptance remain open.

## Sep-29 current-main owner pass

The audited base was `a9a43529` (`main` and `origin/main`). The shared working
tree also contained 68 pre-existing modified paths outside this pass's edit
ownership; results that load those paths are current-overlay checks, not clean
HEAD or release qualification. The following changes and checks are owner-local:

| Scope | Source result and observed local check | Remaining boundary |
| --- | --- | --- |
| ENG-04 | The executor reuses one validated AST for dialect filtering, HIR translation and capture erasure. Regex package 98 tests, lexical preview 17 and SDK/daemon preview/restart 7 passed; Clippy passed. | Pinned parser/compiler/cache APIs have no request pre-allocation hook. Aggregate physical admission remains `BLOCKED`, with no observed overrun claim. Dependency hooks and a cross-route lease, or a separately designed OS-isolated worker, are required. |
| ENG-02 | Coverage-row decoding no longer requests exact allocation per row. Owner tests and 128/512/2,048-file diagnostics passed; a 4,097th row refuses. | Three authenticated base scans still occur. At 2,048 mixed files, each reads 256 pages and 883,542 encoded bytes. Total ingest sidecars and phase physical heap remain unqualified. |
| BENCH-02 | Sourcegraph native line spans beyond the supplied line's character length now refuse, including non-ASCII input; the Sourcegraph adapter suite passed 32 tests. | This is an offline native fixture, not live five-product capture or indexed-universe attestation. |
| IO-5 | External JSONL replay now validates bounded lines and complete pinned bytes without a 16 MiB whole-file cap. The adapter suite passed 20 tests, including a larger-than-16-MiB file and malformed inventory. | Other actual adapters, fresh-process RSS and hosted limits remain open. |
| Local process | The clean-owner `runtime_extended_suite` crash matrix passed 4 tests with a launched daemon and public SDK. Three selected ignored SDK L2 tests also passed against a real daemon, including delta/restart, cross-stream activation and named crash cuts. | The runs included the shared dirty dependency overlay; the SDK L2 test and client were themselves pre-existing dirty files. Installed distribution, external Semantica issuer, full suite and supported Linux/hosted claims remain separate. |

The source changes above require a serial final-source check before integration
closure. Passing local owner tests does not change the independent benchmark
gold, holdout, product or performance statuses.

The broad `just python-test` run observed 2,815 passes, 9 platform skips and
one retrieval proof-inventory failure on the changing shared overlay. The
new Sourcegraph refusal initially introduced a collected test identity outside
the existing required-test authority. Its assertions now live in the existing
native-span test; the Sourcegraph and proof-contract selectors passed 66/66 on
the corrected source, and repository-wide Ruff lint/format passed. The broad
run was not repeated on an immutable final source, so it is not a full-suite
`VERIFIED` result.

## Sep-29 post-integration audit

At the start of this audit, `6e79e558` was both `main` and `origin/main`;
the prior 68-path overlay was committed and the checkout was clean. The new
retrieval rank-unit and semantic work-bounded paths are present in current
source. The proof inventory missed the newly collected Sourcegraph
out-of-range line test, so its source-controlled required-test list was
corrected in this pass. The retrieval proof inventory then passed 35/35,
the retrieval benchmark Python suite passed 384/384, and Python lint and
format checks passed. These are local checks; installed/external
qualification has separate status.

Current implementation gaps remain ENG-04's request-wide physical regex
allocation admission and ENG-02's repeated authenticated base coverage walks
and unqualified total heap. BENCH-01–04 still require independent reviewed
inputs, complete native captures and indexed-universe evidence, and qualified
product measurement. The new rank-unit and semantic request contracts do not
by themselves close those acceptance boundaries.

The current tree also has real binary-SDK bounded-semantic settlement and
ANN-generation exact-bypass tests (`sdk_frontdoor` and
`e2e_ann_incremental_seal`); the earlier concern that this route had only
wire and unsupported-adapter tests was stale. A persisted semantic-adapter
regression now pins the native boundary separately: on a sealed ANN
generation, insufficient work refuses before any dense query, exact allowance
serves an independent exhaustive cosine oracle through the exact lane, and a
second charge against that allowance refuses without another query. This is
owner-local functional proof, not physical resource or installed-product
qualification. The focused semantic-adapter test and the selected
`runtime_extended_suite` ANN and `runtime_fast_suite` binary-SDK tests each
passed 1/1 locally on this source. ENG-02 and ENG-04 remain open on their
stated mechanisms.

The subsequent structural `where` audit found a reachable refusal gap:
searchd compiled regexes only while traversing candidates, and the Boolean
dispatcher could skip invalid regexes after an empty lexical intersection.
Current main precompiles and reuses each structural block's distinct regexes,
preserves invalid-input versus resource versus engine-failure codes, and
validates skipped Boolean leaves without invoking the producer. The structural
owner suite (82 tests), structural dispatcher selection (57 tests), searchd
structural selection (6 tests), binary-SDK structural E2E (1 test), fmt and
affected-crate Clippy passed locally before commit `19e9add9`. This is a
scoped source fix; it does not qualify ENG-04's aggregate physical heap or
ENG-02's repeated coverage scans.

The next current-source RCA found that the eight-distinct-`where` limit above
was applied per block and to skipped leaves, but a Boolean request could spread
nine patterns across nine executed leaves. Structural dispatch now counts
distinct patterns across the complete expression before universe or producer
work; direct producer callers retain the per-block gate. Skipped leaves keep
only validated pattern identities and release temporary compiled engines.
The structural selection passed 59/59 tests, the SDK structural E2E passed
1/1, affected-crate Clippy and fmt passed, and the contract public-API
baseline was brought into sync with the already exported limit constant.
ENG-02's current 128-file mixed-delta probe still read 100 base pages and
54,479 page bytes in each of three phases; both between-phase mutation tests
passed. These source-local results do not close ENG-02's repeated scans or
ENG-04's physical aggregate allocation boundary.

## Sep-29 current-head audit and execution decision

At `a6821da6`, `main` and `origin/main` match and the checkout is clean. The
latest commit adds an indexed/manual selected-source integrity regression and
changes only structural test formatting besides that test. On this head,
`./scripts/cargow test --locked -p quanta-index-lexical --lib
l4_source_decode_regressions` passed 4/4 and `./scripts/cargow test --locked -p
quanta-index-search-plane --lib structural` passed 62/62. These focused owner
results do not qualify the full repository, installed daemon or benchmark.

The current code still calls lexical coverage planning from the outer
materializer preflight, the lock-held preflight and lexical build. Each delta
plan re-verifies the sealed base. Keep both pre-intent and lock-held tamper
refusals; first measure all pipeline phases and physical heap, then remove a
redundant walk only if an authenticated immutable base capability owns the
verified bytes through build. If that capability is unsafe or the measured
benefit is immaterial, retain the scans and report the observed cost.

Structural lowering now counts distinct `where` regexes across the complete
Boolean request before Sourcegraph engine compilation or producer work. The
shared executor still applies only per-engine NFA/DFA limits, and parser,
compiler and retained-cache allocations have no request-wide pre-allocation
owner. Prove dependency hook feasibility across parse, compile, first search,
cache and drop before promising a physical ceiling. If the pinned stack cannot
provide that contract, choose a controlled dependency change or an explicitly
designed isolated worker; logical charges and post-allocation RSS are not
substitutes.

The declared matrix verifier already exists and returns
`diagnostic_unqualified`. BENCH-01–04 therefore need independently reviewed
families/gold/holdout, remaining native entrypoint/format refusals, real
five-product captures and indexed-universe attestations, then admitted
equal-work quality/resource measurements. Do not reimplement the matrix
inventory. Run full, installed, external-producer and hosted/platform
qualification only after the affected implementation and input contracts are
selected on one final source.
