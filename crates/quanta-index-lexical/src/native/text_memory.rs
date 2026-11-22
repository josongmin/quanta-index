//! Fund Native Text preparation buffers and retain their charge until adoption.

use std::io::{self, Write};

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};
use quanta_index_lq_text_normalizer::{CaseMode, TokenCursor, case_chars};
use serde::Serialize;

pub(super) const MAX_TERM_KEY_BYTES: u64 = 1_024;
pub(super) const MAX_NFC_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const MAX_NFC_WORK_BYTES: u64 = 64 * 1024 * 1024;

pub(super) type IndexedTokenCursor<'a> = TokenCursor<'a, Option<MemoryLease>>;

pub(super) fn prepare_indexed_token_cursor<'a>(
    body: &'a str,
    budget: &mut WorkBudget,
) -> Result<IndexedTokenCursor<'a>, IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(body.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(4)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    TokenCursor::try_new_bounded(
        body,
        usize::try_from(MAX_NFC_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
        usize::try_from(MAX_NFC_WORK_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
        |_units| budget.checkpoint(),
        |bytes| {
            budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )
        },
    )
    .map_err(|error| {
        error
            .into_checkpoint()
            .map_or(IndexError::ResourceLimited, std::convert::identity)
    })
}

pub(super) fn check_term_key(key: &[u8]) -> Result<(), IndexError> {
    decode_case_term_key(key).map(|_| ())
}

pub(super) fn decode_case_term_key(key: &[u8]) -> Result<(CaseMode, &str), IndexError> {
    let invalid = || IndexError::CorruptObject("invalid native term key".into());
    if key.len() < 2
        || u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?
            > MAX_TERM_KEY_BYTES
    {
        return Err(invalid());
    }
    let (case, token) = match key.split_first() {
        Some((0, token)) => (CaseMode::Sensitive, token),
        Some((1, token)) => (CaseMode::Folded, token),
        _ => return Err(invalid()),
    };
    Ok((
        case,
        std::str::from_utf8(token).map_err(|_error| invalid())?,
    ))
}

/// Append a canonical term key within already funded backing. Publication
/// and query preparation use the same header and normalization bytes.
pub(super) fn append_canonical_case_key(
    keys: &mut Vec<u8>,
    text: &str,
    case: CaseMode,
    limit: usize,
) -> Result<(), IndexError> {
    let start = keys.len();
    let capacity = keys.capacity().min(limit);
    let append = |keys: &mut Vec<u8>, bytes: &[u8]| -> Result<(), IndexError> {
        let end = keys
            .len()
            .checked_add(bytes.len())
            .ok_or(IndexError::ResourceLimited)?;
        if end > capacity {
            return Err(IndexError::BackendViolation(
                "canonical case key exceeded its funded census".into(),
            ));
        }
        keys.extend_from_slice(bytes);
        Ok(())
    };
    append(
        keys,
        &[match case {
            CaseMode::Sensitive => 0,
            CaseMode::Folded => 1,
        }],
    )?;
    match case {
        CaseMode::Sensitive => append(keys, text.as_bytes())?,
        CaseMode::Folded if text.is_ascii() => {
            let end = keys
                .len()
                .checked_add(text.len())
                .ok_or(IndexError::ResourceLimited)?;
            if end > capacity {
                return Err(IndexError::BackendViolation(
                    "canonical folded key exceeded its funded census".into(),
                ));
            }
            keys.extend(text.bytes().map(|byte| byte.to_ascii_lowercase()));
        }
        CaseMode::Folded => {
            let mut encoded = [0; 4];
            for ch in case_chars(text, case) {
                append(keys, ch.encode_utf8(&mut encoded).as_bytes())?;
            }
        }
    }
    let _decoded = decode_case_term_key(keys.get(start..).ok_or(IndexError::ResourceLimited)?)?;
    Ok(())
}

/// Bounded private row backing owns its admission through reuse and growth.
pub(super) struct PreparedRows<T> {
    rows: Vec<T>,
    maximum: usize,
    memory: Option<MemoryLease>,
}

impl<T> PreparedRows<T> {
    pub(super) fn new(maximum: usize) -> Result<Self, IndexError> {
        if maximum == 0 || std::mem::size_of::<T>() == 0 {
            return Err(IndexError::InvalidInput(
                "invalid preparation row envelope".into(),
            ));
        }
        Ok(Self {
            rows: Vec::new(),
            maximum,
            memory: None,
        })
    }

    pub(super) fn with_capacity(maximum: usize, budget: &WorkBudget) -> Result<Self, IndexError> {
        let mut result = Self::new(maximum)?;
        result.reserve(maximum, budget)?;
        Ok(result)
    }

    fn reserve(&mut self, capacity: usize, budget: &WorkBudget) -> Result<(), IndexError> {
        let bytes = capacity
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if rows.capacity() > capacity {
            return Err(IndexError::ResourceLimited);
        }
        rows.append(&mut self.rows);
        self.rows = rows;
        self.memory = memory;
        Ok(())
    }

    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(super) fn as_slice(&self) -> &[T] {
        &self.rows
    }

