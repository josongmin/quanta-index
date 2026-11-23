//! Length-prefixed CBOR frame codec for search-plane IPC envelopes.

use quanta_index_core::{
    RequestBudgetV1,
    index::{MemoryLease, MemoryLifetime, MemoryScope},
};
use std::io::{ErrorKind, Read, Write};

use sha2::Digest;

use crate::error::{IpcError, MAX_FRAME_BODY_BYTES};

/// Maximum decoded body after bounded IPC request compression.
const MAX_DECOMPRESSED_FRAME_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Maximum complete CBOR request admitted across multiple bounded frames.
///
/// The ingest resource policy still independently bounds semantic text,
/// source bytes, records, and vector residency before publication.
pub const MAX_MULTIFRAME_REQUEST_BODY_BYTES: usize = 128 * 1024 * 1024;

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

/// Transport-only diagnostic capability exchange. It precedes a request on
/// the same stream and is never a CBOR request frame.
pub(crate) const DIAGNOSTIC_PREFACE_MAGIC: [u8; 8] = *b"QIPCDG01";
pub(crate) const DIAGNOSTIC_PREFACE_BYTES: usize = 24;
pub(crate) const DIAGNOSTIC_ACK_BYTES: usize = 9;

pub(crate) fn diagnostic_preface(capture_id: [u8; 16]) -> [u8; DIAGNOSTIC_PREFACE_BYTES] {
    let mut bytes = [0; DIAGNOSTIC_PREFACE_BYTES];
    bytes[..8].copy_from_slice(&DIAGNOSTIC_PREFACE_MAGIC);
    bytes[8..].copy_from_slice(&capture_id);
    bytes
}

pub(crate) fn diagnostic_ack(accepted: bool) -> [u8; DIAGNOSTIC_ACK_BYTES] {
    let mut bytes = [0; DIAGNOSTIC_ACK_BYTES];
    bytes[..8].copy_from_slice(&DIAGNOSTIC_PREFACE_MAGIC);
    bytes[8] = u8::from(accepted);
    bytes
}

pub(crate) fn diagnostic_ack_accepted(bytes: &[u8; DIAGNOSTIC_ACK_BYTES]) -> Option<bool> {
    if bytes[..8] != DIAGNOSTIC_PREFACE_MAGIC || bytes[8] > 1 {
        return None;
    }
    Some(bytes[8] == 1)
}

/// Width of the length-prefix header in bytes.
const FRAME_HEADER_BYTES: usize = 4;

/// The decoded-body decision for a request before any wire framing choice.
/// Both compressed and multiframe requests have the same 128 MiB logical cap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestSizeDecisionV1 {
    Fits { decoded_bytes: u64 },
    ExceedsDecodedCap { decoded_bytes: u64, cap_bytes: u64 },
}

/// Count a request without allocating its CBOR body.
///
/// Callers may omit an optional diagnostic token only when this typed limit decision changes from
/// `ExceedsDecodedCap` with the token to `Fits` without it.
pub fn request_size_decision_v1<T: serde::Serialize>(
    envelope: &T,
) -> Result<RequestSizeDecisionV1, IpcError> {
    let decoded_bytes = cbor_payload_len(envelope)?;
    let cap_bytes = u64::try_from(MAX_MULTIFRAME_REQUEST_BODY_BYTES)
        .map_err(|_overflow| IpcError::Encode("request cap does not fit u64".into()))?;
    Ok(if decoded_bytes > cap_bytes {
        RequestSizeDecisionV1::ExceedsDecodedCap {
            decoded_bytes,
            cap_bytes,
        }
    } else {
        RequestSizeDecisionV1::Fits { decoded_bytes }
    })
}

