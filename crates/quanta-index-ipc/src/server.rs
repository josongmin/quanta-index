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
    DiagnosticCaptureIdV1, DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1,
    DiagnosticLaneV1, DiagnosticPlaneV1, DiagnosticRootKindV1, DiagnosticSpanGuardV1,
    DiagnosticSpanIdV1, DiagnosticStageV1, DiagnosticTerminalV1, DiagnosticTraceRefV1,
    RequestBudgetV1, RequestCorrelationV1, RequestProviderStageV1, RequestStageDiagnosticPortV1,
    index::MemoryScope,
};

use crate::admission::{
    DecodePermit, DispatchSlots, IngressBudget, ServerAdmissionPolicy, SlotRefusal,
};
use crate::peer_credentials::{KernelPeerCredentials, PeerCredentialsSource};
use crate::socket_access::{PeerRefusal, SocketAccessPolicy, admit_peer};

use quanta_index_contract::{
    DiagnosticAttemptFieldV1, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneIngestIpcRequestEnvelope,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponseEnvelope,
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

use crate::codec::{
    AdmittedClientRequestFrame, AdmittedClientResponseBody, DIAGNOSTIC_ACK_BYTES,
    DIAGNOSTIC_PREFACE_MAGIC, decode_request_guarded, decode_response, diagnostic_ack,
    diagnostic_ack_accepted, diagnostic_preface, encode_request,
    encode_request_single_frame_admitted, read_response_body_admitted,
    read_response_body_admitted_with_work_v1,
};
#[cfg(test)]
use crate::codec::{decode_request, encode_response};
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
    /// This request's explicit child context when tracing is configured.
    pub diagnostic_context: Option<DiagnosticContextV1>,
    /// Request-local typed backend error observation for diagnostic terminal state.
    pub diagnostic_backend_error: Arc<AtomicBool>,
}

impl DispatchContextV1 {
    /// Emit a payload-free backend stage under the transport's validated ID.
    pub fn record_event_v1(&self, stage: RequestEventStageV1) {
        if matches!(
            stage,
            RequestEventStageV1::BackendOutcome { error: Some(_), .. }
        ) {
            self.diagnostic_backend_error.store(true, Ordering::Release);
        }
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
    diagnostic_root: Option<DiagnosticSpanGuardV1>,
    diagnostic_context: Option<DiagnosticContextV1>,
    backend_error: Arc<AtomicBool>,
}

fn trace_span_or_health(
    context: &DiagnosticContextV1,
    result: Result<DiagnosticSpanGuardV1, quanta_index_core::DiagnosticTraceErrorV1>,
) -> Option<DiagnosticSpanGuardV1> {
    match result {
        Ok(span) => Some(span),
        Err(error) => {
            let code = match error {
                quanta_index_core::DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    DiagnosticHealthCodeV1::CounterOverflow
                }
                quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed => {
                    DiagnosticHealthCodeV1::ClockRegressed
                }
                quanta_index_core::DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                quanta_index_core::DiagnosticTraceErrorV1::MissingParentSpan => {
                    DiagnosticHealthCodeV1::MissingParentSpan
                }
            };
            context.health(code);
            None
        }
    }
}

impl<'a> RequestEventScope<'a> {
    fn new(
        sink: &'a IpcServerCounters,
        request_id: NonZeroU64,
        plane: IpcPlane,
        connection_id: u64,
        capture: Option<&DiagnosticContextV1>,
        attempt: DiagnosticAttemptFieldV1,
    ) -> Self {
        let diagnostic_root = capture.and_then(|capture| {
            trace_span_or_health(
                capture,
                capture.enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(0)),
            )
        });
        let diagnostic_context = diagnostic_root
            .as_ref()
            .zip(capture)
            .map(|(root, capture)| capture.child_context(root));
        if let (Some(root), Some(context)) = (diagnostic_root.as_ref(), diagnostic_context.as_ref())
        {
            let trace_plane = match plane {
                IpcPlane::Query => DiagnosticPlaneV1::Query,
                IpcPlane::Control => DiagnosticPlaneV1::Control,
                IpcPlane::Ingest => DiagnosticPlaneV1::Ingest,
            };
            context.request_identity(root, request_id.get(), trace_plane, connection_id);
        }
        if let (Some(root), Some(context), Some(token)) = (
            diagnostic_root.as_ref(),
            diagnostic_context.as_ref(),
            attempt.token(),
        ) {
            context.link(
                root,
                DiagnosticTraceRefV1 {
                    capture_id: DiagnosticCaptureIdV1(token.capture_id()),
                    span_id: DiagnosticSpanIdV1::from_nonzero(token.attempt_span_id()),
                },
            );
        }
        if let (Some(root), Some(context)) = (diagnostic_root.as_ref(), diagnostic_context.as_ref())
        {
            match attempt {
                DiagnosticAttemptFieldV1::Invalid => {
                    context.counter(root, DiagnosticCounterV1::InvalidAttemptTokens, 1);
                    context.health(DiagnosticHealthCodeV1::InvalidAttemptToken);
                    context.health(DiagnosticHealthCodeV1::UnlinkedAttempt);
                }
                DiagnosticAttemptFieldV1::Duplicate => {
                    context.counter(root, DiagnosticCounterV1::InvalidAttemptTokens, 1);
                    context.health(DiagnosticHealthCodeV1::DuplicateAttemptToken);
                    context.health(DiagnosticHealthCodeV1::UnlinkedAttempt);
                }
                DiagnosticAttemptFieldV1::Absent | DiagnosticAttemptFieldV1::Linked(_) => {}
            }
        }
        let scope = Self {
            sink,
            request_id,
            connection_id,
            started: Instant::now(),
            finished: false,
            diagnostic_root,
            diagnostic_context,
            backend_error: Arc::new(AtomicBool::new(false)),
        };
        scope.emit(RequestEventStageV1::Validated);
        scope
    }

    fn diagnostic_context(&self) -> Option<&DiagnosticContextV1> {
        self.diagnostic_context.as_ref()
    }

    fn backend_error(&self) -> bool {
        self.backend_error.load(Ordering::Acquire)
    }

    fn enter(&self, stage: DiagnosticStageV1) -> Option<DiagnosticSpanGuardV1> {
        self.diagnostic_context.as_ref().and_then(|context| {
            trace_span_or_health(context, context.enter(stage, DiagnosticLaneV1(0)))
        })
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
        let outcome = match stage {
            RequestEventStageV1::ResponseWritten if self.backend_error() => {
                DiagnosticTerminalV1::Error { code: 4 }
            }
            RequestEventStageV1::ResponseWritten => DiagnosticTerminalV1::Ok,
            RequestEventStageV1::PeerCancelled => DiagnosticTerminalV1::Cancelled { code: 1 },
            RequestEventStageV1::Validated
            | RequestEventStageV1::ShuttingDown
            | RequestEventStageV1::QueueAdmitted
            | RequestEventStageV1::QueueRefusedGlobal
            | RequestEventStageV1::QueueRefusedRepository
            | RequestEventStageV1::DispatchStarted
            | RequestEventStageV1::BackendStarted
            | RequestEventStageV1::BackendReturned
            | RequestEventStageV1::BackendOutcome { .. }
            | RequestEventStageV1::ProviderStarted { .. }
            | RequestEventStageV1::ProviderReturned { .. }
            | RequestEventStageV1::IngestWindowStarted { .. }
            | RequestEventStageV1::IngestWindowReturned { .. }
            | RequestEventStageV1::DispatchReturned
            | RequestEventStageV1::PeerWatchFailed
            | RequestEventStageV1::ResponseEncodeFailed
            | RequestEventStageV1::ResponseWriteFailed
            | RequestEventStageV1::Aborted
            | RequestEventStageV1::Panicked => DiagnosticTerminalV1::Error { code: 1 },
        };
        self.finish_with_outcome(stage, outcome);
    }

    fn finish_with_outcome(&mut self, stage: RequestEventStageV1, outcome: DiagnosticTerminalV1) {
        self.emit(stage);
        if let Some(root) = self.diagnostic_root.take() {
            let _diagnostic_result = root.finish(outcome);
        }
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

    /// Acquire data-only response backing from the composition's ledger.
    /// Count, encode and write share one admitted phase budget. Ordinary replies
    /// retain the dispatch budget; explicitly classified terminal replies use
    /// a bounded transport phase without renewing backend execution.
    /// Standalone dispatchers explicitly use the transport's bounded codec
    /// without a service ledger; native production overrides this method.
    fn reserve_response_memory(
        &self,
        _budget: &RequestBudgetV1,
        _bytes: u64,
    ) -> Result<Option<quanta_index_core::index::MemoryLease>, IpcError> {
        Ok(None)
    }

    /// Only called when the request envelope explicitly selects streaming.
    /// Each emitted payload is a nonterminal response frame. The returned
    /// payload is the sole terminal frame. Unsupported is a typed refusal
    /// supplied by the response envelope, never an accidental one-shot reply.
    fn dispatch_stream(
        &self,
        _context: &DispatchContextV1,
        _request: Request,
        _budget: &RequestBudgetV1,
        _emit: &mut dyn FnMut(Response) -> Result<(), IpcError>,
    ) -> StreamDispatch<Response> {
        StreamDispatch::Unsupported
    }
}

