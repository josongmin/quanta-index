//! Ranked-row buffers retain byte reservations across segment/merge/page moves.

use std::cmp::Ordering;
use std::ops::Deref;

use quanta_index_core::LexicalMemoryReservation;
use tantivy::TantivyError;

use super::{CollectionBudget, RankedRow};

/// Keep guards outside map elements, which drop before their containing nodes.
/// Declare this before a local map or after the map field in its owning struct.
pub(crate) struct CollectionMemory {
    guards: Vec<LexicalMemoryReservation>,
    buffer: Option<LexicalMemoryReservation>,
    budget: CollectionBudget,
}

impl CollectionMemory {
    pub(crate) fn new(budget: CollectionBudget) -> Self {
        Self {
            guards: Vec::new(),
            buffer: None,
            budget,
        }
    }

    /// Retain a reservation before the corresponding allocation is made.
    pub(crate) fn hold(&mut self, guard: LexicalMemoryReservation) -> tantivy::Result<()> {
        if self.guards.len() == self.guards.capacity() {
            let capacity = self
                .guards
                .capacity()
                .checked_mul(2)
                .map(|value| value.max(4))
                .ok_or_else(|| {
                    TantivyError::InvalidArgument("collection guard capacity overflow".to_string())
                })?;
            let bytes = capacity
                .checked_mul(std::mem::size_of::<LexicalMemoryReservation>())
                .ok_or_else(|| {
                    TantivyError::InvalidArgument("collection guard buffer overflow".to_string())
                })?;
            let replacement = self.budget.reserve_bytes(bytes)?;
            let additional = capacity.checked_sub(self.guards.len()).ok_or_else(|| {
                TantivyError::InternalError("collection guard capacity shrank".to_string())
            })?;
            self.guards.try_reserve_exact(additional).map_err(|error| {
                TantivyError::InvalidArgument(format!(
                    "collection guard allocation failed: {error}"
                ))
            })?;
            self.buffer = Some(replacement);
        }
        self.guards.push(guard);
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct RankedRows {
    // Declaration order matters: release the buffer before its reservation.
    rows: Vec<RankedRow>,
    memory: Option<LexicalMemoryReservation>,
    budget: Option<CollectionBudget>,
}

impl RankedRows {
    pub(crate) fn with_capacity(
        capacity: usize,
        budget: Option<CollectionBudget>,
    ) -> tantivy::Result<Self> {
        let memory = reserve_rows(budget.as_ref(), capacity)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(capacity).map_err(|error| {
            TantivyError::InvalidArgument(format!("ranked row allocation failed: {error}"))
        })?;
        Ok(Self {
            rows,
            memory,
            budget,
        })
    }

    /// Every collector knows this buffer's maximum length before allocation.
    /// Refuse a violated bound instead of silently growing an uncharged buffer.
    pub(crate) fn push(&mut self, row: RankedRow) -> tantivy::Result<()> {
        if self.rows.len() == self.rows.capacity() {
            return Err(TantivyError::InternalError(
                "ranked row buffer exceeded its reserved capacity".to_string(),
            ));
        }
        self.rows.push(row);
        Ok(())
    }

    pub(crate) fn sort_by(
        &mut self,
        compare: impl FnMut(&RankedRow, &RankedRow) -> Ordering,
    ) -> tantivy::Result<()> {
        // Rust 1.92 stable sort uses insertion sort through 20 rows, otherwise
        // at most max(len, SMALL_SORT_GENERAL_SCRATCH_LEN = 48) scratch rows.
        // Keep this bound synchronized with the pinned core slice sort owner.
        let scratch_rows = if self.rows.len() <= 20 {
            0
        } else {
            self.rows.len().max(48)
        };
        let _scratch = reserve_rows(self.budget.as_ref(), scratch_rows)?;
        self.rows.sort_by(compare);
        Ok(())
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.rows.truncate(len);
    }

    pub(crate) fn retain(&mut self, keep: impl FnMut(&RankedRow) -> bool) {
        self.rows.retain(keep);
    }
}

impl Deref for RankedRows {
    type Target = [RankedRow];

    fn deref(&self) -> &Self::Target {
        &self.rows
    }
}

pub(crate) struct RankedRowsIntoIter {
    // Retain the complete allocation lease even after yielding the last row;
    // Vec's iterator still owns its allocation until the iterator is dropped.
    inner: std::vec::IntoIter<RankedRow>,
    _memory: Option<LexicalMemoryReservation>,
}

impl IntoIterator for RankedRows {
    type Item = RankedRow;
    type IntoIter = RankedRowsIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        RankedRowsIntoIter {
            inner: self.rows.into_iter(),
            _memory: self.memory,
        }
    }
}

impl Iterator for RankedRowsIntoIter {
    type Item = RankedRow;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for RankedRowsIntoIter {}

fn reserve_rows(
    budget: Option<&CollectionBudget>,
    count: usize,
) -> tantivy::Result<Option<LexicalMemoryReservation>> {
    let Some(budget) = budget else {
        return Ok(None);
    };
    let bytes = count
        .checked_mul(std::mem::size_of::<RankedRow>())
        .ok_or_else(|| {
            TantivyError::InvalidArgument("ranked row buffer size overflow".to_string())
        })?;
    budget.reserve_bytes(bytes).map(Some)
}