pub fn encode_request<T: serde::Serialize>(envelope: &T) -> Result<Vec<u8>, IpcError> {
    let logical_len = usize::try_from(cbor_payload_len(envelope)?)
        .map_err(|_overflow| IpcError::Encode("request body length overflowed usize".into()))?;
    if logical_len <= MAX_FRAME_BODY_BYTES {
        let count = u64::try_from(logical_len)
            .map_err(|_error| IpcError::Encode("request length overflow".into()))?;
        return encode_counted_frame(envelope, None, count, |_bytes| Ok(None))
            .map(|frame| frame.bytes);
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
    encode_multiframe_request(body)
}

/// Admit one single-frame client request before allocating its encoded body.
///
/// The returned frame retains the original memory charge through the write.
/// Larger requests require a separately admitted multiframe implementation;
/// this API refuses instead of silently taking the standalone allocation path.
pub fn encode_request_single_frame_admitted<T: serde::Serialize>(
    envelope: &T,
    budget: &RequestBudgetV1,
    memory: &MemoryScope,
) -> Result<AdmittedClientRequestFrame, IpcError> {
    encode_owned_frame(envelope, Some(budget), |bytes| {
        memory
            .reserve(bytes, MemoryLifetime::Retained)
            .map(Some)
            .map_err(|error| IpcError::from_client_memory_error(bytes, error))
    })
    .map(AdmittedClientRequestFrame)
}

/// Use the existing fragment wire when an admitted request exceeds one frame.
/// Encode the counted CBOR body into the admitted wire allocation, then insert
/// fragment headers in place before any connection is opened.
pub(crate) fn encode_request_multiframe_admitted_v1<T: serde::Serialize>(
    envelope: &T,
    budget: &RequestBudgetV1,
    memory: &MemoryScope,
) -> Result<AdmittedClientRequestFrame, IpcError> {
    let bytes = count_frame_body(envelope, Some(budget))?;
    let length = usize::try_from(bytes).map_err(|_| IpcError::Oversized(bytes))?;
    let retain = |bytes| {
        memory
            .reserve(bytes, MemoryLifetime::Retained)
            .map(Some)
            .map_err(|error| IpcError::from_client_memory_error(bytes, error))
    };
    if length <= MAX_FRAME_BODY_BYTES {
        return encode_counted_frame(envelope, Some(budget), bytes, retain)
            .map(AdmittedClientRequestFrame);
    }
    if length > MAX_MULTIFRAME_REQUEST_BODY_BYTES {
        return Err(IpcError::Oversized(bytes));
    }
    let layout = MultiframeLayout::new(length)?;
    let wire_bytes = u64::try_from(layout.wire_bytes)
        .map_err(|_| IpcError::Encode("fragmented request wire bytes exceed u64".into()))?;
    let wire = allocate_frame(layout.wire_bytes, retain(wire_bytes)?, Some(budget))?;
    let wire = encode_frame_body(envelope, wire, length, Some(budget))?;
    frame_multiframe_request(wire, layout, Some(budget)).map(AdmittedClientRequestFrame)
}

/// An encoded client request whose wire bytes and charge share one lifetime.
#[derive(Debug)]
pub struct AdmittedClientRequestFrame(ResponseFrame);

impl std::ops::Deref for AdmittedClientRequestFrame {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

const MULTIFRAME_FIRST_HEADER_BYTES: usize = FRAME_HEADER_BYTES + MULTIFRAME_FIRST_METADATA_BYTES;
const MULTIFRAME_NEXT_HEADER_BYTES: usize = FRAME_HEADER_BYTES + MULTIFRAME_NEXT_METADATA_BYTES;
const MULTIFRAME_NEXT_CAPACITY: usize = MAX_FRAME_BODY_BYTES - MULTIFRAME_NEXT_METADATA_BYTES;

struct MultiframeLayout {
    body_bytes: usize,
    first_fragment_bytes: usize,
    next_frames: usize,
    wire_bytes: usize,
}

impl MultiframeLayout {
    fn new(body_bytes: usize) -> Result<Self, IpcError> {
        let first_fragment_bytes =
            body_bytes.min(MAX_FRAME_BODY_BYTES - MULTIFRAME_FIRST_METADATA_BYTES);
        let next_frames = (body_bytes - first_fragment_bytes).div_ceil(MULTIFRAME_NEXT_CAPACITY);
        let wire_bytes = next_frames
            .checked_mul(MULTIFRAME_NEXT_HEADER_BYTES)
            .and_then(|bytes| bytes.checked_add(MULTIFRAME_FIRST_HEADER_BYTES))
            .and_then(|bytes| bytes.checked_add(body_bytes))
            .ok_or_else(|| IpcError::Encode("fragmented request wire length overflow".into()))?;
        Ok(Self {
            body_bytes,
            first_fragment_bytes,
            next_frames,
            wire_bytes,
        })
    }
}

fn encode_multiframe_request(mut body: Vec<u8>) -> Result<Vec<u8>, IpcError> {
    let layout = MultiframeLayout::new(body.len())?;
    body.try_reserve_exact(layout.wire_bytes - layout.body_bytes)
        .map_err(|_| IpcError::Encode("fragmented request framing allocation refused".into()))?;
    let frame = ResponseFrame {
        bytes: body,
        _memory: None,
    };
    frame_multiframe_request(frame, layout, None).map(|frame| frame.bytes)
}

fn frame_multiframe_request(
    mut frame: ResponseFrame,
    layout: MultiframeLayout,
    budget: Option<&RequestBudgetV1>,
) -> Result<ResponseFrame, IpcError> {
    if frame.bytes.len() != layout.body_bytes || frame.bytes.capacity() < layout.wire_bytes {
        return Err(IpcError::Encode(
            "fragmented request lost counted backing".into(),
        ));
    }
    let mut digest = sha2::Sha256::new();
    for chunk in frame.bytes.chunks(MAX_FRAME_BODY_BYTES) {
        response_checkpoint(budget).map_err(IpcError::Io)?;
        digest.update(chunk);
    }
    let digest = digest.finalize();
    let wire = &mut frame.bytes;
    wire.resize(layout.wire_bytes, 0);

    // Move fragments from the end backwards so a header or moved fragment
    // never overwrites the unread prefix of the original CBOR body.
    for index in (0..layout.next_frames).rev() {
        response_checkpoint(budget).map_err(IpcError::Io)?;
        let body_start = layout.first_fragment_bytes + index * MULTIFRAME_NEXT_CAPACITY;
        let body_end = layout.body_bytes.min(body_start + MULTIFRAME_NEXT_CAPACITY);
        let header_start =
            body_start + MULTIFRAME_FIRST_HEADER_BYTES + index * MULTIFRAME_NEXT_HEADER_BYTES;
        let fragment_start = header_start + MULTIFRAME_NEXT_HEADER_BYTES;
        wire.copy_within(body_start..body_end, fragment_start);

        let fragment_len = u32::try_from(MULTIFRAME_NEXT_METADATA_BYTES + body_end - body_start)
            .map_err(|_| IpcError::Encode("fragment body length exceeds u32".into()))?;
        let sequence = u32::try_from(index + 1)
            .map_err(|_| IpcError::Encode("fragment sequence exceeds u32".into()))?;
        let header = &mut wire[header_start..fragment_start];
        let (length, metadata) = header.split_at_mut(FRAME_HEADER_BYTES);
        let (magic, sequence_bytes) = metadata.split_at_mut(MULTIFRAME_REQUEST_MAGIC.len());
        length.copy_from_slice(&fragment_len.to_le_bytes());
        magic.copy_from_slice(MULTIFRAME_REQUEST_MAGIC);
        sequence_bytes.copy_from_slice(&sequence.to_le_bytes());
    }

    response_checkpoint(budget).map_err(IpcError::Io)?;
    wire.copy_within(
        0..layout.first_fragment_bytes,
        MULTIFRAME_FIRST_HEADER_BYTES,
    );
    let first_len = u32::try_from(MULTIFRAME_FIRST_METADATA_BYTES + layout.first_fragment_bytes)
        .map_err(|_| IpcError::Encode("first fragment body length exceeds u32".into()))?;
    let body_len = u64::try_from(layout.body_bytes)
        .map_err(|_| IpcError::Encode("fragmented request body length exceeds u64".into()))?;
    let header = &mut wire[..MULTIFRAME_FIRST_HEADER_BYTES];
    let (length, metadata) = header.split_at_mut(FRAME_HEADER_BYTES);
    let (magic, metadata) = metadata.split_at_mut(MULTIFRAME_REQUEST_MAGIC.len());
    let (body_length, digest_bytes) = metadata.split_at_mut(std::mem::size_of::<u64>());
    length.copy_from_slice(&first_len.to_le_bytes());
    magic.copy_from_slice(MULTIFRAME_REQUEST_MAGIC);
    body_length.copy_from_slice(&body_len.to_le_bytes());
    digest_bytes.copy_from_slice(&digest);
    response_checkpoint(budget).map_err(IpcError::Io)?;
    Ok(frame)
}

pub fn encode_response<T: serde::Serialize>(envelope: &T) -> Result<Vec<u8>, IpcError> {
    encode_frame(envelope)
}

/// Immutable encoded response data; backing dies before its retained charge.
/// This owner contains no execution or request slot and cannot be deep-cloned.
#[derive(Debug)]
pub struct ResponseFrame {
    bytes: Vec<u8>,
    _memory: Option<MemoryLease>,
}
impl std::ops::Deref for ResponseFrame {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

/// Count under the original budget, admit before allocation, and encode into
/// one bounded frame. The callback names the composition's data-only ledger.
pub fn encode_response_admitted<T: serde::Serialize>(
    value: &T,
    budget: &RequestBudgetV1,
    admit: impl FnOnce(u64) -> Result<Option<MemoryLease>, IpcError>,
) -> Result<ResponseFrame, IpcError> {
    encode_owned_frame(value, Some(budget), admit)
}

fn encode_error(error: ciborium::ser::Error<std::io::Error>) -> IpcError {
    match error {
        ciborium::ser::Error::Io(error) => IpcError::Io(error),
        ciborium::ser::Error::Value(message) => IpcError::Encode(message),
    }
}

fn response_checkpoint(budget: Option<&RequestBudgetV1>) -> std::io::Result<()> {
    if let Some(budget) = budget {
        budget.checkpoint("ipc_response_encode").map_err(|error| {
            let kind = if budget.deadline() <= std::time::Instant::now() {
                ErrorKind::TimedOut
            } else {
                // std::Write::write_all retries Interrupted. Permanent budget
                // cancellation must escape the CBOR adapter immediately.
                ErrorKind::Other
            };
            std::io::Error::new(kind, error)
        })?;
    }
    Ok(())
}

struct ResponseLength<'a> {
    bytes: u64,
    budget: Option<&'a RequestBudgetV1>,
}
impl Write for ResponseLength<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        response_checkpoint(self.budget)?;
        self.bytes = self
            .bytes
            .checked_add(u64::try_from(bytes.len()).map_err(std::io::Error::other)?)
            .ok_or_else(|| std::io::Error::other("encoded response length overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        response_checkpoint(self.budget)
    }
}

struct ResponseWriter<'a> {
    frame: ResponseFrame,
    maximum: usize,
    budget: Option<&'a RequestBudgetV1>,
}
impl Write for ResponseWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        response_checkpoint(self.budget)?;
        let next = self
            .frame
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|next| *next <= self.maximum)
            .ok_or_else(|| {
                std::io::Error::new(
                    ErrorKind::InvalidData,
                    "response serialization exceeds counted capacity",
                )
            })?;
        if next > self.frame.bytes.capacity() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "response backing lost admitted capacity",
            ));
        }
        self.frame.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        response_checkpoint(self.budget)
    }
}

