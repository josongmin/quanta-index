# LEX-06 — Ranking layer (composite ranker over IDF + signals + explain)

> Status: `Spec — Wave 4 / TDD red`
> Parent RFC: [../rfc.md](../rfc.md) — `LEX-06`, § Invariants, § Claim Discipline, § Execution Model
> Sibling docs: [../feature-scope.md](../feature-scope.md), [../usecase.md](../usecase.md), [../dsl.md](../dsl.md), [../implementation-plan.md](../implementation-plan.md)
> Repo posture: [../../../../CLAUDE.md](../../../../CLAUDE.md) — breaking-first, D18 (no serde derive), Verification rail
> Wave: 4 (parallel with `LEX-07`). Blocks `STR-01`, `RT-01`.
> Sizing per [../implementation-plan.md §5.10](../implementation-plan.md): **L** (~2–3 weeks).

---

## §1. Purpose

Land a **deterministic, explainable composite ranker** on top of the lexical recall stage, so that:

1. `LqQueryV1` execution returns `Vec<LexicalCandidate>` ordered by a **frozen-per-generation** scoring function, with a stable tiebreak tuple that survives across instances and binary versions of the same major release ([../rfc.md § Execution Model § Merge determinism rule](../rfc.md)).
2. Every score is **fully decomposed** into a typed `SearchExplanation` v2 envelope (closes `GAP-05`) so `UC-OPS-06` is provable.
3. Ranking signals are **named, weighted, and weight-pinned** — no hidden helper-string semantics, no learned model with rolling weights, no NaN coercion to a default ([../rfc.md § Non-Negotiable Invariants 4, 5, 8](../rfc.md)).
4. The `boost:` directive contribution from `LqOptionSet::boost` lowers into a typed ranker input, never an out-of-band side-channel ([../dsl.md §6.2](../dsl.md), [../feature-scope.md §1.1.4](../feature-scope.md) `boost:` row).
5. Performance budget for ranking is bounded: per-candidate ranking cost p99 ≤ specified contribution; top-k merge across shards uses a bounded heap of size `k` from `count:` option.

A wave-end `LEX-06` is `done` when (a) every `UC-LEX-*` row's `score desc` ordering is reproducible across two instances; (b) `SearchExplanation` v2 round-trips on `UC-OPS-06`; (c) IR-eval `precision@10 ≥ 0.85` against the Sourcegraph reference labels per [../rfc.md § Claim Discipline §10](../rfc.md); (d) NaN signal injection returns typed `RANK_INVALID_SIGNAL`; (e) `boost:` directive moves a candidate's score in a `SearchExplanation`-visible direction.

---

## §2. Background

### §2.1 Why this ticket exists

After `LEX-05` lands deterministic merge across shards, the **per-candidate score** is still a Tantivy BM25 raw value plus zero or one ad-hoc boosters. That is insufficient for three reasons:

1. **Sourcegraph parity** — Sourcegraph ranking blends BM25 with proximity, file-importance (README / entry-point boost), symbol-class priors, and the `boost:` query directive ([../feature-scope.md §1.1.4](../feature-scope.md) `boost:` row, [../dsl.md §6.2](../dsl.md)). The current adapter cannot reproduce that.
2. **Explainability invariant** — RFC § Non-Negotiable Invariant 4 forbids "hidden filter/ranking semantics in helper string functions". The current `SearchExplanation` is `{ summary: String }` ([../implementation-plan.md §2.1](../implementation-plan.md)); a string is not provable.
3. **Determinism invariant** — RFC § Execution Model § Merge determinism rule pins the merge tuple but does **not** pin the score function. Without a frozen-per-generation scoring function, two instances of the same major release can drift their ranking and still pass the merge tuple test. That is a silent break of [../rfc.md § Claim Discipline §8](../rfc.md).

### §2.2 What the upstream signals look like at LEX-06 entry

By Wave-4 entry (LEX-05 exit gate green):

- Tantivy chunk index ([crates/quanta-index-lexical/src/](../../../../crates/quanta-index-lexical/src/)) exposes per-hit `bm25_score: f32`, `term_freq` per leaf, `doc_len`, and (post-LEX-03) phrase position vectors.
- IDF stats per generation are computed per `LEX-01` exit gate; the planner injects an `IdfStatsRef` into each `ShardResult` row.
- Path index ([feature-scope.md §1.1.3](../feature-scope.md) `file:` / `path:` rows) carries `repo_relative_path`. Path-prior signal derives from a fixed-per-generation classifier (README → +0.05; tests/ → -0.10; default → 0.00).
- Symbol shard ([feature-scope.md §1.1.3](../feature-scope.md) `type:symbol`, [usecase.md §C](../usecase.md)) carries `symbol_kind` (Wave-0 `PRE-CONTRACT-EXT` lands `SymbolKind` enum).
- Generation catalog carries `manifest_generation`, which feeds the tiebreak tuple ([../rfc.md § Execution Model](../rfc.md)).
- `LqOptionSet::boost` carries the parsed `boost:<n>` directive value (`f32`, finite, bounded). Default = `1.0`.
- `LqOptionSet::count` carries `Bounded(n) | All`.

### §2.3 Authority position

LEX-06 is **planner-owned**, not adapter-owned. The Tantivy adapter returns raw shard recall + per-hit signals; the ranker composes them into a final `score: f32` and a typed `ScoreBreakdown` consumed by the explainer. No vendor specifics (Tantivy field IDs, Lance vectors, etc.) cross the ranker port boundary.

