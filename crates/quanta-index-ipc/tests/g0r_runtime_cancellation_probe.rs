//! G0-R — runtime cancellation and scheduling probe, now the W5 proof.
//!
//! As a W0 gate this target pinned the transport's baseline against the real
//! `UdsServer::run` loop: one in-flight dispatch blocked every other client,
//! a disconnected peer's dispatch ran to completion for nobody, and shutdown
//! drained the in-flight dispatch. W5 (QI-BB-002) changed the first two, and
//! the same probe now proves the new contract against the same loop:
//!
//! 1. An in-flight dispatch does not block another client. (head-of-line)
//! 2. When the requesting peer disconnects, the dispatch's budget is
//!    cancelled, and a dispatcher that checkpoints observes it.
//! 3. Shutdown still drains the in-flight dispatch rather than abandoning it.
//!
//! Every ordering claim is enforced with a barrier or channel handshake. No
//! test here sleeps and then assumes an interleaving happened.
//!
//! What this probe cannot measure is recorded in the G0-R ADR instead: the
//! frame decoder's allocation peak (`decode_frame` allocates the declared body
//! length before reading any body byte) needs a counting allocator, which this
//! workspace's `#![forbid(unsafe_code)]` rules out.
//!
//! Emits `G0R-EVIDENCE` lines; run with `-- --nocapture` for the gate ADR.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_ipc::{
    ClientIoPolicy, IpcDispatcher, IpcError, RequestBudgetV1, RequestEnvelope, ResponseEnvelope,
    SlotRefusal, UdsServer, send_request,
};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

type ProbeResult = Result<(), Box<dyn Error>>;

/// Generous bound for a handshake that must complete; a hang here is a probe
/// defect, not evidence.
const HANDSHAKE_BOUND: Duration = Duration::from_secs(20);
/// Short client budget used only where the probe asserts a client *cannot*
/// be served because the server is provably occupied.
const BLOCKED_CLIENT_BUDGET: Duration = Duration::from_millis(150);
const ACCEPT_IDLE: Duration = Duration::from_millis(1);

// ---------------------------------------------------------------------------
// Minimal wire types (manual serde: the workspace bans serde derives).
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct ProbeRequest {
    request_id: u64,
    payload: u64,
}

impl Serialize for ProbeRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("ProbeRequest", 2)?;
        state.serialize_field("request_id", &self.request_id)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
    }
}

struct ProbeRequestVisitor;

impl<'de> Visitor<'de> for ProbeRequestVisitor {
    type Value = ProbeRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ProbeRequest map")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut request_id = None;
        let mut payload = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "request_id" => request_id = Some(map.next_value()?),
                "payload" => payload = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, &["request_id", "payload"])),
            }
        }
        Ok(ProbeRequest {
            request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
            payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
        })
    }
}

impl<'de> Deserialize<'de> for ProbeRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "ProbeRequest",
            &["request_id", "payload"],
            ProbeRequestVisitor,
        )
    }
}

impl RequestEnvelope<u64> for ProbeRequest {
    fn into_parts(self) -> (u64, u64) {
        (self.request_id, self.payload)
    }

    fn repo_scope(_request: &u64) -> Option<String> {
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ProbeResponse {
    request_id: u64,
    payload: u64,
}

impl Serialize for ProbeResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("ProbeResponse", 2)?;
        state.serialize_field("request_id", &self.request_id)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
    }
}

struct ProbeResponseVisitor;

impl<'de> Visitor<'de> for ProbeResponseVisitor {
    type Value = ProbeResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ProbeResponse map")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut request_id = None;
        let mut payload = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "request_id" => request_id = Some(map.next_value()?),
                "payload" => payload = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, &["request_id", "payload"])),
            }
        }
        Ok(ProbeResponse {
            request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
            payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
        })
    }
}

impl<'de> Deserialize<'de> for ProbeResponse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "ProbeResponse",
            &["request_id", "payload"],
            ProbeResponseVisitor,
        )
    }
}

impl ResponseEnvelope<u64> for ProbeResponse {
    fn from_parts(request_id: u64, payload: u64) -> Self {
        Self {
            request_id,
            payload,
        }
    }

