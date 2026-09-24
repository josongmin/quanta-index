# SEP-21 action list

Use the [residual execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md) for
current work. The original wave checklist and importer steps are in Git
history. Check a task against current source and raw evidence before marking
it complete; a historical handoff or a `passed` JSON alias is not current
qualification.

| Area | Current action | Authority |
| --- | --- | --- |
| P00-P10 owners | Run the registered owner recipe at a frozen source and issue an exact-source receipt. | `tools/ci/proof-authority.toml`, `Justfile` |
| P03-P10 release | Implement any missing registered release authority and execute it on the required Linux host and binary. | `authority_state`, `required_host`, and test targets in the registry |
| P09 readiness | Retain the bound-socket identity/path-loss process regression; add actual supervised-child, maintenance/backend-loss, and bounded request-diagnostic process targets before enabling the Linux release node. | [S21-10](S21-10-control-authorization-readiness-and-observability.md) |
| P10 state | Prove current-format backup/restore/verify and typed legacy refusal; inventory real target data before cutover. | [operator runbook](../../../operator/state-cutover-runbook.md) |
| P11 | Execute the new clean-source fresh-build gate on the frozen Quanta/Semantica pair and bind its raw result to the manifest (the writer's content archive alone does not prove the build ran); issue distinct deployment, activation, and rollback receipts. | [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md) |
| P12A | Validate existing aggregate/handoff custody and issue the exact-pair infrastructure receipt after P11. | `tools/ci/write-proof-aggregate.py`, `tools/ci/lint/handoff_validation.py` |
| P12Q | Reissue all final-source dependencies, create the aggregate, then issue and validate the P12 manifest. | `just proof-authority-final-qualification` |

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
