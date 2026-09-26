# Sep 25 source-bound closeout record

Historical snapshot only. For the latest observed checkout and remaining TOPT
conditions, use [CURRENT-STATUS.md](CURRENT-STATUS.md). All HEADs, commands,
host observations and verdicts below apply only to their stated Sep 25 source
and environment. The section titled “Remaining work” is the Sep 25 handoff,
not a claim that `dad270f1` is the present HEAD.

Status at the recorded sources: `BLOCKED` for Rust qualification and
performance evidence.

## Sep 25 follow-up — DSL baseline admission replay

Standalone `compare_dsl_bench.py --update-baseline` could pair an old DSL
artifact with a newly supplied `clean` preflight receipt from a similar host.
The artifact does not identify the receipt or capture invocation. The direct
promotion option now refuses with exit 2. The only baseline-admission path is
`python3 tools/benchmark/benchctl.py run dsl-authority --admit-baseline`:
one invocation preflights, freezes the source, runs both registered producers,
validates their artifacts, checks the unchanged preflight digest, same-host
identity, fresh file metadata, exact HEAD/mode, complete rows and sample floors,
then writes both candidates after both pass. Review and commit remain manual.
The preflight hostname digest was also aligned with Rust `HostV1`'s
domain-separated framing; raw SHA-256 was incompatible with exact host matching.

At clean shared `main` `dad270f13fced316ead544f55eaab9be3fd300a2`, the
nine-file benchmark-control pytest selection passed 157/157. Raw output:
`artifacts/qualification/topt-2026-09-25/benchmark-python-dad270f1.log`,
SHA-256 `edf2773ee693b73f04fc5027631c92c45c3f5f7319196d67b23fa5873738b9b6`.
Ruff lint/format and `git diff --check` passed. A real `benchctl run
dsl-authority --admit-baseline` on macOS exited 1 with
`unsupported_host expected_os=linux actual_os=darwin` before producers; the
isolated checkout's receipt is `artifacts/benchmark-receipts/dsl-authority/preflight.json`, SHA-256
`81cab82ab040b294c6ef554cb70c15aa1f73c4c78740ebbf0b6bbd57b4edcb4b`.
The local timing host still had load averages 57.21/63.05/67.65 and used
20,150.81 MiB of 21,504 MiB swap, so no Rust-wide or performance run was
admitted. These are Python control-plane results, not performance evidence.

This removes the replayable standalone promotion path, not the need for a
quiet canonical Linux run. File timestamps and local receipts are not
cryptographic attestation against a malicious local writer. Preflight and
source checkpoints cannot prove absence of transient contention or edits
entirely within a producer. No current-source DSL measurement or full Rust
qualification is claimed by this control-plane fix. The pre-implementation
TOPT admission record cannot be reconstructed from later measurements.

## Sep 25 RCA — source drift during benchmark run

At clean `91c1585341f68270354938368edcf5add77d289d`, `benchctl run`
checked for a clean checkout only before preflight. Its final artifact
validator checked the then-current HEAD, but there was no assertion that HEAD
and dirty state remained the same between preflight, producer execution, and
final validation. A concurrent commit could therefore pair an old preflight
with artifacts attributed to a new HEAD. A concurrent uncommitted edit that
persisted until a checkpoint could likewise escape the entry check.

Commit `4865ea456f93edb0b0d460e5eaf684aabd8cb8d5` freezes the initial
HEAD and rechecks HEAD and dirty state after preflight, after each producer,
after validation, and after comparison. Two regression tests first showed
that the old runner incorrectly returned success after simulated HEAD drift
or a dirty first producer; the new runner refuses with exit 2 before admitting
the artifacts. `VERIFIED`: the nine-file benchmark-control pytest selection
passed 150/150 on the clean code commit. Raw output:
`artifacts/qualification/topt-2026-09-25/benchmark-python-4865ea45.log`,
SHA-256 `6e04c6d216a16a69bbc6122162ee62a55263078b9938a2ba5bf2cdb5fe93bb6e`.
Ruff lint/format, Python compilation, and `git diff --check` passed.

The standalone replay boundary described in the original RCA was closed by
the guarded admission path above. The checkpoint limitation remains.

## Sep 25 current-head overload guard

