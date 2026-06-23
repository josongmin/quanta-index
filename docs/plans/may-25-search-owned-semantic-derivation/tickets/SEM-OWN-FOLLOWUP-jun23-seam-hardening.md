# SEM-OWN follow-up — jun-23 semantic-search seam hardening

Status: `partial` — 3 of 7 audit findings landed; 4 deferred with rationale.

A jun-23 seam audit of the semantic / hybrid search path produced 7 findings.
The high-value, bounded ones are LANDED; the rest are deferred here (honestly,
not silently dropped) with the reason each is not worth its current risk.

## Landed

- **① Hybrid explanation honesty** — the hybrid semantic lane runs
  `search_scoped(query_vector, lexical_ids)`, so it re-ranks lexical recall and
  cannot surface a semantic-only hit. The explanation no longer hardcodes
  `engines_touched=[Lexical,Semantic]` / `strategy="rrf"`; both derive from real
  per-lane hit counts (`rrf` only when both lanes contribute, else
  `lexical_only` / `empty`), and the planner trace exposes
  `semantic_scoped_to_lexical=true`. (`query_dispatcher.rs`)
- **② Query-time model-identity gate** — query time previously checked only the
  vector dimension. `QueryTextEmbedderPort` now exposes `model_id()/model_version()`,
  `SemanticSearcher` exposes `index_model_id()/index_model_version()` (backed by
  `LoadedGeneration` retaining the manifest model), and the semantic/hybrid/
  hybrid_seed paths reject a query whose embedder model differs from the indexed
  generation's model (`SEM_MODEL_MISMATCH`), after embed so `SEM_PROVIDER_UNAVAILABLE`
  still surfaces first. SSOT: `SEARCH_OWNED_SEMANTIC_MODEL_ID`. Wiring + ordering
  proven by dispatcher-level tests.
- **⑤ Non-finite cosine distance fail-closed** — `cosine_distance_to_score_v1`
  rejects a NaN/Inf lancedb distance (`SEM_INVALID_VECTOR`) instead of seeding a
  non-finite score into ranked output. (`semantic/src/search.rs`)

## Deferred (with rationale)

- **③ Provisioning asymmetry** — `QueryTextEmbedderMode::ProviderUnavailable`
  blocks queries while the corpus still indexes (a populated-but-unqueryable
  index). NOT a correctness bug: ② already makes such a query fail closed
  (`SEM_PROVIDER_UNAVAILABLE`), so it is silent-wrong-free, only wasteful. The fix
  threads the mode into the corpus materializer wiring (moderate-risk, ingest
  path); low value because `ProviderUnavailable` is an explicit test-only config.
  Do alongside ④.
- **④ Embedder seam unification** — corpus derivation calls the `hash_query_text`
  free fn directly (`ingest_dispatcher.rs:derive_embedding_record`) rather than a
  shared embedder port, and the port signature is sync/single-text. A real
  (neural/remote) swap wants one batched/async `TextEmbeddingProvider` port used
  by BOTH query and corpus. Deferred to the real SEM-OWN swap: ② already makes a
  query↔index model disagreement fail closed, so this is now a cleanliness/perf
  refactor, not a safety gap. Large blast radius (ingest path + trait signature).
- **⑥ Over-fetch floor cushion** — `over_fetch_top_k` floors to
  `MIN_INTERNAL_FETCH_K=100` with no multiplier over `top_k`, so for `top_k>=100`
  the per-lane cushion is zero, weakening RRF recall of mid-ranked cross-lane
  items. A `max(100, top_k*factor)` floor would help, but it CHANGES ranking
  results (behavioral) for marginal gain on large queries; not worth the
  regression surface without a labeled relevance fixture to prove the win.
- **⑦ Encode dimension in model_id** — `model_id` ("search-owned-hash-text-v1")
  is decoupled from `SEARCH_OWNED_SEMANTIC_DIMENSION`. Encoding the dim
  (`...-d{DIM}`) would tie them, but it CHANGES the persisted manifest model_id,
  making every existing index mismatch ② at query time — a breaking data-format
  migration. Defer to a dedicated migration. ② already fail-closes on a real
  dim change via `check_query_dim`, so this is honesty-metadata only.
