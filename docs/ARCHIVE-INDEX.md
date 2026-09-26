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
| `docs/bugbash/sep-22-test-optimization/` | 6 | [execution SSOT](plans/sep-27-misc/tickets/INDEX.md) |
| `tickets/sep-22-test-optimization/RCA-2026-09-23-current-source.md`, `SEP23-GATE-FOLLOWUP.md` | 2 | [execution SSOT](plans/sep-27-misc/tickets/INDEX.md); old receipts are historical |
| `docs/analysis/test-execution-optimization-2026-09-21.md` | 1 | [execution SSOT](plans/sep-27-misc/tickets/INDEX.md), code-owned test authority |
| `docs/handoff/jun-24-wave2-embedding-ab-hardening.md` | 1 | Historical snapshot only; re-audit current source and [semantic ownership ledger](plans/may-25-search-owned-semantic-derivation/README.md) before reopening an item |
| `docs/ssot/may-23-storage-architecture-endgame-implementation.md` | 1 | [accepted decisions](adr/README.md), [SEP-21 residual plan](plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) |
| `docs/analysis/jun-4-dsl-capabilty.md` | 1 | [current capability entrypoint](reference/dsl-capabilities.md) and generated parity matrix |
| `docs/ssot/README.md` | 1 | [accepted decisions](adr/README.md), [active residual plan](plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) |

The purpose validation checklist, DSL capability entrypoint, Potion model
operations and operator runbook were retained. The TOPT packet was subsequently
replaced by the self-contained SEP-27 execution SSOT.
Completed plan records have a separate
[plan history index](plans/ARCHIVE-INDEX.md).

## SEP-27 execution consolidation

The four `docs/handoff/sep-27/agent-*.md` files and twelve TOPT Markdown files
were removed along with 25 RB/BM/RBR plan files. This is one 41-document
consolidation, not a claim of implementation completion. All current contracts
and remaining tickets are inline in the [execution SSOT](plans/sep-27-misc/tickets/INDEX.md).
Pre-deletion HEAD was `63f09be92fca533bd18d1d71a6464ca30e8073d1`, with dirty and
untracked documents, so Git alone does not recover every preimage.

Exact content backup, including dirty/untracked preimages and affected
navigation/closure files: `/Users/songmin/Documents/qi-docs-ssot-backup-sep27.bubnQ2/before-consolidation.tar.gz`,
SHA-256 `8c581c274d7c6f302d0726982a4471427d7bb957fe84042c4c54b8d4f4e39d81`.
All 56 regular file bodies were compared with the live preimage and matched.
Extended-attribute warnings do not affect that byte comparison; this is content
recovery, not filesystem-metadata backup. Recover into a separate directory,
not over the current worktree. The archive is not a live authority.