pub enum StreamDispatch<Response> {
    Terminal(Response),
    /// The dispatcher wrote exactly one terminal through the retained emitter.
    EmittedTerminal,
    Unsupported,
}

pub trait RequestEnvelope<Request>: serde::de::DeserializeOwned + Send + Sync + 'static {
    fn into_parts(self) -> (u64, Request, DiagnosticAttemptFieldV1);

    /// Stream routing is explicit per request variant; ordinary requests
    /// always retain their single-response path.
    fn streaming(_request: &Request) -> bool {
        false
    }

    /// Split the envelope and prove its request id is admissible
    /// (W10-R2): 0 is a malformed envelope refused with a typed error
    /// before admission, dispatch and any response. The single gate every
    /// plane's connection loop calls; the wire shape stays `u64`.
    fn validated(self) -> Result<(NonZeroU64, Request, DiagnosticAttemptFieldV1), IpcError> {
        let (request_id, payload, attempt) = self.into_parts();
        let admitted = NonZeroU64::new(request_id).ok_or(IpcError::ZeroRequestId)?;
        Ok((admitted, payload, attempt))
    }

    /// The repository this request is scoped to, for the per-repository
    /// in-flight cap (QI-BB-002); `None` for a request that names no
    /// repository, which only the global slot bound applies to.
    fn repo_scope(request: &Request) -> Option<String>;
}

pub trait ResponseEnvelope<Response>: serde::Serialize + Send + Sync + 'static {
    fn from_parts(request_id: u64, payload: Response) -> Self;

    /// A closed terminal result may need to cross the wire after dispatch
    /// interruption: a typed interruption or an already durable corpus receipt.
    /// This grants only a bounded reply phase, never more backend work. Other
    /// replies retain their original execution deadline, including late success.
    fn terminal_reply_phase(&self) -> bool {
        false
    }

    #[must_use]
    fn stream_unsupported(_request_id: u64) -> Option<Self>
    where
        Self: Sized,
    {
        None
    }

    fn stream_stage(_payload: &Response) -> bool {
        false
    }

    fn stream_terminal(_payload: &Response) -> bool {
        false
    }

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

