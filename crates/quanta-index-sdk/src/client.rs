use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use quanta_index_contract::{
    GenerationPin, GenerationSelector, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
};

use crate::{
    ConnectOptions, GenerationNamespace, HistoryNamespace, LexicalNamespace,
    ObservabilityNamespace, QuarantineNamespace, QueryTransport, RepoMapNamespace,
    RuntimeNamespace, SdkError, SearchCorpusNamespace, SearchNamespace, SemanticNamespace,
    StructuralNamespace, SymbolNamespace, UdsControlTransport, UdsIngestTransport,
    UdsQueryTransport,
};
use crate::{ControlTransport, IngestTransport};

struct QuantaIndexInner {
    query_transport: Arc<dyn QueryTransport>,
    control_transport: Arc<dyn ControlTransport>,
    ingest_transport: Arc<dyn IngestTransport>,
    next_request_id: AtomicU64,
}

#[derive(Clone)]
pub struct QuantaIndex {
    inner: Arc<QuantaIndexInner>,
}

impl QuantaIndex {
    pub fn connect(options: ConnectOptions) -> Result<Self, SdkError> {
        let resolved = options.resolve()?;
        Ok(Self::from_resolved(resolved))
    }

    #[must_use]
    pub fn lexical(&self) -> LexicalNamespace<'_> {
        LexicalNamespace::new(self)
    }

    #[must_use]
    pub fn search_corpus(&self) -> SearchCorpusNamespace<'_> {
        SearchCorpusNamespace::new(self)
    }

    #[must_use]
    pub fn symbol(&self) -> SymbolNamespace<'_> {
        SymbolNamespace::new(self)
    }

    #[must_use]
    pub fn semantic(&self) -> SemanticNamespace<'_> {
        SemanticNamespace::new(self)
    }

    #[must_use]
    pub fn search(&self) -> SearchNamespace<'_> {
        SearchNamespace::new(self)
    }

    #[must_use]
    pub fn history(&self) -> HistoryNamespace<'_> {
        HistoryNamespace::new(self)
    }

    #[must_use]
    pub fn runtime(&self) -> RuntimeNamespace<'_> {
        RuntimeNamespace::new(self)
    }

    #[must_use]
    pub fn structural(&self) -> StructuralNamespace<'_> {
        StructuralNamespace::new(self)
    }

    #[must_use]
    pub fn repomap(&self) -> RepoMapNamespace<'_> {
        RepoMapNamespace::new(self)
    }

    #[must_use]
    pub fn generations(&self) -> GenerationNamespace<'_> {
        GenerationNamespace::new(self)
    }

    #[must_use]
    pub fn observability(&self) -> ObservabilityNamespace<'_> {
        ObservabilityNamespace::new(self)
    }

    #[must_use]
    pub fn quarantine(&self) -> QuarantineNamespace<'_> {
        QuarantineNamespace::new(self)
    }

    #[must_use]
    pub fn reader(&self) -> ReaderClient<'_> {
        ReaderClient::new(self)
    }

    #[must_use]
    pub fn producer(&self) -> ProducerClient<'_> {
        ProducerClient::new(self)
    }

    #[must_use]
    pub fn control(&self) -> ControlClient<'_> {
        ControlClient::new(self)
    }

    /// Test-only generic namespace entry point used by SDK-local
    /// namespace conformance tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn ns<N>(&self) -> crate::namespace::NamespaceHandle<'_, N>
    where
        N: ?Sized,
    {
        crate::namespace::NamespaceHandle::new(self)
    }

    pub(super) fn dispatch_query(
        &self,
        payload: SearchPlaneQueryIpcRequest,
    ) -> Result<SearchPlaneQueryIpcResponse, SdkError> {
        let request_id = self.next_request_id();
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response = self.inner.query_transport.send(envelope)?;
        if response.request_id != request_id {
            return Err(SdkError::Protocol(format!(
                "query response request_id {} != request {}",
                response.request_id, request_id
            )));
        }
        match response.payload {
            SearchPlaneQueryIpcResponse::Error(error) => Err(SdkError::Remote {
                code: error.code,
                message: error.message,
                repair: error.repair,
            }),
            payload @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => Ok(payload),
        }
    }

    pub(super) fn dispatch_control(
        &self,
        payload: SearchPlaneControlIpcRequest,
    ) -> Result<SearchPlaneControlIpcResponse, SdkError> {
        let request_id = self.next_request_id();
        let envelope = SearchPlaneControlIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response = self.inner.control_transport.send(envelope)?;
        if response.request_id != request_id {
            return Err(SdkError::Protocol(format!(
                "control response request_id {} != request {}",
                response.request_id, request_id
            )));
        }
        match response.payload {
            SearchPlaneControlIpcResponse::Error(error) => Err(SdkError::Remote {
                code: error.code,
                message: error.message,
                repair: error.repair,
            }),
            payload @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)) => Ok(payload),
        }
    }

    /// QI-SDK-01: typed ingest dispatch. Returns the non-Error response
    /// variant on success; converts `SearchPlaneIngestIpcResponse::Error`
    /// into a typed [`SdkError::Remote`].
    pub(super) fn dispatch_ingest(
        &self,
        payload: SearchPlaneIngestIpcRequest,
    ) -> Result<SearchPlaneIngestIpcResponse, SdkError> {
        let request_id = self.next_request_id();
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response = self.inner.ingest_transport.send(envelope)?;
        if response.request_id != request_id {
            return Err(SdkError::Protocol(format!(
                "ingest response request_id {} != request {}",
                response.request_id, request_id
            )));
        }
        match response.payload {
            SearchPlaneIngestIpcResponse::Error(error) => Err(SdkError::Remote {
                code: error.code,
                message: error.message,
                repair: error.repair,
            }),
            payload @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(
                _,
            )
            | quanta_index_contract::SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::StructuralReceipt(
                _,
            )
            | quanta_index_contract::SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(
                _,
            )) => Ok(payload),
        }
    }

    pub(super) fn selection_to_fields(
        selection: GenerationSelector,
    ) -> (Option<GenerationPin>, Option<GenerationSelector>) {
        match selection {
            GenerationSelector::Pinned(pin) => (Some(pin), None),
            active @ GenerationSelector::Active { .. } => (None, Some(active)),
        }
    }

    pub(super) const fn query_response_kind(
        response: &SearchPlaneQueryIpcResponse,
    ) -> &'static str {
        match response {
            SearchPlaneQueryIpcResponse::Text(_) => "text",
            SearchPlaneQueryIpcResponse::Symbol(_) => "symbol",
            SearchPlaneQueryIpcResponse::Semantic(_) => "semantic",
            SearchPlaneQueryIpcResponse::Hybrid(_) => "hybrid",
            SearchPlaneQueryIpcResponse::HybridSeed(_) => "hybrid_seed",
            SearchPlaneQueryIpcResponse::History(_) => "history",
            SearchPlaneQueryIpcResponse::Structural(_) => "structural",
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => "repomap",
            SearchPlaneQueryIpcResponse::Explain(_) => "explain",
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
                "cluster_membership_batch_read"
            }
            SearchPlaneQueryIpcResponse::Error(_) => "error",
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => "runtime_metadata",
        }
    }

    pub(super) const fn control_response_kind(
        response: &SearchPlaneControlIpcResponse,
    ) -> &'static str {
        match response {
            SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_) => {
                "search_corpus_activation_cas_ack"
            }
            SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_) => {
                "search_corpus_rollback_cas_ack"
            }
            SearchPlaneControlIpcResponse::RepoMapMutationAck(_) => "repomap_mutation_ack",
            SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_) => {
                "current_generation_snapshot"
            }
            SearchPlaneControlIpcResponse::GenerationStatusReport(_) => "generation_status_report",
            SearchPlaneControlIpcResponse::MetricsSnapshot(_) => "metrics_snapshot",
            SearchPlaneControlIpcResponse::QuarantineInventory(_) => "quarantine_inventory",
            SearchPlaneControlIpcResponse::QuarantineDiscardAck(_) => "quarantine_discard_ack",
            SearchPlaneControlIpcResponse::Error(_) => "error",
        }
    }

    pub(super) const fn ingest_response_kind(
        response: &SearchPlaneIngestIpcResponse,
    ) -> &'static str {
        match response {
            SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_) => "search_corpus_receipt",
            SearchPlaneIngestIpcResponse::RepoMapReceipt(_) => "repomap_receipt",
            SearchPlaneIngestIpcResponse::HistoryReceipt(_) => "history_receipt",
            SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_) => {
                "repo_commit_recency_receipt"
            }
            SearchPlaneIngestIpcResponse::RepoTopicReceipt(_) => "repo_topic_receipt",
            SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_) => "file_ownership_receipt",
            SearchPlaneIngestIpcResponse::FileContributorReceipt(_) => "file_contributor_receipt",
            SearchPlaneIngestIpcResponse::RepoMetaReceipt(_) => "repo_meta_receipt",
            SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => "repo_description_receipt",
            SearchPlaneIngestIpcResponse::DirtyReceipt(_) => "dirty_receipt",
            SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_) => "runtime_catalog_receipt",
            SearchPlaneIngestIpcResponse::StructuralReceipt(_) => "structural_receipt",
            SearchPlaneIngestIpcResponse::Error(_) => "error",
        }
    }

    fn from_resolved(resolved: crate::config::ResolvedConnectOptions) -> Self {
        // `state_root` is resolved for config validation only; the client talks
        // to the daemon over sockets and never touches the state root itself.
        let query_transport =
            Arc::new(UdsQueryTransport::new(resolved.query_socket, resolved.io_policy));
        let control_transport =
            Arc::new(UdsControlTransport::new(resolved.control_socket, resolved.io_policy));
        let ingest_transport =
            Arc::new(UdsIngestTransport::new(resolved.ingest_socket, resolved.io_policy));
        Self {
            inner: Arc::new(QuantaIndexInner {
                query_transport,
                control_transport,
                ingest_transport,
                next_request_id: AtomicU64::new(1),
            }),
        }
    }

    #[cfg(test)]
    pub(super) fn from_transports(
        query_transport: Arc<dyn QueryTransport>,
        control_transport: Arc<dyn ControlTransport>,
        ingest_transport: Arc<dyn IngestTransport>,
    ) -> Self {
        Self {
            inner: Arc::new(QuantaIndexInner {
                query_transport,
                control_transport,
                ingest_transport,
                next_request_id: AtomicU64::new(1),
            }),
        }
    }

    fn next_request_id(&self) -> u64 {
        self.inner.next_request_id.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Clone, Copy)]
