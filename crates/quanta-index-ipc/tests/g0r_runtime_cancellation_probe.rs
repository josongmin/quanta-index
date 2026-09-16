//! G0-R — runtime cancellation and scheduling probe (W0 decision gate).
//!
//! The structural plan's W5 work (bounded scheduling, flight-owned
//! cancellation, drain on shutdown) has to start from what the transport
//! actually does today. This target pins that baseline against the real
//! `UdsServer::run` loop rather than from reading it:
//!
//! 1. Does one in-flight dispatch block every other client? (head-of-line)
//! 2. When the requesting peer disconnects, is its dispatch cancelled, or does
//!    the server keep doing the work for nobody?
//! 3. Does shutdown drain the in-flight dispatch, or abandon it?
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
    ClientIoPolicy, IpcDispatcher, IpcError, IpcIoOperation, RequestEnvelope, ResponseEnvelope,
    UdsServer, send_request,
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
}

impl IpcDispatcher<u64, u64> for GatedDispatcher {
    fn dispatch(&self, request: u64) -> u64 {
        if request == HOLD {
            // A probe that cannot observe entry has no ordering evidence; a
            // failed send means the test side already went away.
            if self.entered.send(request).is_err() {
                return u64::MAX;
            }
            let _wait = self.release.wait();
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
    _dir: tempfile::TempDir,
}

fn start_server() -> Result<Server, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("g0r.sock");
    let server = UdsServer::bind(&socket)?;
    let shutdown = server.shutdown_handle();
    let (entered_tx, entered_rx) = mpsc::channel();
    let release = Arc::new(Barrier::new(2));
    let completed = Arc::new(AtomicU64::new(0));
    let sequence = Arc::new(AtomicU64::new(0));
    let completion_sequence = Arc::new(Mutex::new(Vec::new()));
    let dispatcher = Arc::new(GatedDispatcher {
        entered: entered_tx,
        release: Arc::clone(&release),
        completed: Arc::clone(&completed),
        sequence: Arc::clone(&sequence),
        completion_sequence: Arc::clone(&completion_sequence),
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

/// (1) One in-flight dispatch blocks every other client.
///
/// Client A parks inside the dispatcher on a barrier the test holds. While it
/// is held, client B — an independent connection with an independent, trivial
/// request — cannot be served: its bounded read times out. Releasing A lets B
/// through on retry. The block is proven by the barrier, not by timing.
#[test]
fn an_in_flight_dispatch_blocks_every_other_client() -> ProbeResult {
    let mut server = start_server()?;
    let holder = spawn_holder(server.socket.clone(), HANDSHAKE_BOUND);
    let entered = server.wait_entered()?;
    if entered != HOLD {
        return Err(format!("dispatcher entered with unexpected payload {entered}").into());
    }

    // B is a different client with a request the dispatcher would answer
    // instantly. It is refused service only because A occupies the loop.
    let blocked_started = Instant::now();
    let blocked = send(&server.socket, 2, 5, BLOCKED_CLIENT_BUDGET);
    let blocked_for = blocked_started.elapsed();

    let _released = server.release.wait();
    let holder_outcome = holder
        .join()
        .map_err(|panic| format!("holder thread panicked: {panic:?}"))?;
    let served_after_release = send(&server.socket, 3, 5, HANDSHAKE_BOUND);

    evidence(
        "head_of_line",
        &[
            ("blocked_client_outcome", describe(&blocked)),
            (
                "blocked_client_waited_ms",
                blocked_for.as_millis().to_string(),
            ),
            ("holder_outcome", describe(&holder_outcome)),
            ("served_after_release", describe(&served_after_release)),
            (
                "dispatch_completions",
                server.completed.load(Ordering::SeqCst).to_string(),
            ),
        ],
    );
    server.stop()?;

    match blocked {
        Err(IpcError::Timeout {
            operation: IpcIoOperation::Read | IpcIoOperation::Connect,
            ..
        }) => {}
        other => {
            return Err(format!(
                "second client must be starved while the first dispatch is held, got {other:?}"
            )
            .into());
        }
    }
    if holder_outcome?
        != (ProbeResponse {
            request_id: 1,
            payload: HOLD.saturating_add(1),
        })
    {
        return Err("held client must still receive its own response after release".into());
    }
    if served_after_release?
        != (ProbeResponse {
            request_id: 3,
            payload: 6,
        })
    {
        return Err("client must be served once the loop is free".into());
    }
    Ok(())
}

/// (2) A peer that disconnects mid-dispatch does not cancel its work.
///
/// Client A parks inside the dispatcher, then abandons its connection by
/// letting its read budget expire. The dispatcher is still parked, still
/// counted as in flight, and — once released — runs to completion and
/// increments the completion counter for a response nobody will read.
#[test]
fn a_disconnected_peer_does_not_cancel_its_dispatch() -> ProbeResult {
    let mut server = start_server()?;
    let holder = spawn_holder(server.socket.clone(), BLOCKED_CLIENT_BUDGET);
    let _entered = server.wait_entered()?;

    // A gives up: its read budget is shorter than the hold. This is the
    // disconnect. The dispatcher has no way to observe it.
    let abandoned = holder
        .join()
        .map_err(|panic| format!("holder thread panicked: {panic:?}"))?;
    let completions_while_abandoned = server.completed.load(Ordering::SeqCst);

    // Only now does the test release the parked dispatch.
    let _released = server.release.wait();
    // A trivial follow-up request is only answerable after the abandoned
    // dispatch has run to completion, so its success bounds that completion.
    let follow_up = send(&server.socket, 9, 1, HANDSHAKE_BOUND);
    let completions_after_release = server.completed.load(Ordering::SeqCst);

    evidence(
        "disconnect_no_cancel",
        &[
            ("abandoned_client_outcome", describe(&abandoned)),
            (
                "completions_while_abandoned",
                completions_while_abandoned.to_string(),
            ),
            (
                "completions_after_release",
                completions_after_release.to_string(),
            ),
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
    // The abandoned dispatch (1) plus the follow-up (1).
    if completions_after_release != 2 {
        return Err(format!(
            "expected the abandoned dispatch to run to completion for nobody (2 completions), got {completions_after_release}"
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
