//! Repo-map port test double and request builders.

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapDocType, RepoMapEntryDto, RepoMapExactnessSummary,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapQueryRequest,
    RepoMapQueryResponse, RepoMapRedactionState, RepoMapSnapshotMeta, RevisionId,
    SearchPlaneQueryIpcResponse,
};
use quanta_index_core::{
    CoreError, PinnedRepoMapSnapshot, RepoMapSnapshotAcquirePort, RepoMapSnapshotAcquireV1,
    RepoMapSnapshotEvidenceV1,
};

/// A recording acquire-port double: every acquisition pins one stub
/// snapshot whose evidence names the requested identity.
#[derive(Default)]
pub(crate) struct StubRepoMapSnapshotPort {
    pub(crate) acquired: std::sync::Mutex<Vec<RepoMapSnapshotEvidenceV1>>,
}

pub(crate) struct StubPinnedRepoMapSnapshot {
    evidence: RepoMapSnapshotEvidenceV1,
}

impl PinnedRepoMapSnapshot for StubPinnedRepoMapSnapshot {
    fn query(&self, request: RepoMapQueryRequest) -> Result<RepoMapQueryResponse, CoreError> {
        Ok(RepoMapQueryResponse {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            manifest_generation: request.manifest_generation,
            snapshot_meta: RepoMapSnapshotMeta {
                snapshot_id: "dispatch-snapshot".to_string(),
                projection_version: 1,
                authority_digest: "dispatch-digest".to_string(),
                item_index_availability: RepoMapItemIndexAvailability::Available,
                graph_coverage_class: RepoMapGraphCoverageClass::Full,
                exactness_summary: RepoMapExactnessSummary::Exact,
            },
            entries: vec![RepoMapEntryDto {
                subject_identity: "src/lib.rs::Owner".to_string(),
                subject_doc_type: RepoMapDocType::Symbol,
                subject_kind: "symbol".to_string(),
                owner_path: "src/lib.rs".to_string(),
                score: 1.0,
                final_score_millis: 1000,
                rank: 1,
                importance_score_millis: 900,
                utility_score_millis: 700,
                freshness_score_millis: 600,
                evidence_priority_millis: 500,
                token_budget_hint: 64,
                contributing_signals: std::collections::BTreeMap::new(),
                projection_evidence_kind: "ParserItemIndex".to_string(),
                projection_authority_artifact_id: "repo-map:dispatch:1".to_string(),
                projection_authority_digest: "d".repeat(64),
                projection_status: "Complete".to_string(),
                redaction_state: RepoMapRedactionState::Unredacted,
            }],
            dropped_entries_count: 0,
            drop_reason_codes: Vec::new(),
            degraded_reason_codes: Vec::new(),
        })
    }

    fn evidence(&self) -> &RepoMapSnapshotEvidenceV1 {
        &self.evidence
    }
}

impl RepoMapSnapshotAcquirePort for StubRepoMapSnapshotPort {
    fn acquire(
        &self,
        acquire: RepoMapSnapshotAcquireV1,
    ) -> Result<Box<dyn PinnedRepoMapSnapshot>, CoreError> {
        let evidence = RepoMapSnapshotEvidenceV1 {
            repo_id: acquire.repo_id.as_str().to_string(),
            revision_id: acquire.revision_id.as_str().to_string(),
            manifest_generation: acquire.manifest_generation.get(),
            candidate_commitment: format!(
                "stub-commitment-{}-{}",
                acquire.repo_id.as_str(),
                acquire.manifest_generation.get()
            ),
            activation_epoch: 1,
        };
        self.acquired
            .lock()
            .map_err(|_poisoned| {
                CoreError::Storage("repo map stub acquire log poisoned".to_string())
            })?
            .push(evidence.clone());
        Ok(Box::new(StubPinnedRepoMapSnapshot { evidence }))
    }
}

pub(crate) fn repo_map_request() -> RepoMapQueryRequest {
    RepoMapQueryRequest {
        repo_id: RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-map-ipc")
            .expect("static fixture ID satisfies canonical policy"),
        manifest_generation: ManifestGeneration::new(9),
        query_text: "dispatch owner".to_string(),
        top_k: 4,
        token_budget: 256,
        focus_subjects: vec![quanta_index_contract::RepoMapFocusSubjectDto {
            subject_identity: "src/lib.rs::Owner".to_string(),
            subject_doc_type: RepoMapDocType::Symbol,
        }],
    }
}

pub(crate) fn into_repo_map_query_response(
    response: SearchPlaneQueryIpcResponse,
) -> Result<RepoMapQueryResponse, Box<dyn std::error::Error>> {
    match response {
        SearchPlaneQueryIpcResponse::RepoMapQuery(response) => Ok(response),
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(format!("expected repo-map query response, got {other:?}").into())
        }
    }
}
