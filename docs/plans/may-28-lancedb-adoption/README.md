# May 28 LanceDB Adoption Plan

Status: `in-progress` (real lancedb migration, 2026-05-30)
Date: `2026-05-28`
Scope: replace the current semantic `journal.cbor` plus boot-time full replay
path with a persisted Lance-family semantic backend while preserving
generation-pinned, fail-closed search-plane semantics.

This packet is the planning SSOT for the semantic storage cutover.

History (2026-05-29 → 2026-05-30): an initial implementation landed an
in-house CBOR + HNSW backend under the same generation-scoped layout, justified
in §3.1 by cited engineering trade-offs. That implementation satisfied the
*shape* obligations but did NOT use the `lancedb` crate the packet name asks
for. The decision was re-opened: the real `lancedb` crate is now being wired
in. See §3.2 for the revised backend decision. The runtime cutover, migration
scaffolding, layout, manifest, fail-closed semantics, and full test/proof
surface from the first attempt are preserved; only the storage backend is
swapped from in-house CBOR+HNSW to lancedb.

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

## 3.1 Backend Decision — SUPERSEDED (LDB-00 initial outcome)

Frozen 2026-05-29, **superseded by §3.2 on 2026-05-30**. Retained here as the
historical record of the original decision and its stated rationale. The
implementation that followed §3.1 ran on an in-house CBOR + HNSW backend and
did not adopt the `lancedb` crate; that was the wrong call for a packet
literally named "LanceDB adoption" and was reversed in §3.2.

**Original Decision: an in-house, generation-scoped, columnar durable semantic
store inside `quanta-index-semantic`. The heavyweight async `lance` / `lancedb`
crates are NOT pulled in.**

Rationale — this repo's current port/runtime shape makes the in-house store the
better engineering choice, not a shortcut:

- **Sync port surface.** `SemanticBatchBuildPort`, `SemanticIndexOpenPort`, and
  `SemanticSearcher` are all synchronous (`-> Result<_, CoreError>`). `lance` /
  `lancedb` are async-first (tokio + `object_store`). Adopting them forces a
  `block_on` bridge and a tokio runtime *inside* the adapter — exactly the kind
  of hidden runtime coupling the hexagonal rules push against.
- **Supply-chain policy.** `deny.toml` runs `unmaintained = all`,
  `unsound = all`, `yanked = deny`. The arrow / datafusion / lance dependency
  tree (hundreds of transitive crates) would require a pile of advisory
  `ignore` exceptions to go green — which violates the spirit of the policy.
- **Build hygiene.** The repo deliberately bounds cold-build seconds (derive
  allowlist, llvm-lines budget, no-proc-macro-serde). Arrow + datafusion add
  minutes of cold-build cost and a large proc-macro-derive footprint.
- **Verifiability.** Compile/behaviour claims require a real `cargo` run. A
  ~400-crate native dependency (with `protoc` / C++ build steps) makes a green
  result environment-fragile; the in-house store compiles and tests
  deterministically with `ciborium` as its only new dependency.

What the decision keeps faithful to the packet objective: the durable *shape*
is exactly the "Lance-family" target — one columnar generation directory per
`(repo, revision, generation)`, a manifest beside the dataset, explicit
readiness/seal markers, direct open of a sealed generation with no boot replay,
and fail-closed open on missing / incomplete / mismatched state. "Lance" stays
the planning label for that shape; the bytes are an in-house CBOR columnar
shard plus a persisted HNSW graph (built once at seal, loaded — never rebuilt —
at open).

Cutover rule (frozen): `state_root/semantic/journal.cbor` is legacy migration
input only (LDB-04). After migration it is never a second live semantic
authority; the durable generation directories under
`state_root/indexes/semantic` are the sole serve-time semantic authority.

## 3.2 Backend Decision — REVISED (LDB-00 actual outcome)

Frozen 2026-05-30. **Supersedes §3.1.**

**Decision: adopt the real `lancedb` crate (currently 0.30) as the durable
semantic backend, with explicit, scoped supply-chain exceptions for its
dependency tree.** The §3.1 in-house argument was sound *if* the packet were
"add a persisted semantic store of any shape." It is not. The packet is
"adopt LanceDB." Refusing to use lancedb while keeping that packet name is a
contradiction — the right action is to pay the supply-chain / async-bridge
cost honestly and ship lancedb.

How the §3.1 objections are actually resolved:

- **Sync port surface.** The semantic adapter owns a `tokio::runtime::Runtime`
  and bridges async lancedb calls into the sync port surface with
  `Runtime::block_on`. The clippy `disallowed_methods` rule against
  `block_on` is honored by a single, narrowly-scoped `#[expect(..., reason)]`
  inside the adapter — the adapter *is* the async↔sync seam the rule names.
  Core / contract / search-plane stay sync as before.
- **Supply-chain policy.** `deny.toml` is extended with **named, scoped
  exceptions** that follow the same pattern already established for the
  `tantivy@0.22` subtree: one `lancedb@0.30` `skip-tree` entry for in-tree
  multi-version dupes, one `RUSTSEC-2024-0436` advisory ignore for the
  unmaintained `paste` proc-macro (build-time only), and four additional
  permissive licenses (`BSD-3-Clause`, `MPL-2.0`, `ISC`, `BSL-1.0` — all
  OSI-approved + FSF-libre) added to the workspace allowlist with grouped
  justification. The supply-chain *posture* (deny-by-default, justified
  exceptions only) is preserved.
- **Build hygiene.** Cold build now takes ~4–5 minutes for the semantic crate
  (vs ~10s in §3.1). Accepted cost. The llvm-lines budget remains scoped to
  `contract` + `core` (semantic adapter is not snapshotted); the no-proc-
  macro-serde rule remains binding in our own crates.
- **Verifiability.** `cargo check -p quanta-index-semantic` succeeds locally
  (probed before commit). `cargo deny check` passes with the four named
  exceptions above. If the lancedb tree fails to build in a downstream
  environment (e.g. missing `protoc`), the failure is loud and explicit, not
  silent.

What stays from the §3.1 implementation (reused, not thrown away):

- runtime cutover (`searchd::app::runtime::assemble`, `searchd::app::semantic_boot`)
- legacy-journal migration scaffolding (`LegacySemanticJournalStore`, one-shot
  `migrate_legacy_semantic_journal` with idempotent `MIGRATED` marker)
- generation directory layout (`{state_root}/indexes/semantic/{repo}/{rev}/g{gen}/`)
- `SemanticManifest` (scope metadata: repo / rev / gen / manifest_digest /
  model contract / row_count / built_at), with manual `ciborium` codec
- `MARKER_READY` / `MARKER_SEALED` explicit lifecycle markers
- `SemanticBootReport` observability surface
- the full port contract surface and all fail-closed test scenarios

What is replaced:

- `dataset.rs` (CBOR columnar shard) → lancedb-managed Arrow dataset
- `graph.rs` (CBOR HNSW persistence) → lancedb's own vector index (IVF_HNSW_SQ
  or whatever lancedb chooses; we do not run our own ANN graph)
- `hnsw.rs` (hand-rolled HNSW) → deleted
- in-house `content_checksum` → dropped (lancedb has its own dataset integrity)

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
