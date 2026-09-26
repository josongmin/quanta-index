# SEP-27 benchmark / retrieval / test-optimization — execution SSOT

Status: `ACTIVE`. Latest documentation/source re-audit: 2026-09-27 KST,
Current source-inspection baseline and latest archive checkpoint source:
`main@98601a66d8cab9c86232b3e62ce490c8b43b71b6`, shared dirty checkout.
The earlier documentation-only audit used `106d7abe`.
The latest documentation-only refresh owned only this file. The subsequent
IO-2 implementation owns the shared raw/archive modules, pair/corpus/lexical
callers and their regression tests described below. It preserves the separately
active engine/resource work and does not qualify those dirty changes.
The subsequent IO-3 change owns `evidence.py`, `agent_outcome/__main__.py`,
`recorded_capture.py`, `criterion_capture.py` and their existing owner tests.
It streams recorded/Cargo JSONL. The next coordinated IO-3 change owns native
capture/validation/DSL control readers, lexical capture/scoring and their owner
tests. The latest IO-3 change migrates portable production, canonical receipts,
nextest/domain readers, detached validation and retrieval capture control reads.
The subsequent paired-verdict change connects `retrieval/run.py` command-log
freeze/verification and isolated runner bundles to the same file/archive owner.
Preserve concurrent engine
changes in that module and the shared command/closure/catalog files.
The earlier documentation epoch began at `66cee47e` and crossed another
writer's commit. Its source inventory and test results are historical, not
the current ownership inventory.
Another writer committed the bulk execution/archive migration before the final
archive owner run. Archive metadata refinements/tests and this contract remain
dirty at the checkpoint; preserve their exact bytes, not just the new HEAD.
A commit does not establish final-source qualification.
This document is the single current work/acceptance contract for this scope.
All required decisions, ticket bodies, negative tests, measurement rules and
handoff conditions are inline. No deleted plan or handoff is a prerequisite.
Code paths below are repository-relative and identify implementation owners,
not alternative planning authorities. Machine-readable schemas and test
inventories continue to define their executable contracts.

Implementation in the shared dirty checkout is allowed. Preserve unrelated
changes. Qualification requires an immutable source/input/environment identity;
do not manufacture a clean receipt from dirty console output. This document is
bound by both retrieval and benchmark source closures. Finish normative edits
before freezing; put post-freeze results in external digest-bound receipts.

## 1. Final audit and evidence boundary

### Latest code-first checkpoint: paired-verdict file custody

The paired-verdict tail now uses `RawFile` for receipt/binary/log copies and the
shared `raw_archive` pack/unpack implementation. Portable owner tests: 100
selected/executed/passed; expanded caller verification is pending at this edit.
The earlier 628-case checkpoint has a different source/selection and remains
historical. No full-source, actual Rust/SDK/searchd, whole-capture resource or
integrated qualification is claimed.
Next implementation order is IO-4, EXEC-1, then resource and serial qualification.
C5 is still open; do not duplicate
its already landed Just/closure entries. The earlier planning diagnosis below
is historical; the current findings and implementation matrices reflect the
subsequent repair.

### Earlier final planning refresh: pre-implementation diagnosis

This refresh owns only this document. It inspected current code on dirty
`main@98601a66d8cab9c86232b3e62ce490c8b43b71b6`; it did not implement repairs,
rerun the historical 520-case selection, or execute product/runtime benchmarks.
Source inspection confirms the four remaining implementation boundaries below.
They refine existing tickets rather than create duplicate work. No named
workflow from an earlier turn is an additional acceptance authority.

| Priority / single ticket owner | Current reachable boundary | Required structural result |
| --- | --- | --- |
| 1 — MISC-03 / IO-3 | Portable production copies metadata/inventory/nextest output into bytes; `validate` retains all captured evidence in `dict[str, bytes]`; contract/SDK/inventory and canonical-receipt readers materialize it again. | Complete the entire producer → parser → canonical receipt → relocated replay chain using one committed-file identity. Stream event logs and hashes; cap actual control documents before decoding. Exact file/function/test matrix is inline in section 3. |
| 2 — MISC-03 / IO-4 | Adapter preparation and native preflight/execution precede `publish_capture`; publication therefore cannot record their failures. | One capture epoch begins before input acquisition, records observed phases and failure custody, and reaches success only at the existing pointer commit. No second publication implementation. |
| 3 — MISC-02 / EXEC-1 | Native recipe dispatch still uses `subprocess.run`; promotion sets `lease_mode = "shared"` and `lease_samples = 1`. | Join the existing process/log owner, collect capture-bound diagnostic host observations, and derive summaries during validation/replay. Do not treat cooperative reservation as quiet-host qualification. |
| 4 — MISC-04 / C5 | Both extra regression modules are selected by Just and present in relevant closure paths, but neither has a `python_targets` entry; the explicit mutation test does not name them. | Add exact authority/scope enrollment and live collection/omission/duplicate/source-mutation controls. Do not repeat the already landed command/closure repair. |

After these repairs, IO-5 resource evidence and MISC-04/05 integrated runtime
acceptance remain `NOT_RUN`. MISC-06/07 require separate measurement and input
admission; a code repair does not establish quality or performance superiority.
The earlier publication, shared execution, archive and native/lexical/recorded/
Criterion streaming changes are retained, not reopened as missing code.

Current audit artifacts are external evidence only:
`/private/tmp/qi-misc-final-plan.oUBrSS/receipt.json` records before/after
source and document identities, exact policy commands and raw results, the
41-document removal census, and the recovery-archive hash. Its covered scope
is documentation/policy and source inspection, not product execution. All task
requirements remain here; no receipt or deleted document must be consulted to
understand or implement the plan. Source drift invalidates only claims bound
to the changed inputs and is reported rather than hidden.

### Latest source re-audit and current decision

This refresh re-read the four archived handoff preimages, their current
dispositions below, the implemented publication/execution/storage boundaries,
all remaining archive/preparation call sites and canonical selector wiring.
No implementation or product test was run by that documentation-only refresh.
The subsequent archive implementation and owner proof have their own checkpoint
below; the documentation-only receipt cannot cover those later changes.
The 617-case execution checkpoint below is retained historical owner evidence,
not a new test result. Policy-only rechecks, source inventories and the 41-body
deletion census are recorded under
`/private/tmp/qi-misc-ssot-reaudit.3DPuax`; `closeout-receipt.json` binds their commands,
terminal results, raw hashes and final document digest. That receipt is
evidence only: it contains no additional task requirements.

The source-backed remaining repairs retained from this handoff union have two
runtime implementation owners: producer/host lifecycle (MISC-02) and
payload-sized memory plus failure-output custody (MISC-03). A newly confirmed
test-selection/source-binding omission (C5) belongs to MISC-04, not another
runtime abstraction. A concurrent writer added its command/closure membership
during this audit; authority enrollment and regression/terminal proof remain.
MISC-01 and the original three-module C4 fix are present.
MISC-04 through MISC-07 also retain distinct integration, product/platform and
measurement obligations; unrun proof is not itself a runtime defect.
No ranking/default change or unrelated proposal is accepted by this refresh.
This is a bounded audit of the consolidated work, not a claim that every defect
in the repository has been ruled out.

| Scope | Current status | Evidence / next condition |
| --- | --- | --- |
| Python owner tests | Portable checkpoint: 628 selected/executed/passed, zero failure/error/skip; final closure/integrated-source qualification `NOT_RUN` | This seventeen-module selection has unchanged owner/test bytes across execution. Concurrent engine changes and subsequent normative-document edits prevent whole-source qualification. Earlier 520, 606, 368, 617 and 460 selections are different historical scopes, not additive counts. |
| MISC-01 Rust shared GC lock | Current implementation confirmed by source inspection; execution `NOT_RUN` in this refresh | Earlier 51-case result below is historical owner evidence, not a new Rust run. |
| MISC-02 native lifecycle and host observations | Implementation `NOT_RUN` | Native recipe still calls `subprocess.run`; `shared` / one sample is assigned, not observed; `host_monitor.py` is absent. |
| MISC-03 bounded I/O and retained failure epoch | Partially implemented; final-source qualification `NOT_RUN` | File-backed staging, execution, pair/corpus archives, native preparation, recorded/Cargo/lexical JSONL, portable producer/receipt/replay and paired-verdict command-log ZIP paths are implemented. Capture-wide failure records and end-to-end resource proof remain open. No measured OOM claim. |
| C5 canonical test selection/source custody | Partial concurrent repair observed; acceptance `NOT_RUN` | Both modules now occur in the canonical command and affected closures, but still lack `python_targets` entries. Complete owner/scope registration and omission/mutation regression proof before canonical closure. |
| MISC-04 through MISC-07 final-source acceptance | `NOT_RUN`; input-dependent claims become `BLOCKED` only when their actual prerequisite is missing | Execute the inline ticket-specific oracles after the shared API cutover; do not rerun already sufficient owner checks under multiple ticket names. |
| Superseded document removal | `VERIFIED` filesystem census | No Markdown bodies remain in the five replaced directory groups; exact external backup SHA-256 rechecked. No additional deletion was needed in this refresh. |
| Documentation/policy epochs | Latest portable postcheck: test-authority `FAILED`; other five checks exit zero | Concurrent `quanta-index-contract-base/tests/l4_preview_emission.rs` and `quanta-index-contract/tests/l4_preview_wire.rs` have no catalog entry. Record their exact owner enrollment under the existing integration boundary; do not count the Python owner pass as governance closure. Earlier policy epochs remain historical. |

