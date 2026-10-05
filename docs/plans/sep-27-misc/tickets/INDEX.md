# Benchmark and retrieval remaining execution

Status: `ACTIVE`. Completed implementation/RCA/terminal chronology has been
consolidated into [SEP-27-004](../../../adr/SEP-27-004-benchmark-capture-and-resource-custody.md).
Code-search contracts are in [SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md).
Registry/source/test authority remains executable; this is the single common
execution and acceptance ledger. Historical counts are not required inventories.

## Remaining root-cause union

| Owner | Remaining action / acceptance |
| --- | --- |
| BENCH-02 | Local cs/Sourcegraph/OpenGrok fixed-native path/hit refusal is implemented. Audit every admitted acquisition/scoring/replay entrypoint and remaining format; complete the native negative matrix and qualify actual captures. The [native ticket](../../sep-27-code-search-remediation/rfcs/CS-BENCH-02-native-response-validation.md) owns remaining semantics; the bare normalized scorer is diagnostic. |
| BENCH-03 | Current qualified verdict resamples whole query families and refuses insufficient independent families/categories; task-level intervals are descriptive. Admit independent multi-repository gold/holdout, frozen effects/budgets and repository-level inference before broader quality claims. [Statistics ticket](../../sep-27-code-search-remediation/rfcs/CS-BENCH-03-tracks-metrics-and-statistics.md) owns the remaining acceptance. |
| BENCH-04 | OpenGrok can opt into exact indexed-file inventory and served-byte probes bracketing search queries; this remains diagnostic without posting freshness and current admitted external corpus. Sourcegraph/cs index scope, product mutation/restart and equal-work timing are unrun. [Comparator ticket](../../sep-27-code-search-remediation/rfcs/CS-BENCH-04-comparators-performance-and-incremental.md) owns execution. |
| MISC-05 / ENG-02 | Qualify total coverage pipeline work/physical heap/bytes, including remaining preflight/build/open full scans and every sidecar. Same-build seal no longer decodes coverage twice, but re-hashes every effective page. [Coverage ticket](../../sep-27-code-search-remediation/rfcs/CS-ENG-02-capability-publication-and-freshness.md) owns the cost issue; no stale-result defect is implied. |
| MISC-04 / actual producers and consumers | After owner changes, execute required Python/Rust contracts, actual native/Criterion/SDK/contract production, fresh promoted-path validation and relocated Python/Rust consumers/replay. Preserve process/monitor/capture failure controls and live test identities. Local selected passes do not establish hosted/product qualification. |
| MISC-03 / IO-5 | Complete adapter-specific large successful/failed output, many-entry metadata/archive and interruption/corruption acceptance through prepare/execute/publish/load/replay. Declare retained-metadata limits separately; use independent bytes/digests and fresh-process RSS for actual resource claims. Generic streaming/probe success is not every adapter's acceptance. |
| MISC-05 / product/platform | Execute selected functional invariants, combined Rust/daemon, actual installed ingest/restart/crash and supported Linux delegated-cgroup/Landlock scopes. External producer issuance and current-format rebuild/activation need their own boundary; no unrun platform promotion. |
| MISC-06/07 / measurements | Execute admitted real profile/comparator pilot and independent quality/performance/update/test-cost measurements with declared units, host, inputs and denominators. Exploratory output remains diagnostic; manual labels/license/host prerequisites block only dependent claims. |

The owner implementations for atomic profile publication/GC, sticky capture
failure, process/monitor construction, bounded raw/archive I/O and C4/C5 selection
are retained decisions, not open feature tickets. Their required actual-producer,
consumer, resource and product acceptance is owned above. Reopen implementation
only for a demonstrated regression. Old proof-navigation work is closed after
historical proof cleanup; do not recreate per-run repository evidence files.