    fn result_too_large(_request_id: u64, _encoded_bytes: u64, _limit_bytes: u64) -> Option<Self> {
        None
    }

    fn overloaded(_request_id: u64, _refusal: &SlotRefusal) -> Option<Self> {
        None
    }
}

// ---------------------------------------------------------------------------
// Controllable dispatcher: a request whose payload is `HOLD` parks on a barrier
// until the test releases it; every other request answers immediately.
// ---------------------------------------------------------------------------

const HOLD: u64 = 1_000;

struct GatedDispatcher {
    entered: mpsc::Sender<u64>,
    release: Arc<Barrier>,
    completed: Arc<AtomicU64>,
    /// Monotonic sequence stamped at completion, so a test can prove ordering
    /// against other events without a clock.
    sequence: Arc<AtomicU64>,
    completion_sequence: Arc<Mutex<Vec<u64>>>,
    /// Held dispatches whose budget reported cancellation once released.
    cancelled_observed: Arc<AtomicU64>,
}

impl IpcDispatcher<u64, u64> for GatedDispatcher {
    fn dispatch(&self, request: u64, budget: &RequestBudgetV1) -> u64 {
        if request == HOLD {
            // A probe that cannot observe entry has no ordering evidence; a
            // failed send means the test side already went away.
            if self.entered.send(request).is_err() {
                return u64::MAX;
            }
            let _wait = self.release.wait();
            // The checkpoint a real route would place after its native work:
            // it is the only place cooperative cancellation can be seen.
            if budget.is_cancelled() {
                let _observed = self.cancelled_observed.fetch_add(1, Ordering::SeqCst);
            }
        }
        let stamp = self.sequence.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut log) = self.completion_sequence.lock() {
            log.push(stamp);
        }
        let _completed = self.completed.fetch_add(1, Ordering::SeqCst);
        request.saturating_add(1)
    }
}

struct Server {
    socket: std::path::PathBuf,
    shutdown: quanta_index_ipc::ShutdownHandle,
    join: Option<thread::JoinHandle<Result<(), IpcError>>>,
    entered: mpsc::Receiver<u64>,
    release: Arc<Barrier>,
    completed: Arc<AtomicU64>,
    sequence: Arc<AtomicU64>,
    completion_sequence: Arc<Mutex<Vec<u64>>>,
    cancelled_observed: Arc<AtomicU64>,
    _dir: tempfile::TempDir,
}

fn start_server() -> Result<Server, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    // The server refuses a socket directory wider than 0700 (QI-BB-014); a
    // tempdir is created under the umask, so it is narrowed here.
    std::fs::set_permissions(
        dir.path(),
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )?;
    let socket = dir.path().join("g0r.sock");
    let server = UdsServer::bind(&socket)?;
    let shutdown = server.shutdown_handle();
    let (entered_tx, entered_rx) = mpsc::channel();
    let release = Arc::new(Barrier::new(2));
    let completed = Arc::new(AtomicU64::new(0));
    let sequence = Arc::new(AtomicU64::new(0));
    let completion_sequence = Arc::new(Mutex::new(Vec::new()));
    let cancelled_observed = Arc::new(AtomicU64::new(0));
    let dispatcher = Arc::new(GatedDispatcher {
        entered: entered_tx,
        release: Arc::clone(&release),
        completed: Arc::clone(&completed),
        sequence: Arc::clone(&sequence),
        completion_sequence: Arc::clone(&completion_sequence),
        cancelled_observed: Arc::clone(&cancelled_observed),
    });
    let join = thread::spawn(move || {
        server
            .run::<ProbeRequest, u64, ProbeResponse, u64, GatedDispatcher>(&dispatcher, ACCEPT_IDLE)
    });
    Ok(Server {
        socket,
        shutdown,
        join: Some(join),
        entered: entered_rx,
        release,
        completed,
        sequence,
        completion_sequence,
        cancelled_observed,
        _dir: dir,
    })
}

impl Server {
    fn wait_entered(&self) -> Result<u64, Box<dyn Error>> {
        Ok(self.entered.recv_timeout(HANDSHAKE_BOUND)?)
    }

