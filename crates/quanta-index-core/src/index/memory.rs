//! Live allocation custody, separate from an execution slot's lifetime.
//!
//! An escaping allocation commits future floor/headroom capacity before it is
//! made. The scheduler moves its live charge when the execution guard ends;
//! retaining a lease must not retain CPU, IO or provider execution capacity.

use std::{fmt, sync::Arc};

use super::IndexError;

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
