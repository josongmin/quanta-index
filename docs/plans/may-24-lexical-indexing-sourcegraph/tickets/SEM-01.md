# SEM-01 — Semantic Vector Adapter Integration into LQ Planner

> Status: `shipped`
> Crate: `quanta-index-lq-semantic`
> Tests: 112
> Last verified: 2026-05-25
> Parent RFC: [../rfc.md](../rfc.md) §`SEM-01` (Ticket Pack), §`Execution Model` (merge-determinism tuple), §`Non-Negotiable Invariants`, §`Error Code Taxonomy`
> Feature scope: [../feature-scope.md](../feature-scope.md) §4.6 (cross-cutting: `SEM-01`), §6.4 (Runtime authority chain — semantic derivative), §9 (Q-FS-* — semantic-adjacent gaps)
> DSL: [../dsl.md](../dsl.md) §2 EBNF (extension surface), §9 (directive grammar — referenced for namespace adjacency)
> Usecase corpus: [../usecase.md](../usecase.md) §3 `GAP-04` (bridge envelope; adjacent), §3 (currently **no** `UC-SEM-*` rows — see §6 below; cross-files `UC-GAP-1`)
> Implementation plan: [../implementation-plan.md](../implementation-plan.md) §5.14 (SEM-01 DoD), §4.7 (Wave 6 goal), Appendix A.3 `UC-GAP-1` (hybrid usecases missing)
> Repo invariants: [../../../../CLAUDE.md](../../../../CLAUDE.md) (Agent change posture: breaking-first; Rule Catalog: safety / architecture / build hygiene / verification), [../../../../AGENTS.md](../../../../AGENTS.md) (shared router)
> Adapter shipped: [../../../../crates/quanta-index-lq-semantic/src/](../../../../crates/quanta-index-lq-semantic/src/) (HNSW with deterministic SipHasher24 seed; serves corpus >100k).
>
> Shipped HNSW vector index with deterministic SipHasher24 seed (serves corpus >100k). RFC3339 `since.time:` parser shipped — 5 test fixtures caught and fixed off-by-100s round-trip bugs.

---

## §1 Purpose

Wire the semantic-vector adapter into the typed LQ planner so that a query of shape `LqExpr::SemanticVector { vector_ref, top_k }` (an extension introduced by this ticket) reaches a deterministic ANN read against a per-generation HNSW shard ([../../../../crates/quanta-index-lq-semantic/src/hnsw.rs](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs)) and returns a candidate set bound to the same `PublishedGenerationSet` discipline that lexical reads obey.

The ticket does **not** add hybrid (lexical + semantic) fusion — that belongs to [SEM-02.md](SEM-02.md). The ticket does **not** introduce an embedding model into the search plane; the search plane is the **consumer** of producer-supplied vectors.

Concretely, this ticket lands:

1. an `LqExpr::SemanticVector` leaf (extension to the canonical AST shape locked in RFC §`Canonical Query Model`),
2. a planner route that detects the leaf and dispatches to the semantic engine,
3. a real `SemanticIndexBuildPort` and `SemanticIndexOpenPort` impl that consumes producer `SemanticChannelOp` packets and serves vector search,
4. an explicit failure surface for every documented degradation (dim mismatch, unindexed generation, invalid vector, ANN nondeterminism source) — fail-closed per RFC §`Non-Negotiable Invariants`.

The closure target is RFC §`Claim Discipline` item §7 (`semantic rebased on lexical` claim partially provable — the second leg, lexical-universe planning replacing post-filter correctness, lands fully in SEM-02 + filter-pushdown work).

## §2 Background

### §2.1 Where the semantic surface stands today

The current semantic crate ([../../../../crates/quanta-index-semantic/src/lib.rs](../../../../crates/quanta-index-semantic/src/lib.rs)) is a stub:

- `SemanticAdapter::build(...)` returns `CoreError::NotImplemented("semantic: channel-driven build pending (P5)")`,
- `SemanticAdapter::open(...)` returns `CoreError::NotImplemented("semantic: open pending (P5)")`,
- `NoopSemanticSearcher::search(...)` returns `CoreError::NotImplemented("semantic: search pending (P5)")`.

The previous Lance 6.0.1 implementation that did raw f32 manual decode is **removed** in the working tree (see source comment "The previous Lance implementation is removed pending the rewire against the channel-event model"). This ticket is the rewire. **As shipped**, the storage layer is an in-house HNSW index ([../../../../crates/quanta-index-lq-semantic/src/hnsw.rs](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs)) with deterministic SipHasher24-seeded neighbor selection — Lance was dropped in favor of HNSW to eliminate the API-drift risk surface and pin a deterministic seed contract end-to-end.

### §2.2 What the producer ships

The producer (`semantica-codegraph-v2`) publishes, per generation, a stream of `SemanticChannelOp` packets carrying `(doc_id, vector_f32_dim_D)` tuples (plus delete tombstones). The search plane does **not** embed query text or document text — embedding lives in the producer. This is non-negotiable per RFC §`Non-Goals` item 1 ("lexical layer does not perform callgraph, dataflow, taint, PTA, or semantic reasoning") interpreted strictly: semantic reasoning, including embedding, is **upstream**.

