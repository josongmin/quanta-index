# jun-24 wave2 — Embedding A/B hardening (batching · benchmark · metric honesty)

Handoff for the next session. Covers what landed in this wave and the **open
items / problems still to solve**, evidence-based and prioritized.

- Repo: `quanta-index` (search engine). Consumed by `semantica-codegraph-v2` over
  text-first IPC.
- Main at handoff: `origin/main = c80c68e` (all work below is merged; no unmerged
  local commits).
- Prior context: `docs/plans/jun-23-embedding-pipeline-sota/rfc.md`,
  `docs/plans/may-25-search-owned-semantic-derivation/`.

---

## What landed this wave (for context)

Real OpenAI semantic embeddings shipped earlier (opt-in; hash stays default). This
wave hardened the **OpenAI-vs-hash A/B** after its first run exposed weaknesses.

| Area | Commit | Summary |
|------|--------|---------|
| Corpus batching | `efd95ef` | `derive_semantic_batch_from_search_corpus_batch` now embeds ALL scopes of a batch in ONE `embed_batch` call (was one per scope ≈ one per file); draining-iter redistribution, fail-closed. Added `E2eRuntime::ingest_text_files_one_batch`. |
| A/B metric honesty | `2fa293f`, `3d4b925` | `on_topic_rank` is the headline discriminative signal; `recall@20` demoted to a coarse, honestly-labelled secondary. |
| Benchmark depth | `8ebfef4` | Paraphrase A/B expanded n=1 → 12 engineered pairs (synonym on-topic vs lexical-trap hard negative). |
| Concurrency (parallel work, by repo owner) | `0ed9486`, `3c43fc1`, `faea28a`, `a4fc8e2` | Bounded-concurrency dispatch (`QUANTA_INDEX_EMBED_CONCURRENCY`), per-thread jitter, sharded cache. Composes with the batching above. |

**Last measured A/B** (real API, 57-file fixture): corpus embedded in **1 batched
request** (provider-stats `57 texts → 14 http_requests`, the 13 remainder are
per-query); paraphrase **12/12 openai ranks the on-topic file higher than hash**
(hash drops it out of top-20 entirely in 5/12), top-1 openai 2 vs hash 0.

---

## Open items (do next), prioritized

### P1 — Client-side ingest batch granularity is the real bottleneck ⚠️
**The searchd-side batching only helps if the client sends multi-scope batches —
and it currently may not.**

- `efd95ef` batches across all scopes *within one received* `SearchCorpusIngestBatch`.
  But every batch constructor in this repo builds a **single-scope** batch:
  `crates/quanta-index-searchd-harness/src/harness.rs` `ingest_text_chunks`
  (`replace_scopes: vec![one]`), and the test/boot builders. The realistic
  multi-file batch only exists via the new `ingest_text_files_one_batch` test helper.
- Production batches arrive from **codegraph** via IPC `PublishSearchCorpusBatch`.
  If codegraph emits **one file per batch**, the searchd batching (and therefore
  the bounded-concurrency dispatch) never fire on real ingest — the A/B win would
  not reproduce in production.
- **Action**: in `semantica-codegraph-v2`, find where the searchd ingest client
  builds `SearchCorpusIngestBatch` and confirm/raise the scope count per batch
  (group N files per IPC batch, bounded by file count + a token budget). Without
  this, P1 of the A/B review is only half-solved.
- **Done when**: a production-shaped ingest of K files issues ≪ K embedding
  requests (telemetry `http_request_count`), not ~K.

### P2 — Load-scale measurement of batching × concurrency
- Only the 57-text fixture was measured. No thousands-of-chunks run to confirm the
  bounded-concurrency dispatch actually parallelizes the token-budget sub-requests,
  that the `DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST = 4096`
  (`crates/quanta-index-embed/src/openai.rs:19`) split behaves, and the wall-clock
  win is real.
- **Action**: a synthetic large-batch ingest bench (or a `--scale` flag on the A/B
  bin) reporting requests, wall-clock, and max concurrent in-flight. Pair with P1.

### P3 — Cache effectiveness is unproven (cache_hits always 0)
- Every A/B run boots a fresh temp state root, so `FileEmbeddingCache` reports
  `cache_hits = 0`. The cache's whole purpose — skip re-embedding unchanged chunks
  on rebuild/incremental — is never exercised end-to-end.
- **Action**: a rebuild-scenario test: ingest a corpus, re-ingest the same corpus
  under the same model, assert `cache_hits > 0` and **zero** new HTTP requests; plus
  a model-swap case asserting a full miss (no stale reuse).

### P4 — Benchmark depth + top-1 rate + artifact history
- 12 paraphrase pairs is a real signal, but on-topic files are short synonym stubs
  and openai reaches **top-1 only 2/12** (it ranks higher 12/12). Richer, realistic
  multi-line content + more intent categories + a held-out split would strengthen
  the claim.
- A/B artifacts overwrite `artifacts/search-quality/relevance/openai-ab/latest/`
  (gitignored). No dated archive → no delta-over-time / regression tracking of the
  hash-vs-openai gap.
- **Action**: deepen the fixture; archive dated runs + a small drift check.

### P5 — Query embeddings are one-at-a-time
- The A/B issues one request per query (13 queries → 13 requests). Fine for
  interactive search (queries arrive singly), but bulk eval re-embeds every query
  each run. Consider caching query-vectors in the A/B bin (the corpus cache already
  exists) or batching eval queries. Low priority.

---

## Accepted trade-offs (NOT bugs — documented so they are not "re-discovered")

- **Token estimate heuristic** (`openai.rs`, byte-based, capped at 4096 tok/req):
  intentionally conservative to never exceed the API per-request token limit; the
  bounded-concurrency dispatch absorbs any over-splitting latency. Do not "tighten"
  it with a heavy tokenizer dep (tiktoken) unless over-splitting is *measured* as a
  cost problem. Correctness > performance here.
- **`recall@20` kept as a coarse secondary metric** — `on_topic_rank` is the
  headline. Honest by design (`3d4b925`); do not re-promote recall.
- **Hash stays the CI/default embedder**; OpenAI A/B is a key-gated, paid, local
  tool (`just rust-capture-quality-relevance-openai-ab`), never a blocking CI gate.

---

## Quick verifications to run first (cheap)

- Confirm the per-thread jitter (`3c43fc1`, `next_jitter_u64` thread-local at
  `crates/quanta-index-embed/src/openai.rs:596`) has a test proving de-correlation
  across threads; if absent, add one (the commit *claims* de-correlation).
- `just rust-capture-quality-relevance-openai-ab` (needs `OPENAI_API_KEY`) to
  reproduce the A/B; inspect `provider-stats.json` `http_request_count` vs
  `total_texts_observed` as the batching health metric.

## Deferred by the RFC (unchanged)

- Phase 2 **contextual chunk header** (prepend a short file/section context to each
  chunk before embedding) — upstream in codegraph, not this repo.

---

🤖 Generated with [Claude Code](https://claude.com/claude-code)
