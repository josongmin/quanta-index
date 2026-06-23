# RFC: Semantic embedding pipeline — scalability + measured experimentation

- Status: `proposed`
- Date: 2026-06-23
- Area: `quanta-index` semantic search plane (`quanta-index-embed`, `quanta-index-search-plane`, `quanta-index-searchd`, `quanta-index-searchd-harness`)
- Depends on: SEM-OWN OpenAI provider wave (landed — `docs/plans/may-25-search-owned-semantic-derivation/tickets/SEM-OWN-OPENAI-PROVIDER.md`)
- Keywords: MUST / SHOULD / MUST NOT per RFC-2119

## 1. Summary

The real (OpenAI) neural embedder has landed and is opt-in. This RFC specifies the
next increment: make the embedding pipeline (a) **measurable** so chunking / model /
dimension / fusion changes are adopted by evidence, not guess, and (b) **correct +
scalable** under cost and rate-limit pressure. It is the reconciled output of a
structural audit + an adversarial review + an objective review; the rejected
alternatives in §7 record why the obvious-but-wrong options were dropped.

## 2. Motivation / Context

- Semantic search now supports a real neural embedder, but the pipeline was built
  for the deterministic hash embedder. Three structural weaknesses surfaced:
  cost (duplicate-text embedding), reliability (synchronized retry storms), and
  the inability to *prove* a config change helped (no semantic relevance gate).
- There is no semantic relevance gate today, so any tuning (model, dimension,
  fusion, chunking) is unfalsifiable. Measurement MUST come before throughput
  tuning.

## 3. Current state — structural audit (verified, file:line)

- **Embedder seam (good).** `TextEmbeddingProvider` (`quanta-index-core` `domains/semantic/outbound.rs`) unifies query + corpus; impls `HashingQueryTextEmbedder` and `OpenAiEmbeddingProvider`; one instance serves both via `runtime.rs::build_semantic_embedders` + `QueryEmbedderAdapter`.
- **Corpus embed is sequential + blocking.** `ingest_dispatcher.rs::derive_semantic_batch_from_search_corpus_batch` does one `embedder.embed_batch(...)` **per scope**, scopes iterated sequentially; `OpenAiEmbeddingProvider::embed_batch` loops `texts.chunks(max_batch)` sequentially; `reqwest::blocking`. Runs inside `DirectSearchCorpusMaterializer::publish_batch` on the ingest handler thread.
- **Cache coalesces per call but does NOT dedup identical texts within a call.** `cache.rs::CachingEmbeddingProvider::embed_batch` collects all misses → one inner `embed_batch`, but `miss_texts` is positional — two identical miss texts ⇒ two vectors requested.
- **Retry backoff has no jitter.** `openai.rs::backoff_delay` = `RETRY_BASE_DELAY * 2^attempt` (deterministic).
- **Eval harness exists but is lexical-only.** `quanta-index-searchd-harness/src/relevance/` has `recall_at_k` / `ndcg_at_k` / `reciprocal_rank_at_k` (`metrics.rs`) + a checked-in path-keyed `JudgedQuery` corpus with ordering invariants (`corpus.rs`). `RelevanceRoute` has only `Lexical`. The lexical route projects candidate → `repo_relative_path` (`report.rs`); the semantic route returns chunk-ids, which the path-keyed grades cannot score.
- **Generation-scoped isolation exists.** Per-generation `EmbeddingModelContract` (model_id/dim) + query-time `ensure_query_model_matches_index_v1` gate. Enables A/B by building two generations.
- **Operational knobs are env-wired.** `config.rs::OpenAiEmbedderTuning` (`QUANTA_INDEX_EMBED_BATCH` / `_MAX_RETRIES` / `_TIMEOUT_SECS` / `_CACHE`) threaded into the provider at the composition root.

## 4. Goals / Non-goals

Goals:
- A CI-safe, deterministic **semantic relevance gate** (no API key) that makes config changes measurable.
- Ship measurement-independent **correctness/cost fixes** (dedup, jitter) now.
- A measured path to throughput tuning (batching/concurrency) — only after a proven bottleneck.

Non-goals:
- Changing the default embedder (stays `hash`; openai is opt-in).
- Building a new metrics framework (the `relevance` harness MUST be reused).
- In-CI OpenAI A/B (key + network + spend + nondeterminism — local/manual only).
- Re-chunking inside quanta-index (chunking is upstream in codegraph).

## 5. Design — phased work spec

### Phase 0 — measurement-independent correctness (ship now)

**P0-1 Cache text-dedup** — `crates/quanta-index-embed/src/cache.rs`
- Change `CachingEmbeddingProvider::embed_batch` to dedup `miss_texts` by content before the inner call, then fan each returned vector back to **all** positions sharing that text.
- MUST preserve input-order output and the existing count/dim fail-closed checks.
- Tests (owner-local): duplicate texts in one batch ⇒ inner provider asked **once** per distinct text (assert via a counting inner); output still aligns per position; mixed cached/dup/fresh case.

**P0-2 Backoff jitter** — `crates/quanta-index-embed/src/openai.rs`
- `backoff_delay(attempt)` MUST return a value randomized in `[0, RETRY_BASE_DELAY * 2^attempt]` (full jitter), saturating, no `as`/overflow (deny-lints).
- Randomness MUST NOT break determinism elsewhere; confine to retry timing only.
- Tests: jitter stays within the exponential bound across attempts; attempt 0 bound is `RETRY_BASE_DELAY`. (Retry *behavior* — recover-after-429, exhaustion — already covered.)

Acceptance: `cargo test -p quanta-index-embed` green; no behavior change for the no-duplicate, no-retry path.

