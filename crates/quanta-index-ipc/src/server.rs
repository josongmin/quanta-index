//! `AF_UNIX` stream server + client.
//!
//! The accept loop hands every connection to its own thread, bounded by the
//! server's [`ServerAdmissionPolicy`]; each connection decodes one envelope
//! per frame, takes a dispatch slot, routes through the [`IpcDispatcher`]
//! supplied by the composition root under a per-request
//! [`RequestBudgetV1`], and writes the response back on the same connection.
//! A peer that hangs up mid-dispatch cancels that request's budget.
//!
//! Connection-fatal failures (framing, oversized, CBOR decode) close the
//! connection without writing a response. Request-domain failures (e.g.
//! `NOT_READY`, `INVALID_REQUEST`), a full dispatch queue and an oversized
//! response flow through as an `Error` variant in the response envelope.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use quanta_index_core::RequestBudgetV1;

use crate::admission::{DispatchSlots, ServerAdmissionPolicy, SlotRefusal};

use quanta_index_contract::{
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponseEnvelope,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponseEnvelope,
};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
#[cfg(not(target_os = "linux"))]
use rustix::io::{ioctl_fioclex, ioctl_fionbio};
#[cfg(not(target_os = "linux"))]
use rustix::net::socket;
#[cfg(any(
    target_vendor = "apple",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
))]
use rustix::net::sockopt::set_socket_nosigpipe;
use rustix::net::{
    AddressFamily, RecvFlags, SendFlags, SocketAddrUnix, SocketType, connect, recv, send,
    sockopt::socket_error,
};
#[cfg(target_os = "linux")]
use rustix::net::{SocketFlags, socket_with};

use crate::codec::{
    IpcError, IpcIoOperation, MAX_FRAME_BODY_BYTES, decode_request, decode_response,
    encode_request, encode_response,
};

/// Dispatch hook supplied by the composition root.
///
/// Receives a fully-parsed request payload and returns a fully-typed response
/// payload. Domain errors MUST be surfaced through the response type rather
/// than panicking.
pub trait IpcDispatcher<Request, Response>: Send + Sync {
    /// Handle one request under its budget. The budget's deadline is the
    /// server's dispatch budget from admission; its cancellation fires if the
    /// peer disconnects while this call runs. Implementations check it at
    /// their own boundaries and answer with a typed interruption.
    fn dispatch(&self, request: Request, budget: &RequestBudgetV1) -> Response;
}

pub trait RequestEnvelope<Request>: serde::de::DeserializeOwned + Send + Sync + 'static {
    fn into_parts(self) -> (u64, Request);
}

pub trait ResponseEnvelope<Response>: serde::Serialize + Send + Sync + 'static {
    fn from_parts(request_id: u64, payload: Response) -> Self;

    /// The envelope to send when the real response encoded to `encoded_bytes`
    /// and does not fit under `limit_bytes` (QI-BB-005). `None` means the
    /// envelope type has no typed error payload and the connection closes,
    /// which is the only honest option for such a type.
    fn result_too_large(request_id: u64, encoded_bytes: u64, limit_bytes: u64) -> Option<Self>
    where
        Self: Sized;

    /// The envelope to send when no dispatch slot came free within the
    /// server's queue wait (QI-BB-002). `None` closes the connection.
    fn overloaded(request_id: u64, waited: Duration, slots: usize) -> Option<Self>
    where
        Self: Sized;
}

impl RequestEnvelope<quanta_index_contract::SearchPlaneQueryIpcRequest>
    for SearchPlaneQueryIpcRequestEnvelope
{
    fn into_parts(self) -> (u64, quanta_index_contract::SearchPlaneQueryIpcRequest) {
        (self.request_id, self.payload)
    }
}

impl ResponseEnvelope<quanta_index_contract::SearchPlaneQueryIpcResponse>
    for SearchPlaneQueryIpcResponseEnvelope
{
    fn from_parts(
        request_id: u64,
        payload: quanta_index_contract::SearchPlaneQueryIpcResponse,
    ) -> Self {
        Self {
            request_id,
            payload,
        }
    }

    fn result_too_large(request_id: u64, encoded_bytes: u64, limit_bytes: u64) -> Option<Self> {
        Some(Self {
            request_id,
            payload: quanta_index_contract::SearchPlaneQueryIpcResponse::Error(
                quanta_index_contract::SearchPlaneIpcError::result_too_large(
                    encoded_bytes,
                    limit_bytes,
                ),
            ),
        })
    }

    fn overloaded(request_id: u64, waited: Duration, slots: usize) -> Option<Self> {
        Some(Self {
            request_id,
            payload: quanta_index_contract::SearchPlaneQueryIpcResponse::Error(
                quanta_index_contract::SearchPlaneIpcError::overloaded(waited, slots),
            ),
        })
    }
}

impl RequestEnvelope<quanta_index_contract::SearchPlaneControlIpcRequest>
    for SearchPlaneControlIpcRequestEnvelope
{
    fn into_parts(self) -> (u64, quanta_index_contract::SearchPlaneControlIpcRequest) {
        (self.request_id, self.payload)
    }
}

impl ResponseEnvelope<quanta_index_contract::SearchPlaneControlIpcResponse>
    for SearchPlaneControlIpcResponseEnvelope
{
    fn from_parts(
        request_id: u64,
        payload: quanta_index_contract::SearchPlaneControlIpcResponse,
    ) -> Self {
        Self {
            request_id,
            payload,
        }
    }

    fn result_too_large(request_id: u64, encoded_bytes: u64, limit_bytes: u64) -> Option<Self> {
        Some(Self {
            request_id,
            payload: quanta_index_contract::SearchPlaneControlIpcResponse::Error(
                quanta_index_contract::SearchPlaneIpcError::result_too_large(
                    encoded_bytes,
                    limit_bytes,
                ),
            ),
        })
    }

    fn overloaded(request_id: u64, waited: Duration, slots: usize) -> Option<Self> {
        Some(Self {
            request_id,
            payload: quanta_index_contract::SearchPlaneControlIpcResponse::Error(
                quanta_index_contract::SearchPlaneIpcError::overloaded(waited, slots),
            ),
        })
    }
}

impl RequestEnvelope<quanta_index_contract::SearchPlaneIngestIpcRequest>
    for SearchPlaneIngestIpcRequestEnvelope
{
    fn into_parts(self) -> (u64, quanta_index_contract::SearchPlaneIngestIpcRequest) {
        (self.request_id, self.payload)
    }
}

impl ResponseEnvelope<quanta_index_contract::SearchPlaneIngestIpcResponse>
    for SearchPlaneIngestIpcResponseEnvelope
{
    fn from_parts(
        request_id: u64,
        payload: quanta_index_contract::SearchPlaneIngestIpcResponse,
    ) -> Self {
        Self {
            request_id,
            payload,
        }
    }

    fn result_too_large(request_id: u64, encoded_bytes: u64, limit_bytes: u64) -> Option<Self> {
        Some(Self {
            request_id,
            payload: quanta_index_contract::SearchPlaneIngestIpcResponse::Error(
                quanta_index_contract::SearchPlaneIpcError::result_too_large(
                    encoded_bytes,
                    limit_bytes,
                ),
            ),
        })
    }

    fn overloaded(request_id: u64, waited: Duration, slots: usize) -> Option<Self> {
        Some(Self {
            request_id,
            payload: quanta_index_contract::SearchPlaneIngestIpcResponse::Error(
                quanta_index_contract::SearchPlaneIpcError::overloaded(waited, slots),
            ),
        })
    }
}