At clean `5925a9b4b44968c19d91745482d5f60ccd851528`, the timing preflight
returned exit 0 and `status=clean` with no foreign Rust process, despite a
one-minute load of 51.44 on 16 logical CPUs. Raw preflight receipt:
`/tmp/qi-topt-current-preflight-5925a9b4.json`, SHA-256
`677596af9ec010aa6e92d94c844764012ac0cf311e3b5d8fd3f627321b15a69f`.
This was a live false admission of an obviously overloaded host; the existing
process-only guard was insufficient.

Commit `547202fd1341b8e94c8c90c0ebdb188c5bb54dc1` in a clean isolated
checkout adds a conservative one-minute-load ceiling of half the logical CPU
count. Missing or malformed load authority is an error. The preflight's exit
code, console verdict, and receipt now derive from one status. Both benchmark
profile execution and DSL baseline admission recompute and check the load
evidence, so a forged `clean` status over an overloaded host is refused.

- `VERIFIED`: nine-file benchmark-control pytest selection exited 0, 148/148
  passed on clean `547202fd`. Raw output:
  `artifacts/qualification/topt-2026-09-25/benchmark-python-547202fd.log`,
  SHA-256 `5846a86325732a7f286cbb5c5eb596cbcd7d40316d755c307271fdb3c32c9f5b`.
- `VERIFIED`: `just rust-policy` exited 0 on the same source. Raw output:
  `artifacts/qualification/topt-2026-09-25/rust-policy-547202fd.log`,
  SHA-256 `c57495412c20b981f0eac8a03cb25d3308504d08f0accd411aa0f14881a84cff`.
  It reported registry-only proof authority and zero benchmark artifacts; it
  does not establish execution qualification.
- `BLOCKED`: the exact preflight at `547202fd` exited 1, recording six foreign
  Rust processes and one-minute load 63.72 against limit 8. Raw output and
  receipt: `artifacts/qualification/topt-2026-09-25/timing-preflight-547202fd.log`
  (SHA-256 `5d10cd72c36cf3d87697d84ec65be78b30459f0cb600010ff2fecdd494f58256`)
  and `timing-preflight-547202fd.json` (SHA-256
  `f4dfc8eaf4ba541aa00bd49661d60be65be4979fbb753fc059a8fb1d7ed25595`).
  No timing samples were admitted.
- `NOT_RUN`: full `verify-rust`, daemon-all, and source-triggered public
  API/fuzz rails at this source. This Python timing-control repair does not
  change Rust, IPC, or SDK surfaces, but earlier unqualified source drift
  still prevents a current-head code-qualification claim.

The load ceiling rejects obvious overcommit; it does not monitor contention
after preflight or create a quiet-host measurement. TOPT-00 and TOPT-08 remain
open for source-bound execution, the retrospective paired protocol, and an
explicit decision on the irrecoverable pre-implementation admission record.

## Sep 25 follow-up — fail-closed timing snapshot

At clean isolated `f7b10d6340d7e957f761736e482b4d704209bab9`
(cherry-picked to shared `main` as `8f811417`; both commits have the same
tree), a new TOPT-00 control-plane gap was reproduced and fixed. A successful
`ps` call with empty, malformed, or duplicate process rows previously dropped
the bad rows and could report zero foreign Rust processes. A snapshot missing
the preflight process itself was likewise not rejected. The parser now refuses
those inputs; the entry point returns error status 2 and an error receipt
instead of admitting a clean timing run. Regression tests first failed 5/5
against the old implementation.

- `VERIFIED`: the nine-file benchmark-control pytest selection passed 140/140
  on the clean source. Raw log:
  `artifacts/qualification/topt-2026-09-25/benchmark-python-8f811417.log`,
  SHA-256 `11b9370be2d372a32fc753f419757e9ee02a837d8abbcb322e94fe128bac3ec6`.
- `VERIFIED`: `just rust-policy` exited 0 on the same clean source. Raw log:
  `artifacts/qualification/topt-2026-09-25/rust-policy-8f811417.log`,
  SHA-256 `00e95aee51caeb31201c31e776155c8ed29623ce98466baa97f865611b643c25`.
  Its proof-authority stage was registry-only and found no benchmark artifacts;
  it is not an execution or performance verdict.
