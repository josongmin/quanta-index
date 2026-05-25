# SEM-02 — Hybrid (Lexical + Semantic) Result Fusion

> Status: `shipped`
> Crate: `quanta-index-lq-hybrid`
> Tests: 76
> Last verified: 2026-05-25
> Parent RFC: [../rfc.md](../rfc.md) §`SEM-02` (Ticket Pack — current framing: "incremental semantic derivatives"; this spec reframes per [SEM-01.md](SEM-01.md) split — see §2.3), §`Execution Model` (merge-determinism tuple), §`Non-Negotiable Invariants`, §`Error Code Taxonomy`
> Feature scope: [../feature-scope.md](../feature-scope.md) §4.6 (cross-cutting: SEM-02), §1.5 (Bridge — adjacent; fusion ≠ bridge), §5.4 (QI-extensions)
> DSL: [../dsl.md](../dsl.md) §2 EBNF (directive surface — `hybrid(...)` lands here), §9 (directive grammar), §13 (limits — fusion budget)
> Usecase corpus: [../usecase.md](../usecase.md) §3 (currently **no** `UC-HYB-*` rows — see §6 below; forcing function `UC-GAP-1` from [../implementation-plan.md](../implementation-plan.md) Appendix A.3)
> Implementation plan: [../implementation-plan.md](../implementation-plan.md) §5.15 (current SEM-02 DoD), §4.8 (Wave 7 goal), Appendix A.3 `UC-GAP-1` (hybrid usecases missing — **forcing function for this ticket**)
> Repo invariants: [../../../../CLAUDE.md](../../../../CLAUDE.md) (Agent change posture: breaking-first; Rule Catalog), [../../../../AGENTS.md](../../../../AGENTS.md)
> Upstream ticket: [SEM-01.md](SEM-01.md) — semantic vector adapter integration (consumed by this ticket)
>
> Shipped fusion engine: RRF (default, k=60) + Weighted (opt-in). 8-component merge tuple is the deterministic ordering authority.

---

## §1 Purpose

Land hybrid (lexical + semantic) result fusion as a deterministic, explainable post-pass over two engine result streams. At ticket end, a query of shape `hybrid(lex_query, sem_query, weights={...})` returns one fused candidate stream whose ordering is bit-exact reproducible across instances and whose `SearchExplanation` carries both lexical and semantic contribution per candidate.

The ticket does **not** introduce embedding into the search plane (already non-negotiable per RFC §`Non-Goals`; [SEM-01.md](SEM-01.md) §2.2). The ticket does **not** train fusion weights at request time. The ticket does **not** ship "incremental semantic derivatives" (changed-chunk-set propagation to invalidation catalog) — that framing of SEM-02 from [../implementation-plan.md](../implementation-plan.md) §5.15 is reassigned (see §2.3 below) so the original `affected:` / `invalidated_by:` work is split out into a separate follow-on under RT-01 / SEM-derivative scope.

The closure target is RFC §`Claim Discipline` item §7 (`semantic rebased on lexical` claim — the second leg, "lexical-universe planning replaces post-filter correctness", lands here as the fusion contract).

## §2 Background

### §2.1 Forcing function — `UC-GAP-1`

[../implementation-plan.md](../implementation-plan.md) Appendix A.3 explicitly names `UC-GAP-1`:

> **UC-GAP-1** — corpus §2 lists 85 usecases + 15 anti-usecases = 100 rows but explicit hybrid-query rows (lexical ∩ semantic) are absent. SEM-01 DoD (§5.14) flags this as "conformance rows absent from corpus". Add UC-HYB-* category.

This ticket **is the forcing function** for resolving `UC-GAP-1`: §6.1 below defines the UC-HYB-* representative set, which must be filed back into [../usecase.md](../usecase.md) §2 in this ticket's PR.

### §2.2 Why fusion is a separate ticket from SEM-01

[SEM-01.md](SEM-01.md) lands the **vector adapter + planner route + `LqExpr::SemanticVector` leaf**. That ticket explicitly rejects any AST shape mixing lexical and semantic leaves without an opt-in directive (per SEM-01 §4.3 — `PARSE_UNSUPPORTED_COMBO`). The opt-in directive is the surface this ticket introduces.

Keeping the split clean prevents SEM-01 from absorbing hybrid concerns and prevents this ticket from re-deciding adapter shape.

### §2.3 RFC framing reassignment

RFC §`Ticket Pack` names `SEM-02` as "incremental semantic derivatives" and [../implementation-plan.md](../implementation-plan.md) §5.15 frames the DoD around the changed-chunk-set / invalidation catalog. **Per CLAUDE.md §`Agent change posture` (breaking-first)**, this ticket reassigns the `SEM-02` label to hybrid fusion (the corpus gap forces it), and the previous SEM-02 scope (`affected:` / `invalidated_by:` evaluation) is split into a follow-on under RT-01 closure or a new `SEM-DERIV` ticket. Both options are surfaced in §12 Q-FRAMING-1 and require a forcing-function PR before Wave 7 entry.

This is not a silent retag: §13 lists the file-back action that updates RFC §`Ticket Pack`, feature-scope.md §4.6, and implementation-plan.md §5.15 to reflect the reassignment.

### §2.4 Fusion is post-filter, not post-rank

Fusion runs **after** each engine's deterministic top-k (per engine's own merge tuple per RFC §`Execution Model`) and **before** the global merge-determinism tuple is applied to the fused stream. The fused stream then goes through the global merge tuple (RFC §`Merge determinism rule`) for the final wire envelope. See §4.4 for the strategy choice (RRF vs weighted-score).

### §2.5 Authority

Per RFC §`Claim Discipline` §7: "semantic rebased on lexical is not claimable before lexical-universe planning replaces post-filter correctness". The lexical-universe predicates (`repo:`, `file:`, `lang:`, `rev:`) **must** push down into both sub-queries (lexical and semantic) before fusion. A fusion request that fails to push down a lexical-universe predicate to the semantic engine is a planner failure — see §8 `HYB_PUSHDOWN_INCOMPLETE`.

## §3 Inputs

### §3.1 Sub-query inputs

A hybrid query carries:

- `lex_query: LqExpr` — pure-lexical AST (no `SemanticVector` leaf).
- `sem_query: LqExpr::SemanticVector` — pure-semantic AST.
- `weights: HybridWeights` — see §3.3.
- `fusion_strategy: HybridFusionStrategy::{ReciprocalRankFusion, WeightedScore}` — see §4.4.
- `top_k: u32` — fused output size; bounded `1 ≤ k ≤ 10_000` per DSL §13 ceiling.
- shared filters (lexical-universe: `repo:`, `file:`, `lang:`, `rev:`) push down into **both** sub-queries.

### §3.2 DSL directive surface

DSL grammar extension (lands in this ticket; references [../dsl.md](../dsl.md) §2 EBNF):

```
HybridDirective = "hybrid" "(" lex_subquery "," sem_subquery [ "," WeightsClause ] [ "," StrategyClause ] ")"
WeightsClause   = "weights" "=" "{" "lex" "=" weight-value "," "sem" "=" weight-value "}"
weight-value    = decimal-fraction              (* parsed as f32; must be finite and ≥ 0.0 *)
StrategyClause  = "strategy" "=" ( "rrf" | "weighted" )

(* lex_subquery is a full Expression restricted to non-semantic leaves;
   sem_subquery is the SemanticVector leaf form from SEM-01.
   Lexical-universe filters at the outer query level
   apply transitively (push-down). *)
```

Hybrid is the **only** way to combine lexical + semantic AST per the SEM-01 §4.3 rule. A bare `LqExpr::All { lexical, semantic }` outside `hybrid(...)` is still `PARSE_UNSUPPORTED_COMBO`.

### §3.3 Weights

`HybridWeights { lex: f32, sem: f32 }` with the contract:

- both finite (no NaN, no infinity);
- both `≥ 0.0`;
- at least one `> 0.0` (sum > 0 — see §8 `HYB_INVALID_WEIGHTS`);
- normalization is **implicit** at fusion time: weights are L1-normalized to sum to 1.0 internally. Caller-side normalization is not required.

Default when `WeightsClause` is omitted: `{lex: 0.5, sem: 0.5}` (matches the placeholder Phase-1 default cited in [../implementation-plan.md](../implementation-plan.md) §5.14 ADR-013 / predecessor D10). Override via DSL directive only; per-tenant weights are out of scope at MVP (Q-WEIGHTS-1).

### §3.4 Generation pinning

Per RFC §`Execution Model`, both sub-queries pin the **same** generation set (single `PublishedGenerationSet`). If `lex_query` and `sem_query` resolve to different generations (e.g., one cites an explicit `rev:` mismatched against the semantic shard), the planner returns `HYB_GEN_MISMATCH` (fail-closed; see §8). Cross-generation fusion is **forbidden** — semantic doc identity must align with lexical doc identity per RFC §`Semantic derivative model` item 2.

## §4 Deliverables

### §4.1 Contract surface (additive)

- `LqDirective::Hybrid { lex_sub: LqExpr, sem_sub: LqExpr, weights: HybridWeights, strategy: HybridFusionStrategy, top_k: u32 }` (additive variant on `LqDirective` per the directive shape laid in [SEM-01.md](SEM-01.md) and PRE-CONTRACT-EXT).
- `HybridWeights` (hand-rolled serde per D18).
- `HybridFusionStrategy::{ReciprocalRankFusion, WeightedScore}` enum.
- Extension to `LexicalCandidate` via `SearchExplanation` v2 (built on `GAP-05` resolution in PRE-CONTRACT-EXT): each candidate carries `HybridContribution { lex_rank: Option<u32>, lex_score: Option<f32>, sem_rank: Option<u32>, sem_score: Option<f32>, fused_score: f32 }` in the explanation envelope. The `LexicalCandidate.score` field stores `fused_score`.
- New error codes (added to `LexicalErrorCode`):
  - `HYB_INVALID_WEIGHTS` — weights sum to 0, or any weight NaN / negative / non-finite.
  - `HYB_GEN_MISMATCH` — lex and sem sub-queries resolve to different generations (fail-closed).
  - `HYB_PUSHDOWN_INCOMPLETE` — a lexical-universe filter (`repo:`, `file:`, `lang:`, `rev:`) failed to push down into the semantic sub-query (assertion-class; surfaces planner bug, not user error).
  - `HYB_TOP_K_INVALID` — `top_k = 0` or `top_k > 10_000`.
  - `HYB_STRATEGY_UNSUPPORTED` — strategy name parses but is not wired (e.g., a future learned-fusion strategy).
  - `HYB_SUBQUERY_INVALID` — lex sub-query contains `SemanticVector`, or sem sub-query is not `SemanticVector`.

### §4.2 Planner route

The planner detects `LqDirective::Hybrid` and:

1. Validates weights (§3.3).
2. Validates sub-query shapes (lex-side has no `SemanticVector`; sem-side is `SemanticVector`).
3. Resolves the shared generation pin once, applied to both sub-queries (§3.4).
4. Pushes down lexical-universe filters into both sub-queries; asserts completeness via the post-pushdown invariant test (§5.7 step 2). Incomplete pushdown → `HYB_PUSHDOWN_INCOMPLETE`.
5. Dispatches each sub-plan to its engine (lexical → `SearchExecutor::fanout` per LEX-05; semantic → SEM-01 path).
6. Hands the two result streams to the fusion stage.

### §4.3 Fusion stage

A new module under `quanta-index-core::domains::fusion::` (or `crates/quanta-index-fusion/` if ADR-018 picks a new crate — see §12). The module owns:

- `ReciprocalRankFusion::fuse(lex_results, sem_results, weights, k) -> Vec<FusedCandidate>`
- `WeightedScore::fuse(lex_results, sem_results, weights, k) -> Vec<FusedCandidate>`
- A canonical `merge_with_tiebreak` that applies the extended tuple from §4.5.

### §4.4 Fusion strategy decision

Two strategies are shipped at MVP; the default is **RRF** (Reciprocal Rank Fusion). Rationale and ADR candidate:

**ADR-019 candidate** (proposed by this ticket; lands in `docs/adr/ADR-019-hybrid-fusion-strategy.md`):