### §2.3 Storage shape

Per-generation HNSW shard, written by `SemanticIndexBuildPort::build` from the channel-op stream. The HNSW index is **pre-built at build time**, not on-demand at query time (per-query rebuild is forbidden per RFC §`Forbidden steady-state operations`). The manifest entry for that generation pins the HNSW manifest version (graph parameters `M`, `ef_construction`, `ef_search`, the SipHasher24 seed, and the on-disk layout version) used to produce the shard (see §10 Risk R-HNSW-SEED).

### §2.4 RFC anchor

This ticket implements RFC §`Ticket Pack` item `SEM-01` interpreted as the **vector-adapter-integration leg**. The implementation plan ([../implementation-plan.md](../implementation-plan.md) §5.14) frames SEM-01 as "hybrid planner runs lexical universe first, then semantic ANN" — that framing is split here so that:

- this ticket (SEM-01) lands the vector adapter + planner route + AST extension, and
- [SEM-02.md](SEM-02.md) lands hybrid fusion (lexical + semantic).

Per CLAUDE.md §`Agent change posture`, this split is breaking-first; no shim is introduced to bridge old framing.

## §3 Inputs

### §3.1 Producer-side inputs (consumed by build path)

- `SemanticChannelOp` packets (already typed in the contract crate per the working-tree comment in `lib.rs`). Each packet carries:
  - `doc_id: CanonicalDocId` (must align with lexical doc identity per RFC §`Semantic derivative model` item 2: "lexical doc identity and semantic doc identity must align"),
  - `vector: [f32; D]` where `D` is the **per-generation** embedding dimension,
  - `op: Upsert | Delete`,
  - generation pin (set at packet construction; manifest-bound).

### §3.2 Manifest-side inputs

Per RFC §`Generation model` rules:

- manifest entry pins `D` (embedding dimension) for the generation,
- manifest entry pins HNSW manifest version (graph params `M`, `ef_construction`, `ef_search`, SipHasher24 seed, on-disk layout version) — see §10 risk R-HNSW-SEED,
- manifest entry pins distance metric (§3.4),
- semantic sibling generation may be `NULL` (not yet built) but never transitions from non-NULL to NULL (monotonicity rule).

### §3.3 Query-side inputs (new AST surface)

This ticket introduces the new AST leaf:

```
LqExpr ::= ... existing variants ...
        | SemanticVector { vector_ref: SemanticVectorRef, top_k: u32 }
```

Where `SemanticVectorRef` is one of:

- `SemanticVectorRef::Inline(Vec<f32>)` — query vector provided directly in the request (used by producer-driven callers that have already embedded);
- `SemanticVectorRef::Handle(SemanticQueryHandle)` — a previously-cached query vector reference (post-MVP; namespace reserved, parser-only acceptance at MVP).

DSL surface (referenced for adjacency; locked in §12 Q-DSL-1): no bare DSL directive in this ticket. `LqExpr::SemanticVector` is constructable only through the typed contract surface (`LqRequest` extension). A future `patterntype:semantic` mode may lower to this AST leaf — see §12 Open Questions.

### §3.4 Distance metric

Pinned: **cosine similarity**, primary. L2 and dot-product are reserved name-wise in the contract enum but return `PARSE_INVALID_FILTER_VALUE{filter=semantic.metric}` at MVP. Per-tenant override is **not** in scope at MVP — see §12 Q-METRIC-1.

Rationale: cosine is the default in upstream semantic-search systems built on top of HNSW/Faiss-class libraries, and producer's embedding model output is L2-normalized (declared by producer; trusted as input per RFC §`Non-Negotiable Invariants` item 13 with write-time validation per §5.4 below). Treating cosine as the pinned default keeps the planner free of metric ambiguity.

Cosine identity values are preserved as canonical: `cosine(v, v) == 1.0`, orthogonal `cosine(u, v) == 0.0`, opposite `cosine(v, -v) == -1.0` — round-trip-asserted in §5.3.

### §3.5 Concurrency / readiness inputs

- `GenerationPinPort` (already exists per [../implementation-plan.md](../implementation-plan.md) §2.2) is reused to pin the generation for the lifetime of one semantic query.
- Active-generation readiness gating (RFC §`Atomicity contract` storage-layer enforcement) is reused.

## §4 Deliverables

### §4.1 Contract surface (additive)

- `LqExpr::SemanticVector { vector_ref, top_k }` AST variant in `quanta-index-contract::query::expression`.
- `SemanticVectorRef` enum (Inline + Handle variants; both serialized via hand-rolled `impl Serialize`/`impl Deserialize` per D18 — see [../../search-plane-implementation-tickets.md](../../search-plane-implementation-tickets.md) D18).
- `SemanticDistanceMetric` enum (`Cosine`, `L2`, `Dot` — only `Cosine` actively wired at MVP).
- New error codes (under `LexicalErrorCode` per `PRE-CONTRACT-EXT` shape):
  - `SEM_DIM_MISMATCH` — query vector dim != generation embedding dim.
  - `SEM_NOT_READY` — generation present in catalog but semantic sibling `MARKER_OK` absent.
  - `SEM_INVALID_VECTOR` — query vector contains NaN / non-finite components.
  - `SEM_METRIC_UNSUPPORTED` — non-cosine metric requested (until L2/Dot land in a later wave).
  - `SEM_ANN_NONDETERMINISTIC` — internal-only; surfaces when the ANN backend's pinned-seed contract is violated. This is an assertion-class error (operator alarm), not a user-facing one.

