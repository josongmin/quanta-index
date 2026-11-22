//! Memory-funded canonical case keys for a bounded token batch.

use super::text_memory::{MAX_TERM_KEY_BYTES, append_canonical_case_key};
use quanta_index_core::index::{IndexError, WorkBudget};
use quanta_index_lq_text_normalizer::{CaseMode, case_chars};

pub(super) struct TokenRow {
    start: usize,
    end: usize,
    position: u32,
}

impl TokenRow {
    pub(super) const fn position(&self) -> u32 {
        self.position
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct SourceRun {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) position: u32,
}

impl SourceRun {
    fn text(self, normalized: &str) -> Result<&str, IndexError> {
        normalized.get(self.start..self.end).ok_or_else(|| {
            IndexError::BackendViolation("native token source range is invalid".into())
        })
    }
}

pub(super) struct TokenBatch {
    keys: Vec<u8>,
    rows: Vec<TokenRow>,
    // Both data allocations drop before their grants, including error/unwind.
    key_memory: Option<quanta_index_core::index::MemoryLease>,
    _row_memory: Option<quanta_index_core::index::MemoryLease>,
}

impl TokenBatch {
    pub(super) fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
            .checked_mul(std::mem::size_of::<TokenRow>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            keys: Vec::new(),
            rows,
            key_memory: None,
            _row_memory: memory,
        })
    }

    #[cfg(test)]
    pub(super) fn key_bytes(&self) -> &[u8] {
        &self.keys
    }

    #[cfg(test)]
    pub(super) fn row_capacity(&self) -> usize {
        self.rows.capacity()
    }

    pub(super) fn rows(&self) -> &[TokenRow] {
        &self.rows
    }

    pub(super) fn term(&self, row: &TokenRow) -> Result<&[u8], IndexError> {
        self.keys.get(row.start..row.end).ok_or_else(|| {
            IndexError::BackendViolation("native token arena range is invalid".into())
        })
    }

    pub(super) fn clear(&mut self) {
        self.rows.clear();
        self.keys.clear();
    }

    fn reserve_keys(&mut self, needed: usize, budget: &WorkBudget) -> Result<(), IndexError> {
        if needed <= self.keys.capacity() {
            return budget.checkpoint();
        }
        // The previous arena and its grant stay live through admission and
        // allocation. Exact census avoids worst-case overreservation.
        let memory = budget.reserve_memory(
            u64::try_from(needed).map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let mut keys = Vec::new();
        keys.try_reserve_exact(needed)
            .map_err(|_error| IndexError::ResourceLimited)?;
        // No old key bytes are needed after successful replacement admission.
        self.keys = keys;
        self.key_memory = memory;
        Ok(())
    }

    pub(super) fn prepare(
        &mut self,
        normalized: &str,
        runs: &[SourceRun],
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let row_count = runs
            .len()
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        if row_count == 0 || row_count > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            return Err(IndexError::ResourceLimited);
        }
        let maximum =
            usize::try_from(MAX_TERM_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?;
        let mut lengths = [0usize; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
        let mut slots = lengths.iter_mut();
        let mut needed = 0usize;
        for run in runs {
            budget.checkpoint()?;
            let text = run.text(normalized)?;
            if text.is_empty() {
                return Err(IndexError::ResourceLimited);
            }
            let ascii = text.is_ascii();
            for case in [CaseMode::Sensitive, CaseMode::Folded] {
                let length = if ascii {
                    text.len()
                        .checked_add(1)
                        .filter(|length| *length <= maximum)
                        .ok_or(IndexError::ResourceLimited)?
                } else {
                    case_chars(text, case).try_fold(1usize, |bytes, ch| {
                        bytes
                            .checked_add(ch.len_utf8())
                            .filter(|next| *next <= maximum)
                            .ok_or(IndexError::ResourceLimited)
                    })?
                };
                *slots.next().ok_or(IndexError::ResourceLimited)? = length;
                needed = needed
                    .checked_add(length)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        self.reserve_keys(needed, budget)?;
        self.clear();
        let mut lengths = lengths.into_iter();
        for run in runs {
            budget.checkpoint()?;
            let text = run.text(normalized)?;
            for case in [CaseMode::Sensitive, CaseMode::Folded] {
                let start = self.keys.len();
                let end = start
                    .checked_add(lengths.next().ok_or(IndexError::ResourceLimited)?)
                    .ok_or(IndexError::ResourceLimited)?;
                append_canonical_case_key(&mut self.keys, text, case, needed)?;
                if self.keys.len() != end {
                    return Err(IndexError::BackendViolation(
                        "native token key census changed".into(),
                    ));
                }
                self.rows.push(TokenRow {
                    start,
                    end,
                    position: run.position,
                });
            }
        }
        if self.keys.len() != needed || self.rows.len() != row_count {
            return Err(IndexError::BackendViolation(
                "native token packet census changed".into(),
            ));
        }
        Ok(())
    }
}