### Phase 1 — semantic relevance gate (the unblocker)

**P1-1 Harness embedder-profile override** — `crates/quanta-index-searchd-harness` (`harness.rs` `build_config`/driver)
- Add a way to set `SemanticEmbedderProfile` on the spawned config (default stays `Hash`). Prereq for a deterministic semantic run and for later A/B.

**P1-2 Semantic route + projection** — `relevance/{corpus.rs,report.rs}`
- Add `RelevanceRoute::Semantic` (and `Hybrid`). In `produced_order`, project the semantic candidate `candidate_id` → repo-relative path (reuse `chunk_ids_by_path` / `candidate_id_for_path`), so chunk-id candidates score against **path-keyed** grades. MUST mirror the lexical projection; without it nDCG is uniformly 0.

**P1-3 Dedicated semantic fixture** — `relevance/corpus.rs`
- A small, NEW judged set (do NOT reuse the lexical corpus — its grades are BM25-calibrated). File-granular, with paraphrase/synonym intents where a token-overlap negative shares no vocabulary, so only a meaning-aware embedder ranks the on-topic file top.
- Runs on the **Hash** embedder in CI: deterministic, key-free. (Hash will likely score *poorly* on paraphrase — that is the honest baseline the openai A/B later beats; the gate asserts the harness mechanics + a deterministic floor, not that hash is good.)

Acceptance: `recall@k` / `nDCG@k` / `MRR` reported for the semantic route on the hash embedder, deterministic across runs, in CI without a key.

### Phase 2 — highest-leverage quality lever (start now, lands later)

**P2-1 Contextual chunk header** — UPSTREAM in `semantica-codegraph-v2` (index projection)
- Prepend a discriminative header (`repo_relative_path::symbol(signature)`) to the embedded chunk text so the embedder gets path/symbol tokens.
- Cross-repo lead time ⇒ start the codegraph-side change now; measure the gain against the Phase 1 fixture.

### Phase 3 — throughput tuning (only after measured bottleneck)

**P3-1 OpenAI A/B (local/manual, NOT CI)** — harness
- `#[ignore]` + `OPENAI_API_KEY`-gated (pattern: `openai.rs` real-API test). Report hash-vs-openai and dimension sweep (512 / 768 / 1536) on the Phase 1 fixture via generation-scoped isolation.

**P3-2 Cross-scope flatten + bounded concurrency** — `ingest_dispatcher.rs` derive + `quanta-index-embed`
- Flatten all batch chunk texts into `≤max_batch` (token-budget bounded) requests; bounded-parallel (`std::thread::scope`, N from a new `QUANTA_INDEX_EMBED_CONCURRENCY` knob, default 1).
- MUST land **after** P0-2 jitter and **only if** P3-1 shows ingest is embedding-throughput-bound. Redistribution MUST preserve per-chunk order under deny-lints (no indexing/arithmetic-side-effects).

## 6. Sequencing & rationale

`P0 → P1 → (P2 in parallel) → P3`. P0 are correctness/cost wins independent of any
measurement and ship immediately. P1 is the gate that makes P3 falsifiable —
throughput/quality tuning before a measurement is unjustifiable. P2 has long
cross-repo lead time so it starts in parallel. P3 is gated behind a measured
bottleneck.

## 7. Alternatives considered and REJECTED

- **Flatten-across-scopes first (original "B1").** Rejected: the cache already
  coalesces misses per call, so flatten's only gain is cross-call; worse, it
  *amplifies* the within-call duplicate-text cost. Superseded by P0-1 cache dedup,
  which is cheaper, helps all callers, and makes a future flatten safe.
- **Parallelism before jitter / before measurement (original "B2").** Rejected:
  N concurrent requests on a no-jitter backoff create synchronized retry storms on
  the first 429; and with no relevance/latency measurement there is no proven
  bottleneck. Deferred to P3-2 behind P0-2 + P3-1.
- **Reuse the lexical judged corpus for semantic eval.** Rejected: its grades are
  BM25-calibrated; bolting semantic on muddies the lexical gate. Use a dedicated
  semantic fixture (P1-3).
- **Chunk-id-keyed qrels.** Rejected: chunk-ids are opaque/engine-generated and
  unreviewable; keep path-keyed grades + a chunk-id→path projection (P1-2).
- **OpenAI A/B in CI.** Rejected: key + network + spend + nondeterministic
  vectors. Local/manual only (P3-1).

## 8. Risks & mitigations

- **Path-granular fixture cannot discriminate intra-file paraphrase.** Accept for
  the first gate (file-granularity already separates neural from lexical); a
  chunk-keyed sub-corpus is a later refinement, not a precondition.
- **Hash scores low on the semantic fixture.** Expected; the gate asserts harness
  mechanics + a deterministic floor + ordering invariants, not hash quality.
- **Concurrency lint pain (P3-2).** `std::thread::scope` + ordered redistribution
  under deny-lints; gated behind real need, jitter first.

## 9. Verification / acceptance

- Phase 0: `cargo test -p quanta-index-embed` green; dedup + jitter unit tests; no-dup/no-retry path byte-identical.
- Phase 1: semantic-route relevance report deterministic in CI on the hash embedder, no key; projection proven (nonzero, sane nDCG on the dedicated fixture).
- Phase 3-1: local `OPENAI_API_KEY=... cargo test ... -- --ignored` produces a hash-vs-openai metric delta.

## 10. Open questions

- Default `QUANTA_INDEX_EMBED_CONCURRENCY` and provider RPM ceiling (set when P3-1 measures throughput).
- Whether `Hybrid` route eval needs union (not lexical-scoped) recall to be meaningful — decide from P1 data.