pub struct ReaderClient<'a> {
    client: &'a QuantaIndex,
}

impl<'a> ReaderClient<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn lexical(&self) -> crate::LexicalQueryBuilder<'a> {
        self.client.lexical().query()
    }

    pub fn lexical_request(
        &self,
        request: quanta_index_contract::TextQueryRequest,
    ) -> Result<quanta_index_contract::TextQueryResponse, SdkError> {
        self.client.lexical().query_request(request)
    }

    #[must_use]
    pub fn symbol(&self) -> crate::SymbolQueryBuilder<'a> {
        self.client.symbol().query()
    }

    pub fn symbol_request(
        &self,
        request: quanta_index_contract::SymbolQueryRequest,
    ) -> Result<quanta_index_contract::SymbolQueryResponse, SdkError> {
        self.client.symbol().query_request(request)
    }

    #[must_use]
    pub fn semantic(&self) -> crate::SemanticQueryBuilder<'a> {
        self.client.semantic().query()
    }

    pub fn semantic_request(
        &self,
        request: quanta_index_contract::SemanticQueryRequest,
    ) -> Result<quanta_index_contract::SemanticQueryResponse, SdkError> {
        self.client.semantic().query_request(request)
    }

    #[must_use]
    pub fn hybrid_seed(&self) -> crate::HybridSeedQueryBuilder<'a> {
        self.client.search().hybrid_seed()
    }

    pub fn hybrid_seed_request(
        &self,
        request: quanta_index_contract::HybridSeedQueryRequest,
    ) -> Result<quanta_index_contract::HybridSeedQueryResponse, SdkError> {
        self.client.search().hybrid_seed_request(request)
    }

    pub fn explain(
        &self,
        generation: quanta_index_contract::GenerationPin,
        candidate: quanta_index_contract::LexicalCandidate,
    ) -> Result<quanta_index_contract::SearchPlaneExplainQueryResponse, SdkError> {
        self.client.search().explain(generation, candidate)
    }

    #[must_use]
    pub fn history(&self) -> crate::HistoryQueryBuilder<'a> {
        self.client.history().query()
    }

    pub fn history_request(
        &self,
        request: quanta_index_contract::HistoryQueryRequest,
    ) -> Result<quanta_index_contract::SearchPlaneHistoryQueryResponse, SdkError> {
        self.client.history().query_request(request)
    }

    #[must_use]
    pub fn runtime(&self) -> crate::RuntimeQueryBuilder<'a> {
        self.client.runtime().query()
    }

    pub fn runtime_request(
        &self,
        request: quanta_index_contract::RuntimeMetadataQueryRequest,
    ) -> Result<quanta_index_contract::SearchPlaneRuntimeMetadataQueryResponse, SdkError> {
        self.client.runtime().query_request(request)
    }

    #[must_use]
    pub fn structural(&self) -> crate::StructuralQueryBuilder<'a> {
        self.client.structural().query()
    }

    pub fn structural_request(
        &self,
        request: quanta_index_contract::StructuralQueryRequest,
    ) -> Result<quanta_index_contract::SearchPlaneStructuralQueryResponse, SdkError> {
        self.client.structural().query_request(request)
    }

    pub fn repomap_query(
        &self,
        request: quanta_index_contract::RepoMapQueryRequest,
    ) -> Result<quanta_index_contract::RepoMapQueryResponse, SdkError> {
        self.client.repomap().query(request)
    }
}

