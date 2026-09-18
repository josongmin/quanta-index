//! Semantic domain — owns vector index build, open, and query.

mod inbound;
mod outbound;
mod service;
mod stream;

pub use inbound::SemanticQueryPort;
pub use outbound::{
    DenseIndexBuildV1, DenseIndexEffortV1, DenseIndexSegmentBuildV1, DenseIndexTrainingV1,
    DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1, EMBED_CHECKPOINT,
    SEMANTIC_ROW_ROOT_MISMATCH_CODE, SemanticContentRootsPort, SemanticIndexOpenPort,
    SemanticIngestPort, SemanticReadiness, SemanticSearchHitV1, SemanticSearcher,
    TextEmbeddingProvider,
};
pub use service::{L2_UNIT_NORM_TOLERANCE, L2UnitEmbeddingProvider, SemanticPolicy};
pub use stream::{
    ResidentScopeSource, SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE,
    SEMANTIC_STREAM_WINDOW_EXCEEDED_CODE, SEMANTIC_STREAM_WINDOW_SCOPES,
    SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES,
    SemanticBatchIdentityV1, SemanticBatchMutationsV1, SemanticGenerationContractV1,
    SemanticIngestHeaderV1, SemanticScopeSource, SemanticScopeStreamBuildPort,
    SemanticScopeWindowV1, SemanticStreamTallyV1, SemanticStreamWindowPolicy, SemanticWindowFillV1,
    SemanticWindowIssuerV1, SemanticWindowLeaseV1, SemanticWindowPlacementV1,
    SemanticWindowResidencyV1, build_resident_semantic_batch_v1, owner_key_v1,
};
