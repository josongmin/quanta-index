//! Embedding-record byte format used by the semantic build port.
//!
//! Phase-1 byte layout (`RawF32` envelope):
//!
//! ```text
//! header (16 bytes):
//!     u32 LE magic = 0x51495345 ("ESIQ" reversed; spells "QISE" big-endian)
//!     u32 LE record_count
//!     u32 LE vector_dim
//!     u32 LE _reserved (0)
//! records (record_count of them):
//!     u32 LE entity_id_len
//!     entity_id_bytes (entity_id_len bytes, UTF-8)
//!     (vector_dim * 4) bytes of f32 LE
//! ```
//!
//! All reads use `u32::from_le_bytes` over slices obtained via
//! `get(range).ok_or(...)`. No panicking slicing is used.

#![expect(
    clippy::redundant_pub_crate,
    reason = "wire is an internal-only sub-module reached via crate::wire::* paths; \
              pub(crate) is the correct visibility and pub would expose internal \
              encoding details on the public surface"
)]

use quanta_index_core::CoreError;

/// Magic header tag identifying the embedding payload.
///
/// The bytes spell `"QISE"` when read big-endian; on the wire the
/// little-endian encoding is `[0x45, 0x53, 0x49, 0x51]`.
pub(crate) const EMBEDDING_RECORDS_MAGIC: u32 = 0x5149_5345;

/// Fixed header size in bytes.
pub(crate) const HEADER_LEN: usize = 16;

/// Per-record fixed prefix (entity-id length field) size in bytes.
pub(crate) const RECORD_LEN_PREFIX: usize = 4;

/// Decoded embedding record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EmbeddingRecord {
    pub(crate) entity_id: String,
    pub(crate) vector: Vec<f32>,
}

/// Decoded embedding payload.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EmbeddingPayload {
    pub(crate) vector_dim: u32,
    pub(crate) records: Vec<EmbeddingRecord>,
}

fn invalid(detail: &str) -> CoreError {
    CoreError::InvalidContract(format!("embedding_records: {detail}"))
}

fn read_u32_le(bytes: &[u8], start: usize) -> Result<u32, CoreError> {
    let end = start
        .checked_add(4)
        .ok_or_else(|| invalid("integer overflow computing u32 slice end"))?;
    let window: [u8; 4] = bytes
        .get(start..end)
        .ok_or_else(|| invalid("truncated u32 field"))?
        .try_into()
        .map_err(|_err| invalid("u32 slice failed length conversion"))?;
    Ok(u32::from_le_bytes(window))
}

fn usize_from_u32(value: u32, label: &str) -> Result<usize, CoreError> {
    usize::try_from(value).map_err(|_err| {
        invalid(&format!(
            "{label}: u32 value does not fit in usize on this target"
        ))
    })
}

/// Decode an embedding-records buffer into an `EmbeddingPayload`.
///
/// Validates the header magic, that `record_count > 0`, that
/// `vector_dim > 0`, that each record's entity-id is non-empty, and that
/// the total buffer length exactly matches the expected length.
pub(crate) fn decode_embedding_records(bytes: &[u8]) -> Result<EmbeddingPayload, CoreError> {
    if bytes.len() < HEADER_LEN {
        return Err(invalid("buffer shorter than 16-byte header"));
    }
    let magic = read_u32_le(bytes, 0)?;
    if magic != EMBEDDING_RECORDS_MAGIC {
        return Err(invalid("magic mismatch"));
    }
    let record_count_u32 = read_u32_le(bytes, 4)?;
    let vector_dim_u32 = read_u32_le(bytes, 8)?;
    let reserved = read_u32_le(bytes, 12)?;
    if reserved != 0 {
        return Err(invalid("reserved header field must be zero"));
    }
    if record_count_u32 == 0 {
        return Err(invalid("record_count must be > 0"));
    }
    if vector_dim_u32 == 0 {
        return Err(invalid("vector_dim must be > 0"));
    }

    let record_count = usize_from_u32(record_count_u32, "record_count")?;
    let vector_dim = usize_from_u32(vector_dim_u32, "vector_dim")?;

    let vector_byte_len = vector_dim
        .checked_mul(4)
        .ok_or_else(|| invalid("vector_dim * 4 overflows usize"))?;

    let mut cursor: usize = HEADER_LEN;
    let mut records: Vec<EmbeddingRecord> = Vec::with_capacity(record_count);
    for record_index in 0..record_count {
        let id_len_u32 = read_u32_le(bytes, cursor)?;
        if id_len_u32 == 0 {
            return Err(invalid(&format!(
                "record {record_index}: entity_id_len must be > 0"
            )));
        }
        let id_len = usize_from_u32(id_len_u32, "entity_id_len")?;
        let id_start = cursor
            .checked_add(RECORD_LEN_PREFIX)
            .ok_or_else(|| invalid("record cursor overflow at id_start"))?;
        let id_end = id_start
            .checked_add(id_len)
            .ok_or_else(|| invalid("record cursor overflow at id_end"))?;
        let id_bytes = bytes
            .get(id_start..id_end)
            .ok_or_else(|| invalid(&format!("record {record_index}: entity_id truncated")))?;
        let entity_id = std::str::from_utf8(id_bytes)
            .map_err(|err| {
                invalid(&format!(
                    "record {record_index}: entity_id not utf-8: {err}"
                ))
            })?
            .to_owned();

        let vec_start = id_end;
        let vec_end = vec_start
            .checked_add(vector_byte_len)
            .ok_or_else(|| invalid("record cursor overflow at vec_end"))?;
        let vec_bytes = bytes
            .get(vec_start..vec_end)
            .ok_or_else(|| invalid(&format!("record {record_index}: vector body truncated")))?;
        let mut vector: Vec<f32> = Vec::with_capacity(vector_dim);
        for lane in 0..vector_dim {
            let lane_start = lane
                .checked_mul(4)
                .ok_or_else(|| invalid("lane offset overflow"))?;
            let lane_end = lane_start
                .checked_add(4)
                .ok_or_else(|| invalid("lane end overflow"))?;
            let window: [u8; 4] = vec_bytes
                .get(lane_start..lane_end)
                .ok_or_else(|| invalid("lane slice missing"))?
                .try_into()
                .map_err(|_err| invalid("lane slice length mismatch"))?;
            vector.push(f32::from_le_bytes(window));
        }

        records.push(EmbeddingRecord { entity_id, vector });
        cursor = vec_end;
    }

    if cursor != bytes.len() {
        return Err(invalid(&format!(
            "trailing bytes after final record: cursor={cursor}, buffer_len={}",
            bytes.len()
        )));
    }

    Ok(EmbeddingPayload {
        vector_dim: vector_dim_u32,
        records,
    })
}
