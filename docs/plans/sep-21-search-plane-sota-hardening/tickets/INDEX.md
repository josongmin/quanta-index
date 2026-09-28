# SEP-21 remaining work index

Status: `ACTIVE — qualification remains separate from implementation`.
Completed identity/publication/query/process decisions are in the
[SEP-21 accepted registry](../../../adr/SEP-21-DECISION-REGISTRY.md).
Completed repair decisions are in
[SEP-27-005](../../../adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md).
Owner execution histories are recoverable through the [plan archive](../../ARCHIVE-INDEX.md).
No historical count or status is current qualification.

| Active owner | Scope |
| --- | --- |
| [Residual ledger](CURRENT-RESIDUAL-2026-09-26.md) | One current R0–R6 root-cause/acceptance union |
| [Execution plan](FINAL-RESIDUAL-EXECUTION-PLAN.md) | Concrete owners, independent negative oracles and dependency order |
| [S21-11](S21-11-state-migration-backup-and-restore.md) | Current-format owner/release/authorized-target backup and restore proof |
| [S21-12](S21-12-cross-repo-terminal-receipt-cutover.md) | Exact producer/dependency/binary receipt and operational action boundaries |
| [S21-13](S21-13-release-evidence-and-sota-qualification.md) | Trusted proof production and final registered DAG/aggregate |

The completed S21-05 read-view design is owned by SEP-21-003 and SEP-27-005;
its remaining process/lifetime acceptance is retained under execution-plan R1/P04.
S21-00 and historical lane/ticket IDs remain executable registry identities,
not live implementation tickets. Source, test/proof catalogs, `Justfile` and the
operator runbook own current commands and selected targets.

Retain immutable candidate publication versus activation CAS, exact original
operation replay, one SQLite visibility authority, request-held read handles,
truthful windows, default-deny provider admission and continuous runtime lease
custody. Reopen implemented owners only for a reproduced current failure.

[Execution commands](prompts/README.md) and
[handoff custody/schema](handoffs/README.md) remain active acceptance guidance;
these are not historical handoff bodies. Authentic old handoff audit is distinct
from current-source release qualification. Common execution and measurement
acceptance is owned once in [MISC](../../sep-27-misc/tickets/INDEX.md).
