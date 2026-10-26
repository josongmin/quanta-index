//! Data-only custody for the single canonical multi-root map output.

use std::{
    ops::{Deref, Range},
    sync::Arc,
};

use super::{IndexError, MapRoot, MemoryLease, MemoryLifetime, WorkBudget};

#[derive(Clone, Debug, Default)]
pub struct StagedMapRoots {
    inner: Option<Arc<MapRootData>>,
}

#[derive(Debug)]
struct MapRootData {
    roots: Vec<MapRoot>,
    // Destroy backing before releasing the last shared charge.
    _memory: Option<MemoryLease>,
}

impl StagedMapRoots {
    #[must_use]
    pub fn as_slice(&self) -> &[MapRoot] {
        self.inner
            .as_ref()
            .map_or(&[], |data| data.roots.as_slice())
    }
    /// Includes requested array capacity, shared value and Arc-header allowance.
    /// Logical admission does not claim a physical allocator/RSS ceiling.
    pub fn allocation_bytes(count: usize) -> Result<u64, IndexError> {
        if count == 0 {
            return Ok(0);
        }
        let bytes = count
            .checked_mul(std::mem::size_of::<MapRoot>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<MapRootData>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[usize; 2]>()))
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
    }

    /// The producing port admits backing before either Vec or Arc birth.
    pub fn prepare(count: usize, budget: &WorkBudget) -> Result<MapRootBuilder, IndexError> {
        budget.checkpoint()?;
        let bytes = Self::allocation_bytes(count)?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Retained)?
        };
        let mut roots = Vec::new();
        roots
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(MapRootBuilder {
            roots,
            expected: count,
            memory,
        })
    }
}

