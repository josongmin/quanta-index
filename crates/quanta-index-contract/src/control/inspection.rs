use crate::{BundleArtifactRef, PublishedGenerationSet, PublishedSearchBundleManifest};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleInspectRequest {
    pub generation: PublishedGenerationSet,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleInspectResponse {
    pub manifest: PublishedSearchBundleManifest,
    pub mode: String,
    pub artifacts: Vec<BundleArtifactRef>,
    pub state: String,
}
