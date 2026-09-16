//! W5 admission proofs over the real control envelopes (QI-BB-002).
//!
//! The G0-R probe proves head-of-line freedom and peer-driven cancellation
//! with a private wire type. This target proves the other three admission
//! rules against `UdsServer::run` with the contract's own envelopes, so the
//! typed refusals a `searchctl` or producer client will actually decode are
//! what is asserted:
//!
//! 1. A full dispatch queue answers `SERVER_OVERLOADED` after the policy's
//!    queue wait — on the requester's own connection, without waiting for
//!    the holder — and serves again once the slot frees.
//! 2. Connections past the policy's cap are closed at accept and counted,
//!    never queued; a connection that drains frees its place.
//! 3. The budget the server hands a dispatch carries the policy's deadline:
//!    a dispatcher that outlives it sees `REQUEST_DEADLINE_EXCEEDED` at its
//!    next checkpoint, with the checkpoint named in the message.
//!
//! Every ordering claim is enforced with a channel or barrier handshake.

#![forbid(unsafe_code)]

use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::ipc::{
    CurrentGenerationRequest, ERR_SERVER_OVERLOADED, GenerationSnapshot,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponse, SearchPlaneControlIpcResponseEnvelope, SearchPlaneTrackKind,
};
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneIpcError};
use quanta_index_core::{CoreError, REQUEST_DEADLINE_EXCEEDED_CODE};
use quanta_index_ipc::{
    ClientIoPolicy, IpcDispatcher, IpcError, RequestBudgetV1, ServerAdmissionPolicy, UdsServer,
    send_request,
};

type TestResult = Result<(), Box<dyn Error>>;

const HANDSHAKE_BOUND: Duration = Duration::from_secs(20);
const ACCEPT_IDLE: Duration = Duration::from_millis(1);
/// Queue wait for the overload proof: long enough that a slot refusal is
/// unambiguously the policy firing, short enough to keep the test quick.
const QUEUE_WAIT: Duration = Duration::from_millis(120);
/// Dispatch budget for the deadline proof; the dispatcher sleeps past it.
const SHORT_DISPATCH_BUDGET: Duration = Duration::from_millis(60);
const OVERSLEEP: Duration = Duration::from_millis(150);

/// A repo id that parks the dispatch on the test's barrier.
const HOLD_REPO: &str = "hold";
/// A repo id that sleeps past the dispatch budget and then checkpoints.
const OVERSLEEP_REPO: &str = "oversleep";

struct ControlStub {
    entered: mpsc::Sender<()>,
    release: Arc<Barrier>,
    served: Arc<AtomicU64>,
}

impl IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse> for ControlStub {
    fn dispatch(
        &self,
        request: SearchPlaneControlIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneControlIpcResponse {
        let SearchPlaneControlIpcRequest::CurrentGeneration(request) = request else {
            return error_response(
                "TEST_UNEXPECTED_REQUEST",
                "only CurrentGeneration is stubbed",
            );
        };
        match request.repo_id.as_str() {
            HOLD_REPO => {
                if self.entered.send(()).is_err() {
                    return error_response("TEST_OBSERVER_GONE", "test side went away");
                }
                let _wait = self.release.wait();
            }
            OVERSLEEP_REPO => {
                thread::sleep(OVERSLEEP);
                // The checkpoint a route places after its native work.
                if let Err(err) = budget.checkpoint("after-native") {
                    return match err {
                        CoreError::Typed { code, message } => error_response(&code, message),
                        other @ (CoreError::InvalidContract(_)
                        | CoreError::NotReady(_)
                        | CoreError::NotImplemented(_)
                        | CoreError::NotFound(_)
                        | CoreError::Storage(_)) => {
                            error_response("TEST_UNEXPECTED_ERROR", other.to_string())
                        }
                    };
                }
            }
            _ => {}
        }
        let _served = self.served.fetch_add(1, Ordering::SeqCst);
        SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(GenerationSnapshot {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            track: request.track,
            manifest_generation: ManifestGeneration::new(1),
            manifest_digest: "digest".to_string(),
        })
    }
}

fn error_response(code: &str, message: impl Into<String>) -> SearchPlaneControlIpcResponse {
    SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
        code: code.to_string(),
        message: message.into(),
        repair: None,
    })
}

struct Harness {
    uds: Arc<UdsServer>,
    socket: std::path::PathBuf,
    join: Option<thread::JoinHandle<Result<(), IpcError>>>,
    entered: mpsc::Receiver<()>,
    release: Arc<Barrier>,
    served_count: Arc<AtomicU64>,
    _dir: tempfile::TempDir,
}