> **Decision**: RRF is the default hybrid fusion strategy in `LQ/Core-1.0` hybrid surface.
>
> **Context**: lexical BM25 scores and semantic cosine scores are on different scales (`BM25 ∈ [0, ~30]` empirically; `cosine ∈ [-1.0, 1.0]`). Naive weighted-score blending requires score normalization, which is non-trivial and engine-version-sensitive.
>
> **Rationale**:
> 1. RRF is **scale-free**: it operates on ranks, not raw scores. This is robust to BM25 parameter drift (LEX-06 ADR-010), Lance ANN backend drift (SEM-01 R-LANCE), and cosine-vs-L2 metric churn (SEM-01 Q-METRIC-1).
> 2. RRF is **deterministic**: same `(lex_results, sem_results, weights, k)` → same fused order. No score-normalization step that depends on data distribution.
> 3. RRF is **explainable**: each candidate's contribution decomposes into `lex_rank` and `sem_rank` integers, both visible in `SearchExplanation`.
> 4. RRF is **the upstream default** for hybrid retrieval systems (Vespa, Weaviate, Elastic hybrid). Sourcegraph parity (RFC §`Compatibility Rules`) is preserved by defaulting to the upstream-default strategy.
>
> **RRF formula** (canonical, pinned in the ADR):
> ```
> rrf_score(d) = sum over engine e of [ w_e * 1.0 / (k_rrf_constant + rank_e(d)) ]
> k_rrf_constant = 60      (* literature standard; configurable per deployment but the parser refuses < 1 *)
> rank_e(d) = 1-indexed rank of d in engine e's top-k, or +inf if d not in engine e's top-k
> ```
>
> **`WeightedScore` strategy** remains shipped at MVP as an opt-in (`strategy=weighted` in DSL); it requires a documented per-engine score-normalization pass. Score-normalization shape is L1-normalize-within-engine-top-k. Operators electing `weighted` accept the score-drift risk.

### §4.5 Extended merge-determinism tuple

RFC §`Merge determinism rule` pins the lexical merge tuple:

```
merge order = (score DESC, repo_id ASC, manifest_generation ASC, candidate_id ASC)
```

For hybrid fusion, the extended tuple is:

```
fused merge order = (fused_score DESC, lex_score DESC NULL_LAST, sem_score DESC NULL_LAST, repo_id ASC, manifest_generation ASC, repo_relative_path ASC, start_line ASC, candidate_id ASC)
```

Each component is total within its domain. `NULL_LAST` on `lex_score` and `sem_score` handles the case where a candidate is in only one engine's top-k. The tuple is total → ties cannot exist → bit-exact reproducibility holds.

This extension is **additive** to RFC §`Execution Model` and must be filed back to the RFC (see §13).

### §4.6 Top-k semantics

**The fused top-k is NOT the union of each engine's top-k.** This must be explicitly documented in `SearchExplanation` and in [../usecase.md](../usecase.md). Concrete worked example (also lands in usecase.md §3 alongside UC-HYB-*):

```
Setup:
  lexical top-3 (k_lex=3):
    rank 1: doc A (lex_score = 12.5)
    rank 2: doc B (lex_score = 11.0)
    rank 3: doc C (lex_score = 9.8)
  semantic top-3 (k_sem=3):
    rank 1: doc D (sem_score = 0.92)
    rank 2: doc A (sem_score = 0.88)
    rank 3: doc E (sem_score = 0.85)

Fusion with RRF (weights = {lex: 0.5, sem: 0.5}, k_rrf_constant = 60):
  doc A: 0.5 * 1/(60+1) + 0.5 * 1/(60+2) = 0.00820 + 0.00806 = 0.01626  ← appears in BOTH
  doc B: 0.5 * 1/(60+2) + 0                                   = 0.00806
  doc D: 0                  + 0.5 * 1/(60+1) = 0.00820
  doc C: 0.5 * 1/(60+3) + 0                                   = 0.00794
  doc E: 0                  + 0.5 * 1/(60+3) = 0.00794
  fused top-5 (k = 5):
    rank 1: doc A (fused = 0.01626)
    rank 2: doc D (fused = 0.00820)
    rank 3: doc B (fused = 0.00806)
    rank 4: doc C (fused = 0.00794)  ← tie with E by fused_score, broken by candidate_id ASC
    rank 5: doc E (fused = 0.00794)
```

Doc A appears once with combined rank contribution. Doc D appears at rank 2 even though it was not in lexical top-3. This is the load-bearing top-k-of-fusion-≠-union-of-top-k property.

### §4.7 Pre-fetch top-k strategy

Because fusion can promote a candidate appearing in only one engine's top-k to the fused top-1, each sub-query must fetch a **larger than `k`** top-k. The pin:

- `k_lex_internal = k_sem_internal = max(k, 100)` at MVP.
- Above `k = 1000`, internal fetch caps at `k * 2` to bound the fusion-cost budget (§9).
- Operator override via per-deployment config; default sane.

This is necessary for fusion-correctness; without over-fetch, top-k-of-fusion underestimates documents that score well only in one engine.

### §4.8 Single-query opt-in vs implicit hybrid

**Locked**: hybrid is **explicit opt-in only**. A bare AST mixing lexical + semantic is `PARSE_UNSUPPORTED_COMBO` (per [SEM-01.md](SEM-01.md) §4.3). The DSL `hybrid(...)` directive is the **only** way to request hybrid. Rationale: implicit hybrid silently changes query semantics for callers that did not opt in, violating RFC §`Non-Negotiable Invariants` item 2 ("no fuzzy-by-default keyword semantics") interpreted to its full intent — no implicit semantic widening either.

This locks Q-IMPLICIT-1 raised in the ticket guidance.

## §5 Implementation steps (TDD)

### §5.1 Contract extension (red first)

1. Failing test: serde round-trip for `LqDirective::Hybrid`, `HybridWeights`, `HybridFusionStrategy` (hand-rolled per D18).
2. Failing test: each new `HYB_*` error code (instantiation + serde round-trip).
3. Failing test: `SearchExplanation::HybridContribution` serializes with both null and populated `lex_*` / `sem_*` fields.
4. Implement contract surface.

### §5.2 DSL grammar extension (red first)

