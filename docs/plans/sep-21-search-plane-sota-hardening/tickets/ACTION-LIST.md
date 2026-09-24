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
| P10 state | Prove current-format backup/restore/verify and typed legacy refusal; inventory real target data before cutover. | [operator runbook](../../../operator/state-cutover-runbook.md) |
| P11 | Prove the exact Quanta/Semantica pair and issue distinct deployment, activation, and rollback receipts. | [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md) |
| P12A | Validate existing aggregate/handoff custody and issue the exact-pair infrastructure receipt after P11. | `tools/ci/write-proof-aggregate.py`, `tools/ci/lint/handoff_validation.py` |
| P12Q | Reissue all final-source dependencies, create the aggregate, then issue and validate the P12 manifest. | `just proof-authority-final-qualification` |

`just proof-authority-lint` checks registry structure only.
`just proof-authority-current-gate` checks a fresh P00 receipt.
`just proof-authority-release-gate` requires the complete current-source
bundle. Keep code completion, owner tests, release qualification, deployment,
activation, and rollback as separate states.