### §2.4 ADR slot

This ticket proposes **ADR-006**: *Composite ranker — linear weighted sum over named signals; weights frozen per generation*. Rationale and alternatives in §10. ADR-006 must be written and ratified before LEX-06 implementation start; the empty file slot is opened at Wave-0 entry per [../implementation-plan.md §10](../implementation-plan.md). ADR-010 (BM25 `k1`/`b` parameters) is a **separate** decision, owned by LEX-06 but tracked distinct from ADR-006.

---

## §3. Inputs

### §3.1 Per-candidate signals (typed)

Every shard-result row delivered to the ranker carries the following named signals. None may be `NaN`. None may be implicitly defaulted on production paths.

| Signal | Source | Type | Domain | Failure surface if absent |
|---|---|---|---|---|
| `bm25_score` | Tantivy `TextScorer` post-LEX-03 | `f32` | finite, `≥ 0.0` | `RANK_INVALID_SIGNAL{signal=bm25_score, reason=missing}` |
| `idf_score` | per-generation IDF stats from LEX-01 | `f32` | finite, `≥ 0.0` | `RANK_INVALID_SIGNAL{signal=idf_score}` |
| `term_freq` | Tantivy term frequency at hit | `u32` | `≥ 0` | typed `RANK_INVALID_SIGNAL` if leaf reports `None` |
| `phrase_proximity` | from phrase positions (LEX-03) | `f32` | `[0.0, 1.0]` | absent if no phrase leaf — set to `0.0` (explicit zero, not default) |
| `path_prior` | per-generation path-classifier table | `f32` | `[-1.0, 1.0]` | typed error if classifier table missing |
| `symbol_class_boost` | symbol shard (LEX-03) | `f32` | `[0.0, 1.0]` | only present when `type:symbol`; absent ⇒ `0.0` explicit |
| `doc_recency` | generation catalog age of doc | `f32` | `[0.0, 1.0]`, newer = higher | typed error if generation timestamp absent |
| `boost_directive` | `LqOptionSet::boost` per-clause | `f32` | finite, `[0.125, 8.0]` (cap from [../dsl.md §13](../dsl.md)) | default `1.0` is allowed; `NaN` ⇒ typed error |
| `manifest_generation` | tiebreak only, not a score input | `u64` | strictly increasing | absent ⇒ `STATE_NOT_READY: STALE_SIBLING` ([../rfc.md § Monotonicity rules](../rfc.md)) |

Every signal is a **named field**, never a positional slot — D18 hand-rolled serde on the carrier types preserves auditability ([../implementation-plan.md §1.4](../implementation-plan.md)).

### §3.2 Per-query inputs

- `LqQueryV1` canonical AST (via LEX-01) — read-only.
- `LqOptionSet { count, case, patterntype, boost, index, timeout, ... }` ([../dsl.md §6.2](../dsl.md)).
- `PublishedGenerationSet` — pinned per request at LEX-02 front door (predecessor `D11`).
- Per-tenant config: weight overrides if and only if open-question Q-LEX-06-W2 lands as "per-tenant override allowed"; see §12. Phase-1 default: **no per-tenant override**; weights are global per-generation.

### §3.3 Static / build-time inputs

- **Weight table** — frozen at build time per generation, named `RankerWeightsV1`, lives in [../../../../crates/quanta-index-core/](../../../../crates/quanta-index-core/) (subject to G-CONTROL-LOC per [../implementation-plan.md §2.3a](../implementation-plan.md)). Default values land via ADR-006 ratification.
- **Path-prior classifier table** — frozen per generation, populated at index build by `LEX-03`. Format: `BTreeMap<PathPattern, f32>` with deterministic iteration order.
- **Tiebreak tuple** — fixed: `(score DESC, repo_id ASC, manifest_generation ASC, repo_relative_path ASC, start_line ASC, candidate_id ASC)`. Extends RFC § Merge determinism rule by appending `repo_relative_path` and `start_line` to ensure tie-resolution within a single `(repo, generation)`; **the extension is documented here and surfaced as RFC-GAP follow-up**, see §10.

### §3.4 Input invariants

- Every signal must be `f32::is_finite()` before composition.
- `count:` option must be `Bounded(k)` with `k ≤ 10_000` ([../dsl.md §6.2](../dsl.md)) or `All` with the global hard cap of `100_000` ([../feature-scope.md §7](../feature-scope.md)).
- `boost:` directive value must be in `[0.125, 8.0]`; out-of-range is a parse-time error in LEX-01, not a ranker concern.
- `RankerWeightsV1` must be ratified by ADR-006 before LEX-06 land; absence ⇒ ticket is `blocked`.

---

## §4. Deliverables

### §4.1 Code surfaces

1. **`Ranker` port** in `quanta-index-core`:
   - `fn rank(candidates: Vec<RawCandidate>, weights: &RankerWeightsV1, ctx: &RankContext) -> Result<Vec<LexicalCandidate>, LexicalErrorCode>`.
   - Returns candidates in **canonical tiebreak order** (§3.3) — never unsorted.
   - Bounded heap of size `k = ctx.count_bound()` inside the impl; never materializes more than `k` ranked rows.

2. **`Explainer` port** in `quanta-index-core`:
   - `fn explain(plan: &LqPlan, ranked: &[LexicalCandidate]) -> SearchExplanation` (v2 schema, GAP-05 resolution).
   - Per-candidate `ScoreBreakdown { bm25, idf, phrase_proximity, path_prior, symbol_class_boost, doc_recency, boost_directive, final_score, weights_version }`.

