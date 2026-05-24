//! UDS listener + per-connection dispatch loop (T4.4, hellgate H-SP3).
//!
//! Owns the tokio `UnixListener` lifecycle, frames every connection via the
//! `quanta-index-ipc` CBOR codec, and routes requests to the supplied query
//! dispatcher. Decode failures are returned as `SearchPlaneIpcResponse::Error`
//! envelopes without killing the listener (H-SP3 fail-closed).
//!
//! Style note: this module triggers a handful of clippy nursery lints whose
//! suggested rewrites conflict with workspace `disallowed-methods` (e.g. the
//! suggestions push `unwrap_or` / `Runtime::block_on` patterns). The
//! `#[expect]` annotations on individual sites document each one explicitly.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use quanta_index_contract::{
    SearchPlaneIpcError, SearchPlaneIpcRequest, SearchPlaneIpcRequestEnvelope,
    SearchPlaneIpcResponse, SearchPlaneIpcResponseEnvelope,
};
use quanta_index_core::{
    CoreError, SearchPlaneExplainQueryPort, SearchPlaneHybridQueryPort,
    SearchPlaneLexicalQueryPort, SearchPlaneSemanticQueryPort,
};
use quanta_index_ipc::{IpcError, decode_request, encode_response};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

/// Driving-side dispatcher composed of the 4 inbound query ports.
///
/// `searchd::app` wires this to the `DomainQueryEngine` at composition time;
/// tests inject a lighter-weight stub that implements the same trait bundle.
pub trait QueryDispatcher:
    SearchPlaneLexicalQueryPort
    + SearchPlaneSemanticQueryPort
    + SearchPlaneHybridQueryPort
    + SearchPlaneExplainQueryPort
    + Send
    + Sync
{
}

impl<T> QueryDispatcher for T where
    T: SearchPlaneLexicalQueryPort
        + SearchPlaneSemanticQueryPort
        + SearchPlaneHybridQueryPort
        + SearchPlaneExplainQueryPort
        + Send
        + Sync
{
}

/// Server handle returned by [`UdsListener::bind`]. Drop the handle (or call
/// [`UdsListener::shutdown`]) to stop accepting connections.
pub struct UdsListener {
    listener: UnixListener,
    socket_path: PathBuf,
    shutdown: Arc<Notify>,
}

impl UdsListener {
    /// Bind a Unix socket at `socket_path`. Removes any pre-existing socket
    /// file at that path (operator-owned), then creates a fresh listener.
    pub async fn bind(socket_path: impl Into<PathBuf>) -> Result<Self> {
        let socket_path = socket_path.into();
        if let Some(parent) = socket_path.parent() {
            let parent_display = parent.display().to_string();
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| anyhow!("create socket parent {parent_display}: {error}"))?;
        }
        // If a stale socket file is present, remove it. We do NOT probe for a
        // live listener here because the operator is expected to ensure no
        // other searchd instance is running on the same path.
        match tokio::fs::remove_file(&socket_path).await {
            Ok(()) | Err(_) => {}
        }
        let bind_display = socket_path.display().to_string();
        let listener = UnixListener::bind(&socket_path)
            .map_err(|error| anyhow!("UnixListener::bind {bind_display}: {error}"))?;
        Ok(Self {
            listener,
            socket_path,
            shutdown: Arc::new(Notify::new()),
        })
    }

    /// Return a shutdown trigger that signals the accept loop to stop.
    #[must_use]
    pub fn shutdown_trigger(&self) -> Arc<Notify> {
        Arc::clone(&self.shutdown)
    }

    /// Socket path the listener is bound to.
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Run the accept loop. Each accepted connection is spawned onto the
    /// current tokio runtime via `dispatcher`'s `Clone` so the listener
    /// continues accepting new connections while a request is in flight.
    pub async fn serve<D>(self, dispatcher: Arc<D>) -> Result<()>
    where
        D: QueryDispatcher + 'static,
    {
        let shutdown = Arc::clone(&self.shutdown);
        let listener = self.listener;
        let socket_path = self.socket_path.clone();
        let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let accept_display = socket_path.display().to_string();
                    let (stream, _peer) = accepted
                        .map_err(|error| anyhow!("accept on {accept_display}: {error}"))?;
                    let task_dispatcher = Arc::clone(&dispatcher);
                    let handle = tokio::spawn(async move {
                        let _result: Result<(), anyhow::Error> = handle_connection(stream, task_dispatcher).await;
                    });
                    tasks.push(handle);
                }
                () = shutdown.notified() => {
                    break;
                }
            }
        }
        // Best-effort: wait for in-flight connections to drain. We don't
        // forcibly cancel since each connection is already cooperative.
        for task in tasks {
            let _result: Result<(), tokio::task::JoinError> = task.await;
        }
        // Operator owns lifecycle of the socket file; best-effort cleanup.
        let _removed: Result<(), std::io::Error> = tokio::fs::remove_file(&socket_path).await;
        Ok(())
    }
}

