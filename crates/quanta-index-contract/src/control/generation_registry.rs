use crate::{PublishedGenerationSet, RepoId, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchGenerationActivateRequest {
    pub generation: PublishedGenerationSet,
    pub lexical_ready: bool,
    pub semantic_ready: bool,
    pub active_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchGenerationActivateResponse {
    pub activated: bool,
    pub active_generation: Option<PublishedGenerationSet>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchGenerationReadinessResponse {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub prepared_bundle_count: u64,
    pub active_generation: Option<PublishedGenerationSet>,
    pub lexical_ready: bool,
    pub semantic_ready: bool,
    pub mode: String,
    pub reason: Option<String>,
}
