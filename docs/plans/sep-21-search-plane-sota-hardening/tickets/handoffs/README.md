# SEP-21 handoff validation usage

Current handoff schema/validator owns fields and accepted
[custody](../../../../adr/SEP-21-004-process-supervision-state-cutover-and-proof.md)
plus [proof interpretation](../../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md)
owns permanent rules. A handoff is an actual result, never a placeholder or a
current release dependency inferred from an old lane name.

| Command | Scope |
| --- | --- |
| `just lane-handoff-check artifacts/sep-21/handoffs/<LANE>.json` | Strict current-source checkpoint closeout: schema plus semantic/clean-source/proof binding |
| `just lane-handoff-check-historical <path>` | Authentic historical archive/ancestry without rebinding old result to current HEAD |
| `just lane-handoff-chain-check` | Historical omission/duplicate/order/P02 join/adjacent-SHA audit; not current release qualification |

Current final-source qualification is owned by
[S21-13](../S21-13-release-evidence-and-sota-qualification.md).
Missing old records cannot be reconstructed from current manifests. Original
lane execution/custody prose is recoverable through [history](../../../ARCHIVE-INDEX.md#oct-05-repository-wide-history-cleanup).
