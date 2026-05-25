use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use quanta_index_contract::{
    GenerationPin, GenerationSelector, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
};

use crate::{
    ConnectOptions, GenerationNamespace, LexicalNamespace, QueryTransport, RepoMapNamespace,
    SdkError, SearchNamespace, SemanticNamespace, SymbolNamespace, UdsControlTransport,
    UdsQueryTransport,
};
use crate::{ControlTransport, config::ResolvedConnectOptions};

struct QuantaIndexInner {
    state_root: Option<std::path::PathBuf>,
    query_transport: Arc<dyn QueryTransport>,
    control_transport: Arc<dyn ControlTransport>,
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
    pub fn repomap(&self) -> RepoMapNamespace<'_> {
        RepoMapNamespace::new(self)
    }

    #[must_use]
    pub fn generations(&self) -> GenerationNamespace<'_> {
        GenerationNamespace::new(self)
    }

    pub(crate) fn dispatch_query(
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
            payload => Ok(payload),
        }
    }

    pub(crate) fn dispatch_control(
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
            payload => Ok(payload),
        }
    }

    pub(crate) fn state_root(&self) -> Result<&Path, SdkError> {
        self.inner.state_root.as_deref().ok_or_else(|| {
            SdkError::Usage(
                "publish surface requires a resolved state root; connect via state root".to_string(),
            )
        })
    }

    pub(crate) fn selection_to_fields(
        selection: GenerationSelector,
    ) -> (Option<GenerationPin>, Option<GenerationSelector>) {
        match selection {
            GenerationSelector::Pinned(pin) => (Some(pin), None),
            active @ GenerationSelector::Active { .. } => (None, Some(active)),
        }
    }

    pub(crate) fn encode_cbor<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, SdkError> {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(value, &mut buf)
            .map_err(|err| SdkError::Serialization(format!("cbor encode failed: {err}")))?;
        Ok(buf)
    }

    fn from_resolved(resolved: ResolvedConnectOptions) -> Self {
        let query_transport = Arc::new(UdsQueryTransport::new(resolved.query_socket));
        let control_transport = Arc::new(UdsControlTransport::new(resolved.control_socket));
        Self {
            inner: Arc::new(QuantaIndexInner {
                state_root: resolved.state_root,
                query_transport,
                control_transport,
                next_request_id: AtomicU64::new(1),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_transports(
        state_root: Option<std::path::PathBuf>,
        query_transport: Arc<dyn QueryTransport>,
        control_transport: Arc<dyn ControlTransport>,
    ) -> Self {
        Self {
            inner: Arc::new(QuantaIndexInner {
                state_root,
                query_transport,
                control_transport,
                next_request_id: AtomicU64::new(1),
            }),
        }
    }

    fn next_request_id(&self) -> u64 {
        self.inner.next_request_id.fetch_add(1, Ordering::Relaxed)
    }
}
