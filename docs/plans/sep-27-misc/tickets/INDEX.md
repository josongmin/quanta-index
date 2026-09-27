# SEP-27 benchmark / retrieval / test-optimization — execution SSOT

Status: `ACTIVE`. Final code/document audit: 2026-09-27 KST.
Audit source: shared dirty `main@2102966246866398f01833bebf71396831377149`.
This refresh owns only this SSOT; existing Python/Rust implementation and
active engine documents are preserved. The untracked `host_monitor.py` is
unfinished code, not an active capture facility. A commit, a file's presence
or a focused owner run does not establish whole-source qualification.

This is the single work/acceptance contract for the former SEP-27 four-agent
handoff union and RB/BM/RBR/TOPT work. Requirements, RCA, file/function owners,
logic, negative controls, dependencies and acceptance criteria are inline.
Deleted documents and external receipts are not additional task specifications.
Executable schemas, registry and tests remain code contracts, not competing
planning documents. Code paths below are repository-relative implementation
owners. Do not restore duplicate ticket bodies or redirect stubs.

## 1. Final audit: current facts and remaining work

The old chronology has been removed from this SSOT. Historical test totals and
snapshots are not current proof and must not be added together. Implementation
presence, owner tests, integrated execution and product/performance qualification
are separate claims. The audit result is **not repository qualification**.

| Boundary | Current source fact | Remaining action / claim status |
| --- | --- | --- |
| MISC-01 publication/GC | `profile_capture.publish_capture` is the shared complete-profile owner; `commit_capture` is the pointer commit; Rust `RunStore::collect` takes custody before checking the capture marker. | Retain implementation. Actual producers, both consumers and fresh final-source replay: `NOT_RUN` in this audit, owned once by MISC-04. |
| MISC-02 / EXEC-1 | Native immutable capture uses `current_capture().execute(execute, ...)`; the no-evidence-root branch still calls `subprocess.run`. Promotion assigns `shared/1` without a transcript. `host_monitor.py` exists but has no production caller, no owner tests and no capture/replay binding. | Complete the existing monitor and connect lifecycle, descriptor custody, raw derivation and fresh replay in one cutover. Do not reimplement migrated immutable dispatch. Integrated acceptance: `NOT_RUN`. |
| MISC-03 / IO-1–3 | Shared `RawFile`/`RawWriter`, process log owner, bounded controls/JSONL, streamed archives, portable receipts/replay and paired command ZIP are present. | Retain the contracts below; no blanket whole-repository bounded-memory claim. IO-5 whole-capture resource acceptance: `NOT_RUN`. |
| MISC-03 / IO-4 | The epoch spans native plus five adapters. Nested nonzero/exception/refusal is sticky; source/replay callbacks cannot publish after recording failure. Native primary refusal reasons and the positive fresh-replay test boundary are repaired. | Current eight-module selection: `FAILED`, 358/359 pass. Pair replay correctly refused a changed symbol preflight policy commitment while another writer edited its source. The focused case passed separately, but does not replace the full selection. Frozen-source rerun remains required. |
| MISC-04 / C5 | `test_pair_replay_workspace.py` and `test_cargo_preparation.py` are in the Just command and affected closure paths. Neither has a `python_targets` entry. Explicit collection/owner/mutation guards do not cover both. | Register owner/scope and extend guards; do not repeat existing command/closure additions. Acceptance: `NOT_RUN`. |
| MISC-05 | Existing TOPT/retrieval invariants are qualification obligations, not presumed new bugs. Active SDK/engine changes are outside this edit. | Re-audit and run focused/full/installed/platform scopes after final freeze; `NOT_RUN` here. |
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

### IO-4 owner-local proof, provenance failure and integration blockers

