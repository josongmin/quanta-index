//! Network-backed text embedding providers for the semantic search plane.
//!
//! The query/corpus embedder seam is [`quanta_index_core::TextEmbeddingProvider`].
//! This crate supplies an OpenAI-backed implementation behind a **blocking**
//! transport so it satisfies the synchronous embedder trait without introducing
//! an async runtime into the search/ingest hot paths. The blocking HTTP client
//! reuses the in-tree `reqwest` + `rustls` stack (no new TLS dependency).

mod cache;
mod openai;
mod telemetry;

pub use cache::{
    CachingEmbeddingProvider, EmbeddingCache, FileEmbeddingCache, InMemoryEmbeddingCache,
};
pub use openai::{
    DEFAULT_MAX_BATCH, DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST, DEFAULT_MAX_RETRIES,
    DEFAULT_TIMEOUT, EmbeddingTransport, HttpResponse, OpenAiEmbeddingProvider,
    OpenAiProviderConfig, ReqwestBlockingTransport,
};
pub use telemetry::{
    OpenAiEmbedStatsSnapshot, OpenAiRequestSample, reset_openai_embed_stats,
    snapshot_openai_embed_stats,
};
