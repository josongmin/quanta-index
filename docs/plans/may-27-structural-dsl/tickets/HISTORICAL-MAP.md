# Historical Ticket Map

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Parent packet: [../README.md](../README.md)

This file attaches the old structural/DSL tickets to the `may-27` packet
without moving the original files.

Normative parent docs:

- [../../may-24-lexical-indexing-sourcegraph/dsl.md](../../may-24-lexical-indexing-sourcegraph/dsl.md)
- [../../may-24-lexical-indexing-sourcegraph/feature-scope.md](../../may-24-lexical-indexing-sourcegraph/feature-scope.md)
- [../../may-24-lexical-indexing-sourcegraph/implementation-plan.md](../../may-24-lexical-indexing-sourcegraph/implementation-plan.md)

---

## 1. Structural engine lineage

| Historical doc | Current truth | New owner here | Note |
| --- | --- | --- | --- |
| [STR-01](../../may-24-lexical-indexing-sourcegraph/tickets/STR-01.md) | `partial-live[truthful-subset; broader semantics deferred]` | `SDL-01`, `SDL-02`, `SDL-04`, `SDL-05` | foundational structural engine spec; deferred semantics move here |
| [LXE-09](../../may-25-lexical-enhancement/tickets/LXE-09-structural-live-integration.md) | `implemented[truthful-subset]` | `SDL-01`, `SDL-E2E-01` | live runtime route already landed; successor work is semantics expansion |
| [E2E-04](../../may-25-lexical-enhancement/tickets/E2E-04-history-structural-e2e.md) | `completed` | `SDL-E2E-01` | baseline structural e2e rows are carried forward here |
| [STR-02](../../may-26-indexing-residue-tasks/tickets/STR-02-authority-matcher-tree-walk-expansion.md) | `shipped` | `SDL-01` | current matcher breadth is the predecessor surface |
| [STR-03](../../may-26-indexing-residue-tasks/tickets/STR-03-native-structural-semantics-ast-ir-expansion.md) | `shipped` | `SDL-01`, `SDL-02` | typed AST/IR substrate already landed |
| [STR-04](../../may-26-indexing-residue-tasks/tickets/STR-04-structural-query-surface-expansion.md) | `shipped` | `SDL-01`, `SDL-03` | current top-level one-leaf fence and filter fence start here |

## 2. Sourcegraph and bridge lineage

| Historical doc | Current truth | New owner here | Note |
| --- | --- | --- | --- |
| [BRIDGE-01](../../may-24-lexical-indexing-sourcegraph/tickets/BRIDGE-01.md) | `shipped` for Sourcegraph to LQ | `SDL-03` | SG translator base; structural richer subset continues here |
| [RFC-BRIDGE-01-CodeQL](../../may-24-lexical-indexing-sourcegraph/tickets/RFC-BRIDGE-01-CodeQL.md) | `deferred to v2` | `SDL-05` | direct predecessor for structural `into:codeql` |
| [BRIDGE-02](../../may-26-indexing-residue-tasks/tickets/BRIDGE-02-sourcegraph-structural-honesty-gate.md) | `shipped` | `SDL-03` | honesty gate remains a prerequisite |
| [BRIDGE-03](../../may-26-indexing-residue-tasks/tickets/BRIDGE-03-sourcegraph-structural-syntax-and-lowering.md) | `shipped` subset | `SDL-03` | quoted/keyword SG structural subset is the current baseline |

## 3. Proof and observability lineage

| Historical doc | Current truth | New owner here | Note |
| --- | --- | --- | --- |
| [LXE-10](../../may-25-lexical-enhancement/tickets/LXE-10-observability-and-bridge-sink.md) | residual metrics surface remains open | `SDL-E2E-01` | only the structural bounded-label slice is pulled forward here |
| [E2E-07](../../may-25-lexical-enhancement/tickets/E2E-07-performance-and-chaos.md) | partial closeout | `SDL-E2E-01` | structural perf/chaos rows move here |

## 4. What stays outside this packet

These old docs remain related, but they are not owned by the structural DSL
packet:

- [LXE-08](../../may-25-lexical-enhancement/tickets/LXE-08-history-live-integration.md)
- [SEM-OWN-00](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-00.md) through
  [SEM-OWN-05](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-05.md)

Reason:

- their truth sources are history/runtime/semantic ownership, not structural
  semantics
- they may intersect proof rails, but they are not successor tickets for this
  packet
