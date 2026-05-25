use quanta_index_contract::{
    RepoMapActivateGenerationRequest, RepoMapMutationAck, RepoMapQueryRequest,
    RepoMapQueryResponse, RepoMapSourceBundle, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponse, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
};

use crate::{QuantaIndex, SdkError};

pub struct RepoMapNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> RepoMapNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    pub fn query(&self, request: RepoMapQueryRequest) -> Result<RepoMapQueryResponse, SdkError> {
        let response = self
            .client
            .dispatch_query(SearchPlaneQueryIpcRequest::RepoMapQuery(request))?;
        match response {
            SearchPlaneQueryIpcResponse::RepoMapQuery(result) => Ok(result),
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => Err(SdkError::Protocol(format!(
                "expected repomap query response, got {}",
                QuantaIndex::query_response_kind(&other)
            ))),
        }
    }

    /// QI-SDK-01: repo-map publish via typed ingest IPC. `RepoMap` ingest
    /// moves off the control socket to the new ingest socket so the
    /// producer-side surface is uniform across lexical / semantic / repomap
    /// (cf. plan QI-INT-01 / QI-RM-01 follow-ups). The control socket's
    /// `RepoMapIngest` variant remains for now until external consumers
    /// migrate.
    pub fn publish(&self, bundle: RepoMapSourceBundle) -> Result<RepoMapMutationAck, SdkError> {
        let response = self
            .client
            .dispatch_ingest(SearchPlaneIngestIpcRequest::PublishRepoMapBundle(bundle))?;
        match response {
            SearchPlaneIngestIpcResponse::RepoMapReceipt(ack) => Ok(ack),
            other @ (SearchPlaneIngestIpcResponse::LexicalReceipt(_)
            | SearchPlaneIngestIpcResponse::SemanticReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected repomap receipt, got {}",
                QuantaIndex::ingest_response_kind(&other)
            ))),
        }
    }

    pub fn activate(
        &self,
        request: RepoMapActivateGenerationRequest,
    ) -> Result<RepoMapMutationAck, SdkError> {
        let response = self
            .client
            .dispatch_control(SearchPlaneControlIpcRequest::RepoMapActivate(request))?;
        match response {
            SearchPlaneControlIpcResponse::RepoMapMutationAck(ack) => Ok(ack),
            other @ (SearchPlaneControlIpcResponse::ActivationAck(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(SdkError::Protocol(format!(
                    "expected repomap activate ack, got {}",
                    QuantaIndex::control_response_kind(&other)
                )))
            }
        }
    }
}
