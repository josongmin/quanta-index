//! `AF_UNIX` stream server + client.
//!
//! The server accepts connections sequentially (phase-1 rule: at-most-one
//! in-flight request per connection), decodes a single envelope per frame,
//! routes through the [`IpcDispatcher`] supplied by the composition root,
//! and writes the response back on the same connection.
//!
//! Connection-fatal failures (framing, oversized, CBOR decode) close the
//! connection without writing a response. Request-domain failures (e.g.
//! `NOT_READY`, `INVALID_REQUEST`) flow through as an `Error` variant in the
//! response envelope.

use std::io::{ErrorKind, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use quanta_index_contract::{
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponseEnvelope,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponseEnvelope,
};

use crate::codec::{IpcError, decode_request, decode_response, encode_request, encode_response};

/// Dispatch hook supplied by the composition root.
///
/// Receives a fully-parsed request payload and returns a fully-typed response
/// payload. Domain errors MUST be surfaced through the response type rather
/// than panicking.
pub trait IpcDispatcher<Request, Response>: Send + Sync {
    fn dispatch(&self, request: Request) -> Response;
}

pub trait RequestEnvelope<Request>: serde::de::DeserializeOwned + Send + Sync + 'static {
    fn into_parts(self) -> (u64, Request);
}

pub trait ResponseEnvelope<Response>: serde::Serialize + Send + Sync + 'static {
    fn from_parts(request_id: u64, payload: Response) -> Self;
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
}

/// Synchronous `AF_UNIX` stream server.
pub struct UdsServer {
    listener: UnixListener,
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
}

impl UdsServer {
    /// Bind a new listener at `path`. Removes any pre-existing socket file
    /// at that path (only socket files — never a regular file).
    pub fn bind(path: &Path) -> Result<Self, IpcError> {
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
        listener.set_nonblocking(true).map_err(IpcError::Io)?;
        Ok(Self {
            listener,
            socket_path: path.to_path_buf(),
            shutdown: Arc::new(AtomicBool::new(false)),
        })
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

    /// Run the accept loop until `shutdown` is triggered. Each connection is
    /// handled inline (phase-1: single-flight per connection); the listener is
    /// non-blocking, so an empty accept queue sleeps `accept_idle` before
    /// retrying.
    pub fn run<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
        &self,
        dispatcher: &Arc<D>,
        accept_idle: Duration,
    ) -> Result<(), IpcError>
    where
        RequestEnvelopeT: RequestEnvelope<Request>,
        ResponseEnvelopeT: ResponseEnvelope<Response>,
        D: IpcDispatcher<Request, Response> + ?Sized,
    {
        while !self.shutdown.load(Ordering::Acquire) {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    let dispatcher = Arc::clone(dispatcher);
                    let reason = handle_connection::<
                        RequestEnvelopeT,
                        Request,
                        ResponseEnvelopeT,
                        Response,
                        D,
                    >(stream, dispatcher.as_ref());
                    report_connection_close(&reason);
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(accept_idle);
                }
                Err(err) => return Err(IpcError::Io(err)),
            }
        }
        drop(std::fs::remove_file(&self.socket_path));
        Ok(())
    }
}

impl Drop for UdsServer {
    fn drop(&mut self) {
        drop(std::fs::remove_file(&self.socket_path));
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

/// Per-connection read/write timeout.
///
/// Prevents slow-loris `DoS` where a peer opens a connection, writes a length
/// prefix, and never sends a body — the single-flight accept loop would
/// otherwise stall every other client.
const CONNECTION_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
enum ConnectionCloseReason {
    BlockingModeConfigFailed(String),
    TimeoutConfigFailed(String),
    PeerClosed,
    RequestDecodeFailed(IpcError),
    ResponseEncodeFailed(IpcError),
    ResponseWriteFailed(String),
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
            Self::RequestDecodeFailed(err) => write!(f, "request decode failed: {err}"),
            Self::ResponseEncodeFailed(err) => write!(f, "response encode failed: {err}"),
            Self::ResponseWriteFailed(message) => write!(f, "response write failed: {message}"),
        }
    }
}

fn report_connection_close(reason: &ConnectionCloseReason) {
    if matches!(reason, ConnectionCloseReason::PeerClosed) {
        return;
    }
    eprintln!("quanta-index-ipc: closed connection: {reason}");
}

fn handle_connection<RequestEnvelopeT, Request, ResponseEnvelopeT, Response, D>(
    mut stream: UnixStream,
    dispatcher: &D,
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
    // Apply a bounded read/write timeout so a stalled peer cannot pin the
    // dispatcher thread indefinitely.
    if let Err(err) = stream.set_read_timeout(Some(CONNECTION_IO_TIMEOUT)) {
        return ConnectionCloseReason::TimeoutConfigFailed(format!(
            "set_read_timeout: {err}"
        ));
    }
    if let Err(err) = stream.set_write_timeout(Some(CONNECTION_IO_TIMEOUT)) {
        return ConnectionCloseReason::TimeoutConfigFailed(format!(
            "set_write_timeout: {err}"
        ));
    }
    loop {
        let request = match decode_request::<RequestEnvelopeT, _>(&mut stream) {
            Ok(env) => env,
            Err(IpcError::Truncated) => return ConnectionCloseReason::PeerClosed,
            Err(err) => return ConnectionCloseReason::RequestDecodeFailed(err),
        };
        let (request_id, request_payload) = request.into_parts();
        let response_payload = dispatcher.dispatch(request_payload);
        let response = ResponseEnvelopeT::from_parts(request_id, response_payload);
        let frame = match encode_response(&response) {
            Ok(frame) => frame,
            Err(err) => return ConnectionCloseReason::ResponseEncodeFailed(err),
        };
        if let Err(err) = stream.write_all(&frame) {
            return ConnectionCloseReason::ResponseWriteFailed(err.to_string());
        }
        // continue: next request on same conn
    }
}

