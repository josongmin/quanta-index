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
use quanta_index_ipc::send_request;

use crate::SdkError;

pub(crate) trait QueryTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError>;
}

pub(crate) trait ControlTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UdsQueryTransport {
    socket_path: PathBuf,
}

impl UdsQueryTransport {
    #[must_use]
    pub(crate) fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    #[must_use]
    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl QueryTransport for UdsQueryTransport {
    fn send(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError> {
        send_request(self.socket_path(), &request).map_err(SdkError::Transport)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UdsControlTransport {
    socket_path: PathBuf,
}

impl UdsControlTransport {
    #[must_use]
    pub(crate) fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    #[must_use]
    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl ControlTransport for UdsControlTransport {
    fn send(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError> {
        send_request(self.socket_path(), &request).map_err(SdkError::Transport)
    }
}

/// QI-SDK-01: typed ingest transport.
///
/// The SDK's `publish()` paths route through this trait so the producer never
/// opens a channel publisher directly. `quanta-index-channel` is
/// intentionally absent from the SDK's `Cargo.toml` dependency surface.
pub(crate) trait IngestTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, SdkError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UdsIngestTransport {
    socket_path: PathBuf,
}

impl UdsIngestTransport {
    #[must_use]
    pub(crate) fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    #[must_use]
    pub(crate) fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl IngestTransport for UdsIngestTransport {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, SdkError> {
        send_request(self.socket_path(), &request).map_err(SdkError::Transport)
    }
}