/// Only typed interruption replies may leave an interrupted query/control lane.
fn terminal_interruption(code: quanta_index_contract::SearchPlaneErrorCodeV2) -> bool {
    matches!(
        code,
        quanta_index_contract::SearchPlaneErrorCodeV2::RequestDeadlineExceeded
            | quanta_index_contract::SearchPlaneErrorCodeV2::RequestCancelled
    )
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
    fn into_parts(
        self,
    ) -> (
        u64,
        quanta_index_contract::SearchPlaneQueryIpcRequest,
        DiagnosticAttemptFieldV1,
    ) {
        (self.request_id, self.payload, self.diagnostic_attempt)
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

    fn terminal_reply_phase(&self) -> bool {
        matches!(
            &self.payload,
            quanta_index_contract::SearchPlaneQueryIpcResponse::Error(error)
                if terminal_interruption(error.code)
        )
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
    fn into_parts(
        self,
    ) -> (
        u64,
        quanta_index_contract::SearchPlaneControlIpcRequest,
        DiagnosticAttemptFieldV1,
    ) {
        (self.request_id, self.payload, self.diagnostic_attempt)
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

    fn terminal_reply_phase(&self) -> bool {
        matches!(
            &self.payload,
            quanta_index_contract::SearchPlaneControlIpcResponse::Error(error)
                if terminal_interruption(error.code)
        )
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
    fn into_parts(
        self,
    ) -> (
        u64,
        quanta_index_contract::SearchPlaneIngestIpcRequest,
        DiagnosticAttemptFieldV1,
    ) {
        (self.request_id, self.payload, self.diagnostic_attempt)
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

    fn terminal_reply_phase(&self) -> bool {
        matches!(
            &self.payload,
            quanta_index_contract::SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        ) || matches!(
            &self.payload,
            quanta_index_contract::SearchPlaneIngestIpcResponse::Error(error)
                if terminal_interruption(error.code)
        )
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
    diagnostic_context: Option<DiagnosticContextV1>,
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
            diagnostic_context: None,
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

    /// Attach the process-local capture explicitly to this socket server.
    #[must_use]
    pub fn with_diagnostic_context(mut self, context: DiagnosticContextV1) -> Self {
        self.diagnostic_context = Some(context);
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
    /// non-blocking, so an empty accept queue waits up to `accept_idle` for
    /// readability before retrying. On shutdown the loop stops accepting and joins the
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
        let mut connection_threads = OwnedConnectionThreads::new(Arc::clone(&self.shutdown));
        let accept_result: Result<(), IpcError> = loop {
            if self.shutdown.load(Ordering::Acquire) {
                break Ok(());
            }
            // Finished connection handles are joined, never dropped
            // unjoined (S21-09): after `is_finished` the join is bounded
            // teardown, and a panicked connection is observed here.
            connection_threads.reap_finished();
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
                    let diagnostic_context = self.diagnostic_context.clone();
                    let shutdown = Arc::clone(&self.shutdown);
                    let policy = self.policy;
                    let owner_uid = self.owner;
                    let connection_id = self
                        .next_connection_id
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let spawned = std::thread::Builder::new()
                        .name("uds-connection".to_string())
                        .spawn(move || {
                            let reason = handle_connection_traced::<
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
                                diagnostic_context.as_ref(),
                            );
                            drop(live);
                            reason
                        });
                    match spawned {
                        Ok(handle) => connection_threads.push(handle),
                        // The un-moved guard releases the live count here.
                        Err(err) => break Err(IpcError::Io(err)),
                    }
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if let Err(error) = wait_for_accept_ready(&self.listener, accept_idle) {
                        break Err(error);
                    }
                }
                Err(err) => break Err(IpcError::Io(err)),
            }
        };
        // An accept/spawn failure must stop further dispatch and retain the
        // caller's resources until every already accepted worker exits.
        if let Err(_error) = &accept_result {
            self.shutdown.store(true, Ordering::Release);
        }
        let accept_result = finish_connection_threads(connection_threads, accept_result);
        let remove_result = self.remove_owned_socket_path().map_err(IpcError::Io);
        accept_result?;
        remove_result
    }

    fn remove_owned_socket_path(&self) -> std::io::Result<()> {
        if self.socket_path_identity.still_owns(&self.socket_path)? {
            std::fs::remove_file(&self.socket_path)?;
        }
        Ok(())
    }
}

/// Wait for a queued connection without extending the shutdown polling interval.
/// EINTR and timeout return to the accept loop so it can recheck shutdown.
fn wait_for_accept_ready(listener: &UnixListener, accept_idle: Duration) -> Result<(), IpcError> {
    let timeout = Timespec::try_from(accept_idle)
        .map_err(|reason| IpcError::Io(std::io::Error::new(ErrorKind::InvalidInput, reason)))?;
    let mut poll_fd = [PollFd::new(listener, PollFlags::IN)];
    match poll(&mut poll_fd, Some(&timeout)) {
        Ok(0) | Err(Errno::INTR) => Ok(()),
        Ok(_) => {
            let [fd] = &poll_fd;
            let ready = fd.revents();
            if ready.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL)
                || !ready.contains(PollFlags::IN)
            {
                Err(IpcError::Io(std::io::Error::other(
                    "IPC listener poll reported a non-readable event",
                )))
            } else {
                Ok(())
            }
        }
        Err(reason) => Err(IpcError::Io(reason.into())),
    }
}

/// Retains connection ownership through normal return, errors and unwinding.
struct OwnedConnectionThreads<T> {
    handles: Vec<std::thread::JoinHandle<T>>,
    shutdown: Arc<AtomicBool>,
}

impl<T> OwnedConnectionThreads<T> {
    fn new(shutdown: Arc<AtomicBool>) -> Self {
        Self {
            handles: Vec::new(),
            shutdown,
        }
    }

    fn push(&mut self, handle: std::thread::JoinHandle<T>) {
        self.handles.push(handle);
    }

    fn reap_finished(&mut self) {
        for handle in self.handles.extract_if(.., |handle| handle.is_finished()) {
            drop(handle.join());
        }
    }

    fn join_all(&mut self) {
        for handle in self.handles.drain(..) {
            drop(handle.join());
        }
    }
}

impl<T> Drop for OwnedConnectionThreads<T> {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.join_all();
    }
}

fn finish_connection_threads<T>(
    mut connection_threads: OwnedConnectionThreads<T>,
    accept_result: Result<(), IpcError>,
) -> Result<(), IpcError> {
    connection_threads.join_all();
    accept_result
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
fn handle_connection_traced<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
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
    diagnostic_context: Option<&DiagnosticContextV1>,
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
        let (request, _decode_permit, attested_capture) = match (|| {
            let mut prefix = [0_u8; 4];
            read_exact_transport(&mut ingress_reader, &mut prefix)?;
            let mut attested_capture = None;
            if prefix == DIAGNOSTIC_PREFACE_MAGIC[..4] {
                let mut rest = [0_u8; 20];
                read_exact_transport(&mut ingress_reader, &mut rest)?;
                if rest[..4] != DIAGNOSTIC_PREFACE_MAGIC[4..] {
                    return Err(IpcError::Decode(
                        "invalid diagnostic preface version".into(),
                    ));
                }
                let mut capture_id = [0_u8; 16];
                capture_id.copy_from_slice(&rest[4..]);
                let accepted =
                    diagnostic_context.is_some_and(|context| context.capture_id().0 == capture_id);
                ingress_reader
                    .stream
                    .write_all(&diagnostic_ack(accepted))
                    .map_err(IpcError::Io)?;
                if accepted {
                    attested_capture = Some(capture_id);
                }
                read_exact_transport(&mut ingress_reader, &mut prefix)?;
            }
            let mut prefixed = std::io::Cursor::new(prefix).chain(&mut ingress_reader);
            let (request, permit) = decode_request_guarded::<RequestEnvelopeT, _, _>(
                &mut prefixed,
                |body_bytes| slots.try_acquire_decode(body_bytes, plane),
                DecodePermit::reserve,
            )?;
            Ok::<_, IpcError>((request, permit, attested_capture))
        })() {
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
        let (request_id, request_payload, attempt) = match request.validated() {
            Ok(parts) => parts,
            Err(err) => {
                counters.request_decode_failed();
                return ConnectionCloseReason::RequestDecodeFailed(err);
            }
        };
        let attempt = match (attested_capture, attempt) {
            (Some(capture), DiagnosticAttemptFieldV1::Linked(token))
                if token.capture_id() == capture =>
            {
                DiagnosticAttemptFieldV1::Linked(token)
            }
            (_, DiagnosticAttemptFieldV1::Linked(_)) => DiagnosticAttemptFieldV1::Invalid,
            (_, other) => other,
        };
        let mut event_scope = RequestEventScope::new(
            counters,
            request_id,
            plane,
            connection_id,
            diagnostic_context,
            attempt,
        );
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
        let queue_span = event_scope.enter(DiagnosticStageV1::QueueWait);
        let permit = match slots.acquire(policy.queue_wait(), repo_scope.as_deref()) {
            Ok(permit) => {
                if let Some(span) = queue_span {
                    let _diagnostic_result = span.finish(DiagnosticTerminalV1::Ok);
                }
                permit
            }
            Err(refusal) => {
                if let Some(span) = queue_span {
                    let _diagnostic_result = span.finish(DiagnosticTerminalV1::Error { code: 2 });
                }
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
                let refusal_budget = RequestBudgetV1::for_duration(policy.dispatch_budget());
                match write_response(
                    &mut stream,
                    &response,
                    counters,
                    &event_scope,
                    &refusal_budget,
                    |bytes| dispatcher.reserve_response_memory(&refusal_budget, bytes),
                ) {
                    Ok(()) => {
                        event_scope.finish_with_outcome(
                            RequestEventStageV1::ResponseWritten,
                            DiagnosticTerminalV1::Error { code: 2 },
                        );
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
        let mut budget = RequestBudgetV1::for_duration(policy.dispatch_budget())
            .with_correlation(RequestCorrelationV1::from_admitted(request_id))
            .with_diagnostics(diagnostics);
        if let Some(trace) = event_scope.diagnostic_context() {
            budget = budget.with_diagnostic_context(trace.clone());
        }
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
            deadline: budget.deadline(),
            cancellation: budget.cancel_handle(),
            events: event_sink,
            request_started: event_scope.started,
            diagnostic_context: event_scope.diagnostic_context().cloned(),
            diagnostic_backend_error: Arc::clone(&event_scope.backend_error),
        };
        if RequestEnvelopeT::streaming(&request_payload) {
            let _in_flight =
                crate::counters::InFlightDispatchGuard::start(counters, permit.waited());
            let watch = match PeerWatch::arm(&stream, budget.cancel_handle(), Arc::clone(counters))
            {
                Ok(watch) => watch,
                Err(error) => {
                    event_scope.finish(RequestEventStageV1::PeerWatchFailed);
                    return ConnectionCloseReason::PeerWatchFailed(error.to_string());
                }
            };
            event_scope.emit(RequestEventStageV1::DispatchStarted);
            let backend_span = event_scope.enter(DiagnosticStageV1::Backend);
            let mut stream_failure = None;
            let mut terminal_written = false;
            let terminal = {
                let mut emit = |payload: Response| -> Result<(), IpcError> {
                    if stream_failure.is_some() {
                        return Err(IpcError::Io(std::io::Error::other(
                            "progressive response stream already failed",
                        )));
                    }
                    if terminal_written
                        || !(ResponseEnvelopeT::stream_stage(&payload)
                            || ResponseEnvelopeT::stream_terminal(&payload))
                    {
                        stream_failure = Some(ConnectionCloseReason::ResponseEncodeFailed(
                            IpcError::Encode("invalid or post-terminal stream payload".into()),
                        ));
                        return Err(IpcError::Encode(
                            "invalid or post-terminal stream payload".into(),
                        ));
                    }
                    let is_terminal = ResponseEnvelopeT::stream_terminal(&payload);
                    let response = ResponseEnvelopeT::from_parts(request_id.get(), payload);
                    match write_response(
                        &mut stream,
                        &response,
                        counters,
                        &event_scope,
                        &budget,
                        |bytes| dispatcher.reserve_response_memory(&budget, bytes),
                    ) {
                        Ok(()) => {
                            terminal_written = is_terminal;
                            Ok(())
                        }
                        Err(reason) => {
                            let message = reason.to_string();
                            stream_failure = Some(reason);
                            budget.cancel_handle().cancel();
                            Err(IpcError::Io(std::io::Error::other(message)))
                        }
                    }
                };
                dispatcher.dispatch_stream(&context, request_payload, &budget, &mut emit)
            };
            if let Some(span) = backend_span {
                let outcome = if event_scope.backend_error() {
                    DiagnosticTerminalV1::Error { code: 4 }
                } else {
                    DiagnosticTerminalV1::Ok
                };
                let _diagnostic_result = span.finish(outcome);
            }
            event_scope.emit(RequestEventStageV1::DispatchReturned);
            if let Some(reason) = stream_failure {
                event_scope.finish(RequestEventStageV1::ResponseWriteFailed);
                return reason;
            }
            if terminal_written {
                if !matches!(terminal, StreamDispatch::EmittedTerminal) {
                    event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                    return ConnectionCloseReason::ResponseEncodeFailed(IpcError::Encode(
                        "stream dispatcher returned a second terminal".into(),
                    ));
                }
                let peer_hung_up = match watch.disarm() {
                    Ok(PeerWatchOutcome::HungUp) => true,
                    Ok(PeerWatchOutcome::Stopped) => false,
                    Err(error) => {
                        event_scope.finish(RequestEventStageV1::PeerWatchFailed);
                        return ConnectionCloseReason::PeerWatchFailed(error);
                    }
                };
                drop(permit);
                counters.request_dispatched(peer_hung_up);
                if peer_hung_up {
                    event_scope.finish(RequestEventStageV1::PeerCancelled);
                    return ConnectionCloseReason::PeerClosed;
                }
                event_scope.finish(RequestEventStageV1::ResponseWritten);
                continue;
            }
            let response = match terminal {
                StreamDispatch::Terminal(payload)
                    if ResponseEnvelopeT::stream_terminal(&payload) =>
                {
                    ResponseEnvelopeT::from_parts(request_id.get(), payload)
                }
                StreamDispatch::Terminal(_) => {
                    event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                    return ConnectionCloseReason::ResponseEncodeFailed(IpcError::Encode(
                        "stream returned a nonterminal payload".into(),
                    ));
                }
                StreamDispatch::Unsupported => {
                    let Some(refusal) = ResponseEnvelopeT::stream_unsupported(request_id.get())
                    else {
                        event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                        return ConnectionCloseReason::ResponseEncodeFailed(IpcError::Encode(
                            "stream unsupported without typed refusal".into(),
                        ));
                    };
                    refusal
                }
                StreamDispatch::EmittedTerminal => {
                    event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                    return ConnectionCloseReason::ResponseEncodeFailed(IpcError::Encode(
                        "stream dispatcher omitted terminal frame".into(),
                    ));
                }
            };
            let written = write_response(
                &mut stream,
                &response,
                counters,
                &event_scope,
                &budget,
                |bytes| dispatcher.reserve_response_memory(&budget, bytes),
            );
            let peer_hung_up = match watch.disarm() {
                Ok(PeerWatchOutcome::HungUp) => true,
                Ok(PeerWatchOutcome::Stopped) => false,
                Err(error) => {
                    event_scope.finish(RequestEventStageV1::PeerWatchFailed);
                    return ConnectionCloseReason::PeerWatchFailed(error);
                }
            };
            drop(permit);
            counters.request_dispatched(peer_hung_up);
            if let Err(reason) = written {
                event_scope.finish(RequestEventStageV1::ResponseWriteFailed);
                return reason;
            }
            if peer_hung_up {
                event_scope.finish(RequestEventStageV1::PeerCancelled);
                return ConnectionCloseReason::PeerClosed;
            }
            event_scope.finish(RequestEventStageV1::ResponseWritten);
            continue;
        }
        // The dispatch slot and the in-flight count are one RAII pair
        // (S21-09): a panicking dispatcher unwinds through both drops,
        // and the peer watch stops and joins its thread the same way.
        let (response_payload, watch) = {
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
            let backend_span = event_scope.enter(DiagnosticStageV1::Backend);
            let response_payload = dispatcher.dispatch(&context, request_payload, &budget);
            if let Some(span) = backend_span {
                let outcome = if event_scope.backend_error() {
                    DiagnosticTerminalV1::Error { code: 4 }
                } else {
                    DiagnosticTerminalV1::Ok
                };
                let _diagnostic_result = span.finish(outcome);
            }
            event_scope.emit(RequestEventStageV1::DispatchReturned);
            (response_payload, watch)
        };
        drop(permit);
        let peer_hung_up = watch.peer_hung_up();
        counters.request_dispatched(peer_hung_up);
        if peer_hung_up {
            if let Err(error) = watch.disarm() {
                event_scope.finish(RequestEventStageV1::PeerWatchFailed);
                return ConnectionCloseReason::PeerWatchFailed(error);
            }
            // Nothing to write to; the dispatcher already saw the
            // cancellation at its next checkpoint (or ran to completion).
            event_scope.finish(RequestEventStageV1::PeerCancelled);
            return ConnectionCloseReason::PeerClosed;
        }
        let response = ResponseEnvelopeT::from_parts(request_id.get(), response_payload);
        let reply_budget = if response.terminal_reply_phase() {
            let deadline = Instant::now()
                .checked_add(policy.io_timeout())
                .unwrap_or_else(Instant::now);
            let mut reply =
                RequestBudgetV1::until_with_external_cancellation(deadline, watch.hangup_flag());
            if let Some(correlation) = budget.correlation() {
                reply = reply.with_correlation(correlation);
            }
            if let Some(diagnostic) = budget.diagnostic_context() {
                reply = reply.with_diagnostic_context(diagnostic.clone());
            }
            reply
        } else {
            budget.clone()
        };
        let mut encode_span = event_scope.enter(DiagnosticStageV1::Encode);
        let mut response_too_large = false;
        let frame =
            match crate::codec::encode_response_admitted(&response, &reply_budget, |bytes| {
                dispatcher.reserve_response_memory(&reply_budget, bytes)
            }) {
                Ok(frame) => frame,
                // The answer was computed but cannot cross the wire. Tell the
                // caller so with a typed refusal instead of dropping the
                // connection, which would be indistinguishable from a crash.
                Err(IpcError::Oversized(encoded_bytes)) => {
                    response_too_large = true;
                    let limit_bytes =
                        u64::try_from(MAX_FRAME_BODY_BYTES).map_or(u64::MAX, |limit| limit);
                    let Some(refusal) = ResponseEnvelopeT::result_too_large(
                        request_id.get(),
                        encoded_bytes,
                        limit_bytes,
                    ) else {
                        if let Some(span) = encode_span.take() {
                            let _diagnostic_result =
                                span.finish(DiagnosticTerminalV1::Error { code: 5 });
                        }
                        event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                        return ConnectionCloseReason::ResponseEncodeFailed(IpcError::Oversized(
                            encoded_bytes,
                        ));
                    };
                    match crate::codec::encode_response_admitted(&refusal, &reply_budget, |bytes| {
                        dispatcher.reserve_response_memory(&reply_budget, bytes)
                    }) {
                        Ok(frame) => frame,
                        Err(err) => {
                            if let Some(span) = encode_span.take() {
                                let _diagnostic_result =
                                    span.finish(DiagnosticTerminalV1::Error { code: 5 });
                            }
                            event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                            return ConnectionCloseReason::ResponseEncodeFailed(err);
                        }
                    }
                }
                Err(err) => {
                    if let Some(span) = encode_span.take() {
                        let _diagnostic_result =
                            span.finish(DiagnosticTerminalV1::Error { code: 5 });
                    }
                    event_scope.finish(RequestEventStageV1::ResponseEncodeFailed);
                    return ConnectionCloseReason::ResponseEncodeFailed(err);
                }
            };
        if let Some(span) = encode_span.take() {
            let _diagnostic_result = span.finish(DiagnosticTerminalV1::Ok);
        }
        let write_span = event_scope.enter(DiagnosticStageV1::Write);
        if let Err(err) = write_all_before(&mut stream, &frame, &reply_budget) {
            if let Some(span) = write_span {
                let _diagnostic_result = span.finish(DiagnosticTerminalV1::Error { code: 3 });
            }
            event_scope.finish(RequestEventStageV1::ResponseWriteFailed);
            return ConnectionCloseReason::ResponseWriteFailed(err.to_string());
        }
        if let Some(span) = write_span {
            let _diagnostic_result = span.finish(DiagnosticTerminalV1::Ok);
        }
        counters.response_written(frame_bytes(&frame));
        if let Err(error) = watch.disarm() {
            event_scope.finish(RequestEventStageV1::PeerWatchFailed);
            return ConnectionCloseReason::PeerWatchFailed(error);
        }
        if response_too_large {
            event_scope.finish_with_outcome(
                RequestEventStageV1::ResponseWritten,
                DiagnosticTerminalV1::Error { code: 5 },
            );
        } else {
            event_scope.finish(RequestEventStageV1::ResponseWritten);
        }
        // continue: next request on same conn
    }
}

#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "preserves the established untraced test call shape"
)]
fn handle_connection<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
    stream: UnixStream,
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
    handle_connection_traced::<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
        stream,
        dispatcher,
        slots,
        policy,
        plane,
        principal,
        owner_uid,
        connection_id,
        shutdown,
        counters,
        None,
    )
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

fn read_exact_transport(reader: &mut impl Read, bytes: &mut [u8]) -> Result<(), IpcError> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            IpcError::Truncated
        } else {
            IpcError::Io(error)
        }
    })
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
    scope: &RequestEventScope<'_>,
    budget: &RequestBudgetV1,
    admit: impl FnOnce(u64) -> Result<Option<quanta_index_core::index::MemoryLease>, IpcError>,
) -> Result<(), ConnectionCloseReason> {
    let encode_span = scope.enter(DiagnosticStageV1::Encode);
    let frame = match crate::codec::encode_response_admitted(response, budget, admit) {
        Ok(frame) => {
            if let Some(span) = encode_span {
                let _diagnostic_result = span.finish(DiagnosticTerminalV1::Ok);
            }
            frame
        }
        Err(error) => {
            if let Some(span) = encode_span {
                let _diagnostic_result = span.finish(DiagnosticTerminalV1::Error { code: 5 });
            }
            return Err(ConnectionCloseReason::ResponseEncodeFailed(error));
        }
    };
    let write_span = scope.enter(DiagnosticStageV1::Write);
    if let Err(error) = write_all_before(stream, &frame, budget) {
        if let Some(span) = write_span {
            let _diagnostic_result = span.finish(DiagnosticTerminalV1::Error { code: 3 });
        }
        return Err(ConnectionCloseReason::ResponseWriteFailed(
            error.to_string(),
        ));
    }
    if let Some(span) = write_span {
        let _diagnostic_result = span.finish(DiagnosticTerminalV1::Ok);
    }
    counters.response_written(frame_bytes(&frame));
    Ok(())
}

