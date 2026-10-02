//! Length-prefixed CBOR frame codec for search-plane IPC envelopes.

use std::io::{ErrorKind, Read};
use std::time::Duration;

use sha2::Digest;

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

/// Maximum decoded body after bounded IPC request compression.
const MAX_DECOMPRESSED_FRAME_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Maximum complete CBOR request admitted across multiple bounded frames.
///
/// The ingest resource policy still independently bounds semantic text,
/// source bytes, records, and vector residency before publication.
const MAX_MULTIFRAME_REQUEST_BODY_BYTES: usize = 128 * 1024 * 1024;

/// Private transport marker; the following bytes are decoded length, SHA-256,
/// then one zstd frame containing the original CBOR request body.
const COMPRESSED_REQUEST_MAGIC: &[u8; 8] = b"QIPCZST1";
const COMPRESSED_REQUEST_HEADER_BYTES: usize = 8 + 8 + 32;
const COMPRESSED_REQUEST_METADATA_BYTES: usize = 8 + 32;

/// A large request is one logical CBOR body split across adjacent frames on
/// the same connection.
///
/// The complete body and digest are checked before dispatch. Sequence numbers
/// make reordered or duplicated fragments fail closed. A disconnect discards
/// the in-memory assembly.
const MULTIFRAME_REQUEST_MAGIC: &[u8; 8] = b"QIPCMF01";
const MULTIFRAME_FIRST_METADATA_BYTES: usize = 8 + 8 + 32;
const MULTIFRAME_FIRST_MIN_BODY_BYTES: usize = 41;
const MULTIFRAME_NEXT_METADATA_BYTES: usize = 8 + 4;

/// Width of the length-prefix header in bytes.
const FRAME_HEADER_BYTES: usize = 4;

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

pub fn encode_request<T: serde::Serialize>(envelope: &T) -> Result<Vec<u8>, IpcError> {
    let logical_len = usize::try_from(cbor_payload_len(envelope)?)
        .map_err(|_overflow| IpcError::Encode("request body length overflowed usize".into()))?;
    if logical_len <= MAX_FRAME_BODY_BYTES {
        return encode_frame(envelope);
    }
    if logical_len > MAX_MULTIFRAME_REQUEST_BODY_BYTES {
        return Err(IpcError::Encode(format!(
            "request body is {logical_len} bytes, exceeding the {MAX_MULTIFRAME_REQUEST_BODY_BYTES} byte decoded-request cap"
        )));
    }

    let body = encode_cbor_payload(envelope)?;
    if body.len() != logical_len {
        return Err(IpcError::Encode(
            "request body length changed between count and encode".into(),
        ));
    }
    if logical_len <= MAX_DECOMPRESSED_FRAME_BODY_BYTES {
        let compressed = zstd::bulk::compress(&body, 1)
            .map_err(|err| IpcError::Encode(format!("request compression failed: {err}")))?;
        let wire_len = COMPRESSED_REQUEST_HEADER_BYTES
            .checked_add(compressed.len())
            .ok_or_else(|| IpcError::Encode("compressed request length overflowed usize".into()))?;
        if wire_len <= MAX_FRAME_BODY_BYTES {
            let digest = sha2::Sha256::digest(&body);
            let wire_len_u32 = u32::try_from(wire_len).map_err(|_overflow| {
                IpcError::Encode("compressed body length overflowed u32".into())
            })?;
            let frame_len = wire_len.checked_add(FRAME_HEADER_BYTES).ok_or_else(|| {
                IpcError::Encode("compressed frame length overflowed usize".into())
            })?;
            let mut frame = Vec::with_capacity(frame_len);
            frame.extend_from_slice(&wire_len_u32.to_le_bytes());
            frame.extend_from_slice(COMPRESSED_REQUEST_MAGIC);
            frame.extend_from_slice(
                &u64::try_from(logical_len)
                    .map_err(|_overflow| {
                        IpcError::Encode("request body length overflowed u64".into())
                    })?
                    .to_le_bytes(),
            );
            frame.extend_from_slice(&digest);
            frame.extend_from_slice(&compressed);
            return Ok(frame);
        }
    }
    encode_multiframe_request(&body)
}