1. Failing parser test for `hybrid(lex_q, sem_q, weights={lex=0.5, sem=0.5}, strategy=rrf)` golden string.
2. Failing parser test: omitted `weights` → default `{0.5, 0.5}`.
3. Failing parser test: omitted `strategy` → default `rrf`.
4. Failing parser test: `weights={lex=-0.1, sem=0.1}` → `HYB_INVALID_WEIGHTS` at parse time.
5. Failing parser test: `weights={lex=0.0, sem=0.0}` → `HYB_INVALID_WEIGHTS`.
6. Failing parser test: `hybrid(...)` containing a `SemanticVector` in the lex slot → `HYB_SUBQUERY_INVALID`.
7. Implement parser branch.

### §5.3 Planner pushdown invariant (red first)

1. Failing invariant test: for every `LqDirective::Hybrid` plan, the post-pushdown sem sub-plan carries the same lexical-universe filters as the lex sub-plan.
2. Failing test: a hybrid plan with `rev:abc` outer-scoped propagates `rev:abc` into the semantic sub-plan.
3. Failing test: a hybrid plan where lex `rev:abc` and sem `rev:def` (impossible at the parser level since `rev:` is outer; but property test exercises a planner-internal regression where pushdown drops the filter) → `HYB_PUSHDOWN_INCOMPLETE`.
4. Implement pushdown pass.

### §5.4 RRF strategy (red first)

1. Failing unit test: RRF on the §4.6 worked example produces the documented fused top-5 ordering.
2. Failing property test (proptest, 1k cases): for random `(lex_results, sem_results)` and random weights, RRF is **commutative** under engine-swap when weights are symmetric.
3. Failing property test: RRF result is independent of input ordering within each engine's top-k (since RRF reads ranks, not order — but defensive test guards against accidental sorting elsewhere).
4. Failing property test: RRF respects `top_k` bound (output size ≤ `k`).
5. Implement RRF.

### §5.5 WeightedScore strategy (red first)

1. Failing unit test: WeightedScore with L1-normalize-within-engine on synthetic data.
2. Failing unit test: WeightedScore with an empty engine result list (sub-query returned 0 hits) → fused output equals the other engine's weighted top-k.
3. Implement weighted strategy.

### §5.6 Generation-pin sharing (red first)

1. Failing test: a hybrid query where the lex generation pin differs from the sem generation pin (forced via test scaffold) → `HYB_GEN_MISMATCH`.
2. Failing test: a hybrid query honors the active generation set; reads pin once and share across sub-queries.
3. Implement shared-pin logic.

### §5.7 Cross-instance reproducibility (red first)

1. Failing CI test (two-process single-binary, per [../implementation-plan.md](../implementation-plan.md) §8.2): same hybrid query against same generation, two instances → byte-identical CBOR envelope (including `SearchExplanation.HybridContribution` values).
2. Failing property test (proptest): tuple-tiebreak invariant — for fused results with identical `fused_score`, tiebreak by `(lex_score, sem_score, repo_id, manifest_generation, repo_relative_path, start_line, candidate_id)` is **deterministic** across 1k random inputs.
3. Implement tuple application.

### §5.8 Pre-fetch over-fetch policy (red first)

1. Failing test: hybrid with `top_k = 50` fetches `100` from each sub-query (over-fetch min cap).
2. Failing test: hybrid with `top_k = 5000` fetches `min(5000 * 2, 10_000) = 10_000` (above-cap halt).
3. Failing test: hybrid with `top_k = 10_001` → `HYB_TOP_K_INVALID`.
4. Implement.

### §5.9 Explanation wire-shape (red first)

1. Failing integration test: a hybrid query that returns 5 candidates produces a `SearchExplanation` with 5 `HybridContribution` entries; each entry has both `lex_*` and `sem_*` populated when the candidate is in both top-ks; only one side populated when in one.
2. Failing test: `SearchExplanation.engines_routed` includes `["lexical", "semantic"]` for every hybrid query.
3. Implement explanation builder.

### §5.10 Observability (red first)

1. Failing test: OpenTelemetry span tree includes `lq.fusion` between `lq.merge` and the engine sub-spans for every hybrid query.
2. Failing test: metric `hybrid.fusion.duration_ms` is emitted.
3. Implement.

### §5.11 Negative-path cleanup

For each `HYB_*` code in §8, verify a 1:1 negative test exists and the underlying engine error message is preserved in the typed payload (no error swallowing — RFC §`Non-Negotiable Invariants` item 8).

## §6 Test plan

### §6.1 New conformance rows — `UC-HYB-*` (resolves `UC-GAP-1`, filed back to [../usecase.md](../usecase.md))

This is the representative set for the `UC-HYB-*` category that this ticket forces into the corpus. Per `UC-GAP-1`, these rows are absent today; this ticket's PR must merge them into `usecase.md` §2.

