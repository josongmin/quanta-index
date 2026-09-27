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
consolidation, not a claim of implementation completion. Contracts and remaining
tickets were initially inlined in the [execution SSOT](plans/sep-27-misc/tickets/INDEX.md).
Completed decisions now live in SEP-27-003/004; the SSOT retains open acceptance.
Pre-deletion HEAD was `63f09be92fca533bd18d1d71a6464ca30e8073d1`, with dirty and
untracked documents, so Git alone does not recover every preimage.

Exact content backup, including dirty/untracked preimages and affected
navigation/closure files: `/Users/songmin/Documents/qi-docs-ssot-backup-sep27.bubnQ2/before-consolidation.tar.gz`,
SHA-256 `8c581c274d7c6f302d0726982a4471427d7bb957fe84042c4c54b8d4f4e39d81`.
All 56 regular file bodies were compared with the live preimage and matched.
Extended-attribute warnings do not affect that byte comparison; this is content
recovery, not filesystem-metadata backup. Recover into a separate directory,
not over the current worktree. The archive is not a live authority.


## SEP-27 completed owner and terminal records

The latest completed L1–L5 handoff/RCA packet and completed MISC implementation
sections are consolidated in
[SEP-27-003](adr/SEP-27-003-code-search-source-and-preview-contract.md) and
[SEP-27-004](adr/SEP-27-004-benchmark-capture-and-resource-custody.md).
The [plan archive](plans/ARCHIVE-INDEX.md#sep-27-completed-code-search-and-implementation-compaction)
records the exact pre-deletion revision, removed sets and recovery commands.
Open allocation, update cost, native scoring, integration and measurement are
retained in their active owners; removing history does not qualify them.

The second completion sweep moves SEP-21 catalog/recovery/supervision/proof
decisions into [SEP-27-005](adr/SEP-27-005-catalog-recovery-supervision-and-proof-custody.md).
Its execution history, completed read-view design and June semantic seam audit
are removed; the [plan history index](plans/ARCHIVE-INDEX.md#sep-27-second-completion-sweep)
records exact recovery and remaining owners.

## SEP-27 remaining-document compaction

At `0b4839a4a8b4cf99e870b4251395b6e3df8f4a21`, the unchanged
`docs/build-resource-budget.md` was removed: operator options live in the root
README and cooperative admission/native preparation decisions in SEP-27-004.
Its source-specific unit-graph counts are historical, not an active requirement.
The purpose checklist retains all 143 G0–G13 IDs and oracle obligations while
removing repeated rail lists and stale initial routing. The lexical owner index
retains all ten canonical predicates; the machine proof ledger is unchanged.
MISC T00–T17, sample floors and comparison admission now live in SEP-26-003;
unfinished measurements, input decisions and actual-producer/platform acceptance
remain in MISC. No proof row or product qualification is closed by compaction.
Exact dirty preimages of all nine affected documents are retained outside the
repository at `/tmp/qi-doc-compaction-w8x8vuv4`; `preimages.json` records their
content digests. Git recovers the removed build-resource body at the revision above.
