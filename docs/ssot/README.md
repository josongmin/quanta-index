# docs/ssot — Search-plane SSOT index

This directory holds long-lived architecture notes for `quanta-index`. It is
not the full plan/ticket catalog; use `docs/plans/` for packet execution state.

## Current authority entry points

| Document | Role |
| --- | --- |
| [`../adr/README.md`](../adr/README.md) | Accepted product decisions and decision registries. |
| [`../plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md`](../plans/sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) | Current residual implementation ledger for the Sep 16 and Sep 21 hardening lineage. |
| [`../../tools/ci/proof-authority.toml`](../../tools/ci/proof-authority.toml) | Machine-readable proof authority. A listed rail still requires a fresh exact-source receipt. |
| [`../plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`](../plans/may-25-lexical-enhancement/dsl-proof-ledger.toml) | Machine-readable DSL proof inventory |
| [`../plans/may-25-lexical-enhancement/lexical-capability-matrix.md`](../plans/may-25-lexical-enhancement/lexical-capability-matrix.md) | Human capability matrix aligned to proof rails |

## Maintained mixed-state map

| Document | Role |
| --- | --- |
| [`may-23-storage-architecture-endgame-implementation.md`](may-23-storage-architecture-endgame-implementation.md) | Older implementation map retained for unresolved rows. Read its SPA-00 note; accepted ADRs and current source take precedence. |

## Historical archive only

| Document | Role |
| --- | --- |
| [`channel-architecture.md`](channel-architecture.md) | Pre de-channelize channel architecture |
| [`producer-handoff.md`](producer-handoff.md) | Pre de-channelize producer/search-plane handoff note |
| [`../bugbash/sep-16/findings.md`](../bugbash/sep-16/findings.md) | Sep 16 frozen production-readiness audit; not a current defect inventory |

The complete non-plan archive is indexed in
[`docs/ARCHIVE-INDEX.md`](../ARCHIVE-INDEX.md). Do not treat archive documents
as live runtime truth. Prefer accepted ADRs, current source, checked inventories,
and fresh source-bound receipts.
