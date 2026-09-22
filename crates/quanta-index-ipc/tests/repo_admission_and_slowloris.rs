//! QI-BB-002 closure proofs over the real query envelopes and the real
//! `UdsServer::run` loop:
//!
//! 1. The per-repository in-flight cap: a repository at its cap is refused
//!    `SERVER_OVERLOADED` by name after the queue wait, counted under the
//!    repository-scoped counter, while another repository is served from a
//!    free slot; releasing the holder lets the first repository serve again.
//! 2. Slowloris: one peer sends half a frame and stalls, one peer holds a
//!    dispatch slot, and a third, ordinary query is served while both are
//!    still there. Ordering is proven by completion counts read while the
//!    holder is provably parked and the staller provably connected.
//! 3. A peer that pipelines a second frame and then hangs up still cancels
//!    the dispatch that is running: the peer watch keeps probing after a
//!    pipelined frame instead of giving up.
//!
//! Every ordering claim is enforced with a channel or barrier handshake,
//! and every admission outcome is also read back from the server's
//! counters, the way a metrics scrape reads them (QI-BB-015).

#![forbid(unsafe_code)]

use std::error::Error;
use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ERR_SERVER_OVERLOADED, GenerationPin, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RevisionId, SearchPlaneErrorCodeV2, SearchPlaneIpcError, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::{
    ClientIoPolicy, IpcDispatcher, IpcError, RequestBudgetV1, ServerAdmissionPolicy, UdsServer,
    encode_request, send_request,
};

type TestResult = Result<(), Box<dyn Error>>;

const HANDSHAKE_BOUND: Duration = Duration::from_secs(20);
const ACCEPT_IDLE: Duration = Duration::from_millis(1);
/// Queue wait for the overload proofs: long enough that a refusal is
/// unambiguously the policy firing, short enough to keep the test quick.
const QUEUE_WAIT: Duration = Duration::from_millis(120);
/// The repository whose requests park on the test's barrier.
const HOLD_REPO: &str = "hold";
const SERVED_CODE: SearchPlaneErrorCodeV2 = SearchPlaneErrorCodeV2::Internal;

/// A dispatcher that parks every request for [`HOLD_REPO`] on a barrier
/// until the test releases it and answers every other request at once.
///
/// A held dispatch reports whether its budget was cancelled once released.
struct QueryStub {
    entered: mpsc::Sender<()>,
    release: Arc<Barrier>,
    served: Arc<AtomicU64>,
    cancelled_observed: Arc<AtomicU64>,
}

impl IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse> for QueryStub {
    fn dispatch(
        &self,
        _context: &quanta_index_ipc::DispatchContextV1,
        request: SearchPlaneQueryIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let SearchPlaneQueryIpcRequest::Text(text) = request else {
            return error_response(SearchPlaneErrorCodeV2::Internal, "only Text is stubbed");
        };
        let repo = text
            .generation
            .as_ref()
            .map_or("unpinned", |pin| pin.repo_id.as_str())
            .to_string();
        if repo == HOLD_REPO {
            if self.entered.send(()).is_err() {
                return error_response(SearchPlaneErrorCodeV2::Internal, "test side went away");
            }
            let _wait = self.release.wait();
            if budget.is_cancelled() {
                let _observed = self.cancelled_observed.fetch_add(1, Ordering::SeqCst);
            }
        }
        let _served = self.served.fetch_add(1, Ordering::SeqCst);
        error_response(SERVED_CODE, repo)
    }
}

fn error_response(
    code: SearchPlaneErrorCodeV2,
    message: impl Into<String>,
) -> SearchPlaneQueryIpcResponse {
    SearchPlaneQueryIpcResponse::Error(SearchPlaneIpcError {
        code,
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
    cancelled_observed: Arc<AtomicU64>,
    _dir: tempfile::TempDir,
}

fn start_server(policy: ServerAdmissionPolicy) -> Result<Harness, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    // The server refuses a socket directory wider than 0700 (QI-BB-014); a
    // tempdir is created under the umask, so it is narrowed here.
    std::fs::set_permissions(
        dir.path(),
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )?;
    let socket = dir.path().join("query.sock");
    let uds = Arc::new(UdsServer::bind_with_policy(&socket, policy)?);
    let (entered_tx, entered_rx) = mpsc::channel();
    let release = Arc::new(Barrier::new(2));
    let served_count = Arc::new(AtomicU64::new(0));
    let cancelled_observed = Arc::new(AtomicU64::new(0));
    let dispatcher = Arc::new(QueryStub {
        entered: entered_tx,
        release: Arc::clone(&release),
        served: Arc::clone(&served_count),
        cancelled_observed: Arc::clone(&cancelled_observed),
    });
    let join = {
        let uds = Arc::clone(&uds);
        thread::spawn(move || {
            uds.run::<SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponseEnvelope, SearchPlaneQueryIpcResponse, QueryStub>(
                &dispatcher,
                quanta_index_ipc::IpcPlane::Query,
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
        cancelled_observed,
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

#[expect(
    clippy::expect_used,
    reason = "test fixture IDs provably satisfy the canonical ID policy"
)]
fn text_query(request_id: u64, repo: &str) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(
                RepoId::new(repo).expect("test fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(1),
            )),
            generation_selector: None,
            top_k: 10,
            cursor: None,
        }),
    }
}

