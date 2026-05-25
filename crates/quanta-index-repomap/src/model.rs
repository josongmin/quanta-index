use std::collections::BTreeMap;

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapEntryDto, RepoMapSnapshotMeta, RevisionId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoMapEntryV1 {
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
    #[serde(default)]
    pub search_text: String,
    #[serde(default)]
    pub source_symbol_count: u32,
    #[serde(default)]
    pub source_chunk_token_total: u32,
    #[serde(default)]
    pub source_call_incoming_edges: u32,
    #[serde(default)]
    pub source_call_outgoing_edges: u32,
    #[serde(default)]
    pub source_import_incoming_edges: u32,
    #[serde(default)]
    pub source_import_outgoing_edges: u32,
}

impl RepoMapEntryV1 {
    #[must_use]
    pub fn to_dto(&self) -> RepoMapEntryDto {
        RepoMapEntryDto {
            subject_identity: self.subject_identity.clone(),
            subject_doc_type: self.subject_doc_type.clone(),
            subject_kind: self.subject_kind.clone(),
            owner_path: self.owner_path.clone(),
            score: self.score,
            final_score_millis: self.final_score_millis,
            included: self.included,
            rank: self.rank,
            importance_score_millis: self.importance_score_millis,
            utility_score_millis: self.utility_score_millis,
            freshness_score_millis: self.freshness_score_millis,
            evidence_priority_millis: self.evidence_priority_millis,
            token_budget_hint: self.token_budget_hint,
            contributing_signals: self.contributing_signals.clone(),
            projection_evidence_kind: self.projection_evidence_kind.clone(),
            projection_authority_artifact_id: self.projection_authority_artifact_id.clone(),
            projection_authority_digest: self.projection_authority_digest.clone(),
            projection_status: self.projection_status.clone(),
            redaction_state: self.redaction_state.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepoMapSnapshotV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_meta: RepoMapSnapshotMeta,
    pub entries: Vec<RepoMapEntryV1>,
}
