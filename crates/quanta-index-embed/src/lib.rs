//! Network-backed text embedding providers for the semantic search plane.
//!
//! The query/corpus embedder seam is [`quanta_index_core::TextEmbeddingProvider`].
//! This crate supplies an OpenAI-backed implementation behind a **blocking**
//! transport so it satisfies the synchronous embedder trait without introducing
//! an async runtime into the search/ingest hot paths. The blocking HTTP client
//! reuses the in-tree `reqwest` + `rustls` stack (no new TLS dependency).

mod cache;
mod openai;

pub use cache::{
    CachingEmbeddingProvider, EmbeddingCache, FileEmbeddingCache, InMemoryEmbeddingCache,
};
pub use openai::{
    EmbeddingTransport, HttpResponse, OpenAiEmbeddingProvider, OpenAiProviderConfig,
    ReqwestBlockingTransport,
};