Earlier documentation audit evidence root:
`/private/tmp/qi-misc-final-doc-audit.oO2YSv`. `inventory.json` records the
41-body deletion census, recovery-archive hash, retained owner-artifact checks
and current-source differences. `receipt.json` retains the first post-edit
gate epoch; `final-receipt.json` records the final five documentation/policy
commands, raw hashes and before/after source inventory.
The latter checks documentation/policy only; no producer, Rust, installed,
platform, hosted-CI or performance run is claimed by this refresh. Concurrent
lexical implementation changes after the first gate epoch are outside this
audit's work ownership and cannot inherit its historical test results.
That earlier gate's exact failure was `orphan integration test target: no catalog
entry` for the lexical test named above. The other four commands exited zero.
An unrelated new handoff changed during that epoch; no whole-worktree stable
qualification is claimed. This subsequent status note changes the document
digest; `note-receipt.json` binds its doc-path and whitespace recheck only.

The retained 460-case run in `/private/tmp/qi-misc-file-raw.xHC7fo` had unchanged
tracked file hashes across execution. Concurrent tracked edits were already
present in its BEFORE snapshot; two newly untracked files were not selected.
Do not misclassify that historical run as source-drifted merely because Git
status changed. The later pair-test change is a separate, current-source drift.
Retained receipt SHA-256:
`5705a3fbad402a9b14ed238bc86d970741d7217e7d445adeff0c0ceb125be5db`;
JUnit SHA-256:
`c64d4dcdc804740516c9ebae0b1421eedf83855b385c6bf670ede62e692c6a01`.
External files are evidence, never additional task specifications. Every
required decision, implementation step and acceptance rule remains inline.

### Current findings

| ID | Finding and evidence | Classification |
| --- | --- | --- |
| C1 | Pre-patch native publication lost runs to GC and validated newest families instead of a complete capture. All six Python adapters now use `profile_capture.publish_capture`; native validation loads one pointer-bound capture. Rust GC takes the same lock before checking the capture marker. | Repaired in current source; owner-local checks `VERIFIED` only within the historical checkpoint below. Final-source actual-producer/fresh-replay qualification `NOT_RUN`. |
| C2 | Native recipes use direct `subprocess.run`; host envelope uses static `shared`/one sample. Criterion is honestly diagnostic (`none`/zero samples), not capture-time host proof. Shared execution and capture modules already exist. | Source-backed integration gap; MISC-02 `NOT_RUN`. No measured slowdown claim. |
| C3 | Verification, hashing, staging, producer output, pair/corpus archives, native preparation, recorded/Cargo/lexical/nextest JSONL, portable validation and paired-verdict command logs use file-backed I/O. The 64 MiB aggregate transcript payload limit is preserved, with separate ZIP metadata/envelope bounds. | MISC-03 partial: IO-3 code is implemented; IO-4 and full capture resource proof remain. A finite 64 MiB admission ceiling was not an unbounded-memory defect; component RSS cannot establish end-to-end bounds. |
| C4 | The three execution/cache/timing regression modules were absent from canonical selection and source closure. They are now registered in Just, test authority and the affected closure profiles, with exact owner, duplicate/omission, nonempty live collection and source-mutation guards. | Coverage wiring repaired; owner-local checks `VERIFIED`. Full MISC-04 integration/hosted execution remains `NOT_RUN`. |
| C5 | Initial inspection found `test_pair_replay_workspace.py` and `test_cargo_preparation.py` absent from the canonical command, authority and four computed closures. Concurrent changes added command membership and the required closures: both in benchmark-control/micro/retrieval, preparation also in retrieval. Authority entries remain absent; existing explicit C4 guards protect only their three modules. | Partially repaired coverage/receipt-binding gap under MISC-04. Registration and specific omission/mutation/collection proof remain open. This is not a failing product algorithm or absence from every possible broad test command. |

C5 diagnostic command:
`uv run --frozen --extra dev python /private/tmp/qi-misc-ssot-reaudit.3DPuax/selection_audit.py`.
It parses the actual Just recipe/catalog and calls the current static Python
import-closure owner; historical `selection_audit.json` records both test-file digests,
empty authority entries, false command membership and all four false closure
memberships. A second execution with final argument `selection-final.json`
records the concurrent repair: command/required closures now present, authority
entries still empty. Neither diagnostic collects or executes tests. The
preparation test itself also changed; do not reuse its earlier digest as current.

Pre-repair diagnostic command, run on clean `66cee47e`:

```sh
PYTHONDONTWRITEBYTECODE=1 /private/tmp/qi-rbr-guard-final.kmqdr8/venv/bin/python3 /private/tmp/qi-sep27-final-ssot.rOKWvq/probe.py
```

- Driver SHA-256: `563936e97402fc6e639c83176d669542af66ac869903b8c6efe31f592fc38a7c`.
- Result: `/private/tmp/qi-sep27-final-ssot.rOKWvq/result.json`, SHA-256
  `abd6e4e473085cb3cd17c2331fa8d520de87fd0234c3f2e944e249e94433a938`.
- Raw: `/private/tmp/qi-sep27-final-ssot.rOKWvq/raw.log`, SHA-256
  `48805c99494fa9c6fe57a26bed05cfc92a96f569ab35b25311e9ef543555469e`.
- Six implementation/test inputs were unchanged before/after; their hashes
  and the dirty inventory are in the result. Python 3.13 private runtime;
  fixed native-artifact fixtures with injected second-family failure and GC.
  Driver exit 0 means the diagnostic executed, not that the invariant passed.
- GC was controlled from a competing thread using the real store; five run
  directories were removed, promotion returned 0 and validation returned 2.
  This establishes a reachable counterexample, not cross-process coverage.
- Excludes actual producers, quiet-host measurements, full-suite execution,
  clean-source qualification and hostile same-UID filesystem attestation.
- `latest` is advisory. Its change alone is NOT a defect. The required
  invariant is complete-profile publication/custody, not advisory rollback.

### Pre-repair local audit checks and limitations

Executed on clean `66cee47e`; 231 selected/executed/passed, zero
failed/skipped, 40.68 seconds. These include the three C4 modules when explicitly
selected; a local pass does not repair their omission from the canonical rail.

```sh
uv run --frozen --extra dev python -m pytest tools/ci/tests/test_benchctl.py tools/ci/tests/test_benchmark_profile_capture.py tools/ci/tests/test_benchmark_evidence_bridge.py tools/ci/tests/test_bench_protocol_conformance.py tools/ci/tests/test_benchmark_source_closure.py tools/ci/tests/test_producer_notifications.py tools/ci/tests/test_bootstrap_cache.py tools/ci/tests/test_proof_command_timings.py -q -p no:cacheprovider --junitxml=/private/tmp/qi-sep27-final-ssot.rOKWvq/owner-tests.xml
```

Raw output is `owner-tests.log` in that external audit directory. The
`before.json` records the clean revision/tree, 1,152 source file hashes,
Python executable/version, installed dependency versions and relevant override
environment. The external audit receipts bind raw/JUnit/document hashes,
exact documentation/policy gate commands, historical artifact hash checks,
deletion census and this audit's only owned changed path. `audit-result.json`
retains the first gate attempt; `final-result.json` records the final document
identity and gate attempt without overwriting it. These are evidence artifacts, not
additional planning documents. Missing/tampered evidence cannot support a claim.

The first repository-wide doc-path check `FAILED`: another writer created
`docs/plans/sep-27-code-search-remediation/` during this audit, with 12 unresolved
RFC links. Its new untracked files are not stale documents in this deletion
scope and were left untouched. Benchmark policy, test-authority, generated-doc
lint and `git diff --check` passed. That failure is historical, not the current
documentation-gate status; use the final documentation audit's exact terminal
result for its bound source.

Status `VERIFIED` is limited to those local checks on their recorded pre-edit
source. Updating this normative document changes both source closures; the
result is not a final-source receipt, full suite, actual producer capture,
performance, installed-platform or hosted-CI qualification. C1 was `FAILED`
despite those passing tests: the old suite did not enforce its missing
complete-native-profile invariant. The new regression proof is separate below.
Final-source rails remain `NOT_RUN`.

### Code-first checkpoint: publication and coverage wiring

- MISC-01 extends `profile_capture.publish_capture` as the single prepared-run
  transaction owner. Native, Criterion, retrieval-contract, lexical, pair and
  recorded adapters retain domain preparation/replay but no longer duplicate
  run/pointer publication loops. All producers finish outside store custody.
- It validates the complete family/case/source inventory before writes, creates
  the capture marker before the first run, holds custody through all raw-domain
  replays and the final source check, then commits once. Native validation binds
  the current registry and one capture ID/digest; newer uncommitted runs cannot
  shadow it. Failed second-family/source/replay epochs preserve the prior pointer.
- Rust `RunStore::collect` now acquires `.custody.lock` before marker inspection,
  closing the earlier pre-marker race. A no-follow regular single-link lock and
  symlink-free POSIX root are required. Non-POSIX GC refuses explicitly rather
  than using an uncoordinated collector. Native Windows pair remains excluded.
  `rustix` reuses the workspace's locked 1.1.4 dependency; no wire format changed.
- New tests exercise a real competing Python GC process, actual Python-flock /
  Rust-collector interoperability, the Rust lock/marker race, coherent forged
  payloads, missing pointer, newer orphan runs, second-run
  failure, final-source failure and actual process exit immediately before/after
  capture/pointer publication. Existing symlink, archive and staging refusals
  remain in the selected adapter suites. A failed response after pointer commit
  resolves to the exact new complete capture, not rollback of immutable history.
  Both native fixture profiles replay every promoted run through the public CLI
  in fresh processes; this is fixture-backed replay, not actual producer capture.
- C4 enrolls three regression modules in the canonical command and authority;
  all three enter the benchmark closure, evaluator/timing tests enter retrieval,
  and derived closures inherit them. Guards require unique/nonempty actual
  collection and invalidate a bound closure when an owner test changes.

External local evidence root: `/private/tmp/qi-misc-implementation.csCnZf`.
`c1-red.log` preserves both pre-patch pointer failures; `rust-gc-red.log` preserves
the pre-patch collector race failure. `publication-final.xml`/`.log` report 300
passing Python cases across the six adapters and control-plane owners (one
intentional duplicate-ZIP fixture warning). `rust-interoperability.log` records
51 executed Rust contract cases, not the zero-case library/doc harnesses.
`rust-interoperability-clippy.log` records warning-denied all-target checking. Later added
collection/source-mutation guards are covered by the separate owner closeout;
do not add counts across these overlapping selections.

`owner-closeout.xml`/`.log` report 180 passing focused cases, and
`fresh-native-replay.xml`/`.log` report two passing fixture profiles with fresh
CLI replays. `implementation-receipt.json` binds this checkpoint's exact
commands, terminal counts, code/diff/runtime identity
and artifact hashes. These are dirty-source owner receipts, not MISC-04 clean
integration, actual SDK/model/product execution, quiet-host measurements or
cross-platform qualification. At that checkpoint MISC-02/03 remained outstanding; one costly
runtime batch follows the complete owner-local repair set, not this checkpoint.

### Bounded-I/O checkpoint: verification and control-file custody

- `evidence.py::_consume_regular_file` extends the existing no-follow descriptor
  owner. Raw materialization, control reads and hashing share one ancestor/leaf
  identity check. The opened descriptor must match the prechecked inode, mode,
  link count, size and change epoch before and after consumption. The underlying
  helper still requires complete consumption; no ZIP-seeking exception was added.
- `RunStore._verify_raw` and both existing bridge hash helpers consume 64 KiB
  chunks and bind full SHA-256 plus exact byte count. They no longer materialize
  raw payloads to validate them. At that checkpoint this did not change byte-based staging,
  native preparation, ZIP packing/unpacking or producer log accumulation.
- Evidence, latest, baseline, capture and profile-pointer documents have a
  16 MiB byte ceiling. All corresponding reader/GC paths use the same bounded
  control reader. Atomic control writes and exclusive capture writes check the
  same ceiling before modifying the destination, so an oversized capture cannot
  leave an unreadable immutable reference that blocks GC. This is an I/O
  resource limit, not a wire-field/schema migration or a corpus payload limit.
- Owner tests cover the exact 64 KiB boundary, empty inputs, digest/count
  equality, oversized control input before decoding, inclusive byte limits,
  unchanged prior pointer on oversized writes and GC safety after rejection.
  Mutations cover same-byte inode substitution, growth, truncation, restored
  mtime, parent replacement and hardlink replacement across all three consumers.
- Fresh child-process hash measurements at 8 MiB and 128 MiB raw sizes observed
  peak RSS 21,938,176 and 24,199,168 bytes respectively. The fixed read-size oracle
  and these measurements support the hasher's bounded payload buffers only;
  they do not prove archive/log/capture memory bounds or performance qualification.

External evidence: `/private/tmp/qi-misc-bounded-io.JlIU83`.
`red.xml` / `red.log` preserve two pre-fix behavioral failures: an unbounded raw
read and oversized evidence reaching the decoder. `writer-red.xml` / `.log`
preserve the later writer asymmetry and retained oversized capture failures.
`adapters.xml` reports 363 selected/executed/passed, no failures/errors/skips,
across nine store/publication/native/adapter modules (one deliberate duplicate-ZIP
fixture warning). `adapters-receipt.json` SHA-256:
`282fa3c73bda71c54c02b951cc42b9c480dfe2be29e77fe6e39ee6caf3aa0ac6`.
Its full tracked-source before/after hashes match; actual Python/package identity,
exact argv, raw/JUnit hashes and RSS properties are bound there. The subsequent
writer correction has 121 passing conformance/publication cases in
`writer-green.xml`, SHA-256
`0526b9280995bd8399a70335c255f94929a657d8d1998d436c2625c79531b823`.
These overlapping selections are not additive. Final owner revalidation is
retained separately in `closeout-owner.json`; post-edit policy gates are in
`policy-receipt.json`. None is a clean-source or actual-producer runtime batch.

The subsequent checkpoint below supersedes the earlier staging/bridge gap.
At that historical checkpoint, remaining MISC-03 work was deterministic streamed archives with declared
inventory/byte limits; file-backed execution logs including cleanup; capture-wide
failed-epoch records; remaining adapter and JSONL/consumer materialization; and
end-to-end memory measurements. Do not label MISC-03 complete from component
bounds. MISC-02 native execution/host integration is still `NOT_RUN`.

### File-backed raw checkpoint: one promotion contract and failed-stage custody

- `evidence.RawFile` carries an absolute safe path, SHA-256 commitment and exact
  size. Construction rejects malformed scalar/path identities; consumption
  revalidates actual pinned bytes. `capture`, `copy_to` and `write_raw_file` own
  capture/copy/spool semantics. The spool takes producer blocks, slices writes
  to 64 KiB, uses exclusive no-follow output creation, flush/fsync and post-write
  byte verification. It retains partial output and reports short write, disk
  full, interruption or invalid stream blocks instead of issuing a reference.
- `StagingRun.write_raw` accepts only a `RawFile`, with a canonical flat raw
  destination. Its copy checks both digest and count against the prepared
  commitment. The old bytes parameter is removed, not retained as another mode.
- `evidence_bridge.promote_native_run` accepts only a nonempty named `raw_files`
  inventory. Native, Criterion, retrieval, lexical, pair and recorded adapters,
  direct fixture producers and their tests migrated together. The old
  `native_path`/`native_bytes`/`additional_native` promotion contract is gone.
  Duplicate native destination names refuse before dictionary construction.
  Declared raw order is retained: sorting filenames changed concurrency row
  order on fresh replay and was explicitly removed after its failing regression.
- Retrieval proof binaries/native files and lexical native files are file-backed
  during preparation. Pair binary freeze/replay uses streamed copies/hashes and
  corpus bundles are referenced without complete reads at promotion preparation.
  Other producer outputs still arrive as bytes and are spooled in external work
  directories; this checkpoint does not claim to bound their upstream allocation.
- A bridge failure retains `.staging/<run-id>` and writes a bounded failure
  record, including cancellation. Failed staging cannot be promoted even if raw
  and envelope bytes are otherwise valid. If failure recording also fails,
  preserve the primary exception as the cause and report the secondary failure;
  do not claim a persisted marker. No automatic deletion or changed GC retention
  policy is introduced; successful immutable-run publication is unchanged.

Owner proof root: `/private/tmp/qi-misc-file-raw.xHC7fo`.
`red.xml` / `.log` reproduce deleted diagnostic raw after a failed promotion.
`owner-final.xml` / `.log` retain the introduced raw-sort replay regression
(one failure, 249 passes); `owner-repaired.xml` / `.log` record the same four-module
rail after its semantic repair: 250 selected/executed/passed, no failures/skips.
Repaired JUnit SHA-256:
`ebb018d28492b482b0e2878dab4938a71cb545e6c104c28ec3fa77d01b6a3352`.
The earlier five-adapter rail has 145 passes (one deliberate duplicate-ZIP
fixture warning); that is earlier component evidence, not composable final-source
closure. `closeout.json` records the final combined owner/adapter/selector run
with before/after tracked source hashes, runtime/packages, exact commands,
terminal counts, raw/JUnit hashes and exclusions. Do not add overlapping counts.

The new read-size oracle covers staging as well as store verification. Fresh
process measurements hash and copy 8 MiB / 128 MiB inputs, requiring full digest
and size equality and less than 32 MiB RSS growth for the 120 MiB payload increase.
JUnit properties retain actual measurements. This is local component resource
proof, not a bound for ZIP/producer logs, quiet-host timing or a qualified run.

The next execution transition is implemented in the checkpoint below. Pair
pack/unpack must still consume the same file commitment without whole ZIP or
entry buffers. No byte-returning execution compatibility API is retained.

### File-backed execution checkpoint: normal, cleanup and interrupted logs

- `evidence.RawWriter` is the one exclusive no-follow incremental sink behind
  both `write_raw_file` and producer pipe drains. Writes are unbuffered and
  capped at 64 KiB per operation; finalization verifies digest/count, descriptor
  facts and output namespace. An acknowledged drain must not remain only in
  Python buffers when the controller is killed. No power-loss durability claim.
- `producer_execution.execute` requires a fresh caller-owned external `log_dir`
  and returns `ExecutionResult(stdout: RawFile, stderr: RawFile, command)`.
  Both normal and cleanup drains use selectors and the same sinks; no
  bytearray accumulation or `communicate` path remains in this owner.
  Preserve SIGCHLD notifications, parent lifeline, kill-before-reap process-group
  identity and finite cleanup; escaped sessions remain an explicit exclusion.
- Every completed controller attempt retains stdout/stderr and an execution
  record with request, actual known terminal, error class and byte commitments.
  A timeout has no fabricated terminal. Nonzero/interrupt/cleanup/recording
  failures raise; a secondary output/record failure retains the primary cause.
  A killed controller may leave raw prefixes without a terminal record, never
  a success claim. This is per-command custody, not the missing capture-wide
  preparation/publication failure record.
- Criterion, retrieval, lexical, pair, corpus Git and portable-proof callers
  migrated together. Diagnostic Git/tool probes retain fresh external log
  directories; capture commands use their external work epoch. Criterion keeps
  logs as references and concatenates stderr by streamed committed bytes;
  replay reads only required size-limited control files, not unused large logs.
  Binary identity uses bounded hashing. Existing wire command/raw shapes remain.
- `RawFile.read_control` enforces the existing 16 MiB control limit plus exact
  commitment; `tail` retains at most one chunk while hashing all input bytes.
  Portable proof still returns bounded control bytes to existing consumers.
  This explicit refusal is not completion of IO-3: large JSONL, metadata and
  native artifact consumers still need their planned streaming cutover.

Proof root: `/private/tmp/qi-misc-execution-files.i6Epmu`.
`red.json` runs the actual `106d7abe` producer preimage with 8/128 MiB output:
RSS increased 256,851,968 bytes for 120 MiB additional payload, failing the
32 MiB component limit. `buffered-red.xml` preserves the new sink's initial
controller-SIGKILL prefix loss; `buffered-green.xml` records its repaired test.
`owner-first.xml` preserves the introduced raw-open error-class regression;
the repaired four-module owner rail has 182 passes in `owner-repaired.xml`.
The first eight-module caller rail has 206 passes in `adapters-first.xml`.
These are intermediate, overlapping results, not additive final qualification.
`closeout.json` binds the later combined selected owner/adapter tests, exact argv,
raw/JUnit hashes, RSS properties and before/after source/runtime identity. Use
its actual terminal outcome and drift inventory, not the presence of this path.
The combined terminal is 617 selected/executed/passed, zero failure/error/skip,
with two intentional duplicate-ZIP fixture warnings. Receipt SHA-256:
`67732501b4c38524b8d650de0768082d63a9b923170da4933f7a994475fe870b`;
JUnit SHA-256:
`e35f866ee93bf75549f2c10488d3788b8388d7ef85e0fb2277770f9d9e19cef1`.
Execution peak RSS at 8/128 MiB output was 26,509,312 / 25,853,952 bytes;
the comparison checks a component bound, not a performance improvement claim.
HEAD and this batch's Python source/test bytes were unchanged during execution,
but concurrent engine files and `tools/ci/test-authority.toml` changed. The
five subsequent policy commands exited zero; do not compose this mutable
whole-tree epoch into MISC-04 qualification. `post.json` records the targeted
policy/source-closure rerun after this status update, separately from runtime.

At the execution checkpoint, pair/corpus archives were still open. The archive
checkpoint below supersedes that specific gap. Native `benchctl.py` dispatch/
host monitoring remains MISC-02; residual preparation/JSONL parsing,
capture-wide failure records and end-to-end memory proof remain MISC-03.
Heavy MISC-04/05 runs wait for the complete owner-local repair set.

### File-backed archive checkpoint: both domains and replay reuse

- `raw_archive.py` is one ZIP mechanics owner used by pair and corpus binding.
  Both writers now return `RawFile`; both readers consume it. No bytes-returning
  compatibility path, whole-tree byte dictionary or complete-entry read remains
  in these archive boundaries. Domain owners retain exact inventory and native
  scorer/Git reconstruction responsibilities.
- Writers use the existing exclusive no-follow `RawWriter`, sorted canonical
  names, fixed 1980 timestamp, stored regular entries and fixed mode metadata.
  Input file commitments are verified during streaming. Pair tree inventory is
  checked again after packing; source/output overlap refuses before writes.
- `RawFile.consume_seekable` hashes all pinned bytes before seeking and again
  after the callback. The same open descriptor, ancestor/leaf identity and
  change-epoch checks remain active; the complete-read primitive still observes
  consumed EOF. This adds I/O passes, not an unmeasured performance claim.
- ZIP end/ZIP64 records and central row bounds/counts are checked before
  constructing `ZipFile`. A forged small count cannot hide an oversized actual
  central inventory. Sorted names, duplicate/prefix aliases, NUL truncation,
  multi-disk, links, encryption, compression, CRC/truncation and unsafe output
  ancestors are refused. Extraction streams at 64 KiB per payload read and uses
  exclusive output creation; failure leaves diagnostic partial output only.
- Resource policy is explicit: pair ZIP at most 64 GiB, corpus ZIP retains
  its existing 256 MiB ceiling, each archive at most 100,000 entries and a
  16 MiB central directory. Pair discovery also caps total directory/file
  entries at 100,000. These are refusal limits, not a claim that a 64 GiB
  product benchmark has executed. ZIP64 is supported; metadata memory can
  scale with admitted entry count, never with the total entry payload.
- `_ReplayWorkspace.restore` hashes both incoming archives on every case,
  retaining only digest/count commitments across cases. Restored workspace
  regular files (including Git objects) and pair executable postchecks use the
  same streaming digest owner. Existing corpus/native/mode/link/extra-file and
  archive-change refusal tests remain applicable.
- Corpus/lexical capture, replay and input-digest construction migrated together.
  Shared archive imports automatically join the existing static source closure;
  a regression traces both domain roots through archive/evidence/descriptor
  owners. No shared engine-owned selector or closure file was edited here.

External owner evidence root: `/private/tmp/qi-misc-archive-stream.Bpxru4`.
`red.xml` records the actual preimage's time-dependent ZIP identity failure.
`adapters-first.xml` records 82 passes and five failures from the introduced
lexical bytes-to-file digest migration omission; both capture/replay digest
sites were then corrected. `archive-owner.xml` records an intermediate 23-pass
archive selection, not the final complete adapter scope. `owner-final.json`
binds the subsequent nine-module command, raw/JUnit, source/runtime/dependency
identities, drift and RSS properties: 368 selected/executed/passed, zero
failure/error/skip, two deliberate duplicate-ZIP fixture warnings. HEAD and all
14 selected owner/test file hashes were unchanged across this run; engine and
normative-document files changed, so this is not whole-source qualification.
JUnit SHA-256:
`7edcdfff6fcfc9f8d92b45fddbc4308127e0dd8702d102a64eb12ccdf278c6d4`.
Pack peak RSS at 8/128 MiB: 26,427,392 / 27,820,032 bytes; unpack:
27,557,888 / 27,574,272 bytes. These support component payload-memory bounds,
not speed or full capture resource qualification. `post.json` binds the later
documentation/policy-only checks after this normative update. The selected rail covers
fixture-backed adapters and real Git reconstruction, not a live product pair,
installed daemon, hosted CI or performance admission.

The owner suite includes independent fixed ZIP payload/metadata checks,
inclusive byte/count limits, forged central metadata rejected before allocation,
forced-small-threshold ZIP64, original-source relocation, 2,000-entry roundtrip,
descriptor/namespace mutation and separate-process pack/unpack RSS at 8/128 MiB.
Large failure-path output, many-file metadata scaling and end-to-end capture
measurements remain IO-5; do not infer them from payload-only RSS measurements.

### Bounded JSONL checkpoint: recorded outcomes and Criterion Cargo output

The actual recorded-import preimage read the full agent JSONL before calling
the existing evaluator, which then read the file again. Criterion decoded and
split the full Cargo build log; its execution caller's 16 MiB control reader
also refused otherwise valid larger logs. The coordinated repair is:

- `RawFile.consume_lines` owns bounded binary `readline`, incremental SHA/count,
  complete-consumption enforcement and the same pinned descriptor/namespace/
  epoch checks as the other raw consumers. Default and maximum line size is
  16 MiB including any newline. It never silently drops a final partial line;
  the domain must parse it. A valid final JSON value without LF is allowed.
- `agent_outcome::load_file` validates each original trajectory through the
  existing strict owner, discards only that validated trajectory, and retains
  metadata needed for exact global task/trial/arm pairing. Retained metadata
  has a separate cumulative 16 MiB serialized-ASCII budget. This is not a
  16 MiB RSS bound: Python object overhead and domain row decoding still exist.
  Invalid UTF-8, duplicate keys/records, blank rows, nonfinite values, malformed
  trailing JSON, conflicting task identity/baselines/configuration and incomplete
  A/B/C groups continue to refuse. No pair is dropped or defaulted to success.
- `recorded_capture` passes file commitments directly to that owner and to
  promotion. Replay uses the same API; input digests bind those committed bytes.
  Scan summary JSON and derived agent-summary replay use the explicit 16 MiB
  control-document reader. Imports remain unauthenticated diagnostics, not
  proof that an underlying agent or scanner actually ran.
- Criterion capture and replay pass the committed build log to `_binary`.
  It retains one matching executable/features tuple and one successful terminal
  event, rejecting duplicate artifacts, duplicate terminal events and Cargo
  object messages after `build-finished`. Routing diagnostic lines remain
  outside Cargo-message semantics. UTF-8 and JSON parsing stay strict for
  recognized Cargo rows. Criterion listing reads are bounded control reads;
  sample/estimate derivation and diagnostic-only admission remain unchanged.

Owner evidence root: `/private/tmp/qi-misc-jsonl-stream.RvMCS9`.
`red.xml` records the actual preimage failing the no-whole-agent-input oracle.
`stream-focused.xml` records 79 passed / 171 deselected before the final two
adapter-control regressions. Final driver command:

```sh
uv run --frozen --extra dev python /private/tmp/qi-misc-jsonl-stream.RvMCS9/verify.py owner-final
```

The driver executes the canonical frozen Python environment with pytest over
these thirteen `tools/ci/tests/` modules, `-q -p no:cacheprovider`, legacy JUnit
properties and a bound external `--junitxml` path: `test_agent_outcome_benchmark`,
`test_recorded_capture`, `test_criterion_capture`, `test_bench_protocol_conformance`,
`test_benchmark_evidence_bridge`, `test_benchmark_profile_capture`, `test_benchctl`,
`test_retrieval_capture`, `test_lexical_capture`, `test_pair_capture`,
`test_portable_proof`, `test_benchmark_source_closure`, `test_benchmark_policy`
(all `.py`). `owner-final.json` binds the exact argv, Python/dependencies,
before/after source and dirty-state inventories, raw/JUnit hashes, observed
RSS properties and drift. Result: 606 selected/executed/passed, zero
failure/error/skip, exit 0; driver duration 129.85 seconds. HEAD remained
`98601a66d8cab9c86232b3e62ce490c8b43b71b6`; all 17 selected owner/test files
were unchanged. Concurrent engine and normative-document changes exclude
whole-source qualification. JUnit SHA-256:
`6d93bab01adfa9151a04b3fc4ca8123bef864fca24e4461169fc8d4a429eddde`.

Fresh-child-process component measurements (bytes, not latency):

| Consumer | Small input / peak RSS | Large input / peak RSS | Oracle |
| --- | --- | --- | --- |
| Recorded agent | 7,831,130 / 29,114,368 | 133,131,430 / 40,665,088 | 40 and 680 exact paired trials; three records per trial, no excluded/unknown pairs; RSS delta below 32 MiB. Global pair metadata legitimately grows. |
| Cargo build | 7,685,698 / 29,442,048 | 130,654,018 / 29,917,184 | One exact executable/features tuple and successful final event; RSS delta below 32 MiB. |

Further decisive controls cover zero/invalid/oversized line limits, oversize
before domain decode, forged commitment, short-circuit consumers, six mutation/
namespace races, LF/CRLF/no-final-LF, independent pair-metadata overflow,
symlink CLI input, oversized scan JSON and replay of a build log larger than
16 MiB. Existing paired-summary, fresh CLI replay, canonical wire, archive,
publication and source-closure regressions remain in the combined selection.
`post.json` binds subsequent document/policy/format-lint checks and the old-doc
deletion census. No Rust, actual producer, installed daemon, hosted CI,
qualified pair, end-to-end memory or latency result is claimed by this run.
At that checkpoint native/lexical/portable-proof reads remained open. The next
checkpoint supersedes native/lexical preparation only; whole-capture failure
custody, portable proof, native host execution and final source freeze remain.

### Native and lexical preparation: current implementation boundary

- Native artifact scanning, snapshot, preparation and detached replay share
  the validator's fixed fan-out ceiling: one summary per family; concurrency
  at most the three existing 1/8/32-client summaries. Overflow refuses before
  JSON parsing or accumulating the native artifact list. Existing exact
  concurrency identity/completeness checks still run; a ceiling is not proof
  of complete measurement.
- Native summaries are bounded 16 MiB control documents. The validator,
  capture/replay, diagnostic summary, DSL comparator and baseline-admission
  readers use the same pinned control reader. No large raw trace or corpus is
  reclassified as control JSON. Native preparation retains `(path, RawFile)`
  plus bounded decoded metadata, not another payload byte copy. Publication
  consumes those original commitments directly and rejects changed source
  bytes. The opaque run-ID suffix now hashes ordered name/digest/size metadata
  instead of concatenated complete payloads; persisted evidence schema is unchanged.
- Lexical capture freezes all nine role inputs by committed streaming copies.
  Specs, suites, query packs, pair report/lock/verdict/native summaries, origin,
  corpus binding and derived report are bounded control JSON. Product observation
  files (`sourcegraph_rows`, `opengrok_rows`, `cs_rows`) are JSONL, not a whole-file
  size-limited document. Their existing scorer consumes bounded lines through
  `RawFile.consume_lines`; source/binary/lockfile identities use streamed hashes.
- The scorer parses every row and scores only the declared `symbol_only` lane,
  as before. It retains per-query metrics, exact task coverage, hit/recall and
  latency denominators, not complete observations/responses. Cumulative
  serialized per-product metric metadata is capped at 16 MiB; this is not an
  RSS cap. Blank rows, invalid UTF-8, malformed trailing JSON and oversized lines
  refuse instead of being skipped. Valid final JSON without LF remains accepted.
  A post-scorer digest/count check rejects changed frozen inputs before publication.
- `rr` review includes all changed readers, capture-to-promotion references,
  detached replay, CLI/control-file refusal and the existing domain scorer.
  During migration the shared reader wrapped missing-file errors; DSL's public
  missing-artifact classification was restored from the actual OS exception cause,
  without a check-then-open pathname fallback. Current tests and policy results
  are recorded in the external checkpoint, not inferred from historical counts.

External work/evidence root: `/private/tmp/qi-misc-native-lexical.JGmZBo`.
`red.xml` contains two actual preimage failures through native publication and
lexical scoring: both attempted whole-payload `Path.read_bytes`.
`adapters-first.xml` has 142 passed / three failures: two incomplete validator
fixtures exposed by earlier inventory admission, plus a manifest recipe error
while shared Justfile was concurrently edited. The fixture wiring was repaired;
the recipe-error cause is not promoted to a product claim from that one run.
`adapters-second.xml` has 195 passed / two failures: the earlier symlink refusal
now has an explicit regular-file diagnostic, and the DSL missing-artifact error
classification needed repair. `repair.xml` has three targeted passes after
those repairs and the explicit lexical result-metadata overflow regression.
These intermediate counts are not final-source qualification.

The first combined `owner-final.json` run executed 519 cases with no failures;
an exception-chaining lint correction was then applied and the same module
selection was rerun, rather than treating its earlier source as current proof.
Final command:

```sh
uv run --frozen --extra dev python /private/tmp/qi-misc-native-lexical.JGmZBo/verify.py source-final
```

`source-final.json` records the exact nested frozen-environment pytest argv,
raw log/JUnit hashes, Python/dependency identity, before/after file digests and
dirty inventories. The twelve `tools/ci/tests/` modules are `test_benchctl`,
`test_lexical_capture`, `test_lexical_file_comparison`, `test_check_bench_artifacts`,
`test_compare_dsl_bench`, `test_benchmark_evidence_bridge`,
`test_benchmark_profile_capture`, `test_corpus_binding`,
`test_concurrency_sample_contract`, `test_bench_protocol_conformance`,
`test_benchmark_source_closure`, `test_benchmark_policy` (all `.py`), with
`-q -p no:cacheprovider -o junit_family=legacy` and external JUnit output.
Result: 520 selected/executed/passed, zero failure/error/skip, exit 0,
166.38 seconds driver wall time; one deliberate duplicate-ZIP fixture warning.
The extra selected case comes from the concurrently maintained closure-test
module between the two runs; it is not an additional new test authored by this
IO-3 change. Both runs retain separate source inventories.

HEAD stayed `98601a66d8cab9c86232b3e62ce490c8b43b71b6`; all 18 monitored
owner/test files were unchanged during the final run. Engine files, this
normative document and `tools/ci/source_closure.py` changed. Consequently this
checkpoint supports the exercised owner behavior, not final source-closure
acceptance, whole-source qualification, installed/real-producer execution,
hosted CI, qualified performance or an actual search-quality comparison.
Final JUnit SHA-256:
`d103b521856f821f2d8aa050d1fd85fb7edb43fbf85f18d0c952a6edadda2fe3`.

Fresh-process lexical scorer RSS was 28,590,080 bytes for 7,707,970 input bytes
(120 tasks), and 32,620,544 bytes for 131,038,290 input bytes (2,040 tasks).
Each test independently checks complete SHA-256, exact task/hit/row counts,
recall/hit rate 1.0 and mean latency 2.0 from fixed synthetic observations;
the RSS delta is below the explicit 32 MiB regression threshold. This measures
the scorer component only. It does not measure real search latency, full
five-product capture RSS, many-file scaling or failure-path resource cost.

New regressions also cover native reference-only preparation, source mutation
before publication, finite artifact fan-out before decode, oversized summaries,
leaf/ancestor aliases, lexical freeze/replay without whole observation reads,
frozen-input mutation, partial/invalid/blank trailing JSONL, row/control/retained
metadata ceilings and comparator oversize/missing-file behavior. Existing
native fresh-process replay, complete-profile pointer preservation, corpus Git
reconstruction, concurrency semantics and lexical five-product parity remain
in the executed selection. `post.json` records the later documentation/policy
checks; it cannot repair the broader source drift or close MISC-04.

### Paired-verdict command-log checkpoint

Owned changes are limited to the paired proof functions in
`tools/benchmark/retrieval/run.py`, import-mode support in
`tools/benchmark/raw_archive.py`, regressions in `test_portable_proof.py`, and
the independent sorted ZIP fixture/error assertions in
`test_retrieval_benchmark.py`. Concurrent symbol/engine changes in the latter
two shared production/test surfaces are preserved. Shared Just/closure/catalog
files are not edited by this checkpoint.

- `freeze_receipts` captures/copies receipts and executables through `RawFile`
  and packs transcript references through the shared deterministic archive
  owner. It rejects source aliases/mutation and never overwrites an existing
  destination artifact. It does not read a full transcript into bytes.
- `_verify_execution_context` bounds context, closure and collection controls,
  extracts transcripts with the shared metadata-first streamed parser into a
  private temporary directory, and retains only file commitments. Ordinary
  stdout/stderr are compared by complete-byte digest; the three native build
  controls retain existing Cargo/collection/binary-domain validation.
- Aggregate transcript payload remains limited to 64 MiB, inclusive. The ZIP
  envelope is capped at payload ceiling plus 64 KiB; central metadata is capped
  at 32 KiB and entry count at the exact rail inventory. Aggregate payload is
  checked after bounded extraction and before domain admission. Temporary
  extraction is removed on success or refusal. This is a bounded-disk tradeoff,
  not a claim of zero extra I/O or a measured whole-capture RSS bound.
- Sorted unique canonical entry order is now required for this archive too.
  Old unordered command-log archives must be regenerated from the producer
  evidence; no legacy reader or bypass is retained. Existing context/receipt
  schemas, exact command/environment checks and 64 MiB payload policy remain.
- Before success, rehash raw evidence, binaries, context, closure and the
  original command-log ZIP; recheck the exact binary directory inventory.
  A mutation after extraction/domain inspection must not reuse earlier hashes.
- Static import traversal was checked against current `retrieval/run.py` and
  discovers both `raw_archive.py` and `evidence.py`. Absence of a duplicated
  explicit root is not a source-closure defect; do not add redundant entries.
- Same-file caller audit also reproduced `validate_runner_bundle` reading an
  undeclared ZIP member before inventory refusal. `build_runner_bundle` and
  `validate_runner_bundle` now share the file/archive owner too: a 16 MiB
  archive ceiling, 16 KiB central-directory ceiling, exactly six members,
  source-file digest/size equality and final original-archive rehash. The
  generated `__main__.py` must match the prescribed bootstrap bytes even when
  an attacker rehashes all embedded and declared manifests consistently.
  The ordinary sorted stored zipapp remains executable by isolated Python;
  changed canonical metadata means old bundle digests are not reusable proof.

Owner regressions cover contract and SDK freeze/consumer paths with actual
20 MiB transcripts while prohibiting whole-file/ZIP-entry reads; exact payload
boundary; missing/extra/duplicate/reordered/aliased/compressed/linked/CRC-bad/
truncated/oversized ZIPs; pre-parser metadata refusal; source and destination
custody; 16 MiB control refusal; and mutations after successful domain checks.
Producer/tool execution and source-authority fixtures are synthetic, while
the actual file/ZIP/domain readers run. This does not establish SDK execution,
platform portability, independent quality, host quietness or performance.

Verification artifacts for this checkpoint live outside the repository at
`/private/tmp/qi-misc-paired-io.SAWvM7`. The initial four boundary regressions
failed against the old whole-file readers; the RED JUnit is
`/private/tmp/qi-misc-paired-red.xml`. First portable run: 100 tests passed;
subsequent final-input boundary run: 12 tests passed. These selections overlap
and are not additive. `bundle-red.xml` records the undeclared-member read
reproduction; `bundle-first.xml` records its repair plus the existing actual
isolated zipapp command test. Bundle refusal tests initially passed 8 cases;
final selection also includes post-domain archive mutation. Expanded
final-source owner proof is pending below.

The first expanded nine-module run selected/executed/passed 746 tests with
zero failures/errors/skips (`owners.json`, pytest exit 0, 692.15 seconds).
That run overlapped our final-input/bundle repairs and foreign engine/test
changes: `owners_and_selected_tests_stable=false`. It is diagnostic evidence,
not proof of the final code. Its receipt SHA-256 is
`85e84ddb2f4faf3a69ad988b5f8682d7da9d7dc398bc283b111d8d12bb02c565`.
The final rerun selects all cases in the other eight modules and only the
directly affected execution-context, frozen-command, freeze-receipt,
OS-attestation and runner-bundle cases in `test_retrieval_benchmark.py`.
This avoids repeating its unrelated expensive isolation/search-policy cases;
it is not a final-source whole-retrieval-suite claim.

### Portable producer, receipt and replay: earlier file-backed checkpoint

- `_run` and `_run_reused_nextest` now return retained `RawFile` outputs, not
  decoded control bytes. Metadata and inventory are bounded control documents;
  nextest events are streamed/copied. Reuse-build epochs, selected binary
  identities, tool/environment custody, command timings and the concurrent
  `l5_parser_regressions` selector additions are preserved.
- `portable_proof.validate` caches file commitments instead of all payload bytes.
  It compares complete-byte digest/size for raw/copy equality and revalidates
  every committed file before returning. Canonical receipt verification uses
  those same files and bounded summaries, retaining original paths on relocation.
- Contract/SDK/inventory readers share `evidence.read_control` for Path, raw-file
  and existing in-memory control inputs. Production event readers use the one
  `nextest_events` semantic parser over bounded lines. Each line and serialized
  identity metadata are limited to 16 MiB; inventories and JUnit XML are bounded
  before decoding. This is not a 16 MiB Python RSS claim. Runner/searchd binaries
  are streamed hashes, not control documents subject to that ceiling.
- The invoked `write-verification-receipt.py` also streams event/role hashes and
  preserves raw commitments through the final pre-publication check. Inputs
  changed after summary derivation refuse without a receipt. Its existing wire
  schema, selection/count and command/source rules are unchanged. Missing/bad
  inventory does not turn a failing event stream into success.
- `retrieval_capture.py` control readers are bounded; Cargo.lock identity uses
  streaming hashing. Its proof payload and detached domain replay are unchanged.
- Meaningful initial RED: a real subprocess produced 20 MiB successfully, but
  `_run` failed at its unconditional 16 MiB `read_control`. The new regression
  verifies the retained path, size and independent full-output hash.
  Other tests cover both portable branches with >20 MiB logs and relocated
  replay, real receipt CLI execution, 18 MiB runner bytes, per-line/control/
  metadata limits, no-follow inputs, mutation during parsing and before receipt
  publication. Large branch fixtures mock tool execution/source admission;
  they are not actual Rust/SDK or qualification proof.
- RR traced the paired consumer too: `retrieval/run.py::freeze_receipts` and
  `_verify_execution_context` still have their own command-log ZIP copies and
  payload dictionary. That same-boundary tail remains explicitly open below;
  the current patch does not claim end-to-end portable/paired I/O closure.

Evidence root: `/private/tmp/qi-misc-portable-io.47FSYA`.
`first.xml` records 174 passes; `second.xml` 185; `third.xml` 119 focused cases.
The first expanded run, `owner-final.xml`, records 623 passes and four failures:
old bytes arguments/assertions and an obsolete bytes-returning execution mock
in `test_portable_tool_execution.py`. Their file-reference migration retains
the independent tool-environment and output-digest substitution assertions;
`repair.xml` records 67 passes. Those intermediate scopes are not additive and
do not override the final source epoch.

Final command:

```sh
uv run --frozen --extra dev python /private/tmp/qi-misc-portable-io.47FSYA/verify.py source-final
```

`source-final.json` records the exact nested pytest argv across 17 modules,
before/after source hashes, dependencies, environment, raw log and JUnit hashes.
Result: **628 selected/executed/passed, zero failed/error/skipped**, pytest exit 0,
187.29 seconds including driver overhead; one intentional duplicate-ZIP fixture
warning. All eight changed production owners and all selected test modules
were byte-stable. Engine source and foreign handoff artifacts changed during
execution; no whole-source stability or integrated closure acceptance is claimed.
Python 3.13.9, macOS 15.6 arm64; no Rust/product benchmark was run.

- Receipt SHA-256: `6a875330a74f894b1c0fba3fc01561b5911cea3c1ba5d9f8568d13e4a53e72fa`.
- JUnit SHA-256: `10a5dcd3224d9f3ef6fedde67f5820ff672c57a19d11ab76ae1815c9fcbcaf12`.
- Reader RSS: 100 cases / 6,565,812 raw bytes / 37,732,352 peak bytes versus
  1,600 cases / 105,055,514 raw bytes / 40,042,496 peak bytes. Independent fixed
  case counts and full-file SHA matched through receipt and domain readers;
  peak growth is below the 32 MiB test threshold. This covers those readers,
  not all producer work, paired ZIP replay, failure paths or host qualification.

`post.json` binds documentation/policy checks after this normative update.
The latest `check-test-authority.py` run is `FAILED`: concurrent new files
`crates/quanta-index-contract-base/tests/l4_preview_emission.rs` and
`crates/quanta-index-contract/tests/l4_preview_wire.rs` are orphan integration
targets with no catalog entries. Their appearance is recorded in the owner's
before/after source inventory. Shared catalog edits belong to the active
integration/engine owners and were preserved. The other five checks (doc paths,
benchmark policy, prompt-manager lint, diff whitespace and touched Ruff) exit 0.
`note.json` binds this subsequent failure-status update and its doc-path/diff
recheck; it does not erase or retry the unchanged authority failure.
Historical receipts and fixture proofs do not close C5, actual SDK/contract
capture, both integrated consumers, IO-4/EXEC-1/IO-5 or MISC-04 through MISC-07.

### Historical completed frozen proof, not current-source qualification

Frozen HEAD `a66eb6b89f772ae71b871123456e654096beee7d`, tree
`3fa072de5f2fce5ed4beb17e53c33dfd9f3755ef`, clean at capture:

- CI: selected/executed/passed 1,655; zero failed/skipped; 15 dynamic subtests
  reported separately; 12 gates; eight optimized-Python terminal mutants refused.
- Native: SDK 18, Python 324, Rust 108 selected/executed/passed; terminal exit 0.
- Fresh validation plus two replays, both real receipt consumers and eight
  metadata refusals, five bound binary copies, independent source/runtime POST.
- Root receipt: `/private/tmp/qi-rbr-guard-final.kmqdr8/root-final-closeout.json`,
  SHA-256 `64f232c6298614636e25a3a4f96a50ba1e7cabb97280d2008e924a0f9a2bd77e`.
  All 76 referenced artifact hashes were checked by the original consolidation
  audit's external verifier, not rerun in this refresh. That check verified
  retained bytes, not execution of those commands on the current source.
  The receipt contains exact commands, environments, raw paths and hashes.
- Native capture: `retrieval-contract-a9bb876c8b28437b9f09d3ce50e40dfa`.
- Scope `VERIFIED`: those frozen local rails only. This migration changes
  source/contract identity; using that proof for the new source is `BLOCKED`.
  New-source qualification is `NOT_RUN`, not a newly discovered code defect.
- Full Rust workspace, installed lifecycle matrix, Linux positive isolation,
  qualified pair/performance, hosted CI and deployment were not established.
  The prior hosted billing block was not refreshed; do not assert it is current.

The external paths are evidence, not documentation dependencies. If absent or
changed, affected historical proof is `BLOCKED`; use a new capture, not a
similar filename/count. They do not become release authority.

### Do not reopen as implementation work

- Exact Python inventory 319-to-324 repair, live-collection guard, and the
  consumer helper correction are present; derive identities, never freeze 324
  as a permanent expected count.
- Query-observation parity and macOS PID/peak/sample validation are present.
  Measurement and platform qualification remain separate.
- Python store reader/writer no-follow, linked-parent refusal, exclusive raw
  creation, duplicate staging and hardlink protections are present. Preserve
  them; do not import an older store implementation wholesale.
- The 18 test-optimization findings below are implemented-owner invariants to
  requalify, not 18 open bugs. Reopen only a reproduced reachable regression.
- Workspace placement and the single Python CLI are decided. No second Rust
  CLI, broad layout rewrite, default-ranker change or optional symbol-authority
  expansion is authorized by this packet.

SIGCHLD-driven completion in `producer_execution.py`, pure bootstrap caching
in `retrieval/evaluator.py`, diagnostic command timings in
`retrieval/portable_proof.py`, and corpus/lexical test updates are now committed
in `66cee47e`; they are no longer pending dirty integrations. Preserve them.
Their focused regression modules passed and C4 canonical coverage wiring is
now repaired. The execution change does not replace the native
direct-subprocess path. Its former bytearray log accumulation is removed by
the later execution checkpoint above. Do not overwrite
event-driven completion with an older polling implementation. Diagnostic
command timings do not establish attributable speedup or quiet-host admission.

## 2. Architecture and common contracts

```text
benchctl + registry: select declared profile and route commands
  adapter: prepare inputs, run producers, invoke existing domain validator/scorer
    producer_execution: process/session/terminal/log/cleanup custody
    host_monitor: declared capture interval observations and local reservation
  profile_capture: complete-profile publication owner
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
- Proposed internal values: `PreparedRun` carries family/case/source/input/
  build/host/command/payload/raw references; `ExecutionResult` carries actual
  terminal state and bounded file-backed stdout/stderr references. Names may
  change; there must not be competing semantic representations.
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

