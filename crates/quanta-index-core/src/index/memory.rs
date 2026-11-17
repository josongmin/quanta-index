//! Live allocation custody, separate from an execution slot's lifetime.
//!
//! An escaping allocation commits future floor/headroom capacity before it is
//! made. The scheduler moves its live charge when the execution guard ends;
//! retaining a lease must not retain CPU, IO or provider execution capacity.

use std::{
    fmt,
    ops::{Deref, DerefMut},
    sync::{Arc, atomic::AtomicUsize},
};

use super::{IndexError, WorkBudget};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryLifetime {
    /// Must be released before its execution guard. Retaining this charge after
    /// execution is a backend violation, not permission to bypass admission.
    Temporary,
    /// May outlive execution. Future retained capacity is admitted first.
    Retained,
}

/// Implemented by the existing reservation authority. One scope names one
/// actual execution reservation; a stale scope must refuse new allocations.
pub trait MemoryScopePort: Send + Sync {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError>;

    /// Best-effort observation of the original request's first mandatory
    /// refusal, before custody unwinds. Optional optimization misses do not
    /// call this hook. It must not alter admission or the returned failure.
    fn observe_refusal(&self, _bytes: u64, _lifetime: MemoryLifetime) {}

    /// Optional pre-effect credit backed by this same allocation authority.
    /// Older scopes keep the original allocation path.
    fn try_reserve_credit(
        &self,
        _limits: MemoryCreditLimits,
    ) -> Result<Option<MemoryCredit>, IndexError> {
        Ok(None)
    }
}

/// Maximum simultaneous data and lease backings in a scoped optimization.
/// A released child may reuse its credit while the scope remains open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryCreditLimits {
    pub temporary_data_bytes: u64,
    pub retained_data_bytes: u64,
    pub max_live_temporary_leases: u32,
    pub max_live_retained_leases: u32,
}

impl MemoryCreditLimits {
    pub fn validate(self) -> Result<(), IndexError> {
        if (self.temporary_data_bytes == 0) != (self.max_live_temporary_leases == 0)
            || (self.retained_data_bytes == 0) != (self.max_live_retained_leases == 0)
            || self.temporary_data_bytes == 0 && self.retained_data_bytes == 0
        {
            return Err(IndexError::InvalidInput(
                "memory credit needs nonzero data and live-lease slots".into(),
            ));
        }
        Ok(())
    }

    pub fn checked_add(self, other: Self) -> Result<Self, IndexError> {
        Ok(Self {
            temporary_data_bytes: self
                .temporary_data_bytes
                .checked_add(other.temporary_data_bytes)
                .ok_or(IndexError::ResourceLimited)?,
            retained_data_bytes: self
                .retained_data_bytes
                .checked_add(other.retained_data_bytes)
                .ok_or(IndexError::ResourceLimited)?,
            max_live_temporary_leases: self
                .max_live_temporary_leases
                .checked_add(other.max_live_temporary_leases)
                .ok_or(IndexError::ResourceLimited)?,
            max_live_retained_leases: self
                .max_live_retained_leases
                .checked_add(other.max_live_retained_leases)
                .ok_or(IndexError::ResourceLimited)?,
        })
    }
}

/// An authority closes unused credit; outstanding child leases remain owned.
pub trait MemoryCreditClosePort: Send + Sync {
    fn close(&self);
}

pub struct MemoryCredit {
    scope: MemoryScope,
    closer: Arc<dyn MemoryCreditClosePort>,
}

impl MemoryCredit {
    #[must_use]
    pub fn new(scope: MemoryScope, closer: Arc<dyn MemoryCreditClosePort>) -> Self {
        Self { scope, closer }
    }

    #[must_use]
    pub fn scope(&self) -> MemoryScope {
        self.scope.clone()
    }
}

impl Drop for MemoryCredit {
    fn drop(&mut self) {
        self.closer.close();
    }
}

/// Restores the parent's allocation scope before closing unused credit.
pub struct ScopedMemoryBudget<'a> {
    pub(super) budget: &'a mut WorkBudget,
    pub(super) previous: Option<MemoryScope>,
    pub(super) previous_optional: Option<MemoryScope>,
    pub(super) _credit: MemoryCredit,
}

impl Deref for ScopedMemoryBudget<'_> {
    type Target = WorkBudget;

    fn deref(&self) -> &Self::Target {
        self.budget
    }
}

impl DerefMut for ScopedMemoryBudget<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.budget
    }
}

