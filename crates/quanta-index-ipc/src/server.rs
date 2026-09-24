//! `AF_UNIX` stream server + client.
//!
//! The accept loop hands every connection to its own thread, bounded by the
//! server's [`ServerAdmissionPolicy`]; each connection decodes one envelope
//! per frame, takes a dispatch slot, routes through the [`IpcDispatcher`]
//! supplied by the composition root under a per-request
//! [`RequestBudgetV1`], and writes the response back on the same connection.
//! A peer that hangs up mid-dispatch cancels that request's budget.
//!
//! Every accepted connection is first checked against the socket's
//! [`SocketAccessPolicy`] using the credentials the kernel reports for the
//! peer; a peer the policy does not admit is closed before a frame is read
//! and counted, never answered (QI-BB-014).
//!
//! Connection-fatal failures (framing, oversized, CBOR decode) close the
//! connection without writing a response. Request-domain failures (e.g.
//! `NOT_READY`, `INVALID_REQUEST`), a full dispatch queue and an oversized
//! response flow through as an `Error` variant in the response envelope.

use std::io::{ErrorKind, Read, Write};
use std::num::NonZeroU64;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use quanta_index_core::{
    RequestBudgetV1, RequestCorrelationV1, RequestProviderStageV1, RequestStageDiagnosticPortV1,
};

use crate::admission::{DispatchSlots, ServerAdmissionPolicy, SlotRefusal};
use crate::peer_credentials::{KernelPeerCredentials, PeerCredentialsSource};
use crate::socket_access::{PeerRefusal, SocketAccessPolicy, admit_peer};

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
use crate::counters::{IpcServerCounters, RequestEventSinkV1, RequestEventStageV1, RequestEventV1};
use crate::socket_access::PeerCredentials;

/// Which daemon plane one server serves (S21-10). The transport names it
/// so a dispatch context cannot misreport which socket carried a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpcPlane {
    Query,
    Control,
    Ingest,
}

/// The kernel-derived dispatch context the transport creates for every
/// request (S21-10 `DispatchContextV1`).
///
/// `principal` is what the kernel reported for the connected peer at
/// `accept` — never something the payload asserted. It is `None` only on
/// in-process call paths that bypass a socket; authorization treats that
/// as a missing credential context and refuses (default deny).
#[derive(Clone, Debug)]
pub struct DispatchContextV1 {
    /// The envelope's admitted request id (W10-R2): nonzero by
    /// construction — the connection loop refuses 0 before this context
    /// exists — correlated through diagnostics and typed responses.
    pub request_id: NonZeroU64,
    /// The plane whose socket carried the request.
    pub plane: IpcPlane,
    /// The peer's kernel-reported credentials, or `None` off-socket.
    pub principal: Option<PeerCredentials>,
    /// The effective uid the daemon bound the socket as. A peer running as
    /// this uid (or as root) is the operator; any other admitted peer is
    /// observe-only.
    pub owner_uid: u32,
    /// One-up id of the connection the request arrived on.
    pub connection_id: u64,
    /// The dispatch deadline the budget enforces.
    pub deadline: std::time::Instant,
    /// The cancellation handle that fires if the peer hangs up mid-flight.
    pub cancellation: quanta_index_core::CancelHandleV1,
    /// The one transport-owned, bounded diagnostic sink. It is not the
    /// provider usage ledger or a replacement for monotonic IPC counters.
    pub events: Arc<dyn RequestEventSinkV1>,
    /// Admission's monotonic start, shared with backend stage timings.
    pub request_started: Instant,
}

impl DispatchContextV1 {
    /// Emit a payload-free backend stage under the transport's validated ID.
    pub fn record_event_v1(&self, stage: RequestEventStageV1) {
        self.events.record_request_event_v1(RequestEventV1 {
            request_id: self.request_id,
            connection_id: self.connection_id,
            stage,
            elapsed_micros: elapsed_micros_v1(self.request_started),
        });
    }
}

fn elapsed_micros_v1(started: Instant) -> u64 {
    // The wire field is bounded; saturate only if the monotonic duration
    // cannot be represented in that field.
    let elapsed = started.elapsed();
    elapsed
        .as_secs()
        .saturating_mul(1_000_000)
        .saturating_add(u64::from(elapsed.subsec_micros()))
}

/// Emits one terminal event even when a dispatcher unwinds or a transport
/// branch returns early. Diagnostic loss is counted by the sink, never hidden.
struct RequestEventScope<'a> {
    sink: &'a IpcServerCounters,
    request_id: NonZeroU64,
    connection_id: u64,
    started: Instant,
    finished: bool,
}

impl<'a> RequestEventScope<'a> {
    fn new(sink: &'a IpcServerCounters, request_id: NonZeroU64, connection_id: u64) -> Self {
        let scope = Self {
            sink,
            request_id,
            connection_id,
            started: Instant::now(),
            finished: false,
        };
        scope.emit(RequestEventStageV1::Validated);
        scope
    }

    fn emit(&self, stage: RequestEventStageV1) {
        self.sink.record_request_event_v1(RequestEventV1 {
            request_id: self.request_id,
            connection_id: self.connection_id,
            stage,
            elapsed_micros: elapsed_micros_v1(self.started),
        });
    }

    fn finish(&mut self, stage: RequestEventStageV1) {
        self.emit(stage);
        self.finished = true;
    }
}

impl Drop for RequestEventScope<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.emit(if std::thread::panicking() {
                RequestEventStageV1::Panicked
            } else {
                RequestEventStageV1::Aborted
            });
        }
    }
}

/// Per-request identity bridge into the same IPC ring. The provider boundary
/// receives this through its existing budget; it cannot allocate another ID
/// or write a second diagnostic/usage ledger.
#[derive(Debug)]
struct ProviderEventBridgeV1 {
    sink: Arc<IpcServerCounters>,
    request_id: NonZeroU64,
    connection_id: u64,
    started: Instant,
}

impl RequestStageDiagnosticPortV1 for ProviderEventBridgeV1 {
    fn record_provider_stage_v1(&self, stage: RequestProviderStageV1) {
        let stage = match stage {
            RequestProviderStageV1::Started { ticket_id } => {
                RequestEventStageV1::ProviderStarted { ticket_id }
            }
            RequestProviderStageV1::Returned { ticket_id } => {
                RequestEventStageV1::ProviderReturned { ticket_id }
            }
            RequestProviderStageV1::IngestWindowStarted { window_ordinal } => {
                RequestEventStageV1::IngestWindowStarted { window_ordinal }
            }
            RequestProviderStageV1::IngestWindowReturned { window_ordinal } => {
                RequestEventStageV1::IngestWindowReturned { window_ordinal }
            }
        };
        self.sink.record_request_event_v1(RequestEventV1 {
            request_id: self.request_id,
            connection_id: self.connection_id,
            stage,
            elapsed_micros: elapsed_micros_v1(self.started),
        });
    }
}

/// Dispatch hook supplied by the composition root.
///
/// Receives the transport's dispatch context, a fully-parsed request
/// payload and the budget, and returns a fully-typed response payload.
/// Domain errors MUST be surfaced through the response type rather than
/// panicking.
pub trait IpcDispatcher<Request, Response>: Send + Sync {
    /// Handle one request under its budget. The budget's deadline is the
    /// server's dispatch budget from admission; its cancellation fires if the
    /// peer disconnects while this call runs. Implementations check it at
    /// their own boundaries and answer with a typed interruption.
    fn dispatch(
        &self,
        context: &DispatchContextV1,
        request: Request,
        budget: &RequestBudgetV1,
    ) -> Response;
}

pub trait RequestEnvelope<Request>: serde::de::DeserializeOwned + Send + Sync + 'static {
    fn into_parts(self) -> (u64, Request);

    /// Split the envelope and prove its request id is admissible
    /// (W10-R2): 0 is a malformed envelope refused with a typed error
    /// before admission, dispatch and any response. The single gate every
    /// plane's connection loop calls; the wire shape stays `u64`.
    fn validated(self) -> Result<(NonZeroU64, Request), IpcError> {
        let (request_id, payload) = self.into_parts();
        let admitted = NonZeroU64::new(request_id).ok_or(IpcError::ZeroRequestId)?;
        Ok((admitted, payload))
    }

    /// The repository this request is scoped to, for the per-repository
    /// in-flight cap (QI-BB-002); `None` for a request that names no
    /// repository, which only the global slot bound applies to.
    fn repo_scope(request: &Request) -> Option<String>;
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
    /// server's queue wait, or the request's repository held its cap for
    /// that long (QI-BB-002). `None` closes the connection.
    fn overloaded(request_id: u64, refusal: &SlotRefusal) -> Option<Self>
    where
        Self: Sized;
}

/// The repository a pinned or selected generation names.
fn pinned_repo_scope(
    pin: Option<&quanta_index_contract::GenerationPin>,
    selector: Option<&quanta_index_contract::GenerationSelector>,
) -> Option<String> {
    if let Some(pin) = pin {
        return Some(pin.repo_id.as_str().to_string());
    }
    match selector? {
        quanta_index_contract::GenerationSelector::Active { repo_id, .. }
        | quanta_index_contract::GenerationSelector::ResolvedActive { repo_id, .. } => {
            Some(repo_id.as_str().to_string())
        }
        quanta_index_contract::GenerationSelector::Pinned(pin) => {
            Some(pin.repo_id.as_str().to_string())
        }
    }
}

