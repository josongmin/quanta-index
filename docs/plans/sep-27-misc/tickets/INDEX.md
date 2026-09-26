# SEP-27 benchmark / retrieval / test-optimization — execution SSOT

Status: `ACTIVE`. Latest documentation/source re-audit: 2026-09-27 KST,
`main@106d7abec2dd3fa03f9db5a19a3de41df2f0afad`, shared dirty checkout.
This refresh owns only this file. It preserves existing implementation, tests
and the separately active engine work; it does not qualify those dirty changes.
The earlier documentation epoch began at `66cee47e` and crossed another
writer's commit. Its source inventory and test results are historical, not
the current ownership inventory.
Earlier implementation checkpoints are included in `106d7abe`. The subsequent
file-backed execution/caller/test changes and this contract remain uncommitted.
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

### Latest source re-audit and current decision

This refresh re-read the four archived handoff preimages, their current
dispositions below, the implemented publication/execution/storage boundaries,
all remaining archive/preparation call sites and canonical selector wiring.
No implementation or product test was run by this documentation-only refresh.
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
| Python publication, execution/raw I/O, adapters and C4 owner tests | Retained checkpoint: 617 executed/passed, zero failure/error/skip; rerun in this documentation refresh `NOT_RUN`; full integrated-source qualification `NOT_RUN` | Execution checkpoint below binds its commands and source bytes. Owner Python files were unchanged during that run; engine and test-authority changes prevent a whole-source stability claim. The earlier 460-case result is also historical. |
| MISC-01 Rust shared GC lock | Current implementation confirmed by source inspection; execution `NOT_RUN` in this refresh | Earlier 51-case result below is historical owner evidence, not a new Rust run. |
| MISC-02 native lifecycle and host observations | Implementation `NOT_RUN` | Native recipe still calls `subprocess.run`; `shared` / one sample is assigned, not observed; `host_monitor.py` is absent. |
| MISC-03 bounded I/O and retained failure epoch | Partially implemented; execution checkpoint below adds owner-local proof, final-source qualification `NOT_RUN` | File-backed staging/promotion and normal/cleanup execution logs are implemented. Whole-tree ZIP, remaining preparation/parsing and capture-wide failure records remain open. No measured OOM claim. |
| C5 canonical test selection/source custody | Partial concurrent repair observed; acceptance `NOT_RUN` | Both modules now occur in the canonical command and affected closures, but still lack `python_targets` entries. Complete owner/scope registration and omission/mutation regression proof before canonical closure. |
| MISC-04 through MISC-07 final-source acceptance | `NOT_RUN`; input-dependent claims become `BLOCKED` only when their actual prerequisite is missing | Execute the inline ticket-specific oracles after the shared API cutover; do not rerun already sufficient owner checks under multiple ticket names. |
| Superseded document removal | `VERIFIED` filesystem census | No Markdown bodies remain in the five replaced directory groups; exact external backup SHA-256 rechecked. No additional deletion was needed in this refresh. |
| Documentation/policy epochs | Prior test-authority `FAILED`; later execution-checkpoint policy commands all exit zero | The engine owner updated the catalog; the prior `l2_file_mutation.rs` orphan failure no longer reproduces in that later run. MISC-04 still binds final integrated selectors; catalog presence alone is not executed engine proof. |

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
| C3 | Verification, hashing, staging and producer output use bounded file-backed I/O. Pair packing still materializes the tree and ZIP; unpacking reads complete entries. Remaining corpus/recorded and portable-proof parsing is not fully streamed. | MISC-03 partial: execution logs now join repaired owner boundaries; archives, remaining preparation and capture-wide failure custody outstanding. RSS proof covers digest/copy/execution components, not end-to-end capture. |
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