## 3. Ticket map and dependencies

| Ticket | Owner lane | Depends on | Remaining outcome |
| --- | --- | --- | --- |
| MISC-01 | Publication/integration | Implemented; owner-local proof below | Final integrated-source actual capture/fresh replay under MISC-04; retain all publication regressions |
| MISC-02 | Execution/host | Shared execution/raw contract agreed | Owned producer lifecycle and capture-time diagnostic host evidence |
| MISC-03 | Evidence/data path | Verification/control/staging, execution and archive APIs implemented | Remaining preparation/parsing, capture-wide failure records and end-to-end memory proof |
| MISC-04 | Serial integration | Original C4 repaired; C5 authority/regression proof open; final integration after 01-03 | Exact owner selection/source custody, same-source gates, actual capture/replay and hosted/closure reconciliation |
| MISC-05 | Functional qualification | Stable integrated source | TOPT invariants, Rust/daemon, installed ingest and platform checks |
| MISC-06 | Measurement | 02/04/05 and admitted host/inputs | Distinct test-cost/query/ingest/micro/system measurements |
| MISC-07 | Profile/product coverage | 04; available external inputs | Complete execution inventory, live comparison, conditional outcome claims |

Implementation repair 02 is `NOT_RUN`; 03 is partial as detailed above;
01/C4 have historical owner-local proof only, not current-source qualification.
C5 is a partially repaired enrollment/proof gap within ticket 04.
Tickets 04-07 retain
unexecuted final-source acceptance scopes, not a claim that every underlying
implementation is absent. C1's pre-patch negative diagnostic remains historical `FAILED`; missing prerequisites
block only dependent claim scopes. User-owned inputs in section 11 are not
automatic coding subtasks. A result satisfying multiple tickets is referenced
by one capture ID; do not schedule duplicate execution.