    pub(super) fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.rows
    }

    pub(super) fn push(&mut self, row: T, budget: &WorkBudget) -> Result<(), IndexError> {
        let needed = self
            .rows
            .len()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if needed > self.maximum {
            return Err(IndexError::ResourceLimited);
        }
        if needed > self.rows.capacity() {
            let capacity = self
                .rows
                .capacity()
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?
                .max(needed)
                .clamp(self.maximum.min(8), self.maximum);
            self.reserve(capacity, budget)?;
        } else {
            budget.checkpoint()?;
        }
        self.rows.push(row);
        Ok(())
    }

    /// Append to a fully funded backing while a discardable cache slot is locked.
    /// This cannot reserve memory or call back into that slot.
    pub(super) fn push_preallocated(&mut self, row: T) -> Result<(), IndexError> {
        if self.rows.len() >= self.maximum || self.rows.len() == self.rows.capacity() {
            return Err(IndexError::BackendViolation(
                "prepared rows exceeded preallocated capacity".into(),
            ));
        }
        self.rows.push(row);
        Ok(())
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
    }

    /// Move one scalar row while retaining the admitted backing for reuse.
    pub(super) fn pop(&mut self) -> Option<T> {
        self.rows.pop()
    }
}

/// A private preparation key owns its allocation, never a naked escaping copy.
#[derive(Debug)]
pub(super) struct PreparedKey {
    bytes: Vec<u8>,
    memory: Option<MemoryLease>,
}

impl PreparedKey {
    /// The caller has already validated this one NFC token. Case conversion is
    /// the canonical normalizer's iterator; census and write use the same bytes.
    pub(super) fn from_case_term(
        term: &str,
        case: CaseMode,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        let length = if case == CaseMode::Sensitive || term.is_ascii() {
            term.len()
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?
        } else {
            case_chars(term, case).try_fold(1usize, |length, ch| {
                length
                    .checked_add(ch.len_utf8())
                    .ok_or(IndexError::ResourceLimited)
            })?
        };
        let key_bytes = u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?;
        if !(2..=MAX_TERM_KEY_BYTES).contains(&key_bytes) {
            return Err(IndexError::ResourceLimited);
        }
        let memory = budget.reserve_memory(key_bytes, MemoryLifetime::Temporary)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_error| IndexError::ResourceLimited)?;
        append_canonical_case_key(&mut bytes, term, case, length)?;
        if bytes.len() != length {
            return Err(IndexError::BackendViolation(
                "native term key census changed".into(),
            ));
        }
        budget.checkpoint()?;
        Ok(Self { bytes, memory })
    }

    pub(super) fn copy(bytes: &[u8], budget: &WorkBudget) -> Result<Self, IndexError> {
        if bytes.is_empty() {
            return Err(IndexError::CorruptObject(
                "empty native preparation key".into(),
            ));
        }
        let memory = budget.reserve_memory(
            u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        Self::copy_with_memory(bytes, memory, budget)?.ok_or(IndexError::ResourceLimited)
    }

    /// Adopt an exact optional grant as the owned key's actual allocation.
    pub(super) fn copy_with_memory(
        bytes: &[u8],
        memory: Option<MemoryLease>,
        budget: &WorkBudget,
    ) -> Result<Option<Self>, IndexError> {
        budget.checkpoint()?;
        if bytes.is_empty() {
            return Err(IndexError::CorruptObject(
                "empty native preparation key".into(),
            ));
        }
        let expected = u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?;
        if memory
            .as_ref()
            .is_some_and(|grant| grant.bytes() != expected)
        {
            return Err(IndexError::BackendViolation(
                "native preparation key grant differs from its output".into(),
            ));
        }
        let mut owned = Vec::new();
        if owned.try_reserve_exact(bytes.len()).is_err() {
            return Ok(None);
        }
        let capacity =
            u64::try_from(owned.capacity()).map_err(|_error| IndexError::ResourceLimited)?;
        if memory
            .as_ref()
            .is_some_and(|grant| grant.bytes() < capacity)
        {
            return Ok(None);
        }
        owned.extend_from_slice(bytes);
        Ok(Some(Self {
            bytes: owned,
            memory,
        }))
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    /// A bounded SQL-write packet retains the grant alongside the canonical edit.
    pub(super) fn into_parts(self) -> (Vec<u8>, Option<MemoryLease>) {
        (self.bytes, self.memory)
    }
}

pub(super) struct PreparedPayload {
    bytes: Vec<u8>,
    // Data drops before its allocation charge on all failure/unwind paths.
    memory: Option<MemoryLease>,
}

impl PreparedPayload {
    pub(super) fn from_staged(
        payload: quanta_index_core::index::StagedPayload,
    ) -> Result<Self, IndexError> {
        let capacity = u64::try_from(payload.bytes.capacity())
            .map_err(|_error| IndexError::ResourceLimited)?;
        if payload
            .memory
            .as_ref()
            .is_some_and(|grant| grant.bytes() < capacity)
        {
            return Err(IndexError::BackendViolation(
                "staged text payload exceeds its allocation grant".into(),
            ));
        }
        Ok(Self {
            bytes: payload.bytes,
            memory: payload.memory,
        })
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    /// The native write batch takes custody of both parts in the same operation.
    pub(super) fn into_parts(self) -> (Vec<u8>, Option<MemoryLease>) {
        (self.bytes, self.memory)
    }
}

struct JsonWriter<'a> {
    output: Option<&'a mut Vec<u8>>,
    written: usize,
    limit: usize,
    budget: &'a WorkBudget,
    refusal: Option<IndexError>,
}

impl JsonWriter<'_> {
    fn fail(&mut self, error: IndexError) -> io::Error {
        self.refusal = Some(error);
        io::Error::other("native JSON allocation or request refused")
    }

    fn serialization_error(&mut self, error: &serde_json::Error) -> IndexError {
        self.refusal
            .take()
            .unwrap_or_else(|| IndexError::InvalidInput(error.to_string()))
    }
}

