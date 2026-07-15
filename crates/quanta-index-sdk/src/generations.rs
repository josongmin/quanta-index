use quanta_index_contract::{
    RepoId, RevisionId, SearchPlaneTrackKind,
    ipc::{
        CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport,
        GenerationStatusRequest, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
        SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneSearchCorpusRollbackCasAck,
    },
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

    /// Apply an explicit composite rollback guarded by the exact expected
    /// lexical plus semantic active identity.
    pub fn rollback(
        &self,
        request: SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    ) -> Result<SearchPlaneSearchCorpusRollbackCasAck, SdkError> {
        validate_composite_rollback_request_v1(&request)?;
        let expected_ack = SearchPlaneSearchCorpusRollbackCasAck {
            active: request.target.clone(),
            previous_sealed_active: request.expected_active.clone(),
        };
        let response = self.client.dispatch_control(
            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(request),
        )?;
        match response {
            SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(ack) => {
                validate_composite_rollback_ack_v1(&ack, &expected_ack)?;
                Ok(ack)
            }
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
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
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
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected generation status report, got {}",
                QuantaIndex::control_response_kind(&other)
            ))),
        }
    }
}

fn validate_composite_rollback_request_v1(
    request: &SearchPlaneRollbackSearchCorpusGenerationCasRequest,
) -> Result<(), SdkError> {
    request.validate_v1().map_err(|error| {
        SdkError::Protocol(format!("composite rollback request is invalid: {error}"))
    })
}

fn validate_composite_rollback_ack_v1(
    observed: &SearchPlaneSearchCorpusRollbackCasAck,
    expected: &SearchPlaneSearchCorpusRollbackCasAck,
) -> Result<(), SdkError> {
    if observed != expected {
        return Err(SdkError::Protocol(
            "composite rollback acknowledgement does not match the requested target and expected active identity"
                .to_string(),
        ));
    }
    Ok(())
}
