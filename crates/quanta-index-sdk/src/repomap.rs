use quanta_index_contract::{
    RepoMapActivateGenerationRequestV1, RepoMapMutationAckV1, RepoMapQueryRequestV1,
    RepoMapQueryResponseV1, RepoMapSourceBundleV1, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponse, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
};

use crate::{QuantaIndex, SdkError};

pub struct RepoMapNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> RepoMapNamespace<'a> {
    pub(crate) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    pub fn query(&self, request: RepoMapQueryRequestV1) -> Result<RepoMapQueryResponseV1, SdkError> {
        let response = self
            .client
            .dispatch_query(SearchPlaneQueryIpcRequest::RepoMapQuery(request))?;
        match response {
            SearchPlaneQueryIpcResponse::RepoMapQuery(result) => Ok(result),
            other => Err(SdkError::Protocol(format!(
                "expected repomap query response, got {other:?}"
            ))),
        }
    }

    pub fn publish(&self, bundle: RepoMapSourceBundleV1) -> Result<RepoMapMutationAckV1, SdkError> {
        let response = self
            .client
            .dispatch_control(SearchPlaneControlIpcRequest::RepoMapIngest(bundle))?;
        match response {
            SearchPlaneControlIpcResponse::RepoMapMutationAck(ack) => Ok(ack),
            other => Err(SdkError::Protocol(format!(
                "expected repomap ingest ack, got {other:?}"
            ))),
        }
    }

    pub fn activate(
        &self,
        request: RepoMapActivateGenerationRequestV1,
    ) -> Result<RepoMapMutationAckV1, SdkError> {
        let response = self
            .client
            .dispatch_control(SearchPlaneControlIpcRequest::RepoMapActivate(request))?;
        match response {
            SearchPlaneControlIpcResponse::RepoMapMutationAck(ack) => Ok(ack),
            other => Err(SdkError::Protocol(format!(
                "expected repomap activate ack, got {other:?}"
            ))),
        }
    }
}
