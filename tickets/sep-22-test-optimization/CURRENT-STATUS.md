# TOPT current status

Observed: 2026-09-27 02:49 KST, `main` HEAD
`1043314615c53cb387d067b0c92c6ac7d081b1ec`. The shared checkout had
35 dirty paths, including unrelated documentation and search-plane edits.
This is a status inventory, **not** a frozen-source qualification receipt.
Recheck HEAD, dirty paths and evidence before using it for a later revision.

Status: `IMPLEMENTATION_HISTORY_RETAINED`; `CODE_QUALIFIED` and
`PERF_EVIDENCE_CLEAN` are **not established for the observed checkout**.
TOPT-00 and TOPT-08 remain open. The Sep 22 audit and Sep 23/Sep 25 gate
records are historical, source-bound records, not 18 current open code bugs or
evidence for this HEAD. Their recovery pointers are in [INDEX.md](INDEX.md)
and [SEP25-CURRENT-CLOSEOUT.md](SEP25-CURRENT-CLOSEOUT.md).

## Remaining TOPT conditions

| Condition | Current classification | Decisive next evidence |
| --- | --- | --- |
| Exact-source Rust/daemon qualification | `NOT_RUN` for this observed HEAD | Freeze a clean revision; run `just rust-profile verify-rust` and `QUANTA_INDEX_TEST_THREADS=1 just rust-profile test-daemon-all`. Run public-API/fuzz escalation only if the final changed surfaces require it. Preserve terminal results, selected/executed counts, toolchain/config and source identity. Earlier focused or partial gates do not transfer. |
| TOPT-00 selector and paired timing | `BLOCKED` for performance qualification | On a quiet eligible host, capture the exact selector inventory and at least five warm paired samples for R1-R4 and TH-4, reporting cold build separately. Compare R5 by selected/executed cases and daemon boots. Bind source, target-root, toolchain, features, cache class and raw logs; reject contended runs. |
| Historical pre-implementation admission | `BLOCKED` pending an explicit acceptance decision | The missing prospective record cannot be reconstructed. Either keep it as a permanent exclusion and label a later old/new run **retrospective only**, or retain the original acceptance requirement and leave TOPT-00/08 open. Do not relabel a later capture as the missing record. |

The current host is macOS; `dsl-authority` baseline capture requires canonical
Linux. No warm/cold baseline files were present in `tools/benchmark/baselines`
at this observation. These facts do not prove whether another host or external
evidence store has a completed run; no such source-bound receipt was adopted
into this TOPT ledger.

## Current benchmark authority and separate scope

[`registry.toml`](../../tools/benchmark/registry.toml) is the active profile
registry and [`benchctl.py`](../../tools/benchmark/benchctl.py) is the current
orchestrator. `manifest.py` is a projection; old references to
`manifest.json` are historical. `just rust-profile-list` confirmed that
`verify-rust` and `test-daemon-all` remain registered. The current
`benchmark-prep-local` recipe uses locked `uv`, Python control-plane tests,
Rust bench-protocol tests and harness compile/library checks; it is PREP, not
the TOPT performance or full Rust qualification verdict.

The [benchmark migration active gaps](../../docs/plans/sep-26-bench-migration/tickets/CURRENT-AUDIT.md)
are a **separate** partial program. A registered profile, accepted
orchestration ADR, or diagnostic replay does not close TOPT or qualify DSL
performance. Conversely, TOPT's historical owner fixes do not close BM-02/07.

## Evidence boundary

This update inspected `git rev-parse HEAD`, `git status --porcelain=v1`,
`git ls-tree`, the current Justfile/profile list, active benchmark ADR and
ledger, and local repository artifacts. It did **not** execute the Rust gates,
producer benchmarks, paired timing, or hosted CI. The last TOPT-local Python
control-plane log in the repository is bound to earlier `e854d934`, not this
HEAD. Source changes or dirty overlays invalidate any attempted promotion of
those earlier results.
