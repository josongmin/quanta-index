//! Semantic domain — owns vector index build, open, and query.

mod inbound;
mod outbound;
mod service;

pub use inbound::SemanticQueryPort;
pub use outbound::{
    SemanticBatchBuildPort, SemanticIndexOpenPort, SemanticIngestPort, SemanticReadiness,
    SemanticSearchHitV1, SemanticSearcher, TextEmbeddingProvider,
};
pub use service::{L2_UNIT_NORM_TOLERANCE, L2UnitEmbeddingProvider, SemanticPolicy};