/// Run the per-connection request/response loop until the peer half-closes or
/// emits a transport error we cannot recover from.
async fn handle_connection<D: QueryDispatcher + 'static>(
    mut stream: UnixStream,
    dispatcher: Arc<D>,
) -> Result<()> {
    loop {
        let frame = match read_frame(&mut stream).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(()), // peer closed cleanly
            Err(io_error) => return Err(io_error),
        };
        let envelope_outcome = decode_envelope(&frame);
        let response = match envelope_outcome {
            Ok(envelope) => dispatch(&dispatcher, envelope),
            Err(decode_error) => SearchPlaneIpcResponseEnvelope {
                request_id: 0,
                payload: SearchPlaneIpcResponse::Error(SearchPlaneIpcError {
                    code: "ipc.decode".to_owned(),
                    message: format!("{decode_error}"),
                }),
            },
        };
        let response_bytes =
            encode_response(&response).map_err(|error| anyhow!("encode response: {error:?}"))?;
        stream
            .write_all(&response_bytes)
            .await
            .map_err(|error| anyhow!("write response: {error}"))?;
        stream
            .flush()
            .await
            .map_err(|error| anyhow!("flush response: {error}"))?;
    }
}

/// Read a single framed envelope. Returns `Ok(None)` for clean peer EOF
/// (zero bytes on first header byte), `Ok(Some(body))` for a complete frame,
/// `Err` for any other transport error.
async fn read_frame(stream: &mut UnixStream) -> Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    let mut filled = 0usize;
    while filled < header.len() {
        let Some(dst) = header.get_mut(filled..) else {
            return Err(anyhow!("internal: header slice index out of range"));
        };
        match stream.read(dst).await {
            Ok(0) => {
                if filled == 0 {
                    return Ok(None);
                }
                return Err(anyhow!("EOF mid-header after {filled} bytes"));
            }
            Ok(n) => {
                filled = filled.saturating_add(n);
            }
            Err(error) => return Err(anyhow!("read header: {error}")),
        }
    }
    let length = u32::from_le_bytes(header);
    let length_usize = usize::try_from(length)
        .map_err(|error| anyhow!("header length {length} overflows usize: {error}"))?;
    let total = length_usize
        .checked_add(4)
        .ok_or_else(|| anyhow!("frame length {length_usize} + header overflows usize"))?;
    let mut body = vec![0u8; total];
    let Some(prefix) = body.get_mut(..4) else {
        return Err(anyhow!("internal: body prefix slice index out of range"));
    };
    prefix.copy_from_slice(&header);
    let Some(body_dst) = body.get_mut(4..) else {
        return Err(anyhow!("internal: body slice index out of range"));
    };
    let mut body_filled = 0usize;
    while body_filled < length_usize {
        let Some(dst) = body_dst.get_mut(body_filled..) else {
            return Err(anyhow!("internal: body slice index out of range"));
        };
        match stream.read(dst).await {
            Ok(0) => {
                return Err(anyhow!(
                    "EOF mid-body after {body_filled} of {length_usize} bytes"
                ));
            }
            Ok(n) => {
                body_filled = body_filled.saturating_add(n);
            }
            Err(error) => return Err(anyhow!("read body: {error}")),
        }
    }
    Ok(Some(body))
}

/// Decode a frame's bytes into a request envelope by feeding them through
/// the IPC codec's `decode_request`.
fn decode_envelope(frame: &[u8]) -> Result<SearchPlaneIpcRequestEnvelope, IpcError> {
    let mut cursor = std::io::Cursor::new(frame);
    decode_request(&mut cursor)
}