impl Drop for ScopedMemoryBudget<'_> {
    fn drop(&mut self) {
        self.budget
            .restore_scoped_memory_credit(self.previous.take(), self.previous_optional.take());
    }
}

/// The last lease owner releases its exact charge. Implementations fail closed
/// on corrupted accounting; destructors cannot substitute a successful result.
pub trait MemoryLeaseOwner: Send + Sync {
    fn release(&self, bytes: u64);
}

#[derive(Clone)]
pub struct MemoryScope {
    owner: Arc<dyn MemoryScopePort>,
}

impl MemoryScope {
    #[must_use]
    pub fn new(owner: Arc<dyn MemoryScopePort>) -> Self {
        Self { owner }
    }

    pub fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if bytes == 0 {
            return Err(IndexError::InvalidInput(
                "live memory reservation must be nonzero".into(),
            ));
        }
        let lease = self.owner.reserve(bytes, lifetime)?;
        if lease.bytes() != bytes {
            return Err(IndexError::BackendViolation(
                "allocation lease differs from requested live memory".into(),
            ));
        }
        Ok(lease)
    }

    pub fn try_reserve_credit(
        &self,
        limits: MemoryCreditLimits,
    ) -> Result<Option<MemoryCredit>, IndexError> {
        limits.validate()?;
        self.owner.try_reserve_credit(limits)
    }

    pub(super) fn observe_refusal(&self, bytes: u64, lifetime: MemoryLifetime) {
        self.owner.observe_refusal(bytes, lifetime);
    }
}

impl fmt::Debug for MemoryScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryScope")
            .finish_non_exhaustive()
    }
}

struct Lease {
    bytes: u64,
    owner: Arc<dyn MemoryLeaseOwner>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.owner.release(self.bytes);
    }
}

/// A shared charge for one allocation. Clone this only with immutable sharing
/// of the same bytes; copying those bytes requires another admitted lease.
#[derive(Clone)]
pub struct MemoryLease {
    inner: Arc<Lease>,
}

impl MemoryLease {
    /// Payload layout of the shared lease backing allocated by [`Self::new`].
    /// An external reservation authority uses its canonical Arc layout
    /// calculation to admit this backing before constructing the lease. This
    /// excludes the separately owned authority and allocator metadata.
    #[must_use]
    pub fn allocation_payload_layout() -> std::alloc::Layout {
        std::alloc::Layout::new::<Lease>()
    }

    /// The Arc header and payload backing of one new lease. Clones share it.
    pub fn allocation_backing_layout() -> Result<std::alloc::Layout, IndexError> {
        Self::shared_allocation_backing_layout(Self::allocation_payload_layout())
    }

    /// Canonical Arc header and caller-owned payload layout. The same external
    /// authority must admit this complete backing before allocating the Arc.
    pub fn shared_allocation_backing_layout(
        payload: std::alloc::Layout,
    ) -> Result<std::alloc::Layout, IndexError> {
        std::alloc::Layout::new::<[AtomicUsize; 2]>()
            .extend(payload)
            .map(|(layout, _offset)| layout.pad_to_align())
            .map_err(|_error| IndexError::ResourceLimited)
    }

    /// The authority constructs this only after admitting the charge.
    #[must_use]
    pub fn new(bytes: u64, owner: Arc<dyn MemoryLeaseOwner>) -> Self {
        Self {
            inner: Arc::new(Lease { bytes, owner }),
        }
    }

    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.inner.bytes
    }
}

impl fmt::Debug for MemoryLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryLease")
            .field("bytes", &self.bytes())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod shared_backing_layout_tests_v1 {
    use super::*;
    #[test]
    fn shared_backing_admits_payload_larger_than_the_lease_v1() {
        #[repr(align(64))]
        struct LargerPayload([u8; 128]);
        let payload = std::alloc::Layout::new::<LargerPayload>();
        assert!(payload.size() > MemoryLease::allocation_payload_layout().size());
        let backing = MemoryLease::shared_allocation_backing_layout(payload)
            .expect("large aligned Arc backing");
        assert_eq!(backing.align(), payload.align());
        assert!(backing.size() >= payload.size() + 2 * std::mem::size_of::<usize>());
        assert_eq!(backing.size() % backing.align(), 0);
        assert_eq!(
            MemoryLease::allocation_backing_layout().expect("existing lease backing"),
            MemoryLease::shared_allocation_backing_layout(MemoryLease::allocation_payload_layout())
                .expect("same canonical backing")
        );
        let LargerPayload(bytes) = LargerPayload([0; 128]);
        assert_eq!(bytes.len(), 128);
    }
}
