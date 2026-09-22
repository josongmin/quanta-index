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
    IpcDispatcher, IpcError, IpcPlane, IpcServerCounters, RequestEnvelope, ResponseEnvelope,
    ServerAdmissionPolicy, ShutdownHandle as IpcShutdownHandle, SocketAccessPolicy, UdsServer,
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
    /// Which plane this server carries (S21-10): the dispatch context
    /// names it so a control-socket request can never claim another plane.
    plane: IpcPlane,
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
    /// Bind a server to `socket_path` under `policy` (QI-BB-002) and
    /// `access` (QI-BB-014), counting into `counters` (QI-BB-015).
    ///
    /// The three daemon sockets share one directory, held to
    /// `directory_access` — the widest of their policies — so each
    /// socket's admitted peers can reach it while the directory stays no
    /// wider than any of them needs.
    pub fn bind(
        plane: IpcPlane,
        thread_name: impl Into<String>,
        socket_path: &Path,
        dispatcher: Arc<D>,
        policy: ServerAdmissionPolicy,
        access: SocketAccessPolicy,
        directory_access: &SocketAccessPolicy,
        counters: Arc<IpcServerCounters>,
    ) -> Result<Self, IpcError> {
        let server =
            UdsServer::bind_observed_in(socket_path, policy, access, directory_access, counters)?;
        Ok(Self {
            server,
            dispatcher,
            plane,
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
            plane,
            thread_name,
            marker: _,
        } = self;
        let handle = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                server.run::<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
                    &dispatcher,
                    plane,
                    accept_idle,
                )
            })?;
        Ok(handle)
    }
}