fn start_server(policy: ServerAdmissionPolicy) -> Result<Harness, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("admission.sock");
    let uds = Arc::new(UdsServer::bind_with_policy(&socket, policy)?);
    let (entered_tx, entered_rx) = mpsc::channel();
    let release = Arc::new(Barrier::new(2));
    let served_count = Arc::new(AtomicU64::new(0));
    let dispatcher = Arc::new(ControlStub {
        entered: entered_tx,
        release: Arc::clone(&release),
        served: Arc::clone(&served_count),
    });
    let join = {
        let uds = Arc::clone(&uds);
        thread::spawn(move || {
            uds.run::<SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponseEnvelope, SearchPlaneControlIpcResponse, ControlStub>(
                &dispatcher,
                ACCEPT_IDLE,
            )
        })
    };
    Ok(Harness {
        uds,
        socket,
        join: Some(join),
        entered: entered_rx,
        release,
        served_count,
        _dir: dir,
    })
}

impl Harness {
    fn wait_entered(&self) -> TestResult {
        self.entered.recv_timeout(HANDSHAKE_BOUND)?;
        Ok(())
    }

    fn served(&self) -> u64 {
        self.served_count.load(Ordering::SeqCst)
    }

    fn stop(&mut self) -> TestResult {
        self.uds.shutdown_handle().trigger();
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|panic| format!("server thread panicked: {panic:?}"))??;
        }
        Ok(())
    }
}

fn current_generation(repo: &str) -> SearchPlaneControlIpcRequestEnvelope {
    SearchPlaneControlIpcRequestEnvelope {
        request_id: 7,
        payload: SearchPlaneControlIpcRequest::CurrentGeneration(CurrentGenerationRequest {
            repo_id: RepoId::new(repo),
            revision_id: RevisionId::new("rev"),
            track: SearchPlaneTrackKind::Lexical,
        }),
    }
}

fn send(
    socket: &std::path::Path,
    repo: &str,
    client_budget: Duration,
) -> Result<SearchPlaneControlIpcResponse, IpcError> {
    let policy = ClientIoPolicy::try_new(client_budget)?;
    send_request::<SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope>(
        socket,
        &current_generation(repo),
        policy,
    )
    .map(|envelope| envelope.payload)
}

fn spawn_holder(
    socket: std::path::PathBuf,
) -> thread::JoinHandle<Result<SearchPlaneControlIpcResponse, IpcError>> {
    thread::spawn(move || send(&socket, HOLD_REPO, HANDSHAKE_BOUND))
}

fn join_holder(
    holder: thread::JoinHandle<Result<SearchPlaneControlIpcResponse, IpcError>>,
) -> Result<SearchPlaneControlIpcResponse, Box<dyn Error>> {
    Ok(holder
        .join()
        .map_err(|panic| format!("holder thread panicked: {panic:?}"))??)
}

fn expect_snapshot(response: SearchPlaneControlIpcResponse, repo: &str) -> TestResult {
    match response {
        SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(snapshot) => {
            if snapshot.repo_id.as_str() != repo {
                return Err(format!(
                    "snapshot answered for {} instead of {repo}",
                    snapshot.repo_id.as_str()
                )
                .into());
            }
            Ok(())
        }
        other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
        | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
        | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
        | SearchPlaneControlIpcResponse::Error(_)
        | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
            Err(format!("expected a snapshot for {repo}, got {other:?}").into())
        }
    }
}

fn expect_error(
    response: SearchPlaneControlIpcResponse,
) -> Result<SearchPlaneIpcError, Box<dyn Error>> {
    match response {
        SearchPlaneControlIpcResponse::Error(error) => Ok(error),
        other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
        | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
        | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
        | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
        | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
            Err(format!("expected a typed error, got {other:?}").into())
        }
    }
}

fn ensure(condition: bool, message: impl Into<String>) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