    fn stop(&mut self) -> Result<(), Box<dyn Error>> {
        self.shutdown.trigger();
        if let Some(join) = self.join.take() {
            join.join()
                .map_err(|panic| format!("server thread panicked: {panic:?}"))??;
        }
        Ok(())
    }
}

fn send(
    socket: &std::path::Path,
    request_id: u64,
    payload: u64,
    budget: Duration,
) -> Result<ProbeResponse, IpcError> {
    let policy = ClientIoPolicy::try_new(budget)?;
    send_request::<ProbeRequest, ProbeResponse>(
        socket,
        &ProbeRequest {
            request_id,
            payload,
        },
        policy,
    )
}

/// Spawn a client that sends a `HOLD` request and reports how it ended.
fn spawn_holder(
    socket: std::path::PathBuf,
    budget: Duration,
) -> thread::JoinHandle<Result<ProbeResponse, IpcError>> {
    thread::spawn(move || send(&socket, 1, HOLD, budget))
}

#[expect(
    clippy::print_stdout,
    reason = "the probe's whole purpose is to emit machine-readable gate evidence for the G0-R ADR"
)]
fn evidence(label: &str, fields: &[(&str, String)]) {
    let rendered: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    println!("G0R-EVIDENCE {label} {}", rendered.join(" "));
}

/// (1) One in-flight dispatch does not block another client.
///
/// Client A parks inside the dispatcher on a barrier the test holds. While it
/// is held, client B — an independent connection with an independent, trivial
/// request — is served on its own connection thread and its own dispatch
/// slot. Releasing A then delivers A's own response. Under the W0 baseline B
/// timed out here; that is the head-of-line block W5 removed.
#[test]
fn an_in_flight_dispatch_does_not_block_other_clients() -> ProbeResult {
    let mut server = start_server()?;
    let holder = spawn_holder(server.socket.clone(), HANDSHAKE_BOUND);
    let entered = server.wait_entered()?;
    if entered != HOLD {
        return Err(format!("dispatcher entered with unexpected payload {entered}").into());
    }

    let served_started = Instant::now();
    let served_while_held = send(&server.socket, 2, 5, HANDSHAKE_BOUND);
    let served_in = served_started.elapsed();
    let completions_while_held = server.completed.load(Ordering::SeqCst);

    let _released = server.release.wait();
    let holder_outcome = holder
        .join()
        .map_err(|panic| format!("holder thread panicked: {panic:?}"))?;

    evidence(
        "head_of_line",
        &[
            ("served_while_held", describe(&served_while_held)),
            ("served_in_ms", served_in.as_millis().to_string()),
            ("completions_while_held", completions_while_held.to_string()),
            ("holder_outcome", describe(&holder_outcome)),
        ],
    );
    server.stop()?;

    if served_while_held?
        != (ProbeResponse {
            request_id: 2,
            payload: 6,
        })
    {
        return Err("second client must be served while the first dispatch is held".into());
    }
    if completions_while_held != 1 {
        return Err(format!(
            "exactly the second client's dispatch must have completed while the first was held, got {completions_while_held}"
        )
        .into());
    }
    if holder_outcome?
        != (ProbeResponse {
            request_id: 1,
            payload: HOLD.saturating_add(1),
        })
    {
        return Err("held client must still receive its own response after release".into());
    }
    Ok(())
}

