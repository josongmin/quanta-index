# Documentation History Index

Status: `HISTORICAL RECOVERY INDEX`

Pre-deletion revision: `eff53181b2ab7a3d017a5c613574b12e4000b52e`.
The 26 completed or superseded non-plan Markdown records below were removed
from the live tree. Recover an exact file with
`git show eff53181:<repository-relative-path>` and enumerate a group with
`git ls-tree -r --name-only eff53181 -- <directory>`.
Recovered status and receipts are historical, not current-source proof. The
custody decision is [SEP-27-001](adr/SEP-27-001-documentation-authority-and-historical-record-custody.md).

| Removed record set | Files | Current authority |
|---|---:|---|
| `docs/analysis/quanta-index-purpose-static-audit-2026-09-21*.md` | 3 | [SEP-21 decisions](adr/SEP-21-DECISION-REGISTRY.md), active residual ledger |
| `docs/bugbash/sep-16/` | 8 | SEP-21 decisions and current-source verification |
| `docs/ssot/channel-architecture.md`, `producer-handoff.md` | 2 | [SDK/ingress](adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md) and live runtime source |
| `docs/bugbash/sep-22-test-optimization/` | 6 | [active TOPT packet](../tickets/sep-22-test-optimization/INDEX.md) |
| `tickets/sep-22-test-optimization/RCA-2026-09-23-current-source.md`, `SEP23-GATE-FOLLOWUP.md` | 2 | [current TOPT closeout](../tickets/sep-22-test-optimization/SEP25-CURRENT-CLOSEOUT.md) |
| `docs/analysis/test-execution-optimization-2026-09-21.md` | 1 | [current TOPT closeout](../tickets/sep-22-test-optimization/SEP25-CURRENT-CLOSEOUT.md), code-owned test authority |
| `docs/handoff/jun-24-wave2-embedding-ab-hardening.md` | 1 | Historical snapshot only; re-audit current source and [semantic ownership ledger](plans/may-25-search-owned-semantic-derivation/README.md) before reopening an item |
| `docs/ssot/may-23-storage-architecture-endgame-implementation.md` | 1 | [accepted decisions](adr/README.md), [SEP-21 residual plan](plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) |
| `docs/analysis/jun-4-dsl-capabilty.md` | 1 | [current capability entrypoint](reference/dsl-capabilities.md) and generated parity matrix |
| `docs/ssot/README.md` | 1 | [accepted decisions](adr/README.md), [active residual plan](plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) |

The purpose validation checklist, DSL capability entrypoint, Potion model
operations, operator runbook and current TOPT packet were retained.
Completed plan records have a separate
[plan history index](plans/ARCHIVE-INDEX.md).
