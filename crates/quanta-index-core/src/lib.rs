#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Application core for the search-plane. Defines domain ports + policies. Driven
//! adapters (`quanta-index-channel`, `quanta-index-lexical`, `quanta-index-semantic`,
//! `quanta-index-ipc`) implement these ports; the composition root in
//! `quanta-index-searchd` wires them together.

pub mod domains;
pub mod error;

pub use error::CoreError;

pub use domains::channel::{ChannelDispatchPolicy, ChannelObserver};
pub use domains::hybrid::{ExplainQueryPort, HybridOrchestratorPolicy, HybridQueryPort};
pub use domains::lexical::{
    LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalPolicy, LexicalQueryPort, LexicalReadiness,
    LexicalSearcher,
};
pub use domains::repomap::{
    RepoMapBundleIngestPort, RepoMapGenerationActivatePort, RepoMapPolicy, RepoMapQueryPort,
    RepoMapService, RepoMapSnapshotReadPort,
};
pub use domains::semantic::{
    SemanticIndexBuildPort, SemanticIndexOpenPort, SemanticPolicy, SemanticQueryPort,
    SemanticReadiness, SemanticSearcher,
};