3. **`RankerWeightsV1` carrier** in `quanta-index-contract`:
   - Hand-rolled serde (D18); fields: per-signal `f32` weight + version tag + frozen-per-generation hash.
   - `weights_hash: SHA-256` over canonical CBOR encoding of the weight set (mirrors [../dsl.md §11.2](../dsl.md) hashing).

4. **`SearchExplanation` v2 schema** wired in `quanta-index-contract`:
   - `{ planner_trace: PlannerTrace, engines_touched: Vec<EngineId>, early_stop_reason: Option<EarlyStopReason>, per_candidate: Vec<ScoreBreakdown>, weights_version: WeightsVersion }`.
   - PRE-CONTRACT-EXT lands the type ([../implementation-plan.md §5.1](../implementation-plan.md) DoD item 5); LEX-06 wires content.

5. **Lexical adapter wiring** in `quanta-index-lexical`:
   - `TantivyLexicalAdapter::recall_with_signals(...) -> Vec<RawCandidate>` — emits all signals from §3.1 from a single recall pass.
   - No score composition in the adapter; adapter returns raw signals only.

6. **Ranker integration in searchd**:
   - `DomainQueryEngine` (predecessor `T4.4`) composes `Ranker::rank` after the deterministic merge stage from LEX-05.
   - `SearchExplanation` populated when `LqRequest.explain_requested == true` and unpopulated otherwise; never `null`-typed when requested.

### §4.2 Test deliverables

- Unit tests per signal × per branch (NaN, out-of-range, missing).
- Integration test exercising every `UC-LEX-*` row's ordering (re-uses PRE-CONF runner).
- Property test: same `(plan, RawCandidate set, weights)` ⇒ byte-identical `Vec<LexicalCandidate>` across 1k random shuffles of input order.
- Property test: any reordering of two distinct rank tuples produces a different output order (anti-stability for distinct keys).
- Property test: ranker output is bounded by `k` for `count:Bounded(k)`; equal to total recall for `count:All` (≤ §7 ceiling).
- Criterion bench `lex_06_rank_bench` — see §9.
- IR-eval golden set per [../implementation-plan.md §5.10](../implementation-plan.md) DoD item 3.

### §4.3 Documentation deliverables

- `docs/adr/ADR-006-composite-ranker.md` — ratified before implementation.
- `docs/adr/ADR-010-bm25-parameters.md` — ratified before implementation.
- `docs/handoffs/lq-rank-1.0.md` — producer handoff describing the new `RawCandidate` signal envelope (producer side may emit additional signals; consumer ignores unknowns, fails closed on missing required).

### §4.4 Non-deliverables (anti-scope)

- **No learned model** in Phase 1 (no neural reranker, no XGBoost). ADR-006 rationale documents the exclusion. A future learned-rerank layer is a separate RFC (deferred per [../feature-scope.md §3](../feature-scope.md) "ranking ML / personalization").
- **No `select:` projection logic** — that is owned by the result-assembler layer (LEX-01 + LEX-02 wiring per [../feature-scope.md §4.1](../feature-scope.md) `select:` row); LEX-06 ranks before projection.
- **No history ranking** — `type:commit` / `type:diff` ranking is `LEX-07` scope.
- **No structural ranking** — `STR-01` owns structural candidate ordering.
- **No bridge candidate ordering** — `BRIDGE-01` consumes already-ranked candidates.
- **No per-tenant overrides** in Phase 1 (see §12 Q-LEX-06-W2).

---

## §5. Implementation steps (TDD)

Strict red-green-refactor. Each step lands one PR. Each PR opens with a failing test, lands the smallest passing change, then refactors.

### §5.1 Step 1 — Failing test for `Ranker` port shape (red)

- Add a `Ranker` trait stub in `quanta-index-core` with `rank()` returning `unimplemented!()`.
- Write `crates/quanta-index-core/tests/ranker_port_shape.rs` asserting trait signature, public methods, and that constructing a `Ranker` requires a `RankerWeightsV1` reference.
- Commit: test red.

### §5.2 Step 2 — `RankerWeightsV1` contract type (green)

- Land `RankerWeightsV1` in `quanta-index-contract` with hand-rolled serde (D18).
- Land `weights_hash: SHA-256` computation using the same canonical CBOR pipeline as `LqCanonicalHashV1` ([../dsl.md §11.1–§11.2](../dsl.md)).
- Add unit test: two `RankerWeightsV1` values with same fields ⇒ same hash; one field differs ⇒ different hash.
- Wire `WeightsVersion = (u16 generation_epoch, SHA-256 weights_hash)`.

### §5.3 Step 3 — `RawCandidate` envelope (green)

- Land `RawCandidate` type in `quanta-index-core` carrying every §3.1 signal as a named field.
- Hand-rolled serde, D18.
- Failing test: `RawCandidate::validate()` rejects NaN/non-finite in any signal slot with `RANK_INVALID_SIGNAL`.
- Pass: implement `validate()`.

### §5.4 Step 4 — Linear weighted composition (green)