| ID | Title | Persona | Golden query (DSL form) | Expected | Engines | Parity |
| --- | --- | --- | --- | --- | --- | --- |
| `UC-HYB-01` | Hybrid bare (default weights, default strategy) | `P1` | `hybrid(Iterator, sem_vec(v))` | `multi`, fused order, default `{lex:0.5, sem:0.5}` RRF | L, semantic | `Q+` |
| `UC-HYB-02` | Hybrid with explicit RRF weights | `P3` | `hybrid(unsafe, sem_vec(v), weights={lex=0.7, sem=0.3}, strategy=rrf)` | `multi` | L, semantic | `Q+` |
| `UC-HYB-03` | Hybrid with WeightedScore | `P3` | `hybrid(panic!, sem_vec(v), strategy=weighted)` | `multi`, L1-normalized score blend | L, semantic | `Q+` |
| `UC-HYB-04` | Hybrid with shared `repo:` filter pushdown | `P3` | `repo:r1 hybrid(unwrap, sem_vec(v))` | `multi`; both sub-plans carry `repo:r1`; assertion via SearchExplanation | L, semantic | `Q+` |
| `UC-HYB-05` | Hybrid with shared `rev:` filter pushdown | `P2` | `rev:main hybrid(panic!, sem_vec(v))` | `multi`; both sub-plans pinned to `main` | L, semantic | `Q+` |
| `UC-HYB-06` | Hybrid with shared `lang:` filter pushdown | `P5` | `lang:rust hybrid(Iterator, sem_vec(v))` | `multi`; both sub-plans pinned to `rust` | L, semantic | `Q+` |
| `UC-HYB-07` | Hybrid top-k over-fetch | `P6` | `hybrid(Iterator, sem_vec(v), top_k=50)` | `multi`, exactly 50; internal over-fetch = 100 per engine | L, semantic | `Q+` |
| `UC-HYB-08` | Hybrid top-k = `count:all` ceiling | `P6` | `hybrid(...)` with `top_k=10_000` | `multi`, exactly 10000; internal over-fetch capped | L, semantic | `Q+` |
| `UC-HYB-09` | Hybrid worked example — top-k ≠ union | `P4` | per §4.6 worked example | exact ordering matches §4.6 | L, semantic | `Q+` |
| `UC-HYB-10` | Hybrid cross-instance determinism | `P6` | same `hybrid(...)` on two instances | byte-identical envelope | L, semantic | `Q+` |
| `UC-HYB-11` | Hybrid explanation wire-shape | `P4` | `hybrid(...)` with explain envelope requested | `multi` + `SearchExplanation` populated with `HybridContribution` per row | L, semantic | `Q+` |
| `UC-HYB-12` | Hybrid empty lex side | `P3` | lex sub-query returns 0 hits | `multi`, fused = sem-only weighted | L, semantic | `Q+` |
| `UC-HYB-13` | Hybrid empty sem side | `P3` | sem sub-query returns 0 hits | `multi`, fused = lex-only weighted | L, semantic | `Q+` |
| `UC-HYB-14` | Hybrid both empty | `P3` | both sub-queries 0 hits | `empty`, generation still bound | L, semantic | `Q+` |
| `UC-HYB-15` | Anti: weights sum to 0 | `P5` | `hybrid(..., weights={lex=0.0, sem=0.0})` | `error:HYB_INVALID_WEIGHTS` | parser | `Q+` |
| `UC-HYB-16` | Anti: negative weight | `P5` | `hybrid(..., weights={lex=-0.1, sem=0.5})` | `error:HYB_INVALID_WEIGHTS` | parser | `Q+` |
| `UC-HYB-17` | Anti: NaN weight | `P5` | `hybrid(..., weights={lex=NaN, sem=0.5})` | `error:HYB_INVALID_WEIGHTS` | parser | `Q+` |
| `UC-HYB-18` | Anti: mismatched generations between lex and sem | `P6` | hybrid where sub-plans resolve to different generations | `error:HYB_GEN_MISMATCH` | planner | `Q+` |
| `UC-HYB-19` | Anti: `SemanticVector` in lex slot | `P5` | `hybrid(sem_vec(u), sem_vec(v))` | `error:HYB_SUBQUERY_INVALID` | parser | `Q+` |
| `UC-HYB-20` | Anti: `top_k = 0` | `P5` | `hybrid(..., top_k=0)` | `error:HYB_TOP_K_INVALID` | parser | `Q+` |
| `UC-HYB-21` | Anti: unknown strategy | `P5` | `hybrid(..., strategy=learned)` | `error:HYB_STRATEGY_UNSUPPORTED` | parser | `Q+` |
| `UC-HYB-22` | Anti: implicit hybrid via bare AST mix | `P3` | `Iterator AND sem_vec(v)` (no `hybrid(...)`) | `error:PARSE_UNSUPPORTED_COMBO` | parser | `Q+` |

### §6.2 Rail matrix

| Rail | Coverage |
| --- | --- |
| Unit | per fusion strategy formula, per error code, per parser branch |
| Property | RRF commutativity (1k), tuple-tiebreak determinism (1k), weight-normalization round-trip |
| Integration | end-to-end UC-HYB-01..22 against real lexical + semantic adapters (no `StubLqEngine` past Wave-6) |
| Conformance | all 22 UC-HYB-* rows under `ci/lq-conformance` |
| Criterion | `hybrid_fusion_bench` measures `lex_budget + sem_budget + fusion_cost_cap` per §9 |
| Cross-instance | two-process reproducibility CI step for hybrid queries |

### §6.3 Coverage policy

Every claim has a 1:1 named test (path + assertion). Per [../implementation-plan.md](../implementation-plan.md) §8.3.

## §7 Observability

OpenTelemetry spans:

- `lq.fusion` — parent of both `lq.exec.lexical` and `lq.exec.semantic` sub-spans for a hybrid query; emitted once per hybrid request.
- `lq.fusion.strategy` — leaf attribute `{rrf | weighted}` carried on `lq.fusion`.
- `lq.fusion.weights` — leaf attributes `{lex_weight, sem_weight}`.
- `lq.fusion.over_fetch` — leaf attribute `{k_lex_internal, k_sem_internal, k_fused}`.

Metrics:

- `hybrid.fusion.duration_ms` (histogram).
- `hybrid.fusion.over_fetch_ratio` (histogram: `(k_lex_internal + k_sem_internal) / (2 * top_k)`).
- `hybrid.weights.lex` / `hybrid.weights.sem` (bucketed: `{0.0..0.1, 0.1..0.3, 0.3..0.7, 0.7..0.9, 0.9..1.0}`).
- `hybrid.strategy` (closed-set gauge: `{rrf, weighted}`).
- `hybrid.error.<HYB_*_code>` (counter, one per error code).
- `hybrid.empty_side` (counter, increments on UC-HYB-12 / -13 / -14 patterns).

Audit log: hybrid queries emit one row per request with `(tenant_id, user_id, canonical_query_hash, generation_set, strategy, lex_weight, sem_weight, top_k, latency_ms, result_count, error_code?)` — extended over the SEM-01 audit row by `(strategy, lex_weight, sem_weight)`.

Cardinality budget: weights are bucketed (closed set of 5); strategy is closed-set 2; top_k is bucketed per the same scheme as SEM-01. No unbounded label keys.

## §8 Error scenarios

