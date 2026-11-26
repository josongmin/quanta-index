//! Pure fixture helpers shared by Native and Legacy test callers.

use anyhow::Result as AnyResult;
use quanta_index_contract::{
    CapabilityStatusV1, ChunkRecord, OwnerDocKind, RawFallbackReasonV1, RepoId,
    SearchPlaneIngestIpcRequest, SemanticCorpusKindV1, SemanticSourceRecordV1,
    SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, SourceRoleV1,
};
use quanta_index_ipc::stamp_batch_digest_v1;

/// Build the typed semantic source scopes that accompany fixture chunks.
/// Lexical chunks alone are not an implicit semantic ingest request.
#[must_use]
pub fn semantic_source_scopes_for_chunk_records(
    batch_repo_id: &RepoId,
    records: &[ChunkRecord],
) -> Vec<SemanticSourceReplaceScopeV1> {
    records
        .iter()
        .map(|record| {
            let owner_id = record.chunk_id.as_str().to_string();
            let source_file = quanta_index_contract::SourceFileKey {
                source_repo_id: record.searchable_repo_id(batch_repo_id).clone(),
                repo_relative_path: record.repo_relative_path.clone(),
            };
            SemanticSourceReplaceScopeV1 {
                scope: SemanticSourceScopeKeyV1 {
                    corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                    owner_kind: OwnerDocKind::Chunk,
                    owner_id: owner_id.clone(),
                    source_file: source_file.clone(),
                },
                scope_digest: format!("e2e-harness:semantic-source:scope:{owner_id}"),
                sources: vec![SemanticSourceRecordV1 {
                    record_id: owner_id.clone(),
                    corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                    owner_kind: OwnerDocKind::Chunk,
                    owner_id: owner_id.clone(),
                    source_doc_id: owner_id.clone(),
                    parent_owner_id: Some(owner_id),
                    source_file,
                    language: Some(record.language.as_str().to_string()),
                    package: None,
                    symbol_kind: None,
                    visibility: None,
                    source_role: SourceRoleV1::RawFallbackText,
                    generated: false,
                    capability_status: CapabilityStatusV1::Degraded,
                    raw_fallback_reason: Some(
                        RawFallbackReasonV1::IntentNotRecoverableFromStructure,
                    ),
                    authority_digest: "e2e-harness:direct-text-source:v1".to_string(),
                    render_policy_digest: "e2e-harness:direct-text-source:v1".to_string(),
                    card_schema_version: 0,
                    text: record.text.to_string(),
                }],
                cluster_memberships: Vec::new(),
            }
        })
        .collect()
}

/// A temporary directory a daemon will accept as its state root or socket
/// directory: mode `0700`.
///
/// The daemon refuses a state root or socket directory wider than `0700`
/// (QI-BB-014), and a tempdir is created under the process umask, so every
/// test that points a daemon at a tempdir narrows it through here.
pub fn private_tempdir() -> std::io::Result<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// Stamp the canonical batch digest on whichever receipt-bearing batch
/// `payload` carries (QI-BB-032); a repo-map bundle names none.
pub fn stamped_ingest_request(
    payload: SearchPlaneIngestIpcRequest,
) -> AnyResult<SearchPlaneIngestIpcRequest> {
    fn stamped<B: quanta_index_core::IngestBatchBodyV1 + serde::Serialize>(
        mut batch: B,
    ) -> AnyResult<B> {
        stamp_batch_digest_v1(&mut batch)?;
        Ok(batch)
    }
    Ok(match payload {
        request @ (SearchPlaneIngestIpcRequest::StageSourcePublication(_)
        | SearchPlaneIngestIpcRequest::PublishStagedSourcePublication(_)
        | SearchPlaneIngestIpcRequest::DiscardSourcePublicationUpload(_)) => request,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) => {
            // A producer event is immutable. Transport stamping must not repair
            // a stale/tampered source commitment or create new source authority.
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishFileContributorBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(stamped(batch)?)
        }
    })
}
