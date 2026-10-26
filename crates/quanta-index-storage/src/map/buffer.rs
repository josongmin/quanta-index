//! Growable map-local allocations admit the simultaneous old/new capacities.

use std::ops::{Deref, DerefMut};

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget};

pub(super) struct MapBuffer<T> {
    values: Vec<T>,
    maximum: usize,
    // Drop the allocation before its charge.
    memory: Option<MemoryLease>,
}

impl<T> MapBuffer<T> {
    pub(super) fn new(maximum: usize) -> Self {
        Self {
            values: Vec::new(),
            maximum,
            memory: None,
        }
    }

    pub(super) fn with_capacity(
        capacity: usize,
        maximum: usize,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        let mut buffer = Self::new(maximum);
        buffer.reserve(capacity, budget)?;
        Ok(buffer)
    }

    fn required_capacity(&self, additional: usize) -> Result<usize, IndexError> {
        let needed = self
            .values
            .len()
            .checked_add(additional)
            .ok_or(IndexError::ResourceLimited)?;
        if needed > self.maximum {
            return Err(IndexError::ResourceLimited);
        }
        if needed <= self.values.capacity() {
            return Ok(self.values.capacity());
        }
        self.values
            .capacity()
            .checked_mul(2)
            .map(|capacity| capacity.max(needed).min(self.maximum))
            .ok_or(IndexError::ResourceLimited)
    }

    fn reserve(&mut self, additional: usize, budget: &WorkBudget) -> Result<(), IndexError> {
        let capacity = self.required_capacity(additional)?;
        if capacity == self.values.capacity() {
            return budget.checkpoint();
        }
        let bytes = capacity
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        values.append(&mut self.values);
        self.values = values;
        self.memory = memory;
        Ok(())
    }

    pub(super) fn push(&mut self, value: T, budget: &WorkBudget) -> Result<(), IndexError> {
        self.reserve(1, budget)?;
        self.values.push(value);
        Ok(())
    }

    pub(super) fn pop(&mut self) -> Option<T> {
        self.values.pop()
    }

    pub(super) fn capacity_bytes(&self) -> Result<usize, IndexError> {
        self.values
            .capacity()
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(IndexError::ResourceLimited)
    }

    /// Allocation and old-element copying done by the next one-element growth.
    pub(super) fn growth_work_bytes(&self) -> Result<usize, IndexError> {
        let capacity = self.required_capacity(1)?;
        if capacity == self.values.capacity() {
            return Ok(0);
        }
        capacity
            .checked_add(self.values.len())
            .and_then(|elements| elements.checked_mul(std::mem::size_of::<T>()))
            .ok_or(IndexError::ResourceLimited)
    }

    pub(super) fn insert(
        &mut self,
        index: usize,
        value: T,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        if index > self.values.len() {
            return Err(IndexError::BackendViolation(
                "map buffer insertion index".into(),
            ));
        }
        self.reserve(1, budget)?;
        self.values.insert(index, value);
        Ok(())
    }

    pub(super) fn remove(&mut self, index: usize) -> Result<T, IndexError> {
        if index >= self.values.len() {
            return Err(IndexError::BackendViolation(
                "map buffer removal index".into(),
            ));
        }
        Ok(self.values.remove(index))
    }

    pub(super) fn truncate(&mut self, length: usize) {
        self.values.truncate(length);
    }

    pub(super) fn as_slice(&self) -> &[T] {
        &self.values
    }
}

impl<T: Clone> MapBuffer<T> {
    pub(super) fn copy(
        values: &[T],
        maximum: usize,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        let mut result = Self::new(maximum);
        result.extend_from_slice(values, budget)?;
        Ok(result)
    }

    pub(super) fn extend_from_slice(
        &mut self,
        values: &[T],
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        self.reserve(values.len(), budget)?;
        self.values.extend_from_slice(values);
        Ok(())
    }
}

impl<T> Deref for MapBuffer<T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl<T> DerefMut for MapBuffer<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    struct Tracker {
        limit: AtomicU64,
        live: AtomicU64,
        peak: AtomicU64,
    }
    impl MemoryLeaseOwner for Tracker {
        fn release(&self, bytes: u64) {
            let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
        }
    }
    struct Scope(Arc<Tracker>);
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Temporary {
                return Err(IndexError::BackendViolation(
                    "map buffer must be temporary".into(),
                ));
            }
            let prior = self
                .0
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_add(bytes)
                        .filter(|next| *next <= self.0.limit.load(Ordering::SeqCst))
                })
                .map_err(|_error| IndexError::ResourceLimited)?;
            let live = prior
                .checked_add(bytes)
                .ok_or(IndexError::ResourceLimited)?;
            let _prior = self.0.peak.fetch_max(live, Ordering::SeqCst);
            let owner = Arc::clone(&self.0);
            Ok(MemoryLease::new(bytes, owner))
        }
    }

    #[test]
    fn growth_admits_old_and_new_and_refusal_preserves_data() -> Result<(), IndexError> {
        let tracker = Arc::new(Tracker {
            limit: AtomicU64::new(5),
            live: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        });
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 10,
                writes: 10,
                bytes: 100,
                mutations: 10,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?,
        );
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(Arc::clone(
            &tracker,
        ))))));
        let mut bytes = MapBuffer::copy(&[1_u8, 2], 4, &work)?;
        if !matches!(bytes.push(3, &work), Err(IndexError::ResourceLimited))
            || bytes.as_slice() != [1, 2]
            || tracker.live.load(Ordering::SeqCst) != 2
        {
            return Err(IndexError::BackendViolation(
                "denied growth changed data or charge".into(),
            ));
        }
        tracker.limit.store(6, Ordering::SeqCst);
        bytes.push(3, &work)?;
        if bytes.as_slice() != [1, 2, 3]
            || tracker.live.load(Ordering::SeqCst) != 4
            || tracker.peak.load(Ordering::SeqCst) != 6
        {
            return Err(IndexError::BackendViolation(
                "growth omitted simultaneous capacities".into(),
            ));
        }
        bytes.truncate(1);
        if tracker.live.load(Ordering::SeqCst) != 4 {
            return Err(IndexError::BackendViolation(
                "truncation released live capacity".into(),
            ));
        }
        drop(bytes);
        if tracker.live.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "buffer charge survived last drop".into(),
            ));
        }
        Ok(())
    }
}
