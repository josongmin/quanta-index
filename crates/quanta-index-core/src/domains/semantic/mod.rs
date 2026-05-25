//! Semantic domain — owns vector index build, open, and query.

mod inbound;
mod outbound;
mod service;

pub use inbound::SemanticQueryPort;
pub use outbound::{
    SemanticIndexBuildPort, SemanticIndexOpenPort, SemanticIngestPort, SemanticReadiness,
    SemanticSearcher,
};
pub use service::SemanticPolicy;