- Implement `fn compose(raw: &RawCandidate, w: &RankerWeightsV1) -> Result<f32, LexicalErrorCode>`.
- Score function (frozen per ADR-006 ratification — provisional values below are placeholders pending ADR vote):

  ```
  final_score = w.bm25            * bm25_score
              + w.idf             * idf_score
              + w.phrase          * phrase_proximity
              + w.path_prior      * path_prior
              + w.symbol_class    * symbol_class_boost
              + w.doc_recency     * doc_recency
              + w.boost_directive * (boost_directive - 1.0)
  ```

  All `w.*` are `f32`, sum-of-positive-weights is **not** required to equal `1.0` (the score is interpretable, not normalized). `boost_directive` enters as `(boost - 1.0)` so that the default `boost = 1.0` contributes zero ([../dsl.md §6.2](../dsl.md) row, default identity preservation).

- Unit tests:
  - identity: `boost = 1.0` ⇒ identical final score with vs without the boost term.
  - monotone: increasing `bm25_score` with all other signals fixed ⇒ non-decreasing `final_score`.
  - NaN injection in any signal ⇒ typed `RANK_INVALID_SIGNAL`.
  - Infinity injection ⇒ typed `RANK_INVALID_SIGNAL`.

### §5.5 Step 5 — Bounded heap top-k (green)

- Implement `rank()` as a single bounded `BinaryHeap` of size `k = ctx.count_bound()`.
- Tiebreak comparator implements §3.3 tuple.
- Property test: 1k random `RawCandidate` lists × `k = 1..=1000` ⇒ output equals the deterministic sort-and-truncate reference.
- Property test: input order independent — shuffle and re-run ⇒ byte-identical output.

### §5.6 Step 6 — `SearchExplanation` v2 wiring (green)

- Wire `Explainer::explain()` to consume the `(RawCandidate, weights, final_score)` tuple per row and emit `ScoreBreakdown`.
- Schema versioning: `SearchExplanation { version: "v2", planner_trace, engines_touched, early_stop_reason, per_candidate, weights_version }`.
- Unit test: round-trip CBOR via D18 serde.
- Integration test: PRE-CONF runner against `UC-OPS-06` returns `ok` with populated `SearchExplanation` v2.

### §5.7 Step 7 — `boost:` directive integration (green)

- Wire `LqOptionSet::boost` into `RankContext::boost_directive`.
- Failing test: query with `boost:2.0` ranks a hit higher than the same query with `boost:0.5` against the same recall set.
- Pass: `compose()` reads `boost_directive` from `RankContext` per shard result.
- Conformance row `UC-LEX-15` / `UC-LEX-16` extended to assert `boost:` movement (conformance corpus authoring discipline per [../usecase.md §6](../usecase.md)).

### §5.8 Step 8 — Cross-instance reproducibility (green)

- Two-process single-binary test (per [../implementation-plan.md §8.2](../implementation-plan.md) cross-cutting rail): run the same query against the same generation set, assert `Vec<LexicalCandidate>` is byte-identical at the CBOR layer.
- Assertion is on the **full envelope** (results + explanation), not on score alone.
- This is the proof for RFC § Claim Discipline §8 (cross-instance reproducibility) at the ranking layer.

### §5.9 Step 9 — IR-eval golden set (green)

- Land `tools/ci/ir-eval/lq-core-1.0/` golden set: 10 sample queries × ~50 labeled docs each.
- Labels owned by LEX-06 author + reviewed by feature-scope owner ([../implementation-plan.md §4.5](../implementation-plan.md) Risk-mitigation).
- CI rail asserts `precision@10 ≥ 0.85`, `MAP ≥ 0.65`, `NDCG@10 ≥ 0.80` against the Sourcegraph reference.
- Drift triggers a follow-up RFC amendment per [../rfc.md § Conformance corpus ownership](../rfc.md).

### §5.10 Step 10 — Refactor + benches (refactor)

- Run `cargo clippy --workspace --all-targets -- -D warnings`.
- Run `cargo fmt --all -- --check`.
- Land `lex_06_rank_bench` (Criterion). p99 per-candidate cost ≤ §9 envelope.
- Land `cargo bench` regression guard per `just rust-bench` ([../../../../CLAUDE.md](../../../../CLAUDE.md) testing rail).

### §5.11 Step 11 — Conformance gate flip (green)

- All `UC-LEX-*` (28 rows) and `UC-OPS-06` flip from `parse_ok` to `ok` in PRE-CONF.
- Wave 4 exit gate from [../implementation-plan.md §4.5](../implementation-plan.md) becomes provable.

---

## §6. Test plan

### §6.1 Unit (per-signal × per-branch)

- `bm25_score` NaN ⇒ `RANK_INVALID_SIGNAL`.
- `idf_score` negative ⇒ `RANK_INVALID_SIGNAL`.
- `phrase_proximity` out of `[0.0, 1.0]` ⇒ `RANK_INVALID_SIGNAL`.
- `path_prior` out of `[-1.0, 1.0]` ⇒ `RANK_INVALID_SIGNAL`.
- `symbol_class_boost` absent on `type:symbol` query ⇒ `RANK_INVALID_SIGNAL`.
- `doc_recency` requires generation timestamp; absent ⇒ `STATE_NOT_READY` (forwarded, not converted to a default).
- `boost_directive` finite ∈ `[0.125, 8.0]` enforced by LEX-01; rank layer asserts finite again at compose time (defense in depth).

### §6.2 Property tests

