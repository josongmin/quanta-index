//! Attempt-local immutable layout preparation, before its parent is installed.

use super::{AttemptRef, ObjectRef, StagedObjectBatch, ports::PreparedObjectMapBatch};

/// One existing root or one admitted immutable child awaiting installation.
///
/// A provisional reference is data for constructing its parent, not a readable
/// object or publication authority. Only successful installation releases it.
pub struct SearchablePreparation {
    pub(super) reference: Option<ObjectRef>,
    pub(super) prepared: Option<StagedObjectBatch>,
    pub(super) attempt: Option<AttemptRef>,
    pub(super) prepared_map: Option<Box<dyn PreparedObjectMapBatch>>,
}

impl SearchablePreparation {
    #[must_use]
    pub const fn existing(reference: Option<ObjectRef>) -> Self {
        Self {
            reference,
            prepared: None,
            attempt: None,
            prepared_map: None,
        }
    }

    /// Provisional identity for canonical parent encoding; do not read it yet.
    #[must_use]
    pub const fn reference(&self) -> Option<ObjectRef> {
        self.reference
    }
}

/// The original searchable child and its dependent coverage parent remain one
/// owned input IR until the selected original backend installs both.
pub struct SearchableParentPreparation {
    pub(super) reference: ObjectRef,
    pub(super) expected_child: Option<ObjectRef>,
    pub(super) inputs: StagedObjectBatch,
    pub(super) attempt: Option<AttemptRef>,
    pub(super) prepared_map: Option<Box<dyn PreparedObjectMapBatch>>,
}

impl SearchableParentPreparation {
    #[must_use]
    pub const fn reference(&self) -> ObjectRef {
        self.reference
    }
}