### Deduplicated handoff disposition

| Former work identity | Single current disposition |
| --- | --- |
| A1/A4 G-01; A4-01 through A4-03 | Historical a66eb6 native/replay/consumer/POST work completed in its own scope; do not restart that epoch. New integrated-source closeout belongs only to MISC-04. |
| A4-04; A3-00/A3-01/A3-07 | Required-test repair and documentation consolidation landed. C4 selector/closure enrollment has owner-local proof; final integration remains in MISC-04; no wholesale branch import. |
| A3-02 | Existing no-follow/staging defenses retained. Native profile transaction is MISC-01; raw/archive/log memory is MISC-03. |
| A3-03/A3-05/A3-06; G-03 | Actual profile execution, accessible live comparator pilot and conditional authenticated/qualified claims are MISC-07. |
| A4-08 | Same admitted pair obligation as G-03 under MISC-07, not another run. |
| A3-04; A4-05/G-02 | Execution/host implementation is MISC-02; micro/system and observation measurements are separate MISC-06 tracks. |
| A2-01/A2-02; A4-06/G-05; A4-07/G-06 | Owner invariants, full Rust, installed ingest and platform qualification are MISC-05; their timing claims belong to MISC-06. |
| A2-03/A2-05 | TOPT test-cost measurement is MISC-06; final receipt reconciliation is MISC-04, not duplicate gates. |
| A2-04/A3-08 | User-owned historical admission choice, approved inputs and host access are section 11 prerequisites, not automatic coding tasks. |
| G-04; Windows/release expansion | Excluded unless separately authorized; no compatibility/ranking/symbol expansion hidden in these fixes. |