fn write_all_before(
    stream: &mut UnixStream,
    bytes: &[u8],
    budget: &RequestBudgetV1,
) -> std::io::Result<()> {
    let deadline = budget.deadline();
    let mut remaining_bytes = bytes;
    while !remaining_bytes.is_empty() {
        budget
            .checkpoint("ipc_response_write")
            .map_err(|error| std::io::Error::new(ErrorKind::Interrupted, error))?;
        let remaining_time = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(deadline_elapsed_error)?;
        // A blocked socket periodically observes cancellation, without changing
        // the absolute deadline or retrying a partially written response frame.
        stream.set_write_timeout(Some(remaining_time.min(Duration::from_millis(50))))?;
        let written = match stream.write(remaining_bytes) {
            Ok(written) => written,
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        if written == 0 {
            return Err(std::io::Error::new(
                ErrorKind::WriteZero,
                "IPC response made no progress",
            ));
        }
        remaining_bytes = remaining_bytes.get(written..).ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::InvalidData,
                "IPC write reported more bytes than supplied",
            )
        })?;
    }
    budget
        .checkpoint("ipc_response_write")
        .map_err(|error| std::io::Error::new(ErrorKind::Interrupted, error))?;
    ensure_deadline_remaining(deadline)
}

mod peer_watch;