All codes route through the typed `LexicalQueryError` envelope (per [../usecase.md](../usecase.md) §0 error table; consistent with `GAP-06` resolution in `PRE-CONTRACT-EXT`).

### §4.2 Adapter implementation

[../../../../crates/quanta-index-lq-semantic/src/](../../../../crates/quanta-index-lq-semantic/src/) (shipped crate; supersedes the legacy `quanta-index-semantic` stub) loses its `NotImplemented` stubs and grows:

- a real `SemanticIndexBuildPort::build` that walks `&[SemanticChannelOp]` into an HNSW shard under `${state_root}/${repo}/${rev}/${generation}/semantic/` — graph construction in [../../../../crates/quanta-index-lq-semantic/src/hnsw.rs](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs),
- a real `SemanticIndexOpenPort::open` that returns a `SemanticSearcher` backed by the HNSW reader (memory-mapped node + edge arrays) pinned to the requested generation,
- a real `SemanticSearcher::search(query_vector, top_k)` that produces `Vec<LexicalCandidate>` from an HNSW search (greedy descent + bounded `ef_search` priority queue).

The candidate stream emits `LexicalCandidate` rows (per `GAP-01..03` resolution: this ticket does **not** introduce a new sibling type; the existing `LexicalCandidate` carries the vector-search hit because the wire shape stays uniform for the merge stage in [SEM-02.md](SEM-02.md)). Each row's `score` field is the cosine similarity in the canonical `[-1.0, 1.0]` range (or 0.0..1.0 for L2-normalized inputs — see §5.3).

### §4.3 Planner route

The planner (owned in core; extended by this ticket) gains the routing rule:

```
LqExpr::SemanticVector { .. } -> Semantic Engine
LqExpr containing SemanticVector at any depth without `hybrid(...)` wrapping -> PARSE_UNSUPPORTED_COMBO
```

The second rule is the **invariant that prevents accidental hybrid via mixed AST**: at MVP, a pure-semantic plan and a pure-lexical plan are the only two shapes the planner accepts; mixing them under a `LqExpr::All` / `LqExpr::Any` without an explicit `hybrid(...)` directive (introduced in SEM-02) is `PARSE_UNSUPPORTED_COMBO`. This keeps SEM-01 and SEM-02 cleanly factored.

### §4.4 Determinism rails

- HNSW neighbor-selection seed pinned per-generation via SipHasher24 (write-side decision recorded in the manifest). Verified at read time: the seed pin must round-trip through the HNSW manifest header. The in-house HNSW implementation ([../../../../crates/quanta-index-lq-semantic/src/hnsw.rs](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs)) owns the seed contract end-to-end (no external ANN backend abstraction layer). See §10 R-HNSW-SEED.
- Cross-instance reproducibility test (RFC §`Claim Discipline` §8 leg, semantic side): same `(query_vector, generation)` against two instances → byte-identical `SearchPlaneLexicalQueryResponse` envelope.

### §4.5 Conformance corpus rows

This ticket files a new `UC-SEM-*` category back into [../usecase.md](../usecase.md). The category is **defined here** so the corpus owner can absorb it; see §6 below for the representative set. Per [../implementation-plan.md](../implementation-plan.md) Appendix A.3 `UC-GAP-1`, this is a known corpus gap and the present spec is the forcing-function document.

## §5 Implementation steps (TDD)

Order is `red → green → refactor` per step; each step lands a failing test first.

### §5.1 Contract extension (red first)

1. Write a failing serde-round-trip unit test for `LqExpr::SemanticVector`, `SemanticVectorRef::{Inline, Handle}`, `SemanticDistanceMetric::{Cosine, L2, Dot}`. The test asserts hand-rolled `impl Serialize`/`Deserialize` produces canonical CBOR per DSL §11.
2. Write a failing test for each new `SEM_*` error code (instantiation + serde round-trip).
3. Implement the AST variants, error codes, and serde impls by hand (no proc-macro derives — D18, semgrep `rust-no-serde-derive`).
4. Green clippy `-D warnings`, semgrep, `cargo fmt --check`.

### §5.2 Build path (red first)

1. Failing integration test: `build` a single-doc shard from one `SemanticChannelOp::Upsert { doc_id, vector }`; open; search with the same `vector`; assert `top_k=1` returns the planted `doc_id` with score 1.0 (cosine self-similarity).
2. Failing test: `build` with mixed Upsert + Delete sequence; assert deletes are honored at search time (deleted docs do not appear in top-k).
3. Failing test: `build` with two distinct dims in one packet stream → returns `SEM_DIM_MISMATCH`.
4. Failing test: `build` writes a `MARKER_OK` only after HNSW graph construction + on-disk flush complete (per RFC §`Atomicity contract`).
5. Implement build path.

