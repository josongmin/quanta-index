use crate::{
    PreparedBundleOutbox, PublishedGenerationSet, RepoId, RevisionId, SearchBundleMutationDelta,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundlePrepareRequest {
    pub outbox: PreparedBundleOutbox,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundlePrepareResponse {
    pub accepted: bool,
    pub external_bundle_id: String,
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleDeltaApplyRequest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: PublishedGenerationSet,
    pub delta: SearchBundleMutationDelta,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleDeltaApplyResponse {
    pub applied: bool,
    pub indexed_generation: PublishedGenerationSet,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleNotify {
    pub outbox_id: String,
}
