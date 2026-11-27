//! Native vector materialization, region search, and bounded exact scoring.
//! The implementation lives in `embedding` while the original public paths stay stable.

mod ann_files;
mod ann_lance;
mod ann_region_policy;
mod ann_session;
mod ann_store;

mod certified_chunks;
mod chunks;
mod embedding;
mod recipe;

pub use certified_chunks::{CertifiedChunkPayload, encode_certified_manifest};
pub use chunks::{ChunkLocation, ChunkMember, Utf8ChunkPolicy};
pub use recipe::{EmbeddingInputPolicy, EmbeddingRecipe, VectorNormalization};

pub use embedding::{
    EmbeddingInput, EmbeddingOutput, EmbeddingPrior, EmbeddingProvider, EmbeddingVector,
    MAX_SEMANTIC_OVERLAY_DOCUMENTS, NativeSemantic, NativeSemanticReadConfig, NativeSemanticReader,
    ReadContext, SearchHit, SearchResult, SemanticCandidate, SemanticLexicalEligibility,
    SemanticLexicalEligibleSource, SemanticOverlayChunk, SemanticOverlayDocument,
    SemanticOverlayHit, SemanticReadRequest, SemanticSourceFile, SemanticSourceFileHitParts,
    TextEmbeddingBridge, TicketSearchQuery, chunk_owner_memory_envelope,
    max_chunk_owner_artifact_json_bytes, require_lance_writable, search_exact_acquired,
    validate_semantic_recipe_instance,
};