#[derive(Clone, Copy)]
pub struct ProducerClient<'a> {
    client: &'a QuantaIndex,
}

impl<'a> ProducerClient<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    pub fn publish_search_corpus<const SEALED: bool>(
        &self,
        batch: &crate::SearchCorpusBatch<SEALED>,
    ) -> Result<crate::BatchReceipt, SdkError> {
        self.client.search_corpus().publish(batch)
    }

    pub fn publish_search_corpus_and_activate(
        &self,
        batch: &crate::SearchCorpusBatch,
        expected_active: Option<quanta_index_contract::SearchCorpusGenerationIdentityV1>,
    ) -> Result<
        (
            crate::BatchReceipt,
            quanta_index_contract::SearchPlaneSearchCorpusActivationCasAck,
        ),
        SdkError,
    > {
        self.client
            .search_corpus()
            .publish_and_activate(batch, expected_active)
    }

    pub fn publish_history(
        &self,
        batch: &crate::HistoryBatch,
    ) -> Result<crate::BatchReceipt, SdkError> {
        self.client.history().publish(batch)
    }

    pub fn publish_dirty(
        &self,
        batch: &crate::DirtyBatch,
    ) -> Result<crate::BatchReceipt, SdkError> {
        self.client.runtime().publish_dirty(batch)
    }

    pub fn publish_structural<const SEALED: bool>(
        &self,
        batch: &crate::StructuralBatch<SEALED>,
    ) -> Result<crate::BatchReceipt, SdkError> {
        self.client.structural().publish(batch)
    }

    pub fn publish_repomap(
        &self,
        bundle: &quanta_index_contract::RepoMapSourceBundle,
    ) -> Result<quanta_index_contract::RepoMapMutationAck, SdkError> {
        self.client.repomap().publish(bundle)
    }
}

