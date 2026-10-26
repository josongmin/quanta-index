//! Composition-owned factory: one canonical binder, distinct held work parent
//! and Lance Session for each concurrent Semantic publication.

use std::path::Path;
use std::sync::Arc;

use lance_io::object_store::file_admission::FileAdmission;
use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::{
    PublicationOutputClass, PublicationOutputPermit, PublicationOutputReservation,
};
use quanta_index_core::publication_capacity::PublicationAllowance;
use quanta_index_semantic::{SemanticFileAdmissionFactory, SemanticMaintenanceFileFactory};

use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_lance_io::PublicationLanceAdmission;
use crate::publication_sidecar::sidecar_output_permit;

pub struct CanonicalSemanticFileAdmissionFactory {
    deletions: Arc<PublicationFileDeletions>,
}

impl CanonicalSemanticFileAdmissionFactory {
    #[must_use]
    pub fn new(deletions: Arc<PublicationFileDeletions>) -> Self {
        Self { deletions }
    }

    #[must_use]
    pub fn read_only(&self) -> Arc<dyn FileAdmission> {
        Arc::new(PublicationLanceAdmission::read_only(Arc::clone(
            &self.deletions,
        )))
    }
}

impl SemanticFileAdmissionFactory for CanonicalSemanticFileAdmissionFactory {
    fn for_work(
        &self,
        held_work: Arc<PublicationAllowance>,
    ) -> Result<Arc<dyn FileAdmission>, CoreError> {
        // The common binder authenticates ledger identity before any create.
        Ok(Arc::new(PublicationLanceAdmission::for_work(
            Arc::clone(&self.deletions),
            held_work,
        )?))
    }
}

/// Reserve maintenance receipts under the canonical authority.
///
/// This factory creates one parent, then one child in the common inode binder.
/// Its `SidecarOutputPermit` retains the parent `Arc`.
pub struct CanonicalSemanticMaintenanceFileFactory {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
}

impl CanonicalSemanticMaintenanceFileFactory {
    #[must_use]
    pub fn new(deletions: Arc<PublicationFileDeletions>) -> Self {
        Self {
            binder: deletions.binder(),
            deletions,
        }
    }
}

impl SemanticMaintenanceFileFactory for CanonicalSemanticMaintenanceFileFactory {
    fn reserve_sidecar_output(
        &self,
        max_encoded_bytes: u64,
    ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
        let bound = max_encoded_bytes.max(1);
        self.deletions.reconcile_pending_namespace()?;
        let parent = self.binder.reserve_maintenance_allowance(bound)?;
        let request =
            PublicationOutputReservation::new(PublicationOutputClass::Semantic, bound, 1)?;
        sidecar_output_permit(Arc::clone(&self.deletions), parent, request)
    }

    fn retirement_admission(&self) -> Result<Arc<dyn FileAdmission>, CoreError> {
        Ok(Arc::new(PublicationLanceAdmission::retirement_only(
            Arc::clone(&self.deletions),
        )))
    }

    fn prepare_retirement_area(&self, entry: &Path) -> Result<(), CoreError> {
        let reclaim_area = quanta_index_semantic::semantic_state_root(self.binder.state_root())
            .join(quanta_index_core::RECLAIM_AREA_DIR_NAME);
        if entry.parent() != Some(reclaim_area.as_path()) || entry.file_name().is_none() {
            return Err(CoreError::InvalidContract(
                "semantic retirement target is not a direct reclaim-area entry".into(),
            ));
        }
        self.binder.ensure_private_parent_directories(entry)
    }
}