| Scenario | Code | Retry semantics | Source |
| --- | --- | --- | --- |
| Weights sum to 0 | `HYB_INVALID_WEIGHTS{reason=ZERO_SUM}` | not retryable | parser at AST construction |
| Negative weight | `HYB_INVALID_WEIGHTS{reason=NEGATIVE}` | not retryable | parser |
| Non-finite weight (NaN / Inf) | `HYB_INVALID_WEIGHTS{reason=NON_FINITE}` | not retryable | parser |
| `top_k = 0` | `HYB_TOP_K_INVALID` | not retryable | parser |
| `top_k > 10_000` | `HYB_TOP_K_INVALID` | not retryable | parser (matches DSL §13 ceiling) |
| Sub-query shape invalid (sem in lex slot, etc.) | `HYB_SUBQUERY_INVALID` | not retryable | parser |
| Strategy name parses but unsupported | `HYB_STRATEGY_UNSUPPORTED` | not retryable | parser |
| Lex and sem sub-plans resolve to different generations | `HYB_GEN_MISMATCH` | not retryable until reconcile | planner |
| Lexical-universe filter fails to push down to sem sub-plan | `HYB_PUSHDOWN_INCOMPLETE` | not retryable; operator alarm | planner-internal invariant test |
| Lex sub-query yields underlying lex error (`PARSE_*`, `EXEC_SHARD_TIMEOUT`, ...) | underlying code preserved; envelope `{kind: HybridSubError, side: "lex", inner: <code>}` | follows inner code's semantics | sub-plan executor |
| Sem sub-query yields underlying sem error (`SEM_DIM_MISMATCH`, `SEM_NOT_READY`, ...) | underlying code preserved; envelope `{kind: HybridSubError, side: "sem", inner: <code>}` | follows inner code's semantics | sub-plan executor |
| Implicit hybrid attempt (bare AST mix without `hybrid(...)`) | `PARSE_UNSUPPORTED_COMBO` (from SEM-01 invariant) | not retryable | parser/planner |
| Mid-flight active-generation rotation | first read returns based on per-query pin (Phase-1 D11 / [SEM-01.md](SEM-01.md) §3.5); the second read sees the new gen | n/a | per-query pin |

No silent fallback. No degraded result. Cross-engine errors are typed and side-tagged (lex vs sem) so the audit trail can blame the right engine.

## §9 Performance envelope

### §9.1 Budget contract

```
hybrid_total_budget_ms = lex_sub_budget_ms + sem_sub_budget_ms + fusion_cost_cap_ms
fusion_cost_cap_ms    = max(10 ms, 0.1 * total_budget_ms)      (* p99 cap *)
```

The fusion stage is bounded by `fusion_cost_cap_ms`; exceeding triggers `EXEC_MERGE_CANCEL` (reuses the existing code per RFC §`Error Code Taxonomy`) for the fusion checkpoint. Sub-query budgets follow LEX-05 and SEM-01 SLOs.

### §9.2 SLO contributions

| Workload | p50 | p95 | p99 | Source |
| --- | --- | --- | --- | --- |
| Hybrid top-k = 50 (single-repo) | < 70 ms | < 350 ms | < 1.3 s | `lex p99 < 1s` + `sem p99 < 250 ms` + `fusion p99 < 50 ms` |
| Hybrid top-k = 100 (single-repo) | < 80 ms | < 400 ms | < 1.5 s | as above, larger fusion sort |
| Fusion-only cost (`top_k = 1000`, 2 × 2000 inputs) | < 5 ms | < 30 ms | < 100 ms | criterion `hybrid_fusion_bench` |

### §9.3 Bounded inputs

- `top_k` ≤ 10_000 (per DSL §13; see §3.1).
- Sub-query over-fetch ≤ 2 × `top_k`, capped at 10_000 (§4.7).
- Weights stored as `f32`, capped finite; no upper cap on magnitude (L1-normalize handles).
- `k_rrf_constant` configurable per deployment; floor `≥ 1`.

### §9.4 Criterion guard

`crates/quanta-index-fusion/benches/hybrid_fusion_bench.rs` (or `crates/quanta-index-core/benches/hybrid_fusion_bench.rs` if ADR-018 picks in-tree). Regression budget: p99 fusion cost may not increase >5% across a wave without an ADR.

## §10 Risks

| ID | Risk | Probability | Impact | Early-warning signal | Mitigation |
| --- | --- | --- | --- | --- | --- |
| R-STRATEGY | Operators discover RRF underperforms WeightedScore for their corpus, lobby for different default | M | M | Sourcegraph parity drift report flags ranking quality regression | ship both strategies; default is RRF; ADR-019 documents tradeoff; per-deployment override is operator's call |
| R-SCALE-MISMATCH | Mid-deployment BM25 parameter change or cosine-vs-L2 swap causes WeightedScore to misrank silently | M | H | hybrid result determinism test red on score-blend strategy | RRF is the default — scale-free; `weighted` is opt-in with documented risk |
| R-OVERFETCH | Internal over-fetch (§4.7) doubles sub-query work for every hybrid request | H | M | criterion bench p99 spike on `top_k > 1000` | hard cap at 10_000 per engine; ADR-021 candidate for tunable over-fetch ratio |
| R-DET-FUSION | Fusion stage introduces nondeterminism via floating-point sort instability | L | H | cross-instance reproducibility test red | tuple-tiebreak fully total (§4.5); sort is stable per Rust `sort_by` contract |
| R-EMPTY-SIDE | Both sub-queries return empty — fused result is empty but caller expected one of them to produce something | M | L | `hybrid.empty_side` metric; logs include `error_code?` even on `empty` result | explicit UC-HYB-14 conformance row; `empty` is a valid shape; no silent error |
| R-GEN-DRIFT | Lex and sem siblings of the same generation drift in build-time (sem still building when lex publishes) | M | H | `STATE_NOT_READY: STALE_SIBLING` at hybrid open | per RFC §`Atomicity contract`: manifest-first ensures both siblings have `MARKER_OK` before the manifest activates; hybrid fails fast with `HYB_GEN_MISMATCH` if forced |
| R-PUSHDOWN-LEAK | A new filter type lands later and is not registered as lexical-universe; hybrid silently filters lex side but not sem side | M | H | post-pushdown invariant test catches at PR time | invariant lint asserts every `LqFilter` variant has a registered pushdown rule (or an explicit `NotPushable` marker that fails closed) |
| R-CORPUS-MISS | `UC-HYB-*` rows authored here but never merged into usecase.md (file-back action missed) | M | H | conformance gate passes silently with no UC-HYB-* coverage | this ticket's DoD §11 explicitly requires usecase.md updated in the same PR — non-negotiable |
| R-RFC-FRAMING | RFC § Ticket Pack still names SEM-02 as "incremental semantic derivatives"; reassignment §2.3 must reach RFC before this ticket ships | H | H | RFC ticket-pack diff vs this spec | §13 file-back action against RFC; Wave-7 entry gate blocks on resolution |
| R-EXPLAIN-SIZE | `HybridContribution` per candidate inflates response envelope size on `top_k > 1000` | L | M | wire frame size metric | explanation envelope is opt-in (caller requests it); default response carries `LexicalCandidate.score = fused_score` only |
| R-WEIGHT-DRIFT | Production deployments fork weight choice; conformance gate green on each but cross-fleet inconsistent | M | M | audit log `(strategy, lex_weight, sem_weight)` per request | per-deployment config locked; ADR documents weight choice; periodic audit |