Execution order: retain the repaired C4/publication boundary; establish the
file-backed raw/result contract and repair 02-03 with owner-local negative tests; finish normative edits;
freeze once for the serial 04/05 runtime batch. Measurements and external-input
claims follow only on admitted hosts/data. Publication owns pointer/GC logic;
execution owns process/log custody; storage owns byte/descriptor integrity.
Overlapping `benchctl.py` and `evidence_bridge.py` edits have one integrator.

### Concrete implementation sequence and shared API boundary

The file-backed raw and execution-result boundaries are implemented. Reuse
`RawFile`, `RawWriter` and `ExecutionResult`; do not reopen their representation
or introduce an alternate bytes mode. Ticket numbering is not an instruction
to implement host monitoring before its archive/parsing/failure dependencies.

| Order | Change owner and logic | Acceptance before handoff |
| --- | --- | --- |
| 1 (retain) | MISC-03: implemented `evidence.py` pinned bounded consumption, `RawFile`, streamed exclusive staging and size-limited control reads. Existing descriptor guard remains the owner. | Preserve digest/size, read bounds, unsafe-link/change-epoch and disk-full/partial-failure tests; do not implement a second reader. |
| 2 (retain) | Shared 02/03 contract implemented: `producer_execution.py::execute` returns one file-backed execution result with terminal state, stdout/stderr references, byte counts/digests and bounded diagnostic tails. | Preserve real success/failure/timeout/interrupt/parent-death tests, bounded normal/cleanup drains and explicit retained-prefix semantics. |
| 3 | MISC-03: retain the migrated pair/corpus archives; finish remaining preparation/parsing on the same raw-reference contract. Bridge/hash helpers and all six promotion callers already use it. `profile_capture.publish_capture` stays the only complete-profile commit owner. | No hidden whole-tree dictionary, whole ZIP, unbounded JSONL or full-output decode in an accepted large-payload path; cross-adapter replay and canonical wire fixtures unchanged. |
| 4 | MISC-02: native `benchctl.py` dispatch joins the shared execution owner; `host_monitor.py` owns reservation/samples; bridge/replay rederive host facts from capture-bound raw observations. | Missing/forged/reordered observations refuse; cooperative reservation is diagnostic only; no static sample count and no new shell/producer wrapper. |
| 5 | MISC-04: one integrator updates selectors/source closures, runs affected owner tests, finishes this normative contract and freezes the combined source. | Each new test is actually selected and its source is bound; no omitted siblings or zero-case success. |
| 6 | MISC-04/05 then 06/07: batch expensive compatible source-bound rails once; reuse exact valid captures, keeping claim-specific prerequisites and denominators separate. | Actual producer/fresh replay, installed/platform and measurement evidence remain separately classified. |

