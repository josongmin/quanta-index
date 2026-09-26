# Completed Plan History Index

Status: `HISTORICAL RECOVERY INDEX`

Pre-deletion revision: `eff53181b2ab7a3d017a5c613574b12e4000b52e`.
The completed or superseded plan bodies are not live documentation. Recover an exact file with
`git show eff53181:<repository-relative-path>`; enumerate a packet with
`git ls-tree -r --name-only eff53181 -- docs/plans/<packet>`.
The old status and receipt in any recovered file apply only to its recorded
source. Current decisions live in [accepted ADRs](../adr/README.md); unfinished
work remains in active tickets and qualification ledgers.

| Removed packet or record set | Markdown files | Current authority |
|---|---:|---|
| `may-24-lexical-indexing-sourcegraph` | 31 | [DSL](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [Sourcegraph](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md), [SDK](../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md) |
| `may-25-lexical-enhancement/tickets` | 20 | DSL and Sourcegraph ADRs; active capability matrix retained |
| `may-25-sdk-cutover-wave-plan.md` | 1 | [SDK](../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md) |
| `may-26-indexing-residue-tasks` | 7 | DSL ADR |
| `may-27-dsl-master-closeout` and `may-27-structural-dsl` | 18 | DSL ADR |
| `may-28-lancedb-adoption` | 9 | [Semantic generation](../adr/MAY-31-001-lancedb-semantic-generation-authority.md) |
| `jun-2-dsl-final-cut`, `jun-2-dsl-hardening`, `jun-2-dsl-advanced` | 25 | DSL and [verification](../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md) ADRs; current mechanics in [benchmark tooling](../../tools/benchmark/README.md) |
| `jun-4-dsl-extension`, `jun-4-sourcegraph-parity` | 21 | DSL and Sourcegraph ADRs |
| `jun-5-sourcegraph-tail-gaps`, `jun-6-sourcegraph-expansion` | 30 | Sourcegraph ADR |
| `jun-7-verification-hellgates` | 15 | Verification ADR |
| Superseded `jun-23-embedding-pipeline-sota/rfc.md` | 1 | [semantic ownership ADR](../adr/MAY-31-001-lancedb-semantic-generation-authority.md), [active semantic ledger](may-25-search-owned-semantic-derivation/README.md) |
| Superseded `search-plane-implementation-tickets.md` | 1 | [SEP-21 decisions](../adr/SEP-21-DECISION-REGISTRY.md), [residual plan](sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) |
| Historical children of active `sep-21-search-plane-sota-hardening` | 11 | [SEP-21 registry](../adr/SEP-21-DECISION-REGISTRY.md) and active residual ledger |
| Superseded `sep-21-search-plane-sota-hardening/tickets/ACTION-LIST.md` | 1 | [current residual audit](sep-21-search-plane-sota-hardening/tickets/CURRENT-RESIDUAL-2026-09-26.md), [execution plan](sep-21-search-plane-sota-hardening/tickets/FINAL-RESIDUAL-EXECUTION-PLAN.md) |
| Historical child of active `may-25-search-owned-semantic-derivation` | 1 | Semantic generation ADR and active parent |
| Superseded `sep-23-search-config-profiles/sdk-interface.md` | 1 | [SDK](../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md), active Sep-24 draft |
| Historical `sep-26-bench-migration` audit/closeout | 2 | [Verification ADR](../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md), [execution SSOT](sep-27-misc/tickets/INDEX.md) |
| Superseded `sep-26-bench-migration` implementation inventory/matrix | 2 | [registry](../../tools/benchmark/registry.toml), [execution SSOT](sep-27-misc/tickets/INDEX.md) |
| Completed `sep-26-bench-migration/tickets/BM-03-DECISION.md` | 1 | [Benchmark orchestration ADR](../adr/SEP-27-002-single-benchmark-orchestrator-and-typed-evidence.md); execution contract consolidated |
| Historical `sep-26-retrieval-remediation/tickets` | 17 | [SEP-26 ADRs](../adr/SEP-26-DECISION-REGISTRY.md), [execution SSOT](sep-27-misc/tickets/INDEX.md) |

Total: 215 removed historical plan Markdown files. The SEP-26 historical
archive manifest and initial `audit-evidence.json` were also removed; both are
recoverable at the same Git revision.

Still live: the May-25 lexical closeout and capability matrix, active semantic
packet, Sep-21 residual execution and Sep-23/24 drafts. RB/BM/RBR and TOPT
execution contracts now live only in the [SEP-27 SSOT](sep-27-misc/tickets/INDEX.md).
A packet is not complete merely because its superseded documents were removed.

## SEP-27 additional consolidation

Removed 25 plan Markdown files: eleven RB, eleven BM and three RBR files.
Together with four handoffs and twelve TOPT files, this is 41 documents; these
counts are additional to the historical 215 above. Contracts, acceptance
matrices and remaining work were inlined in the SSOT; no redirect stubs remain.
Exact dirty/untracked preimages are recoverable from the content backup recorded
in the [documentation recovery index](../ARCHIVE-INDEX.md#sep-27-execution-consolidation).