fn encode_owned_frame<T: serde::Serialize>(
    value: &T,
    budget: Option<&RequestBudgetV1>,
    admit: impl FnOnce(u64) -> Result<Option<MemoryLease>, IpcError>,
) -> Result<ResponseFrame, IpcError> {
    let count = count_frame_body(value, budget)?;
    encode_counted_frame(value, budget, count, admit)
}

fn count_frame_body<T: serde::Serialize>(
    value: &T,
    budget: Option<&RequestBudgetV1>,
) -> Result<u64, IpcError> {
    response_checkpoint(budget).map_err(IpcError::Io)?;
    let mut count = ResponseLength { bytes: 0, budget };
    ciborium::into_writer(value, &mut count).map_err(encode_error)?;
    response_checkpoint(budget).map_err(IpcError::Io)?;
    Ok(count.bytes)
}

fn encode_counted_frame<T: serde::Serialize>(
    value: &T,
    budget: Option<&RequestBudgetV1>,
    counted_bytes: u64,
    admit: impl FnOnce(u64) -> Result<Option<MemoryLease>, IpcError>,
) -> Result<ResponseFrame, IpcError> {
    response_checkpoint(budget).map_err(IpcError::Io)?;
    if counted_bytes == 0 {
        return Err(IpcError::EmptyFrame);
    }
    if counted_bytes
        > u64::try_from(MAX_FRAME_BODY_BYTES)
            .map_err(|_error| IpcError::Encode("frame cap overflow".into()))?
    {
        return Err(IpcError::Oversized(counted_bytes));
    }
    let body_len = usize::try_from(counted_bytes)
        .map_err(|_error| IpcError::Encode("frame body overflow".into()))?;
    let total = body_len
        .checked_add(FRAME_HEADER_BYTES)
        .ok_or_else(|| IpcError::Encode("frame length overflow".into()))?;
    let capacity_bytes = u64::try_from(total)
        .map_err(|_error| IpcError::Encode("frame capacity overflow".into()))?;
    let mut frame = allocate_frame(total, admit(capacity_bytes)?, budget)?;
    let body_u32 = u32::try_from(body_len)
        .map_err(|_error| IpcError::Encode("body length overflowed u32".into()))?;
    frame.bytes.extend_from_slice(&body_u32.to_le_bytes());
    encode_frame_body(value, frame, total, budget)
}

fn allocate_frame(
    total: usize,
    memory: Option<MemoryLease>,
    budget: Option<&RequestBudgetV1>,
) -> Result<ResponseFrame, IpcError> {
    response_checkpoint(budget).map_err(IpcError::Io)?;
    let capacity_bytes = u64::try_from(total)
        .map_err(|_error| IpcError::Encode("frame capacity overflow".into()))?;
    let mut frame = ResponseFrame {
        bytes: Vec::new(),
        _memory: memory,
    };
    frame
        .bytes
        .try_reserve_exact(total)
        .map_err(|_error| IpcError::FrameMemoryLimited {
            bytes: capacity_bytes,
        })?;
    Ok(frame)
}

fn encode_frame_body<T: serde::Serialize>(
    value: &T,
    frame: ResponseFrame,
    total: usize,
    budget: Option<&RequestBudgetV1>,
) -> Result<ResponseFrame, IpcError> {
    let mut writer = ResponseWriter {
        frame,
        maximum: total,
        budget,
    };
    ciborium::into_writer(value, &mut writer).map_err(encode_error)?;
    response_checkpoint(budget).map_err(IpcError::Io)?;
    if writer.frame.bytes.len() != total {
        return Err(IpcError::Encode(
            "response serialization differs from counted capacity".into(),
        ));
    }
    Ok(writer.frame)
}

pub fn encode_cbor_payload<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, IpcError> {
    let mut body: Vec<u8> = Vec::new();
    ciborium::into_writer(value, &mut body).map_err(|err| IpcError::Encode(err.to_string()))?;
    Ok(body)
}

/// Compare the exact payload encoding without retaining another encoded body.
///
/// This still visits the complete value, including on a
/// mismatch, so serialization failures cannot become a successful comparison.
pub fn cbor_payload_matches<T: serde::Serialize>(
    value: &T,
    expected: &[u8],
) -> Result<bool, IpcError> {
    match cbor_payload_matches_with_progress(value, expected, &mut || {
        Ok::<(), std::convert::Infallible>(())
    }) {
        Ok(equal) => Ok(equal),
        Err(CborPayloadMatchProgressError::Encode(error)) => Err(error),
        Err(CborPayloadMatchProgressError::Progress(never)) => match never {},
    }
}

/// A serialization failure or the unchanged failure from the execution owner.
#[derive(Debug)]
pub enum CborPayloadMatchProgressError<E> {
    Encode(IpcError),
    Progress(E),
}

impl<E: std::fmt::Display> std::fmt::Display for CborPayloadMatchProgressError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(error) => write!(
                formatter,
                "CBOR payload comparison encoding failed: {error}"
            ),
            Self::Progress(error) => write!(
                formatter,
                "CBOR payload comparison execution interrupted: {error}"
            ),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for CborPayloadMatchProgressError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(error) => Some(error),
            Self::Progress(error) => Some(error),
        }
    }
}

/// Compare canonical bytes with cooperative execution checkpoints.
///
/// Checkpoints bound comparison work to 64KiB or 256 encoder writes. Custom
/// serializers must cooperate separately for computation before emitting bytes.
pub fn cbor_payload_matches_with_progress<T, E, F>(
    value: &T,
    expected: &[u8],
    progress: &mut F,
) -> Result<bool, CborPayloadMatchProgressError<E>>
where
    T: serde::Serialize,
    F: FnMut() -> Result<(), E>,
{
    cbor_payload_matches_admitted_v1(value, expected, progress, &mut |_| Ok(()))
}

/// Compare the existing IPC wire while charging each serializer write and byte
/// before comparison. No second encoded body or sorted-map representation is born.
pub fn cbor_payload_matches_with_work_v1<T, E, F>(
    value: &T,
    expected: &[u8],
    work: &mut F,
) -> Result<bool, CborPayloadMatchProgressError<E>>
where
    T: serde::Serialize,
    F: FnMut(u64) -> Result<(), E>,
{
    cbor_payload_matches_admitted_v1(value, expected, &mut || Ok(()), work)
}

