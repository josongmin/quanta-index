#![expect(
    clippy::redundant_pub_crate,
    reason = "crate-private transports are shared through the SDK root and tests only"
)]

use std::path::{Path, PathBuf};

use quanta_index_contract::{
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponseEnvelope,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponseEnvelope,
};
use quanta_index_ipc::{
    ClientIoPolicy, ClientIpcTimingV1, send_request, send_request_observed,
};

use crate::SdkError;

pub(crate) trait QueryTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError>;

    fn send_observed(
        &self,
        _request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<(SearchPlaneQueryIpcResponseEnvelope, ClientIpcTimingV1), SdkError> {
        Err(SdkError::Protocol(
            "query transport does not support request-local client observation".to_string(),
        ))
    }
}

pub(crate) trait ControlTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UdsQueryTransport {
    inner: UdsTransport,
}

impl UdsQueryTransport {
    #[must_use]
    pub(crate) fn new(socket_path: impl Into<PathBuf>, io_policy: ClientIoPolicy) -> Self {
        Self {
            inner: UdsTransport::new(socket_path, io_policy),
        }
    }
}

impl QueryTransport for UdsQueryTransport {
    fn send(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError> {
        self.inner.send(&request)
    }

    fn send_observed(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<(SearchPlaneQueryIpcResponseEnvelope, ClientIpcTimingV1), SdkError> {
        self.inner.send_observed(&request)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UdsControlTransport {
    inner: UdsTransport,
}

impl UdsControlTransport {
    #[must_use]
    pub(crate) fn new(socket_path: impl Into<PathBuf>, io_policy: ClientIoPolicy) -> Self {
        Self {
            inner: UdsTransport::new(socket_path, io_policy),
        }
    }
}

impl ControlTransport for UdsControlTransport {
    fn send(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError> {
        self.inner.send(&request)
    }
}

/// QI-SDK-01: typed ingest transport.
///
/// The SDK's `publish()` paths route through this trait so the producer never
/// opens legacy channel publishers directly. `quanta-index-channel` is
/// intentionally absent from the SDK's dependency surface.
pub(crate) trait IngestTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, SdkError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UdsIngestTransport {
    inner: UdsTransport,
}

impl UdsIngestTransport {
    #[must_use]
    pub(crate) fn new(socket_path: impl Into<PathBuf>, io_policy: ClientIoPolicy) -> Self {
        Self {
            inner: UdsTransport::new(socket_path, io_policy),
        }
    }
}

impl IngestTransport for UdsIngestTransport {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, SdkError> {
        self.inner.send(&request)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UdsTransport {
    socket_path: PathBuf,
    io_policy: ClientIoPolicy,
}

impl UdsTransport {
    fn new(socket_path: impl Into<PathBuf>, io_policy: ClientIoPolicy) -> Self {
        Self {
            socket_path: socket_path.into(),
            io_policy,
        }
    }

    fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    fn send<Request, Response>(&self, request: &Request) -> Result<Response, SdkError>
    where
        Request: serde::Serialize,
        Response: serde::de::DeserializeOwned,
    {
        send_request(self.socket_path(), request, self.io_policy).map_err(SdkError::Transport)
    }

    fn send_observed<Request, Response>(
        &self,
        request: &Request,
    ) -> Result<(Response, ClientIpcTimingV1), SdkError>
    where
        Request: serde::Serialize,
        Response: serde::de::DeserializeOwned,
    {
        send_request_observed(self.socket_path(), request, self.io_policy)
            .map_err(SdkError::Transport)
    }
}