/// One-shot client: open a stream, send `request`, read one response.
pub fn send_request<RequestEnvelopeT, ResponseEnvelopeT>(
    socket: &Path,
    request: &RequestEnvelopeT,
) -> Result<ResponseEnvelopeT, IpcError>
where
    RequestEnvelopeT: serde::Serialize,
    ResponseEnvelopeT: serde::de::DeserializeOwned,
{
    let mut stream = UnixStream::connect(socket).map_err(IpcError::Io)?;
    let frame = encode_request(request)?;
    stream.write_all(&frame).map_err(IpcError::Io)?;
    let response = decode_response::<ResponseEnvelopeT, _>(&mut stream)?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionCloseReason, IpcDispatcher, IpcError, RequestEnvelope, ResponseEnvelope,
        decode_response, encode_request, handle_connection,
    };
    use std::io::Write;
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;

    use serde::de::{self, MapAccess, Visitor};
    use serde::ser::SerializeStruct;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    struct TestDispatcher;

    impl IpcDispatcher<u64, u64> for TestDispatcher {
        fn dispatch(&self, request: u64) -> u64 {
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
    }

    struct FailingResponseEnvelope;

    impl Serialize for FailingResponseEnvelope {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(serde::ser::Error::custom("simulated response encode failure"))
        }
    }

    impl ResponseEnvelope<u64> for FailingResponseEnvelope {
        fn from_parts(_request_id: u64, _payload: u64) -> Self {
            Self
        }
    }

    fn test_request(request_id: u64, payload: u64) -> TestRequestEnvelope {
        TestRequestEnvelope {
            request_id,
            payload,
        }
    }

    #[test]
    fn handle_connection_returns_peer_closed_after_successful_round_trip() -> TestRes {
        let (mut client, server) = UnixStream::pair()?;
        let frame = encode_request(&test_request(41, 8))?;
        client.write_all(&frame)?;
        client.shutdown(Shutdown::Write)?;

        let handle = std::thread::spawn(move || {
            handle_connection::<TestRequestEnvelope, u64, TestResponseEnvelope, u64, TestDispatcher>(
                server,
                &TestDispatcher,
            )
        });

        let response = decode_response::<TestResponseEnvelope, _>(&mut client)?;
        assert_eq!(
            response,
            TestResponseEnvelope {
                request_id: 41,
                payload: 9,
            }
        );
        let reason = match handle.join() {
            Ok(reason) => reason,
            Err(_) => return Err("server thread panicked".into()),
        };
        assert!(matches!(reason, ConnectionCloseReason::PeerClosed));
        Ok(())
    }

    #[test]
    fn handle_connection_surfaces_decode_failure_reason() -> TestRes {
        let (mut client, server) = UnixStream::pair()?;
        client.write_all(&[0, 0, 0, 0])?;
        client.shutdown(Shutdown::Write)?;

        let reason = handle_connection::<TestRequestEnvelope, u64, TestResponseEnvelope, u64, TestDispatcher>(
            server,
            &TestDispatcher,
        );
        assert!(matches!(
            reason,
            ConnectionCloseReason::RequestDecodeFailed(IpcError::EmptyFrame)
        ));
        Ok(())
    }

    #[test]
    fn handle_connection_surfaces_response_encode_failure_reason() -> TestRes {
        let (mut client, server) = UnixStream::pair()?;
        let frame = encode_request(&test_request(7, 4))?;
        client.write_all(&frame)?;
        client.shutdown(Shutdown::Write)?;

        let reason = handle_connection::<TestRequestEnvelope, u64, FailingResponseEnvelope, u64, TestDispatcher>(
            server,
            &TestDispatcher,
        );
        match reason {
            ConnectionCloseReason::ResponseEncodeFailed(IpcError::Encode(message))
                if message.contains("simulated response encode failure") => {}
            other => return Err(format!("unexpected close reason: {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn handle_connection_surfaces_response_write_failure_reason() -> TestRes {
        let (mut client, server) = UnixStream::pair()?;
        let frame = encode_request(&test_request(9, 1))?;
        client.write_all(&frame)?;

        struct BlockingDispatcher {
            entered: std::sync::mpsc::Sender<()>,
            gate: std::sync::Arc<std::sync::Barrier>,
        }

        impl IpcDispatcher<u64, u64> for BlockingDispatcher {
            fn dispatch(&self, request: u64) -> u64 {
                assert!(
                    self.entered.send(()).is_ok(),
                    "test must observe dispatcher entry"
                );
                let _wait = self.gate.wait();
                request.saturating_add(1)
            }
        }

        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let dispatcher = BlockingDispatcher {
            entered: entered_tx,
            gate: gate.clone(),
        };
        let handle = std::thread::spawn(move || {
            handle_connection::<TestRequestEnvelope, u64, TestResponseEnvelope, u64, BlockingDispatcher>(
                server,
                &dispatcher,
            )
        });

        match entered_rx.recv() {
            Ok(()) => {}
            Err(err) => {
                return Err(
                    format!("test must observe request decode before closing peer: {err}").into(),
                );
            }
        }
        drop(client);
        let _wait = gate.wait();

        let reason = match handle.join() {
            Ok(reason) => reason,
            Err(_) => return Err("server thread panicked".into()),
        };
        match reason {
            ConnectionCloseReason::ResponseWriteFailed(message) if !message.is_empty() => {}
            other => return Err(format!("unexpected close reason: {other:?}").into()),
        }
        Ok(())
    }
}
