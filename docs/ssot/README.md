# docs/ssot — Search-plane SSOT index

This directory holds long-lived architecture notes for `quanta-index`. It is
not the full plan/ticket catalog; use `docs/plans/` for packet execution state.

## Authoritative for this repo

| Document | Role |
| --- | --- |
| [`may-23-storage-architecture-endgame-implementation.md`](may-23-storage-architecture-endgame-implementation.md) | Canonical implementation plan for the search-plane crate layout and authority model. Read the SPA-00 note at the top before trusting control-plane rows. |
| [`../plans/may-25-lexical-enhancement/dsl-proof-ledger.toml`](../plans/may-25-lexical-enhancement/dsl-proof-ledger.toml) | Machine-readable DSL proof inventory |
| [`../plans/may-25-lexical-enhancement/lexical-capability-matrix.md`](../plans/may-25-lexical-enhancement/lexical-capability-matrix.md) | Human capability matrix aligned to proof rails |
| [`../bugbash/sep-16/findings.md`](../bugbash/sep-16/findings.md) | Current production-readiness findings inventory (2026-09-16) |

## Historical archive only

| Document | Role |
| --- | --- |
| [`channel-architecture.md`](channel-architecture.md) | Pre de-channelize channel architecture |
| [`producer-handoff.md`](producer-handoff.md) | Pre de-channelize producer/search-plane handoff note |

Do not treat archive docs as live runtime truth. Prefer root [`README.md`](../../README.md), crate module docs, and the bugbash inventory for current behavior.
