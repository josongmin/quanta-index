//! `AF_UNIX` stream server + client.
//!
//! The server accepts connections sequentially (phase-1 rule: at-most-one
//! in-flight request per connection), decodes a single envelope per frame,
//! routes through the [`QueryDispatcher`] supplied by the composition root,
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
    SearchPlaneIpcRequest, SearchPlaneIpcRequestEnvelope, SearchPlaneIpcResponse,
    SearchPlaneIpcResponseEnvelope,
};

use crate::codec::{IpcError, decode_request, decode_response, encode_request, encode_response};

/// Dispatch hook supplied by the composition root.
///
/// Receives a fully-parsed request payload and returns a fully-typed response
/// payload. Domain errors (e.g. `NOT_READY`) MUST be surfaced as
/// [`SearchPlaneIpcResponse::Error`] — never panic.
pub trait QueryDispatcher: Send + Sync {
    fn dispatch(&self, request: SearchPlaneIpcRequest) -> SearchPlaneIpcResponse;
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
    pub fn run<D: QueryDispatcher + ?Sized>(
        &self,
        dispatcher: &Arc<D>,
        accept_idle: Duration,
    ) -> Result<(), IpcError> {
        while !self.shutdown.load(Ordering::Acquire) {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    let dispatcher = Arc::clone(dispatcher);
                    handle_connection(stream, dispatcher.as_ref());
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

fn handle_connection<D: QueryDispatcher + ?Sized>(mut stream: UnixStream, dispatcher: &D) {
    // Each connection may carry multiple sequential requests until close.
    if stream.set_nonblocking(false).is_err() {
        // Cannot operate the connection in blocking mode here; drop it.
        return;
    }
    // Apply a bounded read/write timeout so a stalled peer cannot pin the
    // dispatcher thread indefinitely.
    if stream.set_read_timeout(Some(CONNECTION_IO_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(CONNECTION_IO_TIMEOUT)).is_err()
    {
        return;
    }
    loop {
        let request = match decode_request(&mut stream) {
            Ok(env) => env,
            Err(IpcError::Truncated) => return, // peer closed cleanly
            Err(_other_err) => return,          // framing / oversize / decode → close
        };
        let response_payload = dispatcher.dispatch(request.payload);
        let response = SearchPlaneIpcResponseEnvelope {
            request_id: request.request_id,
            payload: response_payload,
        };
        let Ok(frame) = encode_response(&response) else {
            return;
        };
        if stream.write_all(&frame).is_err() {
            return;
        }
        // continue: next request on same conn
    }
}

/// One-shot client: open a stream, send `request`, read one response.
pub fn send_request(
    socket: &Path,
    request: &SearchPlaneIpcRequestEnvelope,
) -> Result<SearchPlaneIpcResponseEnvelope, IpcError> {
    let mut stream = UnixStream::connect(socket).map_err(IpcError::Io)?;
    let frame = encode_request(request)?;
    stream.write_all(&frame).map_err(IpcError::Io)?;
    let response = decode_response(&mut stream)?;
    Ok(response)
}
