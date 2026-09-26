# RFC-LEX-03 — Lexical authority unification (roll-up)

> Archive status: `Historical program record`. Current architecture: [JUN-02-001](../../../adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md), [JUN-06-001](../../../adr/JUN-06-001-sourcegraph-compatibility-boundary.md), and [MAY-27-002](../../../adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md). Archive map: [Completed Plan Archive](../../ARCHIVE-INDEX.md).


| field | value |
|---|---|
| Kind | roll-up spec (RFC `LEX-03` ticket roll-up) |
| Status | partially shipped — per-shard authorities green; cross-shard merge orchestrator open |
| Owner crates | [`quanta-index-lq-trigram`](../../../../crates/quanta-index-lq-trigram), [`quanta-index-lq-positions`](../../../../crates/quanta-index-lq-positions), [`quanta-index-lq-regex`](../../../../crates/quanta-index-lq-regex), [`quanta-index-lq-symbol`](../../../../crates/quanta-index-lq-symbol), [`quanta-index-lq-scorer`](../../../../crates/quanta-index-lq-scorer) |
| Constituent specs | [LEX-00](LEX-00.md), [LEX-01](LEX-01.md), [LEX-02](LEX-02.md), [LEX-03](LEX-03.md), [LEX-04](LEX-04.md), [LEX-05](LEX-05.md) |
| Last verified | 2026-05-25 |

> Roll-up bookkeeping. RFC `LEX-03` "lexical authority unification" rolls up the per-shard authorities under one engine-cohesion claim. Sub-shard specs are shipped; the residual is the cross-shard merge orchestrator that turns N per-shard candidate streams into one engine-level lexical result.

---

## §1 Purpose

