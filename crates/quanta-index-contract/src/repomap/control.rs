use crate::{ManifestGeneration, RepoId, RevisionId};

use super::RepoMapSourceBundleV1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapPrepareRequestV1 {
    pub bundle: RepoMapSourceBundleV1,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapActivateGenerationRequestV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
}
