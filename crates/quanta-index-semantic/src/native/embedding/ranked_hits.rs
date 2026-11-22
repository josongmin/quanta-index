//! Bounded top-k ranking over the backing returned to the search caller.

use super::*;

pub(super) fn compare_hits(left: &SearchHit, right: &SearchHit) -> Ordering {
    right
        .cosine
        .total_cmp(&left.cosine)
        .then_with(|| {
            left.source_file
                .as_ref()
                .map(|file| file.owner)
                .cmp(&right.source_file.as_ref().map(|file| file.owner))
        })
        .then_with(|| left.owner.cmp(&right.owner))
        .then_with(|| left.vector_id.cmp(&right.vector_id))
}

/// A max-heap whose root is the worst retained hit. SearchHit's public
/// equality includes provenance, so the ordering stays local to this heap.
pub(super) struct RankedHits {
    hits: Vec<SearchHit>,
    memory: Option<MemoryLease>,
    limit: usize,
}

impl RankedHits {
    pub(super) fn for_query(
        query: &SearchQuery<'_>,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        // Every ranked durable owner consumes a scan slot. Overlay
        // documents share those slots and only live text can yield hits.
        let owner_limit = if query.coverage.owners == MapRoot(None) {
            let live_overlay = query.overlay.as_ref().map_or(0, |overlay| {
                overlay
                    .documents
                    .iter()
                    .filter(|document| document.text.is_some())
                    .count()
            });
            query
                .max_owners
                .min(u32::try_from(live_overlay).map_err(|_error| IndexError::ResourceLimited)?)
        } else {
            query.max_owners
        };
        // File discovery keeps one hit per source. The whole-file recipe
        // validates one vector per owner; other recipes validate max_vectors.
        let vectors_per_owner =
            if query.source_files || query.recipe.output_policy == whole_file_output_policy() {
                1
            } else {
                query.recipe.max_vectors
            };
        let limit = query
            .top_k
            .min(owner_limit.saturating_mul(vectors_per_owner));
        Self::reserve_window(
            usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?,
            budget,
        )
    }

    pub(super) fn reserve_window(limit: usize, budget: &WorkBudget) -> Result<Self, IndexError> {
        let mut window = Self {
            hits: Vec::new(),
            memory: None,
            limit,
        };
        // Fund scratch before candidate allocations go live. The window
        // does not claim escaping capacity before the result is known.
        // A smaller result can still fit if the upper bound is refused.
        if limit != 0
            && let Some(memory) = budget
                .try_reserve_spare_memory(Self::backing_bytes(limit)?, MemoryLifetime::Temporary)?
            && let Ok(hits) = Self::allocate(limit)
        {
            window.hits = hits;
            window.memory = Some(memory);
        }
        Ok(window)
    }

    pub(super) fn iter(&self) -> std::slice::Iter<'_, SearchHit> {
        self.hits.iter()
    }

    pub(super) fn worst(&self) -> Option<&SearchHit> {
        self.hits.first()
    }

    pub(super) fn is_full(&self) -> bool {
        self.hits.len() == self.limit
    }

    pub(super) fn remove(&mut self, index: usize) -> SearchHit {
        let hit = self.hits.swap_remove(index);
        if index < self.hits.len() && self.sift_up(index) == index {
            self.sift_down(index);
        }
        hit
    }

    pub(super) fn push(&mut self, hit: SearchHit, budget: &WorkBudget) -> Result<(), IndexError> {
        if self.is_full() {
            return Err(IndexError::BackendViolation(
                "semantic rank exceeded the result window".into(),
            ));
        }
        if self.hits.len() == self.hits.capacity() {
            self.grow(budget)?;
        }
        let index = self.hits.len();
        self.hits.push(hit);
        let _ranked_position = self.sift_up(index);
        Ok(())
    }

    fn grow(&mut self, budget: &WorkBudget) -> Result<(), IndexError> {
        let needed = self
            .hits
            .len()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let preferred = needed
            .checked_next_power_of_two()
            .ok_or(IndexError::ResourceLimited)?
            .min(self.limit);
        let spare = if preferred > needed {
            budget.try_reserve_spare_memory(
                Self::backing_bytes(preferred)?,
                MemoryLifetime::Temporary,
            )?
        } else {
            None
        };
        let (capacity, memory) = if let Some(memory) = spare {
            (preferred, Some(memory))
        } else {
            let memory =
                budget.reserve_memory(Self::backing_bytes(needed)?, MemoryLifetime::Temporary)?;
            (if memory.is_some() { needed } else { preferred }, memory)
        };
        // During growth both allocations are live. Keep the old charge until
        // its allocation is gone, even when the replacement cannot be funded.
        self.move_to(Self::allocate(capacity)?, memory);
        Ok(())
    }

    fn backing_bytes(capacity: usize) -> Result<u64, IndexError> {
        capacity
            .checked_mul(std::mem::size_of::<SearchHit>())
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(IndexError::ResourceLimited)
    }

    fn allocate(capacity: usize) -> Result<Vec<SearchHit>, IndexError> {
        let mut hits = Vec::new();
        hits.try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if hits.capacity() > capacity {
            return Err(IndexError::ResourceLimited);
        }
        Ok(hits)
    }

    fn sift_up(&mut self, mut index: usize) -> usize {
        while let Some(parent) = index.checked_sub(1).map(|previous| previous >> 1) {
            if !self.ranks_after(index, parent) {
                break;
            }
            self.hits.swap(index, parent);
            index = parent;
        }
        index
    }

    fn sift_down(&mut self, mut index: usize) {
        while let Some(left) = index
            .checked_mul(2)
            .and_then(|double| double.checked_add(1))
        {
            let larger = left
                .checked_add(1)
                .filter(|right| self.ranks_after(*right, left))
                .unwrap_or(left);
            if !self.ranks_after(larger, index) {
                break;
            }
            self.hits.swap(index, larger);
            index = larger;
        }
    }

    fn ranks_after(&self, left: usize, right: usize) -> bool {
        self.hits
            .get(left)
            .zip(self.hits.get(right))
            .is_some_and(|(left, right)| compare_hits(left, right).is_gt())
    }

    pub(super) fn into_sorted(
        mut self,
        budget: &WorkBudget,
    ) -> Result<(Vec<SearchHit>, Option<MemoryLease>), IndexError> {
        if self.hits.is_empty() {
            return Ok((Vec::new(), None));
        }
        let len = self.hits.len();
        let bytes = Self::backing_bytes(len)?;
        // Return only the used capacity when compaction is affordable.
        // Otherwise let the same allocation escape if its charge can move.
        let compact = if len < self.hits.capacity() {
            budget.try_reserve_spare_memory(bytes, MemoryLifetime::Retained)?
        } else {
            None
        };
        if let Some(memory) = compact
            && let Ok(hits) = Self::allocate(len)
        {
            self.move_to(hits, Some(memory));
        } else if let Some(memory) = self.memory.as_mut()
            && !memory.try_retain()?
        {
            // A refused or unsupported promotion leaves the original
            // charge in place. Reserve only the exact outgoing size.
            let retained = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
            self.move_to(Self::allocate(len)?, retained);
        }
        self.hits.sort_unstable_by(compare_hits);
        Ok((self.hits, self.memory))
    }

    fn move_to(&mut self, mut hits: Vec<SearchHit>, memory: Option<MemoryLease>) {
        hits.append(&mut self.hits);
        self.hits = hits;
        self.memory = memory;
    }
}