- 1k random `RawCandidate` lists × `k ∈ {1, 10, 100, 1000}` ⇒ output equals deterministic sort-and-truncate reference.
- Permutation invariance: shuffle input ⇒ same output.
- Distinct-key separation: two `RawCandidate` rows with different tiebreak tuples never tie in output ordering.
- Hash stability: `weights_hash` stable across processes and architectures (x86_64 + aarch64 CI matrix per [../implementation-plan.md §8.2](../implementation-plan.md)).

### §6.3 Integration

- `UC-LEX-01..28`, `UC-OPS-06`, `UC-OPS-07` run through PRE-CONF against the real `TantivyLexicalAdapter`.
- `UC-EDGE-06` (timeout) confirms the ranker honors the cancel signal at the heap insertion checkpoint (per [../rfc.md § 6.5 cancellation](../rfc.md)).

### §6.4 Cross-instance reproducibility

- Two-process single-binary test asserts byte-identical CBOR envelope (results + explanation) over the same `(canonical_query_hash, generation_set, weights_hash)`.

### §6.5 IR-eval

- 10 queries × ~50 docs each, labeled relevance `[0..3]`.
- Gate: `precision@10 ≥ 0.85`, `MAP ≥ 0.65`, `NDCG@10 ≥ 0.80`.

### §6.6 Criterion benches

- `lex_06_rank_bench` — 10k random `RawCandidate` × `k = 100` ⇒ per-candidate p99 cost ≤ §9.
- Regression budget per wave: p99 may not increase >5% across the wave without an ADR ([../implementation-plan.md §8.2](../implementation-plan.md)).

### §6.7 Negative tests

- NaN in any signal ⇒ `RANK_INVALID_SIGNAL`, no degraded result, no silent zeroing.
- Empty `candidates: Vec<RawCandidate>` ⇒ empty `Vec<LexicalCandidate>` + populated `SearchExplanation` with `engines_touched` evidence; **never** a `NotFound` error (empty-but-authority-present is a legitimate `ok` shape per [../usecase.md §0](../usecase.md)).
- Weights with `weights_hash` not matching the active generation's pinned weights ⇒ `STATE_GENERATION_REGRESSION`.

---

## §7. Observability

### §7.1 OpenTelemetry spans

Per [../implementation-plan.md §9.1](../implementation-plan.md), Wave-4 turns on a new sub-span:

- `lq.rank` — sub-span of `lq.merge`. Attributes: `candidates_in`, `candidates_out (= k)`, `weights_version`, `early_stop_reason?`.

### §7.2 Metrics

| Metric | Unit | Labels | Cardinality budget |
|---|---|---|---|
| `lq.rank.candidates_in` | `count` | `{tenant_id, ticket_id=LEX-06}` | tenant_count × 1 |
| `lq.rank.candidates_out` | `count` | `{tenant_id, ticket_id=LEX-06}` | tenant_count × 1 |
| `lq.rank.duration_ms` | `milliseconds` | `{tenant_id, ticket_id=LEX-06}` | tenant_count × 1 |
| `lq.rank.weights_version` | `gauge` | `{weights_version}` | 1 × generation_epoch |
| `lq.rank.invalid_signal_count` | `count` | `{signal, tenant_id}` | 8 signals × tenant_count |
| `lq.rank.heap_overflow_count` | `count` | `{tenant_id}` | tenant_count |

Label set is closed; new labels require version bump per [../rfc.md § Metric schema](../rfc.md) and [../implementation-plan.md §9.3](../implementation-plan.md).

### §7.3 Structured logs

- One log row per request at completion, emitted by `searchd` composition root (post-LEX-02 wiring), includes `weights_version` and `ranked_count`.
- `canonical_query_hash` always present per [../rfc.md § Observability Requirements §2](../rfc.md).

### §7.4 Audit trail

- Per-tenant audit row at completion (RFC § Audit trail). Added field `weights_version`; mandatory once LEX-06 lands.
- Cardinality is bounded — weights_version is `(u16, SHA-256-prefix-7)`.

---

## §8. Error scenarios

Every failure path ships a typed `LexicalErrorCode`. No untyped error response, no silent default substitution ([../rfc.md § Non-Negotiable Invariants 8](../rfc.md), [../dsl.md §12](../dsl.md)).

| Scenario | Code | Payload | Retry semantics | Source |
|---|---|---|---|---|
| signal is NaN / infinity | `RANK_INVALID_SIGNAL` | `{signal, observed}` | not retryable | new code introduced by this ticket; lands in PRE-CONTRACT-EXT extension see §10 |
| signal out of declared domain | `RANK_INVALID_SIGNAL` | `{signal, observed, expected_domain}` | not retryable | as above |
| required signal missing | `RANK_INVALID_SIGNAL` | `{signal, reason=missing}` | not retryable | as above |
| generation timestamp absent (for `doc_recency`) | `STATE_NOT_READY: STALE_SIBLING` | `{sibling=generation_catalog, manifest_gen, sibling_gen}` | wait-and-retry | reuses RFC `STATE_NOT_READY` |
| weights_hash mismatch with active generation | `STATE_GENERATION_REGRESSION` | `{kind=weights, prev, observed}` | not retryable | RFC `STATE_*` family |
| `count:` overflow (`Bounded(k)` with `k > 10_000`) | `PARSE_INVALID_FILTER_VALUE` (LEX-01) — never reaches ranker | n/a | n/a | enforced upstream |
| empty candidate set after recall | **not an error** | `LexicalQueryResponse { results: [], generation, explanation }` | n/a | repo convention per [../usecase.md §0](../usecase.md) |
| upstream cancel signal observed at heap insertion | `EXEC_MERGE_CANCEL` | `{at_checkpoint=rank_heap}` | retryable | RFC `EXEC_*` family |
| boost out of `[0.125, 8.0]` | `PARSE_INVALID_FILTER_VALUE` (LEX-01) — never reaches ranker | n/a | n/a | enforced upstream |

