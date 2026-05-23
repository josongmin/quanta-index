#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

pub mod artifact_objects;
pub mod bundle_ingest;
pub mod error;
pub mod generation_registry;
pub mod ports;
pub mod query_serving;
pub mod services;

pub use artifact_objects::{
    PublishedSearchArtifactStorePort, SearchPlaneEmbeddingProviderPort,
    SearchPlaneLexicalIndexBuildPort, SearchPlaneLexicalIndexStorePort,
    SearchPlaneMetadataStorePort, SearchPlaneSemanticIndexBuildPort,
    SearchPlaneVectorIndexStorePort,
};
pub use bundle_ingest::{
    BundlePolicy, PublishedSearchBundleDeltaApplyPort, PublishedSearchBundlePreparePort,
};
pub use error::*;
pub use generation_registry::{
    ActivationPolicy, PublishedSearchActivationStatePort, PublishedSearchBundleInspectPort,
    PublishedSearchGenerationActivatePort, PublishedSearchGenerationCatalogPort,
    PublishedSearchGenerationReadinessPort,
};
pub use query_serving::{
    QueryPolicy, SearchPlaneExplainQueryPort, SearchPlaneHybridQueryPort,
    SearchPlaneLexicalQueryPort, SearchPlaneQueryContractPort, SearchPlaneQueryValidator,
    SearchPlaneSemanticQueryPort,
};
