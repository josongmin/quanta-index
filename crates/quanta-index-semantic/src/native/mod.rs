//! Native, owner-local exact vector materialization and bounded cosine oracle.
//! The implementation lives in `embedding` while the original public paths stay stable.

#[cfg(test)]
mod ann_files;
#[cfg(test)]
mod ann_lance;
#[cfg(test)]
mod ann_store;

mod certified_chunks;
mod chunks;
mod embedding;

pub use certified_chunks::{CertifiedChunkPayload, encode_certified_manifest};
pub use chunks::{ChunkLocation, ChunkMember, Utf8ChunkPolicy};

pub use embedding::{
    EmbeddingInput, EmbeddingInputPolicy, EmbeddingOutput, EmbeddingPrior, EmbeddingProvider,
    EmbeddingRecipe, EmbeddingVector, MAX_SEMANTIC_OVERLAY_DOCUMENTS, NativeSemantic,
    NativeSemanticReadConfig, NativeSemanticReader, ReadContext, SearchHit, SearchResult,
    SemanticCandidate, SemanticLexicalEligibility, SemanticLexicalEligibleSource,
    SemanticOverlayChunk, SemanticOverlayDocument, SemanticOverlayHit, SemanticReadRequest,
    SemanticSourceFile, SemanticSourceFileHitParts, TextEmbeddingBridge, TicketSearchQuery,
    VectorNormalization, chunk_owner_memory_envelope, max_chunk_owner_artifact_json_bytes,
    search_exact_acquired, validate_semantic_recipe_instance,
};
