//! Shared IPC protocol limits and typed transport errors.

use std::time::Duration;

/// Client operation that exceeded its configured I/O timeout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpcIoOperation {
    /// Waiting for the Unix-domain socket connection to complete.
    Connect,
    /// Reading the response frame.
    Read,
    /// Writing the request frame.
    Write,
}

/// Maximum IPC frame body size accepted on the wire.
pub const MAX_FRAME_BODY_BYTES: usize = quanta_index_contract::MAX_IPC_FRAME_BODY_BYTES_V1;

/// Codec errors surfaced by encode and decode entry points.
#[derive(Debug)]
pub enum IpcError {
    /// Reader returned fewer bytes than required to complete the frame.
    Truncated,
    /// Declared body length exceeds [`MAX_FRAME_BODY_BYTES`].
    Oversized(u64),
    /// Declared body length is zero.
    EmptyFrame,
    /// CBOR encoder returned an error.
    Encode(String),
    /// CBOR decoder returned an error.
    Decode(String),
    /// Underlying transport returned an I/O error other than short read.
    Io(std::io::Error),
    /// A blocking client read or write exceeded its configured timeout.
    Timeout {
        operation: IpcIoOperation,
        timeout: Duration,
    },
    /// Client request policy supplied a zero I/O timeout.
    InvalidClientIoTimeout,
    /// The owner-supplied absolute request deadline elapsed before dispatch.
    ClientIoDeadlineElapsed,
    /// No ready response arrived within the readiness window: every
    /// attempt either failed at the transport or answered not-ready.
    /// Carries the attempt count and the last observation, so a spent
    /// wait is typed timeout evidence — never a not-ready payload
    /// relabeled as a remote refusal.
    ReadinessTimeout {
        timeout: Duration,
        attempts: u64,
        last: String,
    },
    /// A server admission policy named a zero limit or more dispatch slots
    /// than connections.
    InvalidAdmissionPolicy,
    /// Concurrent request payloads exhausted a server ingress bound.
    IngressSaturated { bytes: usize, requests: usize },
    /// A live listener already answers at the socket path; it was left in
    /// place (QI-BB-014).
    SocketInUse(std::path::PathBuf),
    /// The socket path or its directory cannot be made private: wrong
    /// owner, a symlink, or a mode that lets others in (QI-BB-014).
    SocketPathInsecure {
        path: std::path::PathBuf,
        reason: String,
    },
    /// The socket's shared access policy cannot be honoured here: this
    /// process is not a member of the shared group, or a directory on the
    /// socket's path cannot be traversed by the peers the policy admits
    /// (QI-BB-014, shared mode). Nothing was bound.
    SocketAccessUnsatisfiable {
        path: std::path::PathBuf,
        reason: String,
    },
    /// An envelope carried request id 0 (W10-R2). Transport request ids
    /// are nonzero by construction — the SDK allocator never emits 0 and
    /// the server never admits it — so 0 is a malformed envelope, refused
    /// before admission, typed, with no dispatch and no response.
    ZeroRequestId,
}

impl core::fmt::Display for IpcError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Truncated => f.write_str("ipc frame truncated"),
            Self::Oversized(len) => {
                write!(
                    f,
                    "ipc frame body length {len} exceeds cap of {MAX_FRAME_BODY_BYTES} bytes"
                )
            }
            Self::EmptyFrame => f.write_str("ipc frame declared zero-length body"),
            Self::Encode(msg) => write!(f, "ipc cbor encode failed: {msg}"),
            Self::Decode(msg) => write!(f, "ipc cbor decode failed: {msg}"),
            Self::Io(err) => write!(f, "ipc transport io error: {err}"),
            Self::Timeout { operation, timeout } => {
                write!(
                    f,
                    "ipc {operation:?} timed out after {} ms",
                    timeout.as_millis()
                )
            }
            Self::InvalidClientIoTimeout => {
                f.write_str("client I/O timeout must be greater than zero")
            }
            Self::ClientIoDeadlineElapsed => {
                f.write_str("client I/O deadline elapsed before request dispatch")
            }
            Self::ReadinessTimeout {
                timeout,
                attempts,
                last,
            } => {
                write!(
                    f,
                    "readiness timeout after {} ms and {attempts} attempts; last observed: {last}",
                    timeout.as_millis()
                )
            }
            Self::InvalidAdmissionPolicy => f.write_str(
                "server admission policy must have non-zero connections, slots, per-repository in-flight cap, budget and I/O timeout, with per-repository cap <= slots <= connections",
            ),
            Self::IngressSaturated { bytes, requests } => {
                write!(f, "ipc request ingress admission saturated (up to {bytes} request-buffer bytes and {requests} concurrent requests)")
            }
            Self::SocketInUse(path) => write!(
                f,
                "SOCKET_IN_USE: a live listener already answers at {}; refusing to take its path",
                path.display()
            ),
            Self::SocketPathInsecure { path, reason } => write!(
                f,
                "SOCKET_PATH_INSECURE: {} cannot be served privately: {reason}",
                path.display()
            ),
            Self::SocketAccessUnsatisfiable { path, reason } => write!(
                f,
                "SOCKET_ACCESS_UNSATISFIABLE: {} cannot be shared as configured: {reason}",
                path.display()
            ),
            Self::ZeroRequestId => f.write_str(
                "request envelope carries request id 0: transport request ids are nonzero",
            ),
        }
    }
}

impl std::error::Error for IpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Truncated
            | Self::Oversized(_)
            | Self::EmptyFrame
            | Self::Encode(_)
            | Self::Decode(_)
            | Self::Timeout { .. }
            | Self::InvalidClientIoTimeout
            | Self::ClientIoDeadlineElapsed
            | Self::ReadinessTimeout { .. }
            | Self::InvalidAdmissionPolicy
            | Self::IngressSaturated { .. }
            | Self::SocketInUse(_)
            | Self::SocketPathInsecure { .. }
            | Self::SocketAccessUnsatisfiable { .. }
            | Self::ZeroRequestId => None,
        }
    }
}