/// Route a single request through the dispatcher.
fn dispatch<D: QueryDispatcher>(
    dispatcher: &Arc<D>,
    envelope: SearchPlaneIpcRequestEnvelope,
) -> SearchPlaneIpcResponseEnvelope {
    let request_id = envelope.request_id;
    let payload = match envelope.payload {
        SearchPlaneIpcRequest::Lexical(request) => match dispatcher.lexical_query(request) {
            Ok(response) => SearchPlaneIpcResponse::Lexical(response),
            Err(error) => core_error_to_ipc(&error),
        },
        SearchPlaneIpcRequest::Semantic(request) => match dispatcher.semantic_query(request) {
            Ok(response) => SearchPlaneIpcResponse::Semantic(response),
            Err(error) => core_error_to_ipc(&error),
        },
        SearchPlaneIpcRequest::Hybrid(request) => match dispatcher.hybrid_query(request) {
            Ok(response) => SearchPlaneIpcResponse::Hybrid(response),
            Err(error) => core_error_to_ipc(&error),
        },
        SearchPlaneIpcRequest::Explain(request) => match dispatcher.explain_query(request) {
            Ok(response) => SearchPlaneIpcResponse::Explain(response),
            Err(error) => core_error_to_ipc(&error),
        },
    };
    SearchPlaneIpcResponseEnvelope {
        request_id,
        payload,
    }
}

fn core_error_to_ipc(error: &CoreError) -> SearchPlaneIpcResponse {
    let (code, message) = match error {
        CoreError::InvalidContract(message) => ("invalid_contract", message.clone()),
        CoreError::NotReady(message) => ("not_ready", message.clone()),
        CoreError::NotImplemented(message) => ("not_implemented", message.clone()),
        CoreError::NotFound(message) => ("not_found", message.clone()),
        CoreError::Storage(message) => ("storage", message.clone()),
    };
    SearchPlaneIpcResponse::Error(SearchPlaneIpcError {
        code: code.to_owned(),
        message,
    })
}

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "tokio::test macro expands to Runtime::block_on; tests intentionally use sync wrappers"
)]
#[expect(
    clippy::similar_names,
    reason = "test scratch variables length/length_usize and header/header2 are intentional pairs"
)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test asserts against expected SearchPlaneIpcResponse variant only"
)]
mod tests {
    use std::sync::Arc;

    use quanta_index_contract::{
        LqDirectiveSet, LqExpr, LqFilterSet, LqOptionSet, LqQuery, SearchPlaneExplainQueryRequest,
        SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest,
        SearchPlaneHybridQueryResponse, SearchPlaneIpcError, SearchPlaneIpcRequest,
        SearchPlaneIpcRequestEnvelope, SearchPlaneIpcResponse, SearchPlaneLexicalQueryRequest,
        SearchPlaneLexicalQueryResponse, SearchPlaneSemanticQueryRequest,
        SearchPlaneSemanticQueryResponse,
    };
    use quanta_index_core::{
        CoreError, SearchPlaneExplainQueryPort, SearchPlaneHybridQueryPort,
        SearchPlaneLexicalQueryPort, SearchPlaneSemanticQueryPort,
    };
    use quanta_index_ipc::{decode_response, encode_request};
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::UdsListener;

    struct StaticDispatcher;

    impl SearchPlaneLexicalQueryPort for StaticDispatcher {
        fn lexical_query(
            &self,
            request: SearchPlaneLexicalQueryRequest,
        ) -> Result<SearchPlaneLexicalQueryResponse, CoreError> {
            Ok(SearchPlaneLexicalQueryResponse {
                generation: request.generation.unwrap_or_else(default_generation),
                results: Vec::new(),
            })
        }
    }

    impl SearchPlaneSemanticQueryPort for StaticDispatcher {
        fn semantic_query(
            &self,
            _request: SearchPlaneSemanticQueryRequest,
        ) -> Result<SearchPlaneSemanticQueryResponse, CoreError> {
            Err(CoreError::NotImplemented("semantic stub".into()))
        }
    }

    impl SearchPlaneHybridQueryPort for StaticDispatcher {
        fn hybrid_query(
            &self,
            _request: SearchPlaneHybridQueryRequest,
        ) -> Result<SearchPlaneHybridQueryResponse, CoreError> {
            Err(CoreError::NotImplemented("hybrid stub".into()))
        }
    }

    impl SearchPlaneExplainQueryPort for StaticDispatcher {
        fn explain_query(
            &self,
            _request: SearchPlaneExplainQueryRequest,
        ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
            Err(CoreError::NotReady("explain stub".into()))
        }
    }