Completed regex/input, coverage, native-row and archive mitigations are owned by
[SEP-27-003](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md),
[SEP-27-004](../../../adr/SEP-27-004-benchmark-capture-and-resource-custody.md) and
[OCT-05 ADRs](../../../adr/README.md#oct-05-implemented-contracts).
The exact regex allocation cap remains [conditional/deferred P3](../../../adr/SEP-27-003-code-search-source-and-preview-contract.md#deferred-regex-allocation-cap-cs-eng-04).
Coverage reuse needs authenticated immutable base ownership; actual producers,
native scope, resource and supported-host/platform acceptance remain open above.
Earlier integration receipts are recoverable through
[the plan archive](../../ARCHIVE-INDEX.md#oct-05-benchmark-and-quality-ledger-compaction).

## Execution order and shared boundary

Use the source-first [CS-INT-01 execution plan](../../sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md#integration-order)
with the declared coverage, gold and capture owners:
measured coverage cost → one serial
combined-source integration with IO-5/actual-producer acceptance → admitted
MISC-06/07 measurements. Independent benchmark inputs can be prepared in
parallel, but captures bind the final source. Do not launch competing heavy
Cargo/timing jobs on the shared host or repeat one capture for multiple tickets.

Preserve one owner for shared `benchctl.py`, `evidence_bridge.py`, selector,
source-closure and schema edits. Retain exact source/input/registry/raw and
independent replay refusal for formal qualification. For routine verification,
terminal checks under AGENTS.md are sufficient; no new one-off proof tree is
required. Verification commands below are selected by claim, not an instruction
to rerun every historical target.

### Integration acceptance

Historical owner-local Rust/Python/daemon and Clippy results are archived and
do not establish the final selected source's test state. The integration plan
in [CS-INT-01](../../sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md)
sets the next execution boundary. Installed, external-producer, hosted and
platform rows remain separate.

- [ ] Revalidate C4/C5 inventories, hosted CI and actual producer paths at the
  combined gate; existing owner collection/enrollment is implementation authority.
- [ ] Actual native immutable and command-only execution, Criterion and admitted
  capture adapters preserve terminal/log/cleanup and cooperative monitor facts.
- [ ] Complete-profile pointer/GC, every admitted failure phase, nested sticky
  failure, abrupt death, post-commit uncertainty and relocated replay execute.
- [ ] Python/Rust canonical fixtures and both actual consumers agree; malformed,
  unknown/duplicate/nonfinite, wrong-source/input and partial captures refuse.
- [ ] IO-5 declares byte/cardinality/metadata/heap/RSS scope and measures the actual
  claimed adapter paths at increasing sizes, including large failure output.
- [ ] Final affected functional/SDK/storage/producer/platform scope runs after
  combined source selection; formal qualification binds relevant identities.
- [ ] Hosted CI, installed release, performance and activation are classified
  separately; missing evidence cannot be changed into a passing summary.

## MISC-05 — functional, installed and platform qualification

### Test-optimization invariant census

These are coverage obligations, not presumed remaining implementation defects.
Resolve current selectors from `tools/ci/test-authority.toml` and actual
collection, then record one owner/oracle/terminal result per row.

| ID | Current owner surface | Independent invariant |
| --- | --- | --- |
| D1 | searchd runtime end-to-end/repomap fixtures and searchd harness | Long-path configuration covers all three sockets, including ingest. |
| D2 | searchd runtime `tests/common/searchd_binary_process.rs` | Already-exited child cleanup cannot signal an unrelated reused process. |
| R1 | runtime lifecycle/lease fixtures | Explicit acknowledged release; no mandatory three-second happy-path sleep. |
| R2 | embed OpenAI retry/sleeper seam | Injected test delay; production retry bounds/jitter unchanged. |
| R3 | embed concurrency fixtures | Structural barrier proves four-way overlap, not a 25 ms timing guess. |
| R4 | IPC `PeerWatch` | Explicit wake/disarm/join; cancellation needs no compensating sleeps. |
| R5 | runtime ingest-resource envelope and lower core owners | Boundary assertions conserved below E2E; one wiring proof retained; daemon boots counted. |
| WA-1 | lexical trigram property tests | Unexpected error makes the property fail, never a skipped success. |
| WA-2 | runtime matrix smoke | Exact expected candidate identity/set, not any in-corpus row. |
| WA-3 | runtime state migration | Manifest/object/digest completeness; missing/corrupt data fails. |
| TH-1 | SDK-frontdoor observation waits | Never-ready input returns typed timeout, not stale `Ok`. |
| TH-2 | runtime process-envelope scrape waits | Never-true predicate returns typed timeout. |
| TH-3 | SDK binding fixtures | RAII temporary path custody; no pid-only persistent socket directory. |
| TH-4 | runtime filter-execution cases | Immutable family fixture reuse with per-case context and no shared mutable daemon. |
| PO-1 | core `timeref.rs` | Injected exact clock and fixed boundary matrix; convenience edge samples once. |
| PO-2 | SDK `config.rs` | Injected environment precedence/errors; no process-global mutation seam. |
| PO-3 | catalog connection/idempotency | One clock sample per transition; less/equal/greater deadline matrix. |
| PO-4 | search-plane `single_flight.rs` | Outcome-or-cancellation wake; no correctness dependence on 20 ms polling. |

Do not shorten sleeps, disable production jitter, weaken errors, share mutable
global fixtures or delete lower-layer assertions to make timing look better.
Only reproduce-and-fix a current regression; otherwise retain the implemented
owner and execute its qualification.

### Installed ingest and process/resource checks

- Use actual separately launched searchd and installed/public SDK for publish,
  seal, CAS activation and query. In-process harness/direct IPC is not SDK proof.
- Exercise fresh, replace, delete, edit/rename where claimed, reopen/replay,
  tombstones, real fault injection and restart. Compare full expected row sets,
  unchanged owners, generation, activation and disappearance of old hits.
- Transient ingest observation binds request/repo/revision/batch/generation/
  receipt/activation; reject mixed, missing, partial or stale observations.
  Do not add transient stage timings to durable receipts. Old V2-incompatible
  wire requests must be rejected before dispatch; body digest is preserved.
- macOS v1: reject emitted PID duplicates, per-process peaks above tree peaks
  and per-process samples above global samples. A live zero-RSS root may be
  absent from positive-RSS metric rows; those rows do not prove PID start identity.
- Linux: actual delegated-cgroup and Landlock positive execution under an
  available supported host. Fake-owner/process-group tests are not that proof.
- Whole-process-tree resources include daemon/provider children. Record index
  bytes separately from shared model-cache bytes. Windows native pair and new
  canonical symbol-text authority are `NOT_APPLICABLE` absent new product scope.

DoD: focused invariant terminals plus same-source full Rust/daemon rails and
installed/platform evidence for the claims selected. Remaining host access
blocks only that platform; no generic all-platform success.

### Retained retrieval implementation invariants

These contracts remain required regression coverage; they are not additional
feature tickets. Concurrent engine changes affect several owners, so their
current implementation/qualification must be rechecked at the serial freeze.
This documentation refresh does not claim to have qualified those changes.
Reopen a code change only for a demonstrated failure.

| Surface | Required invariant / negative control |
| --- | --- |
| Query policy | Exactly one of native/literal/natural_language; preserve native DSL AND and literal escaping. Natural-language lexical token-OR and semantic text have distinct bound identities. Gold/category/holdout data never drives planning. |
| Observation | Executed versus contributed lanes are distinct; OFF omits query-stage collection/DTO data but preserves operational/deadline clocks. Compare exact enabled/disabled startup policy and config digest; server/SDK/sidecar timings remain separate. |
| Semble mode dispatch | native-default, hybrid-no-rerank, lexical-only and semantic-only use the same pinned function/profile in cold, warmup and measurement. Bind requested/actual alpha, rerank, lane counts, depth and upstream source; alpha endpoints do not prove only one lane executed. |
| Symbol publication | Chunks and symbols replace together; symbol-only changes alter scope digest. Reject duplicate/cross-kind IDs. Parser/grammar/lockfile/capability identities and per-path unsupported SHA/reason are explicit; supported parse failure is coverage failure. |
| Symbol ownership | Use AST ownership, not delimiter/lexical guesses. Rust generic impl owners and direct method scope, JS/TS function declarations versus explicit methods, and Python nearest named scope have independent fixtures. |
| Result authority | Published typed unit registry, generation, path and byte span authorize hits. Reject forged/stale/unanchored hits. Unsupported symbol Phrase/RawString/Regex/regexp-keyword/content-filter combinations remain typed refusals, never silent chunk fallback or empty exhaustive success. |
| Span accounting | Indexed identity drives rank; returned bytes/tokens drive context cost; independent source spans drive exact recall. Union overlapping spans; hand-check Unicode/CRLF/long-line cases and line-expanded context. |
| Semantic parity / ANN | Full vectors with pinned model/tokenizer/config, canonical adversarial inputs, norms and pairwise directional checks. Reject omitted/subset/reordered/forged vectors and nonfinite/scalar-type substitutions. Independently exhaustive-scan the same rows; cover 255/256, short/full result, filter/page/churn boundaries. |
| Fetch experiment | Only typed integer 25/50/100; default 100. Requested policy, daemon config and actual initial-fetch trace agree. Preserve ceiling/refill/generation pinning/force-empty; reject bool/float/alias/unknown/missing/duplicate settings. No omitted/duplicate hits across pages. |

Minimum owner checks are the retrieval Python contract, Rust chunking/library,
actual SDK process, relevant storage/semantic integration and asset-backed
model tests when claimed. Asset-free validators cannot substitute for actual
model execution or production-served ANN checks.

## MISC-06 — measurement, with separate denominators

| Track | Required workload and reporting |
| --- | --- |
| TOPT R1-R4, TH-4 | At least five warm paired samples per selector; median/p95/min/max, selected/executed/failures. Cold build separate; same features/toolchain/cache/target policy. R5 measures assertions/cases/daemon boots, not invented speedup. |
| Query observation | Same source/binaries/inputs on/off; roomy and tight deadlines; k=1/10/100 plus public cap boundaries; filters and explicit fetch floors 25/50/100. Compare result/order/page/cursor/failure, coverage, planner/lane/candidates. Normalize only request IDs and stage timings. Keep default hybrid floor 100. |
| Installed ingest | Fresh-root time-to-searchable separately from replace/delete/fault/restart latency; full row-set/activation correctness is prerequisite. Fresh root is the statistical unit. |
| DSL authority | Canonical Linux, warm/cold separate, current registered floors 200 warm / 20 cold. Actual admitted baseline and host required; a plan listing is not capture. |
| Micro | Both `quanta-index-lq-norm/pipeline` and `quanta-index-searchd-runtime/dsl_query_matrix`, every binary-listed case, smoke, raw samples/estimates and immutable binary identity. Criterion diagnostic minimum 10 samples and 1,000 resamples is not performance admission. |
| Systems | Freshness truth and offered-load producer; offered/accepted/completed/dropped/error/timeout counts, generator saturation/health, latency, RSS and disk. Closed-loop QPS does not establish open-loop capacity. |

Correctness precedes timing. Retain failure/timeout/partial samples and their
denominator; never remove failures and report a faster survivor distribution.
Predeclare clock/resolution, transport/serialization inclusion, warmup, cache,
power/thermal/governor controls, compiler profile and instrumentation mode.
Do not run competing Cargo gates or both products concurrently on a timing host.
`QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` cannot qualify performance.

Bind before/after source diff for test-cost attribution. Unrelated intervening
changes mean retrospective comparison, not causal proof of one optimization.
CI/p95 from five observations is descriptive; do not invent statistical power.
Baseline admission binds explicit immutable run ID/digest, compatibility,
margin and uncertainty method. Missing baseline is not zero regression.

## MISC-07 — profile coverage and retrieval acceptance

### Execution inventory and product pilot

For every active profile/case, record registry digest, required input,
producer command, terminal count, raw inventory, validator/scorer, capture ID,
fresh replay, claim class and exclusions. `list`, `plan`, `runnable=true`,
compile, fixture and local replay are not interchangeable evidence.

Resolve current profiles/families/cases through `registry.toml` and `benchctl
list`/`plan`; [benchmark usage](../../../../tools/benchmark/README.md) owns commands.
The ledger does not duplicate the registry's profile table.

Distinguish absent external inputs from an absent executable adapter. If an
active registered producer still lacks a supported capture/payload/replay path,
record that exact implementation gap under this ticket and repair its existing
adapter boundary; do not rename it an external-input block or fabricate a
generic latency payload. Do not count one profile's subset as quality-full.

- Run representative real Just/daemon, Criterion, Python adapter and recorded
  import through the common CLI. Unsupported external-input profiles refuse
  before publishing a supported subset as a whole profile.
- Execute a same-release/same-query-pack live pilot on at least two repositories
  for Quanta and accessible comparators: Semble, Sourcegraph, OpenGrok and
  codespelunker where capabilities permit. No obligation to fake support or
  access for all five. Record actual indexed/searchable universe, query
  transformation, native rank, response completion, freshness and failures.
- Separate lexical/semantic/hybrid/symbol strata and `native_default` versus
  `controlled_mechanism` lanes. A lexical file result is not semantic quality;
  file-only output is not a fabricated whole-file span. No combined leaderboard
  across incomparable units or native/emulated/remote execution boundaries.
- Literal/regex truth comes from pinned source bytes and an independent oracle.
  Developer relevance needs adjudicated qrels; keep unjudged distinct from
  irrelevant and report judging-pool coverage/sensitivity.
- Recorded agent imports stay unauthenticated diagnostics. Authenticated
  outcome requires actual frozen tasks, independent baseline tests,
  trajectories and raw executed-test receipts; aggregate booleans do not suffice.
- A two-repository or 20-query pilot proves machinery and diagnoses gaps, not
  all-language superiority. No product ranking/default change without an
  independently demonstrated defect and the declared evaluation below.

### Retrieval acceptance authority

Execute the existing [T00–T17 controls, sample floors and paired decision contract](../../../adr/SEP-26-003-retrieval-evidence-custody-and-qualification.md#t00-t17-blocking-matrix)
on one bound source/input/model/host. They are permanent admission rules, not
unfinished API tickets. Same-model/incremental controls are conditional on those
claims; retain actual fault/restart execution for any corresponding claim.

Current schemas/strategies, independent gold/isolation, byte-density scoring,
returned-window diagnostics, five cold roots, warm distinct-task/root/sample
floors and final-path replay are ADR-owned. Missing external prerequisites block
the affected quality/speed claim; a valid exploratory pair cannot promote itself.
Current runner records use schema 5; schema 3/4 readers serve historical replay.

## User-owned inputs and explicit exclusions

These are prerequisites/decisions, not coding steps:

- License/attribution approval; two independent annotations and adjudication;
  frozen development/holdout corpus/query packs; exact model/tokenizer assets
  and Semble lockfile; actual quiet host and supported Linux access.
- Historical prospective TOPT admission cannot be recreated. User chooses
  whether the original requirement remains unsatisfied or receives a permanent
  explicit exclusion while retrospective measurements are judged separately.
- Hosted CI billing/access needs a fresh check before assigning current status.
  Release/deployment/activation requires separate authorization and evidence.
- New symbol-text authority, native Windows pair and expanded product support
  are out of scope unless explicitly reopened. Keep typed refusals.

Do not create approvals, gold, quiet-host claims, authentication or process
attestation in code. No external input blocks unrelated implementation closure.

## Select verification and stop conditions

Use [benchmark usage](../../../../tools/benchmark/README.md),
[retrieval usage](../../../../tools/benchmark/retrieval/README.md) and current
Just/test authority to select affected owner contracts, actual producers and
public replay. `benchmark-control-contract-local`, `benchmark-prep-local` and
`retrieval-contract-local` are edit-loop entrypoints; source-bound capture,
`verify-rust`, extended daemon, SDK/contract and actual pair rails are separate
acceptance. Do not repeat SDK/contract producers already executed by the same
complete profile. Public API, IPC and module changes add their respective
affected gates; unrelated gates are `NOT_APPLICABLE` with a diff reason.

Use the canonical target-root policy. A preserved target directory is valid
only in an explicitly frozen rail with recorded binary/source/build identity;
never share an unqualified target across competing writers. Resolve selectors
and actual case counts before expensive execution. Do not inject ambient
`PYTHONPATH`, `PYTHONHOME`, `PYTEST_ADDOPTS` or `PYTEST_PLUGINS` into proof rails.

Stop the affected rail when source/input/binary/config/host identity changes,
ownership overlaps, a required oracle is absent, selectors execute zero cases,
or cleanup/evidence becomes partial. Keep failed epochs. Finish code and docs
before one serial qualification boundary; never edit a frozen checkout to add
a passing status. Local, installed, hosted, performance and deployment proof
remain separate even when the same receipt serves more than one ticket.
