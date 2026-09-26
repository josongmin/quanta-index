use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use quanta_index_contract::{
    CurrentGenerationRequest, GenerationPin, GenerationSelector, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneTrackKind,
};

use crate::binding::{
    ControlCallBinding, IngestCallBinding, QueryCallBinding, bind_control_response,
    bind_ingest_response, bind_query_response,
};
use crate::{
    ClientProfile, ConnectOptions, GenerationNamespace, HistoryNamespace, LexicalNamespace,
    ObservabilityNamespace, QuarantineNamespace, QueryTransport, RepoMapNamespace,
    RuntimeNamespace, SdkError, SearchCorpusNamespace, SearchNamespace, SemanticNamespace,
    StructuralNamespace, SymbolNamespace, UdsControlTransport, UdsIngestTransport,
    UdsQueryTransport,
};
use crate::{ControlTransport, IngestTransport};

struct QuantaIndexInner {
    query_transport: Arc<dyn QueryTransport>,
    /// `None` in the query-only profile (S21-07): least privilege, no
    /// dummy transport.
    control_transport: Option<Arc<dyn ControlTransport>>,
    ingest_transport: Option<Arc<dyn IngestTransport>>,
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

    /// Connect a query-only client (S21-07): no control or ingest
    /// transport is configured or required. Calling a control or ingest
    /// method on the result fails with a typed
    /// [`SdkError::PlaneUnavailable`] instead of fabricating a transport.
    pub fn connect_query_only(options: ConnectOptions) -> Result<Self, SdkError> {
        let resolved = options.resolve_profile(ClientProfile::QueryOnly)?;
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
        mut payload: SearchPlaneQueryIpcRequest,
    ) -> Result<SearchPlaneQueryIpcResponse, SdkError> {
        self.pin_active_query(&mut payload)?;
        let selected_lexical_pin = self.resolve_lexical_query_generation(&payload)?;
        let mut binding = QueryCallBinding::from_request(&payload);
        if let Some(pin) = selected_lexical_pin {
            binding = binding.with_resolved_lexical_generation(pin);
        }
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
        bind_query_response(&binding, &response.payload)?;
        match response.payload {
            SearchPlaneQueryIpcResponse::Error(error) => Err(SdkError::Remote {
                code: error.code,
                message: error.message,
                repair: error.repair,
            }),
            payload @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
            | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
            | SearchPlaneQueryIpcResponse::Text(_)
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

    fn resolve_lexical_query_generation(
        &self,
        request: &SearchPlaneQueryIpcRequest,
    ) -> Result<Option<GenerationPin>, SdkError> {
        let SearchPlaneQueryIpcRequest::Text(query) = request else {
            return Ok(None);
        };
        if !query.query_text.contains("rev:at.time") {
            return Ok(None);
        }
        let response = self.dispatch_query(
            SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(query.clone()),
        )?;
        let SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(pin) = response else {
            return Err(SdkError::Protocol(
                "lexical plan resolution did not return a generation pin".to_string(),
            ));
        };
        Ok(Some(pin))
    }

    /// Resolve an active selector on the query plane before submission.
    /// Keep `Active` alongside the explicit resolved pin: the server checks
    /// that the catalog still selects the same generation at dispatch time,
    /// while the SDK exact-binds the final response. Semantic reads also
    /// retain the catalog manifest-digest check in the acquired view.
    /// The query-only profile has no control transport.
    fn pin_active_selector(
        &self,
        generation: &mut Option<GenerationPin>,
        selector: &mut Option<GenerationSelector>,
        track: SearchPlaneTrackKind,
    ) -> Result<(), SdkError> {
        let Some(GenerationSelector::Active {
            repo_id,
            revision_id,
        }) = selector.as_ref()
        else {
            return Ok(());
        };
        let request = CurrentGenerationRequest {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        };
        let response =
            self.dispatch_query(SearchPlaneQueryIpcRequest::ResolveActiveGeneration(request))?;
        let SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(resolution) = response else {
            return Err(SdkError::Protocol(
                "active resolution did not return a generation snapshot".to_string(),
            ));
        };
        let snapshot = resolution.snapshot_v1().ok_or_else(|| {
            SdkError::Protocol("active resolution returned an unsupported track".to_string())
        })?;
        let activation_token = resolution.head.activation_token;
        let resolved = GenerationPin::new(
            snapshot.repo_id.clone(),
            snapshot.revision_id.clone(),
            snapshot.manifest_generation,
        );
        if generation.as_ref().is_some_and(|pin| pin != &resolved) {
            return Err(SdkError::Protocol(
                "explicit generation pin conflicts with active resolution".to_string(),
            ));
        }
        *generation = Some(resolved);
        *selector = Some(GenerationSelector::ResolvedActive {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            activation_token,
        });
        Ok(())
    }

    fn pin_active_query(&self, request: &mut SearchPlaneQueryIpcRequest) -> Result<(), SdkError> {
        match request {
            SearchPlaneQueryIpcRequest::Text(query) => self.pin_active_selector(
                &mut query.generation,
                &mut query.generation_selector,
                SearchPlaneTrackKind::Lexical,
            ),
            SearchPlaneQueryIpcRequest::Symbol(query) => self.pin_active_selector(
                &mut query.generation,
                &mut query.generation_selector,
                SearchPlaneTrackKind::Lexical,
            ),
            SearchPlaneQueryIpcRequest::Semantic(query) => {
                self.pin_active_selector(
                    &mut query.generation,
                    &mut query.generation_selector,
                    SearchPlaneTrackKind::Semantic,
                )?;
                if let Some(scope) = &mut query.lexical_scope {
                    self.pin_active_selector(
                        &mut scope.generation,
                        &mut scope.generation_selector,
                        SearchPlaneTrackKind::Lexical,
                    )?;
                }
                Ok(())
            }
            SearchPlaneQueryIpcRequest::Hybrid(query) => {
                self.pin_active_selector(
                    &mut query.text_query.generation,
                    &mut query.text_query.generation_selector,
                    SearchPlaneTrackKind::Lexical,
                )?;
                self.pin_active_selector(
                    &mut query.generation,
                    &mut query.generation_selector,
                    SearchPlaneTrackKind::Semantic,
                )
            }
            SearchPlaneQueryIpcRequest::HybridSeed(query) => {
                self.pin_active_selector(
                    &mut query.text_query.generation,
                    &mut query.text_query.generation_selector,
                    SearchPlaneTrackKind::Lexical,
                )?;
                self.pin_active_selector(
                    &mut query.generation,
                    &mut query.generation_selector,
                    SearchPlaneTrackKind::Semantic,
                )
            }
            SearchPlaneQueryIpcRequest::History(query) => self.pin_active_selector(
                &mut query.text_query.generation,
                &mut query.text_query.generation_selector,
                SearchPlaneTrackKind::Lexical,
            ),
            SearchPlaneQueryIpcRequest::RuntimeMetadata(query) => self.pin_active_selector(
                &mut query.text_query.generation,
                &mut query.text_query.generation_selector,
                SearchPlaneTrackKind::Lexical,
            ),
            SearchPlaneQueryIpcRequest::Structural(query) => {
                if matches!(
                    query.text_query.generation_selector,
                    Some(
                        GenerationSelector::Active { .. }
                            | GenerationSelector::ResolvedActive { .. }
                    )
                ) {
                    return Err(SdkError::Protocol(
                        "structural active generation is unsupported".to_string(),
                    ));
                }
                Ok(())
            }
            SearchPlaneQueryIpcRequest::ResolveActiveGeneration(_)
            | SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(_)
            | SearchPlaneQueryIpcRequest::RepoMapQuery(_)
            | SearchPlaneQueryIpcRequest::Explain(_)
            | SearchPlaneQueryIpcRequest::ClusterMembershipRead(_) => Ok(()),
        }
    }

    pub(super) fn dispatch_control(
        &self,
        payload: SearchPlaneControlIpcRequest,
    ) -> Result<SearchPlaneControlIpcResponse, SdkError> {
        let binding = ControlCallBinding::from_request(&payload);
        let control_transport = self
            .inner
            .control_transport
            .clone()
            .ok_or(SdkError::PlaneUnavailable { plane: "control" })?;
        let request_id = self.next_request_id();
        let envelope = SearchPlaneControlIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response = control_transport.send(envelope)?;
        if response.request_id != request_id {
            return Err(SdkError::Protocol(format!(
                "control response request_id {} != request {}",
                response.request_id, request_id
            )));
        }
        bind_control_response(&binding, &response.payload)?;
        match response.payload {
            SearchPlaneControlIpcResponse::Error(error) => Err(SdkError::Remote {
                code: error.code,
                message: error.message,
                repair: error.repair,
            }),
            payload @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)
            | SearchPlaneControlIpcResponse::ProcessRequestEventsV1(_)) => Ok(payload),
        }
    }