Command: `uv run --frozen --extra dev python /private/tmp/qi-misc-final-audit.SzqYJZ/audit.py`.
The driver runs pytest over `test_benchctl`, `test_benchmark_profile_capture`,
`test_benchmark_evidence_bridge`, `test_recorded_capture`, `test_retrieval_capture`,
`test_criterion_capture`, `test_pair_capture`, and `test_lexical_capture` under
`tools/ci/tests/` (all `.py`), with `-q -p no:cacheprovider -o
junit_family=legacy` and external JUnit. At dirty
`main@2102966246866398f01833bebf71396831377149`, the latest selection
executed 359, with **358 pass/1 fail**, zero error/skip, exit 1, 117.12
seconds. One deliberate duplicate-ZIP fixture warning. Failure:
`test_pair_capture.py::test_capture_complete_profile_and_replay_with_original_corpus_changed`;
`pair_capture.derive` reported `native pair verdict differs from raw owner
recomputation`. An independent diagnostic comparison of the retained native
verdict against rederived raw showed `PAIR_VALID=pass` recorded versus `fail`
current, with `phase_metrics_invalid:preflight source/grammar/policy commitment
mismatch` and missing T12. The relevant verifier is
`tools/benchmark/retrieval/symbol_coverage.py::policy_digest`, which hashes
`benchmarks/retrieval/src/symbols.rs`, preflight source, build source,
`Cargo.lock`, grammar and limits. `symbols.rs` had filesystem modification time
2026-09-27 09:08:20 KST, inside the latest test window. The same case passed
alone in 75.67 seconds earlier. This supports concurrent producer-policy
source drift as the failure mechanism; the audit did not capture before/after
hashes for that Rust file, so exact causality is an inference, not verified.
Do not weaken the provenance comparison or call this an implementation
regression without a frozen-source reproduction. The full owner claim is
`FAILED` for this run; stop and repeat after the active writer freezes all
source, including symbol policy inputs. The overall dirty status changed
concurrently; this is not a whole-tree or current-source qualification receipt.
The untracked `host_monitor.py` is among the hashed files but is not imported
by a capture path or tested by this selection. EXEC-1 remains `NOT_RUN`.

External audit root: `/private/tmp/qi-misc-final-audit.SzqYJZ`.
Latest `receipt.json` SHA-256:
`649c40da5133a29dd8929bebd1588d879b6c99597513c848a6c09af392be592c`;
`owners.xml` SHA-256:
`bf322d4bf467a1d21fef1f8b254ef8fd0a6c70b12a71cd800bda80ceb078bb3f`;
`owners.log` SHA-256:
`f059971d5ee2b3108639e03831ab72a573804ed5451edebddaeff0d2766b31b7`.
The focused one-case `pair-focused.xml` SHA-256 is
`1fddc26286fd9a0ce37c704f0ae3949f4a5e5104736729b3aa488005f4be7b05`.
The driver records argv, before/after bounded hashes (including this SSOT),
platform/Python identity, relevant environment and raw-log digests. It does
not bind the changing Rust policy source, actual producer binaries, host
quietness or a complete product source closure. The final documentation edit
changes this SSOT digest after the test; it cannot upgrade the failed run.

| Executed check | Status | Exact remaining condition |
| --- | --- | --- |
| `python tools/ci/lint/lint-doc-paths.py` | `FAILED` | Four broken paths in active engine evidence/templates, listed below; no new-SSOT link failure reported. |
| `python tools/ci/lint/check-test-authority.py` | `FAILED` | Five orphan Rust integration targets at the last audit, listed below. This is separate from the two C5 Python gaps. |
| `python tools/ci/lint/check-benchmark-policy.py` | `VERIFIED` in policy scope | Exit 0; not producer or performance evidence. |
| `python tools/prompt-manager/pm.py lint` | `VERIFIED` in generated-document scope | Exit 0. |
| `git diff --check` | `VERIFIED` in whitespace scope | Exit 0 at the checked snapshot. |
| Ruff on current `host_monitor.py` | `VERIFIED` in lint scope | Import order corrected; exit 0. This does not execute the monitor. |
| Full final-source gate | `NOT_RUN` | The monitor is unfinished and not wired. Run after coordinated EXEC-1 edits. |

MISC-04 integration owner must resolve the following live targets, preserving
their writers' work. They are repair targets, not external ticket dependencies:

- `docs/plans/sep-27-code-search-remediation/handoffs/l3-proof/adversarial/prior-L3_HANDOFF.md:3,5`
  has broken `L3_FOLLOWUP_AUDIT.md` and `L3_FOLLOWUP.source.json` relative paths.
  Preserve provenance if it is an immutable evidence preimage; use an explicit
  archival validation policy or truthful navigation correction, not deletion
  of active proof to silence the gate.
- `docs/plans/sep-27-code-search-remediation/handoffs/l4-proof/authority-id/report-template.md:36,72`
  has broken `l4-proof/authority-id/receipt.json` and
  `l4-proof/authority-id/live-closeout.json` relative paths. Correct the live
  template's path semantics without generating a false receipt. The previous
  missing L1 adversarial-report path is now present and no longer an open item.