/// Synchronous `AF_UNIX` stream server.
pub struct UdsServer {
    listener: UnixListener,
    socket_path: PathBuf,
    socket_path_identity: SocketPathIdentity,
    shutdown: Arc<AtomicBool>,
    policy: ServerAdmissionPolicy,
    /// Connections whose threads are alive; the accept loop refuses past
    /// the policy's cap instead of queueing without bound.
    live_connections: Arc<AtomicUsize>,
    /// Connections the accept loop closed because the cap was reached.
    refused_connections: Arc<AtomicUsize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SocketPathIdentity {
    device: u64,
    inode: u64,
}

impl SocketPathIdentity {
    fn capture(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_socket() {
            return Err(std::io::Error::other(
                "bound uds path is no longer a socket",
            ));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn still_owns(self, path: &Path) -> std::io::Result<bool> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => Ok(metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

impl UdsServer {
    /// Bind a new listener at `path` under [`ServerAdmissionPolicy::DEFAULT`].
    pub fn bind(path: &Path) -> Result<Self, IpcError> {
        Self::bind_with_policy(path, ServerAdmissionPolicy::DEFAULT)
    }

    /// Bind a new listener at `path`. Removes any pre-existing socket file
    /// at that path (only socket files — never a regular file).
    pub fn bind_with_policy(path: &Path, policy: ServerAdmissionPolicy) -> Result<Self, IpcError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(IpcError::Io)?;
        }
        // Remove stale socket if present and is a socket. metadata() of a
        // unix socket returns FileType where is_file()==false, is_dir()==false.
        match std::fs::symlink_metadata(path) {
            Ok(meta) => {
                if meta.file_type().is_socket() {
                    std::fs::remove_file(path).map_err(IpcError::Io)?;
                } else if meta.file_type().is_file() {
                    return Err(IpcError::Io(std::io::Error::other(
                        "uds path exists and is a regular file",
                    )));
                }
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(IpcError::Io(err)),
        }
        let listener = UnixListener::bind(path).map_err(IpcError::Io)?;
        let socket_path_identity = SocketPathIdentity::capture(path).map_err(IpcError::Io)?;
        listener.set_nonblocking(true).map_err(IpcError::Io)?;
        Ok(Self {
            listener,
            socket_path: path.to_path_buf(),
            socket_path_identity,
            shutdown: Arc::new(AtomicBool::new(false)),
            policy,
            live_connections: Arc::new(AtomicUsize::new(0)),
            refused_connections: Arc::new(AtomicUsize::new(0)),
        })
    }

    #[must_use]
    pub const fn admission_policy(&self) -> ServerAdmissionPolicy {
        self.policy
    }

    /// Connections the accept loop closed because the cap was reached.
    #[must_use]
    pub fn refused_connections(&self) -> usize {
        self.refused_connections.load(Ordering::Acquire)
    }

    /// Trigger graceful shutdown. Safe to call from any thread / signal handler.
    #[must_use]
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            inner: Arc::clone(&self.shutdown),
        }
    }

    /// Path the listener is bound to.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Run the accept loop until `shutdown` is triggered.
    ///
    /// Every accepted connection runs on its own thread, so reading one
    /// peer's request never waits on another's; dispatch concurrency is
    /// bounded separately by the policy's slots. The listener is
    /// non-blocking, so an empty accept queue sleeps `accept_idle` before
    /// retrying. On shutdown the loop stops accepting and joins the
    /// connection threads, each of which finishes its in-flight request
    /// (bounded by the dispatch budget and I/O timeouts) before exiting.
    pub fn run<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
        &self,
        dispatcher: &Arc<D>,
        accept_idle: Duration,
    ) -> Result<(), IpcError>
    where
        RequestEnvelopeT: RequestEnvelope<Request>,
        ResponseEnvelopeT: ResponseEnvelope<Response>,
        D: IpcDispatcher<Request, Response> + ?Sized + 'static,
    {
        let slots = Arc::new(DispatchSlots::new(self.policy.dispatch_slots()));
        let mut connection_threads: Vec<std::thread::JoinHandle<ConnectionCloseReason>> =
            Vec::new();
        while !self.shutdown.load(Ordering::Acquire) {
            connection_threads.retain(|handle| !handle.is_finished());
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    if self.live_connections.load(Ordering::Acquire)
                        >= self.policy.max_connections()
                    {
                        let _refused = self.refused_connections.fetch_add(1, Ordering::AcqRel);
                        drop(stream);
                        continue;
                    }
                    let _live = self.live_connections.fetch_add(1, Ordering::AcqRel);
                    let dispatcher = Arc::clone(dispatcher);
                    let slots = Arc::clone(&slots);
                    let live_connections = Arc::clone(&self.live_connections);
                    let shutdown = Arc::clone(&self.shutdown);
                    let policy = self.policy;
                    let spawned = std::thread::Builder::new()
                        .name("uds-connection".to_string())
                        .spawn(move || {
                            let reason = handle_connection::<
                                RequestEnvelopeT,
                                Request,
                                ResponseEnvelopeT,
                                Response,
                                D,
                            >(
                                stream, dispatcher.as_ref(), &slots, policy, &shutdown
                            );
                            let _live = live_connections.fetch_sub(1, Ordering::AcqRel);
                            reason
                        });
                    match spawned {
                        Ok(handle) => connection_threads.push(handle),
                        Err(err) => {
                            let _live = self.live_connections.fetch_sub(1, Ordering::AcqRel);
                            return Err(IpcError::Io(err));
                        }
                    }
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(accept_idle);
                }
                Err(err) => return Err(IpcError::Io(err)),
            }
        }
        for handle in connection_threads {
            let _reason = handle.join();
        }
        self.remove_owned_socket_path().map_err(IpcError::Io)?;
        Ok(())
    }

    fn remove_owned_socket_path(&self) -> std::io::Result<()> {
        if self.socket_path_identity.still_owns(&self.socket_path)? {
            std::fs::remove_file(&self.socket_path)?;
        }
        Ok(())
    }
}

impl Drop for UdsServer {
    fn drop(&mut self) {
        drop(self.remove_owned_socket_path());
    }
}

/// Handle returned by [`UdsServer::shutdown_handle`].
#[derive(Clone)]
pub struct ShutdownHandle {
    inner: Arc<AtomicBool>,
}

impl ShutdownHandle {
    pub fn trigger(&self) {
        self.inner.store(true, Ordering::Release);
    }
}

/// Default bounded I/O policy for one-shot clients that do not supply a
/// stricter owner deadline.
pub const DEFAULT_CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Absolute request I/O deadline policy shared by every one-shot IPC client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientIoPolicy {
    request_timeout: Duration,
    absolute_deadline: Option<Instant>,
}

impl ClientIoPolicy {
    pub fn try_new(request_timeout: Duration) -> Result<Self, IpcError> {
        if request_timeout.is_zero() {
            return Err(IpcError::InvalidClientIoTimeout);
        }
        Ok(Self {
            request_timeout,
            absolute_deadline: None,
        })
    }

    pub fn try_with_deadline(absolute_deadline: Instant) -> Result<Self, IpcError> {
        let request_timeout = absolute_deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(IpcError::ClientIoDeadlineElapsed)?;
        Ok(Self {
            request_timeout,
            absolute_deadline: Some(absolute_deadline),
        })
    }

    #[must_use]
    pub const fn request_timeout(self) -> Duration {
        self.request_timeout
    }

    #[must_use]
    pub const fn absolute_deadline(self) -> Option<Instant> {
        self.absolute_deadline
    }

