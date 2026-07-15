use quanta_index_contract::{
    ipc::{
        CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport,
        GenerationStatusRequest, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
        SearchPlaneRollbackGenerationAck, SearchPlaneRollbackGenerationRequest,
    },
    RepoId, RevisionId, SearchPlaneTrackKind,
};

use crate::{QuantaIndex, SdkError};

/// Read and rollback administration for already-published generations.
///
/// New activation authority belongs exclusively to
/// [`crate::SearchCorpusNamespace::publish_and_activate`], whose input carries
/// one validated lexical + semantic composite identity.
pub struct GenerationNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> GenerationNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Apply an explicit semantic rollback guarded by the expected active
    /// generation and digest. This is separate from composite activation.
    pub fn rollback(
        &self,
        request: SearchPlaneRollbackGenerationRequest,
    ) -> Result<SearchPlaneRollbackGenerationAck, SdkError> {
        let response = self
            .client
            .dispatch_control(SearchPlaneControlIpcRequest::RollbackGeneration(request))?;
        match response {
            SearchPlaneControlIpcResponse::RollbackAck(ack) => Ok(ack),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected rollback ack, got {}",
                    QuantaIndex::control_response_kind(&other)
                )))
            }
        }
    }

    /// Look up the active generation for one `(repo, revision, track)` triple.
    pub fn current(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Result<GenerationSnapshot, SdkError> {
        let response =
            self.client
                .dispatch_control(SearchPlaneControlIpcRequest::CurrentGeneration(
                    CurrentGenerationRequest {
                        repo_id,
                        revision_id,
                        track,
                    },
                ))?;
        match response {
            SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(snapshot) => Ok(snapshot),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::RollbackAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected current generation snapshot, got {}",
                QuantaIndex::control_response_kind(&other)
            ))),
        }
    }

    /// Return every active track for one `(repo, revision)` pair.
    pub fn status(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> Result<GenerationStatusReport, SdkError> {
        let response =
            self.client
                .dispatch_control(SearchPlaneControlIpcRequest::GenerationStatus(
                    GenerationStatusRequest {
                        repo_id,
                        revision_id,
                    },
                ))?;
        match response {
            SearchPlaneControlIpcResponse::GenerationStatusReport(report) => Ok(report),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::RollbackAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected generation status report, got {}",
                QuantaIndex::control_response_kind(&other)
            ))),
        }
    }
}