use peer_watch::{PeerWatch, PeerWatchOutcome};
#[cfg(test)]
use peer_watch::{WatchEvent, WatchObserver};

/// Request-local client timings for one successful one-shot IPC call.
///
/// All six `u64` values are durations in nanoseconds. `read_io` is included in
/// `decode_call`; their sum is not a total. The unallocated part of `total`
/// includes deadline checks and local work.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClientIpcTimingV1 {
    pub total: u64,
    pub encode: u64,
    pub connect: u64,
    pub write: u64,
    pub decode_call: u64,
    pub read_io: u64,
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

enum ClientRequestFrame {
    Standalone(Vec<u8>),
    Admitted(AdmittedClientRequestFrame),
}

impl ClientRequestFrame {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Standalone(bytes) => bytes,
            Self::Admitted(frame) => frame,
        }
    }
}

fn encode_client_request_frame<T: serde::Serialize>(
    request: &T,
    budget: Option<&RequestBudgetV1>,
    memory: Option<&MemoryScope>,
) -> Result<ClientRequestFrame, IpcError> {
    match (budget, memory) {
        (Some(budget), Some(memory)) => {
            encode_request_single_frame_admitted(request, budget, memory)
                .map(ClientRequestFrame::Admitted)
        }
        (_, None) => encode_request(request).map(ClientRequestFrame::Standalone),
        (None, Some(_)) => Err(IpcError::Encode(
            "client memory scope requires the original request budget".into(),
        )),
    }
}

trait ClientResponseDecode {
    type Output;

    fn read<R: Read>(&self, reader: &mut R) -> Result<Self::Output, IpcError>;
}

struct TypedClientResponse<T>(std::marker::PhantomData<T>);

impl<T: serde::de::DeserializeOwned> ClientResponseDecode for TypedClientResponse<T> {
    type Output = T;

    fn read<R: Read>(&self, reader: &mut R) -> Result<T, IpcError> {
        decode_response(reader)
    }
}

type AdmittedResponseWork<'a> =
    std::cell::RefCell<&'a mut dyn FnMut(usize) -> Result<(), IpcError>>;

struct AdmittedRawClientResponse<'a> {
    budget: &'a RequestBudgetV1,
    memory: &'a MemoryScope,
    work: Option<AdmittedResponseWork<'a>>,
}

impl ClientResponseDecode for AdmittedRawClientResponse<'_> {
    type Output = AdmittedClientResponseBody;

    fn read<R: Read>(&self, reader: &mut R) -> Result<Self::Output, IpcError> {
        match &self.work {
            Some(work) => read_response_body_admitted_with_work_v1(
                reader,
                self.budget,
                self.memory,
                &mut **work.borrow_mut(),
            ),
            None => read_response_body_admitted(reader, self.budget, self.memory),
        }
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
    send_request_inner::<RequestEnvelopeT, RequestEnvelopeT, _>(
        socket,
        request,
        None,
        io_policy,
        None,
        None,
        None,
        &TypedClientResponse::<ResponseEnvelopeT>(std::marker::PhantomData),
    )
}

