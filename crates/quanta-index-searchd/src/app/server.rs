//! UDS query server wrapper that owns the [`UdsServer`] and runs it in a
//! background thread on demand.

use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use quanta_index_ipc::{IpcError, QueryDispatcher, ShutdownHandle as IpcShutdownHandle, UdsServer};

use crate::app::query::SearchPlaneDispatcher;

/// Composed query server. Holds the bound [`UdsServer`] and a handle to the
/// dispatcher; spawning a serving thread is opt-in via [`Self::spawn`].
pub struct QueryServer {
    server: UdsServer,
    dispatcher: Arc<SearchPlaneDispatcher>,
}

impl QueryServer {
    /// Bind a server to `socket_path`.
    pub fn bind(
        socket_path: &Path,
        dispatcher: Arc<SearchPlaneDispatcher>,
    ) -> Result<Self, IpcError> {
        let server = UdsServer::bind(socket_path)?;
        Ok(Self { server, dispatcher })
    }

    /// Path the listener is bound to.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        self.server.socket_path()
    }

    /// Acquire a shutdown handle without consuming the server.
    #[must_use]
    pub fn shutdown_handle(&self) -> IpcShutdownHandle {
        self.server.shutdown_handle()
    }

    /// Spawn a thread that runs the accept loop until shutdown is triggered.
    /// Returns the join handle for the caller to wait on.
    pub fn spawn(self, accept_idle: Duration) -> Result<JoinHandle<Result<(), IpcError>>> {
        let dispatcher: Arc<dyn QueryDispatcher> = self.dispatcher;
        let server = self.server;
        let handle = std::thread::Builder::new()
            .name("quanta-index-uds".to_string())
            .spawn(move || server.run(&dispatcher, accept_idle))?;
        Ok(handle)
    }
}