    /// QI-SDK-01: typed ingest dispatch. Returns the non-Error response
    /// variant on success; converts `SearchPlaneIngestIpcResponse::Error`
    /// into a typed [`SdkError::Remote`].
    pub(super) fn dispatch_ingest(
        &self,
        payload: SearchPlaneIngestIpcRequest,
    ) -> Result<SearchPlaneIngestIpcResponse, SdkError> {
        let binding = IngestCallBinding::from_request(&payload);
        let ingest_transport = self
            .inner
            .ingest_transport
            .clone()
            .ok_or(SdkError::PlaneUnavailable { plane: "ingest" })?;
        let request_id = self.next_request_id();
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response = ingest_transport.send(envelope)?;
        if response.request_id != request_id {
            return Err(SdkError::Protocol(format!(
                "ingest response request_id {} != request {}",
                response.request_id, request_id
            )));
        }
        bind_ingest_response(&binding, &response.payload)?;
        match response.payload {
            SearchPlaneIngestIpcResponse::Error(error) => Err(SdkError::Remote {
                code: error.code,
                message: error.message,
                repair: error.repair,
            }),
            payload @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
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
            active @ (GenerationSelector::Active { .. }
            | GenerationSelector::ResolvedActive { .. }) => (None, Some(active)),
        }
    }