/// (2) A peer that disconnects mid-dispatch cancels its budget.
///
/// Client A parks inside the dispatcher, then abandons its connection by
/// letting its read budget expire. The connection's peer watch sees the
/// hang-up and cancels the request's budget; once the test releases the
/// parked dispatch, the dispatcher's checkpoint observes the cancellation.
/// The native work between checkpoints still runs — G0-R pinned that hard
/// cancellation is not available — but nothing past the checkpoint does, and
/// the server can name where it stopped.
#[test]
fn a_disconnected_peer_cancels_its_dispatch_budget() -> ProbeResult {
    let mut server = start_server()?;
    let holder = spawn_holder(server.socket.clone(), BLOCKED_CLIENT_BUDGET);
    let _entered = server.wait_entered()?;

    // A gives up: its read budget is shorter than the hold. This is the
    // disconnect the peer watch must notice.
    let abandoned = holder
        .join()
        .map_err(|panic| format!("holder thread panicked: {panic:?}"))?;
    // Give the watch its poll interval to see the hang-up before release.
    thread::sleep(Duration::from_millis(200));
    let completions_while_abandoned = server.completed.load(Ordering::SeqCst);

    let _released = server.release.wait();
    let follow_up = send(&server.socket, 9, 1, HANDSHAKE_BOUND);
    let cancelled_observed = server.cancelled_observed.load(Ordering::SeqCst);

    evidence(
        "disconnect_cancels",
        &[
            ("abandoned_client_outcome", describe(&abandoned)),
            ("completions_while_abandoned", completions_while_abandoned.to_string()),
            ("cancelled_observed", cancelled_observed.to_string()),
            ("follow_up_outcome", describe(&follow_up)),
        ],
    );
    server.stop()?;

    if !matches!(abandoned, Err(IpcError::Timeout { .. })) {
        return Err(format!("client must have abandoned the request, got {abandoned:?}").into());
    }
    if completions_while_abandoned != 0 {
        return Err("dispatch completed before the test released it".into());
    }
    if cancelled_observed != 1 {
        return Err(format!(
            "the released dispatch must observe its peer's hang-up on the budget, observed {cancelled_observed}"
        )
        .into());
    }
    let _follow_up = follow_up?;
    Ok(())
}

/// (3) Shutdown drains the in-flight dispatch rather than abandoning it.
///
/// With client A parked in the dispatcher, the test triggers shutdown, then
/// releases A. The accept loop must return only after A's dispatch completed,
/// and A must receive its response. Ordering is proven by a shared sequence
/// counter stamped at dispatch completion and at loop exit.
#[test]
fn shutdown_drains_the_in_flight_dispatch() -> ProbeResult {
    let mut server = start_server()?;
    let holder = spawn_holder(server.socket.clone(), HANDSHAKE_BOUND);
    let _entered = server.wait_entered()?;

    server.shutdown.trigger();
    let (exited_tx, exited_rx) = mpsc::channel();
    let join = server
        .join
        .take()
        .ok_or("server join handle already consumed")?;
    let sequence = Arc::clone(&server.sequence);
    let waiter = thread::spawn(move || {
        let outcome = join.join();
        let exit_stamp = sequence.fetch_add(1, Ordering::SeqCst);
        let _sent = exited_tx.send(exit_stamp);
        outcome
    });

    let _released = server.release.wait();
    let holder_outcome = holder
        .join()
        .map_err(|panic| format!("holder thread panicked: {panic:?}"))?;
    let exit_stamp = exited_rx.recv_timeout(HANDSHAKE_BOUND)?;
    let run_outcome = waiter
        .join()
        .map_err(|panic| format!("waiter thread panicked: {panic:?}"))?
        .map_err(|panic| format!("server thread panicked: {panic:?}"))?;
    let completion_stamps = server
        .completion_sequence
        .lock()
        .map_err(|err| format!("completion log poisoned: {err}"))?
        .clone();

    evidence(
        "shutdown_drain",
        &[
            ("holder_outcome", describe(&holder_outcome)),
            ("completion_stamps", format!("{completion_stamps:?}")),
            ("loop_exit_stamp", exit_stamp.to_string()),
            ("run_outcome", format!("{run_outcome:?}")),
        ],
    );

    run_outcome?;
    if holder_outcome?
        != (ProbeResponse {
            request_id: 1,
            payload: HOLD.saturating_add(1),
        })
    {
        return Err("in-flight client must receive its response across shutdown".into());
    }
    let Some(completion) = completion_stamps.first() else {
        return Err("no dispatch completed".into());
    };
    if *completion >= exit_stamp {
        return Err(format!(
            "accept loop exited (stamp {exit_stamp}) before the in-flight dispatch completed (stamp {completion})"
        )
        .into());
    }
    Ok(())
}

fn describe<T: fmt::Debug>(outcome: &Result<T, IpcError>) -> String {
    match outcome {
        Ok(value) => format!("ok:{value:?}").replace(' ', "_"),
        Err(err) => format!("err:{err:?}").replace(' ', "_"),
    }
}
