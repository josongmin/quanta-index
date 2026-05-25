# RFC-SEM-02 — Incremental semantic derivatives (roll-up)

| field | value |
|---|---|
| Kind | roll-up spec (RFC `SEM-02` ticket roll-up) |
| Status | partially shipped — semantic adapter green; cross-gen HNSW delta deferred |
| Owner crates | [`quanta-index-lq-semantic`](../../../../crates/quanta-index-lq-semantic), [`quanta-index-contract`](../../../../crates/quanta-index-contract) |
| Constituent specs | [SEM-01](SEM-01.md) (partial — adapter half) |
| Sibling NOT in scope | [SEM-02 (this directory)](SEM-02.md) ships **hybrid fusion** under the lq-hybrid crate; that is a different scope (see [INDEX.md §3.1 RFC-GAP-SEM-02-FRAMING](INDEX.md)) |
| Last verified | 2026-05-25 |

> Roll-up bookkeeping. **NOT** the same as the shipped `SEM-02.md` spec — that spec ships hybrid lex+sem fusion under [`quanta-index-lq-hybrid`](../../../../crates/quanta-index-lq-hybrid). This roll-up tracks RFC `SEM-02` "incremental semantic derivatives" (per-generation ANN re-build incrementality), which is partially shipped via SEM-01's HNSW core and is missing producer-side delta semantics + node-level diff.

---

## §1 Purpose

