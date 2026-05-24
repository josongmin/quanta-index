//! Search-plane IPC driving adapter.
//!
//! Maps frozen `quanta_index_contract::ipc` envelopes to wire bytes and back
//! using a length-prefixed CBOR frame.
//!
//! Query execution remains in `quanta-index-searchd` composition root; this
//! crate exposes only the codec.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

// Driving adapter keeps `quanta-index-core` in its dep set so future
// `CoreError` plumbing can land without re-shuffling the manifest. The
// `use _` rebind keeps `cargo machete` honest.
use quanta_index_core as _;

use std::io::{ErrorKind, Read};

use quanta_index_contract::{SearchPlaneIpcRequestEnvelope, SearchPlaneIpcResponseEnvelope};

/// Maximum CBOR body size accepted on the wire.
///
/// Frames whose declared body length exceeds this constant are rejected
/// fail-closed before any body bytes are read.
pub const MAX_FRAME_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Width of the length-prefix header in bytes.
const FRAME_HEADER_BYTES: usize = 4;

/// Codec errors surfaced by encode and decode entry points.
///
/// `Truncated` covers both a short header and a short body (i.e. EOF before
/// the declared body length is satisfied). It is distinct from `Io`, which
/// reports underlying transport failures other than short read.
#[derive(Debug)]
pub enum IpcError {
    /// Reader returned fewer bytes than required to complete the frame.
    Truncated,
    /// Declared body length exceeds [`MAX_FRAME_BODY_BYTES`]; carries the
    /// rejected length so callers can log the offending value.
    Oversized(u64),
    /// Declared body length is zero, which is never a valid envelope.
    EmptyFrame,
    /// CBOR encoder returned an error.
    Encode(String),
    /// CBOR decoder returned an error, including unknown variants or missing
    /// fields surfaced by the contract crate's manual `Deserialize` impls.
    Decode(String),
    /// Underlying transport returned an I/O error other than short read.
    Io(std::io::Error),
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
            | Self::Decode(_) => None,
        }
    }
}

/// Encode a request envelope into a length-prefixed CBOR frame.
///
/// The encoded frame is `[u32 little-endian body length][CBOR body]`. The
/// length field counts body bytes only. Returns [`IpcError::Oversized`] if
/// the serialized body exceeds [`MAX_FRAME_BODY_BYTES`].
pub fn encode_request(envelope: &SearchPlaneIpcRequestEnvelope) -> Result<Vec<u8>, IpcError> {
    encode_frame(envelope)
}

/// Encode a response envelope into a length-prefixed CBOR frame.
///
/// Frame layout matches [`encode_request`].
pub fn encode_response(envelope: &SearchPlaneIpcResponseEnvelope) -> Result<Vec<u8>, IpcError> {
    encode_frame(envelope)
}

/// Decode a single request envelope from a length-prefixed CBOR frame.
///
/// Reads exactly one frame from `reader`. Oversize headers are rejected
/// before any body bytes are consumed, so the reader cursor is left at the
/// end of the 4-byte header in that case.
pub fn decode_request<R: Read>(reader: &mut R) -> Result<SearchPlaneIpcRequestEnvelope, IpcError> {
    decode_frame(reader)
}

/// Decode a single response envelope from a length-prefixed CBOR frame.
///
/// Behaviour matches [`decode_request`].
pub fn decode_response<R: Read>(
    reader: &mut R,
) -> Result<SearchPlaneIpcResponseEnvelope, IpcError> {
    decode_frame(reader)
}

/// Build an [`IpcError::Oversized`] payload from a `usize` body length.
///
/// We avoid `Result::unwrap_or` and `as` casts: a saturating `match` keeps
/// the clippy `manual_unwrap_or` lint happy while still respecting the
/// workspace ban on `unwrap_or*`.
#[expect(
    clippy::manual_unwrap_or,
    reason = "workspace bans Result::unwrap_or / map_or; an explicit match is the only safe form here."
)]
fn oversized_for(len: usize) -> IpcError {
    let reported = match u64::try_from(len) {
        Ok(value) => value,
        Err(_overflow_source) => u64::MAX,
    };
    IpcError::Oversized(reported)
}

fn encode_frame<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, IpcError> {
    let mut body: Vec<u8> = Vec::new();
    ciborium::into_writer(value, &mut body).map_err(|err| IpcError::Encode(err.to_string()))?;
    if body.len() > MAX_FRAME_BODY_BYTES {
        return Err(oversized_for(body.len()));
    }
    let body_len_u32 = u32::try_from(body.len())
        .map_err(|_overflow_source| IpcError::Encode("body length overflowed u32".into()))?;

    let total = body
        .len()
        .checked_add(FRAME_HEADER_BYTES)
        .ok_or_else(|| IpcError::Encode("frame length overflowed usize".into()))?;
    let mut frame = Vec::with_capacity(total);
    frame.extend_from_slice(&body_len_u32.to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

fn decode_frame<T, R>(reader: &mut R) -> Result<T, IpcError>
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
    let max_body_u64 = u64::try_from(MAX_FRAME_BODY_BYTES).map_err(|_overflow_source| {
        IpcError::Decode("MAX_FRAME_BODY_BYTES does not fit in u64".into())
    })?;
    if body_len_u64 > max_body_u64 {
        // Spec: oversized → fail-closed BEFORE reading the body.
        return Err(IpcError::Oversized(body_len_u64));
    }
    let body_len = usize::try_from(body_len_u32).map_err(|_overflow_source| {
        IpcError::Decode("declared body length overflowed usize".into())
    })?;

    let mut body = vec![0u8; body_len];
    read_exact_or_truncated(reader, &mut body)?;
    let decoded: T =
        ciborium::from_reader(body.as_slice()).map_err(|err| IpcError::Decode(err.to_string()))?;
    Ok(decoded)
}

/// Fill `buf` from `reader`, returning [`IpcError::Truncated`] on EOF.
///
/// We hand-roll the loop instead of using `Read::read_exact` because we want
/// the short-read case to surface as `Truncated`, while only non-EOF I/O
/// failures should bubble out as [`IpcError::Io`]. `read_exact` collapses
/// both into a single `UnexpectedEof` and would force us to inspect the
/// `ErrorKind` after the fact; doing it directly is clearer and avoids the
/// ambiguous `Interrupted` retry semantics of `read_exact`.
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
            Err(ref err) if err.kind() == ErrorKind::Interrupted => {
                // Retry: spurious interruption is not a transport failure.
            }
            Err(err) => return Err(IpcError::Io(err)),
        }
    }
    Ok(())
}