### §8.1 New error code surface

**`RANK_INVALID_SIGNAL`** is a new code in the `EXEC_*` family of [../rfc.md § Error Code Taxonomy](../rfc.md). It must be added to PRE-CONTRACT-EXT scope; if PRE-CONTRACT-EXT has already landed by the time LEX-06 starts, the code lands as a **minor version bump** of `LexicalErrorCode` per [../implementation-plan.md §7.2](../implementation-plan.md). The minor bump is additive; producer-side decoders treat unknown codes as `Other` per the contract crate's hand-rolled serde and surface a warning, not a hard failure.

### §8.2 Failure model invariant

- No score is ever silently coerced to `0.0` on signal absence.
- No candidate is silently dropped on a signal validation failure — the **whole rank call** fails closed with the typed code.
- The ranker is fail-closed at the **call** boundary; partial-rank degradation is not in scope (matches partial-shard policy in [../rfc.md § 6.5](../rfc.md)).

---

## §9. Performance envelope

### §9.1 Per-candidate cost

- p50 per-candidate `compose()` cost: ≤ `0.5 µs`.
- p99 per-candidate `compose()` cost: ≤ `5 µs`.
- p99 per-`rank()` call cost: ≤ `(k_out × 5 µs) + (k_in × log₂ k_out × 1 µs)` for `k_in` recall + `k_out` heap.

### §9.2 Memory envelope

- Bounded heap: `O(k)` `RawCandidate` references (no clone until top-k finalization).
- No persistent allocator state; ranker is stateless aside from the borrowed `RankerWeightsV1`.

### §9.3 Top-k fanout

- Per-shard recall returns its own top-`k` ranked locally (cheap pre-narrow), then the deterministic merge stage from LEX-05 combines via heap of size `k`. **Per RFC § Execution Model**, the merge tuple uses the ranker's `final_score` as the primary key; ranker is therefore on the critical path of merge.
- For `count:all`, the hard ceiling from [../feature-scope.md §7](../feature-scope.md) (`100_000`) is enforced at the front-door (LEX-02); the ranker never sees more than that.

### §9.4 Wave-4 SLO contribution

Per [../implementation-plan.md §9.2](../implementation-plan.md):

- Wave 4 exit: end-to-end p99 < 250 ms on `UC-LEX-*` (warm cache).
- Ranking contribution to that budget: ≤ `25 ms` p99 at `count:100`, ≤ `100 ms` p99 at `count:1000`.
- Above contribution measured by `lex_06_rank_bench` running against synthetic shard-result fixtures of size `1 000`, `10 000`, `100 000`.

---

## §10. Risks

| ID | Description | Prob | Impact | Early-warning signal | Mitigation |
|---|---|---|---|---|---|
| R-LEX-06-01 | ADR-006 (linear vs learned) churn after partial impl | M | H | reviewer disagreement on ADR draft; PR sits >1 week | gate LEX-06 impl on ratified ADR-006; do not start Step 1 until ADR is green |
| R-LEX-06-02 | Weights drift across generations breaks RFC § Claim Discipline §8 | M | H | cross-instance reproducibility test diverges | `weights_hash` pinned in `WeightsVersion`; any change is a minor bump; reproducibility CI step |
| R-LEX-06-03 | IR-eval golden set is subjective | M | M | reviewer disagreement on labels > 20% | ≥2 reviewers per label; drop disagreement rows; per [../implementation-plan.md §6 R11](../implementation-plan.md) |
| R-LEX-06-04 | BM25 `k1`/`b` parameter choice (ADR-010) diverges from Sourcegraph | M | M | `precision@10` < 0.85 vs reference labels | ADR-010 ratifies parity-first defaults; documented divergence requires RFC amendment |
| R-LEX-06-05 | `boost:` directive composition introduces non-monotone score (boost > 1.0 lowers score) | L | H | unit test "monotone in boost" red | identity check in §5.4 unit test; reviewed in code review checklist |
| R-LEX-06-06 | NaN propagation through Tantivy 0.22 scorer in adversarial corpora | L | H | criterion bench OOM or panic | upstream signal validation at `RawCandidate::validate()`; fuzz harness on signal-set fixtures |
| R-LEX-06-07 | Heap allocator churn on large `count:all` | L | M | `lex_06_rank_bench` p99 jumps >5× | bounded heap + reserve-`k`-up-front; criterion regression budget |
| R-LEX-06-08 | RFC-GAP: merge tiebreak extension (`repo_relative_path`, `start_line`) not in RFC § Execution Model | H | M | RFC link-check disagrees with our claim | file RFC-GAP-LEX-06-1 follow-up; surfaced in §13 References |
| R-LEX-06-09 | Per-tenant override request lands mid-impl (Q-LEX-06-W2) | M | M | tenant config schema PR opens against Phase-1 | hold Phase-1 to global-per-generation; per-tenant override is a separate ticket |
| R-LEX-06-10 | `RankerWeightsV1` SHA-256 over CBOR diverges from `LqCanonicalHashV1` hash family | L | M | hash inconsistency between two `*_hash` carriers | reuse the same canonical CBOR pipeline; one helper in `quanta-index-core` |

