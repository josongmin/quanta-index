use quanta_index_contract::{
    RepoId, RepoMapActivateGenerationRequestV2, RepoMapActiveHeadRequestV2,
    RepoMapExpectedActiveV2, RepoMapPublishBundleRequestV2, RepoMapQueryRequest,
    RepoMapQueryResponse, RepoMapTerminalReceiptV2, RevisionId, SearchPlaneControlIpcRequest,
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

    /// Resolve the catalog-owned head token before preparing an activation.
    pub fn active_head(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> Result<Option<RepoMapExpectedActiveV2>, SdkError> {
        let response =
            self.client
                .dispatch_control(SearchPlaneControlIpcRequest::RepoMapActiveHeadV2(
                    RepoMapActiveHeadRequestV2 {
                        repo_id,
                        revision_id,
                    },
                ))?;
        match response {
            SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(head) => Ok(head.active),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
                Err(SdkError::unexpected_response(
                    "repomap active head v2",
                    QuantaIndex::control_response_kind(&other),
                ))
            }
        }
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

    /// Publish through the sole content-bound `RepoMap` ingest contract.
    pub fn publish(
        &self,
        request: &RepoMapPublishBundleRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, SdkError> {
        <RepoMapNs as crate::NamespaceIngest>::publish(self.client, request)
    }

    pub fn activate(
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
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
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
/// The current namespace admits only content-bound requests and terminal
/// receipts that attest the request axes.
struct RepoMapNs;

impl crate::NamespaceIngest for RepoMapNs {
    type Batch = RepoMapPublishBundleRequestV2;
    type Receipt = RepoMapTerminalReceiptV2;

    fn publish(
        client: &QuantaIndex,
        request: &RepoMapPublishBundleRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, SdkError> {
        let response = client.dispatch_ingest(
            SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(request.clone()),
        )?;
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
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "repomap terminal receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }
}
