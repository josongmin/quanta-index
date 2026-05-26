# Historical Ticket Map

Parent packet: [../README.md](../README.md)

This file preserves the original ticket files while re-attaching active
ownership to the master closeout packet.

---

## 1. Prior Packet Ownership

| Historical packet or ticket | Current source truth | New owner here | Note |
| --- | --- | --- | --- |
| [../../may-27-structural-dsl/README.md](../../may-27-structural-dsl/README.md) | predecessor planning packet | `MSTR-03`, `MSTR-04`, `MSTR-05` | structural-only planning packet is now evidence, not active SSOT |
| [../../may-25-lexical-enhancement/README.md](../../may-25-lexical-enhancement/README.md) | active residuals remain | `MSTR-01`, `MSTR-05` | capability matrix and perf / observability residuals move here |
| [../../may-26-indexing-residue-tasks/README.md](../../may-26-indexing-residue-tasks/README.md) | residue packet shipped; broader follow-on remains | `MSTR-03`, `MSTR-04` | current subset is shipped; broader semantics continue here |

## 2. Structural and Translator Lineage

| Historical doc | Current truth | New owner here | Note |
| --- | --- | --- | --- |
| [STR-01](../../may-24-lexical-indexing-sorucegraph/tickets/STR-01.md) | partial live | `MSTR-03`, `MSTR-04` | foundational structural engine spec; broader semantics move here |
| [LXE-09](../../may-25-lexical-enhancement/tickets/LXE-09-structural-live-integration.md) | implemented truthful subset | `MSTR-03`, `MSTR-05` | runtime route is landed; successor work is semantics + proof |
| [STR-02](../../may-26-indexing-residue-tasks/tickets/STR-02-authority-matcher-tree-walk-expansion.md) | shipped | `MSTR-03` | current matcher breadth predecessor |
| [STR-03](../../may-26-indexing-residue-tasks/tickets/STR-03-native-structural-semantics-ast-ir-expansion.md) | shipped | `MSTR-03` | substrate for composition / richer semantics |
| [STR-04](../../may-26-indexing-residue-tasks/tickets/STR-04-structural-query-surface-expansion.md) | shipped subset | `MSTR-03`, `MSTR-04` | one-leaf fence and SG surface fence start here |
| [BRIDGE-02](../../may-26-indexing-residue-tasks/tickets/BRIDGE-02-sourcegraph-structural-honesty-gate.md) | shipped | `MSTR-04` | honesty gate remains required |
| [BRIDGE-03](../../may-26-indexing-residue-tasks/tickets/BRIDGE-03-sourcegraph-structural-syntax-and-lowering.md) | shipped subset | `MSTR-04` | quoted / keyword subset predecessor |

## 3. Capability, Proof, and Observability Lineage

| Historical doc | Current truth | New owner here | Note |
| --- | --- | --- | --- |
| [LXE-10](../../may-25-lexical-enhancement/tickets/LXE-10-observability-and-bridge-sink.md) | docs stale relative to source | `MSTR-05` | bounded metrics are partial-live; residual is coverage/proof normalization |
| [E2E-07](../../may-25-lexical-enhancement/tickets/E2E-07-performance-and-chaos.md) | partial closeout | `MSTR-05` | current owner rail becomes the source of truth |
| [E2E-04](../../may-25-lexical-enhancement/tickets/E2E-04-history-structural-e2e.md) | completed baseline | `MSTR-02`, `MSTR-05` | baseline e2e rows are predecessor evidence |
| [lexical-capability-matrix.md](../../may-25-lexical-enhancement/lexical-capability-matrix.md) | still normative for row truth | `MSTR-01` | active row closure belongs here |

## 4. What Remains Outside

These tickets remain related but are not owned by this packet:

- [../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-00.md](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-00.md)
- [../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-01.md](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-01.md)
- [../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-02.md](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-02.md)
- [../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-03.md](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-03.md)
- [../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-04.md](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-04.md)
- [../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-05.md](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-05.md)

Reason:

- their truth source is semantic ownership rather than DSL closeout
- they can share proof rails, but they are not successor DSL tickets