    fn default_generation() -> quanta_index_contract::PublishedGenerationSet {
        quanta_index_contract::PublishedGenerationSet {
            repo_id: quanta_index_contract::RepoId::new("repo"),
            revision_id: quanta_index_contract::RevisionId::new("rev"),
            manifest_generation: quanta_index_contract::ManifestGeneration::new(1),
            lexical_generation: quanta_index_contract::GenerationId::new(2),
            symbol_generation: quanta_index_contract::GenerationId::new(3),
            structural_generation: None,
            history_generation: None,
            semantic_generation: None,
            metadata_generation: None,
        }
    }

    fn lexical_envelope(request_id: u64) -> SearchPlaneIpcRequestEnvelope {
        SearchPlaneIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneIpcRequest::Lexical(SearchPlaneLexicalQueryRequest {
                query: LqQuery {
                    expr: LqExpr::Raw("anything".into()),
                    filters: LqFilterSet {
                        filters: Vec::new(),
                    },
                    options: LqOptionSet {
                        limit: None,
                        count_all: false,
                        timeout_ms: None,
                    },
                    directives: LqDirectiveSet {
                        directives: Vec::new(),
                    },
                },
                generation: Some(default_generation()),
            }),
        }
    }

    #[tokio::test]
    async fn end_to_end_lexical_request_round_trips_over_socket() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => {
                assert!(false, "tempdir: {error}");
                return;
            }
        };
        let socket = dir.path().join("searchd.sock");
        let listener = match UdsListener::bind(&socket).await {
            Ok(l) => l,
            Err(error) => {
                assert!(false, "bind: {error}");
                return;
            }
        };
        let shutdown = listener.shutdown_trigger();
        let dispatcher = Arc::new(StaticDispatcher);
        let serve_handle = tokio::spawn(listener.serve(dispatcher));

        // Connect and send a Lexical request.
        let mut client = match tokio::net::UnixStream::connect(&socket).await {
            Ok(s) => s,
            Err(error) => {
                assert!(false, "connect: {error}");
                return;
            }
        };
        let envelope = lexical_envelope(42);
        let bytes = match encode_request(&envelope) {
            Ok(b) => b,
            Err(error) => {
                assert!(false, "encode: {error}");
                return;
            }
        };
        if let Err(error) = client.write_all(&bytes).await {
            assert!(false, "write: {error}");
            return;
        }

        // Read response frame: 4-byte LE length + body.
        let mut header = [0u8; 4];
        if let Err(error) = client.read_exact(&mut header).await {
            assert!(false, "read header: {error}");
            return;
        }
        let length = u32::from_le_bytes(header);
        let length_usize = match usize::try_from(length) {
            Ok(value) => value,
            Err(error) => {
                assert!(false, "len overflow: {error}");
                return;
            }
        };
        let mut body = vec![0u8; length_usize];
        if let Err(error) = client.read_exact(&mut body).await {
            assert!(false, "read body: {error}");
            return;
        }
        let mut full = Vec::with_capacity(4_usize.saturating_add(length_usize));
        full.extend_from_slice(&header);
        full.extend_from_slice(&body);
        let mut cursor = std::io::Cursor::new(full);
        let response = match decode_response(&mut cursor) {
            Ok(r) => r,
            Err(error) => {
                assert!(false, "decode response: {error:?}");
                return;
            }
        };
        assert_eq!(response.request_id, 42);
        assert!(matches!(
            response.payload,
            SearchPlaneIpcResponse::Lexical(_)
        ));

        shutdown.notify_waiters();
        drop(client);
        let _join: Result<Result<(), anyhow::Error>, tokio::task::JoinError> = serve_handle.await;
    }

    #[tokio::test]
    async fn malformed_frame_yields_error_envelope_and_keeps_listener_alive() {
        // H-SP3: garbage on the wire returns Error envelope, listener stays up.
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => {
                assert!(false, "tempdir: {error}");
                return;
            }
        };
        let socket = dir.path().join("searchd.sock");
        let listener = match UdsListener::bind(&socket).await {
            Ok(l) => l,
            Err(error) => {
                assert!(false, "bind: {error}");
                return;
            }
        };
        let shutdown = listener.shutdown_trigger();
        let dispatcher = Arc::new(StaticDispatcher);
        let serve_handle = tokio::spawn(listener.serve(dispatcher));

        // Send a frame with garbage CBOR body but valid length header.
        let mut client = match tokio::net::UnixStream::connect(&socket).await {
            Ok(s) => s,
            Err(error) => {
                assert!(false, "connect: {error}");
                return;
            }
        };
        let garbage_body: Vec<u8> = vec![0xff, 0xff, 0xff, 0xff];
        let length = u32::try_from(garbage_body.len()).unwrap_or(4);
        let mut frame = Vec::with_capacity(8);
        frame.extend_from_slice(&length.to_le_bytes());
        frame.extend_from_slice(&garbage_body);
        if let Err(error) = client.write_all(&frame).await {
            assert!(false, "write: {error}");
            return;
        }

        // Read response.
        let mut header = [0u8; 4];
        if let Err(error) = client.read_exact(&mut header).await {
            assert!(false, "read header: {error}");
            return;
        }
        let length = u32::from_le_bytes(header);
        let length_usize = match usize::try_from(length) {
            Ok(value) => value,
            Err(error) => {
                assert!(false, "len overflow: {error}");
                return;
            }
        };
        let mut body = vec![0u8; length_usize];
        if let Err(error) = client.read_exact(&mut body).await {
            assert!(false, "read body: {error}");
            return;
        }
        let mut full = Vec::with_capacity(4_usize.saturating_add(length_usize));
        full.extend_from_slice(&header);
        full.extend_from_slice(&body);
        let mut cursor = std::io::Cursor::new(full);
        let response = match decode_response(&mut cursor) {
            Ok(r) => r,
            Err(error) => {
                assert!(false, "decode response: {error:?}");
                return;
            }
        };
        // request_id is 0 on decode-error envelopes (we couldn't learn it).
        assert_eq!(response.request_id, 0);
        match response.payload {
            SearchPlaneIpcResponse::Error(SearchPlaneIpcError { code, message }) => {
                assert_eq!(code, "ipc.decode");
                assert!(!message.is_empty());
            }
            other => {
                assert!(false, "expected Error envelope, got {other:?}");
            }
        }

        // Subsequent valid request still works → listener is alive.
        let envelope = lexical_envelope(7);
        let bytes = match encode_request(&envelope) {
            Ok(b) => b,
            Err(error) => {
                assert!(false, "encode: {error}");
                return;
            }
        };
        if let Err(error) = client.write_all(&bytes).await {
            assert!(false, "write 2: {error}");
            return;
        }
        let mut header2 = [0u8; 4];
        if let Err(error) = client.read_exact(&mut header2).await {
            assert!(false, "read header 2: {error}");
            return;
        }
        let length2 = u32::from_le_bytes(header2);
        let length2_usize = match usize::try_from(length2) {
            Ok(value) => value,
            Err(error) => {
                assert!(false, "len2 overflow: {error}");
                return;
            }
        };
        let mut body2 = vec![0u8; length2_usize];
        if let Err(error) = client.read_exact(&mut body2).await {
            assert!(false, "read body 2: {error}");
            return;
        }
        let mut full2 = Vec::with_capacity(4_usize.saturating_add(length2_usize));
        full2.extend_from_slice(&header2);
        full2.extend_from_slice(&body2);
        let mut cursor2 = std::io::Cursor::new(full2);
        let response2 = match decode_response(&mut cursor2) {
            Ok(r) => r,
            Err(error) => {
                assert!(false, "decode response 2: {error:?}");
                return;
            }
        };
        assert_eq!(response2.request_id, 7);
        assert!(matches!(
            response2.payload,
            SearchPlaneIpcResponse::Lexical(_)
        ));

        shutdown.notify_waiters();
        drop(client);
        let _join: Result<Result<(), anyhow::Error>, tokio::task::JoinError> = serve_handle.await;
    }

    #[tokio::test]
    async fn shutdown_trigger_stops_serve_loop() {
        let dir = match tempdir() {
            Ok(dir) => dir,
            Err(error) => {
                assert!(false, "tempdir: {error}");
                return;
            }
        };
        let socket = dir.path().join("searchd.sock");
        let listener = match UdsListener::bind(&socket).await {
            Ok(l) => l,
            Err(error) => {
                assert!(false, "bind: {error}");
                return;
            }
        };
        let shutdown = listener.shutdown_trigger();
        let dispatcher = Arc::new(StaticDispatcher);
        let serve_handle = tokio::spawn(listener.serve(dispatcher));
        // Allow listener to enter accept loop, then signal shutdown.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        shutdown.notify_waiters();
        match tokio::time::timeout(std::time::Duration::from_secs(2), serve_handle).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                assert!(false, "serve returned error: {error}");
            }
            Ok(Err(error)) => {
                assert!(false, "serve task joined with error: {error}");
            }
            Err(_) => {
                assert!(false, "serve did not shut down within timeout");
            }
        }
    }
}
