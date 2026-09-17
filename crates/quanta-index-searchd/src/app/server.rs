//! UDS query server wrapper that owns the [`UdsServer`] and runs it in a
//! background thread on demand.

use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use quanta_index_contract::{
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponse, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope,
};
use quanta_index_ipc::{
    IpcDispatcher, IpcError, IpcServerCounters, RequestEnvelope, ResponseEnvelope,
    ServerAdmissionPolicy, ShutdownHandle as IpcShutdownHandle, UdsServer,
};

/// Composed query server. Holds the bound [`UdsServer`] and a handle to the
/// dispatcher; spawning a serving thread is opt-in via [`Self::spawn`].
pub struct QueryServer<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>
where
    RequestEnvelopeT: RequestEnvelope<Request>,
    ResponseEnvelopeT: ResponseEnvelope<Response>,
    D: IpcDispatcher<Request, Response> + ?Sized,
{
    server: UdsServer,
    dispatcher: Arc<D>,
    thread_name: String,
    marker: PhantomData<fn(RequestEnvelopeT, Request, ResponseEnvelopeT, Response)>,
}

pub type SearchPlaneQueryServer<D> = QueryServer<
    SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponseEnvelope,
    SearchPlaneQueryIpcResponse,
    D,
>;

pub type SearchPlaneControlServer<D> = QueryServer<
    SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneControlIpcResponse,
    D,
>;

pub type SearchPlaneIngestServer<D> = QueryServer<
    SearchPlaneIngestIpcRequestEnvelope,
    SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponseEnvelope,
    SearchPlaneIngestIpcResponse,
    D,
>;

impl<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>
    QueryServer<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>
where
    RequestEnvelopeT: RequestEnvelope<Request>,
    ResponseEnvelopeT: ResponseEnvelope<Response>,
    D: IpcDispatcher<Request, Response> + ?Sized + 'static,
{
    /// Bind a server to `socket_path` under `policy` (QI-BB-002), counting
    /// into `counters` (QI-BB-015).
    pub fn bind(
        thread_name: impl Into<String>,
        socket_path: &Path,
        dispatcher: Arc<D>,
        policy: ServerAdmissionPolicy,
        counters: Arc<IpcServerCounters>,
    ) -> Result<Self, IpcError> {
        let server = UdsServer::bind_observed(socket_path, policy, counters)?;
        Ok(Self {
            server,
            dispatcher,
            thread_name: thread_name.into(),
            marker: PhantomData,
        })
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

    /// The admission policy this socket was bound under.
    #[must_use]
    pub const fn admission_policy(&self) -> ServerAdmissionPolicy {
        self.server.admission_policy()
    }

    /// Spawn a thread that runs the accept loop until shutdown is triggered.
    /// Returns the join handle for the caller to wait on.
    pub fn spawn(self, accept_idle: Duration) -> Result<JoinHandle<Result<(), IpcError>>> {
        let Self {
            server,
            dispatcher,
            thread_name,
            marker: _,
        } = self;
        let handle = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                server.run::<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
                    &dispatcher,
                    accept_idle,
                )
            })?;
        Ok(handle)
    }
}
