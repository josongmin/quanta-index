# Tickets — LanceDB Adoption

> Archive status: `Historical program record`. Current architecture: [MAY-31-001](../../../adr/MAY-31-001-lancedb-semantic-generation-authority.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


Status: **all tickets `done` (lancedb rewrite + R1+R2+R3 hardening,
2026-05-30 → 2026-05-31)** — the §3.1 in-house implementation was reversed;
the §3.2 lancedb-backed implementation landed, and three rounds of
adversarial+structural audits closed every verified finding. See each
ticket's `Status:` line and parent [../README.md](../README.md) §3.2.

Parent doc: [../README.md](../README.md)

This ticket pack replaced the pre-lancedb semantic `journal.cbor` plus boot
replay model with a persisted Lance-backed semantic generation store.

Historical pre-pack truth (no longer live authority):

- semantic query state was rebuilt into RAM on every boot
- `SemanticAuthorityStore` persisted accepted semantic batches as
  `state_root/semantic/journal.cbor`
- `SearchdRuntime::assemble` replayed the entire semantic journal into the
  builder before queries resumed

---

## 1. Execution order

| Wave | Ticket | Title | Why first |
|---|---|---|---|
| 0 | [LDB-00-truth-freeze-and-backend-decision.md](LDB-00-truth-freeze-and-backend-decision.md) | Truth freeze and backend decision | prevents another round of "planned Lance" vs live HNSW drift |
| 1 | [LDB-01-semantic-generation-layout-and-manifest-contract.md](LDB-01-semantic-generation-layout-and-manifest-contract.md) | Semantic generation layout and manifest contract | adapter code must not invent its own durable shape ad hoc |
| 2 | [LDB-02-lance-persisted-semantic-adapter.md](LDB-02-lance-persisted-semantic-adapter.md) | Lance persisted semantic adapter | creates the actual durable backend |
| 3 | [LDB-03-search-plane-runtime-and-readiness-cutover.md](LDB-03-search-plane-runtime-and-readiness-cutover.md) | Search-plane runtime and readiness cutover | removes boot replay and wires the real open path |
| 4 | [LDB-04-legacy-semantic-journal-migration.md](LDB-04-legacy-semantic-journal-migration.md) | Legacy semantic journal migration | retires the old authority cleanly |
| 5 | [LDB-E2E-01-cutover-proof-observability-and-doc-closeout.md](LDB-E2E-01-cutover-proof-observability-and-doc-closeout.md) | Cutover proof, observability, and doc closeout | proves the new backend and closes the remaining drift |

## 2. Ticket summary

| Ticket | Primary surface | Main output | Blocking deps |
|---|---|---|---|
| `LDB-00` | docs + decision freeze | one canonical live/backend statement | none |
| `LDB-01` | semantic storage contract | generation layout + manifest/marker schema | `LDB-00` |
| `LDB-02` | `quanta-index-semantic` | persisted Lance build/open/search adapter | `LDB-01` |
| `LDB-03` | `searchd`, `search-plane`, readiness | no-replay boot path + direct readiness seeding | `LDB-02` |
| `LDB-04` | migration + startup cutover | one-shot journal import / retirement posture | `LDB-03` |
| `LDB-E2E-01` | tests, metrics, prompt-manager docs | proof rails + bounded observability + drift closure | `LDB-04` |

## 3. Program exit criteria

The program is complete only when all are true:

1. semantic search can open a sealed generation directly from durable semantic
   state
2. boot no longer requires replaying all historical semantic batches
3. semantic readiness after restart is reconstructed from persisted generation
   state, not from a replayed RAM graph
4. generation pinning still isolates semantic results by
   `(repo, revision, generation)`
5. activation remains fail closed if the persisted semantic generation is
   missing, manifest-mismatched, or unsealed
6. legacy `journal.cbor` is migrated/retired instead of remaining a permanent
   parallel authority
7. documentation no longer claims a shipped Lance backend unless the live code
   actually implements it
8. restart/cold-boot proof and perf evidence are attached to the final closeout

## 4. PR slicing guidance

Recommended PR units:

1. `LDB-00` + `LDB-01`
2. `LDB-02`
3. `LDB-03`
4. `LDB-04`
5. `LDB-E2E-01`

Do not merge:

- an adapter implementation before the durable layout/manifest contract is
  frozen
- readiness seeding before direct persisted open semantics are proven
- a migration path that leaves indefinite dual-write as the steady state
- generated doc updates that still contradict the actual semantic backend

## 5. Explicitly rejected alternatives

- keep the current in-memory HNSW as the main query backend and use Lance only
  as an optional checkpoint sidecar
- preserve `SemanticAuthorityStore` forever and treat Lance open as a cache
- hide persisted-open failures by silently replaying the legacy journal
- move Lance vendor logic into `core`, `contract`, or dispatcher code
- broaden this packet into semantic authorship inversion or hybrid ranking
