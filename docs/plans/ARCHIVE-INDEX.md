# Completed Plan Archive

Status: `HISTORICAL`

Archive boundary: clean revision
`fdb1b0dc568bf626cb0b1d6b62133fa002663c07`, captured 2026-09-27 before this consolidation.

Completed and superseded implementation packets remain at stable paths for source links and audit history. Accepted
architecture lives in `docs/adr`. Current capability and verification status must come from live code, checked
inventories and fresh receipts.

| Packet | Archived files | Final packet state | Canonical decision |
|---|---:|---|---|
| [May-26 indexing residue](may-26-indexing-residue-tasks/README.md) | 7 | shipped | [JUN-02-001](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) |
| [May-27 DSL master closeout](may-27-dsl-master-closeout/README.md) | 9 | superseded by final cut | [JUN-02-001](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) |
| [May-27 structural DSL](may-27-structural-dsl/README.md) | 9 | superseded by final cut | [JUN-02-001](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) |
| [Jun-2 DSL final cut](jun-2-dsl-final-cut/README.md) | 10 | closed | [JUN-02-001](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) |
| [Jun-2 DSL hardening](jun-2-dsl-hardening/README.md) | 6 historical + 1 active detailed RFC | closed | [JUN-02-001](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-08-001](../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md) |
| [Jun-2 DSL advanced](jun-2-dsl-advanced/README.md) | 8 | closed | [JUN-02-001](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md) |
| [Jun-4 DSL extension](jun-4-dsl-extension/rfc.md) | 10 | landed | [JUN-02-001](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md) |
| [Jun-4 Sourcegraph parity](jun-4-sourcegraph-parity/rfc.md) | 11 | landed | [JUN-06-001](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md) |
| [Jun-5 Sourcegraph tail gaps](jun-5-sourcegraph-tail-gaps/rfc.md) | 15 | landed | [JUN-06-001](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md) |
| [Jun-6 Sourcegraph expansion](jun-6-sourcegraph-expansion/rfc.md) | 15 | landed | [JUN-06-001](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md) |
| [Jun-7 verification hellgates](jun-7-verification-hellgates/rfc.md) | 15 | landed architecture | [JUN-08-001](../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md) |
| [May-28 LanceDB adoption](may-28-lancedb-adoption/README.md) | 9 | closed | [MAY-31-001](../adr/MAY-31-001-lancedb-semantic-generation-authority.md) |

Total: 125 Markdown files classified; 124 historical records and one retained detailed active contract,
[Jun-2 DSL Benchmarking RFC](jun-2-dsl-hardening/RFC-DSL-Benchmarking.md).

## Deliberately not archived

| Packet | Reason |
|---|---|
| `may-24-lexical-indexing-sourcegraph` | Contains a partial live structural ticket and planning/spec history with separate current reconciliation. |
| `may-25-lexical-enhancement` | Proof-accounted capability ledger remains current; it explicitly does not claim every surface green. |
| `may-25-search-owned-semantic-derivation` | Partial and deferred work remains. |
| `jun-23-embedding-pipeline-sota` | No verified completion status. |
| `jun-7-search-product-quality` | Planned, not executed. |
| `jul-15-sota-test-hardening` | Active plan. |
| `sep-21-search-plane-sota-hardening` | Implementation and final release qualification remain separate. |
| `sep-23-retrieval-bench` | Final admitted pair and quality/performance qualification remain open. |
| `sep-26-bench-migration` | Partial implementation with multiple `NOT_RUN` gates. |
| `sep-26-retrieval-remediation` | Already consolidated separately; active gaps remain. |

## Retrieval

Use `git show fdb1b0dc568bf626cb0b1d6b62133fa002663c07:<path>` for exact bytes before this archive labeling.
Archived status text and old receipts remain evidence for their recorded source only. They cannot override accepted ADRs,
checked capability inventories or current-source verification.
