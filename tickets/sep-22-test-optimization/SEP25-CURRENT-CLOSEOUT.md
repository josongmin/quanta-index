# Sep 25 current-source closeout boundary

Status: `BLOCKED` for current-source Rust qualification and performance evidence.

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

## Remaining work, in dependency order

1. Finish or stop the foreign host builds through their owners; do not kill
   other tasks from this ticket. Confirm normal launch of generated Rust
   build-script binaries before retrying a wide Cargo gate. `_dyld_start`
   stalling is an observed symptom, not a proven root cause.
2. Freeze one clean committed source after the shared dirty overlay is resolved
   by its owners. Run exact-source `just rust-profile verify-rust`,
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
