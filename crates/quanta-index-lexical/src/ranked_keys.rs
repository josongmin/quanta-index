//! Immutable, segment-bound ranked-order keys. The seal derives these from
//! Tantivy's dictionaries once; collectors never decode an `SSTable` key.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use quanta_index_core::CoreError;
use tantivy::SegmentId;
use tantivy::SegmentReader;
use tantivy::columnar::StrColumn;

use crate::schema::{RANKED_CANDIDATE_ID_COLUMN, RANKED_PATH_COLUMN, RANKED_SOURCE_REPO_COLUMN};

const MAGIC: &[u8; 8] = b"QIRKEY01";
const COLUMNS: [&str; 3] = [
    RANKED_SOURCE_REPO_COLUMN,
    RANKED_PATH_COLUMN,
    RANKED_CANDIDATE_ID_COLUMN,
];
/// This resident table is separate from the per-request collection budget.
///
/// A generation larger than this is refused at seal/open, not served with an
/// unbounded fallback. The snapshot registry separately admits the full
/// generation estimate, including these bytes and the Tantivy index.
pub(crate) const MAX_RANKED_KEYS_BYTES: usize = 64 * 1024 * 1024;
const PREFIX: &str = "ranked-keys-";
const SUFFIX: &str = ".bin";

fn corrupt(name: &str, reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_core::GENERATION_SIDECAR_CORRUPT_CODE,
        message: format!("lexical: ranked keys {name}: {reason}"),
    }
}

pub(crate) fn file_name(reader: &SegmentReader) -> String {
    format!("{PREFIX}{}{SUFFIX}", reader.segment_id().uuid_string())
}