impl Write for JsonWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Err(error) = self.budget.checkpoint() {
            return Err(self.fail(error));
        }
        let Some(next) = self
            .written
            .checked_add(bytes.len())
            .filter(|next| *next <= self.limit)
        else {
            return Err(self.fail(IndexError::ResourceLimited));
        };
        if let Some(output) = &mut self.output {
            output.extend_from_slice(bytes);
        }
        self.written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn measured_encoded_len<T: Serialize>(
    value: &T,
    budget: &WorkBudget,
) -> Result<usize, IndexError> {
    let mut count = JsonWriter {
        output: None,
        written: 0,
        limit: usize::MAX,
        budget,
        refusal: None,
    };
    if let Err(error) = serde_json::to_writer(&mut count, value) {
        return Err(count.serialization_error(&error));
    }
    Ok(count.written)
}

pub(super) fn encode<T: Serialize>(
    value: &T,
    budget: &WorkBudget,
) -> Result<PreparedPayload, IndexError> {
    let length = measured_encoded_len(value, budget)?;
    let memory = budget.reserve_memory(
        u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut output = JsonWriter {
        output: Some(&mut bytes),
        written: 0,
        limit: length,
        budget,
        refusal: None,
    };
    if let Err(error) = serde_json::to_writer(&mut output, value) {
        return Err(output.serialization_error(&error));
    }
    if output.written != length {
        return Err(IndexError::BackendViolation(
            "native JSON serialization census changed".into(),
        ));
    }
    Ok(PreparedPayload { bytes, memory })
}

#[cfg(test)]
mod case_key_tests {
    use std::time::{Duration, Instant};

    use quanta_index_core::index::WorkLimits;

    use super::*;

    #[test]
    fn query_and_published_term_keys_keep_the_same_funded_bytes() -> Result<(), IndexError> {
        let budget = WorkBudget::new(
            WorkLimits {
                reads: 10,
                writes: 10,
                bytes: 4096,
                mutations: 10,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let cases: [(&str, CaseMode, &[u8]); 5] = [
            ("FoO", CaseMode::Sensitive, b"\0FoO"),
            ("FoO", CaseMode::Folded, b"\x01foo"),
            ("CAFÉ", CaseMode::Sensitive, "\0CAFÉ".as_bytes()),
            ("ΟΔΟΣ", CaseMode::Folded, "\x01οδοσ".as_bytes()),
            ("İ", CaseMode::Folded, b"\x01i\xcc\x87"),
        ];
        for (term, case, expected) in cases {
            let mut keys = Vec::with_capacity(expected.len());
            append_canonical_case_key(&mut keys, term, case, expected.len())?;
            assert_eq!(keys.as_slice(), expected);
            assert_eq!(
                PreparedKey::from_case_term(term, case, &budget)?.as_slice(),
                expected
            );
            let (decoded_case, token) = decode_case_term_key(&keys)?;
            assert_eq!(decoded_case, case);
            let expected_token = expected.get(1..).ok_or(IndexError::ResourceLimited)?;
            assert_eq!(token.as_bytes(), expected_token);
        }

        let exact = PreparedKey::from_case_term(&"x".repeat(1023), CaseMode::Folded, &budget)?;
        assert_eq!(exact.as_slice().len(), 1024);
        assert!(matches!(
            PreparedKey::from_case_term(&"x".repeat(1024), CaseMode::Folded, &budget),
            Err(IndexError::ResourceLimited),
        ));
        let invalid_keys: [&[u8]; 3] = [b"\x01", b"\x02foo", b"\x01\xff"];
        for invalid in invalid_keys {
            assert!(matches!(
                decode_case_term_key(invalid),
                Err(IndexError::CorruptObject(_))
            ));
        }
        Ok(())
    }
}