- `crates/quanta-index-contract-base/tests/l4_preview_emission.rs`,
  `crates/quanta-index-contract/tests/l4_preview_wire.rs`,
  `crates/quanta-index-sdk/tests/l1_daemon_query_contract.rs`,
  `crates/quanta-index-sdk/tests/l2_daemon_publication.rs`, and
  newly appearing
  `crates/quanta-index-searchd-runtime/tests/l4_preview_sdk.rs` have no
  catalog entry at the last audit.
  Register exact target/package/features/owner and intended rail in
  `tools/ci/test-authority.toml`; check live selection and closure binding.
  Do not remove the tests or add a blanket gate exemption.

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
| MISC-02 | `benchctl.py`, `producer_execution.py`, `host_monitor.py`, `evidence_bridge.py` | Complete EXEC-1 observed host lifecycle and native dispatch disposition. |
| MISC-03 | `evidence.py`, `raw_archive.py`, capture adapters, portable proof | Retain repaired IO-4 and IO-1–3 regressions; execute IO-5 whole-capture resource proof. |
| MISC-04 | Shared CLI/bridge, Just, test authority, source closures | Finish C5; freeze once and integrate actual producer/consumer/replay and governance evidence. |
| MISC-05 | Runtime/SDK/core/platform owner tests | Qualify retained invariants, full Rust/daemon, installed ingest and supported platforms. |
| MISC-06 | Existing test/query/ingest/micro/system measurement owners | Measure distinct workloads only after correctness, host and input admission. |
| MISC-07 | Registry/domain adapters/retrieval evaluator | Complete real profile execution inventory, live comparator pilot and separately admitted quality/performance claims. |

Order: retain IO-4 owner proof → complete EXEC-1 and C5
(disjoint files may proceed independently) → IO-5 → finish docs and freeze once →
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

IO-5 remains separate: measure complete capture/replay peak RSS with larger
payloads, large failure output and many-file metadata, in fresh processes with
declared platform/tolerance. Component tests or finite admission ceilings alone
cannot establish whole-capture memory bounds or a performance improvement.

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

- `tools/benchmark/benchctl.py`: immutable capture already uses shared execution.
  Route the remaining no-evidence-root native producer branch through the same
  execution owner with external diagnostic log custody. Preserve its command
  semantics, including baseline admission; command diagnostics are not an
  immutable profile capture and must not invent a capture pointer. Retain the
  existing refusal of incompatible baseline/evidence-root options. Remove
  invented one-sample host evidence together with observed inputs and replay
  validation. Do not replace unrelated short Git/toolchain probes.
- `tools/benchmark/producer_execution.py`: retain private session, parent
  lifeline, actual terminal record, unreaped group identity, bounded cleanup
  and signal handling. Exit zero alone does not establish descendants exited.
- `tools/benchmark/criterion_capture.py`: bind observations to observed build,
  listing, smoke and measure command boundaries. Criterion warmup occurs inside
  its measure process; do not invent a separately observed warmup interval.
- Existing unfinished `tools/benchmark/host_monitor.py`: own capture-time observations and
  cooperative local reservation; `evidence_bridge.py` derives typed host facts
  from the validated observations instead of caller-supplied summary constants.
- `tools/ci/source_closure.py`: enroll new module/tests in the affected closures.

### Final implementation split: one lifecycle, no patch-on-patch adapter

All rows below are remaining work, not claims that these APIs already exist.
They share one EXEC-1 acceptance boundary; do not introduce another transaction,
success marker, producer runner or permissive replay parser.

