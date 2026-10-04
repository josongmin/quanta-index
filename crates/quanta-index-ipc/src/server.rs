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
//! connection without writing a response. Ingress admission exhaustion also
//! closes before a request id can be decoded. Request-domain failures (e.g.
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
use std::time::{Duration, Instant};

use quanta_index_core::{
    RequestBudgetV1, RequestCorrelationV1, RequestProviderStageV1, RequestStageDiagnosticPortV1,
};

use crate::admission::{
    DecodePermit, DispatchSlots, IngressBudget, ServerAdmissionPolicy, SlotRefusal,
};
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
use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, connect, sockopt::socket_error};
#[cfg(target_os = "linux")]
use rustix::net::{SocketFlags, socket_with};

#[cfg(test)]
use crate::codec::decode_request;
use crate::codec::{decode_request_guarded, decode_response, encode_request, encode_response};
use crate::counters::{IpcServerCounters, RequestEventSinkV1, RequestEventStageV1, RequestEventV1};
use crate::error::{IpcError, IpcIoOperation, MAX_FRAME_BODY_BYTES};
use crate::plane::IpcPlane;
use crate::socket_access::PeerCredentials;

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
            Request::SemanticWorkBoundedV1(bounded) => pinned_repo_scope(
                bounded.query.generation.as_ref(),
                bounded.query.generation_selector.as_ref(),
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
    ingress: Arc<IngressBudget>,
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

/// A read-only observation of the exact socket path bound by one server.
/// Reusing the server's inode identity prevents readiness from treating an
/// unlinked or replaced path as an accepting plane.
#[derive(Clone, Debug)]
pub struct BoundSocketPathProbe {
    path: PathBuf,
    identity: SocketPathIdentity,
}

impl BoundSocketPathProbe {
    /// Whether the original bound socket is still published at its path.
    pub fn is_current(&self) -> std::io::Result<bool> {
        self.identity.still_owns(&self.path)
    }
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
            ingress: Arc::new(IngressBudget::for_server(policy)),
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

    /// Join the process-wide ingress budget before starting the accept loop.
    #[must_use]
    pub fn with_ingress_budget(mut self, ingress: Arc<IngressBudget>) -> Self {
        self.ingress = ingress;
        self
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

    /// Capture the server-owned socket identity for process readiness.
    #[must_use]
    pub fn bound_socket_path_probe(&self) -> BoundSocketPathProbe {
        BoundSocketPathProbe {
            path: self.socket_path.clone(),
            identity: self.socket_path_identity,
        }
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
        let slots = Arc::new(DispatchSlots::with_shared_ingress(
            self.policy,
            Arc::clone(&self.ingress),
        ));
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
        // The per-operation socket timeout permits a peer to drip bytes
        // forever. Bound the entire request read and retain ingress admission
        // through dispatch and response, including the queue wait.
        let deadline = Instant::now()
            .checked_add(policy.io_timeout())
            .unwrap_or_else(Instant::now);
        let mut ingress_reader = IngressDeadlineReader {
            stream: &mut stream,
            deadline,
            saw_frame_bytes: false,
            first_read: true,
        };
        let (request, _decode_permit) = match decode_request_guarded::<RequestEnvelopeT, _, _>(
            &mut ingress_reader,
            |body_bytes| slots.try_acquire_decode(body_bytes, plane),
            DecodePermit::reserve,
        ) {
            Ok(request) => request,
            Err(err @ IpcError::IngressSaturated { .. }) => {
                counters.ingress_admission_refused();
                return ConnectionCloseReason::RequestDecodeFailed(err);
            }
            Err(err) => {
                if peer_closed_before_frame(&err, ingress_reader.saw_frame_bytes) {
                    return ConnectionCloseReason::PeerClosed;
                }
                counters.request_decode_failed();
                return ConnectionCloseReason::RequestDecodeFailed(err);
            }
        };
        if let Err(err) = stream.set_read_timeout(Some(policy.io_timeout())) {
            return ConnectionCloseReason::TimeoutConfigFailed(format!("set_read_timeout: {err}"));
        }
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

/// Server ingress deadline across the whole request, including all fragments.
///
/// The socket timeout is refreshed to the remaining total deadline before
/// every read so a trickle of bytes cannot renew admission indefinitely.
struct IngressDeadlineReader<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
    saw_frame_bytes: bool,
    first_read: bool,
}

impl Read for IngressDeadlineReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(deadline_elapsed_error());
        }
        if self.first_read {
            // The connection already has the full policy timeout from setup
            // or the previous decoded request. Reconfiguring it here is
            // redundant and can fail on a peer that just closed on Darwin.
            self.first_read = false;
        } else {
            self.stream.set_read_timeout(Some(remaining))?;
        }
        let read = self.stream.read(buffer)?;
        if read != 0 {
            self.saw_frame_bytes = true;
        }
        Ok(read)
    }
}

/// A close before any bytes of the next frame is not a malformed request.
/// A truncated or reset connection after even one byte is a failed decode.
fn peer_closed_before_frame(error: &IpcError, saw_frame_bytes: bool) -> bool {
    if saw_frame_bytes {
        return false;
    }
    matches!(error, IpcError::Truncated)
        || matches!(error, IpcError::Io(io_error) if io_error.kind() == ErrorKind::ConnectionReset)
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

mod peer_watch;

use peer_watch::{PeerWatch, PeerWatchOutcome};
#[cfg(test)]
use peer_watch::{WatchEvent, WatchObserver};

/// Request-local client timings for one successful one-shot IPC call.
/// `read_io_ns` is included in `decode_call_ns`; their sum is not a total.
/// The unallocated part of `total_ns` includes deadline checks and local work.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClientIpcTimingV1 {
    pub total_ns: u64,
    pub encode_ns: u64,
    pub connect_ns: u64,
    pub write_ns: u64,
    pub decode_call_ns: u64,
    pub read_io_ns: u64,
}