RFC [`LEX-03` "lexical authority unification"](../rfc.md#ticket-pack) names the **engine cohesion** layer: the trigram, positions, regex, symbol, and scorer shards each own a slice of lexical authority, but only become a single "lexical engine" when their candidate streams merge under a shared intersect/union/predicate evaluator with a single typed-error surface.

This roll-up is satisfied when:

1. Each per-shard authority is shipped and addressable through a stable port. ([§4.1](#41-shipped))
2. A cross-shard merge orchestrator composes the shards under boolean (`AND` / `OR` / `NOT`) and predicate (`repo:has.file(...)` / `lang:` / `path:`) semantics with deterministic ordering.
3. The composed engine surfaces a single `LexicalEngineResult` shape, not N per-shard shapes.

Constituent specs close (1). (2) and (3) are the **residual gap**.

---

## §2 Background

The RFC ticket pack groups per-shard work under `LEX-04` "incremental lexical indexing kernel" and `LEX-05` "parallel executor and deterministic merge"; `LEX-03` "lexical authority unification" sits between them as the **engine** that exposes the shards as one authority. The per-subsystem spec sheets in this directory split the shard work across LEX-00..05 (see [INDEX.md §1.1](INDEX.md) for the spec-sheet → RFC roll-up map), but no single spec sheet owns the unification layer.

Producer-authorship correction ([INDEX.md §3.6](INDEX.md)) applies: each shard authority decodes producer-emitted records (chunks, symbols, parse trees); the unification layer never reaches across that boundary. All per-shard builders honour the Round-5 delta contract ([INDEX.md §3.9](INDEX.md), [producer-handoff.md §3.5](../../../ssot/producer-handoff.md)) — `upsert_X` / `remove_X` / `from_prior(...)` idempotent under replay.

Persona gating: the lexical engine ships behind `P6` (Build/CI Operator) baseline coverage per [usecase.md](../usecase.md) (P6 is the heaviest cross-shard predicate consumer; `UC-PRED-*` rows are the gate).

---

## §3 Inputs

Subsystem specs:

- [LEX-00](LEX-00.md) — `LexicalNormalizer` port + per-language analyzer dispatch.
- [LEX-01](LEX-01.md) — `LexicalScorer` + per-gen `idf_table.cbor`; BM25 (k1, b) frozen per ADR-010.
- [LEX-02](LEX-02.md) — byte-trigram shard (4k trigrams / query cap; 100k candidates pre-verify).
- [LEX-03](LEX-03.md) — phrase / positions shard; stopword filter **locked OFF forever**; 8-token adjacency default.
- [LEX-04](LEX-04.md) — RE2 regex executor + NFA estimator (`regex = "=1.10.x"` pinned).
- [LEX-05](LEX-05.md) — `SymbolRecordDecoder` (post-correction; reads producer-emitted `UpsertSymbol.symbol`).

Channel ops (per [channel-architecture.md §3.1](../../../ssot/channel-architecture.md)):

- `UpsertChunk` / `DeleteChunk` — primary lexical content authority.
- `UpsertSymbol` / `DeleteSymbol` — symbol shard authority.
- `Seal` — generation boundary.

Cross-shard delta cascade per [producer-handoff.md §3.5.2](../../../ssot/producer-handoff.md):

- `DeleteChunk` cascades to trigram + positions + scorer + symbol (via `SymbolIndexBuilder::remove_doc`) + structural.
- `DeleteSymbol` does NOT cascade.

---

## §4 Deliverables

### 4.1 Shipped (constituent specs)

| Shard | Crate | Authority |
|---|---|---|
| Normalizer | [`quanta-index-lq-text-norm`](../../../../crates/quanta-index-lq-text-norm) | LEX-00 — token stream + per-language analyzer |
| Scorer | [`quanta-index-lq-scorer`](../../../../crates/quanta-index-lq-scorer) | LEX-01 — per-gen IDF table; BM25 (k1, b) frozen |
| Trigram | [`quanta-index-lq-trigram`](../../../../crates/quanta-index-lq-trigram) | LEX-02 — byte trigrams; UTF-8 self-sync |
| Positions | [`quanta-index-lq-positions`](../../../../crates/quanta-index-lq-positions) | LEX-03 — phrase + adjacency; 8-token window |
| Regex | [`quanta-index-lq-regex`](../../../../crates/quanta-index-lq-regex) | LEX-04 — RE2 with NFA bound; lookbehind/lookahead/backref/possessive rejected at parse |
| Symbol | [`quanta-index-lq-symbol`](../../../../crates/quanta-index-lq-symbol) | LEX-05 — `SymbolRecordDecoder` (post-correction; reads `UpsertSymbol.symbol`) |

Round-5 builder hardening shipped on all four delta-applying shards (trigram / positions / symbol / scorer): `upsert_X`, `remove_X`, `from_prior(...)` per [INDEX.md §3.9](INDEX.md).

### 4.2 Residual gap (this roll-up)

| Gap | What it is | Owner |
|---|---|---|
| Cross-shard merge orchestrator | Composes per-shard candidate streams under boolean + predicate semantics; emits one `LexicalEngineResult` | new — `quanta-index-lexical::engine` (proposed) |
| `LexicalEngineResult` envelope | One shape combining `(candidate_id, doc_id, shard_origin, signal_vec, …)` across all shards | new |
| Per-shard then merge semantics | Defines how `(trigram AND positions) OR (symbol)` evaluates; predicate pushdown order | new |
| Deterministic cross-shard ordering | Tiebreak tuple inherited from LEX-06 (6 components); see [LEX-06 §1](LEX-06.md) | gated on LEX-06 (shipped) |
| Boolean evaluator | `AND` short-circuit on smallest shard; `OR` union with dedup; `NOT` subtractive | new |
| Predicate-pushdown sequencing | `repo:has.file(...)` → resolves repo set BEFORE content fanout (per [usecase.md UC-PRED-01](../usecase.md)) | new |

### 4.3 Out of scope (other roll-ups)

- Parallel fanout / parallel executor — owned by [RFC-LEX-05](RFC-LEX-05.md).
- Front-door surface — owned by [RFC-LEX-02](RFC-LEX-02.md).
- Hybrid lex+sem fusion — owned by [SEM-02](SEM-02.md) (lq-hybrid crate).

---

## §5 Implementation steps

Mostly residuals. Sequence:

1. **Re-read shard outputs** — confirm each shard returns a typed `LexicalShardCandidate { candidate_id, doc_id, shard_origin, raw_signals }` shape. Land a normalization step if shapes diverge.
2. **Land `LexicalEngineResult` envelope** in contract crate. Hand-rolled serde per D18. Lives in `quanta-index-contract::lex::engine`.
3. **Boolean evaluator** — `(LexicalShardCandidate, BoolExpr) -> Result<Vec<LexicalEngineResult>, LexicalErrorCode>`. Short-circuit AND on smallest shard, dedup OR, subtractive NOT.
4. **Predicate-pushdown sequencer** — `repo:has.file(...)` / `lang:` / `path:` resolve before content fanout. Order is `predicates → content → rank`. Inputs from RT-01 runtime metadata catalog ([RT-01 §3](RT-01.md)).
5. **Cross-shard merge** — per-shard then merge. NOT parallel (that's RFC-LEX-05). Sequential pull from each shard, merged into one heap of size `k`.
6. **Determinism rail** — same tiebreak tuple as LEX-06 (6 components: `(score, repo, gen, path, line, doc_id)`).
7. **P6 baseline coverage** — `UC-PRED-01..07` corpus rows green at engine level; `UC-LEX-*` rows green at engine level (not just per-shard).

---

## §6 Test plan

Constituent test coverage (green per individual specs):

- LEX-00 — 57 tests (normalizer).
- LEX-01 — IDF/scorer with `from_prior` property tests.
- LEX-02 — trigram with UTF-8 self-sync verification.
- LEX-03 — phrase positions with stopword-OFF invariant.
- LEX-04 — RE2 with NFA bound and forbidden-syntax parse-reject.
- LEX-05 — `SymbolRecordDecoder` from producer wire bytes.

Residual gap (owed by this roll-up):

| Suite | What it asserts |
|---|---|
| `crates/quanta-index-lexical/tests/engine_bool_and_or_not.rs` | `(trigram AND positions) OR (symbol) NOT (regex)` evaluates per-spec |
| `crates/quanta-index-lexical/tests/engine_predicate_pushdown.rs` | `repo:has.file(Cargo.toml)` narrows repo set before content fanout (per UC-PRED-01) |
| `crates/quanta-index-lexical/tests/engine_cross_shard_determinism.rs` | Two-instance run returns byte-identical `LexicalEngineResult` order |
| `crates/quanta-index-lexical/tests/engine_uc_pred_full.rs` | UC-PRED-01..07 (7 rows) green at engine level |
| `crates/quanta-index-lexical/benches/engine_p6_baseline.rs` | criterion: P6 baseline corpus p99 within global SLO contribution |

---

## §7 Observability

Per [OBS-01 §4.1](OBS-01.md):

- Span `lq.exec.lexical_fanout` opens per engine invocation; `lane` attribute = `content | path | symbol | regex`.
- Per-shard child spans `lq.exec.shard{shard_id}` carry `shard_duration_us`, `bytes_touched`, `candidates_emitted`.
- Span `lq.merge` carries `merge_duration_us`, `merge_tuple`.
- Metric `lq_engine_shards_consulted{query_id}` gauge.
- Metric `lq_engine_predicate_pushdown_hit_total{predicate_kind}`.

No new instrumentation owed by this roll-up beyond the OBS-01 contract.

---

## §8 Error scenarios

All routed through `LexicalErrorCode` v1 (29 variants per [PRE-CONTRACT-EXT §4](PRE-CONTRACT-EXT.md)). Engine-specific surfacings:

- `PLAN_NO_SHARD_AVAILABLE` — predicate-pushdown resolved to empty repo set before content fanout.
- `EXEC_SHARD_TIMEOUT` — individual shard exceeded its slice of the engine budget.
- `EXEC_SHARD_CANCELLED` — engine-level cancel propagated.
- `STATE_NOT_READY` — at least one shard's gen is below the activation watermark.
- `STATE_GENERATION_REGRESSION` — shards disagree on the active generation.

No new codes anticipated. Fail-closed posture: a shard error fails the whole engine call, never silently drops the shard.

---

## §9 Performance envelope

Aggregate engine SLO (per [rfc.md § Capacity and SLO Targets](../rfc.md)):

| Stage | p99 budget |
|---|---|
| Predicate pushdown | ≤ 5 ms |
| Per-shard scan (max across shards) | ≤ 40 ms |
| Cross-shard merge | ≤ 5 ms |
| Engine-level dedup + cap to k | ≤ 2 ms |

Sum ≤ 52 ms at the engine boundary; combined with front-door (3 ms) and rank (per LEX-06) the global p99 budget holds. Gated on P6 baseline corpus.

---

## §10 Risks

| ID | Risk | Mitigation |
|---|---|---|
| RU-LEX-03-1 | Boolean evaluator diverges from RFC-LEX-05's parallel fanout semantics | Land RFC-LEX-03 sequential semantics first; RFC-LEX-05 preserves them under parallelization |
| RU-LEX-03-2 | Predicate pushdown order leaks shard internals | Lock order in this roll-up's §5; RT-01 catalog reads are the only allowed pre-fanout I/O |
| RU-LEX-03-3 | Cross-shard determinism breaks under shard gen-skew | Honour `STATE_GENERATION_REGRESSION` — engine refuses to merge across gens |
| RU-LEX-03-4 | Stopword-OFF invariant (per [LEX-03 §1](LEX-03.md)) re-introduced via predicate pushdown | Invariant is shard-level; engine never re-tokenizes |

---

## §11 Definition of Done (provable sub-checklist)

- ✓ [LEX-00 §11 DoD](LEX-00.md) — normalizer port
- ✓ [LEX-01 §11 DoD](LEX-01.md) — scorer + IDF
- ✓ [LEX-02 §11 DoD](LEX-02.md) — trigram
- ✓ [LEX-03 §11 DoD](LEX-03.md) — positions
- ✓ [LEX-04 §11 DoD](LEX-04.md) — regex
- ✓ [LEX-05 §11 DoD](LEX-05.md) — symbol decoder
- ✓ Round-5 `upsert_X` / `remove_X` / `from_prior(...)` on the 4 delta-applying builders ([INDEX.md §3.9](INDEX.md))
- 🔜 `LexicalEngineResult` envelope in contract crate
- 🔜 Boolean evaluator (`AND` / `OR` / `NOT`) lands at engine level
- 🔜 Predicate-pushdown sequencer for `repo:has.file(...)` / `lang:` / `path:`
- 🔜 Sequential cross-shard merge per-spec
- 🔜 `UC-PRED-01..07` (7 rows) green at engine level
- 🔜 Two-instance cross-shard determinism asserted in CI

This roll-up is `done` when the 🔜 rows flip to ✓ and P6 baseline corpus passes at the engine boundary.

---

## §12 Open questions

| ID | Question | Owner |
|---|---|---|
| Q-RFC-LEX-03-1 | Where does the engine live — extend [`quanta-index-lexical`](../../../../crates/quanta-index-lexical) or new `quanta-index-lq-engine`? | crate-graph owner |
| Q-RFC-LEX-03-2 | Predicate-pushdown vs runtime metadata read order — does `meta.*` resolve before or after `repo:has.file(...)`? | RT-01 author |
| Q-RFC-LEX-03-3 | `LexicalEngineResult` shape — does it carry per-shard signal vector for LEX-06 ranker, or only the merged tuple? | LEX-06 author |
| Q-RFC-LEX-03-4 | Cross-shard generation alignment — strict equality vs ledger-published-min? | composition root |

---

## §13 References

- [rfc.md §Ticket Pack](../rfc.md#ticket-pack) — RFC `LEX-03` definition
- [rfc.md §Engine Decomposition](../rfc.md) — engine boundary
- [INDEX.md §1.2](INDEX.md) — bookkeeping gap that this roll-up closes
- [INDEX.md §3.6](INDEX.md) — producer-authorship correction
- [INDEX.md §3.9](INDEX.md) — Round-5 builder hardening (delta contract)
- [producer-handoff.md §3.5](../../../ssot/producer-handoff.md) — delta contract (cascade graph)
- [channel-architecture.md §3.1](../../../ssot/channel-architecture.md) — op catalogue
- Constituent specs: [LEX-00](LEX-00.md) · [LEX-01](LEX-01.md) · [LEX-02](LEX-02.md) · [LEX-03](LEX-03.md) · [LEX-04](LEX-04.md) · [LEX-05](LEX-05.md)
- Related roll-ups: [RFC-LEX-02](RFC-LEX-02.md) (front door) · [RFC-LEX-05](RFC-LEX-05.md) (parallel executor)
- [OBS-01 §4.1](OBS-01.md) — engine spans + metrics
- Code: [`crates/quanta-index-lexical/src`](../../../../crates/quanta-index-lexical/src) · [`crates/quanta-index-lq-trigram`](../../../../crates/quanta-index-lq-trigram) · [`crates/quanta-index-lq-positions`](../../../../crates/quanta-index-lq-positions) · [`crates/quanta-index-lq-regex`](../../../../crates/quanta-index-lq-regex) · [`crates/quanta-index-lq-symbol`](../../../../crates/quanta-index-lq-symbol) · [`crates/quanta-index-lq-scorer`](../../../../crates/quanta-index-lq-scorer)