fn cbor_payload_matches_admitted_v1<T, E, F, W>(
    value: &T,
    expected: &[u8],
    progress: &mut F,
    work: &mut W,
) -> Result<bool, CborPayloadMatchProgressError<E>>
where
    T: serde::Serialize,
    F: FnMut() -> Result<(), E>,
    W: FnMut(u64) -> Result<(), E>,
{
    const CHECKPOINT_BYTES: usize = 64 * 1024;
    const CHECKPOINT_WRITES: usize = 256;
    struct Compare<'a, E, F, W> {
        expected: &'a [u8],
        position: usize,
        equal: bool,
        bytes_since_checkpoint: usize,
        writes_since_checkpoint: usize,
        progress: &'a mut F,
        work: &'a mut W,
        interrupted: Option<E>,
    }
    impl<E, F: FnMut() -> Result<(), E>, W: FnMut(u64) -> Result<(), E>> Compare<'_, E, F, W> {
        fn admit_work(&mut self, units: u64) -> std::io::Result<()> {
            // Interrupted makes std::io::Write::write_all retry. This sentinel
            // must stop the encoder while the typed first cause stays exterior.
            if self.interrupted.is_some() {
                return Err(std::io::Error::from(std::io::ErrorKind::Other));
            }
            if let Err(error) = (self.work)(units) {
                self.interrupted = Some(error);
                return Err(std::io::Error::from(std::io::ErrorKind::Other));
            }
            Ok(())
        }
        fn checkpoint(&mut self) -> std::io::Result<()> {
            if self.interrupted.is_some() {
                return Err(std::io::Error::from(std::io::ErrorKind::Other));
            }
            if let Err(error) = (self.progress)() {
                self.interrupted = Some(error);
                return Err(std::io::Error::from(std::io::ErrorKind::Other));
            }
            self.bytes_since_checkpoint = 0;
            self.writes_since_checkpoint = 0;
            Ok(())
        }
    }
    impl<E, F: FnMut() -> Result<(), E>, W: FnMut(u64) -> Result<(), E>> Write
        for Compare<'_, E, F, W>
    {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.admit_work(1)?;
            if self.writes_since_checkpoint == CHECKPOINT_WRITES {
                self.checkpoint()?;
            }
            self.writes_since_checkpoint = self.writes_since_checkpoint.saturating_add(1);
            for chunk in bytes.chunks(CHECKPOINT_BYTES) {
                if self.bytes_since_checkpoint.saturating_add(chunk.len()) > CHECKPOINT_BYTES {
                    self.checkpoint()?;
                }
                // Each chunk is at most 64KiB, hence representable in u64.
                let units = u64::try_from(chunk.len())
                    .map_err(|_width| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
                self.admit_work(units)?;
                let end = self
                    .position
                    .checked_add(chunk.len())
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
                self.equal &= self.expected.get(self.position..end) == Some(chunk);
                self.position = end;
                self.bytes_since_checkpoint =
                    self.bytes_since_checkpoint.saturating_add(chunk.len());
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    work(0).map_err(CborPayloadMatchProgressError::Progress)?;
    progress().map_err(CborPayloadMatchProgressError::Progress)?;
    let mut comparison = Compare {
        expected,
        position: 0,
        equal: true,
        bytes_since_checkpoint: 0,
        writes_since_checkpoint: 0,
        progress,
        work,
        interrupted: None,
    };
    let encoded = ciborium::into_writer(value, &mut comparison);
    if let Some(error) = comparison.interrupted {
        return Err(CborPayloadMatchProgressError::Progress(error));
    }
    encoded.map_err(|error| {
        CborPayloadMatchProgressError::Encode(IpcError::Encode(error.to_string()))
    })?;
    (comparison.work)(0).map_err(CborPayloadMatchProgressError::Progress)?;
    (comparison.progress)().map_err(CborPayloadMatchProgressError::Progress)?;
    Ok(comparison.equal && comparison.position == expected.len())
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

/// Count the canonical CBOR body while charging every emitted write to the
/// caller's original work owner. The caller's refusal is returned unchanged.
pub fn cbor_payload_len_with_progress_v1<T, E, F>(
    value: &T,
    progress: &mut F,
) -> Result<u64, CborPayloadLengthProgressErrorV1<E>>
where
    T: serde::Serialize,
    F: FnMut(usize) -> Result<(), E> + ?Sized,
{
    struct ProgressLength<'a, E, F: ?Sized> {
        bytes: u64,
        progress: &'a mut F,
        interrupted: Option<E>,
    }

    impl<E, F: FnMut(usize) -> Result<(), E> + ?Sized> Write for ProgressLength<'_, E, F> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.interrupted.is_some() {
                return Err(std::io::ErrorKind::Other.into());
            }
            if let Err(cause) = (self.progress)(bytes.len()) {
                self.interrupted = Some(cause);
                return Err(std::io::ErrorKind::Other.into());
            }
            let added = u64::try_from(bytes.len())
                .map_err(|_overflow| std::io::Error::other("CBOR length overflow"))?;
            self.bytes = self
                .bytes
                .checked_add(added)
                .ok_or_else(|| std::io::Error::other("CBOR length overflow"))?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    progress(0).map_err(CborPayloadLengthProgressErrorV1::Progress)?;
    let mut counter = ProgressLength {
        bytes: 0,
        progress,
        interrupted: None,
    };
    let encoded = ciborium::into_writer(value, &mut counter);
    if let Some(cause) = counter.interrupted {
        return Err(CborPayloadLengthProgressErrorV1::Progress(cause));
    }
    encoded.map_err(|error| {
        CborPayloadLengthProgressErrorV1::Codec(IpcError::Encode(error.to_string()))
    })?;
    (counter.progress)(0).map_err(CborPayloadLengthProgressErrorV1::Progress)?;
    Ok(counter.bytes)
}

#[derive(Debug)]
pub enum CborPayloadLengthProgressErrorV1<E> {
    Codec(IpcError),
    Progress(E),
}

impl<E: core::fmt::Display> core::fmt::Display for CborPayloadLengthProgressErrorV1<E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Codec(cause) => write!(formatter, "CBOR encoding failed: {cause}"),
            Self::Progress(cause) => write!(formatter, "CBOR length interrupted: {cause}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for CborPayloadLengthProgressErrorV1<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(cause) => Some(cause),
            Self::Progress(cause) => Some(cause),
        }
    }
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
/// Guarded preflight also reserves parser-owned text/byte scalar backings
/// before materialization. Visitor-owned collections need their own admission.
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

/// A client response's raw CBOR body and its original retained charge.
///
/// Decoding nested owned values needs a separate, type-specific admission;
/// this body charge alone does not fund deserialized strings or vectors.
#[derive(Debug)]
pub struct AdmittedClientResponseBody {
    bytes: Vec<u8>,
    _memory: MemoryLease,
}

