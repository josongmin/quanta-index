//! Length-prefixed CBOR frame codec for search-plane IPC envelopes.

use std::io::{ErrorKind, Read};

use quanta_index_contract::{SearchPlaneIpcRequestEnvelope, SearchPlaneIpcResponseEnvelope};

/// Maximum CBOR body size accepted on the wire.
pub const MAX_FRAME_BODY_BYTES: usize = 16 * 1024 * 1024;

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

pub fn encode_request(envelope: &SearchPlaneIpcRequestEnvelope) -> Result<Vec<u8>, IpcError> {
    encode_frame(envelope)
}

pub fn encode_response(envelope: &SearchPlaneIpcResponseEnvelope) -> Result<Vec<u8>, IpcError> {
    encode_frame(envelope)
}

pub fn decode_request<R: Read>(reader: &mut R) -> Result<SearchPlaneIpcRequestEnvelope, IpcError> {
    decode_frame(reader)
}

pub fn decode_response<R: Read>(
    reader: &mut R,
) -> Result<SearchPlaneIpcResponseEnvelope, IpcError> {
    decode_frame(reader)
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
    let mut body: Vec<u8> = Vec::new();
    ciborium::into_writer(value, &mut body).map_err(|err| IpcError::Encode(err.to_string()))?;
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
    let max_body_u64 = u64::try_from(MAX_FRAME_BODY_BYTES)
        .map_err(|_overflow| IpcError::Decode("MAX_FRAME_BODY_BYTES does not fit in u64".into()))?;
    if body_len_u64 > max_body_u64 {
        return Err(IpcError::Oversized(body_len_u64));
    }
    let body_len = usize::try_from(body_len_u32)
        .map_err(|_overflow| IpcError::Decode("declared body length overflowed usize".into()))?;

    let mut body = vec![0u8; body_len];
    read_exact_or_truncated(reader, &mut body)?;
    let decoded: T =
        ciborium::from_reader(body.as_slice()).map_err(|err| IpcError::Decode(err.to_string()))?;
    Ok(decoded)
}

/// Fill `buf` from `reader`, returning [`IpcError::Truncated`] on EOF and
/// retrying on `Interrupted`.
pub(crate) fn read_exact_or_truncated<R: Read>(
    reader: &mut R,
    buf: &mut [u8],
) -> Result<(), IpcError> {
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
