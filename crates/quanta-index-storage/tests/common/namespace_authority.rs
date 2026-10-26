//! Lease-only fixture. It denies installation and reclaim: the surrounding
//! integration tests prove namespace ownership, not catalog custody.
use std::{path::Path, sync::Arc};

use quanta_index_core::index::{
    IndexError, ObjectInstallScope, ObjectNamespaceBinding, ObjectRef, ReclaimCandidate,
    ReclaimDrainPage, ReclaimPermit, ReclaimTarget, ReclaimWorkPage, RegisteredObject, RetiringRef,
    WorkBudget,
    ports::{CatalogReclaimPort, ManagedObjectAuthority, ManagedObjectInstallPort},
};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};

struct NamespaceOnlyAuthority;

fn unavailable<T>() -> Result<T, IndexError> {
    Err(IndexError::BackendViolation(
        "namespace-only fixture has no catalog custody".into(),
    ))
}

impl ManagedObjectInstallPort for NamespaceOnlyAuthority {
    fn reserve_managed(
        &self,
        _scope: ObjectInstallScope,
        _objects: &[RegisteredObject],
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        unavailable()
    }

    fn confirm_managed(
        &self,
        _scope: ObjectInstallScope,
        _objects: &[RegisteredObject],
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        unavailable()
    }
}

impl CatalogReclaimPort for NamespaceOnlyAuthority {
    fn reclaim_work_page(
        &self,
        _after: Option<ObjectRef>,
        _limit: u32,
        _budget: &mut WorkBudget,
    ) -> Result<ReclaimWorkPage, IndexError> {
        unavailable()
    }

    fn retire_candidate(
        &self,
        _candidate: ReclaimCandidate,
        _budget: &mut WorkBudget,
    ) -> Result<Option<RetiringRef>, IndexError> {
        unavailable()
    }

    fn enter_reclaiming(
        &self,
        _retiring: RetiringRef,
        _target: ReclaimTarget,
        _budget: &mut WorkBudget,
    ) -> Result<ReclaimPermit, IndexError> {
        unavailable()
    }

    fn validate_reclaiming(
        &self,
        _permit: ReclaimPermit,
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        unavailable()
    }

    fn mark_unlinked(
        &self,
        _permit: ReclaimPermit,
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        unavailable()
    }

    fn drain_unlinked(
        &self,
        _permit: ReclaimPermit,
        _limit: u32,
        _budget: &mut WorkBudget,
    ) -> Result<ReclaimDrainPage, IndexError> {
        unavailable()
    }
}

impl ManagedObjectAuthority for NamespaceOnlyAuthority {
    fn bind_namespace(&self, _binding: ObjectNamespaceBinding) -> Result<(), IndexError> {
        Ok(())
    }
}

pub(crate) fn managed(
    root: impl AsRef<Path>,
    scratch: Arc<FileScratchSpace>,
) -> Result<FileObjectStore, IndexError> {
    let authority: Arc<dyn ManagedObjectAuthority> = Arc::new(NamespaceOnlyAuthority);
    FileObjectStore::open_managed_with_scratch(root, scratch, authority)
}