impl AdmittedClientResponseBody {
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

/// Read a single response body under the caller's retained memory scope.
///
/// The first four header bytes are stack storage; a refused body grant leaves
/// the reader at that header and never allocates or reads response body bytes.
pub fn read_response_body_admitted<R: Read>(
    reader: &mut R,
    budget: &RequestBudgetV1,
    memory: &MemoryScope,
) -> Result<AdmittedClientResponseBody, IpcError> {
    read_response_body_admitted_with_work_v1(reader, budget, memory, &mut |_bytes| Ok(()))
}

/// Charge response-body copy work from its untrusted header before reserving
/// backing or reading body bytes. The callback's refusal prevents allocation.
pub fn read_response_body_admitted_with_work_v1<R: Read>(
    reader: &mut R,
    budget: &RequestBudgetV1,
    memory: &MemoryScope,
    work: &mut dyn FnMut(usize) -> Result<(), IpcError>,
) -> Result<AdmittedClientResponseBody, IpcError> {
    budget
        .checkpoint("ipc_client_response_read")
        .map_err(|error| IpcError::Io(std::io::Error::other(error)))?;
    let mut header = [0_u8; FRAME_HEADER_BYTES];
    read_exact_or_truncated(reader, &mut header)?;
    let body_len_u32 = u32::from_le_bytes(header);
    if body_len_u32 == 0 {
        return Err(IpcError::EmptyFrame);
    }
    let body_len_u64 = u64::from(body_len_u32);
    let frame_limit = u64::try_from(MAX_FRAME_BODY_BYTES)
        .map_err(|_error| IpcError::Decode("response cap does not fit u64".into()))?;
    if body_len_u64 > frame_limit {
        return Err(IpcError::Oversized(body_len_u64));
    }
    let body_len = usize::try_from(body_len_u32)
        .map_err(|_error| IpcError::Decode("response length does not fit usize".into()))?;
    work(body_len)?;
    let memory_lease = memory
        .reserve(body_len_u64, MemoryLifetime::Retained)
        .map_err(|error| IpcError::from_client_memory_error(body_len_u64, error))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(body_len)
        .map_err(|_error| IpcError::FrameMemoryLimited {
            bytes: body_len_u64,
        })?;
    bytes.resize(body_len, 0);
    read_exact_or_truncated(reader, &mut bytes)?;
    budget
        .checkpoint("ipc_client_response_read")
        .map_err(|error| IpcError::Io(std::io::Error::other(error)))?;
    if bytes.starts_with(MULTIFRAME_REQUEST_MAGIC) || bytes.starts_with(COMPRESSED_REQUEST_MAGIC) {
        return Err(IpcError::Decode(
            "request framing is invalid for an IPC response".into(),
        ));
    }
    Ok(AdmittedClientResponseBody {
        bytes,
        _memory: memory_lease,
    })
}

pub fn decode_cbor_payload<T>(bytes: &[u8]) -> Result<T, IpcError>
where
    T: serde::de::DeserializeOwned,
{
    let (decoded, consumed) = quanta_index_cbor::from_slice_with_definite_scalars(bytes)
        .map_err(|err| IpcError::Decode(err.to_string()))?;
    if consumed != bytes.len() {
        return Err(IpcError::Decode(
            "CBOR payload contains trailing bytes after its value".to_string(),
        ));
    }
    Ok(decoded)
}

fn encode_frame<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, IpcError> {
    // Standalone codec callers explicitly own their allocation envelope.
    // Server responses use encode_response_admitted with the service ledger.
    let frame = encode_owned_frame(value, None, |_bytes| Ok(None))?;
    Ok(frame.bytes)
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
        let decoded_storage = crate::cbor_preflight::guarded_request_storage_budget(bytes)?;
        reserve(guard, decoded_storage)?;
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
        COMPRESSED_REQUEST_HEADER_BYTES, COMPRESSED_REQUEST_MAGIC, DIAGNOSTIC_ACK_BYTES,
        DIAGNOSTIC_PREFACE_BYTES, DIAGNOSTIC_PREFACE_MAGIC, IpcError,
        MAX_DECOMPRESSED_FRAME_BODY_BYTES, MAX_FRAME_BODY_BYTES, MAX_MULTIFRAME_REQUEST_BODY_BYTES,
        MULTIFRAME_REQUEST_MAGIC, decode_cbor_payload, decode_request, decode_request_guarded,
        decode_response, diagnostic_ack, diagnostic_ack_accepted, diagnostic_preface,
        encode_cbor_payload, encode_request,
    };

    #[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
    #[test]
    fn stock_ciborium_pointer_integer_extremes_preserve_range() {
        use ciborium::value::Integer;

        for value in [0_usize, 1, usize::MAX] {
            let integer = Integer::from(value);
            assert_eq!(usize::try_from(integer), Ok(value));
            assert_eq!(u128::try_from(integer), u128::try_from(value));
        }
        for value in [isize::MIN, -1, 0, 1, isize::MAX] {
            let integer = Integer::from(value);
            assert_eq!(isize::try_from(integer), Ok(value));
            assert_eq!(i128::try_from(value), Ok(i128::from(integer)));
        }
        assert!(usize::try_from(Integer::from(-1_i8)).is_err());
        assert!(isize::try_from(Integer::from(usize::MAX)).is_err());
    }

    struct ClientMemoryOwner(std::sync::Arc<std::sync::atomic::AtomicU64>);

