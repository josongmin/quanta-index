//! Prepared DATA for two bounded childless install packets in one attempt.

use super::install_ports::ObjectWriteSink;
use super::{AttemptRef, IndexError, ObjectRef, StagedObjectReferences, WorkBudget};

/// Owns the exact original wire, registration and output-slot allocations.
///
/// Preparation is effect-free. Dropping a handle never confirms or publishes.
/// The backend binds its namespace, original request/deadline and attempt.
/// Install or drop within the original execution: temporary grants must not
/// escape it and do not confer CPU, I/O, publication or reclaim authority.
pub trait PreparedObjectInstallRun: Send {
    /// No mutation or work charge when the next distinct input does not fit.
    fn accepts(&self, payload_bytes: usize) -> bool;

    /// Append one distinct childless object in caller order. Every backing
    /// allocation was funded before this handle was returned. Duplicate or
    /// mismatched identities refuse; this is DATA, not Catalog authority.
    /// Any failed adoption poisons the handle; installation then refuses.
    fn push(
        &mut self,
        payload: &[u8],
        expected: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;

    /// Check an alias against the exact already retained wire, without copying.
    fn check_payload(&self, slot: usize, payload: &[u8]) -> Result<(), IndexError>;

    fn references(&self) -> &[ObjectRef];

    /// Child-first packets retain their individual payload/count bounds. No
    /// Catalog write transaction spans immutable-file I/O. This does not govern
    /// the caller's private scratch transaction. The caller must validate ALL
    /// ordered results before finishing its single logical confirmation receipt.
    fn install(
        self: Box<Self>,
        attempt: AttemptRef,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedObjectReferences, IndexError>;
}