- `BLOCKED`: the real timing preflight found 15 foreign Rust processes. The
  host had load averages about 29/46/56 and swap use 17.4/18.4 GiB while
  qualification was attempted. No timing sample or broad Rust run was admitted.
- `NOT_RUN`: exact-source full `verify-rust`, daemon-all, public API/fuzz, and
  TOPT paired timings at this commit. Earlier source-bound outcomes below do
  not transfer to this tree. The shared main still has unrelated dirty files;
  those changes are excluded from the clean-source receipts above.

This control-plane repair does not restore the missing pre-implementation
admission record. TOPT-00 and TOPT-08 remain open under their stated contract.

This ledger supersedes the status line of the historical Sep 23 gate log. It
does not supersede its source-bound receipts. The 18 retained Sep 22 findings
have implementation owners; they are not 18 current open implementation bugs.

## Frozen source and ownership

- Clean isolated checkout: `528dd9f14e3326831d5a06a3b9d6b7aae8a11459`.
- Shared `main` at capture: the same HEAD, but with unrelated uncommitted CI,
  Justfile, prompt-manager, and dependency changes. Those changes are excluded
  from every clean-checkout result below.
- The last completed full Rust and daemon-all code receipt remains clean
  `b0e147a443243aef2c16d38b39ebed0985868649`, not this HEAD.
- Source after `b0e147a4` includes substantial IPC, readiness, SDK, runtime,
  and benchmark changes. No old full-gate result transfers to it.

## Reproduced issue and repair

TOPT-00 explicitly treats a foreign `cargo-mutants` run as timing contention,
but `check_host_contention.py` matched only `cargo`, `cargo-nextest`, and
`rustc`. A foreign Cargo subcommand could therefore be misclassified as a
clean timing host. The new regression test first failed (`[]` instead of the
expected two processes); commit `528dd9f1` matches `cargo-*` executables,
including `cargo-mutants` and `cargo-fuzz`, without matching a similarly named
Python script. The matcher is used by the Justfile timing recipes and
`benchctl.py`. It does not certify host isolation by itself.

## Exact-source verification at `528dd9f1`

| Claim | Status | Evidence and scope |
|---|---|---|
| Timing matcher and adjacent benchmark-control tests | `VERIFIED` | The nine-file benchmark-control pytest selection exited 0, 135 passed; raw output `/tmp/qi-topt-final.splPyP/benchmark-python-528dd9.log`, SHA-256 `638c591f493dbdbc442c3e7a72d71d7b147137c12bd120c86a971f6df8462d87`. Python/benchmark control only. |
| Rust formatting | `VERIFIED` | `QUANTA_INDEX_BUILD_LOGGING=0 bash scripts/cargow --lane fmt-lane fmt --all -- --check` exited 0 in the clean checkout. Formatting only. |
| Repository policy | `VERIFIED` | `just rust-policy` exited 0; raw output `/tmp/qi-topt-final.splPyP/rust-policy-528dd9.log`, SHA-256 `9bcc22eed873c6e2409851dc1c702682fed1f8ba537634b9107cc77e9e4161bd`. This validates policy/inventory, not execution proofs. |
| Full-workspace strict Clippy | `BLOCKED` | `CARGO_BUILD_JOBS=1 QUANTA_INDEX_SCCACHE=0 QUANTA_INDEX_BUILD_LOGGING=0 just --set cargo 'bash ./scripts/cargow' rust-clippy` was interrupted after compiled build-script processes remained at macOS `_dyld_start` for more than two minutes. Partial log `/tmp/qi-topt-final.splPyP/rust-clippy-528dd9.log`, SHA-256 `0f6d25bc3a4613de33479f4de762d5dd669a5e6cb6e36474285e02b0f137d4b0`. No source lint verdict. |
| Full `verify-rust`, exhaustive `test-daemon-all`, and surface-triggered public API/fuzz rails | `NOT_RUN` | No exact-source terminal result. An earlier `just rust-profile verify-rust` attempt at `e1945485` also stalled in `/usr/bin/env` before producing a gate result. |
| Quiet-host TOPT timing | `BLOCKED` | `python3 tools/ci/timing/check_host_contention.py --receipt /tmp/qi-topt-final.splPyP/timing-preflight-528dd9.json --run-id topt-final-528dd9f1` exited 1 with 13 foreign Rust processes. Receipt SHA-256 `fb48fb3200aea6a82c7b714aef5d11246a687f357c75e0b5e15592b5c536a0de`. No timing samples admitted. |

