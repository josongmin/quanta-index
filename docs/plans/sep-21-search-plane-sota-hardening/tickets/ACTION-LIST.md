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
| R2 — P09 | Replace boot-time `required_backend: true` with bounded live health; expose the existing event ring through one bounded Admin-authorized diagnostic path; prove component loss in real processes. | [S21-10](S21-10-control-authorization-readiness-and-observability.md) |
| R3 — semantic scope | Locate an independent producer source-plan oracle before any completeness wire change; preserve legitimate lexical-only/no-op deltas. | [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md) |
| R4 — P10 state | Prove current-format backup/restore/verify and typed legacy refusal; inventory real target data before cutover. | [operator runbook](../../../operator/state-cutover-runbook.md) |
| R5 — P11 exact pair | Freeze both sources and resolved Cargo path roots, then bind fresh build, QBC targets and actual runner result to one receipt. | [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md) |
| R6 — P11 operations/P12 | Issue distinct deployment/activation/rollback action receipts only on authorized targets; recover authentic handoffs, then reissue final-source dependencies and aggregate. | [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md), [S21-13](S21-13-release-evidence-and-sota-qualification.md) |

2026-09-24 checkpoint tested at Quanta `7dec5965` plus the RepoMap receipt
fix, committed as `3b1d7b19`: the owner integration target passed 19/19 and the malformed
terminal-sequence unit target passed 1/1. Before that edit, the clean-Quanta
P12A infrastructure recipe passed 121/121 Python tests while validating zero
proof manifests. These are local checks only. The registry still stages P09
Linux process/component-loss, P10 restore/rollback, P11 exact-pair and
deployment/activation/rollback, and P12Q; no final-source release bundle has
been issued. See the P11/P12A tickets for precise source and exclusions.

`just proof-authority-lint` checks registry structure only.
`just proof-authority-current-gate` checks a fresh P00 receipt.
`just proof-authority-release-gate` requires the complete current-source
bundle. Keep code completion, owner tests, release qualification, deployment,
activation, and rollback as separate states.

The R0 proof-boundary fix is prerequisite to *authoritative* receipts, not to
parallel P09/source-plan development. A source edit invalidates existing
exact-source receipts; reissue only after the final source pair is frozen.
