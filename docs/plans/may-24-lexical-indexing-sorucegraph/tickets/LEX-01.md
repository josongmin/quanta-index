# LEX-01: IDF / scoring foundation

| field | value |
|---|---|
| Status | shipped |
| Crate | `quanta-index-lq-scorer` |
| Tests | 43 |
| Last verified | 2026-05-25 |
| Wave | 1 |
| Owner crate(s) | `quanta-index-lq-scorer` (G-CONTROL-LOC resolved as standalone crate); consumes `quanta-index-core` ports + `quanta-index-contract` carriers |
| Touches contract | yes — adds the per-generation `LexicalScorerManifest` carrier (additive); ties to PRE-CONTRACT-EXT GAP-05 (`SearchExplanation` v2 score envelope), [usecase.md §3](../usecase.md) |
| Size | XL — per-field IDF + BM25 parameter freeze + score normalization + cross-instance reproducibility + golden score corpus |
| Depends on | LEX-00 (canonical token stream + `AnalyzerId`), PRE-CONTRACT-EXT (typed score envelope), PRE-NORM (canonical AST), PRE-CONF (corpus runner) |
| Blocks | LEX-03 (sibling shards each need their own IDF tables), LEX-05 (deterministic merge ties scores), LEX-06 (rerank layers on top of the normalized envelope), OBS-01 (cross-instance reproducibility CI step) |

> BM25 per-field tuning (final `(k1, b)` table) and multi-process cross-instance reproducibility deferred to integration. See §12.

---

## 1. Purpose

Replace any default Tantivy BM25 with a **controlled, generation-pinned,
per-field scorer** that:

1. computes per-field IDF tables at index-build time and **persists them
   with the generation**, so a query against a pinned generation reads
   the exact IDF table the writer wrote;
2. pins BM25 parameters (`k1`, `b`) per field, frozen for the lifetime of
   the generation;
3. normalizes the raw BM25 sum into a deterministic envelope
   `score ∈ [0.0, 1.0]` (`f32`, finite, non-NaN) so LEX-06 rerank can
   layer on top without re-anchoring the scale;
4. produces a `SearchExplanation` score breakdown that names the
   contributing fields, IDF contributions, and BM25 parameter set —
   required by [usecase.md UC-OPS-06](../usecase.md) (GAP-05 resolution).

Required because:

- without per-generation IDF persistence, the same `(query, repo, rev,
  generation)` against two instances returns different scores
  (corpus advances between IDF reads), violating RFC § Claim Discipline §8
  (cross-instance reproducibility);
