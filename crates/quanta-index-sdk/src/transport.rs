use std::path::{Path, PathBuf};

use quanta_index_contract::{
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponseEnvelope,
};
use quanta_index_ipc::send_request;

use crate::SdkError;

pub trait QueryTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, SdkError>;
}

pub trait ControlTransport: Send + Sync {
    fn send(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, SdkError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UdsQueryTransport {
    socket_path: PathBuf,
}

impl UdsQueryTransport {
    #[must_use]
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
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
pub struct UdsControlTransport {
    socket_path: PathBuf,
}

impl UdsControlTransport {
    #[must_use]
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
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