    fn request_deadline(self) -> Result<Instant, IpcError> {
        match self.absolute_deadline {
            Some(deadline) if deadline > Instant::now() => Ok(deadline),
            Some(_) => Err(IpcError::ClientIoDeadlineElapsed),
            None => Instant::now()
                .checked_add(self.request_timeout)
                .ok_or(IpcError::InvalidClientIoTimeout),
        }
    }
}

impl Default for ClientIoPolicy {
    fn default() -> Self {
        Self {
            request_timeout: DEFAULT_CLIENT_IO_TIMEOUT,
            absolute_deadline: None,
        }
    }
}

#[derive(Debug)]
enum ConnectionCloseReason {
    BlockingModeConfigFailed(String),
    TimeoutConfigFailed(String),
    PeerClosed,
    RequestDecodeFailed(IpcError),
    ResponseEncodeFailed(IpcError),
    ResponseWriteFailed(String),
    /// No dispatch slot within the queue wait and the envelope type has no
    /// typed refusal to send.
    Overloaded {
        waited: Duration,
    },
    /// A request arrived after shutdown was triggered.
    ShuttingDown,
    /// The peer watch could not be armed, so the request could not be run
    /// with a live cancellation; it is not run at all.
    PeerWatchFailed(String),
}

impl core::fmt::Display for ConnectionCloseReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BlockingModeConfigFailed(message) => {
                write!(f, "blocking-mode setup failed: {message}")
            }
            Self::TimeoutConfigFailed(message) => {
                write!(f, "timeout setup failed: {message}")
            }
            Self::PeerClosed => f.write_str("peer closed connection cleanly"),
            Self::PeerWatchFailed(message) => write!(f, "peer watch setup failed: {message}"),
            Self::RequestDecodeFailed(err) => write!(f, "request decode failed: {err}"),
            Self::ResponseEncodeFailed(err) => write!(f, "response encode failed: {err}"),
            Self::ResponseWriteFailed(message) => write!(f, "response write failed: {message}"),
            Self::Overloaded { waited } => write!(
                f,
                "no dispatch slot within {} ms and no typed refusal for this envelope",
                waited.as_millis()
            ),
            Self::ShuttingDown => f.write_str("request arrived during shutdown"),
        }
    }
}

fn handle_connection<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
    mut stream: UnixStream,
    dispatcher: &D,
    slots: &DispatchSlots,
    policy: ServerAdmissionPolicy,
    shutdown: &AtomicBool,
) -> ConnectionCloseReason
where
    RequestEnvelopeT: RequestEnvelope<Request>,
    ResponseEnvelopeT: ResponseEnvelope<Response>,
    D: IpcDispatcher<Request, Response> + ?Sized,
{
    // Each connection may carry multiple sequential requests until close.
    if let Err(err) = stream.set_nonblocking(false) {
        return ConnectionCloseReason::BlockingModeConfigFailed(err.to_string());
    }
    // Apply a bounded read/write timeout so a stalled peer cannot pin its
    // own connection thread indefinitely.
    if let Err(err) = stream.set_read_timeout(Some(policy.io_timeout())) {
        return ConnectionCloseReason::TimeoutConfigFailed(format!("set_read_timeout: {err}"));
    }
    if let Err(err) = stream.set_write_timeout(Some(policy.io_timeout())) {
        return ConnectionCloseReason::TimeoutConfigFailed(format!("set_write_timeout: {err}"));
    }
    loop {
        let request = match decode_request::<RequestEnvelopeT, _>(&mut stream) {
            Ok(env) => env,
            Err(IpcError::Truncated) => return ConnectionCloseReason::PeerClosed,
            Err(err) => return ConnectionCloseReason::RequestDecodeFailed(err),
        };
        let (request_id, request_payload) = request.into_parts();
        // A request that arrives during shutdown is not dispatched; the
        // connection closes so the peer retries against the next process.
        if shutdown.load(Ordering::Acquire) {
            return ConnectionCloseReason::ShuttingDown;
        }
        // Admission: ingress happened above, now the dispatch slot. A full
        // queue is a typed answer, not an unbounded wait.
        let permit = match slots.acquire(policy.queue_wait()) {
            Ok(permit) => permit,
            Err(SlotRefusal::QueueWaitExceeded { waited, slots }) => {
                let Some(refusal) = ResponseEnvelopeT::overloaded(request_id, waited, slots) else {
                    return ConnectionCloseReason::Overloaded { waited };
                };
                match write_response(&mut stream, &refusal) {
                    Ok(()) => continue,
                    Err(reason) => return reason,
                }
            }
        };
        let budget = RequestBudgetV1::for_duration(policy.dispatch_budget());
        // A dispatch without a live watch would run with a cancellation that
        // can never fire; refusing the connection is the honest alternative.
        let watch = match PeerWatch::arm(&stream, budget.cancel_handle()) {
            Ok(watch) => watch,
            Err(err) => return ConnectionCloseReason::PeerWatchFailed(err.to_string()),
        };
        let response_payload = dispatcher.dispatch(request_payload, &budget);
        let peer_hung_up = watch.disarm();
        drop(permit);
        if peer_hung_up {
            // Nothing to write to; the dispatcher already saw the
            // cancellation at its next checkpoint (or ran to completion).
            return ConnectionCloseReason::PeerClosed;
        }
        let response = ResponseEnvelopeT::from_parts(request_id, response_payload);
        let frame = match encode_response(&response) {
            Ok(frame) => frame,
            // The answer was computed but cannot cross the wire. Tell the
            // caller so with a typed refusal instead of dropping the
            // connection, which would be indistinguishable from a crash.
            Err(IpcError::Oversized(encoded_bytes)) => {
                let limit_bytes =
                    u64::try_from(MAX_FRAME_BODY_BYTES).map_or(u64::MAX, |limit| limit);
                let Some(refusal) =
                    ResponseEnvelopeT::result_too_large(request_id, encoded_bytes, limit_bytes)
                else {
                    return ConnectionCloseReason::ResponseEncodeFailed(IpcError::Oversized(
                        encoded_bytes,
                    ));
                };
                match encode_response(&refusal) {
                    Ok(frame) => frame,
                    Err(err) => return ConnectionCloseReason::ResponseEncodeFailed(err),
                }
            }
            Err(err) => return ConnectionCloseReason::ResponseEncodeFailed(err),
        };
        if let Err(err) = stream.write_all(&frame) {
            return ConnectionCloseReason::ResponseWriteFailed(err.to_string());
        }
        // continue: next request on same conn
    }
}

fn write_response<ResponseEnvelopeT: serde::Serialize>(
    stream: &mut UnixStream,
    response: &ResponseEnvelopeT,
) -> Result<(), ConnectionCloseReason> {
    let frame = encode_response(response).map_err(ConnectionCloseReason::ResponseEncodeFailed)?;
    stream
        .write_all(&frame)
        .map_err(|err| ConnectionCloseReason::ResponseWriteFailed(err.to_string()))
}

