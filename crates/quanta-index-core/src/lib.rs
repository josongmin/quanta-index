#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

pub mod domains;
pub mod error;

pub use domains::bundle_ingest::{
    BundlePolicy, PublishedSearchBundleDeltaApplyPort, PublishedSearchBundlePreparePort,
};
pub use domains::generation::{
    ActivationPolicy, PublishedSearchActivationStatePort, PublishedSearchBundleInspectPort,
    PublishedSearchGenerationActivatePort, PublishedSearchGenerationCatalogPort,
    PublishedSearchGenerationReadinessPort,
};
pub use domains::materialization::{
    LexicalBuildInput, SearchPlaneLexicalIndexBuildPort, SearchPlaneLexicalIndexStorePort,
    SearchPlaneMetadataStorePort, SearchPlaneSemanticIndexBuildPort,
    SearchPlaneVectorIndexStorePort, SemanticBuildInput,
};
pub use domains::query::{
    GenerationPinPort, QueryPolicy, SearchPlaneExplainQueryPort, SearchPlaneHybridQueryPort,
    SearchPlaneLexicalQueryPort, SearchPlaneQueryContractPort, SearchPlaneQueryValidator,
    SearchPlaneSemanticQueryPort,
};
pub use error::*;
