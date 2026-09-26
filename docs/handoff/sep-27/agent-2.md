# Agent 2 handoff — TOPT residual audit and qualification

Scope: Sep 22 test-optimization packet (`TOPT-00` through `TOPT-08`) only.
The [retrieval handoff](agent-1.md) and
[benchmark-migration gaps](../../plans/sep-26-bench-migration/tickets/CURRENT-AUDIT.md)
are separate owners. Do not count their pending work as 18 new TOPT defects or
silently absorb their dirty paths.

Observed 2026-09-27 02:54 KST: `main` HEAD
`ed18cac2b44181dcfd19c95241a1c7ad13785224`. The shared worktree was
already dirty in `benchmarks/retrieval/proof-required-tests.json` and active
benchmark-migration tickets/README. `docs/handoff/sep-27/agent-1.md` appeared
during this audit. This handoff is **not** a clean-source proof; re-freeze HEAD,
tree, branch, dirty paths, toolchain and target-root identity before any run.
Any later commit or overlapping writer makes this snapshot stale.

## What the current audit establishes

- [TOPT current status](../../../tickets/sep-22-test-optimization/CURRENT-STATUS.md)
  and [ticket crosswalk](../../../tickets/sep-22-test-optimization/INDEX.md)
  retain 18 historical owner findings. TOPT-01–07 are marked code-landed in
  historical tickets; this audit did **not** re-execute their focused tests or
  prove the 18 invariants at the observed HEAD. No new TOPT code defect is
  confirmed by this read-only audit.
- `python3 tools/ci/lint/check-test-authority.py` returned
  `test authority: OK`; this checks the registry, not selected/executed counts.
  `just rust-profile-list` includes `test-fast`, `test-integration`,
  `test-daemon`, `test-daemon-all` and `verify-rust`.
- `python3 tools/benchmark/benchctl.py plan dsl-authority` was read-only and
  listed exactly `dsl-warm` and `dsl-cold`, canonical-Linux host policy,
  200/20 sample floors and registry digest
  `sha256:843c726b5250cd0ec053afe895df67208d94632eeaa4b99a1f610544aba56188`.
  `git ls-tree` found no tracked warm/cold baselines. This is **not** a DSL
  capture, baseline or TOPT timing result.
- The local host was Darwin with load averages 34.85/36.99/34.52 at the
  observation. Repository-local TOPT qualification logs are bound to older
  revisions, most recently Python control-plane `e854d934`; no current-HEAD
  full Rust, daemon-all or quiet paired-timing receipt was adopted here.
  External stores/hosts were not audited.

## Remaining work, in dependency order

| ID | Classification now | Action and terminal exit evidence |
| --- | --- | --- |
| A2-01: freeze and re-audit 18 owners | `NOT_RUN` on this HEAD | In a clean isolated checkout, record HEAD/tree, dirty state, toolchain/features, `CARGO_TARGET_DIR`, registry/selector digests and foreign-process snapshot. Walk all 18 rows in the [crosswalk](../../../tickets/sep-22-test-optimization/INDEX.md#finding-crosswalk) against current code and tests. For each row record owner path, reachable failure/cost, independent oracle, exact selector, selected/executed count and `IMPLEMENTED`, `OPEN` or `STALE`. Do not carry a Sep 23 owner verdict forward by filename alone. If a real code regression is reproduced, repair only its owning abstraction and add a failing regression test before the fix. |
| A2-02: owner and full Rust qualification | `NOT_RUN` on a frozen current source | Run the relevant TOPT-01–07 focused rails after A2-01, then `just rust-profile verify-rust` and `QUANTA_INDEX_TEST_THREADS=1 just rust-profile test-daemon-all` to terminal outcomes on the **same clean revision**. Preserve raw terminal/JUnit or nextest events, selected/executed/pass/fail/skip counts, source/dependency/config/toolchain identity, paths and SHA-256. Run `just rust-public-api` for contract/SDK public-surface changes and `just rust-fuzz-smoke` for IPC wire/decode changes, per `AGENT_PLAYBOOK.md`; otherwise mark each `NOT_APPLICABLE` with a diff rationale. A focused pass, compile-only result or prior-HEAD log is not `CODE_QUALIFIED`. |
| A2-03: TOPT paired test-cost evidence | `BLOCKED` until quiet host and comparable inputs | Use TOPT-00's selector inventory and a quiet host. Capture at least five warm paired samples for R1–R4 and TH-4; keep cold build separate. Report median, p95, min/max, execution and failure counts per selector. R5 requires selected/executed cases, daemon boots and assertion conservation, not a fabricated speedup. Bind before/after source diff, features, toolchain, target-root/cache class, host/load, raw logs and digests. If unrelated intervening changes prevent causal attribution, report a retrospective comparison only. Never use `QUANTA_INDEX_ALLOW_CONTENDED_TIMINGS=1` as authority. |
| A2-04: historical admission decision | `BLOCKED` on an acceptance choice | The pre-implementation TOPT-00 admission record is irrecoverable. Obtain an explicit decision: retain it as an unsatisfied original requirement, or document a permanent exclusion and evaluate new retrospective evidence separately. Do not manufacture a prospective record or close TOPT-00/08 merely because later tests pass. |
| A2-05: final TOPT closeout | `NOT_RUN` | Only after A2-01–04, update the [current ledger](../../../tickets/sep-22-test-optimization/CURRENT-STATUS.md) and TOPT-08 with exact-source `VERIFIED`/`FAILED`/`BLOCKED`/`NOT_RUN`/`NOT_APPLICABLE` outcomes. Keep historical Sep 22/23/25 documents immutable except for navigation corrections. `PRODUCT_QUALIFIED`, hosted CI, deployment and activation remain separate gates. |

## Related boundary, not Agent 2's TOPT closure

The current `registry.toml`/`benchctl.py` control plane has separate BM-02/07
proof gaps. Canonical Linux DSL warm/cold baseline admission uses
`benchctl run dsl-authority --admit-baseline`; it requires a clean source and
host and is not the R1–R5/TH-4 test-cost protocol. No tracked DSL baselines or
current capture were found here. The broader BM-03/07 profile-execution, live
retrieval, authenticated outcome and hosted-CI gaps remain in the
[benchmark-migration active audit](../../plans/sep-26-bench-migration/tickets/CURRENT-AUDIT.md).
Coordinate with that owner; do not mark TOPT green from BM registration or
diagnostic replay, or BM complete from TOPT owner tests.

## Stop and handoff rules

1. Stop if HEAD/tree, dirty overlay, selector inventory, benchmark registry,
   toolchain or input digest changes during evidence capture. Preserve the
   partial result as `FAILED` or `BLOCKED`; re-freeze before retrying.
2. Do not stage, revert or rewrite Agent 1's retrieval manifest or active BM
   ticket/README edits. Check path ownership before touching shared source.
3. On a contended host, do static/selector work only. Do not launch a broad
   Cargo gate or performance rail into the current competing builds. Do not
   terminate foreign processes from this handoff.
4. Hand off one row per remaining condition with exact source/input identity,
   command, raw result, artifact path/digest, covered and excluded scope, and
   the next owner. Missing, stale, partial or wrong-host evidence never becomes
   `PASS`, `GREEN` or `QUALIFIED`.