/// Watches a connection for a hang-up while its request is dispatching.
///
/// The dispatch runs synchronously on the connection thread, so a second
/// thread polls the socket. Readable with data means the peer pipelined
/// its next request; that is not a hang-up and the watch simply stops
/// looking. `POLLHUP`, `POLLERR` or an end-of-file mean the peer at least
/// shut its write side — which is *not* yet a hang-up: a peer that sent
/// its request and half-closed is still waiting for the response. The
/// watch confirms a hang-up by asking whether the peer can still receive
/// (a zero-byte send, which fails with `EPIPE` only once the peer's read
/// side is gone), and after a half-close it keeps asking at the poll
/// interval instead of polling, since the end-of-file stays readable. The
/// watch ends when the dispatch returns.
struct PeerWatch {
    stop: Arc<AtomicBool>,
    hung_up: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PeerWatch {
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    fn arm(
        stream: &UnixStream,
        cancel: quanta_index_core::CancelHandleV1,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let hung_up = Arc::new(AtomicBool::new(false));
        let watched = stream.try_clone()?;
        let thread = {
            let stop = Arc::clone(&stop);
            let hung_up = Arc::clone(&hung_up);
            std::thread::Builder::new()
                .name("uds-peer-watch".to_string())
                .spawn(move || {
                    let mut half_closed = false;
                    while !stop.load(Ordering::Acquire) {
                        let state = if half_closed {
                            std::thread::sleep(Self::POLL_INTERVAL);
                            if peer_can_receive(&watched) {
                                PeerState::HalfClosed
                            } else {
                                PeerState::HungUp
                            }
                        } else {
                            peer_state(&watched)
                        };
                        match state {
                            PeerState::Alive => {}
                            PeerState::HalfClosed => half_closed = true,
                            PeerState::Pipelined => return,
                            PeerState::HungUp => {
                                hung_up.store(true, Ordering::Release);
                                cancel.cancel();
                                return;
                            }
                        }
                    }
                })?
        };
        Ok(Self {
            stop,
            hung_up,
            thread: Some(thread),
        })
    }

    /// Stop watching; reports whether the peer hung up while we watched.
    fn disarm(mut self) -> bool {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _joined = thread.join();
        }
        self.hung_up.load(Ordering::Acquire)
    }
}

enum PeerState {
    Alive,
    /// The peer shut its write side and is waiting for the response.
    HalfClosed,
    Pipelined,
    HungUp,
}

/// Whether the peer can still receive: a zero-byte send succeeds while
/// the peer's read side is open and fails with `EPIPE` once it is gone.
///
/// A half-close (`shutdown(Write)` on the peer) leaves its read side open,
/// so this is what tells a waiting peer from a departed one; `poll` alone
/// cannot, because Darwin reports `POLLHUP` for both. `SIGPIPE` is not a
/// concern: the Rust runtime ignores it, and the socket carries
/// `SO_NOSIGPIPE` where the platform has it.
fn peer_can_receive(stream: &UnixStream) -> bool {
    let fd = std::os::fd::AsFd::as_fd(stream);
    match send(fd, &[], peer_probe_send_flags()) {
        Ok(_sent) => true,
        Err(Errno::AGAIN | Errno::INTR) => true,
        Err(_gone) => false,
    }
}

#[cfg(target_os = "linux")]
fn peer_probe_send_flags() -> SendFlags {
    SendFlags::DONTWAIT | SendFlags::NOSIGNAL
}

#[cfg(not(target_os = "linux"))]
fn peer_probe_send_flags() -> SendFlags {
    SendFlags::DONTWAIT
}

/// One bounded poll of the watched socket, for a peer not yet seen to
/// half-close.
fn peer_state(stream: &UnixStream) -> PeerState {
    let fd = std::os::fd::AsFd::as_fd(stream);
    let mut fds = [PollFd::new(&fd, PollFlags::IN | PollFlags::HUP)];
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: i64::try_from(PeerWatch::POLL_INTERVAL.as_nanos())
            .map_or(50_000_000, |nanos| nanos),
    };
    let closed_or_gone = |stream: &UnixStream| {
        if peer_can_receive(stream) {
            PeerState::HalfClosed
        } else {
            PeerState::HungUp
        }
    };
    match poll(&mut fds, Some(&timeout)) {
        Ok(0) | Err(Errno::INTR) => PeerState::Alive,
        Ok(_) => {
            let revents = fds.first().map_or(PollFlags::empty(), PollFd::revents);
            if revents.contains(PollFlags::IN) {
                // Readable: either pipelined data or the peer's end-of-file.
                // A peek leaves the bytes for the connection thread's next
                // request read.
                let mut probe = [0_u8; 1];
                return match recv(fd, &mut probe, RecvFlags::PEEK) {
                    Ok((0, _)) => closed_or_gone(stream),
                    Ok(_) => PeerState::Pipelined,
                    Err(Errno::AGAIN | Errno::INTR) => PeerState::Alive,
                    Err(_) => closed_or_gone(stream),
                };
            }
            if revents.contains(PollFlags::HUP) || revents.contains(PollFlags::ERR) {
                return closed_or_gone(stream);
            }
            PeerState::Alive
        }
        Err(_) => closed_or_gone(stream),
    }
}

/// One-shot client: open a stream, send `request`, read one response.
pub fn send_request<RequestEnvelopeT, ResponseEnvelopeT>(
    socket: &Path,
    request: &RequestEnvelopeT,
    io_policy: ClientIoPolicy,
) -> Result<ResponseEnvelopeT, IpcError>
where
    RequestEnvelopeT: serde::Serialize,
    ResponseEnvelopeT: serde::de::DeserializeOwned,
{
    let deadline = io_policy.request_deadline()?;
    let frame = encode_request(request)?;
    let stream = connect_before_deadline(socket, deadline)
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Connect, io_policy))?;
    let mut stream = DeadlineStream::new(stream, deadline);
    stream
        .write_all(&frame)
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Write, io_policy))?;
    let response = decode_response::<ResponseEnvelopeT, _>(&mut stream)
        .map_err(|error| classify_client_decode_error(error, IpcIoOperation::Read, io_policy))?;
    ensure_deadline_remaining(deadline)
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Read, io_policy))?;
    Ok(response)
}

fn connect_before_deadline(socket_path: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
    ensure_deadline_remaining(deadline)?;
    let address = SocketAddrUnix::new(socket_path).map_err(std::io::Error::from)?;
    let socket = create_connect_socket()?;

    match connect(&socket, &address) {
        Ok(()) => {}
        Err(error) if connect_requires_completion_wait(error) => {
            wait_for_connect(&socket, deadline)?;
        }
        Err(error) => return Err(error.into()),
    }

    let stream = UnixStream::from(socket);
    stream.set_nonblocking(false)?;
    ensure_deadline_remaining(deadline)?;
    Ok(stream)
}

fn connect_requires_completion_wait(error: Errno) -> bool {
    matches!(
        error,
        Errno::INPROGRESS | Errno::ALREADY | Errno::WOULDBLOCK | Errno::INTR
    )
}

#[cfg(target_os = "linux")]
fn create_connect_socket() -> std::io::Result<OwnedFd> {
    socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )
    .map_err(std::io::Error::from)
}

#[cfg(not(target_os = "linux"))]
fn create_connect_socket() -> std::io::Result<OwnedFd> {
    let socket =
        socket(AddressFamily::UNIX, SocketType::STREAM, None).map_err(std::io::Error::from)?;
    ioctl_fioclex(&socket).map_err(std::io::Error::from)?;
    ioctl_fionbio(&socket, true).map_err(std::io::Error::from)?;
    configure_socket_write_safety(&socket)?;
    Ok(socket)
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
))]
fn configure_socket_write_safety(socket: &OwnedFd) -> std::io::Result<()> {
    set_socket_nosigpipe(socket, true).map_err(std::io::Error::from)
}

#[cfg(all(
    not(target_os = "linux"),
    not(any(
        target_vendor = "apple",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
    )),
))]
const fn configure_socket_write_safety(_socket: &OwnedFd) -> std::io::Result<()> {
    Ok(())
}

