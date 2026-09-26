# SEP-21 action list

Use the [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md) for
current work. The original wave checklist and importer steps are in Git
history. Check a task against current source and raw evidence before marking
it complete; a historical handoff or a `passed` JSON alias is not current
qualification.

| Area | Current action | Authority |
| --- | --- | --- |
| R0 — common result authority | Derive test outcomes from archived machine runner output; bind host and CI producer authority; add typed operational-action results. A hand-written `passed` terminal JSON is not an oracle. | [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md), [S21-13](S21-13-release-evidence-and-sota-qualification.md) |
| R1 — P03-P08 release | Inventory each staged target and independent negative oracle before changing product code or promoting authority. | `tools/ci/proof-authority.toml`, `tools/ci/test-authority.toml` |
| R2 — P09 | Active sealed-identity liveness replaced the boot-time backend constant. A bounded Admin-only control/SDK/searchctl projection of the existing request-event ring is implemented on a shared dirty overlay. Focused contract, authorization, SDK, CLI, in-process and Cargo-built daemon lexical queue→backend→terminal checks passed locally; none is a frozen-source receipt. Still require provider correlation, final-source wrap/drop/restart and socket observer checks, supervised child/maintenance loss, and Linux release qualification. | [S21-10](S21-10-control-authorization-readiness-and-observability.md) |
| R3 — semantic scope | Locate an independent producer source-plan oracle before any completeness wire change; preserve legitimate lexical-only/no-op deltas. | [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md) |
| R4 — P10 state | Prove current-format backup/restore/verify and typed legacy refusal; inventory real target data before cutover. | [operator runbook](../../../operator/state-cutover-runbook.md) |
| R5 — P11 exact pair | Freeze both sources and resolved Cargo path roots, then bind fresh build, QBC targets and actual runner result to one receipt. | [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md) |
| R6 — P11 operations/P12 | Issue distinct deployment/activation/rollback action receipts only on authorized targets; reissue final-source dependencies and aggregate; audit historical handoffs separately. | [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md), [S21-13](S21-13-release-evidence-and-sota-qualification.md) |

2026-09-24 checkpoint tested at Quanta `7dec5965` plus the RepoMap receipt
fix, committed as `3b1d7b19`: the owner integration target passed 19/19 and the malformed
terminal-sequence unit target passed 1/1. Before that edit, the clean-Quanta
P12A infrastructure recipe passed 121/121 Python tests while validating zero
proof manifests. These are local checks only. The registry still stages P09
Linux process/component-loss, P10 restore/rollback, P11 exact-pair and
deployment/activation/rollback; no final-source release bundle has
been issued. See the P11/P12A tickets for precise source and exclusions.

`just proof-authority-lint` checks registry structure only.
`just proof-authority-current-gate` checks a fresh P00 receipt.
`just proof-authority-code-gate` checks the fixed `CODE_QUALIFIED` closure
against the current Quanta/Semantica source pair before deployment. It does
not require deployment, activation, rollback, or the final aggregate.
`just proof-authority-release-gate` checks the complete current-source bundle
after those operational actions. The manual `correctness` workflow selects
`proof_stage=code` or `proof_stage=final` (the default). Neither gate runs on
ordinary PRs. Keep code qualification, deployment, activation, and rollback
as separate states; a code-gate pass is not production readiness.

The R0 proof-boundary fix is prerequisite to *authoritative* receipts, not to
parallel P09/source-plan development. A source edit invalidates existing
exact-source receipts; reissue only after the final source pair is frozen.