fn send(
    socket: &std::path::Path,
    repo: &str,
    client_budget: Duration,
) -> Result<SearchPlaneIpcError, IpcError> {
    let policy = ClientIoPolicy::try_new(client_budget)?;
    let envelope = send_request::<
        SearchPlaneQueryIpcRequestEnvelope,
        SearchPlaneQueryIpcResponseEnvelope,
    >(socket, &text_query(1, repo), policy)?;
    match envelope.payload {
        SearchPlaneQueryIpcResponse::Error(error) => Ok(error),
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)) => Err(IpcError::Decode(format!(
            "the stub answers with a typed marker only, got {other:?}"
        ))),
    }
}

fn spawn_holder(
    socket: std::path::PathBuf,
) -> thread::JoinHandle<Result<SearchPlaneIpcError, IpcError>> {
    thread::spawn(move || send(&socket, HOLD_REPO, HANDSHAKE_BOUND))
}

fn join_holder(
    holder: thread::JoinHandle<Result<SearchPlaneIpcError, IpcError>>,
) -> Result<SearchPlaneIpcError, Box<dyn Error>> {
    Ok(holder
        .join()
        .map_err(|panic| format!("holder thread panicked: {panic:?}"))??)
}

fn ensure(condition: bool, message: impl Into<String>) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

fn expect_served(error: &SearchPlaneIpcError, repo: &str) -> TestResult {
    ensure(
        error.code == SERVED_CODE && error.message == repo,
        format!("expected the served marker for {repo}, got {error:?}"),
    )
}

fn policy(
    slots: usize,
    per_repo: usize,
    queue_wait: Duration,
) -> Result<ServerAdmissionPolicy, IpcError> {
    ServerAdmissionPolicy::new(
        8,
        slots,
        per_repo,
        queue_wait,
        Duration::from_secs(20),
        Duration::from_secs(30),
    )
}

/// (1) A repository at its in-flight cap is refused by name while another
/// repository is served.
#[test]
fn a_repository_at_its_in_flight_cap_is_refused_by_name_while_another_is_served() -> TestResult {
    let mut server = start_server(policy(3, 1, QUEUE_WAIT)?)?;
    let holder = spawn_holder(server.socket.clone());
    server.wait_entered()?;

    // Two slots are free, but `hold` already holds its one.
    let started = Instant::now();
    let refusal = send(&server.socket, HOLD_REPO, HANDSHAKE_BOUND)?;
    let refused_after = started.elapsed();
    ensure(
        refusal.code == ERR_SERVER_OVERLOADED,
        format!("expected SERVER_OVERLOADED, got {refusal:?}"),
    )?;
    ensure(
        refusal.message.contains("repository `hold`") && refusal.message.contains("1 in-flight"),
        format!(
            "the refusal names the repository and its cap: {}",
            refusal.message
        ),
    )?;
    ensure(
        refused_after >= QUEUE_WAIT && refused_after < HANDSHAKE_BOUND,
        format!("the refusal waited exactly the policy's queue wait: {refused_after:?}"),
    )?;
    ensure(
        server.served() == 0,
        "nothing was served while the holder was parked",
    )?;
    // Another repository takes one of the two free slots at once.
    expect_served(&send(&server.socket, "other", HANDSHAKE_BOUND)?, "other")?;
    ensure(server.served() == 1, "the other repository was served")?;
    let counters = server.uds.counters().snapshot();
    ensure(
        counters.requests_overloaded_repo == 1 && counters.requests_overloaded == 0,
        format!("the refusal is counted under the repository-scoped counter: {counters:?}"),
    )?;
    ensure(
        counters.dispatch_in_flight == 1,
        format!("only the holder's slot is in flight: {counters:?}"),
    )?;
    ensure(
        counters.dispatch_queue_waits == 0,
        format!("a refused request is not an admitted wait: {counters:?}"),
    )?;

    let _release = server.release.wait();
    expect_served(&join_holder(holder)?, HOLD_REPO)?;
    // The repository's hold is released with its slot: the next `hold`
    // request is admitted (it parks on the barrier again) and served.
    let again = spawn_holder(server.socket.clone());
    server.wait_entered()?;
    let _release = server.release.wait();
    expect_served(&join_holder(again)?, HOLD_REPO)?;
    let counters = server.uds.counters().snapshot();
    ensure(
        counters.requests_dispatched == 3 && counters.dispatch_in_flight == 0,
        format!("three dispatches, none in flight after they answered: {counters:?}"),
    )?;
    ensure(
        counters.response_bytes > 0,
        format!("every answer's frame bytes are counted: {counters:?}"),
    )?;
    server.stop()
}