impl Deref for StagedMapRoots {
    type Target = [MapRoot];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl PartialEq for StagedMapRoots {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl Eq for StagedMapRoots {}

/// A fixed-count output cannot grow or escape without its original admission.
#[derive(Debug)]
pub struct MapRootBuilder {
    roots: Vec<MapRoot>,
    expected: usize,
    memory: Option<MemoryLease>,
}

impl MapRootBuilder {
    pub fn push(&mut self, root: MapRoot) -> Result<(), IndexError> {
        if self.roots.len() == self.expected || self.roots.len() == self.roots.capacity() {
            return Err(IndexError::BackendViolation(
                "map root output exceeds input ordinals".into(),
            ));
        }
        self.roots.push(root);
        Ok(())
    }

    pub fn finish(self) -> Result<StagedMapRoots, IndexError> {
        if self.roots.len() != self.expected {
            return Err(IndexError::BackendViolation(
                "map root output lost input ordinals".into(),
            ));
        }
        if self.expected == 0 {
            return Ok(StagedMapRoots::default());
        }
        Ok(StagedMapRoots {
            inner: Some(Arc::new(MapRootData {
                roots: self.roots,
                _memory: self.memory,
            })),
        })
    }
}

/// Iteration owns the same shared backing even after the original output drops.
#[derive(Debug)]
pub struct StagedMapRootIter {
    roots: StagedMapRoots,
    indices: Range<usize>,
}
impl Iterator for StagedMapRootIter {
    type Item = MapRoot;
    fn next(&mut self) -> Option<Self::Item> {
        self.roots.get(self.indices.next()?).copied()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}
impl DoubleEndedIterator for StagedMapRootIter {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.roots.get(self.indices.next_back()?).copied()
    }
}
impl ExactSizeIterator for StagedMapRootIter {}
impl std::iter::FusedIterator for StagedMapRootIter {}

impl IntoIterator for StagedMapRoots {
    type Item = MapRoot;
    type IntoIter = StagedMapRootIter;
    fn into_iter(self) -> Self::IntoIter {
        StagedMapRootIter {
            indices: 0..self.len(),
            roots: self,
        }
    }
}
impl<'a> IntoIterator for &'a StagedMapRoots {
    type Item = &'a MapRoot;
    type IntoIter = std::slice::Iter<'a, MapRoot>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{
        Digest, MemoryLeaseOwner, MemoryScope, MemoryScopePort, ObjectRef, WorkLimits,
    };
    use std::{
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    struct Tracker {
        cap: u64,
        live: AtomicU64,
        calls: AtomicU64,
        invalid: AtomicBool,
    }
    impl MemoryLeaseOwner for Tracker {
        fn release(&self, bytes: u64) {
            if self
                .live
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_sub(bytes)
                })
                .is_err()
            {
                self.invalid.store(true, Ordering::Relaxed);
            }
        }
    }
    struct Scope {
        tracker: Arc<Tracker>,
        cancel: Option<crate::RequestBudgetV1>,
    }
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            require(lifetime == MemoryLifetime::Retained)?;
            let _prior = self.tracker.calls.fetch_add(1, Ordering::Relaxed);
            let _prior = self
                .tracker
                .live
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_add(bytes)
                        .filter(|value| *value <= self.tracker.cap)
                })
                .map_err(|_error| IndexError::ResourceLimited)?;
            if let Some(request) = &self.cancel {
                request.cancel_handle().cancel();
            }
            Ok(MemoryLease::new(bytes, self.tracker.clone()))
        }
    }
    fn require(condition: bool) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "map root custody law failed".into(),
            ))
        }
    }
    fn work(cap: u64, cancel: bool) -> Result<(WorkBudget, Arc<Tracker>), IndexError> {
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let tracker = Arc::new(Tracker {
            cap,
            live: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            invalid: AtomicBool::new(false),
        });
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope {
            tracker: tracker.clone(),
            cancel: cancel.then(|| budget.request().clone()),
        }))));
        Ok((budget, tracker))
    }

    #[test]
    fn null_alias_order_shared_clone_and_consuming_iteration_keep_one_charge()
    -> Result<(), IndexError> {
        let one = MapRoot(Some(ObjectRef(Digest([1; 32]))));
        let gold = [MapRoot(None), one, one];
        let required = StagedMapRoots::allocation_bytes(3)?;
        let (budget, tracker) = work(required, false)?;
        let mut builder = StagedMapRoots::prepare(3, &budget)?;
        for root in gold {
            builder.push(root)?;
        }
        let roots = builder.finish()?;
        require(*roots == gold)?;
        let alias = roots.clone();
        require(std::ptr::eq(roots.as_ptr(), alias.as_ptr()))?;
        require(tracker.calls.load(Ordering::Relaxed) == 1)?;
        let mut iterator = alias.into_iter();
        drop(roots);
        drop(budget);
        require(iterator.len() == 3 && tracker.live.load(Ordering::Relaxed) == required)?;
        require(iterator.next() == Some(MapRoot(None)))?;
        require(iterator.next_back() == Some(one) && iterator.len() == 1)?;
        require(iterator.next() == Some(one) && iterator.next().is_none())?;
        require(iterator.next_back().is_none())?;
        require(tracker.live.load(Ordering::Relaxed) == required)?;
        drop(iterator);
        require(
            tracker.live.load(Ordering::Relaxed) == 0 && !tracker.invalid.load(Ordering::Relaxed),
        )
    }

    #[test]
    fn output_floor_overflow_and_original_cancel_refuse_before_backing_birth()
    -> Result<(), IndexError> {
        let minimum = StagedMapRoots::allocation_bytes(2)?;
        let (budget, tracker) = work(minimum - 1, false)?;
        require(matches!(
            StagedMapRoots::prepare(2, &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(
            tracker.calls.load(Ordering::Relaxed) == 1 && tracker.live.load(Ordering::Relaxed) == 0,
        )?;
        let (budget, tracker) = work(u64::MAX, false)?;
        require(matches!(
            StagedMapRoots::prepare(usize::MAX, &budget),
            Err(IndexError::ResourceLimited)
        ))?;
        require(tracker.calls.load(Ordering::Relaxed) == 0)?;
        let (budget, tracker) = work(minimum, true)?;
        require(matches!(
            StagedMapRoots::prepare(2, &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(
            tracker.calls.load(Ordering::Relaxed) == 1 && tracker.live.load(Ordering::Relaxed) == 0,
        )?;
        let (budget, tracker) = work(minimum, false)?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            StagedMapRoots::prepare(2, &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(
            tracker.calls.load(Ordering::Relaxed) == 0 && !tracker.invalid.load(Ordering::Relaxed),
        )
    }

    #[test]
    fn missing_extra_empty_and_discarded_builders_preserve_counts_and_refund()
    -> Result<(), IndexError> {
        let (budget, tracker) = work(u64::MAX, false)?;
        let mut incomplete = StagedMapRoots::prepare(2, &budget)?;
        incomplete.push(MapRoot(None))?;
        require(matches!(
            incomplete.finish(),
            Err(IndexError::BackendViolation(_))
        ))?;
        require(tracker.live.load(Ordering::Relaxed) == 0)?;
        let mut exact = StagedMapRoots::prepare(1, &budget)?;
        exact.push(MapRoot(None))?;
        require(matches!(
            exact.push(MapRoot(None)),
            Err(IndexError::BackendViolation(_))
        ))?;
        let roots = exact.finish()?;
        require(roots.len() == 1)?;
        drop(roots);
        let discarded = StagedMapRoots::prepare(2, &budget)?;
        drop(discarded);
        let before_empty = tracker.calls.load(Ordering::Relaxed);
        let mut empty = StagedMapRoots::prepare(0, &budget)?;
        require(matches!(
            empty.push(MapRoot(None)),
            Err(IndexError::BackendViolation(_))
        ))?;
        require(empty.finish()?.is_empty())?;
        require(tracker.calls.load(Ordering::Relaxed) == before_empty)?;
        require(
            tracker.live.load(Ordering::Relaxed) == 0 && !tracker.invalid.load(Ordering::Relaxed),
        )
    }
}