struct TimedResponseReader<'a, R> {
    inner: &'a mut R,
    read_io: Duration,
}

impl<R: Read> Read for TimedResponseReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let started = Instant::now();
        let result = self.inner.read(buf);
        self.read_io = self.read_io.saturating_add(started.elapsed());
        result
    }
}

fn observed_ns(duration: Duration) -> Result<u64, IpcError> {
    u64::try_from(duration.as_nanos())
        .map_err(|error| IpcError::Decode(format!("client timing nanoseconds exceed u64: {error}")))
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
    send_request_inner(socket, request, io_policy, None)
}

/// The same one-shot request with opt-in, request-local client attribution.
/// Failed calls return their original transport error without a successful
/// observation. Normal callers use [`send_request`] and read no extra clocks.
pub fn send_request_observed<RequestEnvelopeT, ResponseEnvelopeT>(
    socket: &Path,
    request: &RequestEnvelopeT,
    io_policy: ClientIoPolicy,
) -> Result<(ResponseEnvelopeT, ClientIpcTimingV1), IpcError>
where
    RequestEnvelopeT: serde::Serialize,
    ResponseEnvelopeT: serde::de::DeserializeOwned,
{
    let mut timing = ClientIpcTimingV1::default();
    let response = send_request_inner(socket, request, io_policy, Some(&mut timing))?;
    Ok((response, timing))
}

fn send_request_inner<RequestEnvelopeT, ResponseEnvelopeT>(
    socket: &Path,
    request: &RequestEnvelopeT,
    io_policy: ClientIoPolicy,
    mut timing: Option<&mut ClientIpcTimingV1>,
) -> Result<ResponseEnvelopeT, IpcError>
where
    RequestEnvelopeT: serde::Serialize,
    ResponseEnvelopeT: serde::de::DeserializeOwned,
{
    let total_started = timing.as_ref().map(|_| Instant::now());
    let deadline = io_policy.request_deadline()?;
    let encode_started = timing.as_ref().map(|_| Instant::now());
    let frame = encode_request(request)?;
    if let (Some(timing), Some(started)) = (timing.as_deref_mut(), encode_started) {
        timing.encode_ns = observed_ns(started.elapsed())?;
    }
    let connect_started = timing.as_ref().map(|_| Instant::now());
    let stream = connect_before_deadline(socket, deadline)
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Connect, io_policy))?;
    if let (Some(timing), Some(started)) = (timing.as_deref_mut(), connect_started) {
        timing.connect_ns = observed_ns(started.elapsed())?;
    }
    let mut stream = DeadlineStream::new(stream, deadline);
    let write_started = timing.as_ref().map(|_| Instant::now());
    stream
        .write_all(&frame)
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Write, io_policy))?;
    if let (Some(timing), Some(started)) = (timing.as_deref_mut(), write_started) {
        timing.write_ns = observed_ns(started.elapsed())?;
    }
    let decode_started = timing.as_ref().map(|_| Instant::now());
    let mut read_io = Duration::ZERO;
    let response = if timing.is_some() {
        let mut reader = TimedResponseReader {
            inner: &mut stream,
            read_io: Duration::ZERO,
        };
        let result = decode_response::<ResponseEnvelopeT, _>(&mut reader);
        read_io = reader.read_io;
        result
    } else {
        decode_response::<ResponseEnvelopeT, _>(&mut stream)
    }
    .map_err(|error| classify_client_decode_error(error, IpcIoOperation::Read, io_policy))?;
    if let (Some(timing), Some(started)) = (timing.as_deref_mut(), decode_started) {
        timing.decode_call_ns = observed_ns(started.elapsed())?;
        timing.read_io_ns = observed_ns(read_io)?;
    }
    ensure_deadline_remaining(deadline)
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Read, io_policy))?;
    if let (Some(timing), Some(started)) = (timing.as_deref_mut(), total_started) {
        timing.total_ns = observed_ns(started.elapsed())?;
    }
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
        match self.stream.set_read_timeout(Some(remaining)) {
            Ok(()) => {}
            Err(error) if cfg!(target_os = "macos") && error.kind() == ErrorKind::InvalidInput => {
                // Darwin can reject SO_RCVTIMEO after the peer has closed,
                // even when a complete response is still buffered. Poll the
                // same socket under the original absolute deadline and let
                // read/decode distinguish buffered bytes from a truncated EOF.
                let mut poll_fd = [PollFd::new(&self.stream, PollFlags::IN | PollFlags::HUP)];
                loop {
                    let timeout = Timespec::try_from(self.remaining()?)
                        .map_err(|reason| std::io::Error::new(ErrorKind::InvalidInput, reason))?;
                    match poll(&mut poll_fd, Some(&timeout)) {
                        Ok(0) => return Err(deadline_elapsed_error()),
                        Ok(_) => break,
                        Err(Errno::INTR) => {}
                        Err(reason) => return Err(reason.into()),
                    }
                }
            }
            Err(error) => return Err(error),
        }
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
        | IpcError::IngressSaturated { .. }
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
mod tests;
