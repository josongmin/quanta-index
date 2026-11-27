//! The full ranked order of one exhaustive Text query, kept for later pages.
//!
//! A first page already traverses every candidate to report its exact
//! `matched`; capturing the eligible rows in rank order on the way lets a
//! caller answer later pages of the same view, member and query by a cursor
//! search and the projection of their own rows only. Each row keeps its
//! score, owner and representative `CodeSearch` match, never a projection.

use std::sync::Arc;

use quanta_index_core::index::{
    FiniteScore, IndexError, MemoryLease, MemoryLifetime, ObjectRef, OwnerKey, WorkBudget,
    WorkUsage,
};

use super::text_code_search_rank::CodeSearchMatch;

/// Admission from the session's separately reserved rank-order share.
/// Capacity refusal disables only this optional cache; ledger failure is hard.
pub trait TextRankMemoryPort: std::fmt::Debug + Send + Sync {
    fn try_reserve(self: Arc<Self>, bytes: u64) -> Result<Option<MemoryLease>, IndexError>;
}

/// The structural bound above which no order is captured: the owner bound
/// that already admits unanchored and source-predicate candidates.
pub const MAX_TEXT_RANK_ORDER: u64 = super::text_universe::MAX_UNANCHORED_DOCS;

#[expect(
    clippy::as_conversions,
    reason = "a type's size fits u64 on every supported target; usize::try_from is not const"
)]
const ENTRY_BYTES: u64 = std::mem::size_of::<TextRankEntry>() as u64;

/// The heap bytes of the largest order a capture keeps.
pub const MAX_TEXT_RANK_ORDER_BYTES: u64 = MAX_TEXT_RANK_ORDER * ENTRY_BYTES;

/// Rows are reserved in chunks of this many as the capture grows.
const CAPTURE_CHUNK: u64 = 256;

/// One ranked row: what a page cursor and a projection need.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextRankEntry {
    pub score: FiniteScore,
    pub owner: OwnerKey,
    pub code_search: Option<CodeSearchMatch>,
}

/// Every eligible row of one query in page order: score descending, then
/// owner ascending (owner pages) or repository and path ascending (path
/// pages, one row per group, keyed by its representative owner).
#[derive(Debug)]
pub struct TextRankOrder {
    entries: Vec<TextRankEntry>,
    artifact: ObjectRef,
    path: bool,
    // The rows die before their allocation grants.
    memory: Vec<MemoryLease>,
}

impl TextRankOrder {
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn entries(&self) -> &[TextRankEntry] {
        &self.entries
    }

    /// The selected member artifact the order was ranked from.
    #[must_use]
    pub const fn artifact(&self) -> ObjectRef {
        self.artifact
    }

    /// Whether the rows are `select:path` groups.
    #[must_use]
    pub const fn is_path(&self) -> bool {
        self.path
    }

    /// The heap bytes the rows retain.
    pub fn retained_bytes(&self) -> Result<u64, IndexError> {
        u64::try_from(self.entries.capacity())
            .map_err(|_width| IndexError::ResourceLimited)?
            .checked_mul(ENTRY_BYTES)
            .ok_or(IndexError::ResourceLimited)
    }

    /// Move the rows' custody to `memory`, which must cover them, releasing
    /// the grants they were captured under.
    pub fn with_memory(mut self, memory: MemoryLease) -> Result<Self, IndexError> {
        if memory.bytes() < self.retained_bytes()? {
            return Err(IndexError::ResourceLimited);
        }
        self.memory = vec![memory];
        Ok(self)
    }
}

/// A soft capture: above [`MAX_TEXT_RANK_ORDER`] rows, or when its memory is
/// refused, it is abandoned and the query is answered without an order.
pub(super) struct RankOrderCapture {
    entries: Vec<TextRankEntry>,
    memory: Vec<MemoryLease>,
    session_memory: Option<Arc<dyn TextRankMemoryPort>>,
    abandoned: bool,
}

impl RankOrderCapture {
    pub(super) fn new(session_memory: Option<Arc<dyn TextRankMemoryPort>>) -> Self {
        Self {
            entries: Vec::new(),
            memory: Vec::new(),
            session_memory,
            abandoned: false,
        }
    }

    fn abandon(&mut self) {
        self.abandoned = true;
        self.entries = Vec::new();
        self.memory = Vec::new();
    }

    pub(super) fn push(
        &mut self,
        entry: TextRankEntry,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.abandoned {
            return Ok(());
        }
        if self.entries.len() == self.entries.capacity() {
            let capacity = u64::try_from(self.entries.capacity())
                .map_err(|_width| IndexError::ResourceLimited)?;
            let grown = capacity
                .saturating_add(CAPTURE_CHUNK)
                .min(MAX_TEXT_RANK_ORDER);
            let added = grown.saturating_sub(capacity);
            if added == 0 {
                self.abandon();
                return Ok(());
            }
            let bytes = added
                .checked_mul(ENTRY_BYTES)
                .ok_or(IndexError::ResourceLimited)?;
            let added = usize::try_from(added).map_err(|_width| IndexError::ResourceLimited)?;
            // Session captures use only the already-admitted rank-order
            // share. They never consume the page's work or memory budget.
            // Direct lexical tests retain the original budgeted path.
            let lease = if let Some(port) = &self.session_memory {
                budget.checkpoint()?;
                Arc::clone(port).try_reserve(bytes)?
            } else {
                budget.charge(WorkUsage {
                    bytes,
                    ..WorkUsage::default()
                })?;
                match budget.reserve_memory(bytes, MemoryLifetime::Retained) {
                    Ok(lease) => lease,
                    Err(IndexError::ResourceLimited) => {
                        self.abandon();
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                }
            };
            if self.session_memory.is_some() && lease.is_none() {
                self.abandon();
                return Ok(());
            }
            self.memory.extend(lease);
            if self.entries.try_reserve_exact(added).is_err() {
                self.abandon();
                return Ok(());
            }
            if u64::try_from(self.entries.capacity()).map_err(|_width| IndexError::ResourceLimited)?
                > grown
            {
                // An allocator may reserve beyond `try_reserve_exact`'s
                // request. Do not retain capacity the share did not fund.
                self.abandon();
                return Ok(());
            }
        }
        self.entries.push(entry);
        Ok(())
    }

    /// The captured rows, already in page order.
    pub(super) fn finish(self, artifact: ObjectRef, path: bool) -> Option<TextRankOrder> {
        (!self.abandoned).then_some(TextRankOrder {
            entries: self.entries,
            artifact,
            path,
            memory: self.memory,
        })
    }

    /// Sort owner-ranked rows into page order.
    pub(super) fn sort_by(
        &mut self,
        compare: impl FnMut(&TextRankEntry, &TextRankEntry) -> std::cmp::Ordering,
    ) {
        self.entries.sort_unstable_by(compare);
    }
}
