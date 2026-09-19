//! Settling the garbage collection that runs after a durable mutation
//! (QI-BB-020).
//!
//! The mutation stands whatever its GC does, so a GC step that fails
//! because the storage failed is deferred: counted, and found again by the
//! next pass, which derives its work from the disk and the catalog rather
//! than from an earlier pass. A step that fails with a refusal has found
//! something about the state on disk — a directory whose identity
//! contradicts its path, an entry that is not a generation — and a retry
//! would find the same thing, so the refusal fails the caller closed
//! (§3.49).

use quanta_index_core::CoreError;

/// `Ok` when `error` is the storage's, so the step can wait for the next
/// pass; the error itself when it is a refusal.
pub(crate) fn defer_storage_failure(error: CoreError) -> Result<(), CoreError> {
    match error {
        CoreError::Storage(_) => Ok(()),
        refusal @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)) => Err(refusal),
    }
}