## §11 Definition of Done (provable)

Each item provable via the artifact listed; missing artifact = `blocked` per CLAUDE.md §`Verification`. All 22 rows shipped (76 tests in `quanta-index-lq-hybrid`). RRF is the default fusion strategy with `k=60`; Weighted is opt-in. The 8-component merge tuple is the deterministic ordering authority.

| # | Status | DoD | Provable via |
| --- | --- | --- | --- |
| 1 | ✓ shipped | `LqDirective::Hybrid` and related contract types land with hand-rolled serde | `cargo test -p quanta-index-contract --test hybrid_contract_roundtrip` |
| 2 | ✓ shipped | All 6 new `HYB_*` error codes emitted with documented payload | `cargo test -p quanta-index-contract --test hyb_error_codes` |
| 3 | ✓ shipped | DSL parser accepts `hybrid(lex, sem, weights={...}, strategy=...)` per §3.2 grammar | `cargo test -p quanta-index-core --test hybrid_parser` |
| 4 | ✓ shipped | RRF strategy (default `k=60`) produces the §4.6 worked example output exactly | `cargo test -p quanta-index-core --test rrf_worked_example` |
| 5 | ✓ shipped | WeightedScore strategy (opt-in) produces L1-normalized score blend | `cargo test -p quanta-index-core --test weighted_score_strategy` |
| 6 | ✓ shipped | All 22 `UC-HYB-*` corpus rows land in `usecase.md` §2 + 1:1 golden files in `tools/ci/conformance/lq/UC-HYB-*.toml` | `git ls-files docs/plans/may-24-lexical-indexing-sorucegraph/usecase.md` shows category J; `tools/ci/conformance/lq/UC-HYB-*.toml` exist |
| 7 | ✓ shipped | `UC-GAP-1` in [../implementation-plan.md](../implementation-plan.md) Appendix A.3 resolves (text updated to "Resolved by SEM-02") | git diff on implementation-plan.md |
| 8 | ✓ shipped | Lexical-universe filter pushdown is complete for every hybrid plan | `cargo test -p quanta-index-core --test hybrid_pushdown_invariant` |
| 9 | ✓ shipped | Cross-instance reproducibility CI step green for hybrid queries | `ci/lq-cross-instance-hybrid` CI rail |
| 10 | ✓ shipped | Extended 8-component merge-determinism tuple (§4.5) applied; tuple-tiebreak property test green at 1k cases | `cargo test -p quanta-index-core --test hybrid_tiebreak_determinism` |
| 11 | ✓ shipped | `SearchExplanation` v2 carries `HybridContribution` per row for every hybrid response | `cargo test -p quanta-index-contract --test hybrid_explanation` |
| 12 | ✓ shipped | Over-fetch policy (§4.7) honored; sub-queries fetch `max(top_k, 100)` capped at 10_000 | `cargo test -p quanta-index-core --test hybrid_over_fetch` |
| 13 | ✓ shipped | Fusion budget enforced per §9.1; exceed → `EXEC_MERGE_CANCEL` at fusion checkpoint | `cargo test -p quanta-index-core --test hybrid_budget_cancel` |
| 14 | ✓ shipped | Observability spans + metrics emit per §7 | `crates/quanta-index-searchd/tests/otel_hybrid.rs` |
| 15 | ✓ shipped | No `#[derive(Serialize/Deserialize)]` regressions land | semgrep `rust-no-serde-derive` green on PR |
| 16 | ✓ shipped | Criterion bench `hybrid_fusion_bench` p99 < 100 ms at `top_k = 1000` on 2 × 2000-doc fixture | `cargo bench` artifact |
| 17 | ✓ shipped | RFC § Ticket Pack updated: SEM-02 reframed as hybrid fusion (file-back action) | git diff on rfc.md |
| 18 | ✓ shipped | feature-scope.md §4.6 cross-cutting row for SEM-02 updated | git diff on feature-scope.md |
| 19 | ✓ shipped | implementation-plan.md §5.15 SEM-02 DoD rewritten | git diff on implementation-plan.md |
| 20 | ✓ shipped | ADR-019 (hybrid fusion strategy — RRF default `k=60`, Weighted opt-in) committed | `docs/adr/ADR-019-hybrid-fusion-strategy.md` exists |
| 21 | ✓ shipped | Structured agent output validates against `tools/ci/agent/agent_output.schema.json` | CI gate |
| 22 | ✓ shipped | RFC §`Claim Discipline` §7 second leg ("lexical-universe planning replaces post-filter correctness") provable | `crates/quanta-index-core/tests/lexical_universe_pushdown_proof.rs` exists; named test green |

## §12 Open questions

These block at least one design choice; each must be resolved before Wave-7 entry per [../implementation-plan.md](../implementation-plan.md) §10–§11.