Current direct callers of the execution API are `criterion_capture.py`,
`retrieval_capture.py`, `lexical_capture.py`, `pair_capture.py`,
`corpus_release.py` and `retrieval/portable_proof.py`. Native `benchctl.py` is
the missing producer integration, not permission to replace unrelated short
Git/toolchain diagnostic subprocesses. `recorded_capture.py` participates in
the raw-publication cutover even though it imports data rather than executes a
producer. Migrate their owner tests and mocks with the same contract; do not
leave a bytes-returning compatibility execution path.

### Remaining code work: exact integration boundary

These are substeps of MISC-02/03, not new parallel tickets or duplicate gates.
Each row has one owner; an execution caller migration and its mocks must land
together. Overlapping caller files are integrated serially. Earlier baseline
changes are in `106d7abe`, but the execution API/caller/test migration remains
dirty, including `portable_proof.py` and pair tests. Preserve both layers and
check ownership again before new edits.

| Step | RCA and exact code boundary | Required logic and decisive check |
| --- | --- | --- |
| IO-1 (implemented; retain proof) | `producer_execution.py::_wait_for_terminal/execute/_cleanup` and `evidence.RawWriter` now stream both pipes and cleanup into files. | Keep one execution result; exact counts/digests, bounded tails, SIGCHLD/lifeline and kill-before-reap custody. Retain large-output success/nonzero/timeout/interrupt/parent-death and incomplete-cleanup controls. Current-source final integration remains MISC-04. |
| IO-2 (implemented; retain proof) | `raw_archive.py`, `pair_capture.py::pack_native/tree_files/unpack_native/_ReplayWorkspace` and `corpus_binding.py::capture/_replay` share one streaming file contract. | Preserve fixed ZIP metadata, bounded central allocation, pinned complete-byte seeking, count/byte limits and independent payload/relocation/refusal tests. Current-source qualification remains MISC-04; whole-capture memory proof remains IO-5. |
| IO-3 (implemented; retain proof) | Native/lexical/recorded/Criterion, portable producer/receipt/domain replay and paired-verdict command-log paths use file references, bounded controls and streamed events. | Retain the 64 MiB aggregate log admission limit, exact command/entry inventory, binary/reuse-build binding and same-raw parity. Shared ZIP parsing admits bounded metadata before allocation and streams into owned temporary files. Full-source and whole-capture resource acceptance remain separate. |
| IO-4 | `profile_capture.py::publish_capture` records no whole-capture failure across all preparation/replay phases; bridge failure custody covers only one staging epoch. | One capture-level failure record binds phase, source/input identity, retained logs, terminal and primary/cleanup errors outside success inventory. Cover failures before staging, second-family publication, domain replay and final source check; preserve the prior complete pointer. Marker-write failure must report both errors without claiming persisted evidence. |
| EXEC-1 | `benchctl.py` native dispatch bypasses shared execution; `promote_profile_runs` assigns `shared` and one sample; no `host_monitor.py` exists. | After IO-1, route native producers through the same lifecycle. Observe capture-bound host facts and cooperative reservation; bridge/replay rederive them. Test sample omission/reorder/gaps/stale capture and forged summaries. Cooperative reservation must never issue qualified-performance proof by itself. |
| IO-5 | Fresh-process RSS tests cover digest/staging, producer logs, shared archive pack/unpack and recorded/Cargo line consumers; each checkpoint binds actual outcomes. | Remaining: large failure-path output and many-file metadata/end-to-end costs. Reuse one complete owner suite after shared API integration, then the serial runtime batch. Do not infer end-to-end bounds from component measurements. |