| Step | Current code / root cause | Exact change and refusal boundary |
| --- | --- | --- |
| E1: descriptor custody | `producer_execution._execute_owned` already accepts `custody_fds`, but public `execute` does not forward them. Controller-only reservation can end before guarded child cleanup. | Extend the current public owner to forward validated reservation descriptors to the existing guard. Producer children must not inherit them. Test controller death while a nested child exists; a second participating capture must remain excluded until cleanup completes. |
| E2: epoch lifecycle | `CaptureEpoch.execute/__exit__`, `capture_entrypoint` and `publish_capture` do not reference `HostMonitor`. A new module alone creates no observation. | Admit native/Criterion monitoring explicitly. Start before the first admitted producer, record actual phases, finalize after producer cleanup and before publication. Monitor failure is sticky, refuses publication, retains partial raw/failure diagnostics and preserves primary plus cleanup errors. Specify whether failure cancels the active producer or is detected at its next boundary; never claim immediate cancellation without implementing it. |
| E3: transcript custody | The draft validates sequence/time/facts, but `validate` accepts any five nonnegative lock-identity integers. `finish` raises on join timeout before its cleanup `finally`. | Validate regular-file mode and single-link invariant without pretending replay proves a live local lock. Give every start/poll/phase/finish/close failure an explicit bounded custody state. A live observer must not write through closed/reused descriptors; a timeout cannot be reported as successful release. Test malformed headers, bool aliases, changed reservation, missing end, excess gaps, clock jumps and failed cleanup. |
| E4: canonical host derivation | `evidence_bridge.host_identity` accepts caller lease summaries. Native promotion hardcodes `shared/1`; Criterion currently honestly emits `none/0`. | Derive host identity/count from validated `host-observations.jsonl`; bind its exact digest and capture/profile identity in existing raw/input fields. Bind expected capture identity independently of the transcript, including detached replay. Standalone artifact import has no observed producer: emit honest `none/0`, not a fabricated lease. A missing required transcript cannot fall back to unobserved success. |
| E5: all readers together | `profile_capture.load_capture`, `benchctl.replay_command` and `criterion_capture.replay_run` do not rederive monitor facts. Native replay treats every raw file as a native JSON artifact. | Recheck the reserved host transcript in publication, capture loading and fresh replay. Separate only a validated reserved host raw from native artifact cardinality/parsing; arbitrary extra JSONL must still refuse. Reject removed/duplicate/replaced transcript, mismatched commitment, wrong capture/profile and forged host summaries even after envelope digests are recomputed. |
| E6: remaining dispatch | `benchctl._native_tail` has a direct `subprocess.run` branch when no evidence root is selected. | Route that actual producer through `producer_execution.execute` with external diagnostic logs; retain timeout, exit status and baseline-admission semantics. No immutable capture/pointer claim for command-only mode. Leave unrelated Git/rustc probes alone. |
| E7: authority and proof | There is no monitor test module/caller to confer coverage. `source_closure.py` lists individual benchmark Python owners and does not include `host_monitor.py`; file existence is not enrollment. | Enroll the new owner in benchmark-control and derived closures, and add tests to existing capture/bridge/criterion/producer owners or register a single new test owner. Verify live command selection, exact source closure and mutation invalidation. Run common-owner regressions once after integration, followed by actual native/Criterion execution and relocated fresh replay. |

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
| `tools/benchmark/producer_execution.py::_wait_for_terminal/execute/_cleanup` | IO-1 implemented; retain file-backed normal/failure drains and bounded tails. Native immutable dispatch uses this owner; remaining legacy dispatch/host work is EXEC-1. |
| `tools/benchmark/evidence.py::RawFile.consume_lines`, recorded/Criterion/native/lexical/portable capture and domain readers | IO-3 implemented: bounded JSONL/retained metadata/control JSON, file-backed preparation, canonical receipt production/replay and paired-verdict command-log ZIP. Retain owner regressions and final-input rechecks; whole-capture RSS remains IO-5. |
| Adapter preparation/execution and `profile_capture.py::CaptureEpoch/publish_capture` | IO-4 owner implementation and the misplaced native test tail are repaired. Retain section 3's phase/failure matrix and the 359-case owner proof; final integrated qualification remains MISC-04. |

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

- Finish C5 before accepting archive/reuse-build owner proof as canonical:
  retain the concurrent additions of `tools/ci/tests/test_pair_replay_workspace.py` and
  `tools/ci/tests/test_cargo_preparation.py` to
  `Justfile::benchmark-control-contract-local` and the affected source closures.
  Register their exact owners
  (`tools/benchmark/pair_capture.py` and
  `tools/benchmark/retrieval/portable_proof.py`) in
  `tools/ci/test-authority.toml`'s `python_targets` and
  `python_scopes.benchmark-control-capture`. Preserve both in the benchmark
  source closure and preparation in retrieval closure;
  derived micro/retrieval closures must inherit the union. Extend
  `test_benchmark_policy.py` and `test_benchmark_source_closure.py` guards to
  require unique selection, correct owner/scope, actual nonempty collection,
  mutation invalidation and rejection when either module is omitted. Existing
  C4 owner/collection guards protect their declared three-module set; mutation
  guards additionally cover the resource-admission modules, not both C5 modules. Do not close C5
  with another explicitly selected ad hoc run or a fixed expected case count.
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
