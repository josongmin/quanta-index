//! Shared contracts for installing attempt-local immutable data and map roots.

use super::{
    AttemptRef, IndexError, MapEdit, MapRoot, ObjectBatchInput, ObjectInstallScope, ObjectRef,
    RegisteredObject, WorkBudget,
};

/// Shared object-count envelope for one child-first durable installation.
/// Byte/work/memory limits are independently admitted by the original budget.
pub const MAX_OBJECT_INSTALL_BATCH: usize = 256;

pub trait ObjectWriteSink: Send + Sync {
    /// A typed hint, not authority. Managed stores require this scope and
    /// invoke their own paired catalog instead of trusting these callbacks.
    fn managed_scope(&self) -> Option<ObjectInstallScope> {
        None
    }
    fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError>;
    fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError>;
    fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError>;
    fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError>;
}

/// One original backend's prepared map and admitted resident COW stage.
///
/// Consumption appends at most two DATA objects and one original-base edit
/// through the existing child-first installer. An owner scope additionally
/// retains its two initial DATA/two roots within the explicit four/four bound.
pub trait PreparedObjectMapBatch: Send {
    fn root(&self) -> MapRoot;
    /// Original owner metadata precedes newly supplied DATA in the result.
    fn initial_objects(&self) -> &[ObjectRef] {
        &[]
    }
    /// Original owner roots precede the optional prepared delta and final edit.
    fn initial_roots(&self) -> &[MapRoot] {
        &[]
    }
    fn has_prepared_root(&self) -> bool {
        true
    }
    fn install(
        self: Box<Self>,
        objects: &[ObjectBatchInput],
        additional: Option<(MapRoot, &MapEdit)>,
        attempt: AttemptRef,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(super::StagedObjectReferences, super::StagedMapRoots), IndexError>;
}