- **Q-FRAMING-1.** RFC § Ticket Pack names SEM-02 as "incremental semantic derivatives"; this ticket reframes it as hybrid fusion. Resolution: (a) RFC amendment retags SEM-02 → SEM-02-HYB and adds a new SEM-DERIV ticket for the original scope, (b) RFC keeps SEM-02 as derivatives and this ticket re-IDs to SEM-03. **Default answer**: (a), per CLAUDE.md breaking-first posture — clean retag, no shim. Required file-back action listed in §13.
- **Q-WEIGHTS-1.** Per-tenant default weights? MVP says no — one default per deployment. Per-tenant would require an authz-bound weight registry. **Default answer**: defer; operator can pin per-deployment default; tenants requesting different weights must opt in per-request.
- **Q-STRATEGY-DEFAULT.** Default strategy = RRF or WeightedScore? **Default answer**: RRF (ADR-019). Open until ADR ratified.
- **Q-OVERFETCH-RATIO.** Over-fetch min cap = `max(top_k, 100)`; tunable per deployment? **Default answer**: yes; ADR-021 candidate; default cap 100, configurable floor 10 (parser refuses lower).
- **Q-IMPLICIT-1.** Can a single query express hybrid implicitly (any AST containing both lexical and semantic leaves) **OR** must the user opt in via `hybrid(...)`? **Locked: opt-in only** (§4.8). Rationale documented inline. This question is closed but listed here for traceability.
- **Q-RFC-EXEC-MODEL.** RFC §`Execution Model` § merge-determinism rule pins the lexical merge tuple. The hybrid extension §4.5 adds 4 more tuple components. Resolution: (a) RFC §`Execution Model` amendment lands the extended tuple, (b) hybrid documents its own tuple alongside. **Default answer**: (a); §13 lists the file-back action.
- **Q-WEIGHT-NORMALIZE.** L1-normalize-within-engine for WeightedScore, or L1-normalize-across-engines? **Default answer**: within-engine (each engine's top-k scores L1-normalize to sum to 1.0 before weighting). Within-engine is robust to engine-size differences.
- **Q-RRF-K-CONSTANT.** `k_rrf_constant = 60` (literature standard) — pin or expose? **Default answer**: pin at 60; ADR-019 covers; per-deployment configurable with floor `≥ 1`.
- **Q-EXPLAIN-DEFAULT.** Does every hybrid response carry `HybridContribution` by default, or opt-in via request flag? **Default answer**: opt-in via `LqRequest.explain = true`; default false to keep response envelope size bounded (§10 R-EXPLAIN-SIZE).
- **Q-ADR-LOC.** Fusion module location: `crates/quanta-index-core::domains::fusion` (in-tree) vs `crates/quanta-index-fusion/` (new crate)? **Default answer**: ADR-018 candidate; default in-tree at MVP; promote to its own crate if it grows past 2k LoC.

## §13 References

- [../rfc.md](../rfc.md) — RFC; specifically §`Ticket Pack` (SEM-02 — see Q-FRAMING-1), §`Execution Model` (merge-determinism tuple), §`Non-Negotiable Invariants`, §`Error Code Taxonomy`, §`Claim Discipline` §7 (forcing function).
- [../feature-scope.md](../feature-scope.md) — §4.6 (SEM-02 cross-cutting row), §5.4 (QI-extensions; `hybrid(...)` is QI-extension).
- [../usecase.md](../usecase.md) — §0 (result-shape vocabulary, error code table); §3 `UC-GAP-1` resolution lands here. **Forcing function for this ticket**: no UC-HYB-* rows in current corpus.
- [../dsl.md](../dsl.md) — §2 EBNF (extension surface for `hybrid(...)` directive), §9 (directive grammar — current `into:` / `scope:` / `with:` ; `hybrid(...)` is grammatically a directive call but per the EBNF extends the surface), §13 (limits — `top_k` shares `count:` ceiling).
- [../implementation-plan.md](../implementation-plan.md) — §5.15 (current SEM-02 DoD — to be rewritten per §13 file-back), §4.8 (Wave 7 goal), Appendix A.3 `UC-GAP-1` (forcing function). §6 R-* risk register.
- [SEM-01.md](SEM-01.md) — semantic vector adapter (consumed); §4.3 (AST-mix invariant forcing `hybrid(...)` opt-in), §4 (vector adapter shape).
- [../../search-plane-implementation-tickets.md](../../search-plane-implementation-tickets.md) — D10 (predecessor `0.5/0.5` default weights placeholder), D18 (no serde proc-macro derives).
- [../../../../CLAUDE.md](../../../../CLAUDE.md) — Agent change posture (breaking-first), Rule Catalog (safety / architecture / build hygiene / verification).
- [../../../../AGENTS.md](../../../../AGENTS.md) — shared agent router.
- [../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive` rule.
- [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — structured output contract.
- [../../../ssot/producer-handoff.md](../../../ssot/producer-handoff.md) — producer handoff SSOT.
- [INDEX.md](INDEX.md) — ticket index (downstream-migration follow-up tracked under §3.6).

### §13.1 File-back actions (must accompany this ticket's PR)

- [../usecase.md](../usecase.md) — add `UC-HYB-*` category (22 rows per §6.1) to §2; resolve `UC-GAP-1` in §3 (mark resolved by SEM-02); update §0 to declare the new `HYB_*` error codes from §4.1; add the §4.6 worked example as an inline conformance note.
- [../implementation-plan.md](../implementation-plan.md) — §5.15 SEM-02 DoD rewrite from "incremental semantic derivatives" → hybrid fusion; Appendix A.3 `UC-GAP-1` mark resolved.
- [../rfc.md](../rfc.md) §`Ticket Pack` — retag `SEM-02 incremental semantic derivatives` → `SEM-02 hybrid fusion`; add `SEM-DERIV` (or keep numbering tighter) for the original derivative scope (per Q-FRAMING-1 default answer).
- [../rfc.md](../rfc.md) §`Execution Model` § merge-determinism — append the §4.5 extended tuple (per Q-RFC-EXEC-MODEL default answer).
- [../feature-scope.md](../feature-scope.md) §4.6 — update SEM-02 cross-cutting row to reflect hybrid fusion.
- ADR-018 (new) — `docs/adr/ADR-018-hybrid-fusion-crate-location.md`: in-tree vs new crate decision.
- ADR-019 (new) — `docs/adr/ADR-019-hybrid-fusion-strategy.md`: RRF as default, WeightedScore as opt-in, full rationale from §4.4.
- ADR-021 (new, optional) — `docs/adr/ADR-021-hybrid-over-fetch-policy.md`: over-fetch ratio + floor.