fn encode_multiframe_request(body: &[u8]) -> Result<Vec<u8>, IpcError> {
    let digest = sha2::Sha256::digest(body);
    let mut output = Vec::with_capacity(body.len().saturating_add(256));
    let first_capacity = MAX_FRAME_BODY_BYTES - MULTIFRAME_FIRST_METADATA_BYTES;
    let first_len = first_capacity.min(body.len());
    let first_frame_len = MULTIFRAME_FIRST_METADATA_BYTES
        .checked_add(first_len)
        .ok_or_else(|| IpcError::Encode("first fragment length overflowed usize".into()))?;
    output.extend_from_slice(
        &u32::try_from(first_frame_len)
            .map_err(|_overflow| IpcError::Encode("first fragment length overflowed u32".into()))?
            .to_le_bytes(),
    );
    output.extend_from_slice(MULTIFRAME_REQUEST_MAGIC);
    output.extend_from_slice(
        &u64::try_from(body.len())
            .map_err(|_overflow| IpcError::Encode("request body length overflowed u64".into()))?
            .to_le_bytes(),
    );
    output.extend_from_slice(&digest);
    output.extend_from_slice(
        body.get(..first_len)
            .ok_or_else(|| IpcError::Encode("first fragment exceeds body".into()))?,
    );
    let mut offset = first_len;
    let mut sequence = 1_u32;
    let next_capacity = MAX_FRAME_BODY_BYTES - MULTIFRAME_NEXT_METADATA_BYTES;
    while offset < body.len() {
        let end = body.len().min(offset.saturating_add(next_capacity));
        let fragment_len = MULTIFRAME_NEXT_METADATA_BYTES
            .checked_add(end.saturating_sub(offset))
            .ok_or_else(|| IpcError::Encode("fragment length overflowed usize".into()))?;
        output.extend_from_slice(
            &u32::try_from(fragment_len)
                .map_err(|_overflow| IpcError::Encode("fragment length overflowed u32".into()))?
                .to_le_bytes(),
        );
        output.extend_from_slice(MULTIFRAME_REQUEST_MAGIC);
        output.extend_from_slice(&sequence.to_le_bytes());
        output.extend_from_slice(
            body.get(offset..end)
                .ok_or_else(|| IpcError::Encode("fragment exceeds body".into()))?,
        );
        offset = end;
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| IpcError::Encode("fragment sequence overflow".into()))?;
    }
    Ok(output)
}

pub fn encode_response<T: serde::Serialize>(envelope: &T) -> Result<Vec<u8>, IpcError> {
    encode_frame(envelope)
}

pub fn encode_cbor_payload<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, IpcError> {
    let mut body: Vec<u8> = Vec::new();
    ciborium::into_writer(value, &mut body).map_err(|err| IpcError::Encode(err.to_string()))?;
    Ok(body)
}

/// Counts what an encoder writes without keeping it.
struct EncodedLength(u64);

