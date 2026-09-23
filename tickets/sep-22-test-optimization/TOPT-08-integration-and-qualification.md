# TOPT-08 — Same-Source Integration and Qualification

Status: `blocked — workspace Clippy and uncontended performance evidence`

Depends on: TOPT-01 through TOPT-07

Aligned S21 owner: S21-13

## Goal

Integrate the owner fixes without patch-on-patch helpers, then produce honest
correctness and performance evidence from one final source.

## Integration audit

1. Freeze final `HEAD`, dirty digest, toolchain, and target inventory.
2. Re-run the 18-row crosswalk against current source; no `OPEN`, stale path,
   duplicate owner, or compatibility shadow path may remain.
3. Confirm production behavior did not change unintentionally:
   - persisted Unix lease semantics unchanged;
   - provider jitter/retry classification unchanged;
   - cancellation remains fail-closed;
   - runtime isolation is not replaced by global fixture sharing;
   - lower-layer resource-envelope authority remains.
4. Inspect diff ownership: clock/env, wakeup, fixture, provider, oracle, helper,
   and coverage changes must each land in their named owner.

## Verification order

1. Each ticket's focused rails.
2. `just fmt-check`
3. `just rust-clippy`
4. `just rust-profile test-fast`
5. `just rust-profile test-integration`
6. `just rust-profile test-daemon`
7. `just rust-profile test-daemon-all`
8. `just rust-profile verify-rust`

Escalate with `just rust-public-api`, `just rust-fuzz-smoke`,
`just rust-hexagonal`, or `just rust-cargo-modules` only when their governed
surface actually changes, as required by `AGENT_PLAYBOOK.md`.

## Performance closeout

- rerun only on a quiet host under TOPT-00 protocol;
- compare exact selectors, source metadata, execution counts, cache class, and
  raw logs;
- report R1-R4 and TH-4 independently; do not hide regressions in one aggregate;
- R5 closes by conserved coverage plus one fewer daemon scenario, not by timing
  alone.

## Verdicts

- `IMPLEMENTED`: owner diffs and focused rails complete;
- `CODE_QUALIFIED`: full required local Rust rails pass on exact source;
- `PERF_EVIDENCE_CLEAN`: uncontended before/after protocol passes;
- `PRODUCT_QUALIFIED`: reserved for the existing S21 aggregate proof graph;
- `BLOCKED`: missing host, stale source, foreign dirty overlap, or failed rail.

This packet cannot emit `PRODUCT_QUALIFIED` by itself and must not modify the
proof registry to manufacture closure.

## Done

All 18 rows are implemented and evidenced, no required rail is failed/skipped,
timing evidence is source-bound and uncontended, and remaining S21 product
qualification gaps are reported separately.