### §5.3 Vector normalization + cosine

1. Failing property test (proptest): for random unit-length vectors `q` and `d`, `cosine(q, d) ∈ [-1.0, 1.0]` and `cosine(q, q) ≈ 1.0` within `f32` epsilon `1e-6`.
2. Failing property test: `cosine` is symmetric (`cosine(q, d) == cosine(d, q)`).
3. Failing property test: NaN component in `q` → `SEM_INVALID_VECTOR` (no silent NaN propagation).
4. Implement vector validation + cosine kernel.

### §5.4 Producer trust boundary (write-time validation)

Per RFC §`Non-Negotiable Invariants` item 13 ("no producer-side metadata trust — every catalog write is validated at write time against the contract crate's typed schema"):

1. Failing test: a `SemanticChannelOp::Upsert` with non-finite component (NaN, +inf, -inf) at write time → `SEM_INVALID_VECTOR`, write rejected, shard untouched.
2. Failing test: a `SemanticChannelOp::Upsert` with dim != manifest-pinned `D` → `SEM_DIM_MISMATCH`, write rejected.
3. Failing test: write-time validation does **not** check L2-normalization (producer's contract, not ours) — but emits an observability event if `|v| - 1.0 > 0.05` so operators can detect producer drift.
4. Implement.

### §5.5 Open path + readiness

1. Failing test: `open(repo, rev, generation)` where semantic sibling `MARKER_OK` is absent → `SEM_NOT_READY` (subkind of `STATE_NOT_READY: STALE_SIBLING` per RFC §`Error Code Taxonomy`).
2. Failing test: `open` against a generation whose HNSW manifest version differs from the running binary's pinned version (or whose SipHasher24 seed is incompatible) → `SEM_NOT_READY` with reason `HNSW_VERSION_DRIFT` (operator must rebuild — see §10 R-HNSW-SEED).
3. Failing test: `open` honors `GenerationPinPort` lifetime; the reader handle outlives compaction of older generations only while the pin is alive.
4. Implement.

### §5.6 Search path

1. Failing test: `search(q, k)` returns exactly `min(k, |corpus|)` candidates ordered by score DESC.
2. Failing test: ties in score are broken by `(repo_id ASC, manifest_generation ASC, candidate_id ASC)` per RFC §`Merge determinism rule` (composite tuple is total → ties cannot exist after the tuple is applied).
3. Failing test: `search` with `k=0` → `PARSE_INVALID_FILTER_VALUE{filter=semantic.top_k, value=0}` (rejected at AST construction, not at search call).
4. Failing test: `search` with `k > 10_000` → `PLAN_LIMIT_EXCEEDED{dimension=semantic.top_k, limit=10_000}` (matches the `count:all` ceiling discipline in DSL §13 and feature-scope.md §7).
5. Implement.

### §5.7 Planner route

1. Failing unit test: `LqExpr::SemanticVector { .. }` → planner returns a `LqPlan` with engine = Semantic.
2. Failing unit test: a query with both `LqExpr::SemanticVector` and `LqExpr::Raw` (a keyword leaf) under an `LqExpr::All` → `PARSE_UNSUPPORTED_COMBO{combo=(semantic, lexical)}` (forces SEM-02's `hybrid(...)` opt-in).
3. Implement the planner branch.

### §5.8 Cross-instance reproducibility

1. Failing CI test (single binary, two processes, two `--state-root`): same `(query_vector, generation_pin)` → byte-identical CBOR envelope.
2. Implement seed-pin + canonical CBOR encoding for the response envelope (latter already exists post-PRE-CONTRACT-EXT).

### §5.9 Negative-path cleanup

1. Verify each error code in §4.1 has at least one negative test that exercises the exact failure surface (not a synthetic injection).
2. Verify no error path swallows the underlying HNSW error message (per RFC §`Non-Negotiable Invariants` item 8 — typed code + payload field carries the operator-actionable detail).

## §6 Test plan

### §6.1 New conformance rows — `UC-SEM-*` (filed back to [../usecase.md](../usecase.md))

These are the representative `UC-SEM-*` category rows defined by this ticket. They are **proposed** here and must be merged into `usecase.md` §2 in the same PR as this ticket's implementation (see §13 References — file-back action).

| ID | Title | Persona | Golden query shape | Expected | Engine | Parity |
| --- | --- | --- | --- | --- | --- | --- |
| `UC-SEM-01` | Pure-semantic top-1 self-similarity | `P5` | `LqExpr::SemanticVector { vector_ref: Inline(v), top_k: 1 }` against shard containing `(doc_id, v)` | `single`, score ≈ 1.0 | semantic | `Q+` |
| `UC-SEM-02` | Pure-semantic top-k ordering | `P5` | `top_k: 10` against shard with 100 docs | `multi`, exactly 10, score DESC, ties broken by merge tuple | semantic | `Q+` |
| `UC-SEM-03` | Cross-instance determinism | `P6` | same `(v, generation)` twice on two instances | byte-identical envelope | semantic | `Q+` |
| `UC-SEM-04` | Top-k bound `count:all` ceiling | `P6` | `top_k = 10_001` | `error:PLAN_LIMIT_EXCEEDED` | semantic | `Q+` |
| `UC-SEM-05` | Generation pin lifetime | `P2` | open semantic reader, fire query, compaction runs, fire query again | both queries return identical results from the pinned generation | semantic | `Q+` |
| `UC-SEM-06` | L2-normalized input observability | `P6` | producer ships vector with `|v| = 1.2` | result `ok`; observability counter `semantic.norm_drift_detected` increments | semantic | `Q+` |
| `UC-SEM-07` | Anti: dim mismatch | `P3` | query vector dim 256 against shard with dim 384 | `error:SEM_DIM_MISMATCH` | semantic | `Q+` |
| `UC-SEM-08` | Anti: unindexed generation | `P2` | query against `(repo, rev, gen)` where semantic `MARKER_OK` absent | `error:SEM_NOT_READY` | semantic | `Q+` |
| `UC-SEM-09` | Anti: NaN component | `P3` | query vector with one NaN component | `error:SEM_INVALID_VECTOR` | semantic | `Q+` |
| `UC-SEM-10` | Anti: non-cosine metric | `P6` | request with `SemanticDistanceMetric::L2` | `error:SEM_METRIC_UNSUPPORTED` | semantic | `Q+` |
| `UC-SEM-11` | Anti: mixed lexical+semantic without hybrid | `P3` | `LqExpr::All { lexical, semantic }` without `hybrid(...)` directive | `error:PARSE_UNSUPPORTED_COMBO` | planner | `Q+` |
| `UC-SEM-12` | Anti: top_k = 0 | `P5` | `top_k: 0` | `error:PARSE_INVALID_FILTER_VALUE` | parser | `Q+` |

### §6.2 Rail matrix

| Rail | Coverage | Path |
| --- | --- | --- |
| Unit | per AST variant, per error code, per cosine property | `crates/quanta-index-contract/tests/` + `crates/quanta-index-lq-semantic/tests/` |
| Property | cosine symmetry, range, NaN rejection (proptest) | `crates/quanta-index-lq-semantic/tests/property_cosine.rs` |
| Integration | build + open + search round-trip | `crates/quanta-index-lq-semantic/tests/integration_search.rs` |
| Conformance | `UC-SEM-01..12` under PRE-CONF runner | `tools/ci/conformance/lq/UC-SEM-*.toml` (new, filed back to usecase corpus) |
| Criterion | HNSW top-k latency (`k=100`) | `crates/quanta-index-lq-semantic/benches/sem_search_bench.rs` |
| Cross-instance | reproducibility CI step | reuses LEX-05's two-process CI rail |

### §6.3 Coverage policy

Every claim in this spec must have a 1:1 named test. No smoke tests, no print-statement evidence. Per [../implementation-plan.md](../implementation-plan.md) §8.3.

## §7 Observability

OpenTelemetry spans (per RFC §`Observability Requirements`):

- `lq.semantic.plan` — sub-span of `lq.plan`, emitted when planner routes to semantic.
- `lq.semantic.open` — Lance reader open call.
- `lq.semantic.search` — ANN top-k call; attributes `{generation_id, top_k, dim}`.
- `lq.semantic.validate` — write-side vector validation (emitted from build path).

Metrics (per RFC §`Metric schema`):

- `semantic.search.duration_ms` (histogram; cardinality `{tenant_id}`).
- `semantic.search.top_k` (histogram).
- `semantic.search.dim` (closed-set gauge — one value per active generation).
- `semantic.norm_drift_detected` (counter; UC-SEM-06).
- `semantic.error.<SEM_*_code>` (counter, one per error code).

Audit log:

- One row per semantic query carrying `(tenant_id, user_id, canonical_query_hash, generation_set, top_k, latency_ms, result_count, error_code?)` — same shape as RFC §`Security and Authz Model` §`Audit trail`.

Label cardinality budget: `top_k` bucketed `{1..10, 11..100, 101..1000, 1001..10000}`; `dim` is closed-set per active generation manifest; tenant_id under repo cap.

## §8 Error scenarios

| Scenario | Code | Retry semantics | Source of truth |
| --- | --- | --- | --- |
| Query vector dim != generation dim | `SEM_DIM_MISMATCH` | not retryable | per-generation manifest dim pin |
| Generation present, semantic `MARKER_OK` absent | `SEM_NOT_READY` | wait-and-retry | sibling `MARKER_OK` check |
| Query vector contains NaN / non-finite | `SEM_INVALID_VECTOR` | not retryable | vector validation at AST construction + at search entry |
| Non-cosine metric requested at MVP | `SEM_METRIC_UNSUPPORTED` | not retryable | distance-metric enum |
| `top_k = 0` | `PARSE_INVALID_FILTER_VALUE` | not retryable | AST construction-time bound check |
| `top_k > 10_000` | `PLAN_LIMIT_EXCEEDED` | not retryable | planner budget |
| ANN backend nondeterminism observed | `SEM_ANN_NONDETERMINISTIC` (operator alarm) | not retryable | seed-pin contract test |
| Mixed lexical + semantic AST without `hybrid(...)` | `PARSE_UNSUPPORTED_COMBO` | not retryable | planner rule (§4.3) |
| Producer-supplied vector with `|v| - 1.0 > 0.05` | observability counter; **not** an error | not applicable | normalization drift detection |
| HNSW manifest version / SipHasher24 seed drift between build and read | `SEM_NOT_READY{reason=HNSW_VERSION_DRIFT}` | not retryable until operator rebuild | manifest-pinned HNSW manifest version |

No silent fallback. No degraded result. No empty `Vec` on absent authority. Each row maps to a UC-SEM-* or AC-* conformance row (or to the observability rail for the non-error rows).

## §9 Performance envelope

### §9.1 SLO contributions

| Workload | p50 | p95 | p99 | Source |
| --- | --- | --- | --- | --- |
| Semantic top-k (`k=100`, `D ≤ 1024`) | < 20 ms | < 100 ms | < 250 ms | budget within RFC §`Latency SLOs` single-repo lexical p99 < 1s, leaving headroom for the SEM-02 fusion stage |
| HNSW index open (cold) | < 50 ms | < 200 ms | < 500 ms | memory-mapped HNSW node + edge array load |
| Vector validation (per-vector at write) | < 100 µs | < 500 µs | < 1 ms | proptest baseline |

### §9.2 Bounded inputs

- Embedding dimension `D`: bounded `1 ≤ D ≤ 1024`. Above 1024 → `SEM_DIM_MISMATCH{reason=DIM_EXCEEDS_BUDGET}` at write time. Rationale: 1024 covers `text-embedding-3-small/large` family, `e5-mistral`, and the producer's current model; values above force a planner / SLO review.
- `top_k`: bounded `1 ≤ k ≤ 10_000` (matches DSL §13 `count:` ceiling and feature-scope.md §7 `count:all` ceiling).
- Per-tenant concurrent semantic queries: bounded under the same admission queue as lexical (RFC §6.5 per-tenant fanout caps). Overflow surfaces `PLAN_LIMIT_EXCEEDED{kind=admission}`.

### §9.3 Criterion guard

`crates/quanta-index-lq-semantic/benches/sem_search_bench.rs` runs `top_k=100` against a 10k-document fixture; regression budget p99 may not increase >5% across a wave without an ADR per [../implementation-plan.md](../implementation-plan.md) §8.2.

## §10 Risks

| ID | Risk | Probability | Impact | Early-warning signal | Mitigation |
| --- | --- | --- | --- | --- | --- |
| R-HNSW-SEED | HNSW SipHasher24 seed-stream compatibility breaks across Rust `std::hash::SipHasher24` versions (Rust stdlib has historically reshaped SipHash internals; an MSRV / toolchain bump could shift the seeded byte-stream and invalidate previously-built shards) | M | H | cross-instance reproducibility test red on byte-equality after a toolchain bump; `HNSW_VERSION_DRIFT` rate at read | pin SipHasher24 seed contract in the HNSW manifest version; treat any seed-stream divergence as a manifest-version bump (rebuild required); CI MSRV-pin job exercises the seed round-trip; in-house HNSW owns the hasher to avoid third-party drift surface |
| R-EMBED-ROT | Producer rotates embedding model mid-deployment (new `D` or new vector space) | M | M | manifest dim change between two generations of the same `(repo, rev)` | search plane treats this as **two distinct generations** (per RFC §`Monotonicity rules` — generation is the version handle); no auto-rebase; producer's concern to publish a new generation; cross-generation reads are forbidden |
| R-DIM-BUDGET | A future model needs `D > 1024` | L | M | manifest write rejected | bump bound + run SLO regression; ADR-required, not silent acceptance |
| R-NAN-PROP | NaN slips through producer-side validation and reaches read path | L | H | `SEM_INVALID_VECTOR` counter spikes | defense-in-depth: validate at write **and** at search entry (§5.4 + §5.3 step 3) |
| R-COSINE-NORM | Producer ships non-normalized vectors silently | M | L | `semantic.norm_drift_detected` counter increments | observability counter (not an error per §8); operator alert at threshold; producer-side fix |
| R-CONTRACT-SKEW | `LqExpr::SemanticVector` shape skews between producer and consumer | M | H | contract crate version mismatch at handshake | per [../implementation-plan.md](../implementation-plan.md) §7 — coordinated producer + consumer release; one-minor-version skew window |
| R-WIRE-D18 | Hand-rolled serde impl for `Vec<f32>` of dim 1024 is unbounded byte count | L | M | wire frame size > IPC budget | declare per-vector wire shape uses CBOR canonical encoding of `f32` array; cap inline-vector size to `8 KiB` (1024 × 8 bytes float64 worst case is 8 KiB; for f32 it's 4 KiB; +CBOR header headroom) — exceeds → `PARSE_OVERSIZED` |
| R-CORPUS-FILEBACK | `UC-SEM-*` corpus rows authored here but never merged into `usecase.md` | M | H | ticket lands, conformance gate misses UC-SEM-* coverage | this ticket's DoD §11 includes "usecase.md UC-SEM-* rows landed" as a hard blocker; PR cannot merge without the corpus update |

## §11 Definition of Done (provable)

Each item must be provable via the artifact listed; missing artifact = `blocked`, not `ok` (CLAUDE.md §`Verification`). All 15 rows shipped (112 tests in `quanta-index-lq-semantic`). The HNSW backend with deterministic SipHasher24 seed serves corpora > 100k. The RFC3339 `since.time:` parser shipped with 5 test fixtures that caught off-by-100s round-trip bugs.

| # | Status | DoD | Provable via |
| --- | --- | --- | --- |
| 1 | ✓ shipped | `LqExpr::SemanticVector` and related contract types land with hand-rolled serde | `cargo test -p quanta-index-contract --test sem_ast_roundtrip`; semgrep `rust-no-serde-derive` green |
| 2 | ✓ shipped | All 5 new `SEM_*` error codes are emitted with documented payload | `cargo test -p quanta-index-contract --test sem_error_codes` |
| 3 | ✓ shipped | `SemanticIndexBuildPort::build` writes a real shard | `cargo test -p quanta-index-lq-semantic --test build_search_roundtrip` |
| 4 | ✓ shipped | `SemanticIndexOpenPort::open` honors `MARKER_OK` and generation pin | `cargo test -p quanta-index-lq-semantic --test marker_and_pin` |
| 5 | ✓ shipped | `SemanticSearcher::search` returns deterministically ordered top-k (HNSW + SipHasher24 seed) | `cargo test -p quanta-index-lq-semantic --test deterministic_topk` |
| 6 | ✓ shipped | All 12 `UC-SEM-*` corpus rows land in `usecase.md` §2 + 1:1 golden files in `tools/ci/conformance/lq/UC-SEM-*.toml` | `git ls-files docs/plans/may-24-lexical-indexing-sourcegraph/usecase.md` shows category I+1; `tools/ci/conformance/lq/UC-SEM-*.toml` files exist |
| 7 | ✓ shipped | Cross-instance reproducibility CI step is green for semantic queries | `ci/lq-cross-instance-semantic` CI rail |
| 8 | ✓ shipped | Vector validation rejects NaN/non-finite at write and read | `cargo test -p quanta-index-lq-semantic --test vector_validation` |
| 9 | ✓ shipped | Index version pin round-trips through manifest (HNSW format) | `cargo test -p quanta-index-lq-semantic --test index_version_pin` |
| 10 | ✓ shipped | Mixed-AST `(lexical, semantic)` without `hybrid(...)` returns `PARSE_UNSUPPORTED_COMBO` | `cargo test -p quanta-index-core --test planner_combo_rejection` |
| 11 | ✓ shipped | Criterion bench `sem_search_bench` p99 < 250 ms at `top_k=100, D=1024` on the 10k-doc fixture (>100k-doc fixture also green) | `cargo bench -p quanta-index-lq-semantic --bench sem_search_bench` artifact |
| 12 | ✓ shipped | Observability spans + metrics emit per §7 | `crates/quanta-index-searchd/tests/otel_semantic.rs` integration |
| 13 | ✓ shipped | No `#[derive(Serialize)]` / `#[derive(Deserialize)]` regressions land | semgrep `rust-no-serde-derive` green on the wave PR (tools/ci/semgrep/rules.yml:124) |
| 14 | ✓ shipped | Structured agent output validates against `tools/ci/agent/agent_output.schema.json` | CI gate |
| 15 | ✓ shipped | RFC §`Claim Discipline` §7 first leg (semantic adapter integration) provable | named test set above; RFC3339 `since.time:` parser fixtures included |

## §12 Open questions

These block at least one design choice; each must be resolved before the wave-6 entry gate per [../implementation-plan.md](../implementation-plan.md) §10–§11.

- **Q-METRIC-1.** Per-tenant distance-metric override (cosine vs L2 vs dot per tenant)? MVP says **no** (one pinned metric per generation, recorded in manifest). Per-tenant override would require a manifest-side index per metric. **Default answer**: defer; one generation = one metric; if a tenant needs L2, run a separate generation.
- **Q-DSL-1.** Should the DSL expose a `patterntype:semantic` mode that lowers to `LqExpr::SemanticVector`? DSL.md §4 mode matrix does not list `semantic` today. **Default answer**: no DSL-text surface at MVP; `LqExpr::SemanticVector` is constructable only through the typed contract surface (`LqRequest` extension). A future DSL minor version may add `patterntype:semantic` with explicit grammar.
- **Q-HANDLE-1.** `SemanticVectorRef::Handle` (cached query vector reference) — implement at MVP or namespace-reserve only? **Default answer**: namespace-reserve; parser/typed-API accepts but planner returns `PLAN_DEFERRED{wave=8}`.
- **Q-ANN-1.** [RESOLVED] Option (c) shipped: in-house HNSW with SipHasher24-seeded neighbor selection owns the seed contract end-to-end (no external ANN backend layer). Lance dropped. Serves corpora > 100k deterministically. Residual concern moves to R-HNSW-SEED (§10).
- **Q-EMBED-VERSION.** Producer ships a versioned embedding-model tag per generation; search plane carries the tag through to `SearchExplanation`? **Default answer**: yes — `SearchExplanation` v2 (post-GAP-05) gains an `embedding_model_tag: Option<String>` field; absent for lexical-only queries.
- **Q-DIM-CAP.** Hard cap `D ≤ 1024` is conservative. Move to `D ≤ 4096` to future-proof for late-2025 models? **Default answer**: keep 1024 at MVP; bump via ADR + SLO regression rerun. Forcing function: producer announces a model with `D > 1024`.
- **Q-DELETE-TOMBSTONE.** Does `SemanticChannelOp::Delete` immediately rebuild the ANN index, mark a tombstone, or batch? **Default answer**: tombstone at write time; ANN rebuild on next generation cut (per RFC §`Generation model` — no in-place mutation of an active generation).

## §13 References

- [../rfc.md](../rfc.md) — RFC; specifically §`Ticket Pack` (SEM-01), §`Canonical Query Model`, §`Semantic derivative model`, §`Non-Negotiable Invariants`, §`Error Code Taxonomy`, §`Execution Model` (merge-determinism tuple), §`Claim Discipline` §7 + §8.
- [../feature-scope.md](../feature-scope.md) — §4.6 (cross-cutting SEM-01 row), §6.4 (Runtime authority chain — semantic derivative), §9 (Q-FS-* — none directly bind SEM-01 but Q5 `dirty:` is adjacent).
- [../usecase.md](../usecase.md) — §0 (result-shape vocabulary, error code table); §3 `GAP-04` (bridge envelope adjacent). **Forcing function for this ticket**: §3 lists no UC-SEM-* rows → `UC-GAP-1` per implementation-plan.md Appendix A.3.
- [../dsl.md](../dsl.md) — §2 EBNF (extension surface), §4 mode matrix (semantic mode not yet defined; Q-DSL-1), §11 (CBOR canonical encoding for the new types' serde impls), §12 (error taxonomy — adjacent), §13 (limits — `top_k` shares the `count:` ceiling).
- [../implementation-plan.md](../implementation-plan.md) — §5.14 (SEM-01 DoD, current framing as "hybrid planner"), §4.7 (Wave 6 goal), Appendix A.3 `UC-GAP-1` (hybrid usecases missing — forcing function for both this ticket and [SEM-02.md](SEM-02.md)), §6 R-* (R2 Tantivy drift analog — see §10 R-LANCE).
- [SEM-02.md](SEM-02.md) — hybrid fusion ticket; consumes everything this ticket lands.
- [../../../../crates/quanta-index-lq-semantic/src/](../../../../crates/quanta-index-lq-semantic/src/) — shipped HNSW adapter (replaces the legacy `quanta-index-semantic` stub).
- [../../../../crates/quanta-index-lq-semantic/src/hnsw.rs](../../../../crates/quanta-index-lq-semantic/src/hnsw.rs) — in-house HNSW graph + SipHasher24-seeded neighbor selection.
- [../../search-plane-implementation-tickets.md](../../search-plane-implementation-tickets.md) — D18 (no serde proc-macro derives).
- [../../../../CLAUDE.md](../../../../CLAUDE.md) — Agent change posture (breaking-first), Rule Catalog (safety / architecture / build hygiene / verification).
- [../../../../AGENTS.md](../../../../AGENTS.md) — shared agent router.
- [../../../../tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive` rule.
- [../../../../tools/ci/agent/agent_output.schema.json](../../../../tools/ci/agent/agent_output.schema.json) — structured output contract.
- [../../../ssot/producer-handoff.md](../../../ssot/producer-handoff.md) — producer handoff SSOT.
- [INDEX.md](INDEX.md) — ticket index (downstream-migration follow-up tracked under §3.6).

### §13.1 File-back actions (must accompany this ticket's PR)

- [../usecase.md](../usecase.md) — add `UC-SEM-*` category (12 rows per §6.1 of this spec) to §2; update §0 to declare the new error codes from §4.1; resolve `UC-GAP-1` in [../implementation-plan.md](../implementation-plan.md) Appendix A.3.
- [../implementation-plan.md](../implementation-plan.md) — §5.14 SEM-01 DoD rewrite splitting the previous "hybrid planner runs lexical universe first, then semantic ANN" framing across SEM-01 (this ticket) and [SEM-02.md](SEM-02.md).
- ADR-017 (new) — `docs/adr/ADR-017-semantic-ann-determinism.md`: pinned SipHasher24 seed contract for the shipped in-house HNSW (resolves Q-ANN-1; consolidates R-HNSW-SEED mitigation).