impl std::io::Write for EncodedLength {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = u64::try_from(bytes.len())
            .map_err(|err| std::io::Error::other(format!("encoded length overflow: {err}")))?;
        self.0 = self
            .0
            .checked_add(written)
            .ok_or_else(|| std::io::Error::other("encoded length overflows u64".to_string()))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The bytes [`encode_cbor_payload`] would produce for `value`, counted
/// without allocating them: what a page costs in a frame before it is
/// encoded (QI-BB-005 보완 #5).
pub fn cbor_payload_len<T: serde::Serialize>(value: &T) -> Result<u64, IpcError> {
    let mut counter = EncodedLength(0);
    ciborium::into_writer(value, &mut counter).map_err(|err| IpcError::Encode(err.to_string()))?;
    Ok(counter.0)
}

pub fn decode_request<T, R>(reader: &mut R) -> Result<T, IpcError>
where
    T: serde::de::DeserializeOwned,
    R: Read,
{
    decode_frame(reader, true, false, |_| Ok(()), |(), _| Ok(())).map(|(value, ())| value)
}

/// Reserve server ingress bytes after validating the first frame header,
/// before allocating its body.
///
/// The returned guard must cover the decoded request through dispatch and
/// response; otherwise queued envelopes can exceed the ingress bound.
pub(crate) fn decode_request_guarded<T, R, G>(
    reader: &mut R,
    admit: impl FnOnce(usize) -> Result<G, IpcError>,
    reserve: impl FnMut(&mut G, usize) -> Result<(), IpcError>,
) -> Result<(T, G), IpcError>
where
    T: serde::de::DeserializeOwned,
    R: Read,
{
    decode_frame(reader, true, true, admit, reserve)
}

pub fn decode_response<T, R>(reader: &mut R) -> Result<T, IpcError>
where
    T: serde::de::DeserializeOwned,
    R: Read,
{
    decode_frame(reader, false, false, |_| Ok(()), |(), _| Ok(())).map(|(value, ())| value)
}

pub fn decode_cbor_payload<T>(bytes: &[u8]) -> Result<T, IpcError>
where
    T: serde::de::DeserializeOwned,
{
    let mut reader = std::io::Cursor::new(bytes);
    let decoded =
        ciborium::from_reader(&mut reader).map_err(|err| IpcError::Decode(err.to_string()))?;
    if reader.position()
        != u64::try_from(bytes.len()).map_err(|error| {
            IpcError::Decode(format!("CBOR payload length does not fit u64: {error}"))
        })?
    {
        return Err(IpcError::Decode(
            "CBOR payload contains trailing bytes after its value".to_string(),
        ));
    }
    Ok(decoded)
}

#[expect(
    clippy::manual_unwrap_or,
    reason = "workspace bans Result::unwrap_or / map_or; explicit match is the only safe form."
)]
fn oversized_for(len: usize) -> IpcError {
    let reported = match u64::try_from(len) {
        Ok(value) => value,
        Err(_overflow) => u64::MAX,
    };
    IpcError::Oversized(reported)
}

