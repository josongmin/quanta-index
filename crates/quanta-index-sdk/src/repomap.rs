use quanta_index_contract::{
    RepoMapActivateGenerationRequest, RepoMapActivateGenerationRequestV2, RepoMapMutationAck,
    RepoMapPublishBundleRequestV2, RepoMapQueryRequest, RepoMapQueryResponse, RepoMapSourceBundle,
    RepoMapTerminalReceiptV2, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse,
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
            other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
            | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
            | SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                Err(SdkError::unexpected_response(
                    "repomap query response",
                    QuantaIndex::query_response_kind(&other),
                ))
            }
        }
    }

    /// Typed repo-map publish entry point backed by the crate-private
    /// namespace trait owner. See QI-NS-01.
    ///
    /// QI-INT-01: repo-map publish goes through the ingest IPC, not
    /// control. The control surface's `RepoMapIngest` variant has been
    /// removed.
    pub fn publish(&self, bundle: &RepoMapSourceBundle) -> Result<RepoMapMutationAck, SdkError> {
        <RepoMapNs as crate::NamespaceIngest>::publish(self.client, bundle)
    }

    pub fn publish_v2(
        &self,
        request: RepoMapPublishBundleRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, SdkError> {
        let response = self
            .client
            .dispatch_ingest(SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(request))?;
        match response {
            SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repomap V2 publish receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
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
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
                Err(SdkError::unexpected_response(
                    "repomap activate ack",
                    QuantaIndex::control_response_kind(&other),
                ))
            }
        }
    }

    pub fn activate_v2(
        &self,
        request: RepoMapActivateGenerationRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, SdkError> {
        let response = self
            .client
            .dispatch_control(SearchPlaneControlIpcRequest::RepoMapActivateV2(request))?;
        match response {
            SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(receipt) => Ok(receipt),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
                Err(SdkError::unexpected_response(
                    "repomap V2 activate receipt",
                    QuantaIndex::control_response_kind(&other),
                ))
            }
        }
    }
}

/// QI-NS-01: marker type for the built-in repo-map namespace.
///
/// `Batch` is the existing [`RepoMapSourceBundle`] — there is no separate
/// SDK-side builder type because the contract bundle itself is already the
/// typed graph snapshot handed off by the producer. `Receipt` is
/// [`RepoMapMutationAck`] rather than `BatchPublishReceipt` because the
/// repo-map publish is a one-shot bundle ingest, not a streamed batch
/// with a channel sequence range.
struct RepoMapNs;

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
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repomap receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }
}
