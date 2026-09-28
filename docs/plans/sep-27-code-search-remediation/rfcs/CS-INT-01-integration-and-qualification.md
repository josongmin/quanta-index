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

Audit basis: the Sep-28 integrated source through `fb7f98b6`. Recheck these
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
| 3 | BENCH-01/02/03/04: admit an independent immutable corpus/gold/holdout; enforce native-derived validation at every qualified entrypoint and format; prove each comparator's indexed view; implement a fail-closed matrix inventory for applicable repository × query-family × mode cells. Freeze primary effect, critical-stratum regression and resource limits before holdout. | Every applicable cell has a bound `run`/`validate`/`replay` result or explicit failure; unsupported cells are `N/A`. Keep evidence-valid `QUALITY_DELTA` separate from a threshold-based product decision and use repository-level inference for cross-repository claims. |
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

Selected integration branch: `codex/benchmark-validation`, based on
`d7d62b8d`. The R/C/G/V owner changes were integrated serially; use the final
commit printed by `git rev-parse HEAD` for subsequent source-bound runs. The
main checkout's unrelated dirty documentation was not copied into this branch.

| Scope | Current code result | Remaining boundary |
| --- | --- | --- |
| ENG-04 | Pre-parser 64 KiB regex input gate now covers the common executor and direct Tantivy lexical scope compilers; Sourcegraph lowering preserves typed resource refusal. | **OPEN:** pinned `regex-automata` and `tantivy-fst` allocate AST/HIR/compiler/cache data without a fallible pre-allocation hook. A request-wide physical ceiling needs controlled dependency APIs/forks, shared leases and structural/searchd error mapping. This mitigation is not an aggregate heap cap. |
| ENG-02 | Phase counters and growing/mixed lexical fixtures expose three base coverage walks. Mutation after either preflight is refused before target creation; old readers and repaired retry remain valid. | **OPEN:** at 2,048 files each walk read 256 pages, 2,048 rows and 883,542 encoded bytes. The current port supplies no immutable authenticated base token across preflight/build, and the internal lock cannot stop external disk changes. No full pipeline physical-heap claim. |
| BENCH-01 | Source-derived raw-literal/named-function gold capsule and blind family pack have independent fixed-span fixtures; all labels explicitly remain mechanical/unreviewed. | **OPEN:** human adjudication, a fresh valid corpus release and externally sealed holdout are absent. Capsule blind format is distinct from the existing evaluator query pack. |
| BENCH matrix | `code-search matrix-verify` checks the release repository inventory against every declared family and all three modes, replays each native capture, and rejects source/input/binding/route substitution. The owner is enrolled in source closure, test authority and the benchmark-control rail. | **OPEN:** the query-family declaration itself needs pre-capture external review; no valid complete external five-product capture or product indexed-universe attestation exists. Output remains `diagnostic_unqualified`. |
| IO-5 | External rows are written incrementally and large result files/binaries are hashed as payload. Archive names with portable case/Unicode aliases refuse before extraction; large failed stdout/stderr and many-entry controls are added. | **OPEN:** all actual adapter prepare/execute/publish/load/replay paths, heap and hosted resource limits remain to be measured. |

Do not collapse these rows into a single `VERIFIED` claim. The owner tests and
lint/format rail are local code checks; installed producer, native products,
independent labels, Linux/hosted CI, quiet-host benchmark and activation remain
separate execution gates.
