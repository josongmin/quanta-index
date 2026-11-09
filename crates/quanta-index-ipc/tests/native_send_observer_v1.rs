//! End-to-end proof that the process-scoped observer counts real UDS writes.
//!
//! One test in its own integration-test process keeps the global counter
//! independent of parallel unit tests under both cargo test and nextest.

#![cfg(feature = "native-send-observer-v1")]
#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_ipc::native_send_observer_v1::{abandon_v1, disarm_v1, snapshot_v1, try_arm_v1};
use quanta_index_ipc::{
    ClientIoPolicy, RequestBudgetV1, decode_request, encode_response, send_request,
    send_request_negotiated, send_request_with_budget, send_stream_request,
};

type TestResult = Result<(), String>;

struct ArmedObserver;

impl ArmedObserver {
    fn new() -> Self {
        assert!(
            try_arm_v1(),
            "observer already armed in isolated test process"
        );
        Self
    }
}

impl Drop for ArmedObserver {
    fn drop(&mut self) {
        abandon_v1();
    }
}

fn accept_bounded(listener: &UnixListener) -> Result<UnixStream, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|error| error.to_string())?;
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .map_err(|error| error.to_string())?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .map_err(|error| error.to_string())?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("expected UDS connection did not arrive".into());
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn serve_request(listener: &UnixListener, expected: u64, response: u64) -> TestResult {
    let mut stream = accept_bounded(listener)?;
    let request: u64 = decode_request(&mut stream).map_err(|error| error.to_string())?;
    if request != expected {
        return Err(format!("expected request {expected}, got {request}"));
    }
    stream
        .write_all(&encode_response(&response).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn serve_observed_writes(listener: UnixListener) -> TestResult {
    serve_request(&listener, 11, 12)?;
    serve_request(&listener, 21, 22)?;

    // An older peer closes a diagnostic-control-only connection. The client
    // must count the preface and then send its plain request on a new stream.
    let mut probe = accept_bounded(&listener)?;
    let mut preface = [0_u8; 24];
    probe
        .read_exact(&mut preface)
        .map_err(|error| error.to_string())?;
    let (magic, capture) = preface.split_at(8);
    if magic != b"QIPCDG01" || capture != [7_u8; 16] {
        return Err(format!("unexpected diagnostic preface: {preface:?}"));
    }
    drop(probe);
    serve_request(&listener, 31, 32)
}

fn exercise_client(socket: &Path) -> TestResult {
    let _armed = ArmedObserver::new();
    let cancelled = RequestBudgetV1::for_duration(Duration::from_secs(5));
    cancelled.cancel_handle().cancel();
    if send_request_with_budget::<_, u64>(socket, &1_u64, &cancelled).is_ok()
        || send_stream_request(socket, &1_u64, &cancelled).is_ok()
    {
        return Err("cancelled request was accepted before send".into());
    }
    if snapshot_v1() != 0 {
        return Err("pre-send refusal counted a write".into());
    }

    let policy =
        ClientIoPolicy::try_new(Duration::from_secs(5)).map_err(|error| error.to_string())?;
    let response: u64 = send_request(socket, &11_u64, policy).map_err(|error| error.to_string())?;
    if response != 12 || snapshot_v1() != 1 {
        return Err(format!(
            "one-shot response/count: {response}/{}",
            snapshot_v1()
        ));
    }

    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let mut stream =
        send_stream_request(socket, &21_u64, &budget).map_err(|error| error.to_string())?;
    let response: u64 = stream.next_response().map_err(|error| error.to_string())?;
    if response != 22 || snapshot_v1() != 2 {
        return Err(format!(
            "stream response/count: {response}/{}",
            snapshot_v1()
        ));
    }
    drop(stream);

    let response: u64 = send_request_negotiated(socket, &31_u64, &99_u64, [7; 16], policy)
        .map_err(|error| error.to_string())?;
    if response != 32 || snapshot_v1() != 4 {
        return Err(format!(
            "diagnostic response/count: {response}/{}",
            snapshot_v1()
        ));
    }
    if disarm_v1() != 4 {
        return Err("final send count changed during disarm".into());
    }
    Ok(())
}

#[test]
fn real_uds_send_attempts_cover_all_hooks_and_presend_refusal() -> TestResult {
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let socket = dir.path().join("native-send-observer.sock");
    let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let server = thread::spawn(move || serve_observed_writes(listener));
    let client_result = exercise_client(&socket);
    let server_result = server
        .join()
        .map_err(|_| "UDS test server panicked".to_string())?;
    client_result?;
    server_result
}
