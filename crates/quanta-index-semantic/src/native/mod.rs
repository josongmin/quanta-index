//! Native, owner-local exact vector materialization and bounded cosine oracle.
//! The implementation lives in `embedding` while the original public paths stay stable.

mod embedding;

pub use embedding::{
    EmbeddingInput, EmbeddingInputPolicy, EmbeddingProvider, EmbeddingRecipe, EmbeddingVector,
    NativeSemantic, ReadContext, SearchHit, SearchResult, SemanticCandidate, TextEmbeddingBridge,
    TicketSearchQuery, VectorNormalization, search_exact_acquired,
};