RFC [`SEM-02` "incremental semantic derivatives"](../rfc.md#ticket-pack) is the ticket for **per-generation ANN re-build incrementality**: when generation `N+1` opens, the semantic shard MUST reuse `N`'s HNSW graph as much as is sound, applying only the embedding-set delta. Without this, every generation pays a full HNSW rebuild cost, which is unbounded against the corpus size and violates the per-gen apply SLO.

This is **not** hybrid fusion. The local `SEM-02.md` spec file in this directory ships hybrid fusion (RRF + weighted) under `lq-hybrid` and is a name collision called out in [INDEX.md §3.1 RFC-GAP-SEM-02-FRAMING](INDEX.md). This roll-up tracks the **incremental** semantic ticket.

The roll-up is satisfied when:

1. The producer emits `UpsertEmbedding` / `DeleteEmbedding` ops with stable `embedding_id` identity (per [producer-handoff.md §3.5.1](../../../ssot/producer-handoff.md)).
2. The search-side HNSW builder honours a `from_prior(...)` API across the generation boundary, applying only the per-record delta.
3. Determinism rails (pinned RNG seed or exact-NN ≤ 100k docs) are preserved across the cross-generation inherit.

(1) is **shipped** (channel ops in contract, per [SHIPPED.md](../SHIPPED.md)). (2) is **NOT** shipped — `HnswIndex` in [`crates/quanta-index-lq-semantic/src/hnsw.rs`](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs) has no `from_prior` method (verified 2026-05-25). (3) follows from (2).

---

## §2 Background

The RFC ticket pack [§Ticket Pack](../rfc.md#ticket-pack) lists `SEM-02` as "incremental semantic derivatives". The spec sheet authored under that name in this directory ([SEM-02.md](SEM-02.md)) instead ships hybrid lex+sem fusion ("lq-hybrid"). [INDEX.md §3.1 RFC-GAP-SEM-02-FRAMING](INDEX.md) tracks the scope drift; resolution is either (a) rename `SEM-02.md` → `HYB-01.md` and author this roll-up for RFC `SEM-02`, or (b) amend the RFC ticket pack. This roll-up takes path (a) without renaming the sibling — instead a new RFC-SEM-02 file (this one) carries the original RFC scope.

[producer-handoff.md §3.5.5](../../../ssot/producer-handoff.md) locked the cross-generation delta contract: producer chooses mode **(a) FullBundle reset** or mode **(b) delta against prior gen**. Mode (b) is the steady-state path and requires a `from_prior(...)` API on every affected builder. Round-5 builder hardening landed `from_prior` on trigram / positions / symbol / scorer (per [INDEX.md §3.9](INDEX.md)); the HNSW builder is **not yet** in that list.

Producer-authorship correction ([INDEX.md §3.6](INDEX.md)) is honoured here: the search plane does not compute embeddings. All embedding bytes ride `UpsertEmbedding { embedding: EmbeddingRecord }`. Delta authority lives in the producer; the search-side responsibility is to apply the delta against a prior HNSW graph idempotently.

---

## §3 Inputs

Subsystem specs:

- [SEM-01](SEM-01.md) — semantic vector adapter + ANN integration. Ships `HnswIndex` with deterministic insert order, pinned RNG seed, cosine-distance default, `D ≤ 1024` cap, 5 new sem error codes.

Channel ops (per [channel-architecture.md §3.1](../../../ssot/channel-architecture.md)):

- `UpsertEmbedding { embedding: EmbeddingRecord }` — shipped.
- `DeleteEmbedding { embedding_id }` — shipped.
- `Seal { generation }` — shipped.
- `FullBundle { generation, payload }` — shipped (mode (a) reset path).

Contract (per [producer-handoff.md §3.5.1](../../../ssot/producer-handoff.md)):

- `embedding_id` is the producer-assigned wire identity; the search side never re-assigns it.
- Re-emission of the same `embedding_id` with a different payload is an idempotent overwrite (LWW per channel seq).

---

## §4 Deliverables

### 4.1 Shipped (constituent + Round-5)

| Item | Crate / file | Status |
|---|---|---|
| `EmbeddingRecord` wire shape | [`quanta-index-contract/src/channel`](../../../../crates/quanta-index-contract/src/channel) | shipped |
| `UpsertEmbedding` / `DeleteEmbedding` ops | same | shipped |
| `HnswIndex` with deterministic insert order | [`quanta-index-lq-semantic/src/hnsw.rs`](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs) | shipped (SEM-01) |
| Pinned RNG seed determinism | same — `HnswParams` carries the seed | shipped |
| Exact-NN ≤ 100k docs override | same | shipped |
| 5 sem error codes (`SEM_DIM_MISMATCH` / `SEM_NOT_READY` / `SEM_INVALID_VECTOR` / `SEM_METRIC_UNSUPPORTED` / `SEM_ANN_NONDETERMINISTIC`) | PRE-CONTRACT-EXT bump | shipped |

### 4.2 Residual gap (this roll-up)

| Gap | What it is | Status |
|---|---|---|
| Producer-side delta semantics for embeddings | Which embeddings changed in gen `N+1` vs gen `N`. Producer commitment, not search-side logic. | open — depends on [`semantica-codegraph-v2`](../../../ssot/producer-handoff.md) embedding-emission cadence |
| `HnswIndex::from_prior(prior, new_gen, params) -> Result<HnswIndex, SemanticError>` | Search-side API parity with trigram / positions / symbol / scorer (Round-5 builders) | NOT shipped — verified absent in [`hnsw.rs`](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs) |
| HNSW node-level diff under cross-gen delta | `upsert_embedding(id, v) -> Result<(), _>` / `remove_embedding(id) -> Result<(), _>` on the new gen, preserving graph-level entry-points where sound | NOT shipped |
| Determinism rail under cross-gen inherit | property test: `from_prior + delta` ≡ `fresh_full_rebuild` (same recall, same ordering at fixed seed) | NOT written |
| Embedding-side cascade under `DeleteChunk` | `DeleteChunk` cascade per [producer-handoff.md §3.5.2](../../../ssot/producer-handoff.md) does **not** currently list the semantic shard; review for inclusion | open |

### 4.3 Out of scope (other rolls-ups)

- Hybrid fusion (RRF / weighted) — owned by [SEM-02.md](SEM-02.md) in this directory (the lq-hybrid spec, despite the name collision).
- Parallel executor / fanout over the ANN — owned by [RFC-LEX-05](RFC-LEX-05.md).
- Producer ADR for embedding-emission cadence — owned by [producer-handoff.md](../../../ssot/producer-handoff.md).

---

## §5 Implementation steps

Mostly residuals. Sequence:

1. **Confirm producer delta-emission contract** — producer ADR locks: `UpsertEmbedding` cadence (per-chunk-change? batched?), payload hash for short-circuit no-op, `DeleteEmbedding` on `DeleteChunk` cascade. Tracked in [§12](#12-open-questions).
2. **Land `HnswIndex::from_prior`** — analogous to [`PositionsBuilder::from_prior`](../../../../crates/quanta-index-lq-positions/tests/property_from_prior_equivalence.rs). Inputs: prior `HnswIndex` + new `generation` + same `HnswParams`. Output: a new builder seeded with the prior graph, ready to accept `upsert_embedding` / `remove_embedding` deltas. Reject normalizer / dim / metric mismatch with `SEM_DIM_MISMATCH` / typed error.
3. **Land `upsert_embedding` / `remove_embedding` mutators** — idempotent on `embedding_id`. LWW per channel seq. Tombstone strategy for `remove_embedding` (HNSW node logical-delete vs graph reflow): default = tombstone + lazy compaction at `Seal`.
4. **Determinism property test** — `proptest! { fresh_full(corpus) ≡ from_prior(N) + delta(N→N+1) }` at fixed RNG seed. Lives in [`crates/quanta-index-lq-semantic/tests/property_from_prior_equivalence.rs`](../../../../crates/quanta-index-lq-semantic/tests).
5. **Cascade update** — extend [producer-handoff.md §3.5.2](../../../ssot/producer-handoff.md) cascade graph to call out the semantic shard's posture (does `DeleteChunk` purge embeddings on the same `chunk_id` / `doc_id`? Producer commitment vs search-side discovery).
6. **OBS-01 metric** — `lq_hnsw_from_prior_inherited_nodes_total` + `lq_hnsw_delta_apply_ms` per generation.

All steps are bounded; no new error codes anticipated (`SEM_*` v1 set covers).

---

## §6 Test plan

Constituent test coverage (green via [SEM-01](SEM-01.md) §6):

- `HnswIndex` insertion order determinism — seeded RNG.
- Cosine distance correctness, `D ≤ 1024` bound.
- `UC-SEM-01..12` corpus rows (12 new rows landed by SEM-01).

Residual gap (owed by this roll-up):

| Suite | What it asserts |
|---|---|
| `quanta-index-lq-semantic/tests/from_prior_round_trip.rs` | `from_prior` preserves graph; subsequent `upsert_embedding` is idempotent |
| `quanta-index-lq-semantic/tests/property_from_prior_equivalence.rs` | proptest: `from_prior + delta` ≡ `fresh_full(state)` at fixed seed |
| `quanta-index-lq-semantic/tests/cross_gen_dim_mismatch.rs` | `from_prior` rejects dim / metric / params mismatch with typed error |
| `quanta-index-lq-semantic/tests/delete_embedding_tombstone.rs` | `remove_embedding` tombstones node; subsequent query returns no row for the deleted id |
| `quanta-index-lq-semantic/benches/from_prior.rs` | criterion: per-record apply cost ≤ p99 contribution to per-gen SLO |

E2e: one row asserting the full pipeline (producer `UpsertEmbedding` → channel → search-side `from_prior + delta` → query) returns the inserted vector.

---

## §7 Observability

Per [OBS-01](OBS-01.md):

- Span `lq.build.semantic.from_prior` with attributes `prior_gen`, `new_gen`, `inherited_nodes`, `delta_applied_count`.
- Metric `lq_hnsw_from_prior_inherited_nodes_total{generation}`.
- Metric `lq_hnsw_delta_apply_ms` histogram.
- Audit event `SEM_CROSS_GEN_INHERIT` carrying `(prior_gen, new_gen, inherited_count, delta_count, fresh_full_fallback: bool)`.

`fresh_full_fallback: true` indicates the producer sent mode (a) `FullBundle` reset; not an error.

---

## §8 Error scenarios

All within `SEM_*` v1 set (no new codes). The cross-gen-specific surfacings:

- `SEM_DIM_MISMATCH` — `from_prior` called with mismatched embedding dim between prior and new gen.
- `SEM_METRIC_UNSUPPORTED` — distance metric drift between gens.
- `SEM_ANN_NONDETERMINISTIC` — RNG seed mismatch between prior and new gen (proves determinism rail).
- `STATE_GENERATION_REGRESSION` — producer mixes mode (a) and mode (b) within one gen window (per [producer-handoff.md §3.5.5](../../../ssot/producer-handoff.md)).

Refusal rule: cross-gen inherit MUST be fail-closed. No silent fallback to `fresh_full(state)` — if `from_prior` cannot inherit, the producer is informed via typed error and the generation is rejected.

---

## §9 Performance envelope

Aggregate SLO contribution (per [rfc.md § Capacity and SLO Targets](../rfc.md)):

| Path | p99 budget |
|---|---|
| `from_prior` (graph seed only) | ≤ 50 ms for ≤ 1M corpus |
| Per-record `upsert_embedding` | ≤ 0.5 ms (HNSW insert cost) |
| Per-record `remove_embedding` (tombstone) | ≤ 0.05 ms |
| `Seal`-time lazy compaction | ≤ 2× per-gen-rebuild cost, amortized |

The fresh-full path under mode (a) is permitted to dominate; this roll-up only constrains the mode (b) cross-gen delta.

---

## §10 Risks

| ID | Risk | Mitigation |
|---|---|---|
| RU-SEM-02-1 | Tombstoned nodes degrade ANN recall over many generations | Bound: tombstone fraction ≤ 30% at `Seal` triggers full compaction |
| RU-SEM-02-2 | Producer never adopts mode (b); the API is dead weight | Negotiated with producer ADR; if mode (a) is the steady-state, this roll-up resolves as "wontfix" |
| RU-SEM-02-3 | Determinism rail breaks under cross-gen inherit (RNG entropy drift) | Pin RNG seed in `HnswParams`; property test asserts equivalence |
| RU-SEM-02-4 | Name collision with [SEM-02.md](SEM-02.md) confuses readers | Resolved by this file's `RFC-SEM-02.md` filename + INDEX.md §3.1 cross-link |

---

## §11 Definition of Done (provable sub-checklist)

- ✓ [SEM-01 §11 DoD](SEM-01.md) — `HnswIndex` core + determinism rail
- ✓ `UpsertEmbedding` / `DeleteEmbedding` channel ops shipped
- ✓ `EmbeddingRecord` wire shape locked
- 🔜 `HnswIndex::from_prior(prior, new_gen, params) -> Result<HnswIndex, SemanticError>` lands in [`hnsw.rs`](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs)
- 🔜 `upsert_embedding` / `remove_embedding` mutators idempotent on `embedding_id`
- 🔜 Cross-gen property test green (`fresh_full ≡ from_prior + delta` at fixed seed)
- 🔜 `DeleteChunk` cascade decision recorded in [producer-handoff.md §3.5.2](../../../ssot/producer-handoff.md)
- 🔜 OBS-01 `lq.build.semantic.from_prior` span + `lq_hnsw_from_prior_inherited_nodes_total` metric emitted
- 🔜 Per-gen apply SLO contribution measured (criterion bench landed)
- 🔜 Producer ADR for `UpsertEmbedding` cadence merged (producer-side commitment)

This roll-up is `done` when the seven 🔜 rows flip to ✓ and producer adopts mode (b) for at least one in-tree benchmark corpus.

---

## §12 Open questions

| ID | Question | Owner |
|---|---|---|
| Q-RFC-SEM-02-1 | Producer embedding-emission cadence (per-chunk? batched? debounced?) | producer ADR |
| Q-RFC-SEM-02-2 | Tombstone vs reflow on `remove_embedding` — does ANN recall degrade enough that reflow is worth the cost? | SEM-01 author |
| Q-RFC-SEM-02-3 | `DeleteChunk` cascade — does it purge embeddings on the same `chunk_id`, or is that a separate `DeleteEmbedding` op? | producer-handoff doc |
| Q-RFC-SEM-02-4 | Cross-gen `HnswParams` mismatch — typed reject (current default) or silent param-upgrade? | search-side default = typed reject |

---

## §13 References

- [rfc.md §Ticket Pack](../rfc.md#ticket-pack) — RFC `SEM-02` definition
- [INDEX.md §1.2](INDEX.md) — bookkeeping gap that this roll-up closes
- [INDEX.md §3.1 RFC-GAP-SEM-02-FRAMING](INDEX.md) — name collision with [SEM-02.md](SEM-02.md)
- [INDEX.md §3.6](INDEX.md) — producer-authorship correction (semantic shard never computes embeddings)
- [INDEX.md §3.9](INDEX.md) — Round-5 builder-hardening delta contract; HNSW NOT yet in the `from_prior` set
- [producer-handoff.md §3.5](../../../ssot/producer-handoff.md) — delta contract (cross-gen modes, identity rules, cascade graph)
- [channel-architecture.md §3.1](../../../ssot/channel-architecture.md) — op catalogue (`UpsertEmbedding` / `DeleteEmbedding` shipped)
- Constituent spec: [SEM-01](SEM-01.md)
- Sibling (different scope): [SEM-02](SEM-02.md) (hybrid fusion, lq-hybrid crate)
- Code: [`crates/quanta-index-lq-semantic/src/hnsw.rs`](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs)
- Round-5 prior art: [`crates/quanta-index-lq-positions/tests/property_from_prior_equivalence.rs`](../../../../crates/quanta-index-lq-positions/tests/property_from_prior_equivalence.rs)
