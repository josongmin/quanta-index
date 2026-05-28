# May 28 LanceDB Adoption Plan

Status: `final-planning-packet`
Date: `2026-05-28`
Scope: replace the current semantic `journal.cbor` plus boot-time full replay
path with a persisted Lance-family semantic backend while preserving
generation-pinned, fail-closed search-plane semantics.

This packet is the planning SSOT for the semantic storage cutover. It is not a
claim that Lance-backed persistence already ships in the current tree.

`LanceDB` is used here as the external planning label because that is the
desired destination. The concrete Rust dependency choice (`lancedb` crate vs
lower-level `lance*` crates) stays adapter-internal and is frozen by
`LDB-00`.

---

## 1. Objective

Move semantic query serving off the current disposable in-memory HNSW rebuild
path and onto a persisted, generation-scoped semantic index.

The target is not "add a vector DB somewhere". The target is:

- no boot-time full semantic replay from `state_root/semantic/journal.cbor`
- semantic generations that open from durable generation-local state like the
  lexical adapter already does
- fail-closed generation/readiness/activation semantics preserved
- vendor knowledge contained inside `quanta-index-semantic`
- one canonical semantic authority path after migration, not long-lived dual
  write

## 2. Current live truth

Current source-backed semantic posture:

- `quanta-index-semantic` is an in-memory HNSW adapter; `SemanticAdapter::new()`
  starts empty and `build_batch()` populates RAM state
- accepted semantic batches are persisted by
  `quanta-index-search-plane::SemanticAuthorityStore` to
  `state_root/semantic/journal.cbor`
- `quanta-index-searchd::app::runtime::assemble` calls
  `bootstrap_persisted_semantic_state(...)`, which replays every persisted
  `SemanticIngestBatch` into the semantic builder on boot
- direct semantic ingest is effectively `append_batch -> build_batch ->
  ledger update`, with rollback if the live build fails after persistence
- lexical already uses durable generation directories under
  `state_root/indexes/lexical/...`; semantic does not

Current drift that this packet must not ignore:

- `README.md` still describes `quanta-index-control` and "Tantivy + Lance"
  adapter ownership, but the live control plane is inline in
  `quanta-index-search-plane` and the live semantic backend is not Lance-backed
- older planning docs mark a Lance semantic adapter as done; current live code
  does not match that claim

## 3. Direction Lock

Chosen direction: `persisted semantic generation storage inside
quanta-index-semantic`.

Required:

- semantic query-serving state must live under
  `state_root/indexes/semantic/{repo_id}/{revision_id}/g{generation}/`
- opening a semantic generation must use durable generation-local state, not a
  replayed RAM-only reconstruction of all prior batches
- `SemanticIngestBatch` remains the internal batch-native build contract unless
  a later packet explicitly replaces it
- readiness and activation remain generation-scoped and fail closed
- vendor-specific types, async bridging, and file layout knowledge stay inside
  `quanta-index-semantic` and the composition root

Forbidden:

- long-lived `journal.cbor` plus persisted Lance dual-authority mode
- silent fallback from failed Lance open to ad hoc replay rebuild
- core/contract crates learning vendor-specific Lance storage details
- per-query warm-up rebuilds or hidden "populate cache on first query"
- broadening this packet into search-owned embedding derivation or hybrid
  ranking redesign

## 4. In Scope

- semantic generation on-disk layout and manifest contract
- persisted Lance-backed semantic adapter for build/open/search
- runtime/readiness cutover from boot replay to generation open
- one-shot migration from legacy semantic journal state
- restart proof, bounded observability, and cold-boot/perf validation

## 5. Explicitly Out of Scope

- search-owned corpus/query embedding derivation
- producer-side semantic ownership inversion beyond the current internal batch
  contract
- hybrid fusion/ranking redesign
- distributed vector service, replication, or sharding
- broad metadata-filter expansion beyond current semantic result shape

## 6. Dependency Order

Execution order is fixed:

1. `LDB-00` truth freeze and exact backend decision
2. `LDB-01` generation layout and manifest/marker contract
3. `LDB-02` persisted Lance semantic adapter
4. `LDB-03` runtime/readiness cutover and boot no-replay path
5. `LDB-04` legacy journal migration and cutover posture
6. `LDB-E2E-01` proof, observability, doc-drift closure

Rules:

- `LDB-02` must not land before `LDB-01` freezes the on-disk contract
- `LDB-03` must not mark semantic ready from persisted state before `LDB-02`
  proves durable open semantics
- `LDB-04` must not introduce an indefinite dual-write steady state
- `LDB-E2E-01` starts early for expected-failing rows, but it closes only after
  the owning behavior tickets land

## 7. Ticket Pack

- [tickets/INDEX.md](tickets/INDEX.md)
- [tickets/HISTORICAL-MAP.md](tickets/HISTORICAL-MAP.md)
- [tickets/LDB-00-truth-freeze-and-backend-decision.md](tickets/LDB-00-truth-freeze-and-backend-decision.md)
- [tickets/LDB-01-semantic-generation-layout-and-manifest-contract.md](tickets/LDB-01-semantic-generation-layout-and-manifest-contract.md)
- [tickets/LDB-02-lance-persisted-semantic-adapter.md](tickets/LDB-02-lance-persisted-semantic-adapter.md)
- [tickets/LDB-03-search-plane-runtime-and-readiness-cutover.md](tickets/LDB-03-search-plane-runtime-and-readiness-cutover.md)
- [tickets/LDB-04-legacy-semantic-journal-migration.md](tickets/LDB-04-legacy-semantic-journal-migration.md)
- [tickets/LDB-E2E-01-cutover-proof-observability-and-doc-closeout.md](tickets/LDB-E2E-01-cutover-proof-observability-and-doc-closeout.md)

## 8. Exit Criteria

This packet is complete only when all are true:

1. semantic query serving no longer depends on boot-time replay of all persisted
   semantic batches
2. persisted semantic state is generation-scoped under `state_root/indexes/semantic`
3. `quanta-index-semantic` can open a sealed generation directly from durable
   state and serve search without warm-up replay
4. restart preserves semantic readiness truth without `journal.cbor`
   reconstruction
5. semantic activation remains fail closed when the persisted semantic
   generation is absent, incomplete, or manifest-incompatible
6. legacy `state_root/semantic/journal.cbor` is either migrated and retired or
   explicitly rejected; it is not a hidden second authority forever
7. generated docs and prompt-manager sources no longer claim a shipped Lance
   state that the code does not actually implement
8. cold boot/restart evidence shows the semantic cutover improves or at least
   bounds startup cost relative to full replay

## 9. Non-Negotiable Failure Modes

- shipping a persisted semantic backend while still silently rebuilding from the
  legacy journal when open fails
- mixing per-generation persisted semantic data with cross-generation mutable
  shared state that breaks generation pinning
- keeping `SemanticAuthorityStore` as a permanent second source of truth
- leaking Lance vendor types or async/database details into `quanta-index-core`
  or `quanta-index-contract`
- declaring doc closure while `README.md` / prompt-manager sources still
  contradict the implemented semantic backend