    impl quanta_index_core::index::MemoryLeaseOwner for ClientMemoryOwner {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_sub(bytes, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct ClientMemoryScope {
        active: std::sync::Arc<std::sync::atomic::AtomicU64>,
        deny: bool,
    }

    impl quanta_index_core::index::MemoryScopePort for ClientMemoryScope {
        fn reserve(
            &self,
            bytes: u64,
            lifetime: quanta_index_core::index::MemoryLifetime,
        ) -> Result<quanta_index_core::index::MemoryLease, quanta_index_core::index::IndexError>
        {
            if self.deny || lifetime != quanta_index_core::index::MemoryLifetime::Retained {
                return Err(quanta_index_core::index::IndexError::ResourceLimited);
            }
            let _previous = self
                .active
                .fetch_add(bytes, std::sync::atomic::Ordering::SeqCst);
            Ok(quanta_index_core::index::MemoryLease::new(
                bytes,
                std::sync::Arc::new(ClientMemoryOwner(std::sync::Arc::clone(&self.active))),
            ))
        }
    }

    fn client_memory_scope(
        deny: bool,
    ) -> (
        quanta_index_core::index::MemoryScope,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) {
        let active = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        (
            quanta_index_core::index::MemoryScope::new(std::sync::Arc::new(ClientMemoryScope {
                active: std::sync::Arc::clone(&active),
                deny,
            })),
            active,
        )
    }

    #[test]
    fn admitted_client_request_matches_standalone_wire_and_refunds_on_drop() {
        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(2));
        let (scope, active) = client_memory_scope(false);
        let value = (7_u64, "native-source");
        let expected = super::encode_request(&value).expect("standalone request");
        let frame = super::encode_request_single_frame_admitted(&value, &budget, &scope)
            .expect("admitted request");
        assert_eq!(&*frame, expected.as_slice());
        assert_eq!(
            active.load(std::sync::atomic::Ordering::SeqCst),
            u64::try_from(frame.len()).expect("frame length")
        );
        drop(frame);
        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn admitted_client_request_denial_prevents_second_encode_pass() {
        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(2));
        let (scope, active) = client_memory_scope(true);
        let value = ChangingResponse {
            calls: std::sync::atomic::AtomicUsize::new(0),
            shrink: false,
        };
        let result = super::encode_request_single_frame_admitted(&value, &budget, &scope);
        assert!(matches!(result, Err(IpcError::FrameMemoryLimited { .. })));
        assert_eq!(value.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn admitted_client_multiframe_uses_only_retained_wire_backing() {
        use std::sync::atomic::Ordering::SeqCst;

        let request = "x".repeat(MAX_FRAME_BODY_BYTES);
        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(30));
        let (scope, active) = client_memory_scope(false);
        let frame = super::encode_request_multiframe_admitted_v1(&request, &budget, &scope)
            .expect("admitted request exceeds one frame");
        assert!(frame[4..].starts_with(MULTIFRAME_REQUEST_MAGIC));
        assert_eq!(
            active.load(SeqCst),
            u64::try_from(frame.len()).expect("wire length")
        );
        let decoded: String = decode_request(&mut std::io::Cursor::new(&*frame))
            .expect("existing receiver decodes admitted fragments");
        assert_eq!(decoded, request);
        drop(frame);
        assert_eq!(active.load(SeqCst), 0);

        let (denying_scope, denied_active) = client_memory_scope(true);
        assert!(matches!(
            super::encode_request_multiframe_admitted_v1(&request, &budget, &denying_scope),
            Err(IpcError::FrameMemoryLimited { .. })
        ));
        assert_eq!(denied_active.load(SeqCst), 0);
    }

    #[test]
    fn admitted_client_response_body_denial_and_drop_preserve_custody() {
        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(2));
        let wire = super::encode_response(&("ACK", 9_u64)).expect("response frame");
        let (denying_scope, denied_active) = client_memory_scope(true);
        let mut denied = std::io::Cursor::new(wire.as_slice());
        let refused = super::read_response_body_admitted(&mut denied, &budget, &denying_scope);
        assert!(matches!(refused, Err(IpcError::FrameMemoryLimited { .. })));
        assert_eq!(denied.position(), 4);
        assert_eq!(denied_active.load(std::sync::atomic::Ordering::SeqCst), 0);

        let (scope, active) = client_memory_scope(false);
        let mut reader = std::io::Cursor::new(wire.as_slice());
        let body = super::read_response_body_admitted(&mut reader, &budget, &scope)
            .expect("admitted response body");
        assert_eq!(body.as_slice(), wire.get(4..).expect("response body"));
        assert_eq!(
            active.load(std::sync::atomic::Ordering::SeqCst),
            u64::try_from(body.as_slice().len()).expect("body length")
        );
        drop(body);
        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn admitted_client_scope_contract_and_cancelled_refusal_remain_distinct() {
        struct RefusingScope(quanta_index_core::index::IndexError);
        impl quanta_index_core::index::MemoryScopePort for RefusingScope {
            fn reserve(
                &self,
                _bytes: u64,
                _lifetime: quanta_index_core::index::MemoryLifetime,
            ) -> Result<quanta_index_core::index::MemoryLease, quanta_index_core::index::IndexError>
            {
                Err(self.0.clone())
            }
        }

        struct WrongSizeScope(std::sync::Arc<std::sync::atomic::AtomicU64>);
        impl quanta_index_core::index::MemoryScopePort for WrongSizeScope {
            fn reserve(
                &self,
                bytes: u64,
                _lifetime: quanta_index_core::index::MemoryLifetime,
            ) -> Result<quanta_index_core::index::MemoryLease, quanta_index_core::index::IndexError>
            {
                let granted = bytes
                    .checked_add(1)
                    .ok_or(quanta_index_core::index::IndexError::ResourceLimited)?;
                let _previous = self
                    .0
                    .fetch_add(granted, std::sync::atomic::Ordering::SeqCst);
                Ok(quanta_index_core::index::MemoryLease::new(
                    granted,
                    std::sync::Arc::new(ClientMemoryOwner(std::sync::Arc::clone(&self.0))),
                ))
            }
        }

        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(2));
        let cancelled = quanta_index_core::index::MemoryScope::new(std::sync::Arc::new(
            RefusingScope(quanta_index_core::index::IndexError::Cancelled),
        ));
        assert!(matches!(
            super::encode_request_single_frame_admitted(&9_u64, &budget, &cancelled),
            Err(IpcError::Io(error)) if error.to_string().contains("cancelled")
        ));
        let work_limited = quanta_index_core::index::MemoryScope::new(std::sync::Arc::new(
            RefusingScope(quanta_index_core::index::IndexError::WorkLimitExceeded),
        ));
        assert!(matches!(
            super::encode_request_single_frame_admitted(&9_u64, &budget, &work_limited),
            Err(IpcError::Io(error)) if error.to_string().contains("work budget exhausted")
        ));

        let active = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let wrong_size = quanta_index_core::index::MemoryScope::new(std::sync::Arc::new(
            WrongSizeScope(std::sync::Arc::clone(&active)),
        ));
        assert!(matches!(
            super::encode_request_single_frame_admitted(&9_u64, &budget, &wrong_size),
            Err(IpcError::Io(error)) if error.to_string().contains("allocation lease differs")
        ));
        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);

        let response = super::encode_response(&9_u64).expect("response frame");
        let mut reader = std::io::Cursor::new(response.as_slice());
        assert!(matches!(
            super::read_response_body_admitted(&mut reader, &budget, &cancelled),
            Err(IpcError::Io(error)) if error.to_string().contains("cancelled")
        ));
        assert_eq!(reader.position(), 4);
        let mut work_limited_reader = std::io::Cursor::new(response.as_slice());
        assert!(matches!(
            super::read_response_body_admitted(
                &mut work_limited_reader,
                &budget,
                &work_limited
            ),
            Err(IpcError::Io(error)) if error.to_string().contains("work budget exhausted")
        ));
        assert_eq!(work_limited_reader.position(), 4);
    }

