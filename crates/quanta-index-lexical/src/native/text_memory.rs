//! Fund canonical JSON output before birth and retain its charge until adoption.

use std::io::{self, Write};

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget};
use serde::Serialize;

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
            rows.append(&mut self.rows);
            self.rows = rows;
            self.memory = memory;
        } else {
            budget.checkpoint()?;
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
        case: quanta_index_lq_text_normalizer::CaseMode,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        use quanta_index_lq_text_normalizer::{CaseMode, case_chars};

        let length = case_chars(term, case).try_fold(1usize, |length, ch| {
            length
                .checked_add(ch.len_utf8())
                .ok_or(IndexError::ResourceLimited)
        })?;
        if length < 2
            || u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?
                > super::text_spool::MAX_TERM_KEY_BYTES
        {
            return Err(IndexError::ResourceLimited);
        }
        let memory = budget.reserve_memory(
            u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_error| IndexError::ResourceLimited)?;
        bytes.push(match case {
            CaseMode::Sensitive => 0,
            CaseMode::Folded => 1,
        });
        for ch in case_chars(term, case) {
            bytes.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
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
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bytes.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        owned.extend_from_slice(bytes);
        Ok(Self {
            bytes: owned,
            memory,
        })
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

pub(super) fn encode<T: Serialize>(
    value: &T,
    budget: &WorkBudget,
) -> Result<PreparedPayload, IndexError> {
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
    let length = count.written;
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
