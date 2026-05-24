//! RepoMap contract surface.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{ManifestGeneration, RepoId, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoMapSourceBundleV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    pub item_index_availability: String,
    pub graph_coverage_class: String,
    pub exactness_summary: String,
    pub entry_identities: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoMapActivateGenerationRequestV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoMapMutationAckV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoMapSnapshotMetaV1 {
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    pub item_index_availability: String,
    pub graph_coverage_class: String,
    pub exactness_summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct RepoMapFocusSubjectDtoV1 {
    pub subject_identity: String,
    pub subject_doc_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoMapEntryDtoV1 {
    pub subject_identity: String,
    pub subject_doc_type: String,
    pub subject_kind: String,
    pub owner_path: String,
    pub score: f32,
    pub final_score_millis: u32,
    pub included: bool,
    pub rank: u32,
    pub importance_score_millis: u32,
    pub utility_score_millis: u32,
    pub freshness_score_millis: u32,
    pub evidence_priority_millis: u32,
    pub token_budget_hint: u32,
    pub contributing_signals: BTreeMap<String, i64>,
    pub projection_evidence_kind: String,
    pub projection_authority_artifact_id: String,
    pub projection_authority_digest: String,
    pub projection_status: String,
    pub redaction_state: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RepoMapQueryRequestV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub query_text: String,
    pub top_k: u32,
    pub token_budget: u32,
    pub focus_subjects: Vec<RepoMapFocusSubjectDtoV1>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoMapQueryResponseV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_meta: RepoMapSnapshotMetaV1,
    pub entries: Vec<RepoMapEntryDtoV1>,
    pub dropped_entries_count: u32,
    pub drop_reason_codes: Vec<String>,
    pub degraded_reason_codes: Vec<String>,
}