fn wait_for_connect(socket: &OwnedFd, deadline: Instant) -> std::io::Result<()> {
    let mut poll_fd = [PollFd::new(socket, PollFlags::OUT)];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(deadline_elapsed_error)?;
        let timeout = Timespec::try_from(remaining)
            .map_err(|error| std::io::Error::new(ErrorKind::InvalidInput, error))?;

        match poll(&mut poll_fd, Some(&timeout)) {
            Ok(0) => return Err(deadline_elapsed_error()),
            Ok(_) => {
                return socket_error(socket)
                    .map_err(std::io::Error::from)?
                    .map_err(std::io::Error::from);
            }
            Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn ensure_deadline_remaining(deadline: Instant) -> std::io::Result<()> {
    if deadline > Instant::now() {
        Ok(())
    } else {
        Err(deadline_elapsed_error())
    }
}

fn deadline_elapsed_error() -> std::io::Error {
    std::io::Error::new(ErrorKind::TimedOut, "IPC request deadline elapsed")
}

struct DeadlineStream {
    stream: UnixStream,
    deadline: Instant,
}

impl DeadlineStream {
    const fn new(stream: UnixStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }

    fn remaining(&self) -> std::io::Result<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "IPC request deadline elapsed",
            ))
        } else {
            Ok(remaining)
        }
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.remaining()?;
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(buffer)
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let remaining = self.remaining()?;
        self.stream.set_write_timeout(Some(remaining))?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let remaining = self.remaining()?;
        self.stream.set_write_timeout(Some(remaining))?;
        self.stream.flush()
    }
}

fn classify_client_decode_error(
    error: IpcError,
    operation: IpcIoOperation,
    io_policy: ClientIoPolicy,
) -> IpcError {
    match error {
        IpcError::Io(error) => classify_client_io_error(error, operation, io_policy),
        other @ (IpcError::Truncated
        | IpcError::Oversized(_)
        | IpcError::EmptyFrame
        | IpcError::Encode(_)
        | IpcError::Decode(_)
        | IpcError::Timeout { .. }
        | IpcError::InvalidClientIoTimeout
        | IpcError::ClientIoDeadlineElapsed
        | IpcError::InvalidAdmissionPolicy) => other,
    }
}