    #[test]
    fn admitted_client_response_truncation_and_preheader_cancel_refund() {
        let wire = super::encode_response(&("ACK", 9_u64)).expect("response frame");
        let (scope, active) = client_memory_scope(false);
        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(2));
        let truncated_len = wire.len().checked_sub(1).expect("nonempty response");
        let mut truncated =
            std::io::Cursor::new(wire.get(..truncated_len).expect("truncated frame"));
        assert!(matches!(
            super::read_response_body_admitted(&mut truncated, &budget, &scope),
            Err(IpcError::Truncated)
        ));
        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);

        budget.cancel_handle().cancel();
        let mut cancelled = std::io::Cursor::new(wire.as_slice());
        assert!(matches!(
            super::read_response_body_admitted(&mut cancelled, &budget, &scope),
            Err(IpcError::Io(_))
        ));
        assert_eq!(cancelled.position(), 0);
        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn frozen_payload_comparison_rejects_altered_truncated_and_trailing_bytes() {
        let value = ("source", vec!["empty", "한글", "body"]);
        let encoded = encode_cbor_payload(&value).expect("fixed CBOR fixture");
        assert!(super::cbor_payload_matches(&value, &encoded).expect("exact comparison"));
        assert!(!super::cbor_payload_matches(&value, &[]).expect("empty comparison"));
        let (_, truncated) = encoded.split_last().expect("nonempty CBOR fixture");
        assert!(!super::cbor_payload_matches(&value, truncated).expect("truncated comparison"));
        let mut altered = encoded.clone();
        *altered.first_mut().expect("nonempty CBOR fixture") ^= 1;
        assert!(!super::cbor_payload_matches(&value, &altered).expect("altered comparison"));
        let mut trailing = encoded;
        trailing.push(0);
        assert!(!super::cbor_payload_matches(&value, &trailing).expect("trailing comparison"));
    }

    #[test]
    fn original_work_comparison_preserves_ipc_struct_wire_and_refuses_sorted_map() {
        struct Fields {
            z: u8,
            a: u8,
        }
        impl serde::Serialize for Fields {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeStruct as _;

                let mut state = serializer.serialize_struct("Fields", 2)?;
                state.serialize_field("z", &self.z)?;
                state.serialize_field("a", &self.a)?;
                state.end()
            }
        }
        let value = Fields { z: 1, a: 2 };
        let wire = [0xa2, 0x61, b'z', 1, 0x61, b'a', 2];
        let sorted = [0xa2, 0x61, b'a', 2, 0x61, b'z', 1];
        let mut paid = 0_u64;
        assert!(
            super::cbor_payload_matches_with_work_v1(&value, &wire, &mut |units| {
                paid += units;
                Ok::<(), ()>(())
            })
            .expect("existing IPC wire")
        );
        assert!(paid >= u64::try_from(wire.len()).expect("fixed wire length fits u64"));
        assert!(
            !super::cbor_payload_matches_with_work_v1(&value, &sorted, &mut |_| {
                Ok::<(), ()>(())
            })
            .expect("different canonical domain")
        );
    }

    #[test]
    fn original_work_comparison_preserves_first_refusal_during_mismatched_large_output() {
        #[derive(Debug, PartialEq)]
        struct Refused;
        let value = "x".repeat(70 * 1024);
        let mut available = 32_u64;
        let mut refused = false;
        let result = super::cbor_payload_matches_with_work_v1(&value, &[], &mut |units| {
            assert!(!refused, "no further callback after first refusal");
            if units > available {
                refused = true;
                Err(Refused)
            } else {
                available -= units;
                Ok(())
            }
        });
        assert!(refused);
        assert!(matches!(
            result,
            Err(super::CborPayloadMatchProgressError::Progress(Refused))
        ));
    }

    #[test]
    fn frozen_payload_comparison_propagates_serialization_failure_after_mismatch() {
        struct Invalid;
        impl serde::Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeSeq;
                let mut sequence = serializer.serialize_seq(Some(2))?;
                sequence.serialize_element(&7_u8)?;
                Err(serde::ser::Error::custom("invalid frozen source"))
            }
        }
        assert!(matches!(
            super::cbor_payload_matches(&Invalid, &[]),
            Err(IpcError::Encode(message)) if message.contains("invalid frozen source")
        ));
    }

    #[test]
    fn frozen_payload_progress_preserves_refusal_before_serialization() {
        struct MustNotSerialize(std::sync::atomic::AtomicBool);
        impl serde::Serialize for MustNotSerialize {
            fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                Err(serde::ser::Error::custom("unexpected serialization"))
            }
        }
        let value = MustNotSerialize(std::sync::atomic::AtomicBool::new(false));
        let error = super::cbor_payload_matches_with_progress(&value, &[], &mut || {
            Err("original-owner-refusal")
        });
        assert!(matches!(
            error,
            Err(super::CborPayloadMatchProgressError::Progress(
                "original-owner-refusal"
            ))
        ));
        assert!(!value.0.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn frozen_payload_progress_interrupts_a_large_single_encoder_write() {
        struct Body<'a>(&'a [u8]);
        impl serde::Serialize for Body<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_bytes(self.0)
            }
        }
        let bytes = vec![7_u8; 192 * 1024];
        let body = Body(&bytes);
        let mut checkpoints = 0_u32;
        let error = super::cbor_payload_matches_with_progress(&body, &[], &mut || {
            checkpoints = checkpoints.saturating_add(1);
            if checkpoints == 3 {
                Err("lease-expired")
            } else {
                Ok(())
            }
        });
        assert_eq!(checkpoints, 3);
        assert!(matches!(
            error,
            Err(super::CborPayloadMatchProgressError::Progress(
                "lease-expired"
            ))
        ));
    }

    #[test]
    fn frozen_payload_progress_checks_final_owner_and_keeps_byte_equality() {
        let encoded = encode_cbor_payload(&7_u8).expect("fixed CBOR fixture");
        let mut checkpoints = 0_u32;
        let error = super::cbor_payload_matches_with_progress(&7_u8, &encoded, &mut || {
            checkpoints = checkpoints.saturating_add(1);
            if checkpoints == 2 { Err(42) } else { Ok(()) }
        });
        assert!(matches!(
            error,
            Err(super::CborPayloadMatchProgressError::Progress(42))
        ));
        let same =
            super::cbor_payload_matches_with_progress(&7_u8, &encoded, &mut || Ok::<_, ()>(()));
        assert!(matches!(same, Ok(true)));
        let different =
            super::cbor_payload_matches_with_progress(&8_u8, &encoded, &mut || Ok::<_, ()>(()));
        assert!(matches!(different, Ok(false)));
    }

    #[test]
    fn response_oversize_and_cancel_refuse_before_admission() {
        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(5));
        let called = std::sync::atomic::AtomicBool::new(false);
        let large = "x".repeat(MAX_FRAME_BODY_BYTES);
        let outcome = super::encode_response_admitted(&large, &budget, |_bytes| {
            called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        });
        assert!(matches!(outcome, Err(IpcError::Oversized(_))));
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
        budget.cancel_handle().cancel();
        assert!(
            matches!(super::encode_response_admitted(&1_u64, &budget, |_bytes| {
            called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        }), Err(IpcError::Io(error)) if error.kind() == std::io::ErrorKind::Other)
        );
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    struct ChangingResponse {
        calls: std::sync::atomic::AtomicUsize,
        shrink: bool,
    }
    impl serde::Serialize for ChangingResponse {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let first = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            serializer.serialize_u64(if first == self.shrink { 1000 } else { 1 })
        }
    }

    #[test]
    fn response_denial_does_not_start_encoding_and_count_mismatch_fails_closed() {
        let budget =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(5));
        let value = ChangingResponse {
            calls: std::sync::atomic::AtomicUsize::new(0),
            shrink: false,
        };
        let outcome = super::encode_response_admitted(&value, &budget, |bytes| {
            Err(IpcError::FrameMemoryLimited { bytes })
        });
        assert!(matches!(outcome, Err(IpcError::FrameMemoryLimited { .. })));
        assert_eq!(value.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        for shrink in [false, true] {
            let value = ChangingResponse {
                calls: std::sync::atomic::AtomicUsize::new(0),
                shrink,
            };
            assert!(super::encode_response_admitted(&value, &budget, |_bytes| Ok(None)).is_err());
            assert_eq!(value.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        }
    }

    #[test]
    fn diagnostic_capability_frames_have_fixed_versioned_bytes() {
        let capture = [0x5a; 16];
        let preface = diagnostic_preface(capture);
        assert_eq!(preface.len(), DIAGNOSTIC_PREFACE_BYTES);
        assert_eq!(&preface[..8], &DIAGNOSTIC_PREFACE_MAGIC);
        assert_eq!(&preface[8..], &capture);
        let accepted = diagnostic_ack(true);
        let rejected = diagnostic_ack(false);
        assert_eq!(accepted.len(), DIAGNOSTIC_ACK_BYTES);
        assert_eq!(diagnostic_ack_accepted(&accepted), Some(true));
        assert_eq!(diagnostic_ack_accepted(&rejected), Some(false));
        let mut unknown = accepted;
        unknown[7] = b'2';
        assert_eq!(diagnostic_ack_accepted(&unknown), None);
        unknown = accepted;
        unknown[8] = 2;
        assert_eq!(diagnostic_ack_accepted(&unknown), None);
    }

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
    fn guarded_request_admits_owned_scalar_backing_before_deserializing() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static MATERIALIZED: AtomicBool = AtomicBool::new(false);
        #[derive(Debug)]
        struct MustNotMaterialize;
        impl<'de> serde::Deserialize<'de> for MustNotMaterialize {
            fn deserialize<D: serde::Deserializer<'de>>(
                _deserializer: D,
            ) -> Result<Self, D::Error> {
                MATERIALIZED.store(true, Ordering::SeqCst);
                Err(serde::de::Error::custom(
                    "request materialized before admission",
                ))
            }
        }

        for value in [
            ciborium::Value::Text("t".repeat(4_097)),
            ciborium::Value::Bytes(vec![0xab; 6_383]),
        ] {
            let frame = encode_request(&value).expect("bounded scalar request");
            let body_len = frame.len() - super::FRAME_HEADER_BYTES;
            // The raw body and small DTO headers fit; a second scalar backing does not.
            let limit = body_len + 256;
            MATERIALIZED.store(false, Ordering::SeqCst);
            let result = decode_request_guarded::<MustNotMaterialize, _, usize>(
                &mut Cursor::new(frame),
                Ok,
                |held, additional| {
                    let next = held.checked_add(additional).expect("bounded test charge");
                    if next > limit {
                        return Err(IpcError::IngressSaturated {
                            bytes: limit,
                            requests: 1,
                        });
                    }
                    *held = next;
                    Ok(())
                },
            );
            assert!(matches!(result, Err(IpcError::IngressSaturated { .. })));
            assert!(!MATERIALIZED.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn guarded_native_immutable_stage_round_trips_route_sized_payload_and_ordered_children() {
        use quanta_index_contract::index::{
            CollectionId, Digest, IngressRef, ObjectRef, OperationKey, ProducerId, SlotId,
            StreamId, TenantId, WorkspaceKey, WorktreeId,
        };
        use quanta_index_contract::native_protocol::{
            NativeImmutableObjectChildrenV1, NativeImmutableObjectPayloadV1, NativeIndexRequest,
            NativeIndexRequestEnvelope,
        };

        let ingress = IngressRef {
            operation: OperationKey {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 1,
                producer: ProducerId([5; 16]),
                stream: StreamId([6; 16]),
                stream_epoch: 1,
                sequence: 50,
            },
            fence: 7,
        };
        let expected_children: Vec<_> = [1_u8, 3, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 3]
            .into_iter()
            .map(|byte| ObjectRef(Digest([byte; 32])))
            .collect();

        for (request_id, payload, edges) in [
            (70_000, vec![0x31; 64], Vec::new()),
            (70_001, vec![0x32; 64], Vec::new()),
            (70_002, vec![0x7b; 6_383], expected_children.clone()),
        ] {
            let request = NativeIndexRequestEnvelope {
                request_id,
                payload: NativeIndexRequest::StageImmutableObject {
                    ingress,
                    bytes: NativeImmutableObjectPayloadV1::new(payload).expect("bounded payload"),
                    children: NativeImmutableObjectChildrenV1::new(edges).expect("bounded edges"),
                },
            };
            let frame = encode_request(&request).expect("single-frame native request");
            let body_len = usize::try_from(u32::from_le_bytes(
                frame[..4].try_into().expect("frame header"),
            ))
            .expect("frame body length");
            assert_eq!(frame.len(), body_len + 4);
            assert!(body_len < MAX_FRAME_BODY_BYTES);

            let mut reader = Cursor::new(frame.as_slice());
            let (decoded, charged): (NativeIndexRequestEnvelope, usize) = decode_request_guarded(
                &mut reader,
                |bytes| {
                    assert_eq!(bytes, body_len);
                    Ok(bytes)
                },
                |held, additional| {
                    *held = held.checked_add(additional).expect("bounded decode charge");
                    if *held > 64 * 1024 {
                        return Err(IpcError::IngressSaturated {
                            bytes: 64 * 1024,
                            requests: 1,
                        });
                    }
                    Ok(())
                },
            )
            .expect("guarded native request decode");
            assert_eq!(decoded, request);
            assert_eq!(
                reader.position(),
                u64::try_from(frame.len()).expect("frame length")
            );
            assert!(charged >= body_len);
            assert!(charged <= 64 * 1024);
            if request_id == 70_002 {
                let NativeIndexRequest::StageImmutableObject {
                    bytes, children, ..
                } = decoded.payload
                else {
                    panic!("decoded route-sized immutable stage changed opcode");
                };
                assert_eq!(bytes.len(), 6_383);
                assert_eq!(children.as_slice(), expected_children.as_slice());
            }
        }
    }


    #[test]
    fn guarded_query_cursor_preserves_opaque_text_across_reader_scratch_boundary() {
        use quanta_index_contract::{
            ContinuationTokenV2, DiagnosticAttemptFieldV1, QueryConstraintSetV1,
            SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, TextQueryRequest,
            TextQuerySyntax,
        };

        assert!(ContinuationTokenV2::new("").is_err());
        let empty = encode_request(&"").expect("encode empty token counterexample");
        let rejected: Result<ContinuationTokenV2, IpcError> =
            decode_request(&mut Cursor::new(empty));
        assert!(
            matches!(&rejected, Err(IpcError::Decode(message))
                if message.contains("non-empty continuation token")),
            "unexpected rejection: {rejected:?}"
        );

        for (request_id, size) in [(72_000, 4_096), (72_001, 4_097)] {
            let cursor_text = "c".repeat(size);
            let cursor = ContinuationTokenV2::new(cursor_text.clone())
                .expect("nonempty opaque cursor is allowed by its contract");
            let request = SearchPlaneQueryIpcRequestEnvelope {
                request_id,
                diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
                payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                    syntax: TextQuerySyntax::CodeSearch,
                    query_text: "needle".into(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: None,
                    generation_selector: None,
                    top_k: 1,
                    cursor: Some(cursor),
                }),
            };
            let frame = encode_request(&request)
                .unwrap_or_else(|error| panic!("id={request_id} len={size} encode: {error}"));
            let mut reader = Cursor::new(frame.as_slice());
            let (decoded, charged): (SearchPlaneQueryIpcRequestEnvelope, usize) =
                decode_request_guarded(&mut reader, Ok, |held, additional| {
                    *held = held.checked_add(additional).expect("bounded decode charge");
                    if *held > 64 * 1024 {
                        return Err(IpcError::IngressSaturated {
                            bytes: 64 * 1024,
                            requests: 1,
                        });
                    }
                    Ok(())
                })
                .unwrap_or_else(|error| panic!("id={request_id} len={size} decode: {error}"));
            assert_eq!(decoded, request, "id={request_id} len={size}");
            assert_eq!(
                reader.position(),
                u64::try_from(frame.len()).expect("frame length")
            );
            assert!(charged <= 64 * 1024, "id={request_id} len={size}");
            let SearchPlaneQueryIpcRequest::Text(query) = decoded.payload else {
                panic!("id={request_id} len={size} changed query route");
            };
            assert_eq!(
                query.cursor.as_ref().map(ContinuationTokenV2::as_str),
                Some(cursor_text.as_str()),
                "id={request_id} len={size} cursor must reach downstream validation unchanged"
            );
        }
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
            wire_len + logical_len + 3 * std::mem::size_of::<String>() + expected.len()
        );
    }

    #[test]
    fn guarded_multiframe_admits_large_definite_byte_backing() {
        use crate::admission::{DecodePermit, DispatchSlots};
        use crate::plane::IpcPlane;

        // Exercise an inline-source-sized byte string without relying on its
        // compressibility. The integer-array regression below does not cover
        // the parser's owned byte backing.
        let payload_len = 60 * 1024 * 1024;
        let body = encode_cbor_payload(&ciborium::Value::Bytes(vec![0xab; payload_len]))
            .expect("encode definite source-sized value");
        let frame = super::encode_multiframe_request(body).expect("frame large request");
        let slots = DispatchSlots::new(4, 2);
        let (value, _permit): (ciborium::Value, _) = decode_request_guarded(
            &mut Cursor::new(frame),
            |bytes| slots.try_acquire_decode(bytes, IpcPlane::Ingest),
            DecodePermit::reserve,
        )
        .expect("admit framed body and one definite backing");
        let ciborium::Value::Bytes(actual) = value else {
            panic!("decoded value must be the original byte string");
        };
        assert_eq!(actual.len(), payload_len);
        assert!(actual.iter().all(|byte| *byte == 0xab));
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