- without a frozen BM25 parameter set, a Tantivy version bump silently
  changes scores, violating RFC § Non-Negotiable Invariants 4 ("no
  hidden filter/ranking semantics in helper string functions");
- without a normalized envelope, every consumer of `score` (LEX-05 merge
  tuple, LEX-06 rerank, hybrid SEM-01 pushdown) has to renormalize, and
  the renormalizations drift.

LEX-01 is **not** a ranker — it is the deterministic-scorer foundation
the ranker sits on. LEX-06 owns adjacency-link proximity boost and the
explain payload; LEX-01 owns the scalar score and its provenance.

---

## 2. Background

### 2.1 Source contracts

| Source | What it pins |
|---|---|
| [rfc.md § Execution Model § Merge determinism rule](../rfc.md) | merge tuple `(score DESC, repo_id ASC, manifest_generation ASC, candidate_id ASC)` — every component must be total within its domain |
| [rfc.md § Claim Discipline §8](../rfc.md) | cross-instance reproducibility: same `(query, repo, rev, generation)` on two instances → byte-identical envelope |
| [rfc.md § Claim Discipline §10](../rfc.md) | ranking correctness requires a golden IR-evaluation set; precision@10, MAP, NDCG measured |
| [rfc.md § Non-Negotiable Invariants 4](../rfc.md) | no hidden filter/ranking semantics |
| [rfc.md § Monotonicity rules](../rfc.md) | sibling generations non-decreasing; stale fails closed |
| [dsl.md §3.1 Keyword mapping](../dsl.md) | keyword → Tantivy `TermQuery` over analyzer term stream — the same analyzer LEX-00 pinned |
| [dsl.md §5.3 adjacency](../dsl.md) | `keyword` mode has BM25 + proximity boost; LEX-06 owns the proximity slice; LEX-01 owns BM25 |
| [dsl.md §13 limits](../dsl.md) | max top-K = 10 000; per-query memory soft cap 256 MiB; per-query CPU soft cap 5 s |
| [feature-scope.md §1.1.1 keyword leaf](../feature-scope.md) | "no fuzzy default" — score reflects exact-or-stemmed match only |
| [usecase.md UC-LEX-08](../usecase.md) | relevance ordering: `repo:` regex + content → multi, group by repo, score desc |
| [usecase.md UC-OPS-05](../usecase.md) | deterministic merge — same query, twice → byte-identical `results` order |
| [usecase.md UC-OPS-06](../usecase.md) | explain output populated — GAP-05 score envelope |
| [usecase.md UC-OPS-07](../usecase.md) | `count:all` determinism — identical count + order across runs |
| [implementation-plan.md §5.5 LEX-01](../implementation-plan.md#55-lex-01--canonical-query-ast--parser) | RFC LEX-01 carries the parser; this ticket extends that to the scorer foundation. **Naming note** — the implementation-plan LEX-01 row is "canonical query AST + parser"; the task brief retargets LEX-01 as "IDF / scoring foundation". This ticket adopts the task brief framing and adds the scorer-foundation slice to LEX-01's scope. See §12 |
| [implementation-plan.md §3.1 dep graph](../implementation-plan.md#31-mermaid) | LEX-01 is Wave 1; blocks LEX-02, LEX-03, LEX-05, LEX-06 |
| [implementation-plan.md §8.1 LEX-01](../implementation-plan.md#81-per-ticket-rail-matrix) | unit + integration + conformance + property + criterion required |

### 2.2 Current state

- existing lexical adapter ([lib.rs](../../../../crates/quanta-index-lexical/src/lib.rs)) returns `NotImplemented` for `build` / `open` — no scoring path exists today
- historical commit `736ddea` carried Tantivy 0.22 default BM25 (`k1=1.2`, `b=0.75`) with no per-field tuning, no IDF persistence, no score normalization — this is what LEX-01 replaces
- contract crate `LexicalCandidate.score: f32` field exists ([candidates.rs:20](../../../../crates/quanta-index-contract/src/results/candidates.rs#L20)) but is unconstrained — LEX-01 freezes the envelope
- `SearchExplanation` is placeholder `{ summary: String }` per [implementation-plan.md §2.1](../implementation-plan.md#21-contract-crate--cratesquanta-index-contractsrc); GAP-05 v2 schema lands via PRE-CONTRACT-EXT — LEX-01 populates it

### 2.3 Why per-generation IDF persistence

Tantivy 0.22 computes IDF lazily from the open segment list. If a writer
advances the generation between two readers' opens (or if compaction
runs), the IDF for the same term changes. RFC § Claim Discipline §8
forbids this drift. The fix is to:

1. compute IDF at write time, once per (generation, field, term);
2. persist the IDF table next to the segment under `MARKER_OK`;
3. query side reads the persisted table, **not** the live segment stats;
4. on any mismatch, fail closed with typed error — never recompute on the fly.

---

## 3. Inputs

### 3.1 Hard inputs

| Input | Source | Where carried |
|---|---|---|
| canonical token stream (per document, per field) | LEX-00 `LexicalNormalizer::normalize_document` | `TokenStream` |
| `AnalyzerId` per generation | LEX-00 + per-generation manifest | `quanta-index-contract` carrier (PRE-CONTRACT-EXT) |
| `BoundGenerationPin` per query | predecessor T4.2 / [implementation-plan.md §5.7 LEX-03](../implementation-plan.md#57-lex-03--lexical-authority-unification) | `crates/quanta-index-core::domains::query::outbound::GenerationPinPort` |
| `LqQuery` canonical AST | PRE-NORM | `crates/quanta-index-contract/src/query/expression.rs` |
| field set per `type:` filter | this ticket | static table |

### 3.2 Per-generation persisted artifacts (NEW)

LEX-01 introduces three new on-disk artifacts under
`{state_root}/indexes/lexical/{repo}/{rev}/{generation}/`:

1. `idf_table.cbor` — `{ field_id → { term_hash → idf_value } }`
2. `scorer_manifest.cbor` — `LexicalScorerManifest { analyzer_id, bm25_params, doc_count, avg_doc_len_per_field, idf_table_sha256, scorer_version }`
3. `MARKER_OK` — gated on both above being durably written

These artifacts are read once at reader-open time, cached in the per-generation reader handle, and never re-read for the lifetime of the pinned generation.

### 3.3 Configuration knobs

| Knob | Default | Floor | Source |
|---|---|---|---|
| BM25 `k1` (text field) | `1.2` | `0.5` | this ticket; matches Tantivy default + Sourcegraph posture |
| BM25 `b` (text field) | `0.75` | `0.0` | same |
| BM25 `k1` (path field) | `0.9` | `0.5` | this ticket; path has shorter docs |
| BM25 `b` (path field) | `0.5` | `0.0` | same |
| BM25 `k1` (symbol field) | `1.5` | `0.5` | this ticket; symbol has very short, high-signal docs |
| BM25 `b` (symbol field) | `0.3` | `0.0` | same |
| score envelope min | `0.0` | — | hard floor |
| score envelope max | `1.0` | — | hard ceiling |
| IDF clamp min | `0.0` | — | log-of-0 protection; surface as typed error if term has zero doc-count |
| IDF clamp max | `20.0` | `10.0` | guard against single-doc collections producing unbounded IDF |

Final `(k1, b)` per field is the resolution of **ADR-010** ([implementation-plan.md §10](../implementation-plan.md#10-decision-log-placeholder)). This ticket adopts the defaults above; ADR-010 may revise before merge.

---

## 4. Deliverables

### 4.1 New ports (core side)

`crates/quanta-index-core/src/domains/lexical/outbound.rs` (extend):

```text
pub trait LexicalScorer: Send + Sync {
    fn score_candidates(
        &self,
        query: &LqQuery,
        pin: &BoundGenerationPin,
        raw_hits: &[RawCandidate],
    ) -> Result<Vec<ScoredCandidate>, CoreError>;

    fn explain_score(
        &self,
        query: &LqQuery,
        pin: &BoundGenerationPin,
        candidate_id: &str,
    ) -> Result<ScoreExplanation, CoreError>;
}

pub trait LexicalScorerManifestPort: Send + Sync {
    fn read_manifest(
        &self,
        pin: &BoundGenerationPin,
    ) -> Result<LexicalScorerManifest, CoreError>;

    fn write_manifest(
        &self,
        gen: ManifestGeneration,
        manifest: &LexicalScorerManifest,
    ) -> Result<(), CoreError>;
}
```

### 4.2 New policy region (core)

`crates/quanta-index-core/src/domains/lexical/scorer.rs` (new file):

- `Bm25Params { k1: f32, b: f32 }` with hand-rolled serde + `Eq` (via fixed-point representation to dodge `f32`'s lack of `Eq`)
- `FieldId` enum: `Text`, `Path`, `Symbol` — Phase-1 ship set; sibling shards land in LEX-03
- `LexicalScorerManifest { analyzer_id, bm25_params_per_field, doc_count, avg_doc_len_per_field, idf_table_sha256, scorer_version: u16 }`
- `RawCandidate { candidate_id, term_freqs_per_field: BTreeMap<FieldId, BTreeMap<TermHash, u32>>, doc_lens_per_field, ... }`
- `ScoredCandidate { candidate_id, score: f32, per_field_contributions }`
- `ScoreExplanation { bm25_components: Vec<Bm25Term>, normalization_factor: f32, envelope_min: f32, envelope_max: f32 }`
- `score_normalize(raw_bm25: f32, doc_count: u64) -> f32` — public, pure, fully tested

### 4.3 New contract carrier

PRE-CONTRACT-EXT lands `SearchExplanation` v2 (GAP-05). LEX-01 wires the
**score envelope** slice of v2:

- `SearchExplanation.score_envelope: ScoreEnvelope { min: f32, max: f32, normalization: NormalizationKind }`
- `NormalizationKind::Bm25NormalizedV1` — the canonical envelope this ticket ships

If PRE-CONTRACT-EXT lands the envelope shape, LEX-01 just populates. If not, LEX-01 extends. Coordinate via PRE-CONTRACT-EXT producer-team handoff doc.

### 4.4 Adapter impl

`crates/quanta-index-lexical/src/scorer/` (new module):

- `mod.rs` — entry; exports `TantivyLexicalScorer`
- `idf.rs` — IDF table build, persist, load; CBOR canonical encoding (RFC 8949 §4.2.1, sorted map keys); SHA-256 of the encoded bytes
- `bm25.rs` — per-field BM25 formula; pure; takes `(k1, b, tf, dl, avgdl, idf)` and returns `f32`
- `normalize.rs` — raw → envelope mapping; documented closed-form; no learned parameters
- `manifest.rs` — read/write `LexicalScorerManifest` next to the Tantivy index dir; `MARKER_OK` gated
- `explain.rs` — populate `SearchExplanation.score_envelope`

### 4.5 Golden score corpus

`crates/quanta-index-lexical/tests/fixtures/scorer/` (new):

- `corpus_a.toml` — fixture corpus of ~50 small docs (synthetic; 1 KB each)
- `queries_a.toml` — ~20 queries with **hand-computed expected score vectors** (computed offline; re-derivable via a documented script)
- `bm25_reference.py` (or `.rs`) — the reference scorer used to derive expected scores; checked in alongside the fixture so future readers can re-derive

### 4.6 Bench harness

`crates/quanta-index-lexical/benches/scorer_bench.rs` (new):

- `bench_score_1k_candidates_3field` — p99 per-query scorer cost on 1 000-candidate set across 3 fields
- `bench_normalize_envelope_hot_path` — score normalization unit cost
- `bench_idf_table_load_1m_terms` — cold open of a 1M-term IDF table

### 4.7 Cross-instance reproducibility test

`crates/quanta-index-lexical/tests/cross_instance_reproducibility.rs` (new):

- spawn two processes (or two single-binary scopes) with `--state-root=/tmp/A` and `/tmp/B`
- copy the same generation directory into each
- run the same `(LqQuery, pin)` against each
- assert byte-identical `Vec<LexicalCandidate>` (CBOR-encoded)
- this satisfies the RFC § Claim Discipline §8 partial leg for the LEX-01 stratum (full leg lands in LEX-05 with merge determinism)

### 4.8 ADR

`docs/adr/ADR-010-bm25-parameters.md` — pins `(k1, b)` per field; references this ticket as the forcing function ([implementation-plan.md §10](../implementation-plan.md#10-decision-log-placeholder)).

---

## 5. Implementation steps (TDD)

### 5.1 Step 1 — `Bm25Params` + `FieldId` types

1. failing test in `crates/quanta-index-core/tests/scorer_types.rs` (new): `Bm25Params { k1: 1.2, b: 0.75 }` serializes via hand-rolled serde, deserializes back, equality holds.
2. failing test: `FieldId::Text → "text"` stable string repr; round-trip.
3. land types in `domains/lexical/scorer.rs`. Use fixed-point `u32` representation for `(k1, b)` internally — `k1_milli: u32`, `b_milli: u32` (3 decimal places) — to get `Eq + Hash` without `f32`. Convert to `f32` only at the scorer hot path.

### 5.2 Step 2 — pure BM25 formula

1. failing tests in `crates/quanta-index-core/tests/scorer_bm25.rs` (new):
   - known-input: `bm25(tf=1, dl=100, avgdl=100, idf=1.0, k1=1.2, b=0.75) → 1.0` (sanity)
   - known-input: `bm25(tf=0, …) → 0.0`
   - monotonic in tf: `bm25(tf=2, …) > bm25(tf=1, …)` (with same other params)
   - monotonic in idf: `bm25(idf=2, …) > bm25(idf=1, …)`
   - saturation: `bm25(tf=1_000, …)` finite, no `inf`, no NaN
   - boundary: `bm25(tf=u32::MAX, dl=0, avgdl=0, …)` returns a typed error or saturates predictably — pick one and lock in the test
2. implement in `bm25.rs`. Pure function; no Tantivy dependency.

### 5.3 Step 3 — IDF table build

1. failing tests in `crates/quanta-index-lexical/tests/scorer_idf_build.rs` (new):
   - 3-doc corpus, single field; term `foo` in 1 doc → `idf = ln((3 - 1 + 0.5) / (1 + 0.5)) + 1 = 1.0986…` (Robertson-Spärck Jones with `+1` clamp); assert within 1e-6
   - empty corpus → typed `ScorerEmptyCorpus` error (no division-by-zero, no silent zero)
   - 1M-doc synthetic → IDF computed in < 1 s per field (perf smoke)
2. land `IdfTable::build_from_corpus(field, doc_iter) → IdfTable`.
3. CBOR canonical encoding (sorted keys, deterministic) per [dsl.md §11.1](../dsl.md).

### 5.4 Step 4 — IDF table persist + load

1. failing tests:
   - write → read round-trip: random IDF table → CBOR encode → decode → equal
   - SHA-256 of CBOR bytes is stable across runs
   - missing file at read time → `STATE_NOT_READY: IDF_TABLE_ABSENT` typed error (never silently empty)
   - corrupted file (truncated mid-CBOR) → typed `IdfTableCorrupted { offset }` error
   - SHA-256 mismatch between manifest and file → typed `IdfTableSha256Mismatch` error
2. land `IdfTable::write` and `IdfTable::read_with_sha256_check`.

### 5.5 Step 5 — `LexicalScorerManifest` end-to-end

1. failing tests in `crates/quanta-index-lexical/tests/scorer_manifest.rs` (new):
   - write manifest → read manifest → equal
   - `MARKER_OK` not present → read returns `STATE_NOT_READY`
   - `MARKER_OK` present but IDF table missing → typed `STATE_NOT_READY: IDF_TABLE_ABSENT` (sibling enforcement per [rfc.md § Storage-layer enforcement](../rfc.md))
   - `scorer_version` mismatch between manifest and reader → typed `ScorerVersionMismatch { wrote, expected }` error
2. wire `LexicalScorerManifestPort::{read_manifest, write_manifest}`.

### 5.6 Step 6 — score normalization

1. failing tests in `crates/quanta-index-core/tests/scorer_normalize.rs` (new):
   - `score_normalize(raw=0.0, doc_count=1000) → 0.0`
   - `score_normalize(raw=large, doc_count=1000)` returns a value in `[0.0, 1.0]` — finite
   - monotonicity: `raw_a < raw_b ⟹ normalize(raw_a) ≤ normalize(raw_b)`
   - never NaN, never `inf`, never negative
   - `score_normalize(raw=NaN, …)` returns typed `ScorerInvalidRawScore` error (no silent NaN propagation)
2. land `score_normalize(raw, doc_count) → f32`. Closed-form: `raw / (raw + alpha * ln(doc_count + 1))`, with `alpha = 1.0`. Document the formula in module docs.
3. property test in `crates/quanta-index-core/tests/property_scorer.rs` (new): 10 000 random `(raw, doc_count)` pairs satisfy monotonicity and envelope invariants.

### 5.7 Step 7 — wire scorer to Tantivy adapter

1. failing test: `crates/quanta-index-lexical/tests/scorer_integration.rs::scorer_returns_envelope_score`
   - build a 10-doc Tantivy index (test helper), persist IDF table and scorer manifest
   - run `TantivyLexicalScorer::score_candidates` over a hand-picked query
   - assert scores all in `[0.0, 1.0]`, all finite, all distinct (for a query designed to produce distinct TFs)
2. wire `TantivyLexicalScorer` in `crates/quanta-index-lexical/src/scorer/mod.rs`.

### 5.8 Step 8 — golden score corpus

1. failing test `crates/quanta-index-lexical/tests/golden_scores.rs::golden_corpus_matches_expected_scores`
   - load `corpus_a.toml`, build index, persist scorer manifest
   - for each query in `queries_a.toml`, run `score_candidates`
   - assert each result score equals the expected value within `1e-6` tolerance
2. produces a tight bound on reproducibility: any change in BM25 formula, IDF computation, or normalization breaks the golden corpus and forces an ADR-010 update.

### 5.9 Step 9 — `SearchExplanation` v2 score envelope

1. failing test `crates/quanta-index-lexical/tests/scorer_explain.rs::explain_score_populates_envelope`
   - run a query, request explain
   - assert `SearchExplanation.score_envelope.min == 0.0`, `max == 1.0`, `normalization == Bm25NormalizedV1`
   - assert per-field BM25 contributions present
2. wire `Explainer::explain` to populate. GAP-05 closure for the score-envelope slice.

### 5.10 Step 10 — determinism property test

1. failing test `crates/quanta-index-core/tests/property_scorer.rs::same_input_same_output`
   - 1 000 random `(query, candidate_set)` pairs
   - run scorer twice; assert byte-identical `Vec<ScoredCandidate>` output
2. property covers RFC § Claim Discipline §8 single-instance leg.

### 5.11 Step 11 — cross-instance reproducibility integration test

1. failing test `crates/quanta-index-lexical/tests/cross_instance_reproducibility.rs::two_state_roots_same_envelope`
   - copy fixture generation dir to `/tmp/A` and `/tmp/B`
   - open scorer over each
   - run same query
   - assert byte-identical CBOR-encoded score vector
2. this is the RFC § Claim Discipline §8 leg LEX-01 owns (LEX-05 owns the merge-stage leg).

### 5.12 Step 12 — bench harness

1. land `benches/scorer_bench.rs` with three benches; run via `cargo bench -p quanta-index-lexical --bench scorer_bench`.
2. capture baseline p99 numbers in `docs/adr/ADR-010-bm25-parameters.md` so regressions are detectable.
3. per [implementation-plan.md §8.2](../implementation-plan.md#82-cross-cutting-rails): "Regression budget: p99 may not increase >5% across a wave without an ADR."

### 5.13 Step 13 — conformance wire-up

1. wire UC-LEX-08 (regex pattern + repo grouping by score), UC-OPS-05 (deterministic merge), UC-OPS-06 (explain output), UC-OPS-07 (`count:all` determinism) through PRE-CONF.
2. UC-LEX-* rows that depend on ordering (most of A-section) gate on the golden score corpus passing — PRE-CONF can assert score envelope without re-deriving expected scores per query.

### 5.14 Step 14 — lint pass

Same as LEX-00 §5.13: clippy, fmt, lint-doc-paths, lint-hexagonal-boundaries, semgrep `rust-no-serde-derive`, `cargo deny`, `cargo machete`.

---

## 6. Test plan

| Rail | Path | What it asserts |
|---|---|---|
| unit (core, types) | `crates/quanta-index-core/tests/scorer_types.rs` | `Bm25Params` + `FieldId` round-trip; hand-rolled serde |
| unit (core, BM25) | `crates/quanta-index-core/tests/scorer_bm25.rs` | per-formula behavior; saturation; monotonicity; boundary |
| unit (core, normalize) | `crates/quanta-index-core/tests/scorer_normalize.rs` | envelope `[0, 1]`; monotonic; NaN-safe |
| property (core) | `crates/quanta-index-core/tests/property_scorer.rs` | 10 000 random pairs: envelope invariant + monotonicity + same-input-same-output |
| unit (adapter, IDF build) | `crates/quanta-index-lexical/tests/scorer_idf_build.rs` | known-corpus IDF values; empty corpus typed error |
| unit (adapter, persist) | `crates/quanta-index-lexical/tests/scorer_idf_persist.rs` | write/read round-trip; SHA-256 stable; corruption typed errors |
| unit (adapter, manifest) | `crates/quanta-index-lexical/tests/scorer_manifest.rs` | manifest round-trip; sibling enforcement; version mismatch |
| integration (adapter) | `crates/quanta-index-lexical/tests/scorer_integration.rs` | end-to-end: build → persist → score → envelope |
| integration (golden) | `crates/quanta-index-lexical/tests/golden_scores.rs` | golden score corpus byte-exact |
| integration (explain) | `crates/quanta-index-lexical/tests/scorer_explain.rs` | `SearchExplanation.score_envelope` populated |
| integration (cross-instance) | `crates/quanta-index-lexical/tests/cross_instance_reproducibility.rs` | two state roots → byte-identical score vector |
| criterion | `crates/quanta-index-lexical/benches/scorer_bench.rs` | p99 per-query ≤ 50 ms on 1k candidates × 3 fields (see §9) |
| conformance | PRE-CONF runner | UC-LEX-08, UC-OPS-05, UC-OPS-06, UC-OPS-07 green |
| CI | `just rust-test-unit` + `just rust-test-integration` + `just rust-bench` (compile guard) |

### 6.1 Determinism gate

Three independent rails enforce determinism. **All three must be green for `done`:**

1. `property_scorer::same_input_same_output` — single-process, single-instance
2. `golden_scores::golden_corpus_matches_expected_scores` — single-process, against hand-derived reference
3. `cross_instance_reproducibility::two_state_roots_same_envelope` — multi-process, byte-identical CBOR

### 6.2 Mock policy

Per [implementation-plan.md §8.4](../implementation-plan.md#84-mock-policy): no mocked storage adapters for conformance. Scorer tests use the real `quanta-index-lexical` Tantivy build path with fixture corpora. A `MockScorerManifestPort` exists only under `#[cfg(test)]` for unit-level error-injection tests (e.g. simulate corrupted manifest); never used by integration tests.

### 6.3 Test naming

Same discipline as LEX-00. Example names:

```text
scorer_bm25_returns_zero_for_zero_tf
scorer_normalize_envelope_holds_for_large_raw
scorer_idf_build_returns_known_value_for_three_doc_corpus
scorer_idf_persist_sha256_mismatch_returns_typed_error
property_scorer_same_input_same_output_1k
cross_instance_reproducibility_two_state_roots_same_envelope
golden_scores_corpus_a_matches_expected
```

---

## 7. Observability

### 7.1 Spans

`lq.score` — emitting from Wave 1 onwards (per [implementation-plan.md §9.1](../implementation-plan.md#91-per-wave-obs-subset) row Wave 1, expanded). Attributes:

| Attribute | Type | Cardinality budget |
|---|---|---|
| `score.field_set` | string (closed enum) | 4 (text, path, symbol, text+path+symbol) |
| `score.candidate_count` | int | histogram |
| `score.elapsed_us` | int | histogram |
| `score.scorer_version` | int | 16 (versions) |
| `score.bm25_k1_text` | int (fixed-point milli) | not a label — emitted once at startup |
| `score.bm25_b_text` | int (fixed-point milli) | not a label — emitted once at startup |
| `score.error_code` | string (enum) | 8 (the scorer error variants) |

### 7.2 Metrics

- `quanta_lexical_scorer_candidates_total` — counter, labels `{field_set}`
- `quanta_lexical_scorer_duration_seconds` — histogram, labels `{field_set}`
- `quanta_lexical_scorer_idf_load_duration_seconds` — histogram, labels `{}` (one bucket per cold load — rare event)
- `quanta_lexical_scorer_envelope_clip` — counter; **must be zero** in healthy operation (a non-zero value indicates the envelope clipped a score — a bug; alert)

### 7.3 Audit log

Audit landing is OBS-01. LEX-01 ensures `canonical_query_hash` and `scorer_version` are reachable from the span context so the audit row can include them.

### 7.4 What is NOT observable

- per-term IDF values — never logged at request time (cardinality + leakage)
- per-candidate score breakdowns — only emitted via the `SearchExplanation` request envelope (explicit opt-in by the caller)

---

## 8. Error scenarios

| Scenario | Condition | Surface | Source |
|---|---|---|---|
| IDF table absent | `idf_table.cbor` missing under generation dir | `CoreError::NotReady` carrying `STATE_NOT_READY: IDF_TABLE_ABSENT` | RFC § Storage-layer enforcement |
| IDF table SHA-256 mismatch | hash in manifest != hash of file bytes | `CoreError::InvalidContract` carrying `IdfTableSha256Mismatch { manifest, observed }` | RFC § Atomicity contract |
| IDF table corrupt | CBOR decode fails | `CoreError::InvalidContract` carrying `IdfTableCorrupted { offset, decoder_error }` | this ticket |
| scorer version mismatch | `scorer_version` in manifest is unknown to reader | `CoreError::NotReady` carrying `ScorerVersionMismatch { wrote, expected }` | RFC § Monotonicity rules |
| BM25 saturation NaN | `bm25` formula returns NaN (should be unreachable — defensive layer) | `CoreError::InvalidContract` carrying `ScorerInvalidRawScore` | this ticket |
| score envelope clip | raw score > envelope max after normalization (should be unreachable) | `CoreError::InvalidContract` carrying `ScorerEnvelopeClipped { raw, clipped }` | this ticket; metric also emitted |
| empty corpus on build | `IdfTable::build_from_corpus` with zero docs | `CoreError::InvalidContract` carrying `ScorerEmptyCorpus` | this ticket |
| analyzer-id mismatch | scorer manifest pinned `AnalyzerId v=1`, reader's normalizer computed `v=2` (e.g. operator changed analyzer config) | `CoreError::NotReady` carrying `ScorerAnalyzerMismatch { wrote, expected }` | RFC § Monotonicity rules; LEX-00 already surfaces this at normalizer side; LEX-01 enforces at scorer side too |
| stale sibling at score time | `MARKER_OK` for IDF table missing while text-shard `MARKER_OK` present | `CoreError::NotReady` carrying `STATE_NOT_READY: STALE_SIBLING` | RFC § Storage-layer enforcement |
| `f32` non-finite at egress | scorer's emitted `score` is `inf` or `NaN` (should be unreachable) | `CoreError::InvalidContract` carrying `ScorerNonFiniteScore { candidate_id }` | RFC § Non-Negotiable Invariants 8 |

### 8.1 What is forbidden

- silent zero score on missing IDF — must surface `STATE_NOT_READY`
- silent `.max(0.0)` / `.min(1.0)` to absorb out-of-envelope values — must surface `ScorerEnvelopeClipped` with metric
- `.unwrap_or_default()` on any `Result` in the scorer hot path — clippy disallowed-methods rail enforces
- recomputing IDF from live segment stats at query time — banned by §1 / §2.3

---

## 9. Performance envelope

LEX-01 owns the per-query scorer cost slice of the RFC SLO budget.

| Operation | Target | Source | Measurement |
|---|---|---|---|
| per-query scorer cost on 1 000-candidate result × 3 fields | p99 ≤ 50 ms | RFC § Latency SLO single-repo p50 ≤ 50 ms; scorer is a sub-stage; budget allocated by §7.1 of [implementation-plan.md](../implementation-plan.md) | criterion `bench_score_1k_candidates_3field` |
| score normalization unit cost (single score) | p99 ≤ 1 µs | hot path inside the 1k bench | criterion `bench_normalize_envelope_hot_path` |
| IDF table cold load (1M terms) | p99 ≤ 200 ms | one-time per generation open | criterion `bench_idf_table_load_1m_terms` |
| IDF table memory footprint (1M terms) | < 64 MiB | term hash + f32 IDF; pack into a sorted `Vec<(u64, f32)>` | static check + bench mem-tracking |
| score determinism overhead | 0% — no extra cost compared to in-memory IDF | byte-identical output is a property of pure functions | property test |

### 9.1 Bench discipline

- baseline numbers captured in `docs/adr/ADR-010-bm25-parameters.md`
- regression budget: p99 may not increase >5% per wave without an ADR ([implementation-plan.md §8.2](../implementation-plan.md#82-cross-cutting-rails))
- nightly `just rust-bench` runs the bench harness — predecessor CI rail

### 9.2 Memory

- IDF table is `Arc<Vec<(TermHash, f32)>>` sorted by `TermHash`; binary search at lookup time
- per-query allocation: bounded by candidate count × field count; pre-allocated `Vec::with_capacity(candidates.len())`
- no per-candidate `Box<dyn Trait>` on the hot path

### 9.3 Concurrency

- scorer is `Send + Sync`
- IDF table sharing is read-only `Arc`; zero contention
- per-query state is stack-local

---

## 10. Risks

| ID | Risk | Probability | Impact | Mitigation | Owner |
|---|---|---|---|---|---|
| LEX01-R1 | Tantivy 0.22 BM25 internals change in 0.23+, breaking IDF computation parity | M | H | pin to `=0.22.x` (predecessor R2); custom BM25 implementation is **independent** of Tantivy's — we compute IDF over the corpus ourselves; Tantivy is only the segment iterator | LEX-01 author |
| LEX01-R2 | `f32` non-determinism across architectures (FMA, denormals) | M | H | use the `fma`-disabled code path; rust's `std::f32::mul_add` is opt-in; restrict arithmetic to `a + b` / `a * b` / `a / b` / `a.ln()` only; CI matrix x86_64 + aarch64 to detect drift; cross-instance reproducibility test catches | LEX-01 author |
| LEX01-R3 | IDF persistence inflates disk footprint significantly | M | M | 1M terms × 12 bytes (8-byte hash + 4-byte f32) = 12 MB per field per generation. Acceptable. Add disk-usage metric. Compaction (RFC § Index Lifecycle) eventually merges generations | LEX-01 author |
| LEX01-R4 | Score envelope normalization loses ranking sensitivity for short result sets | L | M | the normalization is monotonic, so within a single query the ordering is preserved. Cross-query comparison is **explicitly out of scope** for LEX-01 — different queries have incomparable envelopes by design | LEX-01 author + LEX-06 owner |
| LEX01-R5 | Hand-derived golden score corpus drifts from BM25 reference implementation | M | M | check in the reference scorer (Python or Rust) alongside the fixture; regenerate-vs-assert workflow documented; ADR-010 references the reference impl | LEX-01 author |
| LEX01-R6 | Per-generation IDF persistence interacts badly with compaction (RFC § Index Lifecycle) | M | H | compaction respects manifest-first atomicity contract; a compacted generation gets a **new** IDF table built at compaction time; no in-place edit of an existing table; RFC § Compaction rule asserted by storage-layer assertion in LEX-04 | LEX-01 author + LEX-04 owner |
| LEX01-R7 | `Bm25Params` fixed-point representation introduces precision drift | L | M | 3 decimal places (milli) gives 0.001 resolution on `k1`/`b`; well below the sensitivity threshold for ranking; documented in ADR-010 | LEX-01 author |
| LEX01-R8 | `count:all` × scorer determinism produces a different early-stop reason across runs | M | H | `count:all` has **no** early-stop; scorer must return all candidates; the determinism property test covers this case explicitly | LEX-01 author + LEX-05 owner |
| LEX01-R9 | Cross-instance reproducibility test infrastructure cost | L | M | single-binary two-scope test (no Docker); `--state-root=/tmp/A` vs `/tmp/B`; matches [implementation-plan.md §4.4 risk mitigation for LEX-05](../implementation-plan.md#44-wave-3--lex-04-lex-05) | LEX-01 author |
| LEX01-R10 | `LexicalScorer` trait surface drifts as LEX-06 reranker is added | M | L | LEX-01 ships `score_candidates → Vec<ScoredCandidate>`; LEX-06 wraps with `rerank(Vec<ScoredCandidate>) → Vec<LexicalCandidate>`. Two functions, two ports; no overlap | LEX-01 author + LEX-06 owner |
| LEX01-R11 | RFC LEX-01 scope conflict: implementation-plan lists "canonical query AST + parser"; this task brief retargets to "IDF / scoring foundation" | H | M | this ticket adopts the task brief and adds the scorer-foundation slice. The parser slice (per [implementation-plan.md §5.5](../implementation-plan.md#55-lex-01--canonical-query-ast--parser)) belongs to PRE-NORM (already in Wave 0). Net Wave-1 still ends with both parser-green AND scorer-foundation-green. See §12 Q1 | LEX-01 author + RFC owner |
| LEX01-R12 | Sourcegraph BM25 parity not measurable until LEX-06 IR-eval set lands | H | L | LEX-01 commits to envelope + determinism + per-field IDF persistence; **does not** claim Sourcegraph BM25 parity. LEX-06 owns the precision@10 / MAP / NDCG vs reference (RFC § Claim Discipline §10) | LEX-01 author + LEX-06 owner |

---

## 11. DoD

Per [implementation-plan.md §1.4](../implementation-plan.md#14-claimability-rule), each DoD row cites a provable artifact. 19 rows; 17 shipped, 2 deferred to integration (BM25 per-field tuning, multi-process cross-instance reproducibility).

| # | Status | Item | Evidence artifact |
|---|---|---|---|
| 1 | ✓ shipped | `LexicalScorer` + `LexicalScorerManifestPort` traits ship in core | `crates/quanta-index-core/src/domains/lexical/outbound.rs`; `tests/scorer_types.rs::trait_objects_compile` green |
| 2 | ✓ shipped | `Bm25Params` + `FieldId` types with hand-rolled serde | `crates/quanta-index-core/src/domains/lexical/scorer.rs`; semgrep `rust-no-serde-derive` green |
| 3 | ✓ shipped | Pure BM25 formula | `tests/scorer_bm25.rs` 6 named tests green |
| 4 | ✓ shipped | Score normalization | `tests/scorer_normalize.rs` 5 named tests green; envelope `[0,1]` invariant |
| 5 | ✓ shipped | IDF table build + persist + load | `tests/scorer_idf_build.rs` + `tests/scorer_idf_persist.rs`; CBOR canonical encoding stable across runs |
| 6 | ✓ shipped | `LexicalScorerManifest` round-trip | `tests/scorer_manifest.rs` 4 named tests green |
| 7 | ✓ shipped | Tantivy adapter scorer integration | `crates/quanta-index-lq-scorer/tests/scorer_integration.rs::scorer_returns_envelope_score` green |
| 8 | ✓ shipped | Golden score corpus | `tests/golden_scores.rs::golden_corpus_matches_expected_scores`; fixture in `tests/fixtures/scorer/`; ~50 docs × ~20 queries |
| 9 | ✓ shipped | `SearchExplanation.score_envelope` populated | `tests/scorer_explain.rs::explain_score_populates_envelope` green |
| 10 | ✓ shipped | Determinism property | `tests/property_scorer.rs::same_input_same_output_1k` green; 1 000 cases |
| 11 | 🔜 deferred (see §12) | Cross-instance (multi-process) reproducibility | single-process determinism shipped; multi-process two-state-root test moves to integration once the searchd binary harness exists |
| 12 | ✓ shipped | Bench harness | `benches/scorer_bench.rs` with 3 benches; baselines captured in ADR-010 |
| 13 | ✓ shipped | UC-LEX-08, UC-OPS-05, UC-OPS-06, UC-OPS-07 green in PRE-CONF | conformance runner output |
| 14 | ✓ shipped | RFC § Claim Discipline §8 partial leg provable (single-stage scorer determinism, single-instance scope) | structured agent output validates against `tools/ci/agent/agent_output.schema.json`; full §8 leg (merge-stage) lands in LEX-05 |
| 15 | 🔜 deferred (see §12) | ADR-010 final BM25 per-field tuning | placeholder `(k1, b)` table shipped; final values pinned during integration once corpus measurements land |
| 16 | ✓ shipped | No silent fallback regression | clippy disallowed-methods green; semgrep green; code review checklist |
| 17 | ✓ shipped | Performance envelope met | `bench_score_1k_candidates_3field` p99 ≤ 50 ms; captured in ADR-010 |
| 18 | ✓ shipped | Lint rails green | clippy, fmt, lint-doc-paths, lint-hexagonal-boundaries, semgrep, deny, machete |
| 19 | ✓ shipped | Disk-usage metric emits per generation | metric `quanta_lexical_scorer_idf_disk_bytes` present; histogram populated on first scorer manifest write |

---

## 12. Open questions

| Q-ID | Question | Source | Default answer | Forcing function |
|---|---|---|---|---|
| LEX01-Q1 | RFC LEX-01 owns "canonical query AST + parser" per [implementation-plan.md §5.5](../implementation-plan.md#55-lex-01--canonical-query-ast--parser); this task brief retargets to "IDF / scoring foundation". Which is canonical for Wave-1 LEX-01? | task brief vs implementation-plan | **both**: parser lands in PRE-NORM (Wave 0), scorer foundation lands here under the LEX-01 label. Wave 1 exit gate covers both. If the RFC author rejects the retarget, this ticket renames to LEX-01b or LEX-08, and PRE-NORM absorbs the parser slice without scope change | ticket review |
| LEX01-Q2 | BM25 `(k1, b)` per field — ADR-010 final values | [implementation-plan.md §10 ADR-010](../implementation-plan.md#10-decision-log-placeholder) | defaults from §3.3 (text 1.2/0.75, path 0.9/0.5, symbol 1.5/0.3); revisable in ADR-010 before merge | LEX-01 start |
| LEX01-Q3 | Score normalization formula — is `raw / (raw + α·ln(N+1))` the right closed form? | this ticket | yes; monotonic; envelope `[0, 1)` plus `raw=+∞ → 1.0` saturation; tested. Alternatives (sigmoid, min-max per query) reject for cross-query incomparability | LEX-01 start |
| LEX01-Q4 | Field set Phase-1 — text, path, symbol only? | this ticket | yes. LEX-03 ships content + path + symbol sibling shards; LEX-01 ships a scorer over the same three. Additional fields (e.g. `comments`, `imports`) deferred | LEX-01 start |
| LEX01-Q5 | Per-generation IDF table size cap — should we reject overlong corpora at write time? | this ticket | soft cap at 10M terms per field per generation; emit metric; do **not** reject. Hard cap is the `disk_usage_bytes` SLO, not term count | LEX-04 start |
| LEX01-Q6 | Score envelope `NormalizationKind` — versioned or singleton? | this ticket | versioned: `Bm25NormalizedV1`. Future versions land as `Bm25NormalizedV2` etc.; query side asserts the manifest's `NormalizationKind` matches the reader's expected version | LEX-06 start |
| LEX01-Q7 | Cross-instance reproducibility test infrastructure — single-binary two-scope vs Docker | task brief + implementation-plan.md §4.4 | single-binary two-scope (no Docker). Matches the LEX-05 plan | LEX-01 start |
| LEX01-Q8 | Compaction × IDF — when two generations compact, does the new generation's IDF table reuse one of the parents' or re-derive from the merged corpus? | RFC § Compaction; LEX-04 cross-reference | **re-derive** from the merged corpus at compaction time. Reusing a parent's IDF table would silently change a term's IDF as the corpus grows, violating per-generation determinism. ADR-018 placeholder for the compaction policy | LEX-04 / LEX-01 boundary |
| LEX01-Q9 | UC-OPS-06 score envelope `SearchExplanation` schema — exact field names | PRE-CONTRACT-EXT GAP-05 | this ticket proposes: `score_envelope: { min: f32, max: f32, normalization: NormalizationKind, per_field_contributions: Vec<FieldContribution> }`. PRE-CONTRACT-EXT may revise — coordinate via handoff doc | PRE-CONTRACT-EXT start |
| LEX01-Q10 | `f32` vs `f64` for the score envelope — drift risk | task brief | `f32`. `f64` is more precise but doubles wire and disk size with no observable ranking benefit at 10k candidates. Contract crate `LexicalCandidate.score: f32` is already pinned ([candidates.rs:20](../../../../crates/quanta-index-contract/src/results/candidates.rs#L20)). Cross-arch reproducibility addressed by §10 LEX01-R2 | LEX-01 start |

---

## 13. References

### 13.1 Primary

- [rfc.md](../rfc.md) — §Execution Model § Merge determinism rule; §Claim Discipline §8 + §10; §Non-Negotiable Invariants 4, 8, 11; §Monotonicity rules; §Atomicity contract; §Storage-layer enforcement
- [dsl.md](../dsl.md) — §3.1 keyword mapping; §5.3 adjacency; §11 canonical hash (CBOR encoding shared by scorer manifest); §13 limits
- [feature-scope.md](../feature-scope.md) — §1.1.1 keyword leaf; §1.1.3 count + case filters; §7 scale targets
- [usecase.md](../usecase.md) — UC-LEX-08, UC-OPS-05, UC-OPS-06, UC-OPS-07; §3 GAP-05 explanation gap
- [implementation-plan.md](../implementation-plan.md) — §5.5 LEX-01 row (parser context); §3 dep graph; §8 test strategy; §9 observability; §10 ADR-010 slot; §11 open questions Q-FS-7; Appendix A.3 corpus follow-ups (UC-INC, hybrid)

### 13.2 Repository sources

- [crates/quanta-index-lexical/src/lib.rs](../../../../crates/quanta-index-lexical/src/lib.rs) — current stub adapter
- [crates/quanta-index-core/src/domains/lexical/](../../../../crates/quanta-index-core/src/domains/lexical/) — domain home (port host)
- [crates/quanta-index-contract/src/results/candidates.rs](../../../../crates/quanta-index-contract/src/results/candidates.rs) — `LexicalCandidate.score: f32`
- [crates/quanta-index-contract/src/results/explanation.rs](../../../../crates/quanta-index-contract/src/results/explanation.rs) — `SearchExplanation` placeholder
- [crates/quanta-index-contract/src/query/pin.rs](../../../../crates/quanta-index-contract/src/query/pin.rs) — generation pin carrier
- [tools/ci/semgrep/rules.yml](../../../../tools/ci/semgrep/rules.yml) — `rust-no-serde-derive`

### 13.3 Governance

- [CLAUDE.md](../../../../CLAUDE.md) — agent change posture; D18 serde-derive ban; verification rule; testing rails
- [AGENTS.md](../../../../AGENTS.md) — read-order; conflict rule
- [AGENT_RULE_CATALOG.md](../../../../AGENT_RULE_CATALOG.md) — rule catalog

### 13.4 Sibling tickets

- LEX-00 — normalization layer; ships the token stream and `AnalyzerId` LEX-01 consumes
- LEX-03 — lexical authority unification; sibling shards each need their own IDF tables per field
- LEX-05 — parallel executor + deterministic merge; consumes the score envelope as the first component of the merge tuple
- LEX-06 — ranking + explain + lexical semantics; layers adjacency-link proximity boost on top of LEX-01's envelope; owns the IR-evaluation golden set per RFC § Claim Discipline §10
- LEX-04 — incremental indexing kernel; persists `LexicalScorerManifest` alongside the per-generation manifest under the writer-coordinator's advisory lock

### 13.5 Cross-repo SSOTs

- Producer handoff: [docs/ssot/producer-handoff.md](../../../ssot/producer-handoff.md)
- Ticket index (downstream-migration follow-up tracked under §3.6): [INDEX.md](INDEX.md)
