# SEP-27 benchmark / retrieval / test-optimization — execution SSOT

Status: `ACTIVE`. Code/document re-audit: 2026-09-27 KST.
Observed source: shared dirty `main@b42a9b5d6a91e1895ad8d49367423e8c446bfed9`;
the benchmark Python overlay and concurrent Rust/proof work are not a frozen
qualification source. A commit, a file's presence or an owner-local test run
does not establish whole-source, installed-product or performance qualification.

The [code-search ticket refresh](../../sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md#current-remaining-work-audit--2026-09-27)
also observed the subsequent `a6a39cb3` commit and retained both source profiles.
It adds two reproduced acceptance failures below; historical MISC execution
counts remain scoped to their recorded inputs.

This is the single work/acceptance contract for the former SEP-27 four-agent
handoff union and RB/BM/RBR/TOPT work. Requirements, RCA, file/function owners,
logic, negative controls, dependencies and acceptance criteria are inline.
Deleted documents and external receipts are not additional task specifications.
Executable schemas, registry and tests remain code contracts, not competing
planning documents. Code paths below are repository-relative implementation
owners. Do not restore duplicate ticket bodies or redirect stubs.

## 1. Final audit: current facts and remaining work

The current audit below supersedes older snapshot-level status statements.
Historical test totals and
snapshots are not current proof and must not be added together. Implementation
presence, owner tests, integrated execution and product/performance qualification
are separate claims. The audit result is **not repository qualification**.

### Latest source audit — 2026-09-27, 17:41 KST

Audit scope: current implementation boundaries, CI selection/source custody,
raw terminal evidence and remaining acceptance. Only this SSOT is edited by
this refresh; concurrent engine, comparator and proof edits are preserved.
No runtime repair, Cargo test, installed-product run or real comparator request
was performed by this audit. The rows below are the remaining root-cause union,
not a request to reimplement already retained publication/monitor/I/O code.

| Priority / owner | Current evidence / RCA | Remaining work and acceptance |
| --- | --- | --- |
| P2 ENG-03 / INT-C1 | `just rust-policy` fails on the new `ranked_keys → ranked_page → ranked_keys` cycle after the format-8 merge. Shared column-name constants reside in the collector that consumes table types. | Move shared constants to a lower owner without changing wire/index names; execute cycle/static and ranked regressions. Detailed acceptance and source-bound command evidence are owned by [CS-INT-01](../../sep-27-code-search-remediation/rfcs/CS-INT-01-integration-and-qualification.md#current-remaining-work-audit--2026-09-27). |
| P2 BENCH-02 / native result authority | Current `product_result` accepts a cs normalized-path/hit mutation that changes score 0→1 while a fixed native response remains `other.rs`. Rejection invariant `FAILED`; no actual capture tampering is alleged. | Native-bound acquisition/replay/scoring rejection is [BENCH-02-owned](../../sep-27-code-search-remediation/rfcs/CS-BENCH-02-native-response-validation.md). Raw retention/streaming alone cannot close it; keep common custody/I/O ownership here. |
| P1 MISC-05 / regex allocation | `crates/quanta-index-lexical/src/searcher/candidates.rs::SelectedPreviewContext::prepare_leaf` reserves 16 MiB plus 256 bytes per estimated state; `crates/quanta-index-lq-regex/src/executor.rs::prepare/compile_prepared` still delegates allocating planning/compilation and caches to the public regex wrapper. The code explicitly disclaims an allocator-enforced ceiling. Capture removal and state charging are implemented mitigations, not aggregate allocation admission. | Aggregate-bound claim `BLOCKED`; coordinated implementation/qualification `NOT_RUN`. Admit AST/HIR temporaries, compilation, retained engines and search caches before their expensive allocations, with correct reservation lifetimes. Preserve one canonical truth/range matcher, Unicode/capture-heavy differential cases, cancellation and typed optional-preview refusal without changing selected IDs/order. No current runtime overrun is established by this audit; the older 20,035,301-byte compiler probe is not a measurement of today's mitigated request. |
| P1 MISC-05 / delta coverage amplification | `crates/quanta-index-lexical/src/sealed_generation/coverage.rs::apply_file_coverage` clones the base map; `write_staged_coverage` collects and serializes every row into a new CBOR buffer. `adapter_ingest.rs` invokes it for the candidate generation before index mutation. A one-file delta therefore still rewrites generation-wide coverage; text-authority touched shards are a separate cost. | Bounded incremental storage/heap claim `BLOCKED`; structural repair/qualification `NOT_RUN`. Use generation-bound immutable partitions/inheritance or bounded deltas with an explicit compaction rule, coordinated with seal/verify/open/GC rather than a second authority. Preserve empty/failed coverage, tombstones, source-event binding, old-reader ownership and corruption refusal. Measure changed, inherited, coverage and text-shard bytes separately across growing file counts; declare storage/heap ceilings and test crash/reopen. Do not exclude coverage bytes and call total incremental cost bounded. |
| P1 MISC-04 / external lexical test enrollment | New `tools/ci/tests/test_live_lexical_external.py` has two owner tests, but is absent from `Justfile::benchmark-control-contract-local`, `tools/ci/test-authority.toml` and explicit test roots in `tools/ci/source_closure.py`. The producer is already under the directory-bound `tools/benchmark/retrieval`; its test file is not. Authority lint passes because Python registration is opt-in, not exhaustive discovery. | Missing selection/custody is a current control-plane gap; repair `NOT_RUN`. Register the test to `live_lexical_external.py`, the existing PR rail and capture scope; enroll it in the existing selector and affected closures. Add omission/duplicate/nonempty-collection and test-mutation invalidation guards. Run both tests; the HTTP/process fixture test was not executed by this refresh. Keep this diagnostic producer unqualified: operator-supplied image identity/input manifest is not indexed-universe attestation. |
| P1 MISC-04/05 / final integrated source | The frozen L5 consumer source map binds `0e99f25a…`, source digest `f58a440d…`, and the removed private SSTable dependency. Its 552-case terminal and identical pre/post identities are real historical evidence. Current HEAD contains the sealed ranked-key migration; 32 entries of that local file map differ, including Cargo manifests/lockfile, lexical authority and Python comparator inputs. | Transfer of that receipt to current source is `BLOCKED`; fresh integrated execution `NOT_RUN`. Finish active owners, freeze current dependencies/config/overlay and rebuild once. Execute affected Python control-plane, bench-protocol Rust/canonical fixtures, migrated lexical/core/contract targets and required daemon/SDK/restart scopes, then actual native/Criterion capture and relocated fresh Python/Rust consumption/replay. Record exact selected/executed/ignored counts, binary/input identities and terminal artifacts. Run ignored process cases explicitly when their claim requires them. Hosted/full-workspace/installed/platform claims remain separate. Do not rerun unrelated historical targets blindly or edit old receipts. |
| P2 MISC-03 / IO-5 resource matrix | Generic 8/128 MiB publication/load/replay RSS evidence exists; it does not cover every adapter, failure log, archive cardinality or actual producer. Existing streaming code is retained. | Adapter acceptance `NOT_RUN`: whole preparation/execute/publish/load/replay with large successful and failed output, many-entry archives/metadata, interruption/corruption, an independent raw byte/digest oracle and fresh-process RSS. Declare retained-metadata limits separately from raw-byte scaling. Fix only a reproduced owner violation, not a parallel buffering API. |
| P2 MISC-06/07 / execution and measurement | Live external producer/scorer code is present, but an executable adapter, fake-service test or old pair is not a current same-corpus live experiment. Lexical file-hit/order, semantic relevance, hybrid quality and symbol spans are different strata. | Fresh admitted live pilot, profile inventory and measurements `NOT_RUN`. Use external corpus releases and the same admitted query pack; report native order, exact searchable/indexed universe, completeness, faults and raw responses. Separate cold/warm/time-to-searchable, query on/off, micro/system/test-cost denominators. Report diagnostic results without quality/performance promotion when inputs do not support it. Manual licensing/gold/host provisioning stays outside the code work list; missing inputs block only their dependent qualified claims. |

Execution dependency: structural resource owners and coverage enrollment → one
current-source integration freeze → IO-5/actual-producer acceptance → admitted
pilot/measurements. Do not run competing heavy Cargo or timing jobs against
the shared host.

Cleanup update: one-off handoff proof directories and root-level machine
reports were removed from the repository at the user's request. Historical
terminals remain historical observations, not current-source qualification.
Do not recreate receipts, snapshots, probes or proof-navigation/reissue work
merely to satisfy routine verification. The `hellgate-corrected-20260927`
directory was also removed after its six other-task runners stopped. Removed
files, including dirty contents, are recoverable outside the checkout at
`/Users/songmin/Library/Caches/quanta-index/evidence-cleanup-20260927.mUF8BM`.

Current audit evidence is external, not a second task specification:
`/private/tmp/qi-remaining-audit-20260927.9608H0/receipt.json`, SHA-256
`d6785fe216f391177f0aaf51a8252b3c9abfd25c8f54e889e7ac0d924ea2f65f`.
The driver records exact argv, raw-log digests, pre/post HEAD/dirty/file hashes,
audit Python/platform identity and the historical consumer-map comparison.
The documentation update is subsequent to that observation, not test promotion.

| Current check | Outcome and boundary |
| --- | --- |
| `python3 tools/ci/lint/check-test-authority.py` | `VERIFIED` for catalog validation, exit 0; it does not discover all unregistered Python tests. |
| `python3 tools/ci/lint/check-benchmark-policy.py` | `VERIFIED` for current policy checks, exit 0; the live external test omission is outside existing guards. |
| `python3 tools/prompt-manager/pm.py lint` | `VERIFIED` for five generated targets in sync, exit 0. |
| `git diff --check` | `VERIFIED` for observed whitespace scope, exit 0. |
| `python3 tools/ci/lint/lint-doc-paths.py` | `VERIFIED` for remaining documentation paths after cleanup, exit 0. The earlier four-path failure came from removed disposable proof documents; no proof reissue is required for this cleanup. |
| Pinned `uv run --frozen --extra dev python -m pytest tools/ci/tests/test_lexical_file_comparison.py tools/benchmark/retrieval/test_sourcegraph.py tools/ci/tests/test_live_lexical_external.py::test_cs_process_refuses_excessive_output_and_timeout -q -p no:cacheprovider --junitxml=/private/tmp/qi-remaining-audit-20260927.9608H0/diagnostics.xml` | Raw terminal: 56 passed, zero failure/error/skip, exit 0, 4.87 s; one JUnit `record_property` warning. Current-source qualification `BLOCKED`: `live_lexical_external.py` changed between the audit's pre/post observations. This is a diagnostic execution, not a frozen receipt. JUnit SHA-256 `f8e2a5ecdefd35215b6546f3369f4ce60d193754618b4eb3e7eeb5c47e792d7e`. |
| Historical frozen L5 JUnit | 552 executed, zero failure/error/skip; raw JUnit SHA-256 `657634687041d2493f20a2a1c87017e2f244034e91b75eefe860a560eee25c64`, pre/post source identities equal. Receipt transfer to current source `BLOCKED`; this audit does not requalify that older environment or removed dependency. |

### Retained implementation and historical owner evidence

| Boundary | Current source fact | Remaining action / claim status |
| --- | --- | --- |
| MISC-01 publication/GC | `profile_capture.publish_capture` is the shared complete-profile owner; `commit_capture` is the pointer commit; Rust `RunStore::collect` takes custody before checking the capture marker. | Retain implementation. Actual producers, both consumers and fresh final-source replay: `NOT_RUN` in this audit, owned once by MISC-04. |
| MISC-02 / EXEC-1 | Native/Criterion capture now start a cooperative monitor at the first owned producer, carry its FD through the process-group guard, and bind a validated raw transcript before publication. Native command-only mode also uses owned execution, without a capture pointer. Detached replay rederives host facts; standalone artifact import is `none/0`. | Owner-local positive/negative, parent-death nested-child lease, join-timeout and primary-plus-monitor-error tests ran; eight capture modules passed 379/379 under a changing Rust preflight source. Actual native/Criterion producers and qualified quiet-host acceptance remain `NOT_RUN`. Cooperative observations are diagnostic, not host isolation. |
| MISC-03 / IO-1–3/5 | Shared `RawFile`/`RawWriter`, process log owner, bounded controls/JSONL, streamed archives, portable receipts/replay and paired command ZIP are present. | Generic full publication/load/replay RSS probe passed for 8/128 MiB raw; adapter-specific large failure output, many-entry archive and actual producer resource acceptance remain `NOT_RUN`. No blanket whole-repository bounded-memory claim. |
| MISC-03 / IO-4 | The epoch spans native plus five adapters. Nested nonzero/exception/refusal is sticky; source/replay callbacks cannot publish after recording failure. Native primary refusal and fresh-replay boundaries remain covered. | Post-fix eight-module selection: 379/379 passed, exit 0. `symbols/preflight.rs` changed during execution, so this is owner regression evidence, not frozen-source pair/product qualification. |
| MISC-04 / C5 | Both Python modules are selected by the existing Just command, registered to exact owner/scope/PR rail and affected closures, and guarded for collection and mutation. | Focused C5 guards 9/9; seven related owner modules 344/344; actual `benchmark-control-contract-local` selector 1256/1256. Shared checkout/closure was not frozen; this is not hosted CI or product qualification. |
| MISC-05 | Existing TOPT/retrieval invariants are qualification obligations, not presumed new bugs. Regex aggregate allocation and delta coverage amplification remain the two explicit structural resource gaps above. | Retain verified historical selected Rust/frozen Python evidence in its original scope; do not transfer it across the current ranked-key/dependency migration. Fresh final-source/full/installed/platform scopes: `NOT_RUN` here. |
| MISC-06/07 | Timing, product pilot, independent quality and qualified pair each have distinct prerequisites and denominators. | No fresh measurement/pair executed here: `NOT_RUN`. Missing admitted inputs block only their dependent claims. |

### IO-4 repaired findings and regression obligations

- Owner: `tools/ci/tests/test_benchctl.py::test_native_capture_keeps_real_failed_producer_logs_and_prior_pointer`.
- RCA: the new test was inserted before the previous parameterized
  `test_promotion_is_scoped_to_the_profile_families` test's GC/replay/validation
  tail. That tail now executes in the new function, where `runs`,
  `profile_name`, `manifest` and `source` are not defined.
- Observed terminal: `NameError: name 'runs' is not defined` at line 1416
  of the audited source. The exit-7 raw log, failure record and previous-pointer
  assertions execute before the error; the replay/validation tail does not.
  This is a broken test and lost positive coverage, not evidence of a product
  replay failure.
- Repair under MISC-03/IO-4 with MISC-04 coverage ownership: the original
  GC + per-run fresh CLI replay + `validate_promoted_runs` assertions have been
  restored to the parameterized positive test. The negative producer
  test remains separate, with retained-failure/GC assertions. No assertions
  were removed, no globals added, and fresh replay remains a real subprocess.
- Acceptance: both parameter values (dsl-authority/systems), the negative test,
  complete profile-capture suite and all affected adapter suites pass with
  exact nonzero collection and unchanged selected source bytes.

Pre-repair diagnostic command (historical RED, not current outcome):

```sh
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_benchctl.py::test_native_capture_keeps_real_failed_producer_logs_and_prior_pointer tools/ci/tests/test_benchmark_profile_capture.py -q -p no:cacheprovider --junitxml=/private/tmp/qi-misc-doc-final.eJl5QU/capture-audit.xml
```

Pre-repair terminal: 38 cases, 37 passed, 1 failed, no skips/errors, exit 1, 2.32 seconds.
This is a shared-checkout diagnostic reproduction, not a frozen-source
qualification receipt. JUnit and the documentation audit artifacts are external
evidence only. No Rust, actual SDK/daemon producer, hosted CI, installed product,
quiet-host measurement or full benchmark suite was executed by this refresh.

Additional owner counterexample: a nested entrypoint could return nonzero (or
record a primary refusal) and be ignored by its caller before publication. Both
variants published in the pre-fix regression (`nested-red.xml`, two failures).
The same existing epoch now records nested returns/exceptions, requires the
same repo/root/profile, and refuses further phase/execution/publication after
failure. Source and domain-replay callbacks are checked before progressing to
the pointer commit. This is a demonstrated owner invariant failure, not a claim
that an actual product benchmark had silently published this way.

The current tests also cover initial journal failure, primary plus secondary
errors, duplicate failure-record refusal, real spawn/nonzero/timeout/SIGTERM,
lexical overlap refusal before writes, second-family failure, final-source and
post-pointer/returned-commit failures, plus abrupt process death preserving an
active journal and only a complete old/new pointer. CLI refusal paths preserve
their observed reason/status without inventing a producer terminal. All use
the existing publication/execution owners; no second transaction was added.

### Historical regression results

Owner-local runs demonstrated the IO-4 and recorded-import repairs on their
recorded older source. Keep the implementation and tests; do not transfer old
passes to the current source or rebuild a per-run evidence tree. Relevant
current checks can be reported from their terminal output under the relaxed
verification contract. Full Rust, actual producer/consumer and product claims
still require the checks appropriate to their scope.

### Retained decisions: do not reopen without a reproduced regression

- One Python orchestration CLI, one typed evidence contract, one publication
  owner; no parallel Rust CLI, layout rewrite or bytes-mode compatibility API.
- Exact required test identities derive from live collection, never a permanent
  historical count such as 324. Keep the inventory guard and required manifest
  synchronized.
- Preserve SIGCHLD-driven producer completion, bootstrap caching and diagnostic
  command timings. Neither caching nor a timing record proves a speedup.
- Preserve no-follow/linked-parent/hardlink/exclusive-stage protections,
  query-observation parity, macOS resource validation and the 18 TOPT invariants.
- No ranking/default/fetch-floor change, symbol-authority expansion, implicit
  downloads or generated independent gold is authorized by this packet.
- Shared dirty implementation is permitted; preserve other writers' work.
  Final proof requires frozen source/input/environment, not a fabricated clean
  receipt. Finish normative edits before freezing.

## 2. Architecture and common contracts

```text
benchctl + registry: select declared profile and route commands
  adapter: prepare inputs, run producers, invoke existing domain validator/scorer
    producer_execution: process/session/terminal/log/cleanup custody
    host_monitor (present, NOT wired): capture interval observations and reservation
  profile_capture: admitted capture epoch + complete-profile publication owner
    evidence_bridge: typed run construction, not metric-policy authority
    RunStore: safe raw files and immutable runs
    custody: publication versus destructive GC exclusion
```

- `tools/benchmark/registry.toml` is the only registration authority;
  `registry.py` validates it and `manifest.py` is a read-only projection.
- `tools/benchmark/benchctl.py` is the only current orchestration CLI. Just,
  Cargo and Python producers and domain comparators remain their existing
  owners. Do not recompute domain verdicts in the common wrapper.
- `benchmarks/bench-protocol` owns typed evidence and independent Rust
  validation; Python emits identical canonical bytes. Product crates must not
  acquire normal dependencies on benchmark-only packages.
- Keep one current internal API at each boundary. A coordinated API cutover
  updates all live callers; no versioned twins, fallback readers or repair
  wrappers. Preserve strict persisted-artifact identities and explicit refusal.
- Existing prepared-run dictionaries carry family/case/source/input/build/host/
  command/payload/raw references; `ExecutionResult` carries actual terminal state
  and bounded file-backed stdout/stderr references. No new `PreparedRun` type or
  parallel representation is required merely to complete these tickets.
- A run is one case. A capture is the exact declared family/case inventory
  from one execution. Only the profile pointer is the profile commit point.
  `latest` remains advisory; baselines use explicit admitted ID and digest.
- Profile/input/source/registry identity is fixed before execution and checked
  before commit. Same-source runs alone do not prove the same capture.
- Host reservation and evidence-store custody are different locks. Do not hold
  store custody across long producer execution. Work/raw outputs stay external.
- Corruption, unknown fields, missing inputs, duplicates, stale identities,
  reordered records, partial/timeout/interrupted output and forged digests
  must fail closed. Missing is never zero, empty, skipped-pass or success.
- Local no-follow/custody and process diagnostics are not hostile same-UID or
  remote attestation. Do not expand the security claim beyond the checked
  filesystem/process model.

## 3. Ticket map, file ownership and execution order

| Ticket | Single owner boundary | Work to perform next |
| --- | --- | --- |
| MISC-01 | `profile_capture.py`, `custody.py`, Rust run store | Retain atomic inventory/pointer/GC implementation; final integrated execution is MISC-04, not another gate. |
| MISC-02 | `benchctl.py`, `producer_execution.py`, `host_monitor.py`, `evidence_bridge.py` | Retain implemented EXEC-1 and parent-death/join-failure regressions; remaining actual-producer acceptance is owned once by MISC-04. |
| MISC-03 | `evidence.py`, `raw_archive.py`, capture adapters, portable proof | Retain repaired IO-4 and IO-1–3 regressions; execute IO-5 whole-capture resource proof. |
| MISC-04 | Shared CLI/bridge, Just, test authority, source closures | Retain implemented C5; enroll the new external lexical owner test and its regression guards, then integrate producer/consumer/replay evidence. Routine checks do not require one-off proof files. |
| MISC-05 | Runtime/SDK/core/platform owner tests; canonical regex/resource and sealed coverage owners | Resolve aggregate regex allocation and delta coverage amplification without parallel authority; then qualify retained invariants, full Rust/daemon, installed ingest and supported platforms. |
| MISC-06 | Existing test/query/ingest/micro/system measurement owners | Measure distinct workloads only after correctness, host and input admission. |
| MISC-07 | Registry/domain adapters/retrieval evaluator | Complete real profile execution inventory, live comparator pilot and separately admitted quality/performance claims. |

Order: retain IO-4/EXEC-1/C5 owner implementations → close their missing
negative controls → IO-5 → finish docs and freeze once →
serial MISC-04/05 runtime batch → admitted MISC-06/07 measurements.
One integrator owns overlapping `benchctl.py`, `evidence_bridge.py`, selectors
and source-closure edits. Do not run competing heavy gates or duplicate one
capture because it satisfies multiple tickets.

### Deduplicated handoff union

| Former identity | Sole current disposition |
| --- | --- |
| A1/A4 G-01; A4-01–04; A3-00/01/07 | Historical snapshots do not transfer. Current integrated source, exact inventory, consumers/replay/POST and hosted reconciliation belong to MISC-04. No wholesale old-branch import. |
| A3-02 | Preserve existing store defenses; publication in MISC-01, raw/archive/failure custody in MISC-03. |
| A3-03/05/06; G-03; A4-08 | Execution inventory, live pilot, authenticated outcome and separately qualified pair belong to MISC-07. |
| A3-04; A4-05/G-02 | Process/host construction is MISC-02; micro/system/query measurement is MISC-06. |
| A2-01/02; A4-06/G-05; A4-07/G-06 | Retained owner invariants, full Rust, installed ingest and platform proof are MISC-05; timing is MISC-06. |
| A2-03/05 | Test-cost measurement is MISC-06; final receipt reconciliation is MISC-04. |
| A2-04; A3-08 | Historical admission choice, licensed/adjudicated assets and host access are user-owned prerequisites, not coding steps. |
| G-04; Windows/release expansion | Excluded without separate authorization. |

### IO-4: retained epoch and failure acceptance contract

Current owner is `tools/benchmark/profile_capture.py::CaptureEpoch` plus
`capture_entrypoint/publish_capture`. The native entry is
`benchctl._capture_native_run`; nested `promote_profile_runs` reuses its epoch.
Criterion, retrieval-contract and recorded captures enter directly; pair and
lexical first prove safe routing/root disjointness, then enter
`_capture_admitted`. Unsafe namespace/routing rejection intentionally performs
no epoch write. It is not covered by a claim of durable post-admission failure
records. Source/input acquisition after admission is covered.

Retain and verify:

1. One ID owns preparation, source checks, producer executions, staging,
   promotion, domain replay, final source check and pointer commit.
   `work/<capture-id>/capture.json` is mutable diagnostics;
   `failures/<capture-id>.json` is exclusive bounded failure custody.
   Neither belongs in immutable raw/success inventory.
2. Record only observed source/input/log/terminal facts. Execution attempts
   bind the real `execution.json` digest when present; missing terminal stays
   unavailable. `prepared_command` is claimed prepared metadata, not proof
   that a process ran. Never synthesize exit 0 for a failure.
3. Before pointer commit, failure preserves the previous complete pointer.
   After commit, a response/final-source failure retains attempted/returned
   commit state and must resolve the exact current pointer; do not roll back
   immutable history or label a failed response uncommitted.
4. CLI `return 2` and nonzero subcommand returns must preserve phase and
   primary reason, not only exceptions. Native preflight/comparison/refusal
   paths now preserve the actual refusal using `refuse_capture`; retain this.
   Nested failure is sticky across all entrypoints. Preserve
   primary plus cleanup/marker-persistence errors. If the failure record cannot
   persist, explicitly report `NOT_PERSISTED`; no durable-evidence claim.
5. Retain targeted tests for initial journal-write/disk-full failure,
   source/input failure, real spawn/nonzero/timeout/interrupt, second-family
   promotion, domain replay, final/post-commit source failure, marker persistence
   failure, nested-context mismatch/reset and success-without-commit refusal.
   Retain lexical overlap/no-write coverage matching pair's admission boundary.
   Changes to this boundary require rerunning the affected owner selection.
6. Abrupt process death cannot execute Python exception cleanup. Preserve
   the last journal and pointer semantics; do not promise a finalized failure
   marker after SIGKILL. Test recovery classification separately.
7. Retain the restored positive native replay tail and its parameterized profiles.
   Confirm explicit run GC does not delete diagnostic failure
   custody, and failure markers are never admissible runs.

Regression owners under `tools/ci/tests/`:
`test_benchmark_profile_capture.py`, `test_benchctl.py`,
`test_benchmark_evidence_bridge.py`, `test_recorded_capture.py`,
`test_retrieval_capture.py`, `test_criterion_capture.py`,
`test_pair_capture.py`, `test_lexical_capture.py`.

### IO-1–3 retained APIs and hard limits

Do not replace the implemented shared file contract with payload bytes.

| Owner/caller | Required retained behavior |
| --- | --- |
| `evidence.RawFile/RawWriter` | 64 KiB streaming copy/hash; pinned descriptor, exact count/SHA, no-follow ancestors and before/after inode/mode/link/change epoch. Control JSON at most 16 MiB. `consume_lines` at most 16 MiB per line; full input consumption, including valid final JSON without LF. |
| `producer_execution.execute/_wait_for_terminal/_cleanup` | File-backed stdout/stderr and cleanup drains, bounded tails, actual terminal/reap/lifeline ownership. Timeout/interruption/parent death/partial cleanup never means success. |
| `raw_archive.py`; pair/corpus callers | Deterministic sorted regular ZIP entries and metadata; complete-byte hash before/after seek, streamed extraction. Refuse duplicates/aliases/traversal/links/encryption/compression/corruption/reorder/undeclared entries. Bound central metadata before ZIP allocation; support ZIP64 without an unbounded preparse. |
| `pair_capture.pack_native/tree_files/unpack_native/_ReplayWorkspace` | 64 GiB archive ceiling, 100,000 entries/discovery fanout, 16 MiB central directory. Rehash bundle/ZIP every case; no mtime-only cache. Retain exact tree/mode/binary and cross-case source identity. |
| `corpus_binding.capture/replay` | 256 MiB capsule limit, exact metadata/bundle inventory and real Git reconstruction. Source/output roots remain disjoint and external. |
| Recorded/Criterion | Agent JSONL discards validated trajectories but retains exact A/B/C pair metadata with separate 16 MiB serialized budget. Cargo JSONL has exact artifact/features and one successful final terminal. Duplicate/partial/invalid/nonfinite rows refuse. Recorded imports remain unauthenticated. |
| Native/lexical | Native finite summary fanout (concurrency 1/8/32); bounded controls; streamed observation rows and exact raw commitments. Lexical retained metric metadata has a 16 MiB serialized budget, not a 16 MiB RSS claim. Exclude mutable journal/execution directories from native raw inventory. |
| `retrieval/portable_proof.py::_run/_produce/validate/_canonical_receipt` and contract/SDK/inventory readers | File-returning producer API, bounded metadata/collection controls, streamed nextest/events/hash, exact execution/reuse identity, final raw/binary/source checks and relocated replay. No `dict[str, bytes]` payload cache or hidden full-log decode. |
| `retrieval/run.py::freeze_receipts/_verify_execution_context` | At most 64 MiB aggregate command payload plus 64 KiB ZIP envelope; 32 KiB directory, exact expected entry names/counts. Keep binary/collection roles, same-raw metrics, final binary/context/closure rehash. Noncanonical archives require re-freeze, not a legacy parser. |
| Runner bundle in `retrieval/run.py` | Shared archive owner, 16 MiB envelope, 16 KiB directory, exact member inventory and fixed bootstrap bytes. Rehashing a forged manifest is not admission. |

Exact regression owners also include `test_bench_protocol_conformance.py`,
`test_pair_replay_workspace.py`, `test_corpus_binding.py`,
`test_agent_outcome_benchmark.py`, `test_lexical_file_comparison.py`,
`test_portable_proof.py`, `test_cargo_preparation.py`,
`test_proof_command_timings.py`, `test_portable_tool_execution.py`,
`test_retrieval_contract_proof.py`, `test_retrieval_sdk_proof.py`,
`test_write_verification_receipt.py` and `test_retrieval_benchmark.py`
under `tools/ci/tests/`. Exact selection/authority/closure must be checked;
a similar filename is not coverage.

IO-5 generic capture proof now runs `test_complete_publication_and_reload_peak_rss_is_payload_independent`
in fresh subprocesses on 8 MiB and 128 MiB sparse zero inputs. It rederives
the exact SHA-256 and byte count at the source, promoted raw and replay reader,
then reloads the complete profile. On macOS the child peak RSS values were
28,229,632 and 27,951,104 bytes respectively (threshold: less than 48 MiB
growth for 120 MiB more input). Focused case: 1/1 in 3.31 seconds; external
JUnit `/private/tmp/qi-misc-io5-generic-20260927.xml`, SHA-256
`03b233dc136601ae04e6296b52d7f02f56559f7c993104a219ab62a67da29faa`.
Profile-capture, policy and source-closure owner modules: 145/145 in 71.19
seconds; JUnit `/private/tmp/qi-misc-io5-owner-20260927.xml`, SHA-256
`eb28e6072a1b29a2a9e73d7797a1ff61fba9c0ff8e680dbcaa43b2ea6aae37c6`.
This test was added after the 1256-case Just selector run; that older total is
not a current exact collection count. Large failure output, many-file metadata,
adapter-specific whole-capture paths and actual-producer resource measurements
still need separate fresh-process proof. Neither component tests nor this one
generic capture probe establishes a performance improvement.

### Superseded-document deletion boundary

The earlier consolidation removed 41 bodies: four SEP-27 agent handoffs,
11 RB records, 11 BM records, three RBR records and 12 TOPT records.
This audit rechecked that no Markdown bodies remain in:
`docs/handoff/sep-27`, `docs/plans/sep-23-retrieval-bench`,
`docs/plans/sep-26-bench-migration`, `docs/plans/sep-26-retrieval-remediation`,
`tickets/sep-22-test-optimization`. No additional stale body was found there.

Recovery-only archive (not a requirement/reference dependency):
`/Users/songmin/Documents/qi-docs-ssot-backup-sep27.bubnQ2/before-consolidation.tar.gz`,
SHA-256 `8c581c274d7c6f302d0726982a4471427d7bb957fe84042c4c54b8d4f4e39d81`,
rechecked in this audit. This SSOT's pre-refresh body is backed up externally
at `/private/tmp/qi-misc-doc-final.eJl5QU/INDEX.before.md`.
Recover into a separate directory, never over live work.
Accepted architecture decisions, unrelated plans and the concurrently active
code-search-remediation packet are not superseded execution documents and are
preserved. This refresh adds no second ticket/handoff authority.

## 4. MISC-01 — atomic complete-profile publication

Implementation is present in the current source. The changes and invariants
below describe what must be retained, not five remaining implementation tasks.
Remaining final-source actual-producer acceptance is owned once by MISC-04.

### RCA and file-level changes

Pre-patch `tools/benchmark/benchctl.py::promote_profile_runs` bypassed the complete-capture
owner; `validate_promoted_runs` scanned newest family runs. Existing
`tools/benchmark/profile_capture.py::commit_capture/load_capture` already has
the exact-inventory and atomic-pointer abstraction.

1. Extend the existing profile-capture module with one publication entrypoint
   accepting prepared runs, capture ID, profile, registry digest and expected
   inventory. Do not create another transaction module.
2. In `benchctl.py`, prepare every family and invoke that entrypoint. Derive
   expectations from registry plus independent producer listing, not observed
   successful output. Current native aggregate families use `case_id=None`;
   Criterion case IDs come from the binary listing.
3. Under `tools/benchmark/custody.py::custody`, establish the capture-store
   marker before promoting any run, promote immutable runs, verify exact
   inventory/source/registry, then use `commit_capture` to publish once.
   The marker preserves the Rust collector's refusal of orchestration-owned
   capture stores; both Python and Rust GC must acquire the same custody lock
   before observing any references or the marker, not only refuse an already
   visible marker. The Rust pre-marker interleaving is a retained regression.
4. Validate one `load_capture` result. Retain source, lockfile, host-policy,
   preflight, payload-kind and raw-derived domain checks. Do not weaken them
   when removing newest-family selection. Missing pointer means no complete
   capture; do not synthesize one by scanning runs.
5. Move repeated publication orchestration from capture adapters into this
   owner only where it is genuinely shared. Keep domain preparation/replay
   in the adapters. `evidence_bridge.py::promote_native_run` remains one-run
   materialization, not another profile transaction.

### Commit and failure semantics

```text
PREPARED -> PRODUCERS_COMPLETE -> RAW_VALIDATED
  -> [custody: RUNS_STORED -> CAPTURE_STORED -> PROFILE_POINTER_COMMITTED]
  -> fresh-process validation/replay
```

- Before pointer commit, any failure preserves the prior complete pointer.
  Orphan immutable runs are not a completed profile. Preserve failed epochs;
  subsequent explicit GC may remove only unreferenced data.
- Crash after capture creation but before pointer replacement must not make a
  partial profile current. A valid immutable capture may remain retained.
- Crash/response loss after pointer replacement is an ambiguous response, not
  permission to rewrite history. Resolve the exact capture ID and digest.
- GC validates all custody references before deletion. Malformed references
  stop collection. Never delete unknown references to recover apparent success.
- Do not add advisory `latest` rollback or use `latest` as a baseline.

### Acceptance and tests

Owners: `tools/ci/tests/test_benchctl.py`,
`test_benchmark_profile_capture.py`, `test_benchmark_evidence_bridge.py` and
`test_bench_protocol_conformance.py` under `tools/ci/tests/`.

- Positive multi-family profile roundtrip and fresh replay.
- Failure on second family, missing/extra/duplicate case, wrong profile,
  registry drift, same-source mixed capture and payload/preflight mismatch.
- Cross-process publication/GC interleaving, not only a same-thread mock.
- Failure/crash before and after each commit boundary; old complete pointer
  remains usable or exact new complete pointer is recoverable.
- Existing symlink/linked-parent/hardlink/staging refusals remain intact.
- DoD: the current C1 counterexample no longer loses a complete native profile
  to GC; complete publication semantics hold for every adapter using the owner.

## 5. MISC-02 — producer and host lifecycle

### RCA and owners

- `tools/benchmark/benchctl.py`: immutable and no-evidence-root native producers
  now use the same owned execution path with external diagnostic log custody.
  Preserve its command
  semantics, including baseline admission; command diagnostics are not an
  immutable profile capture and must not invent a capture pointer. Retain the
  existing refusal of incompatible baseline/evidence-root options. Standalone
  artifact import emits `none/0`; monitored runs derive the lease from raw.
  Do not replace unrelated short Git/toolchain probes.
- `tools/benchmark/producer_execution.py`: retain private session, parent
  lifeline, actual terminal record, unreaped group identity, bounded cleanup
  and signal handling. Exit zero alone does not establish descendants exited.
- `tools/benchmark/criterion_capture.py`: bind observations to observed build,
  listing, smoke and measure command boundaries. Criterion warmup occurs inside
  its measure process; do not invent a separately observed warmup interval.
- Existing `tools/benchmark/host_monitor.py`: owns capture-time observations and
  cooperative local reservation; `evidence_bridge.py` derives typed host facts
  from the validated observations instead of caller-supplied summary constants.
- `tools/ci/source_closure.py`: enroll new module/tests in the affected closures.

### Final implementation split: one lifecycle, no patch-on-patch adapter

The rows below distinguish implemented code from unproved acceptance. They
share one EXEC-1 acceptance boundary; do not introduce another transaction,
success marker, producer runner or permissive replay parser.

| Step | Current implementation | Remaining acceptance / refusal boundary |
| --- | --- | --- |
| E1: descriptor custody | Public `execute` forwards `custody_fds` to the existing private guard; the producer child does not inherit them. Direct FD non-inheritance and controller-death/nested-child reservation tests passed. | This proves only the participating local lock domain; it is not quiet-host or hostile-process isolation. |
| E2: epoch lifecycle | Native/Criterion entrypoints explicitly monitor. The epoch starts at first owned producer, marks phases and finalizes before publication. Monitor failure is sticky and retains diagnostics. Injected primary-plus-monitor-error proof passed. | A failure during a long-running producer is detected at the next phase/finalization, not immediate cancellation. |
| E3: transcript custody | Validator now requires a singly linked regular lock identity and strict sequence/time/facts. `finish` refuses join timeout while retaining the live observer/FD; it cannot report successful release. Malformed/partial raw, observation-failure and injected join-timeout/cleanup tests pass. | Replay validates a transcript, not a live lock. |
| E4: canonical host derivation | `host_from_observations` derives identity/count from validated raw; publication binds its digest, capture/profile and input. Standalone import emits `none/0`; required monitored boundaries without raw refuse. | Keep diagnostic scope; actual producer proof and performance admission are separate. |
| E5: all readers together | Publication, capture loading, native replay and Criterion replay rederive the reserved host raw. Native artifact parsing excludes only that reserved name. Replay validates family policy for actual captures; recorded imports retain explicit `any` importer policy. | Eight-module owner selection passed 379/379 with concurrent Rust source drift; fresh-process actual-producer replay remains `NOT_RUN`, and cooperative observations never yield performance verdicts. |
| E6: remaining dispatch | Command-only native producer uses `producer_execution.execute` with external diagnostic logs, retains a real nonzero exit and does not publish a pointer. Diagnostic-root allocation failure returns refusal. | Actual command-only baseline admission beyond mocked dispatch is `NOT_RUN`; unrelated probes stay unchanged. |
| E7: authority and proof | Monitor is enrolled in benchmark-control and derived closures; existing capture/bridge/criterion/producer tests cover it. Policy/closure guard cases and the seven-module owner selection passed. | Full final-source gate, actual native/Criterion execution and relocated fresh replay remain `NOT_RUN`. |

Lock separation is intentional: `tools/ci/resource_admission.py` guards leaf
build/test admission. Do not hold that same lock across a native recipe whose
inner build reacquires it. Reuse its checked-file mechanics, not its lease
scope. The draft host lock is per-user in the OS temporary directory; another
user, a different temporary namespace or a nonparticipating process is not
excluded. Assert only that cooperative domain and require distinct qualified
host admission for stronger isolation claims.

Monitor negative tests must use independently constructed transcripts and
expected counts, not only roundtrip through the same producer. Real subprocess
tests cover contention, parent death and bounded cleanup; mocks cover faults
but do not establish absence of surviving children. Preserve original raw
logs when observation or transcript sealing fails. No ambient argv/environment
secrets may enter observation records.

### Logic

Build/provision outside timed measurement where the protocol permits; bind
binary bytes. Reserve host, record start, record periodic samples with sequence
and monotonic time, execute declared phases, clean up producer resources,
record end and release reservation. Preserve actual phase boundaries. Host
monitoring does not silently redefine cold/warm or correctness boundaries.

Observations bind capture ID, host identity, reservation identity, start/end,
sequence, maximum unobserved gap, clock discontinuity and interference facts.
Hash process commands where needed; do not dump secrets from ambient command
lines or environments. Unknown observations fail, not default to a quiet host.

Timeout, interrupt, spawn failure, malformed terminal or monitor failure leaves
explicit failure and retained raw logs. Preserve primary and cleanup errors.
Do not report completion while pipe drain/reap is incomplete. No broad process
kills, foreign Cargo termination or PID-name ownership heuristics.

A cooperative lock excludes participating controllers only. It cannot prove
thermal/power/scheduler isolation or external process absence. Its successful
capture is diagnostic; qualified performance requires section 10 host admission.
If performance was requested and prerequisites fail, refuse that claim; never
silently downgrade the request and return performance success.

Implement against the current owners and the inline contract above. No isolated
checkout or deleted design note is required. Reuse an existing implementation
only after exact behavioral/hunk comparison; do not replace stronger current
retrieval, query-parity, macOS resource or test-authority behavior.

### Acceptance

- Real timeout/interrupt/parent-death/nested-child tests; no surviving owned
  processes; finite cleanup; malformed or absent terminal cannot pass.
- Monitor tests: missing/reordered/duplicate samples, missing end, stale
  capture, changed reservation, excessive gap, clock discontinuity,
  observation failure and forged host-envelope fields are rejected.
- An otherwise valid cooperative observation cannot issue a performance verdict.
- Correctness on/off observation parity is preserved; monitor/log overhead is
  measured separately, never subtracted from one product without disclosure.
- Existing execution tests in `tools/ci/tests/test_criterion_capture.py` remain
  effective. A dedicated execution-test module may own them after a single
  relocation with exact test-authority/selector updates, not duplicate coverage.

## 6. MISC-03 — bounded evidence, archive and log I/O

### RCA and files

| Owner | Current disposition / required change |
| --- | --- |
| `tools/benchmark/evidence.py::_consume_regular_file/_verify_raw` | Implemented; retain bounded digest/count and no-follow descriptor/epoch guard. Small control documents have an explicit 16 MiB limit. `_read_regular_file` remains a materializing API: large-payload callers still require migration, not a blanket success claim. |
| `tools/ci/lint/handoff_validation.py::_consume_repo_regular_file` | Existing complete-read primitive reused; retain complete-consumption and identity semantics, including through archive seeking. |
| `tools/benchmark/evidence.py::RawFile/write_raw_file/StagingRun.write_raw` | Implemented; retain exclusive creation, incremental digest/count, flush/fsync, safe parent descriptors and explicit failure. No second bytes API. |
| `tools/benchmark/evidence_bridge.py::promote_native_run` | Implemented file-backed inventory and retained failed staging; capture-wide custody is implemented but its remaining acceptance is IO-4. |
| `tools/benchmark/evidence_bridge.py::sha256_file/sha256_hex_file` | Implemented bounded hashing through the same pinned reader; preserve tests. |
| `tools/benchmark/raw_archive.py`, `pair_capture.py::pack_native/unpack_native/tree_files/capture/replay_run` | IO-2 implemented: shared streamed ZIP mechanics, fixed metadata, bounded directory parsing, commitment-only replay cache and streamed workspace/binary hashes. Retain the exact archive/refusal regressions in section 3. |
| `tools/benchmark/corpus_binding.py::capture/replay/_replay` | IO-2 implemented: same file-backed archive owner; retain capsule size ceiling, lexical input digests and exact release/bundle reconstruction. |
| `tools/benchmark/producer_execution.py::_wait_for_terminal/execute/_cleanup` | IO-1 implemented; retain file-backed normal/failure drains and bounded tails. Native immutable and command-only dispatch use this owner; actual-producer/host qualification is still separate. |
| `tools/benchmark/evidence.py::RawFile.consume_lines`, recorded/Criterion/native/lexical/portable capture and domain readers | IO-3 implemented: bounded JSONL/retained metadata/control JSON, file-backed preparation, canonical receipt production/replay and paired-verdict command-log ZIP. Generic complete-profile RSS now has one measured owner proof; adapter-specific RSS remains IO-5. |
| Adapter preparation/execution and `profile_capture.py::CaptureEpoch/publish_capture` | IO-4 owner implementation and misplaced native test tail are repaired. Retain the phase/failure matrix, 344-case selected owner pass and 379-case eight-module post-fix pass. The latter crossed a concurrent Rust source change; final qualification remains MISC-04. |

Coordinated callers: `criterion_capture.py`, `retrieval_capture.py`,
`lexical_capture.py`, `pair_capture.py`, `recorded_capture.py`, `corpus_release.py`, `corpus_binding.py`
under `tools/benchmark/`, and `tools/benchmark/retrieval/portable_proof.py`.
Update every affected read/parse/replay path in the same API cutover. Stream
Cargo JSONL and other unbounded line protocols; bounded control-output reads
must reject oversize, not truncate into valid-looking input.

### Invariants

- Source and destination roots remain disjoint; corpora/models/indexes/results
  stay outside Git. No implicit temp archive in the checkout.
- Pin regular-file descriptors with no-follow ancestor traversal and verify
  size, inode/device/mode and change epoch before/after consumption. Never
  replace this with a check-then-open pathname or only an mtime check.
- Streaming digest binds the exact copied bytes and count. Seek-based ZIP
  replay must preserve pinned-descriptor identity and complete hash accounting;
  do not disable the helper's complete-read check to accommodate seeking.
- Deterministic ZIP: sorted unique canonical names, fixed metadata policy,
  regular entries and declared compression; reject traversal, links, encryption,
  duplicate aliases, corrupt/truncated entries and undeclared inventory.
- Bound archive entry count, total bytes and control-document sizes explicitly;
  record/refuse exceeded limits. Metadata cost may scale with file count but
  payload memory must not scale with total raw bytes.
- Partial writes and disk-full preserve a failed epoch and never publish a
  complete profile. Do not silently retry against a different input or location.
  The bridge now retains failed staging and rejects its promotion. The execution
  API retains per-command failed outputs; the capture epoch now spans admitted
  preparation/replay. Verify all phase boundaries and persist/refuse semantics;
  neither a staging marker nor a command record alone closes IO-4.
- Do not introduce CAS/deduplication or alter corpus-retention policy merely to
  fix streaming. Measure disk/RSS first; shared-blob retention would require
  independently tested references/GC and a separately justified change.

### Acceptance

Owners: `tools/ci/tests/test_bench_protocol_conformance.py`,
`test_benchmark_evidence_bridge.py`, `test_pair_capture.py`,
`test_criterion_capture.py` and affected adapter/portable-proof tests.

1. A deterministic reader oracle rejects unbounded `read()`/whole-buffer use;
   verify maximum chunk size, full digest and exact byte count.
2. Growth, truncation, same-bytes link swap, restored mtime, parent replacement,
   duplicate staging, disk-full and interrupted output cannot become success.
3. Large raw, large entry and many-entry archive roundtrip; byte/case identity
   and domain verdict match the fixed independent oracle.
4. Actual subprocess peak-RSS measurements at increasing input sizes support
   the claimed bound. Declare platform/sampling tolerance; no OOM claim from
   code shape alone and no bound claim from a mocked read-size test alone.
5. Python/Rust canonical evidence fixtures stay equal. No wire-schema expansion
   is needed for file-backed internal I/O; if one becomes necessary, stop and
   revise producers/readers/goldens together rather than add compatibility twins.

## 7. MISC-04 — serial integration and source-bound closeout

- C5 code/control-plane wiring is implemented: the existing Just command and
  affected closures select `test_pair_replay_workspace.py` and
  `test_cargo_preparation.py`; `python_targets`/`benchmark-control-capture`
  bind them to `tools/benchmark/pair_capture.py` and
  `tools/benchmark/retrieval/portable_proof.py` on the PR rail. Policy and
  source-closure tests guard exact owner/scope, live nonempty collection,
  omission and mutation invalidation. The focused C5 guard selection passed
  9/9 and the actual Just selector passed 1256/1256. Do not substitute these
  local results for final hosted CI execution or a fixed case count.
- C4 is implemented; preserve its regression guards before final qualification.
  `Justfile::benchmark-control-contract-local` now selects
  `tools/ci/tests/test_producer_notifications.py`,
  `test_bootstrap_cache.py` and `test_proof_command_timings.py`; keep these owners
  to that existing command and to `tools/ci/test-authority.toml`'s applicable
  Python targets/scope. `.github/workflows/ci.yml` already invokes the command;
  do not create a competing CI test list or a second job just for these modules.
  Enroll all three in the benchmark-control-plane closure and the evaluator/
  portable-proof tests in the retrieval closure in `tools/ci/source_closure.py`.
  Derived benchmark-micro/benchmark-retrieval closures inherit the right union.
  Add exact selector/closure regression checks in the existing policy/source-
  closure tests: removing a required module, duplicating it, selecting zero cases
  or changing a bound module must not pass. Derive case identities from actual
  collection; do not solve this by adding a permanent numerical test count.
- One integration owner controls `benchctl.py`, `evidence_bridge.py`, shared
  internal APIs, source closures, test authority and final documentation.
  Storage and execution lanes may work separately after that contract is fixed.
- Preserve all unrelated dirty edits. The preexisting inventory repair and
  `test_required_python_inventory_matches_live_collection` must remain together.
- Import only independently relevant isolated hunks; no whole-checkout copy.
- Finish normative changes, then freeze HEAD/tree/dirty overlay, Cargo/Python
  dependencies, features, registry, selectors, toolchain, target root, binaries,
  inputs, host and runtime identity. Use a clean isolated qualification source;
  source changes require a new affected run, not an edited passing receipt.
- Run cheap targeted tests first, then affected Python control-plane and Rust
  contract tests; actual SDK/contract capture; fresh validation/replay and both
  actual consumers with metadata refusal controls; independent pre/post identity.
- Required-test inventory is exact identity equality with actual collection,
  command/package/target/features and terminal events. Omitted, duplicate,
  skipped, extra-substituted or zero-selected proof is refused. Test additions
  update the appropriate authority in the same change; counts are derived.
- Recheck local CLI/Just/registry/CI selector parity and same-raw old/new domain
  validator parity where both express the metric. No default values for metrics
  one side cannot represent. Hosted jobs require actual job terminals and their
  source identity; local proof is not hosted CI or release activation.
- DoD: all requested scopes have `VERIFIED`, `FAILED`, `BLOCKED`, `NOT_RUN` or
  `NOT_APPLICABLE`, exact command, raw results/counts, source/input/environment,
  artifact path/SHA-256, covered/excluded scope and next condition.

## 8. MISC-05 — functional, installed and platform qualification

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

## 9. MISC-06 — measurement, with separate denominators

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

## 10. MISC-07 — profile coverage and retrieval acceptance

### Execution inventory and product pilot

For every active profile/case, record registry digest, required input,
producer command, terminal count, raw inventory, validator/scorer, capture ID,
fresh replay, claim class and exclusions. `list`, `plan`, `runnable=true`,
compile, fixture and local replay are not interchangeable evidence.

Audited profile inventory (re-resolve registry and case listing at execution):

| Profile | Declared families |
| --- | --- |
| dsl-authority | dsl-warm, dsl-cold |
| dsl-diagnostic | micro-searchd-runtime-dsl-query-matrix |
| quality-core | relevance, ambiguity, snippet, scale, tail |
| quality-full | relevance, ambiguity, snippet, scale, tail, ann, concurrency, freshness, open-loop, ops, ui |
| systems | freshness, open-loop |
| semantic-ab | relevance-openai-ab; actual provider access required |
| micro | micro-lq-norm-pipeline, micro-searchd-runtime-dsl-query-matrix |
| retrieval-contract | retrieval-sdk, retrieval-contract |
| retrieval-diagnostic | retrieval-pair |
| lexical-diagnostic | lexical-file-comparison |
| recorded | agent-outcome, scan-vs-index |

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

### Retrieval comparison, data and scoring contract

All systems use one pinned repository commit, canonical sorted path/SHA byte
universe and frozen query pack. The current pair runner operates one repository
per capture; aggregate real per-repository pairs, never invent a synthetic
multi-repository commit. Corpus generation excludes unsafe/ignored/binary/
oversize inputs with retained reasons and uses the declared language/file policy;
candidate corpus presence does not prove each product indexed that universe.

The suite, query pack and schema-v3 runner records share byte-equal
`comparison_contract`: `top_k`, tokenizer, tokenizer-budget version and
output-unit policy (`rank_prefix`). Route records reference exact capture
provenance: runner/searchd binaries, generation, seal/activation, model and
chunk strategy/config. Supported strategy names are `whole_file`,
`fixed_window_strict`, `fixed_window_line_aligned`, `brace_heuristic` and
`semble_native` (Semble-owned only). Semble 0.6.0 is the frozen comparison
revision, not a claim that it is the latest release. Pin actual package/commit,
dependency lockfile and model assets for every capture. Reject old/unknown
record contracts rather than auto-upcast.

Tiny fixed-oracle repositories prove mechanics. Real evaluation data is split
before results are seen: separate development/holdout queries, near-duplicate
paraphrases, answer spans, query families, files and definitions as declared.
Reject leakage except explicit reviewed both-sides rationale-backed exceptions.
Gold/grades/category labels are evaluator-only, never runner/planner inputs.
`isolated` quality requires enforced inability to read gold plus an access-block
log. `attested` is weaker and cannot issue isolated quality proof.

Independent reviewers assign grades 0-3 under a frozen rubric: sufficient,
substantial/incomplete, weak support, irrelevant. Two distinct annotation
authorities and adjudication are required for qualified gold. Engine output or
model-generated/model-checked labels alone are not independent holdout truth.

Qualified primary metric: graded density-aware NDCG@10, scorer identity
`rb-rank-context-density-first-coverage`. A candidate earns credit only for
fully containing a gold byte span. Each gold span is credited once at its first
covering rank; use the maximum grade among newly covered spans, not their sum.
Let U be their union byte length and C the candidate bytes. Gain is
`(2**grade - 1) * U/C`; use rank-discounted gain and exact-span ideal DCG sorted
by grade. Publish raw chunk ranks and deterministic best-chunk-per-file collapse
separately. Same-file chunks consume ranks/budget; duplicates cannot boost gain.
Without independent grades, span Recall/MRR/BCY are diagnostics, not substituted
qualified NDCG. Report exact-span Recall@10, rank MRR/Hit@1, context bytes,
BCY and no-answer false-positive/abstention as separate secondary metrics.

Top-k policy `declared_top_k_v1` requires top_k>=10 for the @10 primary; @k above
the captured window is `NOT_APPLICABLE`, not extrapolated. Freeze the candidate
matrix, primary/guards, margins, order/seed, repeats and failure policy before
development evaluation. Select one final combination before opening holdout.
Primary paired 95% CI lower bound must meet the predeclared noninferiority
margin (default zero); the claimed effect must meet its declared improvement
rule, with all strata guards. No post-hoc exclusions or holdout retuning.
Use query/repository clusters for quality and preserve query/root structure for
latency; report strata and leave-one-repository-out sensitivity. Repeated calls
are not independent queries. Report bundle effects as bundle effects unless
separate ablation and independent holdout establish individual contributions.

### T00-T17 blocking matrix

| ID | Positive contract | Required negative controls |
| --- | --- | --- |
| T00 | Both runners' exact tracked path+SHA universe | Dirty/wrong revision, ignored/extra/missing/binary/oversize/symlink input, divergent filter |
| T01 | Pinned label bytes/lines, safe paths, blind pack and universe digest | Wrong location/hash, absent required gold, leaked grade/gold, mismatched universe |
| T02 | Dev/holdout and query-family separation, reviewed categories/grades | Duplicate/normalized/shingle-near queries, cross-split family/span leakage, invented no-answer |
| T03 | Exactly one eligible task/route row, ordered ranks/spans, exact contract and capture pins | Missing/extra/duplicate row, unknown field, nonfinite time, null versus zero confusion, dangling capture, forged lock |
| T04 | Hand-calculated span Recall/MRR/density-NDCG/BCY and independent rubric | Wrong lines, partial span credit, duplicate boosting, budget overflow, whole-file full credit |
| T05 | Separate fresh daemon, public readiness before publish | Wrong socket/state root, stale daemon/index, no readiness |
| T06 | SDK publish/seal receipt and exact CAS activation acknowledgement | Direct-IPC substitute, partial seal, digest mismatch, conflict, query before activation |
| T07 | SDK lexical/semantic/hybrid real ranked spans at expected generation | Wrong route/generation, typed timeout/error turned into empty success, capped result called exhaustive |
| T08 | Original byte slices, line bounds and stable chunk IDs | UTF-8/CRLF/BOM/EOF errors, empty/overflow span, nondeterministic IDs |
| T09 | Supported parser boundaries and explicit fallback coverage | Unsupported grammar, parse/long-declaration failure, silent whole-file fallback, overlap inflation |
| T10 | Rebuild semantic sources per chunk strategy with pinned real model | Reused vectors from another strategy, hash-dev claimed as model quality |
| T11 | Real pinned Semble mapping to exact admitted bytes/spans | Truncated snippet as full chunk, path drift, skipped query, missing model, filter mismatch |
| T12 | Complete real pair with equal commit/files/queries/k/budget/host and rebound pins | One-sided/public-score substitute, partial samples, wrong host/model/cache, repeated few-task qualification |
| T13 | Same immutable raw gives identical aggregate/disagreement/digest after relocation | Path/order-dependent score, omitted errors, duration exceeding enclosing phase, invalid result marked qualified |
| T14 | Registered commands, external output, fresh final-path public replay | Implicit CI downloads, dirty output root, unselected test claim, stage-only replay |
| T15 | Same-model claim: pinned weights/tokenizer/normalization/precision and full raw vectors within declared tolerance | Same name/dimension alone, reordered/subset/forged vectors, wrong model/config/executable |
| T16 | Incremental claim: SDK operation, exact activation and full fresh/incremental row equality | Stale hit, mixed generation, no-op change, sentinel delete, mtime-only truth, rebuild called incremental |
| T17 | Closed qualified admission binds source/corpus/suite/license/two annotators/adjudication/assets/host/receipts | Exploratory promotion, duplicate authority, stale or tampered inputs/receipts, undeclared cache |

T15/T16 are `NOT_APPLICABLE` when same-model/incremental claims are false; do not
make them unconditional pair blockers. When enabled, rederive from raw vectors
or before/operation/fresh/incremental full rows plus actual execution/binary/
configuration/collection/source custody. T16 preserves unrelated owners,
requires meaningful typed operations and one final successful build terminal;
reject contradictory/duplicate/later terminals and bool/float scalar aliases.
Summary pass/count JSON plus a digest is not independent evidence. Fault/restart
needs actual fault execution; full-row comparison alone does not prove it.

Returned-window diagnostic is separate from scorer input. Bind its SHA and
record/pack identities, exact task/route inventory, candidate rank/path/span,
status and hybrid lane contributions. Reject extra/duplicate/nonfinite/forged
fields. Distinguish runner boot/readiness, opaque publish/activate, assembly,
corpus reverification and shutdown; do not invent embed/index/seal subphase
timings. Requested floor and actual initial-fetch trace must agree; preserve
current diagnostic/protocol versions and strict required fields.

### Qualification and sample floors

- Distinct claims: `CONTRACT_GREEN`, `SDK_PATH_GREEN`, `PAIR_VALID`,
  `QUALITY_DELTA`, `PERF_QUALIFIED`. Quality/performance require valid contract,
  SDK and pair proof on the same bound source/run. Preserve specific failures.
- `scope=qualified` requires schema-closed W0-B admission (T17), binding license,
  independent gold, source, repository, corpus/suite/pack, model/tokenizer,
  Semble lockfile, host/cache and exact contract/SDK receipts. Recheck before
  capture and from frozen artifacts. Exploratory scope cannot accept admission
  to manufacture qualification; its quality/performance are not applicable.
- Same exact query text; declare internal transformation. Explicit Semble-to-
  canonical path mapping and both-side path/SHA diff digest are mandatory.
  Same file count is insufficient. Keep warm API latency separate from process
  startup; include each product's embedding/indexing cost consistently.
- Quiet same host: CPU/power/cache configuration, no competing builds/benchmarks,
  thermal/frequency checks, capture-time observations and actual reservation.
  No load-average-only or preflight-only performance approval.
- Cold/time-to-searchable: at least five fresh state roots per repo/system,
  alternate product order, prebuild/provision outside timing. Include discovery,
  chunking, embedding, publish/seal/activation and first successful query.
- Warm: at least 20 distinct admitted tasks, exactly one eligible Quanta route,
  at least one warmup pass/root, five fresh roots and 1,000 valid observations
  per eligible route. Each floor is independent. Example: 20 x 10 x 5.
  Both products follow the same driver-issued randomized schedule.
- Raw monotonic query durations must fit declared enclosing windows/tolerance;
  preserve cold/first-query evidence. Unknown duration is null, not zero.
  Missing eligible timing blocks speed. Do not gate on underpowered p99.
- Freshness/update/rename/delete and concurrency are separate scenarios.
  Primary pair is serial warm latency; concurrency capacity needs equivalent
  client/arrival contracts and full offered/completed/error accounting.
- `verdict.json` uses the existing schema-v2 owner, rederived provenance and
  per-comparison digests. Missing and not-applicable T IDs are disjoint. Every
  verified comparison is reported; no empty comparison success. Fresh public
  replay at the promoted path must reproduce canonical identities and states.

## 11. User-owned inputs and explicit exclusions

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

## 12. Verification commands and stop rules

Commands are run from the chosen source root. Replace angle-bracket placeholders
with fresh external paths or actual immutable IDs. Start with affected owners;
do not launch all expensive commands concurrently.

```sh
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_benchctl.py tools/ci/tests/test_benchmark_profile_capture.py tools/ci/tests/test_benchmark_evidence_bridge.py tools/ci/tests/test_bench_protocol_conformance.py -q -p no:cacheprovider
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_criterion_capture.py tools/ci/tests/test_pair_capture.py tools/ci/tests/test_retrieval_capture.py tools/ci/tests/test_lexical_capture.py tools/ci/tests/test_recorded_capture.py tools/ci/tests/test_portable_proof.py -q -p no:cacheprovider
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_benchmark_source_closure.py tools/ci/tests/test_write_verification_receipt.py tools/ci/tests/test_retrieval_contract_proof.py tools/ci/tests/test_retrieval_sdk_proof.py -q -p no:cacheprovider
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_producer_notifications.py tools/ci/tests/test_bootstrap_cache.py tools/ci/tests/test_proof_command_timings.py -q -p no:cacheprovider
python3 tools/ci/lint/lint-doc-paths.py
python3 tools/ci/lint/check-benchmark-policy.py
python3 tools/ci/lint/check-test-authority.py
python3 tools/prompt-manager/pm.py lint
git diff --check
./scripts/cargow --lane test-daemon-lane test -p quanta-index-bench-protocol --locked
just benchmark-prep-local
just retrieval-contract-local
uv run --frozen --extra dev python tools/benchmark/benchctl.py run retrieval-contract --evidence-root <fresh-external-root> --producer-timeout 7200
uv run --frozen --extra dev python tools/benchmark/benchctl.py validate retrieval-contract --evidence-root <same-external-root>
uv run --frozen --extra dev python tools/benchmark/benchctl.py replay <actual-run-id> --evidence-root <same-external-root>
just rust-profile verify-rust
QUANTA_INDEX_TEST_THREADS=1 just rust-profile test-daemon-all
just retrieval-pair <frozen-external-spec>
just retrieval-verdict <repo> <suite> <run-manifest> <fresh-external-verdict>
```

SDK/contract-only proofs can use `just retrieval-sdk-proof <fresh-root>` and
`just retrieval-contract-proof <fresh-root>`; the common profile already
executes both, so do not repeat them for the same closure without cause.
Public SDK changes add `just rust-public-api`; IPC decode/wire changes add
`just rust-fuzz-smoke`; module-boundary changes add `just rust-hexagonal` and
`just rust-cargo-modules`. Mark unneeded gates `NOT_APPLICABLE` with diff reason.

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
