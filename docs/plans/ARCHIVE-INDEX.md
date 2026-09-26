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
| [May-24 lexical/indexing and Sourcegraph](may-24-lexical-indexing-sourcegraph/SHIPPED.md) | 31 | repo-local closeout; unsupported and deferred cells retained as history | [JUN-02-001](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md), [MAY-27-002](../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md) |
| [May-25 lexical enhancement tickets](may-25-lexical-enhancement/tickets/INDEX.md) | 20 historical + 2 retained active proof documents | ticket execution closed; live capability cells remain proof-accounted | [JUN-02-001](../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../adr/JUN-06-001-sourcegraph-compatibility-boundary.md) |
| [May-25 SDK / ingest IPC cutover](may-25-sdk-cutover-wave-plan.md) | 1 | repo-local transport cutover landed; later combined-helper recovery gap remains active | [MAY-27-002](../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md) |

Total: 179 Markdown files classified; 176 historical records and three retained active contracts or proof documents:
[Jun-2 DSL Benchmarking RFC](jun-2-dsl-hardening/RFC-DSL-Benchmarking.md),
[May-25 lexical closeout](may-25-lexical-enhancement/README.md), and
[the lexical capability matrix](may-25-lexical-enhancement/lexical-capability-matrix.md).

## Historical records inside active packets

These records are archived individually. Their parent packet remains active and
is excluded from the completed-packet total above.

| Active packet | Historical records | Current authority |
|---|---:|---|
| `may-25-search-owned-semantic-derivation` | 1 landed provider ticket | [MAY-31-001](../adr/MAY-31-001-lancedb-semantic-generation-authority.md), [SEP-21-003](../adr/SEP-21-003-read-view-continuation-and-provider-policy.md), active parent packet |
| `sep-21-search-plane-sota-hardening` | 9 historical design tickets (`S21-01..04`, `S21-06..10`) | [SEP-21 decision registry](../adr/SEP-21-DECISION-REGISTRY.md), current residual ledger |
| `sep-23-search-config-profiles` | 1 superseded SDK proposal | [SEP-21-003](../adr/SEP-21-003-read-view-continuation-and-provider-policy.md), active [Sep-24 SDK DSL draft](sep-24-sdk-dsl-rfc.md) |
| `sep-26-bench-migration` | 1 superseded closeout receipt | [JUN-08-001](../adr/JUN-08-001-verification-hellgate-and-benchmark-separation.md), packet `CURRENT-AUDIT.md` |

Total: 12 individually archived records inside four still-active packets.

## Deliberately not archived

| Packet | Reason |
|---|---|
| `may-25-search-owned-semantic-derivation` | One provider ticket is historical; partial and deferred work remains in the parent and other tickets. |
| `search-plane-implementation-tickets.md` | Historical rows include a partial semantic/hybrid ticket and drift from the still-current SSOT owner note; it cannot be closed as one packet. |
| `jun-23-embedding-pipeline-sota` | No verified completion status. |
| `jun-7-search-product-quality` | Planned, not executed. |
| `jul-15-sota-test-hardening` | Active plan. |
| `sep-21-search-plane-sota-hardening` | Nine design tickets are historical; current-source owner proof and final release qualification remain open. |
| `sep-23-retrieval-bench` | Final admitted pair and quality/performance qualification remain open. |
| `sep-23-search-config-profiles` | The old SDK proposal is superseded; the RFC remains proposed and persisted-origin/policy-gate work is explicitly unimplemented. |
| `sep-24-*.md` | Repository-format, SDK DSL and source-preparation documents remain unapproved drafts with implementation `NOT_RUN`. |
| `sep-26-bench-migration` | The old closeout receipt is historical; current audit shows partial implementation and multiple `NOT_RUN` gates. |
| `sep-26-retrieval-remediation` | Already consolidated separately; active gaps remain. |

## Retrieval

Use `git show fdb1b0dc568bf626cb0b1d6b62133fa002663c07:<path>` for exact bytes before this archive labeling.
Archived status text and old receipts remain evidence for their recorded source only. They cannot override accepted ADRs,
checked capability inventories or current-source verification.