pub(crate) fn is_file_name(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .and_then(|id| id.strip_suffix(SUFFIX))
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(crate) fn is_ranked_key_entry(name: &str) -> bool {
    name.starts_with(PREFIX)
}

fn append(bytes: &mut Vec<u8>, piece: &[u8], name: &str) -> Result<(), CoreError> {
    let len = bytes
        .len()
        .checked_add(piece.len())
        .ok_or_else(|| corrupt(name, "size overflow"))?;
    if len > MAX_RANKED_KEYS_BYTES {
        return Err(CoreError::InvalidContract(format!(
            "lexical: ranked keys {name} exceed the generation resident limit"
        )));
    }
    bytes.try_reserve(piece.len()).map_err(|error| {
        CoreError::Storage(format!("lexical: reserve ranked keys {name}: {error}"))
    })?;
    bytes.extend_from_slice(piece);
    Ok(())
}

/// Called at seal after the final Tantivy commit. Its input is the index this
/// process just wrote; older manifest formats are refused at open.
pub(crate) fn encode(reader: &SegmentReader) -> Result<Vec<u8>, CoreError> {
    let name = file_name(reader);
    let mut out = Vec::new();
    append(&mut out, MAGIC, &name)?;
    append(
        &mut out,
        reader.segment_id().uuid_string().as_bytes(),
        &name,
    )?;
    for column_name in COLUMNS {
        let column = reader
            .fast_fields()
            .str(column_name)
            .map_err(|error| corrupt(&name, &format!("open {column_name}: {error}")))?
            .ok_or_else(|| corrupt(&name, &format!("missing {column_name}")))?;
        let dictionary = column.dictionary();
        let offsets_bytes = dictionary
            .num_terms()
            .checked_add(1)
            .and_then(|count| count.checked_mul(8))
            .ok_or_else(|| corrupt(&name, "offset count overflow"))?;
        if offsets_bytes > MAX_RANKED_KEYS_BYTES.saturating_sub(out.len()) {
            return Err(CoreError::InvalidContract(format!(
                "lexical: ranked keys {name} exceed the generation resident limit"
            )));
        }
        let terms = u64::try_from(dictionary.num_terms())
            .map_err(|_error| corrupt(&name, "term count overflow"))?;
        append(&mut out, &terms.to_le_bytes(), &name)?;
        let length_at = out.len();
        append(&mut out, &0_u64.to_le_bytes(), &name)?;
        let data_at = out.len();
        let mut offsets = Vec::new();
        offsets
            .try_reserve_exact(dictionary.num_terms().saturating_add(1))
            .map_err(|error| {
                CoreError::Storage(format!("lexical: reserve {name} offsets: {error}"))
            })?;
        offsets.push(0_u64);
        let mut stream = dictionary
            .stream()
            .map_err(|error| corrupt(&name, &format!("stream {column_name}: {error}")))?;
        let mut prior: Option<Range<usize>> = None;
        while stream.advance() {
            let key = stream.key();
            if std::str::from_utf8(key).is_err() {
                return Err(corrupt(&name, &format!("{column_name} is not UTF-8")));
            }
            if let Some(range) = &prior {
                let previous = out
                    .get(range.clone())
                    .ok_or_else(|| corrupt(&name, "previous key range is missing"))?;
                if previous >= key {
                    return Err(corrupt(
                        &name,
                        &format!("{column_name} is not strictly ordered"),
                    ));
                }
            }
            let start = out.len();
            append(&mut out, key, &name)?;
            prior = Some(start..out.len());
            let data_len = out
                .len()
                .checked_sub(data_at)
                .ok_or_else(|| corrupt(&name, "data length underflow"))?;
            offsets
                .push(u64::try_from(data_len).map_err(|_error| corrupt(&name, "offset overflow"))?);
        }
        if offsets.len() != dictionary.num_terms().saturating_add(1) {
            return Err(corrupt(
                &name,
                &format!("{column_name} term count disagrees with dictionary"),
            ));
        }
        let data_len = out
            .len()
            .checked_sub(data_at)
            .ok_or_else(|| corrupt(&name, "data length underflow"))?;
        let data_len =
            u64::try_from(data_len).map_err(|_error| corrupt(&name, "data length overflow"))?;
        let length_end = length_at
            .checked_add(8)
            .ok_or_else(|| corrupt(&name, "length offset overflow"))?;
        out.get_mut(length_at..length_end)
            .ok_or_else(|| corrupt(&name, "missing length slot"))?
            .copy_from_slice(&data_len.to_le_bytes());
        for offset in offsets {
            append(&mut out, &offset.to_le_bytes(), &name)?;
        }
    }
    Ok(out)
}

#[derive(Clone, Debug)]
struct FieldTable {
    data: Range<usize>,
    offsets: Range<usize>,
    terms: usize,
}

/// One proved file, borrowed without copying key strings during comparisons.
#[derive(Debug)]
pub(crate) struct SegmentKeys {
    id: SegmentId,
    bytes: Arc<Vec<u8>>,
    fields: [FieldTable; 3],
}

fn read_u64(bytes: &[u8], at: usize, name: &str) -> Result<u64, CoreError> {
    let end = at
        .checked_add(8)
        .ok_or_else(|| corrupt(name, "offset overflow"))?;
    let raw = bytes
        .get(at..end)
        .ok_or_else(|| corrupt(name, "truncated integer"))?;
    let mut value = [0_u8; 8];
    value.copy_from_slice(raw);
    Ok(u64::from_le_bytes(value))
}

impl SegmentKeys {
    pub(crate) fn decode(bytes: Vec<u8>, reader: &SegmentReader) -> Result<Self, CoreError> {
        let name = file_name(reader);
        if bytes.len() > MAX_RANKED_KEYS_BYTES || bytes.get(..8) != Some(MAGIC.as_slice()) {
            return Err(corrupt(&name, "bad magic or size"));
        }
        if bytes.get(8..40) != Some(reader.segment_id().uuid_string().as_bytes()) {
            return Err(corrupt(&name, "segment identity mismatch"));
        }
        let mut at = 40_usize;
        let mut fields = Vec::with_capacity(3);
        for column_name in COLUMNS {
            let terms = usize::try_from(read_u64(&bytes, at, &name)?)
                .map_err(|_error| corrupt(&name, "term count overflow"))?;
            at = at
                .checked_add(8)
                .ok_or_else(|| corrupt(&name, "header offset overflow"))?;
            let data_len = usize::try_from(read_u64(&bytes, at, &name)?)
                .map_err(|_error| corrupt(&name, "data length overflow"))?;
            at = at
                .checked_add(8)
                .ok_or_else(|| corrupt(&name, "header offset overflow"))?;
            let data_end = at
                .checked_add(data_len)
                .ok_or_else(|| corrupt(&name, "data overflow"))?;
            let count = terms
                .checked_add(1)
                .ok_or_else(|| corrupt(&name, "offset count overflow"))?;
            let offset_len = count
                .checked_mul(8)
                .ok_or_else(|| corrupt(&name, "offset bytes overflow"))?;
            let offset_end = data_end
                .checked_add(offset_len)
                .ok_or_else(|| corrupt(&name, "offset section overflow"))?;
            if bytes.get(at..offset_end).is_none() {
                return Err(corrupt(&name, "truncated key section"));
            }
            let field = FieldTable {
                data: at..data_end,
                offsets: data_end..offset_end,
                terms,
            };
            let column: StrColumn = reader
                .fast_fields()
                .str(column_name)
                .map_err(|error| corrupt(&name, &format!("open {column_name}: {error}")))?
                .ok_or_else(|| corrupt(&name, &format!("missing {column_name}")))?;
            if column.dictionary().num_terms() != terms {
                return Err(corrupt(
                    &name,
                    &format!("{column_name} ordinal count mismatch"),
                ));
            }
            let mut previous: Option<&[u8]> = None;
            let mut offset_at = data_end;
            for _index in 0..terms {
                let start = usize::try_from(read_u64(&bytes, offset_at, &name)?)
                    .map_err(|_error| corrupt(&name, "key offset overflow"))?;
                offset_at = offset_at
                    .checked_add(8)
                    .ok_or_else(|| corrupt(&name, "key offset position overflow"))?;
                let end = usize::try_from(read_u64(&bytes, offset_at, &name)?)
                    .map_err(|_error| corrupt(&name, "key offset overflow"))?;
                if start > end || end > data_len {
                    return Err(corrupt(&name, "key offset outside data"));
                }
                let key_start = at
                    .checked_add(start)
                    .ok_or_else(|| corrupt(&name, "key start overflow"))?;
                let key_end = at
                    .checked_add(end)
                    .ok_or_else(|| corrupt(&name, "key end overflow"))?;
                let key = bytes
                    .get(key_start..key_end)
                    .ok_or_else(|| corrupt(&name, "key outside data"))?;
                if std::str::from_utf8(key).is_err() {
                    return Err(corrupt(&name, "key is not UTF-8"));
                }
                if previous.is_some_and(|prior| prior >= key) {
                    return Err(corrupt(&name, "keys are not strictly ordered"));
                }
                previous = Some(key);
            }
            let last_offset = offset_end
                .checked_sub(8)
                .ok_or_else(|| corrupt(&name, "offset sentinel underflow"))?;
            if read_u64(&bytes, data_end, &name)? != 0
                || read_u64(&bytes, last_offset, &name)?
                    != u64::try_from(data_len)
                        .map_err(|_error| corrupt(&name, "data length overflow"))?
            {
                return Err(corrupt(&name, "offset sentinels disagree with data"));
            }
            fields.push(field);
            at = offset_end;
        }
        if at != bytes.len() {
            return Err(corrupt(&name, "trailing bytes"));
        }
        let fields: [FieldTable; 3] = fields
            .try_into()
            .map_err(|_error| corrupt(&name, "field count"))?;
        Ok(Self {
            id: reader.segment_id(),
            bytes: Arc::new(bytes),
            fields,
        })
    }

    pub(crate) fn get(&self, column: usize, ord: u64) -> Result<&str, CoreError> {
        let field = self
            .fields
            .get(column)
            .ok_or_else(|| corrupt("ranked key", "unknown column"))?;
        let index =
            usize::try_from(ord).map_err(|_error| corrupt("ranked key", "ordinal overflow"))?;
        if index >= field.terms {
            return Err(corrupt("ranked key", "ordinal outside table"));
        }
        let offset = index
            .checked_mul(8)
            .and_then(|bytes| field.offsets.start.checked_add(bytes))
            .ok_or_else(|| corrupt("ranked key", "offset position overflow"))?;
        let next_offset = offset
            .checked_add(8)
            .ok_or_else(|| corrupt("ranked key", "offset position overflow"))?;
        let start = usize::try_from(read_u64(&self.bytes, offset, "ranked key")?)
            .map_err(|_error| corrupt("ranked key", "offset overflow"))?;
        let end = usize::try_from(read_u64(&self.bytes, next_offset, "ranked key")?)
            .map_err(|_error| corrupt("ranked key", "offset overflow"))?;
        let key_start = field
            .data
            .start
            .checked_add(start)
            .ok_or_else(|| corrupt("ranked key", "key start overflow"))?;
        let key_end = field
            .data
            .start
            .checked_add(end)
            .ok_or_else(|| corrupt("ranked key", "key end overflow"))?;
        let key = self
            .bytes
            .get(key_start..key_end)
            .ok_or_else(|| corrupt("ranked key", "key outside table"))?;
        std::str::from_utf8(key).map_err(|error| corrupt("ranked key", &error.to_string()))
    }

    pub(crate) fn id(&self) -> SegmentId {
        self.id
    }
}

#[derive(Debug)]
pub(crate) struct RankedKeyTables {
    segments: Vec<Arc<SegmentKeys>>,
}

impl RankedKeyTables {
    /// Decoded heap footprint replacing the table files in the generation
    /// resident estimate. Include spare Vec capacity and both Arc headers.
    pub(crate) fn heap_bytes_estimate(&self) -> Result<u64, CoreError> {
        let word = std::mem::size_of::<usize>();
        let pointer_bytes = self
            .segments
            .capacity()
            .checked_mul(std::mem::size_of::<Arc<SegmentKeys>>())
            .ok_or_else(|| {
                CoreError::Storage("lexical: ranked-key heap estimate overflow".into())
            })?;
        let bytes = self.segments.iter().try_fold(pointer_bytes, |sum, table| {
            let segment = table
                .bytes
                .capacity()
                .checked_add(std::mem::size_of::<SegmentKeys>())
                .and_then(|value| value.checked_add(std::mem::size_of::<Vec<u8>>()))
                .and_then(|value| {
                    word.checked_mul(4)
                        .and_then(|header| value.checked_add(header))
                })
                .ok_or_else(|| {
                    CoreError::Storage("lexical: ranked-key heap estimate overflow".into())
                })?;
            sum.checked_add(segment).ok_or_else(|| {
                CoreError::Storage("lexical: ranked-key heap estimate overflow".into())
            })
        })?;
        u64::try_from(bytes).map_err(|_error| {
            CoreError::Storage("lexical: ranked-key heap estimate overflow".into())
        })
    }
    pub(crate) fn rebind(&self, readers: &[SegmentReader]) -> Result<Self, CoreError> {
        Self::bind(self.segments.clone(), readers)
    }
    pub(crate) fn bind(
        segments: Vec<Arc<SegmentKeys>>,
        readers: &[SegmentReader],
    ) -> Result<Self, CoreError> {
        if segments.len() != readers.len() {
            return Err(corrupt("generation", "ranked-key segment count mismatch"));
        }
        let mut by_id = BTreeMap::new();
        for table in segments {
            if by_id.insert(table.id().uuid_string(), table).is_some() {
                return Err(corrupt(
                    "generation",
                    "duplicate ranked-key segment identity",
                ));
            }
        }
        let mut ordered = Vec::new();
        ordered.try_reserve_exact(readers.len()).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reserve ranked-key segment bindings: {error}"
            ))
        })?;
        for reader in readers {
            let table = by_id
                .remove(&reader.segment_id().uuid_string())
                .ok_or_else(|| corrupt("generation", "ranked-key segment identity missing"))?;
            ordered.push(table);
        }
        Ok(Self { segments: ordered })
    }
    pub(crate) fn segment(
        &self,
        ordinal: usize,
        reader: &SegmentReader,
    ) -> Option<&Arc<SegmentKeys>> {
        self.segments
            .get(ordinal)
            .filter(|table| table.id() == reader.segment_id())
    }
}
