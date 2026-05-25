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
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                Err(SdkError::unexpected_response(
                    "repomap query response",
                    QuantaIndex::query_response_kind(&other),
                ))
            }
        }
    }

    /// Sugar for `client.ns::<RepoMapNs>().publish(bundle)`. See QI-NS-01.
    ///
    /// QI-INT-01: repo-map publish goes through the ingest IPC, not
    /// control. The control surface's `RepoMapIngest` variant has been
    /// removed.
    pub fn publish(&self, bundle: &RepoMapSourceBundle) -> Result<RepoMapMutationAck, SdkError> {
        <RepoMapNs as crate::NamespaceIngest>::publish(self.client, bundle)
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
                Err(SdkError::unexpected_response(
                    "repomap activate ack",
                    QuantaIndex::control_response_kind(&other),
                ))
            }
        }
    }
}

/// QI-NS-01: marker type for the built-in repo-map namespace.
///
/// `Batch` is the existing [`RepoMapSourceBundle`] — there is no separate
/// SDK-side builder type because the bundle is already a flat DTO
/// constructed by the producer materializer. `Receipt` is
/// [`RepoMapMutationAck`] rather than `BatchPublishReceipt` because the
/// repo-map publish is a one-shot bundle ingest, not a streamed batch
/// with a channel sequence range.
pub struct RepoMapNs;

impl crate::NamespaceIngest for RepoMapNs {
    type Batch = RepoMapSourceBundle;
    type Receipt = RepoMapMutationAck;

    fn publish(
        client: &QuantaIndex,
        bundle: &RepoMapSourceBundle,
    ) -> Result<RepoMapMutationAck, SdkError> {
        let response = client.dispatch_ingest(
            SearchPlaneIngestIpcRequest::PublishRepoMapBundle(bundle.clone()),
        )?;
        match response {
            SearchPlaneIngestIpcResponse::RepoMapReceipt(ack) => Ok(ack),
            other @ (SearchPlaneIngestIpcResponse::LexicalReceipt(_)
            | SearchPlaneIngestIpcResponse::SemanticReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repomap receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }
}