---

## §11. Definition of Done (provable)

Each item names the specific evidence — test path, bench name, conformance row id — required for `done`. Anything short is `blocked` per [../../../../CLAUDE.md § Verification](../../../../CLAUDE.md).

1. **ADR-006 ratified** — file `docs/adr/ADR-006-composite-ranker.md` committed, status `Accepted`.
2. **ADR-010 ratified** — file `docs/adr/ADR-010-bm25-parameters.md` committed, status `Accepted`.
3. **`Ranker` port lands** — trait + impl in `quanta-index-core`; test `ranker_port_shape.rs` green.
4. **`RankerWeightsV1` lands** — type in `quanta-index-contract`; hand-rolled serde green under semgrep `rust-no-serde-derive`; weights_hash stability test green.
5. **`RawCandidate` validation** — `raw_candidate_validate.rs` unit test green for every signal × every branch.
6. **Linear composition** — `ranker_compose.rs` unit tests green (identity, monotone, NaN reject, infinity reject).
7. **Bounded heap top-k** — `ranker_topk_property.rs` property test green (1k random × k ∈ {1, 10, 100, 1000}).
8. **Permutation invariance** — `ranker_permutation_invariance.rs` property test green.
9. **`SearchExplanation` v2 wiring** — `UC-OPS-06` in PRE-CONF flips to `ok` with populated breakdown.
10. **`boost:` directive integration** — `UC-LEX-15-with-boost.toml` golden row green; positive movement asserted.
11. **Cross-instance reproducibility** — two-process CI step asserts byte-identical CBOR envelope on `UC-LEX-01` and `UC-LEX-22`.
12. **IR-eval gate green** — `tools/ci/ir-eval/lq-core-1.0/` runs; `precision@10 ≥ 0.85`, `MAP ≥ 0.65`, `NDCG@10 ≥ 0.80` against Sourcegraph reference labels.
13. **All 28 `UC-LEX-*` rows pass with `ok`** in PRE-CONF.
14. **`UC-OPS-06` (explain) and `UC-OPS-07` (count:all determinism)** pass with `ok`.
15. **Criterion bench `lex_06_rank_bench`** registered; p99 contribution ≤ §9.4 envelope.
16. **Negative tests** — NaN ⇒ `RANK_INVALID_SIGNAL`; empty recall ⇒ empty `Vec` + populated explanation; weights hash mismatch ⇒ `STATE_GENERATION_REGRESSION`.
17. **OpenTelemetry sub-span `lq.rank`** emits per [../implementation-plan.md §9.1](../implementation-plan.md) Wave 4 row.
18. **`weights_version` field** present in audit log row per §7.4.
19. **`cargo clippy --workspace --all-targets -- -D warnings`** green.
20. **`cargo fmt --all -- --check`** green.
21. **`cargo deny`** green.
22. **`cargo machete`** green.
23. **Semgrep `rust-no-serde-derive`** green on every new type ([../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml)).
24. **Structured agent output** validates against [`agent_output.schema.json`](../../../../tools/ci/agent/agent_output.schema.json) per [../implementation-plan.md §1.4](../implementation-plan.md).
25. **RFC § Claim Discipline §10** provable — IR-eval `precision@10 ≥ 0.85` against Sourcegraph reference; cited test path in PR description.
26. **RFC § Claim Discipline §8** provable at ranker layer — cross-instance byte-identical envelope.
27. **No `unwrap` / `unwrap_or` / `Result::ok`** on production paths (clippy disallowed-methods rail per [../implementation-plan.md §1.4](../implementation-plan.md)).
28. **No `#[derive(Serialize)]` / `#[derive(Deserialize)]`** on new types (D18 per [../../../../CLAUDE.md § Build hygiene](../../../../CLAUDE.md)).

---

## §12. Open questions

| Q-ID | Question | Source | Blocks |
|---|---|---|---|
| Q-LEX-06-W1 | **Weight source** — are weights hardcoded constants in source, a config file, or a per-generation manifest field? Default proposal: hardcoded constants in `quanta-index-core` for Phase 1, with `weights_hash` carrying their identity. A config file is a Phase 4+ feature. | this ticket | §3.3, §5.2 |
| Q-LEX-06-W2 | **Per-tenant override** — should `RankerWeightsV1` be overridable per tenant? Default proposal: **no in Phase 1**. Rationale: RFC § Authz model owns tenant scope; per-tenant ranking interaction is unspecified and risks RFC § Non-Negotiable Invariant 4 (hidden ranking semantics). Open follow-up if a security-audit persona requests deterministic per-tenant boost. | this ticket | §3.2, §4.4 |
| Q-LEX-06-W3 | **ADR-006 — linear vs learned** — is linear weighted sum the right choice, or should we leave room for a learned reranker? Default proposal: **linear, frozen-per-generation** per RFC § Non-Negotiable Invariant 4 (deterministic explainable rerank only). | ADR-006 | §5.1 entry |
| Q-LEX-06-W4 | **Tiebreak tuple extension** — RFC § Execution Model § Merge determinism rule lists 4 tiebreak components; this ticket extends to 6 (`+ repo_relative_path, + start_line`). Should the extension fold into the RFC or stay as a LEX-06 local invariant? Default proposal: **RFC amendment** — file RFC-GAP-LEX-06-1 against [../rfc.md § Execution Model](../rfc.md). | this ticket | §3.3 |
| Q-LEX-06-W5 | **`RANK_INVALID_SIGNAL` placement in taxonomy** — `EXEC_*` family or new `RANK_*` family? Default proposal: **`EXEC_*` family** (ranking is execution-time); document as additive minor bump per [../implementation-plan.md §7.2](../implementation-plan.md). | this ticket | §8.1 |
| Q-LEX-06-W6 | **`select:` interaction** — when `select:repo` collapses to one row per repo, does the ranker rank pre-collapse or post-collapse? Default proposal: **pre-collapse** — ranker emits ordered hits, result assembler collapses while preserving the first-row representative ([../usecase.md `UC-LEX-25..26`](../usecase.md)). | this ticket | §4.4 |
| Q-LEX-06-W7 | **`count:all` cap behavior at ranker** — when `count:all` exceeds the §7 hard ceiling `100_000`, does the ranker fail closed or honor the front-door cap? Default proposal: **front-door caps; ranker never sees more than `100_000`**; matches [../feature-scope.md §9 Q7](../feature-scope.md). | feature-scope Q7 | §3.4 |
| Q-LEX-06-W8 | **Phrase proximity formula** — proximity window default = 8 tokens per [../dsl.md §5.3](../dsl.md). Is the formula `1 / (1 + min_distance)` adequate, or do we need a sigmoid? Default proposal: `1 / (1 + min_distance)` — simple, monotone, bounded `[0, 1]`. | this ticket | §3.1 |