The following caller details are mandatory parts of those rows, not additional
tickets. They prevent a streamed writer from hiding another whole-payload copy.

| Step | Additional live owner/caller | Structural completion condition |
| --- | --- | --- |
| IO-2 retained invariant | `pair_capture.py::_ReplayWorkspace.restore` now retains only complete-byte digest/count commitments for the bundle and ZIP. | Revalidate on every case, even if path/size/mtime are unchanged; preserve cross-case identity without whole archive retention. |
| IO-2 retained invariant | `_replay_tree_identity` and executable postchecks now use streamed hashes. | Retain path/mode/link inventory and workspace mutation refusal; no metadata-only cache or weakened binary postcheck. |
| IO-2 retained invariant | Corpus capture/replay and lexical callers now use one `RawFile` capsule API. | Preserve exact metadata/bundle inventory, real Git reconstruction, 256 MiB ceiling and input digests; no private bytes compatibility branch. |
| IO-3 retained invariant | `recorded_capture.py` passes committed `RawFile` imports directly; `agent_outcome::load_file` removes validated trajectories and retains only bounded pair metadata. Criterion `_binary` consumes bounded lines directly in capture and replay. | Keep full-file consumption/SHA/count/epoch verification; 16 MiB per line and serialized pair-metadata budget, exact A/B/C pairing, duplicate and partial-JSON refusal. Scan JSON and Criterion listings remain bounded control documents. Do not regress to a bytes compatibility path. |
| IO-3 retained invariant | Native `_capture_native_family` holds committed files and bounded metadata; validator/snapshot/replay share the finite inventory ceiling. Lexical capture copies references and the scorer streams observation rows; all named control inputs are bounded. | Retain exact digest/count and mutation refusal, native/lexical replay parity, alias/oversize/partial-row negatives and source-independent expected score/count tests. No whole-payload spool or second parser owner. |
| IO-3 retained invariant | Portable `_run` returns `RawFile`; production copies files, `validate` retains commitments and final-rechecks them, and canonical/domain readers consume those exact files. | Preserve bounded control parsing, streaming events/hashes, reuse-input epochs, all real consumers and file-returning test doubles. No full-log bytes cache or hidden whole-output decode. |
| IO-3 retained invariant | `retrieval/run.py::freeze_receipts` copies committed files and packs through `raw_archive`; `_verify_execution_context` hashes extracted references and passes only three bounded build/metadata/collection controls to the existing domain reader. | Keep exact expected names/digests, the inclusive 64 MiB payload ceiling, 64 KiB envelope allowance, 32 KiB central-directory limit, sorted entry inventory, executable/collection roles and final raw/binary/context/closure rehash. Old unordered archives require re-freeze; no legacy parsing branch. Preserve concurrent engine edits outside these functions. |
| IO-4 | Adapter `capture` preparation occurs before `profile_capture.publish_capture` is entered. | Establish one capture epoch before the first fallible preparation action. Its owner covers preparation, execution, staged/promoted runs, domain replay and final source check. Publication alone cannot record pre-entry failures. Keep raw failure artifacts separate from admissible run inventory and preserve the prior complete pointer. |

