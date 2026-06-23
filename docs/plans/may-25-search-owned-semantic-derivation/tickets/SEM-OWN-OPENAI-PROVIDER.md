# SEM-OWN — OpenAI embedding provider (real neural embeddings)

Status: `landed` (branch `sem-openai-embeddings`, opt-in; hash stays default).

The semantic plane shipped with an FNV-1a token-distribution hash embedder, which
ranks by token overlap and cannot capture meaning (synonyms / paraphrase). This
wave adds a real, network-backed OpenAI embedder so semantic search retrieves by
meaning. It is **opt-in** and does not change the default behavior.

## What landed

- **Phase 0 — unified embedder seam.** One core trait
  `TextEmbeddingProvider { embed_batch, model_id, model_version, dimension }` now
  serves BOTH the query path and corpus derivation (corpus previously bypassed the
  port and called the hash free fn per-chunk). Behavior-preserving for hash.
  *(Resolves the deferred `④` seam-unification item.)*
- **Phase 1 — config profile + dimension SSOT.** `SemanticEmbedderProfile`
  ({ `Hash { dimension }`, `OpenAi { model, dimension, api_key }`, `Unavailable` })
  drives both query and corpus from one source; dimension flows from the profile.
- **Phase 2 — OpenAI provider.** `quanta-index-embed::OpenAiEmbeddingProvider`
  over a blocking transport (reuses the in-tree `reqwest`/`rustls`; no new TLS
  dep, no openssl per deny.toml). Batched `/v1/embeddings`; reorders by index;
  validates count + per-vector dim; bounded backoff retry on 429/5xx/transport;
  typed fail-closed `SEM_PROVIDER_AUTH` / `SEM_PROVIDER_TRANSPORT`. API key held
  only in the provider, redacted in `Debug`.
- **Phase 3 — persistent embedding cache.** `CachingEmbeddingProvider` +
  `FileEmbeddingCache` (key = SHA-256 over model_id+dimension+text) avoids paid
  re-embedding of unchanged chunks on rebuild/incremental. Model+dim scoped, so a
  model swap never reuses a stale vector.

## Enabling it

```
QUANTA_INDEX_EMBEDDER=openai            # default: hash
OPENAI_API_KEY=<key>                    # required for openai; never logged
QUANTA_INDEX_EMBED_MODEL=text-embedding-3-small   # default
QUANTA_INDEX_EMBED_DIM=1536                        # default

# Operational knobs (optional; unset => provider defaults). Resolved by the
# daemon and threaded into the provider/cache at the composition root.
QUANTA_INDEX_EMBED_BATCH=256            # max inputs per /v1/embeddings request (>=1)
QUANTA_INDEX_EMBED_MAX_RETRIES=3        # bounded retries on 429/5xx/transport (0 = none)
QUANTA_INDEX_EMBED_TIMEOUT_SECS=60      # per-request HTTP timeout in seconds (>=1)
QUANTA_INDEX_EMBED_CACHE=true           # on-disk embedding cache; true|false|on|off|1|0
```
Real semantic-relatedness proof (synonym closer than unrelated; impossible with
the hash embedder):
`OPENAI_API_KEY=<key> cargo test -p quanta-index-embed -- --ignored`.

## Migration (generation-scoped, no forced rebuild)

Generations are immutable and model-tagged: each `g{N}` carries its own
`SemanticManifest` (model_id / model_version / dimension) and lancedb table.

1. Switch the daemon to `QUANTA_INDEX_EMBEDDER=openai` and re-ingest the corpus.
   The new sealed generation is tagged `openai:<model>@<dim>` (e.g.
   `openai:text-embedding-3-small@1536`); the lancedb schema dimension is taken
   from the model contract, so 1536/3072 "just works".
2. Older hash generations remain queryable under the hash profile. The query-time
   model-identity gate (`ensure_query_model_matches_index_v1`) blocks querying an
   openai generation with a hash embedder (and vice versa) — `SEM_MODEL_MISMATCH`,
   never a silent garbage ranking.
3. There is no in-place conversion: a generation built with one model stays on
   that model. Roll forward by building a fresh generation.

## Remaining (deferred, see SEM-OWN-FOLLOWUP-jun23)

- `③` provisioning asymmetry: `Unavailable` keeps the deliberate degraded
  contract (corpus hash-derives, query fails closed) — intentionally unchanged.
- `⑥` over-fetch floor cushion — behavioral RRF tuning, separate.
- `⑦` model_id↔dimension binding — the openai model_id is now
  `openai:<model>`; encoding the dimension into the persisted id remains a
  format-migration decision.
- codegraph SRCH-02 casectl: a real synonym discriminator requires running
  searchd under the openai profile with a live key, so it is a key-gated
  follow-up rather than a CI test.