/// One-shot client with the original request's deadline and cancellation.
///
/// A cancelled in-flight write can have reached the peer; callers of durable
/// operations must retain their operation key and reconcile an uncertain ACK.
pub fn send_request_with_budget<RequestEnvelopeT, ResponseEnvelopeT>(
    socket: &Path,
    request: &RequestEnvelopeT,
    budget: &RequestBudgetV1,
) -> Result<ResponseEnvelopeT, IpcError>
where
    RequestEnvelopeT: serde::Serialize,
    ResponseEnvelopeT: serde::de::DeserializeOwned,
{
    check_stream_budget(budget).map_err(IpcError::Io)?;
    let io_policy = ClientIoPolicy::try_with_deadline(budget.deadline())?;
    send_request_inner::<RequestEnvelopeT, RequestEnvelopeT, _>(
        socket,
        request,
        None,
        io_policy,
        None,
        Some(budget),
        None,
        &TypedClientResponse::<ResponseEnvelopeT>(std::marker::PhantomData),
    )
}

/// Send one bounded request and read its raw response under the original budget.
///
/// The returned body retains its charge;
/// parsing it into owned values requires a separate type-specific admission.
pub fn send_request_body_with_budget_admitted<RequestEnvelopeT: serde::Serialize>(
    socket: &Path,
    request: &RequestEnvelopeT,
    budget: &RequestBudgetV1,
    memory: &MemoryScope,
) -> Result<AdmittedClientResponseBody, IpcError> {
    check_stream_budget(budget).map_err(IpcError::Io)?;
    let io_policy = ClientIoPolicy::try_with_deadline(budget.deadline())?;
    send_request_inner::<RequestEnvelopeT, RequestEnvelopeT, _>(
        socket,
        request,
        None,
        io_policy,
        None,
        Some(budget),
        Some(memory),
        &AdmittedRawClientResponse {
            budget,
            memory,
            work: None,
        },
    )
}

/// Admitted one-shot request whose response-body copy is charged to the
/// caller's work owner before any response backing is allocated.
pub fn send_request_body_with_budget_admitted_with_work_v1<RequestEnvelopeT: serde::Serialize>(
    socket: &Path,
    request: &RequestEnvelopeT,
    budget: &RequestBudgetV1,
    memory: &MemoryScope,
    work: &mut dyn FnMut(usize) -> Result<(), IpcError>,
) -> Result<AdmittedClientResponseBody, IpcError> {
    check_stream_budget(budget).map_err(IpcError::Io)?;
    let io_policy = ClientIoPolicy::try_with_deadline(budget.deadline())?;
    send_request_inner::<RequestEnvelopeT, RequestEnvelopeT, _>(
        socket,
        request,
        None,
        io_policy,
        None,
        Some(budget),
        Some(memory),
        &AdmittedRawClientResponse {
            budget,
            memory,
            work: Some(std::cell::RefCell::new(work)),
        },
    )
}

/// Admitted auxiliary one-shot request with the existing diagnostic
/// capability exchange. Both the plain and accepted linked frame use the
/// same original request budget and retained memory scope.
pub fn send_request_body_negotiated_with_budget_admitted<
    RequestEnvelopeT: serde::Serialize,
    LinkedEnvelopeT: serde::Serialize,
>(
    socket: &Path,
    plain_request: &RequestEnvelopeT,
    linked_request: &LinkedEnvelopeT,
    capture_id: [u8; 16],
    budget: &RequestBudgetV1,
    memory: &MemoryScope,
) -> Result<AdmittedClientResponseBody, IpcError> {
    check_stream_budget(budget).map_err(IpcError::Io)?;
    let io_policy = ClientIoPolicy::try_with_deadline(budget.deadline())?;
    send_request_inner(
        socket,
        plain_request,
        Some((linked_request, capture_id)),
        io_policy,
        None,
        Some(budget),
        Some(memory),
        &AdmittedRawClientResponse {
            budget,
            memory,
            work: None,
        },
    )
}

/// Negotiated admitted request with a pre-allocation response-copy work
/// callback. Uses the same capability exchange and frame codec as the
/// ordinary admitted request.
pub fn send_request_body_negotiated_with_budget_admitted_with_work_v1<
    RequestEnvelopeT: serde::Serialize,
    LinkedEnvelopeT: serde::Serialize,
>(
    socket: &Path,
    plain_request: &RequestEnvelopeT,
    linked_request: &LinkedEnvelopeT,
    capture_id: [u8; 16],
    budget: &RequestBudgetV1,
    memory: &MemoryScope,
    work: &mut dyn FnMut(usize) -> Result<(), IpcError>,
) -> Result<AdmittedClientResponseBody, IpcError> {
    check_stream_budget(budget).map_err(IpcError::Io)?;
    let io_policy = ClientIoPolicy::try_with_deadline(budget.deadline())?;
    send_request_inner(
        socket,
        plain_request,
        Some((linked_request, capture_id)),
        io_policy,
        None,
        Some(budget),
        Some(memory),
        &AdmittedRawClientResponse {
            budget,
            memory,
            work: Some(std::cell::RefCell::new(work)),
        },
    )
}

/// One connection and one absolute deadline for a progressive response.
///
/// Dropping the reader closes the connection and cancels server work via peer watch.
pub struct ClientResponseStream {
    stream: DeadlineStream,
    policy: ClientIoPolicy,
}

impl ClientResponseStream {
    pub fn next_response<ResponseEnvelopeT: serde::de::DeserializeOwned>(
        &mut self,
    ) -> Result<ResponseEnvelopeT, IpcError> {
        let response =
            decode_response::<ResponseEnvelopeT, _>(&mut self.stream).map_err(|error| {
                classify_client_decode_error(error, IpcIoOperation::Read, self.policy)
            })?;
        let _remaining = self
            .stream
            .remaining()
            .map_err(|error| classify_client_io_error(error, IpcIoOperation::Read, self.policy))?;
        Ok(response)
    }
}

pub fn send_stream_request<RequestEnvelopeT: serde::Serialize>(
    socket: &Path,
    request: &RequestEnvelopeT,
    budget: &RequestBudgetV1,
) -> Result<ClientResponseStream, IpcError> {
    check_stream_budget(budget).map_err(IpcError::Io)?;
    let io_policy = ClientIoPolicy::try_with_deadline(budget.deadline())?;
    let deadline = io_policy.request_deadline()?;
    let frame = encode_request(request)?;
    check_stream_budget(budget).map_err(IpcError::Io)?;
    let stream = connect_stream_before_budget(socket, budget)
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Connect, io_policy))?;
    let mut stream = DeadlineStream::new(stream, deadline);
    stream.budget = Some(budget.clone());
    stream
        .write_all(&frame)
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Write, io_policy))?;
    check_stream_budget(budget).map_err(IpcError::Io)?;
    Ok(ClientResponseStream {
        stream,
        policy: io_policy,
    })
}

/// Send a diagnostic token only after the peer acknowledges this capture.
///
/// A peer's acknowledgement must arrive on the same stream. An older peer's rejection uses a fresh stream
/// for the original envelope under the same absolute request deadline.
pub fn send_request_negotiated<RequestEnvelopeT, LinkedEnvelopeT, ResponseEnvelopeT>(
    socket: &Path,
    plain_request: &RequestEnvelopeT,
    linked_request: &LinkedEnvelopeT,
    capture_id: [u8; 16],
    io_policy: ClientIoPolicy,
) -> Result<ResponseEnvelopeT, IpcError>
where
    RequestEnvelopeT: serde::Serialize,
    LinkedEnvelopeT: serde::Serialize,
    ResponseEnvelopeT: serde::de::DeserializeOwned,
{
    send_request_inner(
        socket,
        plain_request,
        Some((linked_request, capture_id)),
        io_policy,
        None,
        None,
        None,
        &TypedClientResponse::<ResponseEnvelopeT>(std::marker::PhantomData),
    )
}