Resolutions land in ADR-006 (W1, W3) and ADR-010 (BM25 parameters) before §5.1 implementation start.

---

## §13. References

### §13.1 Parent docs

- [rfc.md](../rfc.md) — LEX-06 ticket row in § Ticket Pack; § Execution Model § Merge determinism rule; § Non-Negotiable Invariants; § Claim Discipline §8 + §10; § Error Code Taxonomy.
- [feature-scope.md](../feature-scope.md) — §1.1.4 `boost:` row, §1.1.3 `select:` row, §4.1 LEX-06 mapping, §7 capacity targets, §9 Q-* open questions (Q6, Q7).
- [usecase.md](../usecase.md) — §A `UC-LEX-01..28`, §C `UC-SYM-*` (symbol ordering), §H `UC-OPS-06` (explain), `UC-OPS-07` (count:all determinism), §3 GAP-05.
- [dsl.md](../dsl.md) — §5.3 (adjacency-link proximity boost), §6.2 (`boost:` filter row), §11 (canonical hash pipeline reused), §13 (limits + budgets).
- [implementation-plan.md](../implementation-plan.md) — §4.5 Wave-4 wave plan; §5.10 LEX-06 DoD baseline; §6 R3, R10, R11, R13 risk rows; §8 test-rail matrix; §9.1 OBS Wave-4 row; §10 ADR-006 + ADR-010 slots.

### §13.2 Repo conventions

- [CLAUDE.md](../../../../CLAUDE.md) — breaking-first; D18 ban on serde derive; verification rail; structured agent output.
- [AGENTS.md](../../../../AGENTS.md) — shared agent router.
- [tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive` rule.
- [tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — agent output validation.

### §13.3 RFC follow-ups filed by this ticket

- **RFC-GAP-LEX-06-1** — RFC § Execution Model § Merge determinism rule lists 4 tiebreak components; ranking practice requires 6. File RFC amendment.
- **RFC-GAP-LEX-06-2** — RFC § Error Code Taxonomy does not enumerate `RANK_INVALID_SIGNAL`; PRE-CONTRACT-EXT or a minor bump introduces it.
- **RFC-GAP-LEX-06-3** — `boost:` directive's interaction with ACL injection (Q-LEX-06-W2) is unspecified; clarify in RFC § Authz interaction.

### §13.4 ADR slots

- **ADR-006** — Composite ranker: linear weighted sum over named signals; weights frozen per generation. New slot opened by this ticket; pre-seed at Wave-0 entry per [../implementation-plan.md §10](../implementation-plan.md).
- **ADR-010** — BM25 parameters `k1`, `b`: Sourcegraph parity vs Tantivy defaults. Existing slot per [../implementation-plan.md §10](../implementation-plan.md).

### §13.5 Upstream references

- Sourcegraph ranking docs: <https://sourcegraph.com/docs/code-search/working/relevance>
- BM25 reference: Robertson, Stephen E. (2009) "The Probabilistic Relevance Framework: BM25 and Beyond".
- Sourcegraph reference release tag: pinned in [../feature-scope.md § Sourcegraph compatibility delta](../feature-scope.md) (forward reference).

---

## End of ticket

This ticket binds LEX-06 to:

- one ratified ADR (ADR-006) before any code lands;
- a fully named, finite-domain signal envelope (`RawCandidate`) with hand-rolled serde;
- a frozen-per-generation `RankerWeightsV1` with `weights_hash` identity;
- a typed `SearchExplanation` v2 envelope closing GAP-05;
- a deterministic tiebreak tuple that **extends** RFC § Merge determinism rule and files RFC-GAP-LEX-06-1 against the extension;
- IR-eval `precision@10 ≥ 0.85` against Sourcegraph reference labels;
- cross-instance byte-identical envelope reproducibility — direct proof for RFC § Claim Discipline §8 at the ranking layer.

Anything short of every §11 DoD item makes the ticket `blocked`, never `ok`, per [../../../../CLAUDE.md § Verification](../../../../CLAUDE.md) and [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json).