#[derive(Clone, Copy)]
pub struct ControlClient<'a> {
    client: &'a QuantaIndex,
}

impl<'a> ControlClient<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    pub fn rollback(
        &self,
        request: quanta_index_contract::SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    ) -> Result<quanta_index_contract::SearchPlaneSearchCorpusRollbackCasAck, SdkError> {
        self.client.generations().rollback(request)
    }

    pub fn current(
        &self,
        repo_id: quanta_index_contract::RepoId,
        revision_id: quanta_index_contract::RevisionId,
        track: quanta_index_contract::SearchPlaneTrackKind,
    ) -> Result<quanta_index_contract::GenerationSnapshot, SdkError> {
        self.client
            .generations()
            .current(repo_id, revision_id, track)
    }

    pub fn status(
        &self,
        repo_id: quanta_index_contract::RepoId,
        revision_id: quanta_index_contract::RevisionId,
    ) -> Result<quanta_index_contract::GenerationStatusReport, SdkError> {
        self.client.generations().status(repo_id, revision_id)
    }

    pub fn activate_repomap(
        &self,
        request: quanta_index_contract::RepoMapActivateGenerationRequest,
    ) -> Result<quanta_index_contract::RepoMapMutationAck, SdkError> {
        self.client.repomap().activate(request)
    }

    /// The daemon's metrics snapshot (QI-BB-015).
    pub fn metrics_snapshot(&self) -> Result<quanta_index_contract::MetricsSnapshotV1, SdkError> {
        self.client.observability().metrics_snapshot()
    }

    /// What the daemon quarantines right now (QI-BB-026).
    pub fn quarantine_inventory(
        &self,
    ) -> Result<quanta_index_contract::QuarantineInventoryV1, SdkError> {
        self.client.quarantine().inventory()
    }

    /// Discard one quarantined entry as it was listed (QI-BB-026).
    pub fn discard_quarantined(
        &self,
        target: &quanta_index_contract::QuarantineTargetV1,
    ) -> Result<quanta_index_contract::QuarantineDiscardAck, SdkError> {
        self.client.quarantine().discard(target)
    }
}