The `/tmp` outputs are local diagnostic artifacts, not durable release receipts.
No current-source `CODE_QUALIFIED`, `PERF_EVIDENCE_CLEAN`, or
`PRODUCT_QUALIFIED` verdict is issued.

Local ignored copies of the cited logs and receipt were placed under
`artifacts/qualification/topt-2026-09-25/` with unchanged SHA-256 digests.
These are still local evidence, not published CI or release receipts.

## Later exact-source Rust sweep — `37b9b4b7`

Three further strict-Clippy failures were exposed by the full workspace gate
and repaired at their owners:

- `b90dcf3a`: removed a redundant SDK Unix-success transport wrapper. The
  `quanta-index-ipc` crate already owns the non-Unix compile refusal. SDK
  all-target strict Clippy and 108/108 SDK library tests passed after the fix.
- `b0e7db0d`: removed the always-`Ok` Unix state-root security wrapper while
  keeping its non-Unix typed refusal. Searchd all-target strict Clippy passed.
- `37b9b4b7`: marked the public harness semantic-source fixture result
  `#[must_use]`. Harness all-target strict Clippy and formatting passed.

The clean isolated `37b9b4b770b70a6673b3ee77b42a8bad945d5aec` checkout
then ran
`CARGO_BUILD_JOBS=4 QUANTA_INDEX_SCCACHE=0 QUANTA_INDEX_BUILD_LOGGING=0 RUST_TEST_THREADS=1 just rust-profile verify-rust`.
Its format, full-workspace strict Clippy, policy, cargo-deny, and cargo-machete
stages completed successfully. The subsequent release-profile bench `--no-run`
was still compiling DataFusion after roughly 14 minutes. Host load averages
were about 52 and swap usage about 15/16 GiB, with multiple foreign Cargo
builds active. This task's bench build was interrupted to avoid worsening host
resource pressure. The full gate therefore exited 130, not 0. Raw partial log:
`artifacts/qualification/topt-2026-09-25/verify-rust-37b.log`, SHA-256
`58d7d35715682cce50dd0890e4fd66dc1ad07cf7036f3a3f18acfe852de89b9b42eb0`.

Classification: `VERIFIED` for the completed static stages at `37b9b4b7`;
`BLOCKED` for the full `verify-rust` gate; `NOT_RUN` for its workspace-test and
rustdoc stages, the independent `test-daemon-all`, and the public API/fuzz
escalations. Compilation progress is not a bench-build pass. The shared main
advanced to `62836029` and retains unrelated dirty changes; neither source
nor that overlay inherits the `37b9b4b7` partial result.

## Sep 25 handoff at its recorded source, in dependency order

1. Obtain a quiet host through the owners of the foreign builds; do not kill
   other tasks from this ticket. Confirm normal launch of generated Rust
   build-script binaries before retrying a wide Cargo gate. `_dyld_start`
   stalling is an observed symptom, not a proven root cause.
2. Shared `main` is clean at `dad270f1`; freeze the selected qualification
   revision and run exact-source `just rust-profile verify-rust`,
   `QUANTA_INDEX_TEST_THREADS=1 just rust-profile test-daemon-all`, and the
   public API/fuzz rails required by changed IPC/SDK surfaces. Retain raw
   command results, source/config identity, selector counts, and artifacts.
3. On a quiet host, run the TOPT-00 prescribed selector inventory and at least
   five warm paired samples for R1-R4 and TH-4, with cold build separate. R5
   needs a selected/executed-case comparison. Report each metric independently.
   A retrospective old/new run can establish only a retrospective comparison;
   it cannot recreate the missing pre-implementation admission record or
   isolate unrelated intervening source changes by assertion.
4. Decide explicitly whether the irrecoverable historical admission is a
   permanent qualification exclusion or whether the acceptance contract must
   change. Until that decision, TOPT-00 and TOPT-08 stay open. A retrospective
   result must not be relabeled as prospective admission.
