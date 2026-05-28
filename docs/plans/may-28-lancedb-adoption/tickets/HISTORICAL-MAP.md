# Historical Ticket Map

Parent packet: [../README.md](../README.md)

This file maps prior semantic-storage planning and proof artifacts onto the
`may-28-lancedb-adoption` packet without moving the original files.

Normative related docs:

- [../../search-plane-implementation-tickets.md](../../search-plane-implementation-tickets.md)
- [../../may-24-lexical-indexing-sourcegraph/tickets/RFC-SEM-02.md](../../may-24-lexical-indexing-sourcegraph/tickets/RFC-SEM-02.md)
- [../../may-25-search-owned-semantic-derivation/README.md](../../may-25-search-owned-semantic-derivation/README.md)
- [../../may-25-lexical-enhancement/tickets/E2E-05-restart-replay-determinism-e2e.md](../../may-25-lexical-enhancement/tickets/E2E-05-restart-replay-determinism-e2e.md)

---

## 1. Lance lineage

| Historical doc | Current truth | New owner here | Note |
| --- | --- | --- | --- |
| [search-plane-implementation-tickets.md §T3.2](../../search-plane-implementation-tickets.md) | `drifted` | `LDB-00`, `LDB-02` | older plan/status says Lance adapter done; live semantic backend is currently in-memory HNSW plus replay |
| [README.md semantic adapter bullets](../../../../README.md) | `drifted` | `LDB-E2E-01` | generated repo docs still imply Lance ownership that current code does not implement |
| [may-24 implementation-plan.md semantic notes](../../may-24-lexical-indexing-sourcegraph/implementation-plan.md) | `historical-only` | `LDB-00` | useful as intent lineage, not as live truth |

## 2. Replay and restart lineage

| Historical doc | Current truth | New owner here | Note |
| --- | --- | --- | --- |
| [E2E-05 restart replay determinism](../../may-25-lexical-enhancement/tickets/E2E-05-restart-replay-determinism-e2e.md) | `implemented for legacy replay path` | `LDB-03`, `LDB-E2E-01` | restart proof must be rewritten around persisted semantic open, not replay |
| [RFC-SEM-02](../../may-24-lexical-indexing-sourcegraph/tickets/RFC-SEM-02.md) | `historical HNSW incrementality line` | `LDB-01`, `LDB-02` | prior work assumed HNSW evolution; this packet replaces that line with persisted Lance generations |

## 3. Semantic ownership lineage

| Historical doc | Current truth | New owner here | Note |
| --- | --- | --- | --- |
| [SEM-OWN packet](../../may-25-search-owned-semantic-derivation/README.md) | `deferred-follow-on` | outside this packet | ownership inversion and Lance persistence intersect but are not the same program |
| [SEM-OWN-03](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-03.md) | `deferred` | outside this packet | manifest/readiness ideas remain related, but this packet only owns the backend cutover |

## 4. What stays outside this packet

These docs remain adjacent, but they are not owned by the LanceDB adoption
packet:

- [SEM-OWN-00](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-00.md) through
  [SEM-OWN-05](../../may-25-search-owned-semantic-derivation/tickets/SEM-OWN-05.md)
- [SEM-02.md](../../may-24-lexical-indexing-sourcegraph/tickets/SEM-02.md)
- [LXE-07 semantic hybrid planner provenance](../../may-25-lexical-enhancement/tickets/LXE-07-semantic-hybrid-planner-provenance.md)

Reason:

- they own semantic authorship, hybrid behavior, or planner provenance
- this packet owns semantic persistence, restart/open semantics, and backend
  truth only