/// (2) Slowloris: a stalled half frame, a held slot and a normal query.
///
/// Peer A writes the length prefix and a few body bytes of a frame and
/// stalls. Peer B parks inside the dispatcher on the test's barrier. Peer
/// C sends an ordinary query and is served while A is still connected and
/// B still parked, which the served count and the live-connection count
/// prove at the moment C's answer arrives.
#[test]
fn a_stalled_half_frame_a_held_slot_and_a_normal_query_do_not_block_each_other() -> TestResult {
    let mut server = start_server(policy(2, 2, QUEUE_WAIT)?)?;

    // Peer A: half a frame, then silence.
    let frame = encode_request(&text_query(1, "stalled"))?;
    // The 4-byte length prefix and a few body bytes: a frame the server
    // has started reading and cannot finish.
    let (prefix, _rest) = frame.split_at(frame.len().min(8));
    let mut staller = UnixStream::connect(&server.socket)?;
    staller.write_all(prefix)?;
    staller.flush()?;

    // Peer B: parked in the dispatcher, holding one of the two slots.
    let holder = spawn_holder(server.socket.clone());
    server.wait_entered()?;

    // Peer C: served on the remaining slot while A stalls and B is parked.
    let served_started = Instant::now();
    let answer = send(&server.socket, "normal", HANDSHAKE_BOUND)?;
    let served_in = served_started.elapsed();
    expect_served(&answer, "normal")?;
    ensure(
        server.served() == 1,
        format!(
            "exactly the normal query completed while B was held: {}",
            server.served()
        ),
    )?;
    ensure(
        served_in < QUEUE_WAIT,
        format!("the normal query never waited for a slot: {served_in:?}"),
    )?;
    let counters = server.uds.counters().snapshot();
    // A and B are still connected (C's own connection may not have
    // unwound yet on the server side).
    ensure(
        counters.connections_live >= 2 && counters.connections_accepted == 3,
        format!("A and B are still connected when C is answered: {counters:?}"),
    )?;
    ensure(
        counters.dispatch_in_flight == 1 && counters.dispatch_queue_waits == 0,
        format!("B holds one slot and nobody waited: {counters:?}"),
    )?;
    ensure(
        counters.requests_overloaded == 0 && counters.requests_overloaded_repo == 0,
        format!("nothing was refused: {counters:?}"),
    )?;

    // The staller is still stalled: it has produced neither a decode
    // failure nor a dispatch.
    ensure(
        counters.request_decode_failures == 0 && counters.requests_dispatched == 1,
        format!("A produced nothing yet: {counters:?}"),
    )?;

    let _release = server.release.wait();
    expect_served(&join_holder(holder)?, HOLD_REPO)?;
    drop(staller);
    server.stop()?;
    let counters = server.uds.counters().snapshot();
    ensure(
        counters.requests_dispatched == 2 && counters.connections_live == 0,
        format!("B and C dispatched, every connection gone after shutdown: {counters:?}"),
    )
}

/// (3) A peer that pipelines a second frame and then hangs up still
/// cancels the running dispatch.
///
/// The client writes a `hold` request and, without waiting for its
/// answer, a second request. Once the dispatcher is parked on the first,
/// the client hangs up. The watch saw a readable socket (the pipelined
/// frame) before the hang-up; it must keep probing rather than stop, so
/// the parked dispatch observes the cancellation when it is released.
#[test]
fn a_peer_that_pipelines_then_hangs_up_still_cancels_the_running_dispatch() -> TestResult {
    let mut server = start_server(policy(2, 2, QUEUE_WAIT)?)?;
    let mut client = UnixStream::connect(&server.socket)?;
    client.write_all(&encode_request(&text_query(1, HOLD_REPO))?)?;
    client.write_all(&encode_request(&text_query(2, "pipelined"))?)?;
    client.flush()?;
    server.wait_entered()?;
    // Give the watch its first poll (it sees the pipelined frame now),
    // then hang up and give it a second interval to see that through the
    // probe. The assertion below is a count the watch either produced or
    // did not; the sleeps only bound how long it had.
    thread::sleep(Duration::from_millis(120));
    drop(client);
    thread::sleep(Duration::from_millis(200));
    let _release = server.release.wait();
    // The held connection thread counts the hang-up after its dispatch
    // returns; wait for that count rather than assume the interleaving.
    let deadline = Instant::now()
        .checked_add(HANDSHAKE_BOUND)
        .ok_or("clock overflow")?;
    while server.uds.counters().snapshot().requests_dispatched == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let observed = server.cancelled_observed.load(Ordering::SeqCst);
    ensure(
        observed == 1,
        format!(
            "the released dispatch must observe its peer's hang-up on the budget after a pipelined frame, observed {observed}"
        ),
    )?;
    let counters = server.uds.counters().snapshot();
    ensure(
        counters.peer_hangups == 1 && counters.requests_dispatched == 1,
        format!("the hang-up is counted once: {counters:?}"),
    )?;
    // The server still serves after the abandoned connection.
    expect_served(
        &send(&server.socket, "follow-up", HANDSHAKE_BOUND)?,
        "follow-up",
    )?;
    server.stop()
}
