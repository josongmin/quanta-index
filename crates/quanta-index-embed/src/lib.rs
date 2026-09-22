//! Text embedding providers for the semantic search plane.
//!
//! The query/corpus embedder seam is [`quanta_index_core::TextEmbeddingProvider`].
//! This crate supplies a pinned local Model2Vec provider and an OpenAI-backed
//! implementation behind a **blocking** transport. The latter reuses the
//! in-tree `reqwest` + `rustls` stack (no new TLS dependency).

mod cache;
mod model2vec;
mod openai;
mod pool;
mod telemetry;

pub use cache::{
    CachingEmbeddingProvider, EmbeddingCache, EmbeddingCacheIdentityV1, EmbeddingCacheKey,
    EmbeddingCacheOpenReport, EmbeddingCacheRetentionPolicy, EmbeddingCacheStats,
    FileEmbeddingCache, InMemoryEmbeddingCache,
};
pub use model2vec::{
    POTION_CODE_DIMENSION, POTION_CODE_MODEL_ID, POTION_CODE_MODEL_REVISION,
    PotionCodeEmbeddingProvider,
};
pub use openai::{
    DEFAULT_CONCURRENCY, DEFAULT_MAX_BATCH, DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST,
    DEFAULT_MAX_RETRIES, DEFAULT_TIMEOUT, EmbeddingTransport, HttpResponse, MAX_CONCURRENCY,
    OpenAiEmbeddingProvider, OpenAiProviderConfig, ReqwestBlockingTransport,
};
pub use pool::{ProviderAttemptDrainReport, ProviderAttemptPool};
pub use telemetry::{
    OpenAiEmbedStatsSnapshot, OpenAiEmbedTelemetrySource, OpenAiRequestSample,
    REQUEST_SAMPLE_CAPACITY, reset_openai_embed_stats, snapshot_openai_embed_stats,
};
