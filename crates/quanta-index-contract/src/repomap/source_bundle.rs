use crate::{ManifestGeneration, RepoId, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepoMapDeltaActionV1 {
    Upsert,
    Delete,
    MarkStale,
    NoOp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapDeltaEnvelopeV1 {
    pub invalidation_unit: String,
    pub target_identity: String,
    pub operation: RepoMapDeltaActionV1,
    pub destructive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSourceBundleV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_artifact_id: String,
    pub authority_digest: String,
    pub item_index_availability: String,
    pub graph_coverage_class: String,
    pub redaction_state: String,
    pub delta: Option<RepoMapDeltaEnvelopeV1>,
    pub file_indices: Vec<String>,
    pub import_edges: Vec<String>,
    pub call_edges: Vec<String>,
    pub chunk_records: Vec<String>,
}