    pub(super) const fn query_response_kind(
        response: &SearchPlaneQueryIpcResponse,
    ) -> &'static str {
        match response {
            SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_) => {
                "active_generation_snapshot"
            }
            SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_) => {
                "resolved_lexical_generation"
            }
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
            SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_) => {
                "repomap_terminal_receipt_v2"
            }
            SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_) => "repomap_active_head_v2",
            SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_) => {
                "current_generation_snapshot"
            }
            SearchPlaneControlIpcResponse::GenerationStatusReport(_) => "generation_status_report",
            SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_) => {
                "search_corpus_active_head_observation"
            }
            SearchPlaneControlIpcResponse::MetricsSnapshot(_) => "metrics_snapshot",
            SearchPlaneControlIpcResponse::QuarantineInventory(_) => "quarantine_inventory",
            SearchPlaneControlIpcResponse::QuarantineDiscardAck(_) => "quarantine_discard_ack",
            SearchPlaneControlIpcResponse::ProcessReadinessReport(_) => "process_readiness_report",
            SearchPlaneControlIpcResponse::ProcessRequestEventsV1(_) => "process_request_events_v1",
            SearchPlaneControlIpcResponse::Error(_) => "error",
        }
    }

    pub(super) const fn ingest_response_kind(
        response: &SearchPlaneIngestIpcResponse,
    ) -> &'static str {
        match response {
            SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_) => "search_corpus_receipt",
            SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_) => {
                "repomap_terminal_receipt_v2"
            }
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
        let query_transport = Arc::new(UdsQueryTransport::new(
            resolved.query_socket,
            resolved.io_policy,
        ));
        let control_transport =
            resolved
                .control_socket
                .map(|socket| -> Arc<dyn ControlTransport> {
                    Arc::new(UdsControlTransport::new(socket, resolved.io_policy))
                });
        let ingest_transport = resolved
            .ingest_socket
            .map(|socket| -> Arc<dyn IngestTransport> {
                Arc::new(UdsIngestTransport::new(socket, resolved.io_policy))
            });
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
                control_transport: Some(control_transport),
                ingest_transport: Some(ingest_transport),
                next_request_id: AtomicU64::new(1),
            }),
        }
    }

    fn next_request_id(&self) -> u64 {
        // W10-R2: the allocator never emits 0 — not at start (the
        // counter seeds at 1) and not at wrap (fetch_add past u64::MAX
        // yields 0 exactly once, skipped here). The server refuses 0
        // anyway, so skipping is belt and braces, never load-bearing.
        loop {
            let id = self.inner.next_request_id.fetch_add(1, Ordering::Relaxed);
            if id != 0 {
                return id;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn test_seed_next_request_id(&self, first: u64) {
        self.inner.next_request_id.store(first, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn test_next_request_id(&self) -> u64 {
        self.next_request_id()
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
        expected_active: Option<quanta_index_contract::SearchCorpusActiveHeadV1>,
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
        request: &quanta_index_contract::RepoMapPublishBundleRequestV2,
    ) -> Result<quanta_index_contract::RepoMapTerminalReceiptV2, SdkError> {
        self.client.repomap().publish(request)
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
        request: quanta_index_contract::RepoMapActivateGenerationRequestV2,
    ) -> Result<quanta_index_contract::RepoMapTerminalReceiptV2, SdkError> {
        self.client.repomap().activate(request)
    }

    /// The daemon's metrics snapshot (QI-BB-015).
    pub fn metrics_snapshot(&self) -> Result<quanta_index_contract::MetricsSnapshotV1, SdkError> {
        self.client.observability().metrics_snapshot()
    }

    /// Process-wide readiness, separate from per-repository generation status.
    pub fn process_readiness(&self) -> Result<quanta_index_contract::ProcessReadinessV1, SdkError> {
        self.client.observability().process_readiness()
    }

    /// Read a bounded request-event window; requires daemon-side Admin capability.
    pub fn request_events(
        &self,
        plane: quanta_index_contract::ProcessRequestEventPlaneV1,
        limit: u16,
    ) -> Result<quanta_index_contract::ProcessRequestEventsV1, SdkError> {
        self.client.observability().request_events(plane, limit)
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