IO-3 portable core matrix: the following rows are implemented at the checkpoint
above and describe retained requirements. The paired-verdict ZIP row immediately
above is implemented as well; whole-capture RSS and integrated qualification
are not implied by these owner-local implementation checkpoints.

| File and function owner | Logic to change / preserve | Existing regression owner |
| --- | --- | --- |
| `tools/benchmark/retrieval/portable_proof.py::_run/_run_reused_nextest/_produce` | Return committed output references; callers explicitly choose bounded control decode or streamed copy. Nextest events are JSONL; the binaries-only list and Cargo metadata are control JSON. Preserve selected binaries, metadata/reuse epoch identity, tool custody, resource admission and command timings. Ignored return values must not force output decoding. | `tools/ci/tests/test_portable_proof.py`, `test_cargo_preparation.py`, `test_proof_command_timings.py`, `test_portable_tool_execution.py` |
| `tools/benchmark/retrieval/portable_proof.py::validate/_canonical_receipt` | Replace `captured: dict[str, bytes]` with immutable file commitments. Derive digest/count from fully consumed guarded files; compare raw/copy identity without retaining both payloads. Preserve relocated execution-root provenance, exact evidence-role inventory, command identity and pending-context rejection. | `tools/ci/tests/test_portable_proof.py`, `test_retrieval_capture.py` |
| `tools/benchmark/retrieval/contract_proof.py::_evidence_bytes/pytest_summary/nextest_summary`, `sdk_proof.py::_evidence_bytes/_nextest_counts/build_summary_from_evidence`, `proof_inventory.py::_evidence_bytes/verify_inventory_authority` | Domain readers consume the same pinned files verified by portable replay. Inventory, runner record and summary JSON use explicit control limits. Nextest logs stream through the existing event parser; exact selected/executed/pass identity remains authoritative, including the required SDK runner case. | `tools/ci/tests/test_retrieval_contract_proof.py`, `test_retrieval_sdk_proof.py`, `test_retrieval_benchmark.py` |
| `tools/ci/nextest_events.py::parse_nextest_inventory/parse_nextest/_parse_nextest_stream` | Bound inventory before JSON decode and each JSONL line before allocation; retain one semantic parser for all callers. Bound retained identity metadata, consume through actual EOF, verify digest/count and reject missing/duplicate/reordered/partial/invalid events. Do not weaken expected-inventory checks to obtain bounded memory. | `tools/ci/tests/test_nextest_ignored_inventory.py`, contract/SDK/portable tests above |
| `tools/ci/junit_events.py::parse_pytest_junit_bytes` and contract caller | Treat JUnit XML as an explicitly size-limited control input before constructing its tree; reject oversize without emitting a passing summary. Preserve grammar, identities and exact terminal counters. Streaming XML is not required for an input contract with a enforced finite limit. | `tools/ci/tests/test_retrieval_contract_proof.py`, `test_write_verification_receipt.py` |
| `tools/ci/write-verification-receipt.py::_nextest_evidence_summary/_summary_json_evidence_summary/_input_evidence` | The invoked receipt CLI shares event/control/hash owners and retains commitments through publication checks. Bind summary and input digests to the same validated byte identity. Retain the public receipt schema and role/command/source checks; mutation must leave no published receipt. | `tools/ci/tests/test_write_verification_receipt.py`, `test_portable_proof.py` |

Test filenames without a directory in that matrix also live in
`tools/ci/tests/`. Final IO-3 acceptance must exercise both contract and SDK
branches, canonical receipt production, detached replay and their real domain
readers. Use large valid multi-line event evidence to prove that the whole-log
control limit no longer blocks an otherwise admitted log; independently assert
the expected identities/counts. Reject one oversized line, oversized control
JSON/XML, truncated tail, duplicate event, swapped evidence, mutation during
consumption and oversized retained inventory. Measure fresh-process resource
use across the complete path; do not infer it from a mocked `_run`, a source
grep, component RSS or a successful spool after a whole-file read. A size limit
is a declared admission refusal, never permission to truncate or omit evidence.
The remaining paired ZIP copies are source-backed, not a reproduced OOM,
dishonest pass or measured production slowdown. Their repair and whole-path
resource proof are `NOT_RUN`; the implemented core's owner evidence is above.

IO-4 construction boundary: `profile_capture.py` remains the capture/publication
owner. Establish an epoch after validating the requested external evidence
namespace, before source/input acquisition and staging; an unsafe root must
refuse without writing a marker through that root. Native construction must start
in `benchctl.main`'s admitted `run` branch before preflight/source acquisition and
producer execution, not only inside the later `promote_profile_runs`. Thread the
same epoch/ID through that native path plus the five adapter `capture` entrypoints
(Criterion, retrieval-contract, lexical, pair, recorded) and `publish_capture`.
Record actual phase transitions and only identities/terminal facts already
observed. Inputs not yet acquired and processes not started retain explicit
unknown/not-started state, never synthetic digest/exit 0. Preserve the existing
publication decorator/GC exclusion and pointer commit point. Failure-marker
persistence errors must retain the original error and must not claim durable
failure evidence. This construction plan is not implemented by the IO-3 patch.
Audit native early `return 2` paths as well as exceptions: catching an error and
returning a status inside the epoch must not erase its failure phase or primary
reason. The CLI remains the outer status-rendering boundary; publication remains
the only successful completion boundary.

Exact existing regression owners: `tools/ci/tests/test_pair_capture.py` and
`test_pair_replay_workspace.py` for archive/workspace equality;
`test_corpus_binding.py` and `test_lexical_capture.py` for capsule relocation;
`test_recorded_capture.py`, `test_benchctl.py`, `test_criterion_capture.py` and
`test_portable_proof.py` for parsing/execution callers;
`test_benchmark_profile_capture.py` and `test_benchmark_evidence_bridge.py` for
failure/publication custody. Prefix all names with `tools/ci/tests/`.
Before relying on any owner result, verify its canonical command selection,
authority enrollment and source closure. New tests are not covered merely
because their filename resembles an enrolled module.

Order: retain the existing raw owner and IO-1 coordinated caller migration ->
retained IO-2 -> IO-3 and IO-4 -> EXEC-1 -> IO-5 and MISC-04 source freeze.
Common callers and publication changes have one integration owner.
No new storage system, alternate CLI, generic compatibility
mode or rank-policy tuning belongs in this work.

Failed work has an explicit capture/epoch identity and remains outside Git.
Diagnostic failure artifacts are not admissible immutable runs or profile
pointers. Keep their provenance and bounded error details until explicit
cleanup; do not put failed work into success inventory to retain it. A small
control JSON may be serialized to an owned file under an explicit byte limit;
that does not justify an unbounded second `native_bytes` promotion API.

### Superseded-document deletion boundary

The consolidation already landed in `66cee47e`: four SEP-27 agent handoffs,
11 RB tickets, 11 BM tickets, three RBR tickets and 12 TOPT records were removed
(41 bodies including formerly untracked handoffs). Current filesystem and Git
inventory have no Markdown files left in those replaced directories. No redirect
stubs or old-ticket dependencies are retained here. Accepted architecture ADRs,
unrelated plans and another writer's active new packet are not stale execution
documents and are not deletion targets.

Recovery only, not an execution dependency: the external preimage archive
`/Users/songmin/Documents/qi-docs-ssot-backup-sep27.bubnQ2/before-consolidation.tar.gz`
has SHA-256 `8c581c274d7c6f302d0726982a4471427d7bb957fe84042c4c54b8d4f4e39d81`.
It retains the 41 removed documents plus affected navigation/source preimages.
Extract into a separate directory if recovery is needed; never overwrite live
work. This refresh adds no duplicate implementation or handoff document.

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

- `tools/benchmark/benchctl.py`: replace direct native-recipe subprocess calls
  with `producer_execution.execute`; remove invented one-sample host evidence.
- `tools/benchmark/producer_execution.py`: retain private session, parent
  lifeline, actual terminal record, unreaped group identity, bounded cleanup
  and signal handling. Exit zero alone does not establish descendants exited.
- `tools/benchmark/criterion_capture.py`: bind observations to declared build,
  warmup and measurement phases rather than a static host dictionary.
- New `tools/benchmark/host_monitor.py`: own capture-time observations and
  cooperative local reservation; `evidence_bridge.py` derives typed host facts
  from the validated observations instead of caller-supplied summary constants.
- `tools/ci/source_closure.py`: enroll new module/tests in the affected closures.

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
| `tools/benchmark/evidence_bridge.py::promote_native_run` | Implemented file-backed inventory and retained failed staging; remaining whole-capture custody is IO-4. |
| `tools/benchmark/evidence_bridge.py::sha256_file/sha256_hex_file` | Implemented bounded hashing through the same pinned reader; preserve tests. |
| `tools/benchmark/raw_archive.py`, `pair_capture.py::pack_native/unpack_native/tree_files/capture/replay_run` | IO-2 implemented: shared streamed ZIP mechanics, fixed metadata, bounded directory parsing, commitment-only replay cache and streamed workspace/binary hashes. Retain archive checkpoint regressions. |
| `tools/benchmark/corpus_binding.py::capture/replay/_replay` | IO-2 implemented: same file-backed archive owner; retain capsule size ceiling, lexical input digests and exact release/bundle reconstruction. |
| `tools/benchmark/producer_execution.py::_wait_for_terminal/execute/_cleanup` | IO-1 implemented; retain file-backed normal/failure drains and bounded tails. Final combined owner evidence is in the execution checkpoint; native dispatch remains EXEC-1. |
| `tools/benchmark/evidence.py::RawFile.consume_lines`, recorded/Criterion/native/lexical/portable capture and domain readers | IO-3 implemented: bounded JSONL/retained metadata/control JSON, file-backed preparation, canonical receipt production/replay and paired-verdict command-log ZIP. Retain checkpoint regressions and final-input rechecks; whole-capture RSS remains IO-5. |
| Adapter preparation/execution and `profile_capture.py::publish_capture` | Open IO-4: follow the exact capture-epoch and failure-boundary matrix in section 3; streaming staging alone does not close whole-capture failure custody. |

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
  API now retains per-command failed outputs; extend capture-wide failure
  custody through adapter preparation/replay boundaries. This remaining scope
  is not closed by either staging or execution markers alone.
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
  C4 guards only protect their declared three-module set. Do not close C5
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
