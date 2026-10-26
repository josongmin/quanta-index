//! Checked retirement of the original production composition.

use std::sync::{Arc, Mutex};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::CoreError;
use quanta_index_searchd::app::runtime::RuntimeRetirementPort;

use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_sqlite_vfs::ServiceFundedCatalogIo;

#[cfg(test)]
mod tests;

struct RetirementState {
    catalog: Option<Arc<SqliteCatalog>>,
    catalog_io: Option<Arc<ServiceFundedCatalogIo>>,
    completed: bool,
}

pub(crate) struct LegacyRuntimeRetirement {
    state: Mutex<RetirementState>,
    deletions: Arc<PublicationFileDeletions>,
}

impl LegacyRuntimeRetirement {
    pub(crate) fn new(
        catalog: Arc<SqliteCatalog>,
        catalog_io: Arc<ServiceFundedCatalogIo>,
        deletions: Arc<PublicationFileDeletions>,
    ) -> Self {
        Self {
            state: Mutex::new(RetirementState {
                catalog: Some(catalog),
                catalog_io: Some(catalog_io),
                completed: false,
            }),
            deletions,
        }
    }
}

impl RuntimeRetirementPort for LegacyRuntimeRetirement {
    fn retire(&self) -> Result<(), CoreError> {
        let mut state = self.state.lock().map_err(|error| {
            CoreError::Storage(format!("production retirement state poisoned: {error}"))
        })?;
        if state.completed {
            return Ok(());
        }
        // With all logical consumers gone, finish original intents before
        // native close may need another deletion or checkpoint operation.
        self.deletions.reconcile_pending_namespace()?;
        self.deletions.reconcile_retired_control_creates()?;
        if let Some(catalog) = state.catalog.as_mut() {
            let catalog = Arc::get_mut(catalog).ok_or_else(|| {
                CoreError::NotReady("Catalog consumers remain after child drain".into())
            })?;
            // Keep the owner in the retirement state throughout native close
            // and error classification, including unwinding from either.
            catalog.close_checked()?;
        }
        drop(state.catalog.take());
        if let Some(io) = state.catalog_io.as_mut() {
            let io = Arc::get_mut(io).ok_or_else(|| {
                CoreError::NotReady("Catalog funding owners remain after physical close".into())
            })?;
            io.retire_checked()?;
        }
        // A successful native scope retirement authorizes surrender of the
        // original service parent. Failure leaves both in the same state.
        drop(state.catalog_io.take());
        // No logical caller remains. Reconcile original namespace outcomes
        // before draining Control creates, which cannot consume foreign intents.
        self.deletions.reconcile_pending_namespace()?;
        self.deletions.reconcile_retired_control_creates()?;
        let binder = self.deletions.binder();
        binder.sweep_retired()?;
        binder.retire_maintenance_reserve_checked()?;
        let snapshot = binder.capacity_snapshot()?;
        let quiescent = binder.is_quiescent()?;
        if !quiescent
            || snapshot.active_allowances != 0
            || snapshot.active_tickets != 0
            || snapshot.uncertain_pending != 0
            || snapshot.poisoned
        {
            return Err(CoreError::NotReady(format!(
                "physical publication authority has live or uncertain custody after retirement: quiescent={quiescent}, capacity={snapshot:?}"
            )));
        }
        state.completed = true;
        drop(state);
        Ok(())
    }
}