/// The same one-shot request with opt-in, request-local client attribution.
///
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
    let response = send_request_inner::<RequestEnvelopeT, RequestEnvelopeT, _>(
        socket,
        request,
        None,
        io_policy,
        Some(&mut timing),
        None,
        None,
        &TypedClientResponse::<ResponseEnvelopeT>(std::marker::PhantomData),
    )?;
    Ok((response, timing))
}

pub fn send_request_observed_negotiated<RequestEnvelopeT, LinkedEnvelopeT, ResponseEnvelopeT>(
    socket: &Path,
    plain_request: &RequestEnvelopeT,
    linked_request: &LinkedEnvelopeT,
    capture_id: [u8; 16],
    io_policy: ClientIoPolicy,
) -> Result<(ResponseEnvelopeT, ClientIpcTimingV1), IpcError>
where
    RequestEnvelopeT: serde::Serialize,
    LinkedEnvelopeT: serde::Serialize,
    ResponseEnvelopeT: serde::de::DeserializeOwned,
{
    let mut timing = ClientIpcTimingV1::default();
    let response = send_request_inner(
        socket,
        plain_request,
        Some((linked_request, capture_id)),
        io_policy,
        Some(&mut timing),
        None,
        None,
        &TypedClientResponse::<ResponseEnvelopeT>(std::marker::PhantomData),
    )?;
    Ok((response, timing))
}