/// (1) A full dispatch queue is refused, typed, after the queue wait.
///
/// One dispatch slot, one holder: the next request is answered
/// `SERVER_OVERLOADED` on its own connection without waiting for the holder,
/// and the message names the slot count. Releasing the holder frees the slot
/// for the next request.
#[test]
fn a_full_dispatch_queue_is_refused_with_a_typed_overload_then_serves_again() -> TestResult {
    let policy = ServerAdmissionPolicy::new(
        8,
        1,
        QUEUE_WAIT,
        Duration::from_secs(20),
        Duration::from_secs(30),
    )?;
    let mut server = start_server(policy)?;
    let holder = spawn_holder(server.socket.clone());
    server.wait_entered()?;

    let started = Instant::now();
    let refusal = expect_error(send(&server.socket, "probe", HANDSHAKE_BOUND)?)?;
    let refused_after = started.elapsed();
    ensure(
        refusal.code == ERR_SERVER_OVERLOADED,
        format!("expected SERVER_OVERLOADED, got {refusal:?}"),
    )?;
    ensure(
        refused_after >= QUEUE_WAIT,
        format!("refused before the queue wait elapsed: {refused_after:?}"),
    )?;
    ensure(
        refused_after < HANDSHAKE_BOUND,
        format!("the refusal waited on the holder instead of the policy: {refused_after:?}"),
    )?;
    ensure(
        refusal.message.contains("(1 slots busy)"),
        format!("message must name the slot count: {}", refusal.message),
    )?;
    ensure(
        server.served() == 0,
        "nothing may be served while the only slot is held",
    )?;

    let _release = server.release.wait();
    expect_snapshot(join_holder(holder)?, HOLD_REPO)?;
    expect_snapshot(send(&server.socket, "after", HANDSHAKE_BOUND)?, "after")?;
    ensure(
        server.served() == 2,
        format!(
            "holder and follow-up must both be served, got {}",
            server.served()
        ),
    )?;
    server.stop()
}

/// (2) `max_connections` is a cap, not a queue.
///
/// With the cap occupied by a held connection, a new connection is closed at
/// accept (the client sees a closed peer, not a slow answer) and counted;
/// once the holder drains, the next connection is admitted.
#[test]
fn connections_past_the_cap_are_closed_at_accept_and_counted() -> TestResult {
    let policy = ServerAdmissionPolicy::new(
        1,
        1,
        Duration::ZERO,
        Duration::from_secs(20),
        Duration::from_secs(30),
    )?;
    let mut server = start_server(policy)?;
    let holder = spawn_holder(server.socket.clone());
    server.wait_entered()?;

    let started = Instant::now();
    let outcome = send(&server.socket, "second", Duration::from_secs(5));
    let closed_after = started.elapsed();
    ensure(
        outcome.is_err(),
        format!("a connection past the cap must not be served: {outcome:?}"),
    )?;
    ensure(
        closed_after < Duration::from_secs(5),
        format!(
            "the refused connection waited on its client budget instead of being closed: {closed_after:?}"
        ),
    )?;
    ensure(
        server.uds.refused_connections() == 1,
        format!(
            "exactly one refusal must be counted, got {}",
            server.uds.refused_connections()
        ),
    )?;

    let _release = server.release.wait();
    expect_snapshot(join_holder(holder)?, HOLD_REPO)?;
    // The holder's connection thread may still be unwinding for an instant
    // after its client saw the response; admission is re-checked per accept,
    // so retry within the handshake bound rather than assume.
    let deadline = Instant::now()
        .checked_add(HANDSHAKE_BOUND)
        .ok_or("clock overflow")?;
    let admitted = loop {
        match send(&server.socket, "third", Duration::from_secs(5)) {
            Ok(response) => break response,
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Err(err) => {
                return Err(format!("never admitted after the holder drained: {err}").into());
            }
        }
    };
    expect_snapshot(admitted, "third")?;
    server.stop()
}

/// (3) The dispatch budget's deadline reaches the dispatcher.
///
/// The budget handed to `dispatch` carries the policy's dispatch budget: a
/// dispatcher that sleeps past it is told so at its next checkpoint, and the
/// client receives that typed answer naming the checkpoint.
#[test]
fn the_dispatch_budget_deadline_reaches_the_dispatcher_checkpoint() -> TestResult {
    let policy = ServerAdmissionPolicy::new(
        8,
        2,
        Duration::ZERO,
        SHORT_DISPATCH_BUDGET,
        Duration::from_secs(30),
    )?;
    let mut server = start_server(policy)?;
    let error = expect_error(send(&server.socket, OVERSLEEP_REPO, HANDSHAKE_BOUND)?)?;
    ensure(
        error.code == REQUEST_DEADLINE_EXCEEDED_CODE,
        format!("expected REQUEST_DEADLINE_EXCEEDED, got {error:?}"),
    )?;
    ensure(
        error.message.contains("checkpoint `after-native`"),
        format!(
            "the interruption must name the checkpoint: {}",
            error.message
        ),
    )?;
    ensure(
        server.served() == 0,
        "an interrupted dispatch must not count as served",
    )?;
    // A request within budget on the same server is unaffected.
    expect_snapshot(send(&server.socket, "quick", HANDSHAKE_BOUND)?, "quick")?;
    server.stop()
}
