//! Immutable canonical results and their data-only allocation custody.

use std::{
    alloc::Layout,
    ops::Deref,
    sync::{Arc, atomic::AtomicUsize},
};

use quanta_index_core::index::MemoryLease;

/// The selected standard Arc allocation contains its strong/weak counters
/// before the value. Include their alignment, apart from hit/lease Vec backing.
pub(super) fn shared_value_bytes<R>() -> Result<usize, quanta_index_core::index::IndexError> {
    Layout::new::<[AtomicUsize; 2]>()
        .extend(Layout::new::<ResultValue<R>>())
        .map(|(layout, _offset)| layout.pad_to_align().size())
        .map_err(|_error| quanta_index_core::index::IndexError::ResourceLimited)
}

#[derive(Debug)]
pub(super) struct ResultValue<R> {
    pub(super) result: R,
    // All owned result data drops before its grants, including partial mapping.
    pub(super) payload_memory: Vec<Option<MemoryLease>>,
    pub(super) _capacity_memory: Option<MemoryLease>,
}

/// Immutable sharing preserves one data allocation charge without retaining an
/// execution slot. Creating a copy of the underlying DTO requires new admission.
#[derive(Debug)]
pub struct ScopedNativeResult<R> {
    inner: Arc<ResultValue<R>>,
}

impl<R> ScopedNativeResult<R> {
    pub(super) fn new(value: ResultValue<R>) -> Self {
        Self {
            inner: Arc::new(value),
        }
    }
}

impl<R> Clone for ScopedNativeResult<R> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<R> Deref for ScopedNativeResult<R> {
    type Target = R;
    fn deref(&self) -> &Self::Target {
        &self.inner.result
    }
}

impl<R: PartialEq> PartialEq for ScopedNativeResult<R> {
    fn eq(&self, other: &Self) -> bool {
        self.inner.result == other.inner.result
    }
}