fn encode_frame<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, IpcError> {
    let body = encode_cbor_payload(value)?;
    if body.len() > MAX_FRAME_BODY_BYTES {
        return Err(oversized_for(body.len()));
    }
    let body_len_u32 = u32::try_from(body.len())
        .map_err(|_overflow| IpcError::Encode("body length overflowed u32".into()))?;

    let total = body
        .len()
        .checked_add(FRAME_HEADER_BYTES)
        .ok_or_else(|| IpcError::Encode("frame length overflowed usize".into()))?;
    let mut frame = Vec::with_capacity(total);
    frame.extend_from_slice(&body_len_u32.to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

fn decode_frame<T, R, G>(
    reader: &mut R,
    allow_compressed_request: bool,
    preflight_request: bool,
    admit: impl FnOnce(usize) -> Result<G, IpcError>,
    mut reserve: impl FnMut(&mut G, usize) -> Result<(), IpcError>,
) -> Result<(T, G), IpcError>
where
    T: serde::de::DeserializeOwned,
    R: Read,
{
    let mut header = [0u8; FRAME_HEADER_BYTES];
    read_exact_or_truncated(reader, &mut header)?;
    let body_len_u32 = u32::from_le_bytes(header);
    if body_len_u32 == 0 {
        return Err(IpcError::EmptyFrame);
    }
    let body_len_u64 = u64::from(body_len_u32);
    let max_body_u64 = u64::try_from(MAX_FRAME_BODY_BYTES)
        .map_err(|_overflow| IpcError::Decode("MAX_FRAME_BODY_BYTES does not fit in u64".into()))?;
    if body_len_u64 > max_body_u64 {
        return Err(IpcError::Oversized(body_len_u64));
    }
    let body_len = usize::try_from(body_len_u32)
        .map_err(|_overflow| IpcError::Decode("declared body length overflowed usize".into()))?;

    let mut guard = admit(body_len)?;
    let mut body = vec![0u8; body_len];
    read_exact_or_truncated(reader, &mut body)?;
    if body.starts_with(MULTIFRAME_REQUEST_MAGIC) {
        if !allow_compressed_request {
            return Err(IpcError::Decode(
                "multiframe request framing is invalid for an IPC response".into(),
            ));
        }
        let decoded = decode_multiframe_request(reader, &body, |extra| reserve(&mut guard, extra))?;
        let value = decode_materialized(
            decoded.as_slice(),
            &mut guard,
            preflight_request,
            &mut reserve,
        )?;
        return Ok((value, guard));
    }
    let Some(compressed) = body.strip_prefix(COMPRESSED_REQUEST_MAGIC) else {
        let value =
            decode_materialized(body.as_slice(), &mut guard, preflight_request, &mut reserve)?;
        return Ok((value, guard));
    };
    if !allow_compressed_request {
        return Err(IpcError::Decode(
            "compressed request framing is invalid for an IPC response".into(),
        ));
    }
    if compressed.len() < COMPRESSED_REQUEST_METADATA_BYTES {
        return Err(IpcError::Decode(
            "compressed request header is truncated".into(),
        ));
    }
    let (length_bytes, rest) = compressed.split_at(8);
    let declared_len_u64 = u64::from_le_bytes(
        length_bytes
            .try_into()
            .map_err(|_error| IpcError::Decode("compressed request length is malformed".into()))?,
    );
    let declared_len = usize::try_from(declared_len_u64).map_err(|_overflow| {
        IpcError::Decode("compressed request length does not fit usize".into())
    })?;
    if declared_len > MAX_DECOMPRESSED_FRAME_BODY_BYTES {
        return Err(IpcError::Decode(format!(
            "compressed request expands to {declared_len} bytes, exceeding the {MAX_DECOMPRESSED_FRAME_BODY_BYTES} byte decoded-request cap"
        )));
    }
    reserve(&mut guard, declared_len)?;
    let (expected_digest, compressed_bytes) = rest.split_at(32);
    if compressed_bytes.is_empty() {
        return Err(IpcError::Decode(
            "compressed request payload is empty".into(),
        ));
    }
    let decoded = zstd::bulk::decompress(compressed_bytes, declared_len)
        .map_err(|err| IpcError::Decode(format!("request decompression failed: {err}")))?;
    if decoded.len() != declared_len {
        return Err(IpcError::Decode(format!(
            "compressed request declared {declared_len} decoded bytes but produced {}",
            decoded.len()
        )));
    }
    let actual_digest = sha2::Sha256::digest(&decoded);
    if actual_digest.as_slice() != expected_digest {
        return Err(IpcError::Decode(
            "compressed request SHA-256 does not match decoded body".into(),
        ));
    }
    let value = decode_materialized(
        decoded.as_slice(),
        &mut guard,
        preflight_request,
        &mut reserve,
    )?;
    Ok((value, guard))
}

fn decode_materialized<T: serde::de::DeserializeOwned, G>(
    bytes: &[u8],
    guard: &mut G,
    preflight_request: bool,
    reserve: &mut impl FnMut(&mut G, usize) -> Result<(), IpcError>,
) -> Result<T, IpcError> {
    if preflight_request {
        let text_storage = crate::cbor_preflight::retained_text_budget(bytes)?;
        reserve(guard, text_storage)?;
    }
    decode_cbor_payload(bytes)
}

fn decode_multiframe_request<R>(
    reader: &mut R,
    first: &[u8],
    reserve: impl FnOnce(usize) -> Result<(), IpcError>,
) -> Result<Vec<u8>, IpcError>
where
    R: Read,
{
    let metadata = first
        .strip_prefix(MULTIFRAME_REQUEST_MAGIC)
        .ok_or_else(|| IpcError::Decode("multiframe marker is missing".into()))?;
    if metadata.len() < MULTIFRAME_FIRST_MIN_BODY_BYTES {
        return Err(IpcError::Decode(
            "multiframe request header is truncated".into(),
        ));
    }
    let (length_bytes, rest) = metadata.split_at(8);
    let declared_len =
        usize::try_from(u64::from_le_bytes(length_bytes.try_into().map_err(
            |_error| IpcError::Decode("multiframe length is malformed".into()),
        )?))
        .map_err(|_overflow| IpcError::Decode("multiframe length does not fit usize".into()))?;
    if declared_len <= MAX_FRAME_BODY_BYTES || declared_len > MAX_MULTIFRAME_REQUEST_BODY_BYTES {
        return Err(IpcError::Decode(format!(
            "multiframe request length {declared_len} is outside ({MAX_FRAME_BODY_BYTES}, {MAX_MULTIFRAME_REQUEST_BODY_BYTES}]"
        )));
    }
    let (expected_digest, fragment) = rest.split_at(32);
    let expected_first_len =
        declared_len.min(MAX_FRAME_BODY_BYTES.saturating_sub(MULTIFRAME_FIRST_METADATA_BYTES));
    if fragment.len() != expected_first_len {
        return Err(IpcError::Decode(
            "multiframe first fragment has noncanonical length".into(),
        ));
    }
    // Account for the decoded body and one following frame while retaining
    // the first frame. Later frames are replaced, not retained together.
    let additional = declared_len
        .checked_add(MAX_FRAME_BODY_BYTES)
        .ok_or_else(|| IpcError::Decode("multiframe reservation overflow".into()))?;
    reserve(additional)?;
    // Admission reserves this capacity before allocation; reading fragments
    // into one buffer avoids transient reallocations of a 128 MiB body.
    let mut decoded = Vec::with_capacity(declared_len);
    decoded.extend_from_slice(fragment);
    let mut expected_sequence = 1_u32;
    while decoded.len() < declared_len {
        let mut header = [0u8; FRAME_HEADER_BYTES];
        read_exact_or_truncated(reader, &mut header)?;
        let frame_len = usize::try_from(u32::from_le_bytes(header))
            .map_err(|_overflow| IpcError::Decode("fragment length does not fit usize".into()))?;
        let remaining = declared_len.saturating_sub(decoded.len());
        let expected_len = MULTIFRAME_NEXT_METADATA_BYTES
            .checked_add(
                remaining.min(MAX_FRAME_BODY_BYTES.saturating_sub(MULTIFRAME_NEXT_METADATA_BYTES)),
            )
            .ok_or_else(|| IpcError::Decode("fragment length overflow".into()))?;
        if frame_len != expected_len {
            return Err(IpcError::Decode(format!(
                "multiframe fragment length {frame_len} != expected {expected_len}"
            )));
        }
        let mut frame = vec![0u8; frame_len];
        read_exact_or_truncated(reader, &mut frame)?;
        let Some(next) = frame.strip_prefix(MULTIFRAME_REQUEST_MAGIC) else {
            return Err(IpcError::Decode(
                "multiframe fragment marker mismatch".into(),
            ));
        };
        let (sequence_bytes, fragment) = next.split_at(4);
        let sequence = u32::from_le_bytes(
            sequence_bytes
                .try_into()
                .map_err(|_error| IpcError::Decode("fragment sequence is malformed".into()))?,
        );
        if sequence != expected_sequence {
            return Err(IpcError::Decode(format!(
                "multiframe fragment sequence {sequence} != expected {expected_sequence}"
            )));
        }
        if fragment.len() > remaining {
            return Err(IpcError::Decode(
                "multiframe fragment exceeds declared length".into(),
            ));
        }
        decoded.extend_from_slice(fragment);
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or_else(|| IpcError::Decode("fragment sequence overflow".into()))?;
    }
    let actual_digest = sha2::Sha256::digest(&decoded);
    if actual_digest.as_slice() != expected_digest {
        return Err(IpcError::Decode(
            "multiframe request SHA-256 does not match decoded body".into(),
        ));
    }
    Ok(decoded)
}

/// Fill `buf` from `reader`, returning [`IpcError::Truncated`] on EOF and
/// retrying on `Interrupted`.
fn read_exact_or_truncated<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<(), IpcError> {
    let mut filled = 0usize;
    while filled < buf.len() {
        let Some(slot) = buf.get_mut(filled..) else {
            return Err(IpcError::Io(std::io::Error::other(
                "internal: read buffer slice out of range",
            )));
        };
        match reader.read(slot) {
            Ok(0) => return Err(IpcError::Truncated),
            Ok(n) => {
                filled = filled
                    .checked_add(n)
                    .ok_or_else(|| IpcError::Decode("read counter overflow".into()))?;
            }
            Err(ref err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) => return Err(IpcError::Io(err)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{
        COMPRESSED_REQUEST_HEADER_BYTES, COMPRESSED_REQUEST_MAGIC, IpcError,
        MAX_DECOMPRESSED_FRAME_BODY_BYTES, MAX_FRAME_BODY_BYTES, MAX_MULTIFRAME_REQUEST_BODY_BYTES,
        MULTIFRAME_REQUEST_MAGIC, decode_cbor_payload, decode_request, decode_request_guarded,
        decode_response, encode_cbor_payload, encode_request,
    };

    #[test]
    fn cbor_payload_round_trip_preserves_tuple_value() {
        let result = (|| -> Result<(), IpcError> {
            let expected = (7_u32, "ranker".to_string(), vec![1_u8, 2, 3]);
            let encoded = encode_cbor_payload(&expected)?;
            let decoded: (u32, String, Vec<u8>) = decode_cbor_payload(encoded.as_slice())?;
            if decoded != expected {
                return Err(IpcError::Decode(format!(
                    "tuple payload round-trip drifted: decoded={decoded:?}"
                )));
            }
            Ok(())
        })();
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn invalid_cbor_payload_returns_typed_decode_error() {
        let result = decode_cbor_payload::<u32>(&[0xff]);
        assert!(matches!(
            result,
            Err(IpcError::Decode(ref message)) if !message.is_empty()
        ));
    }

    #[test]
    fn request_frame_rejects_a_second_cbor_value_after_the_envelope() {
        let mut bytes = encode_cbor_payload(&7_u32).expect("first value");
        bytes.extend(encode_cbor_payload(&8_u32).expect("second value"));
        let result = decode_cbor_payload::<u32>(&bytes);
        assert!(matches!(result, Err(IpcError::Decode(_))), "{result:?}");
        let mut frame = Vec::with_capacity(bytes.len() + 4);
        frame.extend_from_slice(
            &u32::try_from(bytes.len())
                .expect("small frame length")
                .to_le_bytes(),
        );
        frame.extend_from_slice(&bytes);
        let result = decode_request::<u32, _>(&mut Cursor::new(frame));
        assert!(matches!(result, Err(IpcError::Decode(_))), "{result:?}");
    }

    #[test]
    fn ingress_admission_precedes_frame_body_read() {
        let header = u32::try_from(MAX_FRAME_BODY_BYTES)
            .expect("frame cap fits u32")
            .to_le_bytes();
        let mut reader = Cursor::new(header);
        let result = decode_request_guarded::<u32, _, ()>(
            &mut reader,
            |_body_bytes| {
                Err(IpcError::IngressSaturated {
                    bytes: 256,
                    requests: 4,
                })
            },
            |(), _| Ok(()),
        );
        assert!(matches!(
            result,
            Err(IpcError::IngressSaturated {
                bytes: 256,
                requests: 4
            })
        ));
        assert_eq!(reader.position(), 4);
    }

    #[test]
    fn guarded_request_rejects_text_collection_growth_before_deserializing() {
        let frame = encode_request(&vec![String::new(); 1_000]).expect("bounded frame");
        let result = decode_request_guarded::<Vec<String>, _, usize>(
            &mut Cursor::new(frame),
            Ok,
            |held, additional| {
                let next = held
                    .checked_add(additional)
                    .expect("test budget fits usize");
                if next > 10_000 {
                    return Err(IpcError::IngressSaturated {
                        bytes: 10_000,
                        requests: 1,
                    });
                }
                *held = next;
                Ok(())
            },
        );
        assert!(matches!(result, Err(IpcError::IngressSaturated { .. })));
    }

    #[test]
    fn compressible_request_larger_than_wire_cap_round_trips_under_wire_cap() {
        let expected = "x".repeat(MAX_FRAME_BODY_BYTES + 1024);
        let frame = encode_request(&expected).expect("bounded compressed request");
        let header: [u8; 4] = frame
            .get(..4)
            .expect("length header")
            .try_into()
            .expect("length header width");
        let wire_len = usize::try_from(u32::from_le_bytes(header)).expect("wire length fits usize");
        assert!(wire_len <= MAX_FRAME_BODY_BYTES);
        assert!(
            frame
                .get(4..)
                .expect("body after length header")
                .starts_with(COMPRESSED_REQUEST_MAGIC)
        );
        let (actual, reserved): (String, usize) =
            decode_request_guarded(&mut Cursor::new(frame), Ok, |held, additional| {
                *held += additional;
                Ok(())
            })
            .expect("decoded request");
        assert_eq!(actual, expected);
        let logical_len = usize::try_from(super::cbor_payload_len(&expected).expect("CBOR length"))
            .expect("length fits usize");
        assert_eq!(
            reserved,
            wire_len + logical_len + 3 * std::mem::size_of::<String>()
        );
    }

    #[test]
    fn compressed_request_rejects_a_tampered_digest() {
        let expected = "y".repeat(MAX_FRAME_BODY_BYTES + 1024);
        let mut frame = encode_request(&expected).expect("bounded compressed request");
        let digest_start = 4 + COMPRESSED_REQUEST_MAGIC.len() + 8;
        let digest_byte = frame.get_mut(digest_start).expect("digest byte exists");
        *digest_byte ^= 1;
        let result: Result<String, IpcError> = decode_request(&mut Cursor::new(frame));
        assert!(matches!(result, Err(IpcError::Decode(message)) if message.contains("SHA-256")));
    }

    #[test]
    fn compressed_request_framing_is_not_accepted_for_responses() {
        let expected = "z".repeat(MAX_FRAME_BODY_BYTES + 1024);
        let frame = encode_request(&expected).expect("bounded compressed request");
        let result: Result<String, IpcError> = decode_response(&mut Cursor::new(frame));
        assert!(matches!(
            result,
            Err(IpcError::Decode(message)) if message.contains("invalid for an IPC response")
        ));
    }

    #[test]
    fn incompressible_request_larger_than_wire_cap_uses_bounded_fragments() {
        let mut state = 0x9e37_79b9_u32;
        let expected: Vec<u8> = (0..MAX_FRAME_BODY_BYTES + 1024)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                u8::try_from(state & 0xff).expect("masked value fits u8")
            })
            .collect();
        let frame = encode_request(&expected).expect("multiframe request");
        assert!(
            frame
                .get(4..)
                .expect("first body")
                .starts_with(MULTIFRAME_REQUEST_MAGIC)
        );
        let first_frame_len = usize::try_from(u32::from_le_bytes(
            frame
                .get(..4)
                .expect("first header")
                .try_into()
                .expect("header width"),
        ))
        .expect("first length fits");
        let (actual, reserved): (Vec<u8>, usize) = decode_request_guarded(
            &mut Cursor::new(frame.as_slice()),
            Ok,
            |held, additional| {
                *held += additional;
                Ok(())
            },
        )
        .expect("decoded request");
        assert_eq!(actual, expected);
        let logical_len = usize::try_from(super::cbor_payload_len(&expected).expect("CBOR length"))
            .expect("length fits usize");
        assert_eq!(
            reserved,
            first_frame_len + logical_len + MAX_FRAME_BODY_BYTES
        );

        let mut truncated = frame.clone();
        let _last = truncated.pop();
        let result: Result<Vec<u8>, IpcError> = decode_request(&mut Cursor::new(truncated));
        assert!(matches!(result, Err(IpcError::Truncated)));

        let sequence_offset = 4_usize
            .checked_add(first_frame_len)
            .and_then(|value| value.checked_add(4 + MULTIFRAME_REQUEST_MAGIC.len()))
            .expect("second frame sequence offset fits");
        let mut reordered = frame;
        *reordered.get_mut(sequence_offset).expect("sequence byte") = 2;
        let result: Result<Vec<u8>, IpcError> = decode_request(&mut Cursor::new(reordered));
        assert!(matches!(result, Err(IpcError::Decode(message)) if message.contains("sequence")));
    }

    #[test]
    fn multiframe_request_above_64_mib_round_trips_and_detects_corruption() {
        let expected = "x".repeat(MAX_DECOMPRESSED_FRAME_BODY_BYTES + 1);
        let mut frame = encode_request(&expected).expect("bounded multiframe request");
        assert!(
            frame
                .get(4..)
                .expect("first body")
                .starts_with(MULTIFRAME_REQUEST_MAGIC)
        );
        let actual: String =
            decode_request(&mut Cursor::new(frame.as_slice())).expect("decoded request");
        assert_eq!(actual, expected);
        let last = frame.last_mut().expect("nonempty frame");
        *last ^= 1;
        let result: Result<String, IpcError> = decode_request(&mut Cursor::new(frame));
        assert!(matches!(result, Err(IpcError::Decode(message)) if message.contains("SHA-256")));
    }

    #[test]
    fn multiframe_request_rejects_oversized_declaration_before_allocating() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&u32::try_from(49).expect("header fits").to_le_bytes());
        frame.extend_from_slice(MULTIFRAME_REQUEST_MAGIC);
        frame.extend_from_slice(
            &u64::try_from(MAX_MULTIFRAME_REQUEST_BODY_BYTES + 1)
                .expect("limit fits")
                .to_le_bytes(),
        );
        frame.extend_from_slice(&[0; 32]);
        frame.push(0);
        let result: Result<Vec<u8>, IpcError> = decode_request(&mut Cursor::new(frame));
        assert!(matches!(result, Err(IpcError::Decode(message)) if message.contains("outside")));
    }

    #[test]
    fn compressed_request_rejects_declared_decoded_size_above_cap() {
        let mut body = Vec::from(COMPRESSED_REQUEST_MAGIC.as_slice());
        body.extend_from_slice(
            &u64::try_from(MAX_DECOMPRESSED_FRAME_BODY_BYTES + 1)
                .expect("length fits u64")
                .to_le_bytes(),
        );
        body.extend_from_slice(&[0; 32]);
        body.push(0);
        assert_eq!(body.len(), COMPRESSED_REQUEST_HEADER_BYTES + 1);
        let mut frame = Vec::new();
        frame.extend_from_slice(
            &u32::try_from(body.len())
                .expect("test frame fits u32")
                .to_le_bytes(),
        );
        frame.extend_from_slice(&body);
        let result: Result<Vec<u8>, IpcError> = decode_request(&mut Cursor::new(frame));
        assert!(
            matches!(result, Err(IpcError::Decode(message)) if message.contains("decoded-request cap"))
        );
    }
}