fn send_request_inner<RequestEnvelopeT, LinkedEnvelopeT, Decode>(
    socket: &Path,
    request: &RequestEnvelopeT,
    linked_request: Option<(&LinkedEnvelopeT, [u8; 16])>,
    io_policy: ClientIoPolicy,
    mut timing: Option<&mut ClientIpcTimingV1>,
    budget: Option<&RequestBudgetV1>,
    memory: Option<&MemoryScope>,
    decoder: &Decode,
) -> Result<Decode::Output, IpcError>
where
    RequestEnvelopeT: serde::Serialize,
    LinkedEnvelopeT: serde::Serialize,
    Decode: ClientResponseDecode,
{
    if let Some(budget) = budget {
        check_stream_budget(budget).map_err(IpcError::Io)?;
    }
    let total_started = timing.as_ref().map(|_| Instant::now());
    let deadline = io_policy.request_deadline()?;
    let encode_started = timing.as_ref().map(|_| Instant::now());
    let frame = encode_client_request_frame(request, budget, memory)?;
    if let Some(budget) = budget {
        check_stream_budget(budget).map_err(IpcError::Io)?;
    }
    if let (Some(timing), Some(started)) = (timing.as_deref_mut(), encode_started) {
        timing.encode = observed_ns(started.elapsed())?;
    }
    let connect_started = timing.as_ref().map(|_| Instant::now());
    let stream = budget
        .map_or_else(
            || connect_before_deadline(socket, deadline),
            |budget| connect_stream_before_budget(socket, budget),
        )
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Connect, io_policy))?;
    if let (Some(timing), Some(started)) = (timing.as_deref_mut(), connect_started) {
        timing.connect = observed_ns(started.elapsed())?;
    }
    let mut stream = DeadlineStream::new(stream, deadline);
    stream.budget = budget.cloned();
    let selected_request = if let Some((linked, capture_id)) = linked_request {
        // Keep 100 ms of the original request budget for a plain-envelope
        // fallback. The optional probe may use at most another 100 ms.
        const FALLBACK_RESERVE: Duration = Duration::from_millis(100);
        const MAX_PROBE_TIME: Duration = Duration::from_millis(100);
        let probe_started = Instant::now();
        let probe_deadline = deadline
            .checked_sub(FALLBACK_RESERVE)
            .filter(|limit| *limit > probe_started)
            .map(|limit| limit.min(probe_started.checked_add(MAX_PROBE_TIME).unwrap_or(limit)));
        if let Some(probe_deadline) = probe_deadline {
            stream.deadline = probe_deadline;
            let negotiated: std::io::Result<bool> = (|| {
                stream.write_all(&diagnostic_preface(capture_id))?;
                let mut ack = [0_u8; DIAGNOSTIC_ACK_BYTES];
                stream.read_exact(&mut ack)?;
                let accepted = diagnostic_ack_accepted(&ack).ok_or_else(|| {
                    std::io::Error::new(
                        ErrorKind::InvalidData,
                        "invalid diagnostic acknowledgement",
                    )
                })?;
                ensure_deadline_remaining(probe_deadline)?;
                Ok(accepted)
            })();
            stream.deadline = deadline;
            match negotiated {
                Ok(true) => Some(linked),
                Ok(false) => None,
                Err(_) => {
                    // The first stream contained transport control only. The
                    // original product envelope starts on a fresh stream.
                    drop(stream);
                    let retry = budget
                        .map_or_else(
                            || connect_before_deadline(socket, deadline),
                            |budget| connect_stream_before_budget(socket, budget),
                        )
                        .map_err(|error| {
                            classify_client_io_error(error, IpcIoOperation::Connect, io_policy)
                        })?;
                    stream = DeadlineStream::new(retry, deadline);
                    stream.budget = budget.cloned();
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };
    let frame = if let Some(linked) = selected_request {
        encode_client_request_frame(linked, budget, memory)?
    } else {
        frame
    };
    let write_started = timing.as_ref().map(|_| Instant::now());
    stream
        .write_all(frame.as_slice())
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Write, io_policy))?;
    if let Some(budget) = budget {
        check_stream_budget(budget).map_err(IpcError::Io)?;
    }
    if let (Some(timing), Some(started)) = (timing.as_deref_mut(), write_started) {
        timing.write = observed_ns(started.elapsed())?;
    }
    let decode_started = timing.as_ref().map(|_| Instant::now());
    let mut read_io = Duration::ZERO;
    let response = if timing.is_some() {
        let mut reader = TimedResponseReader {
            inner: &mut stream,
            read_io: Duration::ZERO,
        };
        let result = decoder.read(&mut reader);
        read_io = reader.read_io;
        result
    } else {
        decoder.read(&mut stream)
    }
    .map_err(|error| classify_client_decode_error(error, IpcIoOperation::Read, io_policy))?;
    if let Some(budget) = budget {
        check_stream_budget(budget).map_err(IpcError::Io)?;
    }
    if let (Some(timing), Some(started)) = (timing.as_deref_mut(), decode_started) {
        timing.decode_call = observed_ns(started.elapsed())?;
        timing.read_io = observed_ns(read_io)?;
    }
    ensure_deadline_remaining(deadline)
        .map_err(|error| classify_client_io_error(error, IpcIoOperation::Read, io_policy))?;
    if let (Some(timing), Some(started)) = (timing, total_started) {
        timing.total = observed_ns(started.elapsed())?;
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

fn check_stream_budget(budget: &RequestBudgetV1) -> std::io::Result<()> {
    if budget.is_cancelled() {
        // `read_exact` retries Interrupted indefinitely. Other must escape
        // the decoder so a cancelled original request ends this stream.
        return Err(std::io::Error::other(
            "IPC stream cancelled by original request",
        ));
    }
    if budget.remaining().is_zero() {
        return Err(deadline_elapsed_error());
    }
    Ok(())
}

fn connect_stream_before_budget(
    socket_path: &Path,
    budget: &RequestBudgetV1,
) -> std::io::Result<UnixStream> {
    check_stream_budget(budget)?;
    let address = SocketAddrUnix::new(socket_path).map_err(std::io::Error::from)?;
    let socket = create_connect_socket()?;
    check_stream_budget(budget)?;
    match connect(&socket, &address) {
        Ok(()) => {}
        Err(error) if connect_requires_completion_wait(error) => {
            wait_for_stream_connect(&socket, budget)?;
        }
        Err(error) => {
            check_stream_budget(budget)?;
            return Err(error.into());
        }
    }
    check_stream_budget(budget)?;
    let stream = UnixStream::from(socket);
    stream.set_nonblocking(false)?;
    check_stream_budget(budget)?;
    Ok(stream)
}

fn wait_for_stream_connect(socket: &OwnedFd, budget: &RequestBudgetV1) -> std::io::Result<()> {
    let mut poll_fd = [PollFd::new(socket, PollFlags::OUT)];
    loop {
        check_stream_budget(budget)?;
        let wait = budget.remaining().min(Duration::from_millis(1));
        let timeout = Timespec::try_from(wait)
            .map_err(|error| std::io::Error::new(ErrorKind::InvalidInput, error))?;
        let result = poll(&mut poll_fd, Some(&timeout));
        check_stream_budget(budget)?;
        match result {
            Ok(0) | Err(Errno::INTR) => {}
            Ok(_) => {
                let completion = socket_error(socket)
                    .map_err(std::io::Error::from)?
                    .map_err(std::io::Error::from);
                check_stream_budget(budget)?;
                return completion;
            }
            Err(error) => return Err(error.into()),
        }
    }
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
    budget: Option<RequestBudgetV1>,
}

impl DeadlineStream {
    const fn new(stream: UnixStream, deadline: Instant) -> Self {
        Self {
            stream,
            deadline,
            budget: None,
        }
    }

    fn remaining(&self) -> std::io::Result<Duration> {
        if self
            .budget
            .as_ref()
            .is_some_and(RequestBudgetV1::is_cancelled)
        {
            return Err(std::io::Error::other(
                "IPC stream cancelled by original request",
            ));
        }
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
        loop {
            let remaining = self.remaining()?;
            let wait = if self.budget.is_some() {
                remaining.min(Duration::from_millis(100))
            } else {
                remaining
            };
            match self.stream.set_read_timeout(Some(wait)) {
                Ok(()) => {}
                Err(error)
                    if cfg!(target_os = "macos") && error.kind() == ErrorKind::InvalidInput =>
                {
                    // Darwin can reject SO_RCVTIMEO after the peer has closed,
                    // even when a complete response is still buffered. Poll the
                    // same socket under the original absolute deadline and let
                    // read/decode distinguish buffered bytes from a truncated EOF.
                    let mut poll_fd = [PollFd::new(&self.stream, PollFlags::IN | PollFlags::HUP)];
                    loop {
                        let remaining = self.remaining()?;
                        let wait = if self.budget.is_some() {
                            remaining.min(Duration::from_millis(100))
                        } else {
                            remaining
                        };
                        let timeout = Timespec::try_from(wait).map_err(|reason| {
                            std::io::Error::new(ErrorKind::InvalidInput, reason)
                        })?;
                        match poll(&mut poll_fd, Some(&timeout)) {
                            Ok(0) if self.budget.is_some() => {}
                            Ok(0) => return Err(deadline_elapsed_error()),
                            Ok(_) => break,
                            Err(Errno::INTR) => {}
                            Err(reason) => return Err(reason.into()),
                        }
                    }
                }
                Err(error) => return Err(error),
            }
            match self.stream.read(buffer) {
                Err(error)
                    if self.budget.is_some()
                        && matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                outcome => {
                    if self.budget.is_some() {
                        let _remaining = self.remaining()?;
                    }
                    return outcome;
                }
            }
        }
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.budget.is_none() {
            let remaining = self.remaining()?;
            self.stream.set_write_timeout(Some(remaining))?;
            return self.stream.write(buffer);
        }
        loop {
            let remaining = self.remaining()?;
            let wait = remaining.min(Duration::from_millis(100));
            self.stream.set_write_timeout(Some(wait))?;
            match self.stream.write(buffer) {
                Err(error)
                    if self.budget.is_some()
                        && matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                outcome => {
                    let _remaining = self.remaining()?;
                    return outcome;
                }
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.budget.is_none() {
            let remaining = self.remaining()?;
            self.stream.set_write_timeout(Some(remaining))?;
            return self.stream.flush();
        }
        loop {
            let remaining = self.remaining()?;
            let wait = remaining.min(Duration::from_millis(100));
            self.stream.set_write_timeout(Some(wait))?;
            match self.stream.flush() {
                Err(error)
                    if self.budget.is_some()
                        && matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                outcome => {
                    let _remaining = self.remaining()?;
                    return outcome;
                }
            }
        }
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
        | IpcError::FrameMemoryLimited { .. }
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
mod admitted_client_tests {
    use super::{IpcError, send_request_body_with_budget_admitted};
    use quanta_index_core::index::{
        IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
    };
    use std::{
        io::Write,
        os::unix::net::UnixListener,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    struct Owner(Arc<AtomicU64>);

    impl MemoryLeaseOwner for Owner {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
        }
    }

    struct Scope {
        active: Arc<AtomicU64>,
        deny: bool,
    }

    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if self.deny || lifetime != MemoryLifetime::Retained {
                return Err(IndexError::ResourceLimited);
            }
            let _previous = self.active.fetch_add(bytes, Ordering::SeqCst);
            Ok(MemoryLease::new(
                bytes,
                Arc::new(Owner(Arc::clone(&self.active))),
            ))
        }
    }

    fn scope(deny: bool) -> (MemoryScope, Arc<AtomicU64>) {
        let active = Arc::new(AtomicU64::new(0));
        (
            MemoryScope::new(Arc::new(Scope {
                active: Arc::clone(&active),
                deny,
            })),
            active,
        )
    }

    #[test]
    fn response_copy_work_refusal_precedes_body_read_and_retained_birth() {
        let budget = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(2));
        let (memory, active) = scope(false);
        let mut frame = std::io::Cursor::new([3_u8, 0, 0, 0, 11, 12, 13]);
        let mut charged = 0_usize;
        let result = crate::codec::read_response_body_admitted_with_work_v1(
            &mut frame,
            &budget,
            &memory,
            &mut |bytes| {
                charged = bytes;
                Err(IpcError::Io(std::io::ErrorKind::Interrupted.into()))
            },
        );
        assert!(
            matches!(result, Err(IpcError::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted)
        );
        assert_eq!(charged, 3);
        assert_eq!(frame.position(), 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn admitted_client_denial_precedes_connect() {
        let budget = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(2));
        let (memory, active) = scope(true);
        let result = send_request_body_with_budget_admitted(
            std::path::Path::new("/nonexistent/quanta-ipc-funded-client"),
            &7_u64,
            &budget,
            &memory,
        );
        assert!(matches!(result, Err(IpcError::FrameMemoryLimited { .. })));
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn admitted_client_real_uds_retains_body_after_request_frame_refund() {
        let directory = tempfile::tempdir().expect("socket directory");
        let socket = directory.path().join("admitted.sock");
        let listener = UnixListener::bind(&socket).expect("bind native socket");
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept admitted client");
            let request: u64 = crate::codec::decode_request(&mut stream).expect("decode request");
            assert_eq!(request, 7);
            let reply = crate::codec::encode_response(&11_u64).expect("encode response");
            stream.write_all(&reply).expect("write response");
        });
        let budget = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(2));
        let (memory, active) = scope(false);
        let body = send_request_body_with_budget_admitted(&socket, &7_u64, &budget, &memory)
            .expect("admitted UDS reply");
        peer.join().expect("scripted native peer");
        assert_eq!(
            crate::codec::decode_cbor_payload::<u64>(body.as_slice()).expect("scalar payload"),
            11
        );
        assert_eq!(
            active.load(Ordering::SeqCst),
            u64::try_from(body.as_slice().len()).expect("body length")
        );
        drop(body);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
mod tests;