Native `benchctl.py` dispatch/host monitoring is still MISC-02. Remaining
MISC-03 work is pair/corpus archives, residual preparation/JSONL parsing,
capture-wide failure records and archive/end-to-end memory proof. Heavy
MISC-04/05 runs still wait for the complete owner-local repair set.

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
| MISC-03 | Evidence/data path | Verification/control/staging, promotion and file-backed execution API implemented | Archives, remaining preparation/parsing, capture-wide failure records and end-to-end memory proof |
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
| 3 | MISC-03: migrate pair/corpus archives and remaining preparation/parsing to the implemented raw-reference contract. Bridge/hash helpers and all six promotion callers already use it. `profile_capture.publish_capture` stays the only complete-profile commit owner. | No hidden whole-tree dictionary, whole ZIP, unbounded JSONL or full-output decode in an accepted large-payload path; cross-adapter replay and canonical wire fixtures unchanged. |
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
| IO-2 | `pair_capture.py::pack_native/tree_bytes/_unpack_native/replay_run`: whole-tree dictionary, whole ZIP and whole-entry reads. `corpus_binding.py::capture/_replay` has the same representation issue despite its existing 256 MiB capsule ceiling. | Stream both archive families through one file-backed I/O contract, with domain-specific inventory rules retained. Fixed ZIP metadata and sorted canonical entries; pinned seeking plus a complete byte commitment; explicit entry/total-byte limits. Test exact-limit/over-limit, corrupt/truncated/link/alias/duplicate entries, relocation replay and independent payload equality. Preserve the capsule ceiling unless separately justified; bounded input size is not bounded working memory. |
| IO-3 | `criterion_capture.py::capture`, `benchctl.py::_capture_native_family`, `recorded_capture.py::capture/replay_run`, lexical corpus binding and execution callers still prepare/parse complete byte values. | Stream unbounded JSONL and raw artifacts; bound each control document/line before decoding. Keep declared native row order, exact producer inventory and domain-derived payload parity. A spool after a full read is not a memory repair. Retain malformed UTF-8/partial-line/oversize/nonfinite/duplicate-record refusal. |
| IO-4 | `profile_capture.py::publish_capture` records no whole-capture failure across all preparation/replay phases; bridge failure custody covers only one staging epoch. | One capture-level failure record binds phase, source/input identity, retained logs, terminal and primary/cleanup errors outside success inventory. Cover failures before staging, second-family publication, domain replay and final source check; preserve the prior complete pointer. Marker-write failure must report both errors without claiming persisted evidence. |
| EXEC-1 | `benchctl.py` native dispatch bypasses shared execution; `promote_profile_runs` assigns `shared` and one sample; no `host_monitor.py` exists. | After IO-1, route native producers through the same lifecycle. Observe capture-bound host facts and cooperative reservation; bridge/replay rederive them. Test sample omission/reorder/gaps/stale capture and forged summaries. Cooperative reservation must never issue qualified-performance proof by itself. |
| IO-5 | Fresh-process RSS tests cover digest/staging and producer log capture at 8/128 MiB. | Remaining: archive generation/replay, large failure-path output and many-file metadata/end-to-end costs. Reuse one complete owner suite after shared API integration, then the serial runtime batch. Do not infer end-to-end bounds from component measurements. |

The following caller details are mandatory parts of those rows, not additional
tickets. They prevent a streamed writer from hiding another whole-payload copy.

| Step | Additional live owner/caller | Structural completion condition |
| --- | --- | --- |
| IO-2 | `pair_capture.py::_ReplayWorkspace.restore` retains both full `corpus.bundle` and `native-tree.zip` byte values in `self.archives`. | Store immutable file commitments, not archive bytes. Revalidate the complete pinned bytes for every case before reusing a restored workspace; changing either archive must refuse, even if path/size/mtime are unchanged. Preserve the existing cross-case exact-identity rule. |
| IO-2/IO-3 | `pair_capture.py::_replay_tree_identity` reads every restored file, including Git objects; `capture` re-reads full executable bytes after execution. | Use the same streaming digest owner for all regular files and binaries. Retain path/mode/link inventory and workspace mutation refusal. Do not reduce equality to a metadata-only cache or weaken binary postchecks. |
| IO-2 | `corpus_binding.py::capture/replay` and both `lexical_capture.py` callers use a bytes capsule API. | Cut over producer and replay together to the same file commitment. Keep exact metadata/bundle inventory, real Git reconstruction and the existing 256 MiB capsule limit; ZIP payload and central-directory limits are explicit. No private bytes compatibility branch. |
| IO-3 | `benchctl.py::_capture_native_family` reads complete files into `captures`; `recorded_capture.py::capture/replay_run` reads full imports; `criterion_capture.py::_binary` and `retrieval/portable_proof.py::_run` consume complete bounded control output. | Separate raw references from decoded control values. Stream line protocols with bounded individual records and incremental validation; retain exact native row order and domain payload derivation. A bounded 16 MiB control document is not itself an unbounded-memory defect; decide each caller's supported input contract before replacing it. |
| IO-4 | Adapter `capture` preparation occurs before `profile_capture.publish_capture` is entered. | Establish one capture epoch before the first fallible preparation action. Its owner covers preparation, execution, staged/promoted runs, domain replay and final source check. Publication alone cannot record pre-entry failures. Keep raw failure artifacts separate from admissible run inventory and preserve the prior complete pointer. |

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
IO-2/IO-3 and IO-4 -> EXEC-1 -> IO-5 and MISC-04 source freeze. Archive work can
be designed independently, but common callers and publication changes have one
integration owner. No new storage system, alternate CLI, generic compatibility
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
| `tools/benchmark/pair_capture.py::pack_native/unpack_native/tree_bytes/capture/replay_run` | Open IO-2: file-backed ZIP generation and entry streaming; remove whole-tree and whole-archive copies. |
| `tools/benchmark/corpus_binding.py::capture/replay/_replay` | Open IO-2: same archive representation problem; retain capsule size ceiling and exact release/bundle validation. |
| `tools/benchmark/producer_execution.py::_wait_for_terminal/execute/_cleanup` | IO-1 implemented; retain file-backed normal/failure drains and bounded tails. Final combined owner evidence is in the execution checkpoint; native dispatch remains EXEC-1. |
| Remaining adapter preparation/parsing and `profile_capture.py::publish_capture` | Open IO-3/IO-4: follow the exact caller and failure-boundary matrix in section 3; streaming staging alone does not close either. |

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