fn classify_client_io_error(
    error: std::io::Error,
    operation: IpcIoOperation,
    io_policy: ClientIoPolicy,
) -> IpcError {
    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
        IpcError::Timeout {
            operation,
            timeout: io_policy.request_timeout(),
        }
    } else {
        IpcError::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientIoPolicy, ConnectionCloseReason, IpcDispatcher, IpcError, PeerWatch, RequestEnvelope,
        ResponseEnvelope, UdsServer, connect_before_deadline, connect_requires_completion_wait,
        create_connect_socket, decode_response, encode_request, handle_connection, send_request,
        wait_for_connect,
    };
    use rustix::fs::{OFlags, fcntl_getfl};
    use rustix::io::{Errno, FdFlags, fcntl_getfd};
    use std::io::Write;
    use std::net::Shutdown;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    use quanta_index_core::RequestBudgetV1;
    use serde::de::{self, MapAccess, Visitor};
    use serde::ser::SerializeStruct;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::admission::{DispatchSlots, ServerAdmissionPolicy};
    use crate::codec::MAX_FRAME_BODY_BYTES;

    type TestRes = Result<(), String>;

    fn test_slots() -> DispatchSlots {
        DispatchSlots::new(ServerAdmissionPolicy::DEFAULT.dispatch_slots())
    }

    fn test_policy() -> ServerAdmissionPolicy {
        ServerAdmissionPolicy::DEFAULT
    }

    struct ByteStringRequest(Vec<u8>);

    impl Serialize for ByteStringRequest {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_bytes(&self.0)
        }
    }

    #[test]
    fn client_read_timeout_closes_silent_peer_with_typed_error() -> TestRes {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket = dir.path().join("silent-peer.sock");
        let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || -> TestRes {
            let (_stream, _address) = listener.accept().map_err(|error| error.to_string())?;
            release_rx.recv().map_err(|error| error.to_string())?;
            Ok(())
        });

        let timeout = Duration::from_millis(25);
        let policy = ClientIoPolicy::try_new(timeout).map_err(|error| error.to_string())?;
        let result = send_request::<_, TestResponseEnvelope>(
            &socket,
            &TestRequestEnvelope {
                request_id: 1,
                payload: 7,
            },
            policy,
        );
        release_tx.send(()).map_err(|error| error.to_string())?;
        let server_result = server
            .join()
            .map_err(|_panic_payload| "server panicked".to_string())?;
        server_result?;
        if !matches!(
            result,
            Err(IpcError::Timeout {
                operation: super::IpcIoOperation::Read,
                timeout: observed,
            }) if observed == timeout
        ) {
            return Err(format!("expected typed read timeout, got {result:?}"));
        }
        Ok(())
    }

    #[test]
    fn absolute_client_deadline_does_not_restart_at_send_boundary() -> TestRes {
        let deadline = std::time::Instant::now() + Duration::from_millis(20);
        let policy =
            ClientIoPolicy::try_with_deadline(deadline).map_err(|error| error.to_string())?;
        thread::sleep(Duration::from_millis(30));

        let result = send_request::<_, TestResponseEnvelope>(
            std::path::Path::new("/path/that/must/not/be-connected.sock"),
            &TestRequestEnvelope {
                request_id: 99,
                payload: 7,
            },
            policy,
        );

        if !matches!(result, Err(IpcError::ClientIoDeadlineElapsed)) {
            return Err(format!(
                "elapsed absolute deadline must fail before socket connect, got {result:?}"
            ));
        }
        Ok(())
    }

    #[test]
    fn connect_readiness_wait_obeys_absolute_deadline() -> TestRes {
        let (mut writer, _reader) = UnixStream::pair().map_err(|error| error.to_string())?;
        writer
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let payload = vec![0x5a_u8; 64 * 1024];
        loop {
            match writer.write(&payload) {
                Ok(0) => return Err("socket send buffer closed while filling".to_string()),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.to_string()),
            }
        }

        let socket = std::os::fd::OwnedFd::from(writer);
        let deadline = Instant::now() + Duration::from_millis(25);
        let started = Instant::now();
        let error = match wait_for_connect(&socket, deadline) {
            Ok(()) => return Err("saturated socket unexpectedly became writable".to_string()),
            Err(error) => error,
        };
        if error.kind() != std::io::ErrorKind::TimedOut {
            return Err(format!("expected connect readiness timeout, got {error}"));
        }
        if started.elapsed() > Duration::from_secs(1) {
            return Err("connect readiness exceeded its absolute deadline bound".to_string());
        }
        Ok(())
    }

    #[test]
    fn connect_socket_preserves_descriptor_and_blocking_invariants() -> TestRes {
        let socket = create_connect_socket().map_err(|error| error.to_string())?;
        let descriptor_flags = fcntl_getfd(&socket).map_err(|error| error.to_string())?;
        if !descriptor_flags.contains(FdFlags::CLOEXEC) {
            return Err("connect socket must be close-on-exec".to_string());
        }
        let status_flags = fcntl_getfl(&socket).map_err(|error| error.to_string())?;
        if !status_flags.contains(OFlags::NONBLOCK) {
            return Err("connect socket must start nonblocking".to_string());
        }
        #[cfg(any(
            target_vendor = "apple",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
        ))]
        if !rustix::net::sockopt::socket_nosigpipe(&socket).map_err(|error| error.to_string())? {
            return Err("connect socket must suppress SIGPIPE".to_string());
        }
        drop(socket);

        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket_path = dir.path().join("descriptor-state.sock");
        let _listener = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
        let stream = connect_before_deadline(&socket_path, Instant::now() + Duration::from_secs(1))
            .map_err(|error| error.to_string())?;
        let connected_flags = fcntl_getfl(&stream).map_err(|error| error.to_string())?;
        if connected_flags.contains(OFlags::NONBLOCK) {
            return Err("connected client stream must return to blocking mode".to_string());
        }
        Ok(())
    }

    #[test]
    fn interrupted_connect_enters_completion_wait_contract() {
        assert!(connect_requires_completion_wait(Errno::INTR));
    }

    #[test]
    fn dropping_superseded_server_does_not_unlink_replacement_socket() -> TestRes {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket_path = dir.path().join("replacement.sock");
        let superseded = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;
        std::fs::remove_file(&socket_path).map_err(|error| error.to_string())?;
        let replacement = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;

        drop(superseded);

        let client = UnixStream::connect(&socket_path)
            .map_err(|error| format!("replacement socket was unlinked: {error}"))?;
        drop(client);
        drop(replacement);
        Ok(())
    }

    #[test]
    fn client_read_timeout_closes_partial_response_frame_with_typed_error() -> TestRes {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket = dir.path().join("partial-frame.sock");
        let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || -> TestRes {
            let (mut stream, _address) = listener.accept().map_err(|error| error.to_string())?;
            let _request: TestRequestEnvelope =
                super::decode_request(&mut stream).map_err(|error| error.to_string())?;
            stream
                .write_all(&10_u32.to_le_bytes())
                .map_err(|error| error.to_string())?;
            stream
                .write_all(&[0xa1, 0x01])
                .map_err(|error| error.to_string())?;
            release_rx.recv().map_err(|error| error.to_string())?;
            Ok(())
        });

        let timeout = Duration::from_millis(25);
        let policy = ClientIoPolicy::try_new(timeout).map_err(|error| error.to_string())?;
        let result = send_request::<_, TestResponseEnvelope>(
            &socket,
            &TestRequestEnvelope {
                request_id: 2,
                payload: 8,
            },
            policy,
        );
        release_tx.send(()).map_err(|error| error.to_string())?;
        let server_result = server
            .join()
            .map_err(|_panic_payload| "server panicked".to_string())?;
        server_result?;
        if !matches!(
            result,
            Err(IpcError::Timeout {
                operation: super::IpcIoOperation::Read,
                timeout: observed,
            }) if observed == timeout
        ) {
            return Err(format!(
                "expected typed partial-frame timeout, got {result:?}"
            ));
        }
        Ok(())
    }

    #[test]
    fn client_write_timeout_closes_peer_that_never_reads_with_typed_error() -> TestRes {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket = dir.path().join("write-backpressure.sock");
        let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || -> TestRes {
            let (_stream, _address) = listener.accept().map_err(|error| error.to_string())?;
            release_rx.recv().map_err(|error| error.to_string())?;
            Ok(())
        });

        let timeout = Duration::from_millis(25);
        let policy = ClientIoPolicy::try_new(timeout).map_err(|error| error.to_string())?;
        let request = ByteStringRequest(vec![0x5a_u8; 8 * 1024 * 1024]);
        let frame = encode_request(&request).map_err(|error| error.to_string())?;
        if frame.len() <= 1024 * 1024 {
            return Err(format!(
                "write-backpressure fixture frame is unexpectedly small: {} bytes",
                frame.len()
            ));
        }
        let result = send_request::<_, TestResponseEnvelope>(&socket, &request, policy);
        release_tx.send(()).map_err(|error| error.to_string())?;
        let server_result = server
            .join()
            .map_err(|_panic_payload| "server panicked".to_string())?;
        server_result?;
        if !matches!(
            result,
            Err(IpcError::Timeout {
                operation: super::IpcIoOperation::Write,
                timeout: observed,
            }) if observed == timeout
        ) {
            return Err(format!("expected typed write timeout, got {result:?}"));
        }
        Ok(())
    }

    struct TestDispatcher;

    impl IpcDispatcher<u64, u64> for TestDispatcher {
        fn dispatch(&self, request: u64, _budget: &RequestBudgetV1) -> u64 {
            request.saturating_add(1)
        }
    }

    #[derive(Debug, PartialEq)]
    struct TestRequestEnvelope {
        request_id: u64,
        payload: u64,
    }

    impl Serialize for TestRequestEnvelope {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut state = serializer.serialize_struct("TestRequestEnvelope", 2)?;
            state.serialize_field("request_id", &self.request_id)?;
            state.serialize_field("payload", &self.payload)?;
            state.end()
        }
    }

    impl<'de> Deserialize<'de> for TestRequestEnvelope {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct TestRequestEnvelopeVisitor;

            impl<'de> Visitor<'de> for TestRequestEnvelopeVisitor {
                type Value = TestRequestEnvelope;

                fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                    formatter.write_str("a TestRequestEnvelope map")
                }

                fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                where
                    A: MapAccess<'de>,
                {
                    let mut request_id: Option<u64> = None;
                    let mut payload: Option<u64> = None;
                    while let Some(key) = map.next_key::<String>()? {
                        match key.as_str() {
                            "request_id" => {
                                if request_id.is_some() {
                                    return Err(de::Error::duplicate_field("request_id"));
                                }
                                request_id = Some(map.next_value()?);
                            }
                            "payload" => {
                                if payload.is_some() {
                                    return Err(de::Error::duplicate_field("payload"));
                                }
                                payload = Some(map.next_value()?);
                            }
                            _ => {
                                return Err(de::Error::unknown_field(
                                    &key,
                                    &["request_id", "payload"],
                                ));
                            }
                        }
                    }
                    Ok(TestRequestEnvelope {
                        request_id: request_id
                            .ok_or_else(|| de::Error::missing_field("request_id"))?,
                        payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
                    })
                }
            }

            deserializer.deserialize_struct(
                "TestRequestEnvelope",
                &["request_id", "payload"],
                TestRequestEnvelopeVisitor,
            )
        }
    }

    impl RequestEnvelope<u64> for TestRequestEnvelope {
        fn into_parts(self) -> (u64, u64) {
            (self.request_id, self.payload)
        }
    }

    #[derive(Debug, PartialEq)]
    struct TestResponseEnvelope {
        request_id: u64,
        payload: u64,
    }

    impl Serialize for TestResponseEnvelope {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut state = serializer.serialize_struct("TestResponseEnvelope", 2)?;
            state.serialize_field("request_id", &self.request_id)?;
            state.serialize_field("payload", &self.payload)?;
            state.end()
        }
    }

    impl<'de> Deserialize<'de> for TestResponseEnvelope {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct TestResponseEnvelopeVisitor;

            impl<'de> Visitor<'de> for TestResponseEnvelopeVisitor {
                type Value = TestResponseEnvelope;

                fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                    formatter.write_str("a TestResponseEnvelope map")
                }

                fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                where
                    A: MapAccess<'de>,
                {
                    let mut request_id: Option<u64> = None;
                    let mut payload: Option<u64> = None;
                    while let Some(key) = map.next_key::<String>()? {
                        match key.as_str() {
                            "request_id" => {
                                if request_id.is_some() {
                                    return Err(de::Error::duplicate_field("request_id"));
                                }
                                request_id = Some(map.next_value()?);
                            }
                            "payload" => {
                                if payload.is_some() {
                                    return Err(de::Error::duplicate_field("payload"));
                                }
                                payload = Some(map.next_value()?);
                            }
                            _ => {
                                return Err(de::Error::unknown_field(
                                    &key,
                                    &["request_id", "payload"],
                                ));
                            }
                        }
                    }
                    Ok(TestResponseEnvelope {
                        request_id: request_id
                            .ok_or_else(|| de::Error::missing_field("request_id"))?,
                        payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
                    })
                }
            }

            deserializer.deserialize_struct(
                "TestResponseEnvelope",
                &["request_id", "payload"],
                TestResponseEnvelopeVisitor,
            )
        }
    }

    impl ResponseEnvelope<u64> for TestResponseEnvelope {
        fn from_parts(request_id: u64, payload: u64) -> Self {
            Self {
                request_id,
                payload,
            }
        }

        fn result_too_large(
            _request_id: u64,
            _encoded_bytes: u64,
            _limit_bytes: u64,
        ) -> Option<Self> {
            None
        }

        fn overloaded(_request_id: u64, _waited: Duration, _slots: usize) -> Option<Self> {
            None
        }
    }

    #[derive(Debug)]
    struct DelayedTestResponseEnvelope;

    impl<'de> Deserialize<'de> for DelayedTestResponseEnvelope {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            let _response = TestResponseEnvelope::deserialize(deserializer)?;
            thread::sleep(Duration::from_millis(60));
            Ok(Self)
        }
    }

    #[test]
    fn absolute_client_deadline_rejects_response_completed_after_decode() -> TestRes {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket_path = dir.path().join("late-decode.sock");
        let listener = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
        let server = thread::spawn(move || -> TestRes {
            let (stream, _address) = listener.accept().map_err(|error| error.to_string())?;
            let reason = handle_connection::<
                TestRequestEnvelope,
                u64,
                TestResponseEnvelope,
                u64,
                TestDispatcher,
            >(
                stream,
                &TestDispatcher,
                &test_slots(),
                test_policy(),
                &AtomicBool::new(false),
            );
            if !matches!(reason, ConnectionCloseReason::PeerClosed) {
                return Err(format!("unexpected close reason: {reason:?}"));
            }
            Ok(())
        });

        let deadline = Instant::now() + Duration::from_millis(30);
        let policy =
            ClientIoPolicy::try_with_deadline(deadline).map_err(|error| error.to_string())?;
        let result = send_request::<_, DelayedTestResponseEnvelope>(
            &socket_path,
            &test_request(17, 4),
            policy,
        );
        let server_result = server
            .join()
            .map_err(|_panic_payload| "server panicked".to_string())?;
        server_result?;
        if !matches!(
            result,
            Err(IpcError::Timeout {
                operation: super::IpcIoOperation::Read,
                ..
            })
        ) {
            return Err(format!(
                "response completed after its deadline must be rejected, got {result:?}"
            ));
        }
        Ok(())
    }

    struct FailingResponseEnvelope;

    impl Serialize for FailingResponseEnvelope {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(serde::ser::Error::custom(
                "simulated response encode failure",
            ))
        }
    }

    impl ResponseEnvelope<u64> for FailingResponseEnvelope {
        fn from_parts(_request_id: u64, _payload: u64) -> Self {
            Self
        }

        fn result_too_large(
            _request_id: u64,
            _encoded_bytes: u64,
            _limit_bytes: u64,
        ) -> Option<Self> {
            None
        }

        fn overloaded(_request_id: u64, _waited: Duration, _slots: usize) -> Option<Self> {
            None
        }
    }

    /// Encodes past the frame limit on the first serialization and answers
    /// the oversize refusal with a small typed marker, so the test can see
    /// that the server sent the refusal rather than closing.
    enum OversizedResponseEnvelope {
        Oversized { request_id: u64 },
        Refusal { request_id: u64, encoded_bytes: u64 },
    }

    impl Serialize for OversizedResponseEnvelope {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            match self {
                Self::Oversized { request_id } => {
                    let mut state = serializer.serialize_struct("OversizedResponseEnvelope", 2)?;
                    state.serialize_field("request_id", request_id)?;
                    let filler = vec![0_u8; MAX_FRAME_BODY_BYTES.saturating_add(1)];
                    state.serialize_field("payload", &CborBytes(&filler))?;
                    state.end()
                }
                Self::Refusal {
                    request_id,
                    encoded_bytes,
                } => {
                    let mut state = serializer.serialize_struct("OversizedResponseEnvelope", 2)?;
                    state.serialize_field("request_id", request_id)?;
                    state.serialize_field("payload", encoded_bytes)?;
                    state.end()
                }
            }
        }
    }

    /// Serialize a byte vector as a CBOR byte string without a `serde_bytes`
    /// dependency: a `serde::Serialize` shim over `&[u8]`.
    struct CborBytes<'a>(&'a [u8]);

    impl Serialize for CborBytes<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_bytes(self.0)
        }
    }

    impl ResponseEnvelope<u64> for OversizedResponseEnvelope {
        fn from_parts(request_id: u64, _payload: u64) -> Self {
            Self::Oversized { request_id }
        }

        fn result_too_large(
            request_id: u64,
            encoded_bytes: u64,
            _limit_bytes: u64,
        ) -> Option<Self> {
            Some(Self::Refusal {
                request_id,
                encoded_bytes,
            })
        }

        fn overloaded(_request_id: u64, _waited: Duration, _slots: usize) -> Option<Self> {
            None
        }
    }

    struct BlockingDispatcher {
        entered: mpsc::Sender<()>,
        gate: Arc<Barrier>,
        /// Whether, after the gate, the budget reported the peer's hang-up
        /// within a bounded wait: the cooperative cancellation signal.
        observed_cancel: Arc<AtomicBool>,
    }

    impl IpcDispatcher<u64, u64> for BlockingDispatcher {
        fn dispatch(&self, request: u64, budget: &RequestBudgetV1) -> u64 {
            let send_result = self.entered.send(());
            assert!(
                send_result.is_ok(),
                "test must observe dispatcher entry: {send_result:?}"
            );
            let _wait = self.gate.wait();
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(2) {
                if budget.is_cancelled() {
                    self.observed_cancel.store(true, Ordering::Release);
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            request.saturating_add(1)
        }
    }

    /// Signals entry, waits at the gate, then answers; records whether the
    /// budget was cancelled by the time it was released.
    struct HalfCloseDispatcher {
        entered: mpsc::Sender<()>,
        gate: Arc<Barrier>,
        observed_cancel: Arc<AtomicBool>,
    }

    impl IpcDispatcher<u64, u64> for HalfCloseDispatcher {
        fn dispatch(&self, request: u64, budget: &RequestBudgetV1) -> u64 {
            let send_result = self.entered.send(());
            assert!(
                send_result.is_ok(),
                "test must observe dispatcher entry: {send_result:?}"
            );
            let _wait = self.gate.wait();
            if budget.is_cancelled() {
                self.observed_cancel.store(true, Ordering::Release);
            }
            request.saturating_add(1)
        }
    }

    fn test_request(request_id: u64, payload: u64) -> TestRequestEnvelope {
        TestRequestEnvelope {
            request_id,
            payload,
        }
    }

    fn assert_test_ok(result: &TestRes) {
        assert!(result.is_ok(), "{result:?}");
    }

    fn encode_test_frame(request_id: u64, payload: u64) -> Result<Vec<u8>, String> {
        encode_request(&test_request(request_id, payload))
            .map_err(|err| format!("test request must encode: {err}"))
    }

    #[test]
    fn handle_connection_returns_peer_closed_after_successful_round_trip() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let frame = encode_test_frame(41, 8)?;
            client.write_all(&frame).map_err(|err| err.to_string())?;
            client
                .shutdown(Shutdown::Write)
                .map_err(|err| err.to_string())?;

            let handle = thread::spawn(move || {
                handle_connection::<
                    TestRequestEnvelope,
                    u64,
                    TestResponseEnvelope,
                    u64,
                    TestDispatcher,
                >(
                    server,
                    &TestDispatcher,
                    &test_slots(),
                    test_policy(),
                    &AtomicBool::new(false),
                )
            });

            let response = decode_response::<TestResponseEnvelope, _>(&mut client)
                .map_err(|err| format!("server must write one response before closing: {err}"))?;
            let expected = TestResponseEnvelope {
                request_id: 41,
                payload: 9,
            };
            if response != expected {
                return Err(format!("unexpected response: {response:?}"));
            }
            let reason = handle
                .join()
                .map_err(|join_err| format!("server thread panicked: {join_err:?}"))?;
            if !matches!(reason, ConnectionCloseReason::PeerClosed) {
                return Err(format!("unexpected close reason: {reason:?}"));
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    #[test]
    fn handle_connection_surfaces_decode_failure_reason() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            client
                .write_all(&[0, 0, 0, 0])
                .map_err(|err| err.to_string())?;
            client
                .shutdown(Shutdown::Write)
                .map_err(|err| err.to_string())?;

            let reason = handle_connection::<
                TestRequestEnvelope,
                u64,
                TestResponseEnvelope,
                u64,
                TestDispatcher,
            >(
                server,
                &TestDispatcher,
                &test_slots(),
                test_policy(),
                &AtomicBool::new(false),
            );
            if !matches!(
                reason,
                ConnectionCloseReason::RequestDecodeFailed(IpcError::EmptyFrame)
            ) {
                return Err(format!("unexpected close reason: {reason:?}"));
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    /// A response that encodes past the frame limit is answered with the
    /// envelope's typed refusal, on the same connection, instead of a close.
    #[test]
    fn handle_connection_sends_a_typed_refusal_for_an_oversized_response() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let frame = encode_test_frame(12, 4)?;
            client.write_all(&frame).map_err(|err| err.to_string())?;
            client
                .shutdown(Shutdown::Write)
                .map_err(|err| err.to_string())?;

            let handle = thread::spawn(move || {
                handle_connection::<
                    TestRequestEnvelope,
                    u64,
                    OversizedResponseEnvelope,
                    u64,
                    TestDispatcher,
                >(
                    server,
                    &TestDispatcher,
                    &test_slots(),
                    test_policy(),
                    &AtomicBool::new(false),
                )
            });
            let response: TestResponseEnvelope =
                decode_response(&mut client).map_err(|err| err.to_string())?;
            let reason = handle
                .join()
                .map_err(|_panic| "server thread panicked".to_string())?;
            if response.request_id != 12 {
                return Err(format!(
                    "refusal carried request_id {}",
                    response.request_id
                ));
            }
            let limit = u64::try_from(MAX_FRAME_BODY_BYTES).map_err(|err| err.to_string())?;
            if response.payload <= limit {
                return Err(format!(
                    "refusal must report the oversized body, got {} bytes",
                    response.payload
                ));
            }
            if !matches!(reason, ConnectionCloseReason::PeerClosed) {
                return Err(format!("unexpected close reason: {reason:?}"));
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    #[test]
    fn handle_connection_surfaces_response_encode_failure_reason() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let frame = encode_test_frame(7, 4)?;
            client.write_all(&frame).map_err(|err| err.to_string())?;
            client
                .shutdown(Shutdown::Write)
                .map_err(|err| err.to_string())?;

            let reason = handle_connection::<
                TestRequestEnvelope,
                u64,
                FailingResponseEnvelope,
                u64,
                TestDispatcher,
            >(
                server,
                &TestDispatcher,
                &test_slots(),
                test_policy(),
                &AtomicBool::new(false),
            );
            if let ConnectionCloseReason::ResponseEncodeFailed(IpcError::Encode(message)) = &reason
                && message.contains("simulated response encode failure")
            {
                return Ok(());
            }
            Err(format!("unexpected close reason: {reason:?}"))
        })();
        assert_test_ok(&result);
    }

    /// A mid-dispatch hang-up cancels the request's budget (QI-BB-002).
    ///
    /// The peer watch notices the hang-up, the dispatcher sees the
    /// cancellation at its next checkpoint, and the connection closes as a
    /// hang-up instead of a failed write.
    /// A peer that sent its request and shut its write side is waiting for
    /// the response, not gone: the watch must not cancel it, and the
    /// response must still cross the wire. The dispatch is held long enough
    /// that the watch certainly polls after the half-close.
    #[test]
    fn a_half_closed_peer_is_not_a_hang_up_and_still_gets_its_response() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let frame = encode_test_frame(7, 3)?;
            client.write_all(&frame).map_err(|err| err.to_string())?;
            client
                .shutdown(Shutdown::Write)
                .map_err(|err| err.to_string())?;

            let (entered_tx, entered_rx) = mpsc::channel();
            let gate = Arc::new(Barrier::new(2));
            let observed_cancel = Arc::new(AtomicBool::new(false));
            let dispatcher = HalfCloseDispatcher {
                entered: entered_tx,
                gate: Arc::clone(&gate),
                observed_cancel: Arc::clone(&observed_cancel),
            };
            let handle = thread::spawn(move || {
                handle_connection::<
                    TestRequestEnvelope,
                    u64,
                    TestResponseEnvelope,
                    u64,
                    HalfCloseDispatcher,
                >(
                    server,
                    &dispatcher,
                    &test_slots(),
                    test_policy(),
                    &AtomicBool::new(false),
                )
            });
            entered_rx
                .recv()
                .map_err(|err| format!("test must observe dispatcher entry: {err}"))?;
            // Several poll intervals pass with the half-close visible.
            thread::sleep(PeerWatch::POLL_INTERVAL.saturating_mul(4));
            let _wait = gate.wait();

            let response = decode_response::<TestResponseEnvelope, _>(&mut client)
                .map_err(|err| format!("a half-closed peer must still get its response: {err}"))?;
            if response
                != (TestResponseEnvelope {
                    request_id: 7,
                    payload: 4,
                })
            {
                return Err(format!("unexpected response: {response:?}"));
            }
            if observed_cancel.load(Ordering::Acquire) {
                return Err("a half-close must not cancel the budget".to_string());
            }
            let reason = handle
                .join()
                .map_err(|join_err| format!("server thread panicked: {join_err:?}"))?;
            if !matches!(reason, ConnectionCloseReason::PeerClosed) {
                return Err(format!("unexpected close reason: {reason:?}"));
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    #[test]
    fn a_peer_that_hangs_up_mid_dispatch_cancels_the_budget() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let frame = encode_test_frame(9, 1)?;
            client.write_all(&frame).map_err(|err| err.to_string())?;

            let (entered_tx, entered_rx) = mpsc::channel();
            let gate = Arc::new(Barrier::new(2));
            let observed_cancel = Arc::new(AtomicBool::new(false));
            let dispatcher = BlockingDispatcher {
                entered: entered_tx,
                gate: Arc::clone(&gate),
                observed_cancel: Arc::clone(&observed_cancel),
            };
            let handle = thread::spawn(move || {
                handle_connection::<
                    TestRequestEnvelope,
                    u64,
                    TestResponseEnvelope,
                    u64,
                    BlockingDispatcher,
                >(
                    server,
                    &dispatcher,
                    &test_slots(),
                    test_policy(),
                    &AtomicBool::new(false),
                )
            });

            entered_rx.recv().map_err(|err| {
                format!("test must observe request decode before closing peer: {err}")
            })?;
            drop(client);
            let _wait = gate.wait();

            let reason = handle
                .join()
                .map_err(|join_err| format!("server thread panicked: {join_err:?}"))?;
            if !observed_cancel.load(Ordering::Acquire) {
                return Err("dispatcher never saw the peer's hang-up on its budget".to_string());
            }
            if matches!(reason, ConnectionCloseReason::PeerClosed) {
                return Ok(());
            }
            Err(format!("unexpected close reason: {reason:?}"))
        })();
        assert_test_ok(&result);
    }
}
