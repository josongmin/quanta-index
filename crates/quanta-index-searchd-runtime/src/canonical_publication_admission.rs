//! Legacy publication admission over one composition-owned file ledger.
//!
//! This module is deliberately not installed by the production builder. It
//! qualifies neither unbound writers nor process RSS, filesystem blocks, or I/O.

use std::sync::Arc;

use quanta_index_contract::SearchCorpusIngestBatch;
use quanta_index_core::publication_admission::{
    PublicationAdmissionRequest, PublicationControlFileUpdatePort, PublicationObjectLifetimePort,
    PublicationOutputClass, PublicationOutputPermit, PublicationOutputReservation,
    PublicationResourceAdmissionPort, PublicationResourcePermit,
};
use quanta_index_core::publication_capacity::PublicationAllowance;
use quanta_index_core::{CoreError, RequestBudgetV1};

use crate::publication_control_files::PublicationControlFiles;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_lexical_lifetime::PublicationLexicalLifetime;
use crate::publication_sidecar::sidecar_output_permit;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicationAllowanceLimit {
    bytes: u64,
    files: u64,
}

impl PublicationAllowanceLimit {
    pub fn new(bytes: u64, files: u64) -> Result<Self, CoreError> {
        if bytes == 0 || files == 0 {
            return Err(CoreError::InvalidContract(
                "publication allowance bytes and files must be positive".into(),
            ));
        }
        Ok(Self { bytes, files })
    }

    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.bytes
    }

    #[must_use]
    pub const fn files(self) -> u64 {
        self.files
    }
}

/// Control and work are independently reserved from the binder's one ledger.
///
/// The caller supplies explicit operational ceilings; batch footprint is not
/// an encoded-output estimate. Every writer must still use its physical gate.
pub struct CanonicalPublicationAdmission {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    object_lifetime: Arc<dyn PublicationObjectLifetimePort>,
    control_file_updates: Arc<dyn PublicationControlFileUpdatePort>,
    control: PublicationAllowanceLimit,
    work: PublicationAllowanceLimit,
}

impl CanonicalPublicationAdmission {
    #[must_use]
    pub fn new(
        deletions: Arc<PublicationFileDeletions>,
        control: PublicationAllowanceLimit,
        work: PublicationAllowanceLimit,
    ) -> Self {
        let binder = deletions.binder();
        let object_lifetime: Arc<dyn PublicationObjectLifetimePort> =
            Arc::new(PublicationLexicalLifetime::new(Arc::clone(&binder)));
        let control_file_updates: Arc<dyn PublicationControlFileUpdatePort> = Arc::new(
            PublicationControlFiles::new(Arc::clone(&deletions), Arc::clone(&object_lifetime)),
        );
        Self {
            binder,
            deletions,
            object_lifetime,
            control_file_updates,
            control,
            work,
        }
    }

    /// Return the object reader/GC authority backed by this exact file ledger.
    /// This creates no reservation or second physical inventory.
    #[must_use]
    pub fn object_lifetime(&self) -> Arc<dyn PublicationObjectLifetimePort> {
        Arc::clone(&self.object_lifetime)
    }

    /// Share the same pending-delete custody across recreated writer sessions.
    #[must_use]
    pub fn file_deletions(&self) -> Arc<PublicationFileDeletions> {
        Arc::clone(&self.deletions)
    }

    fn acquire(
        &self,
        class: ParentClass,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
        budget.checkpoint(match class {
            ParentClass::Control => "publication control allowance",
            ParentClass::Work => "publication work allowance",
        })?;
        self.deletions.reconcile_retired_control_creates()?;
        budget.checkpoint("publication allowance after retirement reconciliation")?;
        let limit = match class {
            ParentClass::Control => self.control,
            ParentClass::Work => self.work,
        };
        let held = self
            .binder
            .reserve_publication_allowance(limit.bytes(), limit.files())?;
        Ok(Box::new(HeldPublicationPermit {
            binder: Arc::clone(&self.binder),
            deletions: Arc::clone(&self.deletions),
            object_lifetime: Arc::clone(&self.object_lifetime),
            control_file_updates: Arc::clone(&self.control_file_updates),
            held,
            class,
        }))
    }
}

#[derive(Clone, Copy)]
enum ParentClass {
    Control,
    Work,
}

struct HeldPublicationPermit {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    object_lifetime: Arc<dyn PublicationObjectLifetimePort>,
    control_file_updates: Arc<dyn PublicationControlFileUpdatePort>,
    held: Arc<PublicationAllowance>,
    class: ParentClass,
}

impl PublicationResourcePermit for HeldPublicationPermit {
    fn control_file_updates(&self) -> Option<Arc<dyn PublicationControlFileUpdatePort>> {
        match self.class {
            ParentClass::Control => Some(Arc::clone(&self.control_file_updates)),
            ParentClass::Work => None,
        }
    }

    fn object_lifetime(&self) -> Option<Arc<dyn PublicationObjectLifetimePort>> {
        Some(Arc::clone(&self.object_lifetime))
    }

    fn held_file_allowance(&self) -> Result<Option<Arc<PublicationAllowance>>, CoreError> {
        self.binder.validate_held_allowance(&self.held)?;
        Ok(Some(Arc::clone(&self.held)))
    }

    fn reserve_output(
        &self,
        request: PublicationOutputReservation,
    ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
        let belongs = matches!(
            (self.class, request.class()),
            (ParentClass::Control, PublicationOutputClass::Control)
                | (
                    ParentClass::Work,
                    PublicationOutputClass::Lexical
                        | PublicationOutputClass::Semantic
                        | PublicationOutputClass::Source
                        | PublicationOutputClass::Scratch
                )
        );
        if !belongs {
            return Err(CoreError::InvalidContract(
                "publication output class does not match held parent".into(),
            ));
        }
        self.binder.validate_held_allowance(&self.held)?;
        self.deletions.reconcile_retired_control_creates()?;
        sidecar_output_permit(Arc::clone(&self.deletions), Arc::clone(&self.held), request)
    }
}

impl PublicationResourceAdmissionPort for CanonicalPublicationAdmission {
    fn try_acquire_control(
        &self,
        _batch: &SearchCorpusIngestBatch,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
        self.acquire(ParentClass::Control, budget)
    }

    fn try_acquire(
        &self,
        _request: &PublicationAdmissionRequest<'_>,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn PublicationResourcePermit>, CoreError> {
        self.acquire(ParentClass::Work, budget)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod lexical_tests;
