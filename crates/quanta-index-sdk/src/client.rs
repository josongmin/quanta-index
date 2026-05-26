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
    ConnectOptions, GenerationNamespace, HistoryNamespace, LexicalNamespace, QueryTransport,
    RepoMapNamespace, RuntimeNamespace, SdkError, SearchNamespace, SemanticNamespace,
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
        let (_state_root, query_socket, control_socket, ingest_socket) = options.resolve()?;
        Ok(Self::from_resolved(
            query_socket,
            control_socket,
            ingest_socket,
        ))
    }

    #[must_use]
    pub fn lexical(&self) -> LexicalNamespace<'_> {
        LexicalNamespace::new(self)
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
            }),
            payload @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
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
            }),
            payload @ (SearchPlaneControlIpcResponse::ActivationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => Ok(payload),
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
            }),
            payload @ (SearchPlaneIngestIpcResponse::LexicalReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::StructuralReceipt(
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
            SearchPlaneQueryIpcResponse::History(_) => "history",
            SearchPlaneQueryIpcResponse::Structural(_) => "structural",
            SearchPlaneQueryIpcResponse::Bridge(_) => "bridge",
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => "repomap",
            SearchPlaneQueryIpcResponse::Explain(_) => "explain",
            SearchPlaneQueryIpcResponse::Error(_) => "error",
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => "runtime_metadata",
        }
    }

    pub(super) const fn control_response_kind(
        response: &SearchPlaneControlIpcResponse,
    ) -> &'static str {
        match response {
            SearchPlaneControlIpcResponse::ActivationAck(_) => "activation_ack",
            SearchPlaneControlIpcResponse::RepoMapMutationAck(_) => "repomap_mutation_ack",
            SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_) => {
                "current_generation_snapshot"
            }
            SearchPlaneControlIpcResponse::GenerationStatusReport(_) => "generation_status_report",
            SearchPlaneControlIpcResponse::Error(_) => "error",
        }
    }

    pub(super) const fn ingest_response_kind(
        response: &SearchPlaneIngestIpcResponse,
    ) -> &'static str {
        match response {
            SearchPlaneIngestIpcResponse::LexicalReceipt(_) => "lexical_receipt",
            SearchPlaneIngestIpcResponse::RepoMapReceipt(_) => "repomap_receipt",
            SearchPlaneIngestIpcResponse::HistoryReceipt(_) => "history_receipt",
            SearchPlaneIngestIpcResponse::DirtyReceipt(_) => "dirty_receipt",
            SearchPlaneIngestIpcResponse::StructuralReceipt(_) => "structural_receipt",
            SearchPlaneIngestIpcResponse::Error(_) => "error",
        }
    }

    fn from_resolved(
        query_socket: std::path::PathBuf,
        control_socket: std::path::PathBuf,
        ingest_socket: std::path::PathBuf,
    ) -> Self {
        let query_transport = Arc::new(UdsQueryTransport::new(query_socket));
        let control_transport = Arc::new(UdsControlTransport::new(control_socket));
        let ingest_transport = Arc::new(UdsIngestTransport::new(ingest_socket));
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