impl RequestEnvelope<quanta_index_contract::SearchPlaneQueryIpcRequest>
    for SearchPlaneQueryIpcRequestEnvelope
{
    fn into_parts(self) -> (u64, quanta_index_contract::SearchPlaneQueryIpcRequest) {
        (self.request_id, self.payload)
    }

    /// Every query route names its repository through a generation pin or
    /// selector (the repo-map route names it directly); a request that
    /// names none is refused by its route and is admitted unscoped here.
    fn repo_scope(request: &quanta_index_contract::SearchPlaneQueryIpcRequest) -> Option<String> {
        use quanta_index_contract::SearchPlaneQueryIpcRequest as Request;
        match request {
            Request::ResolveActiveGeneration(resolve) => Some(resolve.repo_id.as_str().to_string()),
            Request::ResolveLexicalGeneration(query) => pinned_repo_scope(
                query.generation.as_ref(),
                query.generation_selector.as_ref(),
            ),
            Request::Text(text) => {
                pinned_repo_scope(text.generation.as_ref(), text.generation_selector.as_ref())
            }
            Request::Symbol(symbol) => pinned_repo_scope(
                symbol.generation.as_ref(),
                symbol.generation_selector.as_ref(),
            ),
            Request::Semantic(semantic) => pinned_repo_scope(
                semantic.generation.as_ref(),
                semantic.generation_selector.as_ref(),
            ),
            Request::Hybrid(hybrid) => pinned_repo_scope(
                hybrid
                    .generation
                    .as_ref()
                    .or(hybrid.text_query.generation.as_ref()),
                hybrid
                    .generation_selector
                    .as_ref()
                    .or(hybrid.text_query.generation_selector.as_ref()),
            ),
            Request::HybridSeed(seed) => pinned_repo_scope(
                seed.generation
                    .as_ref()
                    .or(seed.text_query.generation.as_ref()),
                seed.generation_selector
                    .as_ref()
                    .or(seed.text_query.generation_selector.as_ref()),
            ),
            Request::History(history) => pinned_repo_scope(
                history.text_query.generation.as_ref(),
                history.text_query.generation_selector.as_ref(),
            ),
            Request::RuntimeMetadata(runtime) => pinned_repo_scope(
                runtime.text_query.generation.as_ref(),
                runtime.text_query.generation_selector.as_ref(),
            ),
            Request::Structural(structural) => pinned_repo_scope(
                structural.text_query.generation.as_ref(),
                structural.text_query.generation_selector.as_ref(),
            ),
            Request::RepoMapQuery(repo_map) => Some(repo_map.repo_id.as_str().to_string()),
            Request::Explain(explain) => Some(explain.generation.repo_id.as_str().to_string()),
            Request::ClusterMembershipRead(read) => {
                Some(read.generation.repo_id.as_str().to_string())
            }
        }
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

    fn overloaded(request_id: u64, refusal: &SlotRefusal) -> Option<Self> {
        Some(Self {
            request_id,
            payload: quanta_index_contract::SearchPlaneQueryIpcResponse::Error(refusal.ipc_error()),
        })
    }
}

impl RequestEnvelope<quanta_index_contract::SearchPlaneControlIpcRequest>
    for SearchPlaneControlIpcRequestEnvelope
{
    fn into_parts(self) -> (u64, quanta_index_contract::SearchPlaneControlIpcRequest) {
        (self.request_id, self.payload)
    }

    /// Control runs one dispatch at a time by policy; a per-repository
    /// scope would never bind, so control requests are admitted unscoped.
    fn repo_scope(
        _request: &quanta_index_contract::SearchPlaneControlIpcRequest,
    ) -> Option<String> {
        None
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

    fn overloaded(request_id: u64, refusal: &SlotRefusal) -> Option<Self> {
        Some(Self {
            request_id,
            payload: quanta_index_contract::SearchPlaneControlIpcResponse::Error(
                refusal.ipc_error(),
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

    /// Ingest runs one dispatch at a time by policy; see the control
    /// envelope.
    fn repo_scope(_request: &quanta_index_contract::SearchPlaneIngestIpcRequest) -> Option<String> {
        None
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

    fn overloaded(request_id: u64, refusal: &SlotRefusal) -> Option<Self> {
        Some(Self {
            request_id,
            payload: quanta_index_contract::SearchPlaneIngestIpcResponse::Error(
                refusal.ipc_error(),
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
    /// Who the socket admits (QI-BB-014): the file mode was set from it at
    /// bind, and every accepted peer is checked against it before a frame
    /// is read.
    access: SocketAccessPolicy,
    /// The effective uid this server bound as; the owner every policy
    /// admits.
    owner: u32,
    /// Where the accept loop learns each peer's credentials.
    peer_source: Arc<dyn PeerCredentialsSource>,
    /// What this server has counted since bind (QI-BB-015): live
    /// connections, which the accept loop refuses past the policy's cap
    /// instead of queueing without bound, and every admission outcome.
    counters: Arc<IpcServerCounters>,
    /// One-up connection ids, so every dispatch context names the
    /// connection it arrived on (S21-10).
    next_connection_id: std::sync::atomic::AtomicU64,
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

/// Group and other write bits.
const OTHERS_WRITE_BITS: u32 = 0o022;
/// The sticky bit: in a shared directory, only an entry's owner may unlink
/// or rename it.
const STICKY_BIT: u32 = 0o1000;
/// The group traverse bit.
const GROUP_TRAVERSE_BIT: u32 = 0o010;
/// The other traverse bit.
const OTHERS_TRAVERSE_BIT: u32 = 0o001;
/// The permission and special bits of a mode.
const MODE_BITS: u32 = 0o7777;

fn insecure(path: &Path, reason: impl Into<String>) -> IpcError {
    IpcError::SocketPathInsecure {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn unsatisfiable(path: &Path, reason: impl Into<String>) -> IpcError {
    IpcError::SocketAccessUnsatisfiable {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

/// Refuse a shared policy whose group this process is not a member of.
///
/// The check reads the effective gid and the supplementary groups; it is
/// what `chown` would enforce for an unprivileged process, made explicit
/// so root gets the same answer and the refusal names the group.
fn ensure_group_membership(path: &Path, access: &SocketAccessPolicy) -> Result<(), IpcError> {
    let Some(group) = access.group() else {
        return Ok(());
    };
    if rustix::process::getegid().as_raw() == group {
        return Ok(());
    }
    let supplementary = rustix::process::getgroups()
        .map_err(std::io::Error::from)
        .map_err(IpcError::Io)?;
    if supplementary.iter().any(|gid| gid.as_raw() == group) {
        return Ok(());
    }
    Err(unsatisfiable(
        path,
        format!(
            "this process is not a member of gid {group}, so it cannot share a socket with that group"
        ),
    ))
}

/// Create the socket directory for `access`, or verify a pre-existing one.
///
/// A directory this process creates gets the policy's mode (`0700`
/// private, `0710` group-shared, `0711` uid-shared) and, for a group-shared
/// socket, the group; every missing component is created and pinned
/// individually so the umask cannot strip a traverse bit. A pre-existing
/// directory is judged by what it resolves to (`/tmp` is a symlink on some
/// systems; see the policy module doc): it passes when it is a directory
/// that either belongs to this user with no mode bit beyond the policy's
/// — so a directory a permissive umask left at `0755` is refused under a
/// private policy, not merely one others can write — or carries the
/// sticky bit (a shared
/// temporary directory, where others cannot unlink or rename this user's
/// socket). Anything else is refused typed.
fn ensure_socket_directory(
    parent: &Path,
    owner: u32,
    access: &SocketAccessPolicy,
) -> Result<(), IpcError> {
    match std::fs::metadata(parent) {
        Ok(metadata) => verify_existing_socket_directory(parent, owner, access, &metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            create_socket_directory_chain(parent, owner, access)
        }
        Err(error) => Err(IpcError::Io(error)),
    }
}

fn verify_existing_socket_directory(
    parent: &Path,
    owner: u32,
    access: &SocketAccessPolicy,
    metadata: &std::fs::Metadata,
) -> Result<(), IpcError> {
    if !metadata.is_dir() {
        return Err(insecure(parent, "socket directory is not a directory"));
    }
    let mode = metadata.mode() & MODE_BITS;
    let expected = access.directory_mode();
    // No bit outside the policy's mode: `0755` is wider than a private
    // `0700`; `0700` under a group policy is narrower, which the
    // reachability check refuses with the traversal it lacks.
    let owned_within_policy = metadata.uid() == owner && mode & !expected == 0;
    let sticky_shared = mode & STICKY_BIT != 0;
    if !(owned_within_policy || sticky_shared) {
        let reason = if mode & OTHERS_WRITE_BITS != 0 {
            format!(
                "socket directory is writable by others without the sticky bit (mode {mode:04o}, uid {}, this process runs as {owner}); it must be {expected:04o} and owned by this user, or sticky",
                metadata.uid()
            )
        } else {
            format!(
                "socket directory is wider than the policy (mode {mode:04o}, uid {}, this process runs as {owner}); it must be {expected:04o} and owned by this user (chmod {expected:o}), or sticky",
                metadata.uid()
            )
        };
        return Err(insecure(parent, reason));
    }
    Ok(())
}

/// Create every missing component of `parent`, each with the policy's
/// directory mode and group, and re-read each one to prove it.
fn create_socket_directory_chain(
    parent: &Path,
    owner: u32,
    access: &SocketAccessPolicy,
) -> Result<(), IpcError> {
    let mut missing: Vec<&Path> = Vec::new();
    let mut cursor = parent;
    loop {
        match std::fs::metadata(cursor) {
            Ok(_present) => break,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                missing.push(cursor);
                cursor = cursor.parent().ok_or_else(|| {
                    insecure(
                        parent,
                        "socket directory has no existing ancestor to create it under",
                    )
                })?;
            }
            Err(error) => return Err(IpcError::Io(error)),
        }
    }
    let mode = access.directory_mode();
    for component in missing.into_iter().rev() {
        std::fs::DirBuilder::new()
            .mode(mode)
            .create(component)
            .map_err(IpcError::Io)?;
        if let Some(group) = access.group() {
            std::os::unix::fs::chown(component, None, Some(group)).map_err(IpcError::Io)?;
        }
        // The umask applies at creation; the mode is pinned afterwards.
        std::fs::set_permissions(component, std::fs::Permissions::from_mode(mode))
            .map_err(IpcError::Io)?;
        let created = std::fs::metadata(component).map_err(IpcError::Io)?;
        let group_matches = access.group().is_none_or(|group| created.gid() == group);
        if created.uid() != owner || created.mode() & MODE_BITS != mode || !group_matches {
            return Err(insecure(
                component,
                format!(
                    "socket directory came back with mode {:04o}, uid {} and gid {} after creation (wanted mode {mode:04o}, uid {owner}, gid {:?})",
                    created.mode() & MODE_BITS,
                    created.uid(),
                    created.gid(),
                    access.group()
                ),
            ));
        }
    }
    Ok(())
}

/// Prove the peers a shared policy admits can reach the socket: every
/// directory from the socket's (resolved) parent up to the root must be
/// traversable by them.
///
/// A uid-shared socket needs the other-traverse bit on each, since the
/// listed users' groups are not known here. A group-shared socket accepts
/// either that bit or the group-traverse bit on a directory of the shared
/// group. The state root above the default socket directory is created
/// `0700`; sharing a socket there needs the operator to open its traverse
/// bit (its write bits stay owner-only) or to place the socket elsewhere.
/// Nothing is widened here: a policy the filesystem contradicts is refused.
fn ensure_socket_path_reachable(
    parent: &Path,
    access: &SocketAccessPolicy,
) -> Result<(), IpcError> {
    if !access.admits_others() {
        return Ok(());
    }
    let resolved = std::fs::canonicalize(parent).map_err(IpcError::Io)?;
    let mut cursor: Option<&Path> = Some(resolved.as_path());
    while let Some(directory) = cursor {
        let metadata = std::fs::metadata(directory).map_err(IpcError::Io)?;
        let mode = metadata.mode() & MODE_BITS;
        let by_others = mode & OTHERS_TRAVERSE_BIT != 0;
        let by_group = access
            .group()
            .is_some_and(|group| metadata.gid() == group && mode & GROUP_TRAVERSE_BIT != 0);
        if !(by_others || by_group) {
            let group_hint = access
                .group()
                .map_or_else(String::new, |group| format!(", or gid {group} with g+x"));
            return Err(unsatisfiable(
                directory,
                format!(
                    "directory mode {mode:04o} gid {} cannot be traversed by the peers the policy admits (needs o+x{group_hint}); open its traverse bit or place the socket under a directory the peers can reach",
                    metadata.gid()
                ),
            ));
        }
        cursor = directory.parent();
    }
    Ok(())
}

/// Assign the bound socket file its group and mode, then prove both.
fn apply_socket_file_access(path: &Path, access: &SocketAccessPolicy) -> Result<(), IpcError> {
    if let Some(group) = access.group() {
        std::os::unix::fs::chown(path, None, Some(group)).map_err(IpcError::Io)?;
    }
    let mode = access.socket_mode();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(IpcError::Io)?;
    let applied = std::fs::symlink_metadata(path).map_err(IpcError::Io)?;
    let group_matches = access.group().is_none_or(|group| applied.gid() == group);
    if applied.mode() & MODE_BITS != mode || !group_matches {
        return Err(insecure(
            path,
            format!(
                "socket came back with mode {:04o} and gid {} (wanted mode {mode:04o}, gid {:?})",
                applied.mode() & MODE_BITS,
                applied.gid(),
                access.group()
            ),
        ));
    }
    Ok(())
}

/// Leave a live socket alone and reclaim a stale one.
///
/// A socket file at the path is probed with a connect. Success means a
/// listener is alive there: `SOCKET_IN_USE`, and the probe connection is
/// dropped without a frame (the listener sees a peer hang up, which it
/// already handles). A refused connection means no listener: the file is
/// stale, and it is removed — but only if the inode seen before the probe
/// is still the one at the path, so a listener that bound between probe
/// and unlink is not evicted. A socket owned by another user is never
/// touched. A regular file or directory at the path is refused as before.
fn reclaim_socket_path(path: &Path, owner: u32) -> Result<(), IpcError> {
    let before = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(IpcError::Io(error)),
    };
    if !before.file_type().is_socket() {
        return Err(insecure(
            path,
            "path exists and is not a socket; refusing to remove it",
        ));
    }
    if before.uid() != owner {
        return Err(insecure(
            path,
            format!(
                "socket is owned by uid {} and this process runs as {owner}",
                before.uid()
            ),
        ));
    }
    match UnixStream::connect(path) {
        Ok(probe) => {
            drop(probe);
            return Err(IpcError::SocketInUse(path.to_path_buf()));
        }
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => {}
        Err(error) => return Err(IpcError::Io(error)),
    }
    let after = std::fs::symlink_metadata(path).map_err(IpcError::Io)?;
    if after.dev() != before.dev() || after.ino() != before.ino() {
        return Err(IpcError::SocketInUse(path.to_path_buf()));
    }
    std::fs::remove_file(path).map_err(IpcError::Io)
}

impl UdsServer {
    /// Bind a private listener at `path` under
    /// [`ServerAdmissionPolicy::DEFAULT`].
    pub fn bind(path: &Path) -> Result<Self, IpcError> {
        Self::bind_with_policy(path, ServerAdmissionPolicy::DEFAULT)
    }

    /// Bind a private listener at `path` under `policy`, with counters no
    /// scrape sees (plane `unnamed`).
    pub fn bind_with_policy(path: &Path, policy: ServerAdmissionPolicy) -> Result<Self, IpcError> {
        Self::bind_observed(
            path,
            policy,
            SocketAccessPolicy::Private,
            Arc::new(IpcServerCounters::for_plane("unnamed")),
        )
    }

    /// Bind a listener at `path` under `policy` and `access`, counting into
    /// `counters`.
    ///
    /// The caller keeps a handle to the counters so they can be registered
    /// with a scrape before any connection exists (QI-BB-015). Peers are
    /// identified by the kernel's own report. The socket is alone in its
    /// directory: the directory is held to this socket's own policy.
    pub fn bind_observed(
        path: &Path,
        policy: ServerAdmissionPolicy,
        access: SocketAccessPolicy,
        counters: Arc<IpcServerCounters>,
    ) -> Result<Self, IpcError> {
        let directory_access = access.clone();
        Self::bind_with_peer_source(
            path,
            policy,
            access,
            &directory_access,
            counters,
            Arc::new(KernelPeerCredentials),
        )
    }

    /// [`Self::bind_observed`] for a socket that shares its directory with
    /// sockets of other policies.
    ///
    /// The directory is created and verified under `directory_access`,
    /// the widest of them ([`SocketAccessPolicy::widest`]), while this
    /// socket's own file and peer check follow `access`.
    pub fn bind_observed_in(
        path: &Path,
        policy: ServerAdmissionPolicy,
        access: SocketAccessPolicy,
        directory_access: &SocketAccessPolicy,
        counters: Arc<IpcServerCounters>,
    ) -> Result<Self, IpcError> {
        Self::bind_with_peer_source(
            path,
            policy,
            access,
            directory_access,
            counters,
            Arc::new(KernelPeerCredentials),
        )
    }

    /// Bind a listener at `path` (QI-BB-014), learning each accepted peer's
    /// credentials from `peer_source`.
    ///
    /// Before anything is bound, a shared policy is checked against this
    /// process: it must be a member of the group it names, and every
    /// directory on the socket's path must be traversable by the peers the
    /// policy admits. The directory the socket lives in is created with
    /// `directory_access`'s mode (and group) when absent and verified when
    /// present: a real directory, owned by this process's user, no wider
    /// than that mode (or a sticky shared directory, where others cannot
    /// unlink what they do not own). A socket already at the path is probed: one
    /// that answers belongs to a live listener and is left alone under
    /// `SOCKET_IN_USE`; one that refuses connections is stale and reclaimed,
    /// but only if it is still the same inode after the probe, so a listener
    /// that binds in between keeps its path. The bound socket is then
    /// assigned the policy's group and mode (`0600`, `0660` or `0666`).
    ///
    /// `peer_source` is a port so a test can script the peer the accept
    /// loop observes; production binds through [`Self::bind_observed`].
    pub fn bind_with_peer_source(
        path: &Path,
        policy: ServerAdmissionPolicy,
        access: SocketAccessPolicy,
        directory_access: &SocketAccessPolicy,
        counters: Arc<IpcServerCounters>,
        peer_source: Arc<dyn PeerCredentialsSource>,
    ) -> Result<Self, IpcError> {
        let owner = rustix::process::geteuid().as_raw();
        ensure_group_membership(path, &access)?;
        ensure_group_membership(path, directory_access)?;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            ensure_socket_directory(parent, owner, directory_access)?;
            ensure_socket_path_reachable(parent, &access)?;
        }
        reclaim_socket_path(path, owner)?;
        let listener = UnixListener::bind(path).map_err(IpcError::Io)?;
        apply_socket_file_access(path, &access)?;
        let socket_path_identity = SocketPathIdentity::capture(path).map_err(IpcError::Io)?;
        listener.set_nonblocking(true).map_err(IpcError::Io)?;
        Ok(Self {
            listener,
            socket_path: path.to_path_buf(),
            socket_path_identity,
            shutdown: Arc::new(AtomicBool::new(false)),
            policy,
            access,
            owner,
            peer_source,
            counters,
            next_connection_id: std::sync::atomic::AtomicU64::new(1),
        })
    }

    #[must_use]
    pub const fn admission_policy(&self) -> ServerAdmissionPolicy {
        self.policy
    }

    /// Who this socket admits (QI-BB-014).
    #[must_use]
    pub const fn socket_access_policy(&self) -> &SocketAccessPolicy {
        &self.access
    }

    /// Screen one accepted connection before anything is read from it.
    ///
    /// The peer's credentials come from the server's source; a peer the
    /// access policy does not admit, or one whose credentials the kernel
    /// did not report, is refused. Only then is the live-connection cap
    /// consulted, so the cap counts admitted peers alone.
    fn screen(&self, stream: &UnixStream, max_connections: u64) -> AcceptOutcome {
        let credentials = match self.peer_source.peer_credentials(stream) {
            Ok(credentials) => credentials,
            Err(error) => return AcceptOutcome::PeerUnreadable(error),
        };
        if let Err(refusal) = admit_peer(&self.access, credentials, self.owner) {
            return AcceptOutcome::PeerRefused(refusal);
        }
        if self.counters.connections_live() >= max_connections {
            return AcceptOutcome::CapReached;
        }
        // S21-10: the admitted peer's kernel credentials are preserved —
        // screening must not discard what operation authorization needs.
        AcceptOutcome::Admitted(credentials)
    }

    /// Connections the accept loop closed because the cap was reached.
    #[must_use]
    pub fn refused_connections(&self) -> u64 {
        self.counters.snapshot().connections_refused
    }

    /// Everything this server has counted since bind (QI-BB-015).
    #[must_use]
    pub fn counters(&self) -> Arc<IpcServerCounters> {
        Arc::clone(&self.counters)
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
    /// Every accepted connection is screened against the socket's access
    /// policy first (QI-BB-014): a peer that is not admitted, or whose
    /// credentials the kernel did not report, is closed without a frame
    /// being read and counted. Every admitted connection runs on its own
    /// thread, so reading one peer's request never waits on another's;
    /// dispatch concurrency is bounded separately by the policy's slots. The listener is
    /// non-blocking, so an empty accept queue sleeps `accept_idle` before
    /// retrying. On shutdown the loop stops accepting and joins the
    /// connection threads, each of which finishes its in-flight request
    /// (bounded by the dispatch budget and I/O timeouts) before exiting.
    pub fn run<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
        &self,
        dispatcher: &Arc<D>,
        plane: IpcPlane,
        accept_idle: Duration,
    ) -> Result<(), IpcError>
    where
        RequestEnvelopeT: RequestEnvelope<Request>,
        ResponseEnvelopeT: ResponseEnvelope<Response>,
        D: IpcDispatcher<Request, Response> + ?Sized + 'static,
    {
        let slots = Arc::new(DispatchSlots::for_policy(self.policy));
        let max_connections =
            u64::try_from(self.policy.max_connections()).map_or(u64::MAX, |cap| cap);
        let mut connection_threads: Vec<std::thread::JoinHandle<ConnectionCloseReason>> =
            Vec::new();
        while !self.shutdown.load(Ordering::Acquire) {
            // Finished connection handles are joined, never dropped
            // unjoined (S21-09): after `is_finished` the join is bounded
            // teardown, and a panicked connection is observed here.
            let mut still_running: Vec<std::thread::JoinHandle<ConnectionCloseReason>> = Vec::new();
            for handle in std::mem::take(&mut connection_threads) {
                if handle.is_finished() {
                    let _finished = handle.join();
                } else {
                    still_running.push(handle);
                }
            }
            connection_threads = still_running;
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    let admitted = match self.screen(&stream, max_connections) {
                        AcceptOutcome::Admitted(credentials) => credentials,
                        AcceptOutcome::PeerRefused(_refusal) => {
                            self.counters.peer_refused();
                            drop(stream);
                            continue;
                        }
                        AcceptOutcome::PeerUnreadable(_error) => {
                            self.counters.peer_credentials_unreadable();
                            drop(stream);
                            continue;
                        }
                        AcceptOutcome::CapReached => {
                            self.counters.connection_refused();
                            drop(stream);
                            continue;
                        }
                    };
                    // The live-connection permit is RAII (S21-09): a
                    // panicking dispatcher unwinds through its drop and
                    // the live count returns to its baseline.
                    let live = crate::counters::LiveConnectionGuard::admit(&self.counters);
                    let dispatcher = Arc::clone(dispatcher);
                    let slots = Arc::clone(&slots);
                    let counters = Arc::clone(&self.counters);
                    let shutdown = Arc::clone(&self.shutdown);
                    let policy = self.policy;
                    let owner_uid = self.owner;
                    let connection_id = self
                        .next_connection_id
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
                                stream,
                                dispatcher.as_ref(),
                                &slots,
                                policy,
                                plane,
                                admitted,
                                owner_uid,
                                connection_id,
                                &shutdown,
                                &counters,
                            );
                            drop(live);
                            reason
                        });
                    match spawned {
                        Ok(handle) => connection_threads.push(handle),
                        // The un-moved guard releases the live count here.
                        Err(err) => return Err(IpcError::Io(err)),
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

/// What the accept loop decided about one connection before reading it.
#[derive(Debug)]
enum AcceptOutcome {
    /// The peer is admitted; its kernel-reported credentials are carried
    /// into the connection's dispatch contexts (S21-10).
    Admitted(PeerCredentials),
    /// The access policy does not admit this peer; closed, counted, and
    /// never named anywhere but this value.
    PeerRefused(PeerRefusal),
    /// The kernel did not report the peer's credentials; closed and counted
    /// as such, never admitted.
    PeerUnreadable(std::io::Error),
    /// The live-connection cap is reached; closed and counted.
    CapReached,
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
    /// The peer watch could not be armed, or its thread failed while
    /// dispatching. Neither condition can be reported as a clean stop.
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
            Self::PeerWatchFailed(message) => write!(f, "peer watch failed: {message}"),
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

#[expect(
    clippy::too_many_arguments,
    reason = "the transport threads the connection's kernel identity (plane, peer credentials, owner uid, connection id) plus admission and observability handles; bundling them would hide which identity a context was built from"
)]
fn handle_connection<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
    mut stream: UnixStream,
    dispatcher: &D,
    slots: &DispatchSlots,
    policy: ServerAdmissionPolicy,
    plane: IpcPlane,
    principal: PeerCredentials,
    owner_uid: u32,
    connection_id: u64,
    shutdown: &AtomicBool,
    counters: &Arc<IpcServerCounters>,
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
            Err(err) => {
                counters.request_decode_failed();
                return ConnectionCloseReason::RequestDecodeFailed(err);
            }
        };
        // W10-R2: the id gate runs before shutdown, admission and
        // dispatch alike — a 0 envelope is malformed, refused typed, and
        // answered with nothing, exactly like a corrupt frame.
        let (request_id, request_payload) = match request.validated() {
            Ok(parts) => parts,
            Err(err) => {
                counters.request_decode_failed();
                return ConnectionCloseReason::RequestDecodeFailed(err);
            }
        };
        let mut event_scope = RequestEventScope::new(counters, request_id, connection_id);
        // A request that arrives during shutdown is not dispatched; the
        // connection closes so the peer retries against the next process.
        if shutdown.load(Ordering::Acquire) {
            counters.request_refused_shutting_down();
            event_scope.finish(RequestEventStageV1::ShuttingDown);
            return ConnectionCloseReason::ShuttingDown;
        }
        // Admission: ingress happened above, now the dispatch slot, global
        // and per repository. A full queue is a typed answer, not an
        // unbounded wait.
        let repo_scope = RequestEnvelopeT::repo_scope(&request_payload);
        let permit = match slots.acquire(policy.queue_wait(), repo_scope.as_deref()) {
            Ok(permit) => permit,
            Err(refusal) => {
                counters.request_overloaded(refusal.is_repo_scoped());
                event_scope.emit(if refusal.is_repo_scoped() {
                    RequestEventStageV1::QueueRefusedRepository
                } else {
                    RequestEventStageV1::QueueRefusedGlobal
                });
                let Some(response) = ResponseEnvelopeT::overloaded(request_id.get(), &refusal)
                else {
                    event_scope.finish(RequestEventStageV1::Aborted);
                    return ConnectionCloseReason::Overloaded {
                        waited: refusal.waited(),
                    };
                };
                match write_response(&mut stream, &response, counters) {
                    Ok(()) => {
                        event_scope.finish(RequestEventStageV1::ResponseWritten);
                        continue;
                    }
                    Err(reason) => {
                        event_scope.finish(match reason {
                            ConnectionCloseReason::ResponseEncodeFailed(_) => {
                                RequestEventStageV1::ResponseEncodeFailed
                            }
                            ConnectionCloseReason::ResponseWriteFailed(_) => {
                                RequestEventStageV1::ResponseWriteFailed
                            }
                            ConnectionCloseReason::BlockingModeConfigFailed(_)
                            | ConnectionCloseReason::TimeoutConfigFailed(_)
                            | ConnectionCloseReason::PeerClosed
                            | ConnectionCloseReason::RequestDecodeFailed(_)
                            | ConnectionCloseReason::Overloaded { .. }
                            | ConnectionCloseReason::ShuttingDown
                            | ConnectionCloseReason::PeerWatchFailed(_) => {
                                RequestEventStageV1::Aborted
                            }
                        });
                        return reason;
                    }
                }
            }
        };
        event_scope.emit(RequestEventStageV1::QueueAdmitted);
        // W10-R2: the admitted id rides the budget so routes, typed
        // responses and provider audit correlate without an envelope.
        let diagnostics: Arc<dyn RequestStageDiagnosticPortV1> = Arc::new(ProviderEventBridgeV1 {
            sink: Arc::clone(counters),
            request_id,
            connection_id,
            started: event_scope.started,
        });
        let budget = RequestBudgetV1::for_duration(policy.dispatch_budget())
            .with_correlation(RequestCorrelationV1::from_admitted(request_id))
            .with_diagnostics(diagnostics);
        // S21-10: the transport builds the kernel-derived dispatch
        // context every dispatcher authorizes against. The principal is
        // the accept-time kernel report; the payload never asserts one.
        let event_sink: Arc<dyn RequestEventSinkV1> = counters.clone();
        let context = DispatchContextV1 {
            request_id,
            plane,
            principal: Some(principal),
            owner_uid,
            connection_id,
            deadline: std::time::Instant::now()
                .checked_add(policy.dispatch_budget())
                .unwrap_or_else(std::time::Instant::now),
            cancellation: budget.cancel_handle(),
            events: event_sink,
            request_started: event_scope.started,
        };
        // The dispatch slot and the in-flight count are one RAII pair
        // (S21-09): a panicking dispatcher unwinds through both drops,
        // and the peer watch stops and joins its thread the same way.
        let (response_payload, peer_hung_up) = {
            let _in_flight =
                crate::counters::InFlightDispatchGuard::start(counters, permit.waited());
            // A dispatch without a live watch would run with a cancellation
            // that can never fire; refusing the connection is the honest
            // alternative.
            let watch = match PeerWatch::arm(&stream, budget.cancel_handle(), Arc::clone(counters))
            {
                Ok(watch) => watch,
                Err(err) => {
                    event_scope.finish(RequestEventStageV1::PeerWatchFailed);
                    return ConnectionCloseReason::PeerWatchFailed(err.to_string());
                }
            };
            event_scope.emit(RequestEventStageV1::DispatchStarted);
            let response_payload = dispatcher.dispatch(&context, request_payload, &budget);
            event_scope.emit(RequestEventStageV1::DispatchReturned);
            let peer_hung_up = match watch.disarm() {
                Ok(PeerWatchOutcome::HungUp) => true,
                Ok(PeerWatchOutcome::Stopped) => false,
                Err(error) => {
                    event_scope.finish(RequestEventStageV1::PeerWatchFailed);
                    return ConnectionCloseReason::PeerWatchFailed(error);
                }
            };
            (response_payload, peer_hung_up)
        };
        drop(permit);
        counters.request_dispatched(peer_hung_up);
        if peer_hung_up {
            // Nothing to write to; the dispatcher already saw the
            // cancellation at its next checkpoint (or ran to completion).
            event_scope.finish(RequestEventStageV1::PeerCancelled);
            return ConnectionCloseReason::PeerClosed;
        }
        let response = ResponseEnvelopeT::from_parts(request_id.get(), response_payload);
        let frame = match encode_response(&response) {
            Ok(frame) => frame,
            // The answer was computed but cannot cross the wire. Tell the
            // caller so with a typed refusal instead of dropping the
            // connection, which would be indistinguishable from a crash.
            Err(IpcError::Oversized(encoded_bytes)) => {
                let limit_bytes =
                    u64::try_from(MAX_FRAME_BODY_BYTES).map_or(u64::MAX, |limit| limit);
                let Some(refusal) = ResponseEnvelopeT::result_too_large(
                    request_id.get(),
                    encoded_bytes,
                    limit_bytes,
                ) else {
                    event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                    return ConnectionCloseReason::ResponseEncodeFailed(IpcError::Oversized(
                        encoded_bytes,
                    ));
                };
                match encode_response(&refusal) {
                    Ok(frame) => frame,
                    Err(err) => {
                        event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                        return ConnectionCloseReason::ResponseEncodeFailed(err);
                    }
                }
            }
            Err(err) => {
                event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                return ConnectionCloseReason::ResponseEncodeFailed(err);
            }
        };
        if let Err(err) = stream.write_all(&frame) {
            event_scope.finish(RequestEventStageV1::ResponseWriteFailed);
            return ConnectionCloseReason::ResponseWriteFailed(err.to_string());
        }
        counters.response_written(frame_bytes(&frame));
        event_scope.finish(RequestEventStageV1::ResponseWritten);
        // continue: next request on same conn
    }
}

/// A frame's length as the byte count the scrape reports.
fn frame_bytes(frame: &[u8]) -> u64 {
    u64::try_from(frame.len()).map_or(u64::MAX, |bytes| bytes)
}

fn write_response<ResponseEnvelopeT: serde::Serialize>(
    stream: &mut UnixStream,
    response: &ResponseEnvelopeT,
    counters: &IpcServerCounters,
) -> Result<(), ConnectionCloseReason> {
    let frame = encode_response(response).map_err(ConnectionCloseReason::ResponseEncodeFailed)?;
    stream
        .write_all(&frame)
        .map_err(|err| ConnectionCloseReason::ResponseWriteFailed(err.to_string()))?;
    counters.response_written(frame_bytes(&frame));
    Ok(())
}

/// How one watch run ended: the two terminal reasons are distinct types,
/// never one boolean the caller has to interpret.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PeerWatchOutcome {
    /// The peer hung up while watched; its budget was cancelled.
    HungUp,
    /// The watch stopped with the peer still able to receive.
    Stopped,
}

/// The watch thread's lifecycle, observed without touching production
/// state (TOPT-02 / R4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WatchEvent {
    /// The watcher completed its first observation and stays watching.
    /// A first observation that already sees a hang-up reports
    /// `PeerDisconnected` instead, and a stop that wins before the first
    /// observation reports `Stopped`: `Armed` means the watch is live.
    Armed,
    /// The watcher confirmed a hang-up and completed budget cancellation.
    PeerDisconnected,
    /// The watcher stopped with the peer still present.
    Stopped,
    /// The owner reaped the watcher thread.
    Joined,
}

/// A bounded, never-blocking sink for [`WatchEvent`].
///
/// Production arms its watches with [`WatchObserver::null`], whose sends
/// go nowhere; tests arm with [`WatchObserver::channel`] and wait on the
/// receiver instead of sleeping out poll intervals. The observer only
/// receives events — it cannot mutate the watch.
#[derive(Clone, Default)]
struct WatchObserver {
    events: Option<mpsc::SyncSender<WatchEvent>>,
}

impl WatchObserver {
    fn null() -> Self {
        Self { events: None }
    }

    #[cfg(test)]
    fn channel() -> (Self, mpsc::Receiver<WatchEvent>) {
        // Four lifecycle events per watch; the bound only contains a test
        // that stopped draining.
        let (events, received) = mpsc::sync_channel(16);
        (
            Self {
                events: Some(events),
            },
            received,
        )
    }

    fn fire(&self, event: WatchEvent) {
        if let Some(events) = &self.events {
            let _dropped = events.try_send(event);
        }
    }
}

/// Watches a connection for a hang-up while its request is dispatching.
///
/// The dispatch runs synchronously on the connection thread, so a second
/// thread polls the socket. Readable with data means the peer pipelined
/// its next request; that is not a hang-up, but the peer may still leave
/// before its answer, so the watch keeps asking whether it can receive
/// instead of polling (the pipelined bytes keep the socket readable).
/// `POLLHUP`, `POLLERR` or an end-of-file mean the peer at least shut its
/// write side — which is *not* yet a hang-up: a peer that sent its request
/// and half-closed is still waiting for the response. The watch confirms a
/// hang-up by asking whether the peer can still receive (a zero-byte send,
/// which fails with `EPIPE` only once the peer's read side is gone), and
/// after a half-close or a pipelined frame it keeps asking at the probe
/// cadence. The watch ends when the dispatch returns.
///
/// Stopping is event-driven: the watched poll set carries a wake FD beside
/// the peer socket, and disarming signals it before joining, so the join
/// never waits out a poll quantum. The poll timeout and the probe cadence
/// remain only as a defensive fallback — the kernel reports no event when
/// a half-closed peer's read side goes away, so the send probe is still
/// what notices that transition.
struct PeerWatch {
    stop: Arc<AtomicBool>,
    hung_up: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The write end of the wake pair; the watcher thread owns the read
    /// end. Both close exactly once with their owners.
    wake: Option<UnixStream>,
    observer: WatchObserver,
}

impl PeerWatch {
    /// Defensive poll/probe bound: `poll` returns at once on peer events
    /// and on the wake FD, so this only paces the send probe after a
    /// half-close and bounds a stuck poll.
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    fn arm(
        stream: &UnixStream,
        cancel: quanta_index_core::CancelHandleV1,
        counters: Arc<IpcServerCounters>,
    ) -> std::io::Result<Self> {
        Self::arm_with_observer(stream, cancel, counters, WatchObserver::null())
    }

    fn arm_with_observer(
        stream: &UnixStream,
        cancel: quanta_index_core::CancelHandleV1,
        counters: Arc<IpcServerCounters>,
        observer: WatchObserver,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let hung_up = Arc::new(AtomicBool::new(false));
        let watched = stream.try_clone()?;
        let (wake_read, wake_write) = UnixStream::pair()?;
        let thread = {
            let stop = Arc::clone(&stop);
            let hung_up = Arc::clone(&hung_up);
            let observer = observer.clone();
            std::thread::Builder::new()
                .name("uds-peer-watch".to_string())
                .spawn(move || {
                    // Once the socket stays readable (a half-close or a
                    // pipelined frame), polling would spin; the send probe
                    // alone tells a waiting peer from a departed one. The
                    // probe wait is a wake-FD poll, so a stop still lands
                    // at once instead of at the end of a sleep.
                    let mut probe_only = false;
                    let mut armed = false;
                    loop {
                        if stop.load(Ordering::Acquire) {
                            observer.fire(WatchEvent::Stopped);
                            return;
                        }
                        if probe_only {
                            if stop_signalled(&wake_read) && stop.load(Ordering::Acquire) {
                                observer.fire(WatchEvent::Stopped);
                                return;
                            }
                            if peer_can_receive(&watched) {
                                continue;
                            }
                            hung_up.store(true, Ordering::Release);
                            counters.peer_hangup_detected();
                            cancel.cancel();
                            observer.fire(WatchEvent::PeerDisconnected);
                            return;
                        }
                        match peer_state(&watched, &wake_read) {
                            PeerState::Alive => {}
                            PeerState::HalfClosed | PeerState::Pipelined => probe_only = true,
                            PeerState::StopRequested => {
                                if stop.load(Ordering::Acquire) {
                                    observer.fire(WatchEvent::Stopped);
                                    return;
                                }
                                // A wake byte with no stop is impossible —
                                // the only writer sets the flag first — so
                                // drain defensively and keep watching rather
                                // than spin on the readable FD.
                                drain_wake_byte(&wake_read);
                                continue;
                            }
                            PeerState::HungUp => {
                                hung_up.store(true, Ordering::Release);
                                counters.peer_hangup_detected();
                                cancel.cancel();
                                observer.fire(WatchEvent::PeerDisconnected);
                                return;
                            }
                        }
                        if !armed {
                            armed = true;
                            observer.fire(WatchEvent::Armed);
                        }
                    }
                })?
        };
        Ok(Self {
            stop,
            hung_up,
            thread: Some(thread),
            wake: Some(wake_write),
            observer,
        })
    }

    /// Stop the watcher and join its thread — at most once, whether the
    /// owner disarms or unwinds past the watch.
    fn stop_and_join(&mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            if let Some(mut wake) = self.wake.take() {
                // The watcher may already have returned (and closed the
                // read end with its thread): a failed write then means
                // exactly that, and the join below still reaps it.
                let _signalled = wake.write_all(&[1]);
            }
            let joined = thread.join();
            self.observer.fire(WatchEvent::Joined);
            joined.map_err(|panic| format!("peer watch thread panicked: {panic:?}"))?;
        }
        Ok(())
    }

    /// Stop watching; reports how the watch run ended.
    fn disarm(mut self) -> Result<PeerWatchOutcome, String> {
        self.stop_and_join()?;
        if self.hung_up.load(Ordering::Acquire) {
            Ok(PeerWatchOutcome::HungUp)
        } else {
            Ok(PeerWatchOutcome::Stopped)
        }
    }
}

impl Drop for PeerWatch {
    /// Reconcile a watch whose owner did not disarm it — a panicking
    /// dispatcher unwinding (S21-09): the watcher thread is stopped and
    /// joined, never left spinning without an owner.
    fn drop(&mut self) {
        // A dispatcher already unwinding cannot return another error;
        // the normal disarm path above propagates watcher failure.
        let _result = self.stop_and_join();
    }
}

enum PeerState {
    Alive,
    /// The peer shut its write side and is waiting for the response.
    HalfClosed,
    Pipelined,
    HungUp,
    /// The wake FD fired: the owner is stopping the watch.
    StopRequested,
}

/// Block up to one poll interval for the owner's stop signal.
fn stop_signalled(wake: &UnixStream) -> bool {
    let fd = std::os::fd::AsFd::as_fd(wake);
    let mut fds = [PollFd::new(&fd, PollFlags::IN)];
    matches!(poll(&mut fds, Some(&poll_timeout())), Ok(count) if count > 0)
}

/// Best-effort drain of one wake byte; only the defensive path uses it.
fn drain_wake_byte(wake: &UnixStream) {
    let fd = std::os::fd::AsFd::as_fd(wake);
    let mut byte = [0_u8; 1];
    let _drained = recv(fd, &mut byte, RecvFlags::empty());
}

fn poll_timeout() -> Timespec {
    Timespec {
        tv_sec: 0,
        tv_nsec: i64::try_from(PeerWatch::POLL_INTERVAL.as_nanos())
            .map_or(50_000_000, |nanos| nanos),
    }
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
/// half-close. The wake FD rides in the same poll set so a stop lands
/// at once instead of at the end of the timeout.
fn peer_state(stream: &UnixStream, wake: &UnixStream) -> PeerState {
    let fd = std::os::fd::AsFd::as_fd(stream);
    let wake_fd = std::os::fd::AsFd::as_fd(wake);
    let mut fds = [
        PollFd::new(&fd, PollFlags::IN | PollFlags::HUP),
        PollFd::new(&wake_fd, PollFlags::IN),
    ];
    let timeout = poll_timeout();
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
            if fds
                .get(1)
                .map_or(PollFlags::empty(), PollFd::revents)
                .contains(PollFlags::IN)
            {
                return PeerState::StopRequested;
            }
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
        | IpcError::ReadinessTimeout { .. }
        | IpcError::InvalidAdmissionPolicy
        | IpcError::SocketInUse(_)
        | IpcError::SocketPathInsecure { .. }
        | IpcError::SocketAccessUnsatisfiable { .. }
        | IpcError::ZeroRequestId) => other,
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
        ClientIoPolicy, ConnectionCloseReason, IpcDispatcher, IpcError, IpcPlane,
        IpcServerCounters, PeerCredentials, PeerWatch, PeerWatchOutcome, RequestEnvelope,
        RequestEventStageV1, ResponseEnvelope, SocketPathIdentity, UdsServer, WatchEvent,
        WatchObserver, connect_before_deadline, connect_requires_completion_wait,
        create_connect_socket, decode_response, encode_request, handle_connection, send_request,
        wait_for_connect,
    };
    use crate::socket_access::{PRIVATE_DIRECTORY_MODE, PRIVATE_SOCKET_MODE};
    use rustix::fs::{OFlags, fcntl_getfl};
    use rustix::io::{Errno, FdFlags, fcntl_getfd};
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    use quanta_index_core::{RequestBudgetV1, RequestProviderStageV1};
    use serde::de::{self, MapAccess, Visitor};
    use serde::ser::SerializeStruct;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::admission::{DispatchSlots, ServerAdmissionPolicy, SlotRefusal};
    use crate::codec::MAX_FRAME_BODY_BYTES;

    type TestRes = Result<(), String>;

    fn test_counters() -> Arc<IpcServerCounters> {
        Arc::new(IpcServerCounters::for_plane("test"))
    }

    fn test_slots() -> DispatchSlots {
        DispatchSlots::for_policy(ServerAdmissionPolicy::DEFAULT)
    }

    fn test_policy() -> ServerAdmissionPolicy {
        ServerAdmissionPolicy::DEFAULT
    }

    /// Failure-containment bound for watch-event waits: the test fails
    /// instead of hanging when the watch never reports.
    const WATCH_EVENT_BOUND: Duration = Duration::from_secs(30);

    fn expect_watch_event(received: &mpsc::Receiver<WatchEvent>, expected: WatchEvent) -> TestRes {
        let observed = received.recv_timeout(WATCH_EVENT_BOUND).map_err(|err| {
            format!("test must observe watch {expected:?} before the bound: {err}")
        })?;
        if observed != expected {
            return Err(format!(
                "expected watch {expected:?}, observed {observed:?}"
            ));
        }
        Ok(())
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
        let dir = private_tempdir()?;
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

        let dir = private_tempdir()?;
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

    /// A tempdir at exactly the private directory mode: `tempfile` creates
    /// under the umask, and the server refuses a wider existing directory.
    fn private_tempdir() -> Result<tempfile::TempDir, String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        Ok(dir)
    }

    fn typed_bind_error(result: Result<UdsServer, IpcError>) -> Result<IpcError, String> {
        match result {
            Ok(_server) => Err("bind must be refused".to_string()),
            Err(error) => Ok(error),
        }
    }

    fn mode_of(path: &Path) -> Result<u32, String> {
        Ok(std::fs::symlink_metadata(path)
            .map_err(|error| error.to_string())?
            .mode()
            & 0o7777)
    }

    // QI-BB-014: a socket path with a live listener is never taken.
    #[test]
    fn a_live_listener_keeps_its_path_and_a_second_bind_is_refused() -> TestRes {
        let dir = private_tempdir()?;
        let socket_path = dir.path().join("live.sock");
        let first = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;
        let identity =
            SocketPathIdentity::capture(&socket_path).map_err(|error| error.to_string())?;
        let refused = typed_bind_error(UdsServer::bind(&socket_path))?;
        if !matches!(refused, IpcError::SocketInUse(ref path) if path == &socket_path) {
            return Err(format!("expected SOCKET_IN_USE, got {refused}"));
        }
        if !identity
            .still_owns(&socket_path)
            .map_err(|error| error.to_string())?
        {
            return Err("the live socket must keep its inode".to_string());
        }
        // The probe's hang-up did not disturb the listener: it still accepts.
        let client = UnixStream::connect(&socket_path).map_err(|error| error.to_string())?;
        drop(client);
        drop(first);
        Ok(())
    }

    #[test]
    fn a_stale_socket_is_reclaimed_and_the_bound_socket_is_private() -> TestRes {
        let dir = private_tempdir()?;
        let socket_dir = dir.path().join("plane");
        let socket_path = socket_dir.join("stale.sock");
        // A socket file nobody listens on any more, in a directory at the
        // private policy's exact mode.
        std::fs::create_dir_all(&socket_dir).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let stale = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
        drop(stale);
        if !std::fs::symlink_metadata(&socket_path)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_socket()
        {
            return Err("the stale socket file must remain for the test".to_string());
        }
        let server = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;
        if mode_of(&socket_path)? != PRIVATE_SOCKET_MODE {
            return Err(format!(
                "socket mode must be {PRIVATE_SOCKET_MODE:04o}, got {:04o}",
                mode_of(&socket_path)?
            ));
        }
        let client = UnixStream::connect(&socket_path).map_err(|error| error.to_string())?;
        drop(client);
        drop(server);
        Ok(())
    }

    #[test]
    fn a_directory_the_server_creates_is_private() -> TestRes {
        let dir = private_tempdir()?;
        let socket_dir = dir.path().join("created").join("deeper");
        let socket_path = socket_dir.join("private.sock");
        let server = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;
        if mode_of(&socket_dir)? != PRIVATE_DIRECTORY_MODE {
            return Err(format!(
                "created socket directory must be {PRIVATE_DIRECTORY_MODE:04o}, got {:04o}",
                mode_of(&socket_dir)?
            ));
        }
        drop(server);
        Ok(())
    }

    #[test]
    fn a_socket_directory_others_can_write_is_refused_unless_sticky() -> TestRes {
        let dir = private_tempdir()?;
        let permissive = dir.path().join("permissive");
        std::fs::create_dir(&permissive).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&permissive, std::fs::Permissions::from_mode(0o777))
            .map_err(|error| error.to_string())?;
        let refused = typed_bind_error(UdsServer::bind(&permissive.join("open.sock")))?;
        if !matches!(refused, IpcError::SocketPathInsecure { ref reason, .. } if reason.contains("writable by others"))
        {
            return Err(format!(
                "expected SOCKET_PATH_INSECURE for a permissive directory, got {refused}"
            ));
        }
        // The sticky bit makes a shared directory safe to bind under: others
        // cannot unlink this user's socket.
        std::fs::set_permissions(&permissive, std::fs::Permissions::from_mode(0o1777))
            .map_err(|error| error.to_string())?;
        let server =
            UdsServer::bind(&permissive.join("shared.sock")).map_err(|error| error.to_string())?;
        drop(server);
        // A group-writable directory of this user's own is refused too.
        let group = dir.path().join("group");
        std::fs::create_dir(&group).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&group, std::fs::Permissions::from_mode(0o770))
            .map_err(|error| error.to_string())?;
        let refused = typed_bind_error(UdsServer::bind(&group.join("group.sock")))?;
        if !matches!(refused, IpcError::SocketPathInsecure { .. }) {
            return Err(format!(
                "expected SOCKET_PATH_INSECURE for a group-writable directory, got {refused}"
            ));
        }
        // So is one merely wider than the private policy: a directory a
        // permissive umask left at 0755 is not 0700 (QI-BB-014).
        let readable = dir.path().join("readable");
        std::fs::create_dir(&readable).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
        let refused = typed_bind_error(UdsServer::bind(&readable.join("readable.sock")))?;
        if !matches!(refused, IpcError::SocketPathInsecure { ref reason, .. } if reason.contains("wider than the policy") && reason.contains("0700"))
        {
            return Err(format!(
                "expected SOCKET_PATH_INSECURE for a 0755 directory under a private policy, got {refused}"
            ));
        }
        std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let server =
            UdsServer::bind(&readable.join("readable.sock")).map_err(|error| error.to_string())?;
        drop(server);
        Ok(())
    }

    // A symlinked directory is judged by its target (`/tmp` is a symlink on
    // macOS): a private target binds, a permissive target is refused.
    #[test]
    fn a_symlinked_socket_directory_is_judged_by_its_target() -> TestRes {
        let dir = private_tempdir()?;
        let private = dir.path().join("private");
        std::fs::create_dir(&private).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let private_link = dir.path().join("private-link");
        std::os::unix::fs::symlink(&private, &private_link).map_err(|error| error.to_string())?;
        let server = UdsServer::bind(&private_link.join("via-link.sock"))
            .map_err(|error| error.to_string())?;
        drop(server);
        let permissive = dir.path().join("permissive");
        std::fs::create_dir(&permissive).map_err(|error| error.to_string())?;
        std::fs::set_permissions(&permissive, std::fs::Permissions::from_mode(0o777))
            .map_err(|error| error.to_string())?;
        let permissive_link = dir.path().join("permissive-link");
        std::os::unix::fs::symlink(&permissive, &permissive_link)
            .map_err(|error| error.to_string())?;
        let refused = typed_bind_error(UdsServer::bind(&permissive_link.join("via-link.sock")))?;
        if !matches!(refused, IpcError::SocketPathInsecure { .. }) {
            return Err(format!(
                "expected SOCKET_PATH_INSECURE through a symlink to a permissive directory, got {refused}"
            ));
        }
        Ok(())
    }

    #[test]
    fn a_regular_file_at_the_socket_path_is_refused_and_left_alone() -> TestRes {
        let dir = private_tempdir()?;
        let socket_path = dir.path().join("not-a-socket.sock");
        std::fs::write(&socket_path, b"keep me").map_err(|error| error.to_string())?;
        let refused = typed_bind_error(UdsServer::bind(&socket_path))?;
        if !matches!(refused, IpcError::SocketPathInsecure { ref reason, .. } if reason.contains("not a socket"))
        {
            return Err(format!(
                "expected SOCKET_PATH_INSECURE for a regular file, got {refused}"
            ));
        }
        if std::fs::read(&socket_path).map_err(|error| error.to_string())? != b"keep me" {
            return Err("the regular file must be untouched".to_string());
        }
        Ok(())
    }

    #[test]
    fn dropping_superseded_server_does_not_unlink_replacement_socket() -> TestRes {
        let dir = private_tempdir()?;
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
        let dir = private_tempdir()?;
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
        let dir = private_tempdir()?;
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
        fn dispatch(
            &self,
            _context: &super::DispatchContextV1,
            request: u64,
            _budget: &RequestBudgetV1,
        ) -> u64 {
            request.saturating_add(1)
        }
    }

    struct ProviderStageDispatcher;

    impl IpcDispatcher<u64, u64> for ProviderStageDispatcher {
        fn dispatch(
            &self,
            _context: &super::DispatchContextV1,
            request: u64,
            budget: &RequestBudgetV1,
        ) -> u64 {
            budget.record_provider_stage_v1(RequestProviderStageV1::Started { ticket_id: 91 });
            budget.record_provider_stage_v1(RequestProviderStageV1::Returned { ticket_id: 91 });
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

        fn repo_scope(_request: &u64) -> Option<String> {
            None
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

        fn overloaded(_request_id: u64, _refusal: &SlotRefusal) -> Option<Self> {
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
        let dir = private_tempdir()?;
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
                super::IpcPlane::Query,
                super::PeerCredentials {
                    uid: 1000,
                    gid: 1000,
                    pid: None,
                },
                1000,
                1,
                &AtomicBool::new(false),
                &test_counters(),
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

        fn overloaded(_request_id: u64, _refusal: &SlotRefusal) -> Option<Self> {
            Some(Self)
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

        fn overloaded(_request_id: u64, _refusal: &SlotRefusal) -> Option<Self> {
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
        fn dispatch(
            &self,
            _context: &super::DispatchContextV1,
            request: u64,
            budget: &RequestBudgetV1,
        ) -> u64 {
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

    struct FailingWatchDispatcher {
        entered: mpsc::Sender<()>,
        gate: Arc<Barrier>,
        callback_entered: mpsc::Sender<()>,
    }

    struct PanickingDispatcher;

    impl IpcDispatcher<u64, u64> for PanickingDispatcher {
        fn dispatch(
            &self,
            _context: &super::DispatchContextV1,
            _request: u64,
            _budget: &RequestBudgetV1,
        ) -> u64 {
            panic!("scripted dispatcher panic");
        }
    }

    impl IpcDispatcher<u64, u64> for FailingWatchDispatcher {
        fn dispatch(
            &self,
            _context: &super::DispatchContextV1,
            request: u64,
            budget: &RequestBudgetV1,
        ) -> u64 {
            let callback_entered = self.callback_entered.clone();
            let _failing_waiter = budget.cancel_waiter(Arc::new(move || {
                let _reported = callback_entered.send(());
                panic!("scripted watcher cancellation failure");
            }));
            self.entered.send(()).expect("dispatcher entry is observed");
            let _wait = self.gate.wait();
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
        fn dispatch(
            &self,
            _context: &super::DispatchContextV1,
            request: u64,
            budget: &RequestBudgetV1,
        ) -> u64 {
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

    // W10-R2: the id gate admits nonzero and refuses 0 typed, before
    // admission, dispatch and any response exist.
    #[test]
    fn envelope_validation_admits_nonzero_and_refuses_zero() {
        let result = (|| -> TestRes {
            let (admitted, payload) = test_request(41, 8)
                .validated()
                .map_err(|err| format!("nonzero id must validate: {err}"))?;
            if admitted.get() != 41 || payload != 8 {
                return Err(format!(
                    "validated parts must echo the envelope, got {admitted:?}/{payload}"
                ));
            }
            match test_request(0, 8).validated() {
                Err(IpcError::ZeroRequestId) => Ok(()),
                other => Err(format!("zero id must refuse typed, got {other:?}")),
            }
        })();
        assert_test_ok(&result);
    }

    // W10-R2: a zero id over the wire closes the connection typed, with
    // no dispatch, no response bytes and a decode-failure count — the
    // same refusal a corrupt frame gets.
    #[test]
    fn handle_connection_refuses_zero_request_id_without_dispatch_or_response() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let frame = encode_test_frame(0, 8)?;
            client.write_all(&frame).map_err(|err| err.to_string())?;
            client
                .shutdown(Shutdown::Write)
                .map_err(|err| err.to_string())?;

            let counters = test_counters();
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
                IpcPlane::Query,
                PeerCredentials {
                    uid: 0,
                    gid: 0,
                    pid: None,
                },
                0,
                1,
                &AtomicBool::new(false),
                &counters,
            );
            if !matches!(
                reason,
                ConnectionCloseReason::RequestDecodeFailed(IpcError::ZeroRequestId)
            ) {
                return Err(format!("unexpected close reason: {reason:?}"));
            }
            if counters.snapshot().request_decode_failures != 1 {
                return Err(format!(
                    "zero-id refusal must count one decode failure, got {:?}",
                    counters.snapshot()
                ));
            }
            if !counters
                .recent_request_events_v1()
                .map_err(|error| error.to_string())?
                .is_empty()
            {
                return Err("zero ID must not enter the request event ring".to_string());
            }
            let mut probe = [0u8; 1];
            match client.read(&mut probe) {
                Ok(0) => Ok(()),
                other => Err(format!(
                    "refused request must get no response bytes (EOF), got {other:?}"
                )),
            }
        })();
        assert_test_ok(&result);
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

            let counters = test_counters();
            let worker_counters = Arc::clone(&counters);
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
                    IpcPlane::Query,
                    PeerCredentials {
                        uid: 0,
                        gid: 0,
                        pid: None,
                    },
                    0,
                    1,
                    &AtomicBool::new(false),
                    &worker_counters,
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
            let events = counters
                .recent_request_events_v1()
                .map_err(|error| error.to_string())?;
            let stages = events.iter().map(|event| event.stage).collect::<Vec<_>>();
            if stages
                != [
                    RequestEventStageV1::Validated,
                    RequestEventStageV1::QueueAdmitted,
                    RequestEventStageV1::DispatchStarted,
                    RequestEventStageV1::DispatchReturned,
                    RequestEventStageV1::ResponseWritten,
                ]
            {
                return Err(format!("round-trip event stages differ: {stages:?}"));
            }
            if events
                .iter()
                .any(|event| event.request_id.get() != 41 || event.connection_id != 1)
            {
                return Err(format!("round-trip event identity drift: {events:?}"));
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    #[test]
    fn provider_stages_keep_the_admitted_envelope_and_connection_identity() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            client
                .write_all(&encode_test_frame(42, 8)?)
                .map_err(|err| err.to_string())?;
            client
                .shutdown(Shutdown::Write)
                .map_err(|err| err.to_string())?;

            let counters = test_counters();
            let worker_counters = Arc::clone(&counters);
            let handle = thread::spawn(move || {
                handle_connection::<
                    TestRequestEnvelope,
                    u64,
                    TestResponseEnvelope,
                    u64,
                    ProviderStageDispatcher,
                >(
                    server,
                    &ProviderStageDispatcher,
                    &test_slots(),
                    test_policy(),
                    IpcPlane::Query,
                    PeerCredentials {
                        uid: 0,
                        gid: 0,
                        pid: None,
                    },
                    0,
                    7,
                    &AtomicBool::new(false),
                    &worker_counters,
                )
            });
            let response = decode_response::<TestResponseEnvelope, _>(&mut client)
                .map_err(|err| format!("provider-stage response: {err}"))?;
            if response
                != (TestResponseEnvelope {
                    request_id: 42,
                    payload: 9,
                })
            {
                return Err(format!("provider-stage response differs: {response:?}"));
            }
            let reason = handle
                .join()
                .map_err(|error| format!("provider-stage worker panicked: {error:?}"))?;
            if !matches!(reason, ConnectionCloseReason::PeerClosed) {
                return Err(format!("provider-stage close reason differs: {reason:?}"));
            }
            let events = counters
                .recent_request_events_v1()
                .map_err(|error| error.to_string())?;
            let stages = events.iter().map(|event| event.stage).collect::<Vec<_>>();
            if stages
                != [
                    RequestEventStageV1::Validated,
                    RequestEventStageV1::QueueAdmitted,
                    RequestEventStageV1::DispatchStarted,
                    RequestEventStageV1::ProviderStarted { ticket_id: 91 },
                    RequestEventStageV1::ProviderReturned { ticket_id: 91 },
                    RequestEventStageV1::DispatchReturned,
                    RequestEventStageV1::ResponseWritten,
                ]
            {
                return Err(format!("provider-stage sequence differs: {stages:?}"));
            }
            if events
                .iter()
                .any(|event| event.request_id.get() != 42 || event.connection_id != 7)
            {
                return Err(format!("provider-stage event identity differs: {events:?}"));
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    #[test]
    fn ingest_window_markers_project_into_the_same_transport_ring() {
        let counters = test_counters();
        let bridge = super::ProviderEventBridgeV1 {
            sink: Arc::clone(&counters),
            request_id: std::num::NonZeroU64::new(43).expect("fixed nonzero fixture"),
            connection_id: 8,
            started: Instant::now(),
        };
        for stage in [
            RequestProviderStageV1::IngestWindowStarted { window_ordinal: 1 },
            RequestProviderStageV1::IngestWindowReturned { window_ordinal: 1 },
        ] {
            quanta_index_core::RequestStageDiagnosticPortV1::record_provider_stage_v1(
                &bridge, stage,
            );
        }
        let events = counters.recent_request_events_v1().expect("ring snapshot");
        assert_eq!(events.len(), 2);
        let first = events.first().expect("first provider event");
        let second = events.get(1).expect("second provider event");
        assert_eq!(first.request_id.get(), 43);
        assert_eq!(second.request_id.get(), 43);
        assert_eq!(first.connection_id, 8);
        assert_eq!(second.connection_id, 8);
        assert_eq!(
            first.stage,
            RequestEventStageV1::IngestWindowStarted { window_ordinal: 1 }
        );
        assert_eq!(
            second.stage,
            RequestEventStageV1::IngestWindowReturned { window_ordinal: 1 }
        );
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

            let counters = test_counters();
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
                IpcPlane::Query,
                PeerCredentials {
                    uid: 0,
                    gid: 0,
                    pid: None,
                },
                0,
                1,
                &AtomicBool::new(false),
                &counters,
            );
            if !matches!(
                reason,
                ConnectionCloseReason::RequestDecodeFailed(IpcError::EmptyFrame)
            ) {
                return Err(format!("unexpected close reason: {reason:?}"));
            }
            // The failure is counted where a scrape will read it (QI-BB-015).
            let snapshot = counters.snapshot();
            if snapshot.request_decode_failures != 1 || snapshot.requests_dispatched != 0 {
                return Err(format!("decode failure must be counted once: {snapshot:?}"));
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
                    IpcPlane::Query,
                    PeerCredentials {
                        uid: 0,
                        gid: 0,
                        pid: None,
                    },
                    0,
                    1,
                    &AtomicBool::new(false),
                    &test_counters(),
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

            let counters = test_counters();
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
                super::IpcPlane::Query,
                super::PeerCredentials {
                    uid: 1000,
                    gid: 1000,
                    pid: None,
                },
                1000,
                1,
                &AtomicBool::new(false),
                &counters,
            );
            let events = counters
                .recent_request_events_v1()
                .map_err(|error| error.to_string())?;
            if !matches!(events.last(), Some(event) if event.stage == RequestEventStageV1::ResponseEncodeFailed && event.request_id.get() == 7)
            {
                return Err(format!("encode failure event missing: {events:?}"));
            }
            if let ConnectionCloseReason::ResponseEncodeFailed(IpcError::Encode(message)) = &reason
                && message.contains("simulated response encode failure")
            {
                return Ok(());
            }
            Err(format!("unexpected close reason: {reason:?}"))
        })();
        assert_test_ok(&result);
    }

    #[test]
    fn overload_response_encode_failure_has_encode_terminal_event() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            client
                .write_all(&encode_test_frame(18, 4)?)
                .map_err(|err| err.to_string())?;
            let policy = ServerAdmissionPolicy::new(
                1,
                1,
                1,
                Duration::ZERO,
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .map_err(|err| err.to_string())?;
            let slots = DispatchSlots::for_policy(policy);
            let _held = slots
                .acquire(Duration::ZERO, None)
                .map_err(|err| format!("first slot must admit: {err:?}"))?;
            let counters = test_counters();
            let reason = handle_connection::<
                TestRequestEnvelope,
                u64,
                FailingResponseEnvelope,
                u64,
                TestDispatcher,
            >(
                server,
                &TestDispatcher,
                &slots,
                policy,
                IpcPlane::Query,
                PeerCredentials {
                    uid: 1000,
                    gid: 1000,
                    pid: None,
                },
                1000,
                2,
                &AtomicBool::new(false),
                &counters,
            );
            let events = counters
                .recent_request_events_v1()
                .map_err(|err| err.to_string())?;
            if !matches!(reason, ConnectionCloseReason::ResponseEncodeFailed(_)) {
                return Err(format!("wrong overload close reason: {reason:?}"));
            }
            if !matches!(events.last(), Some(event) if event.stage == RequestEventStageV1::ResponseEncodeFailed && event.request_id.get() == 18 && event.connection_id == 2)
            {
                return Err(format!("wrong overload terminal event: {events:?}"));
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    #[test]
    fn dispatcher_panic_emits_one_terminal_event_and_releases_slot() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            client
                .write_all(&encode_test_frame(19, 4)?)
                .map_err(|err| err.to_string())?;
            let counters = test_counters();
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handle_connection::<
                    TestRequestEnvelope,
                    u64,
                    TestResponseEnvelope,
                    u64,
                    PanickingDispatcher,
                >(
                    server,
                    &PanickingDispatcher,
                    &test_slots(),
                    test_policy(),
                    IpcPlane::Query,
                    PeerCredentials {
                        uid: 1000,
                        gid: 1000,
                        pid: None,
                    },
                    1000,
                    3,
                    &AtomicBool::new(false),
                    &counters,
                )
            }));
            if caught.is_ok() {
                return Err("dispatcher panic was not observed".to_owned());
            }
            let events = counters
                .recent_request_events_v1()
                .map_err(|err| err.to_string())?;
            let terminal = events
                .iter()
                .filter(|event| event.stage == RequestEventStageV1::Panicked)
                .count();
            if terminal != 1
                || !matches!(events.last(), Some(event) if event.request_id.get() == 19 && event.connection_id == 3 && event.stage == RequestEventStageV1::Panicked)
            {
                return Err(format!("panic terminal event invalid: {events:?}"));
            }
            if counters.snapshot().dispatch_in_flight != 0 {
                return Err("panic leaked an in-flight slot".to_owned());
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    /// Disarming a watch on a live peer reports `Stopped`: the wake FD
    /// lands at once, the thread is joined, and the budget is untouched.
    #[test]
    fn peer_watch_disarm_with_a_live_peer_reports_stopped() {
        let result = (|| -> TestRes {
            let (_client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
            let (observer, received) = WatchObserver::channel();
            let watch = PeerWatch::arm_with_observer(
                &server,
                budget.cancel_handle(),
                test_counters(),
                observer,
            )
            .map_err(|err| err.to_string())?;
            expect_watch_event(&received, WatchEvent::Armed)?;
            let outcome = watch.disarm()?;
            if outcome != PeerWatchOutcome::Stopped {
                return Err(format!("a live peer disarms Stopped, got {outcome:?}"));
            }
            expect_watch_event(&received, WatchEvent::Stopped)?;
            expect_watch_event(&received, WatchEvent::Joined)?;
            if budget.is_cancelled() {
                return Err("disarming a live peer must not cancel".to_string());
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    /// A peer that closes mid-watch reports `PeerDisconnected`: the hang-up
    /// is counted on the watch thread, the budget is cancelled, and a
    /// disarm after the close still reports `HungUp`.
    #[test]
    fn peer_watch_hangup_reports_disconnect_and_cancels() {
        let result = (|| -> TestRes {
            let (client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
            let counters = test_counters();
            let (observer, received) = WatchObserver::channel();
            let watch = PeerWatch::arm_with_observer(
                &server,
                budget.cancel_handle(),
                Arc::clone(&counters),
                observer,
            )
            .map_err(|err| err.to_string())?;
            expect_watch_event(&received, WatchEvent::Armed)?;
            drop(client);
            expect_watch_event(&received, WatchEvent::PeerDisconnected)?;
            if !budget.is_cancelled() {
                return Err("a hang-up must cancel the budget".to_string());
            }
            if counters.snapshot().peer_hangup_detected != 1 {
                return Err("a hang-up is detected exactly once".to_string());
            }
            let outcome = watch.disarm()?;
            if outcome != PeerWatchOutcome::HungUp {
                return Err(format!("a closed peer disarms HungUp, got {outcome:?}"));
            }
            expect_watch_event(&received, WatchEvent::Joined)?;
            Ok(())
        })();
        assert_test_ok(&result);
    }

    /// A panicking watcher is never a successful disarm.
    ///
    /// A watcher that panics after detecting a disconnect must not be
    /// reported as a successful `HungUp` or `Stopped` observation. The
    /// cancellation callback is the existing owner boundary that can
    /// fail on the watcher thread; no production mutation hook is needed.
    #[test]
    fn peer_watch_thread_failure_is_not_a_successful_disarm() {
        let result = (|| -> TestRes {
            let (client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
            let (callback_entered_tx, callback_entered_rx) = mpsc::channel();
            let _failing_waiter = budget.cancel_waiter(Arc::new(move || {
                let _reported = callback_entered_tx.send(());
                panic!("scripted cancellation callback failure");
            }));
            let (observer, received) = WatchObserver::channel();
            let watch = PeerWatch::arm_with_observer(
                &server,
                budget.cancel_handle(),
                test_counters(),
                observer,
            )
            .map_err(|err| err.to_string())?;
            expect_watch_event(&received, WatchEvent::Armed)?;
            drop(client);
            callback_entered_rx
                .recv_timeout(WATCH_EVENT_BOUND)
                .map_err(|err| format!("cancellation callback must enter before disarm: {err}"))?;
            let failure = watch
                .disarm()
                .expect_err("a panicking watcher cannot disarm successfully");
            if !failure.contains("peer watch thread panicked") {
                return Err(format!("watcher failure lost its cause: {failure}"));
            }
            expect_watch_event(&received, WatchEvent::Joined)?;
            Ok(())
        })();
        assert_test_ok(&result);
    }

    /// A half-close is never a hang-up: the close predates every arm, so
    /// each watch's first observation deterministically sees it, and every
    /// disarm still reports `Stopped` without cancelling.
    #[test]
    fn peer_watch_half_close_never_cancels_across_repeated_arms() {
        let result = (|| -> TestRes {
            for _ in 0..8 {
                let (client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
                client
                    .shutdown(Shutdown::Write)
                    .map_err(|err| err.to_string())?;
                let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
                let (observer, received) = WatchObserver::channel();
                let watch = PeerWatch::arm_with_observer(
                    &server,
                    budget.cancel_handle(),
                    test_counters(),
                    observer,
                )
                .map_err(|err| err.to_string())?;
                // The half-close predates the arm: `Armed` proves the
                // watch observed it and stayed watching.
                expect_watch_event(&received, WatchEvent::Armed)?;
                let outcome = watch.disarm()?;
                if outcome != PeerWatchOutcome::Stopped {
                    return Err(format!("a half-close disarms Stopped, got {outcome:?}"));
                }
                expect_watch_event(&received, WatchEvent::Stopped)?;
                expect_watch_event(&received, WatchEvent::Joined)?;
                if budget.is_cancelled() {
                    return Err("a half-close must not cancel the budget".to_string());
                }
                drop(client);
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    /// Dropping a watch without disarming — the panicking-dispatcher path —
    /// still stops and joins the watcher thread.
    #[test]
    fn peer_watch_drop_without_disarm_stops_and_joins() {
        let result = (|| -> TestRes {
            let (_client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
            let (observer, received) = WatchObserver::channel();
            let watch = PeerWatch::arm_with_observer(
                &server,
                budget.cancel_handle(),
                test_counters(),
                observer,
            )
            .map_err(|err| err.to_string())?;
            expect_watch_event(&received, WatchEvent::Armed)?;
            drop(watch);
            expect_watch_event(&received, WatchEvent::Stopped)?;
            expect_watch_event(&received, WatchEvent::Joined)?;
            if budget.is_cancelled() {
                return Err("dropping a live watch must not cancel".to_string());
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }

    /// Repeated arm/disarm cycles leave no thread or FD behind: a leak
    /// would exhaust the process and fail the run instead of this
    /// assertion.
    #[test]
    fn peer_watch_repeated_arms_leave_no_thread_or_fd_behind() {
        let result = (|| -> TestRes {
            let counters = test_counters();
            for _ in 0..256 {
                let (_client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
                let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
                let watch = PeerWatch::arm(&server, budget.cancel_handle(), Arc::clone(&counters))
                    .map_err(|err| err.to_string())?;
                let outcome = watch.disarm()?;
                if outcome != PeerWatchOutcome::Stopped {
                    return Err(format!("a live peer disarms Stopped, got {outcome:?}"));
                }
            }
            if counters.snapshot().peer_hangup_detected != 0 {
                return Err("no hang-up was ever detected".to_string());
            }
            Ok(())
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
    /// response must still cross the wire. The never-cancel invariant
    /// itself is proven at the owner level across repeated deterministic
    /// arms; this test keeps the wiring: the response still crosses and
    /// the close reason stays honest.
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
                    IpcPlane::Query,
                    PeerCredentials {
                        uid: 0,
                        gid: 0,
                        pid: None,
                    },
                    0,
                    1,
                    &AtomicBool::new(false),
                    &test_counters(),
                )
            });
            entered_rx
                .recv()
                .map_err(|err| format!("test must observe dispatcher entry: {err}"))?;
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
            let counters = test_counters();
            let worker_counters = Arc::clone(&counters);
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
                    IpcPlane::Query,
                    PeerCredentials {
                        uid: 0,
                        gid: 0,
                        pid: None,
                    },
                    0,
                    1,
                    &AtomicBool::new(false),
                    &worker_counters,
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
            let events = counters
                .recent_request_events_v1()
                .map_err(|error| error.to_string())?;
            if !matches!(events.last(), Some(event) if event.stage == RequestEventStageV1::PeerCancelled && event.request_id.get() == 9)
            {
                return Err(format!("peer cancellation event missing: {events:?}"));
            }
            if matches!(reason, ConnectionCloseReason::PeerClosed) {
                return Ok(());
            }
            Err(format!("unexpected close reason: {reason:?}"))
        })();
        assert_test_ok(&result);
    }

    #[test]
    fn handle_connection_surfaces_a_failed_peer_watch() {
        let result = (|| -> TestRes {
            let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            client
                .write_all(&encode_test_frame(11, 1)?)
                .map_err(|err| err.to_string())?;
            let (entered_tx, entered_rx) = mpsc::channel();
            let (callback_tx, callback_rx) = mpsc::channel();
            let gate = Arc::new(Barrier::new(2));
            let dispatcher = FailingWatchDispatcher {
                entered: entered_tx,
                gate: Arc::clone(&gate),
                callback_entered: callback_tx,
            };
            let handle = thread::spawn(move || {
                handle_connection::<
                    TestRequestEnvelope,
                    u64,
                    TestResponseEnvelope,
                    u64,
                    FailingWatchDispatcher,
                >(
                    server,
                    &dispatcher,
                    &test_slots(),
                    test_policy(),
                    IpcPlane::Query,
                    PeerCredentials {
                        uid: 0,
                        gid: 0,
                        pid: None,
                    },
                    0,
                    1,
                    &AtomicBool::new(false),
                    &test_counters(),
                )
            });
            entered_rx
                .recv_timeout(Duration::from_secs(10))
                .map_err(|error| format!("dispatcher entry was not observed: {error}"))?;
            drop(client);
            callback_rx
                .recv_timeout(Duration::from_secs(10))
                .map_err(|error| format!("watcher failure was not observed: {error}"))?;
            let _released = gate.wait();
            let reason = handle
                .join()
                .map_err(|panic| format!("connection thread panicked: {panic:?}"))?;
            if !matches!(&reason, ConnectionCloseReason::PeerWatchFailed(message) if message.contains("peer watch thread panicked"))
            {
                return Err(format!(
                    "watcher failure must close the connection typed: {reason:?}"
                ));
            }
            Ok(())
        })();
        assert_test_ok(&result);
    }
}
