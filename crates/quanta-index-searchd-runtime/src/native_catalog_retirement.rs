//! One retryable owner for the native service's admitted Catalog connection.

use std::sync::{Arc, Mutex};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::CoreError;
use quanta_index_searchd::app::runtime::{RuntimeRetirementPort, StateRootLease};

use crate::native_scratch_io::NativeScratchIo;

struct RetirementState {
    catalog: Option<Arc<SqliteCatalog>>,
    io: Option<Arc<NativeScratchIo>>,
    root: Option<Arc<StateRootLease>>,
    completed: bool,
}

pub(crate) struct NativeCatalogRetirement {
    state: Mutex<RetirementState>,
}

impl NativeCatalogRetirement {
    pub(crate) fn new(
        catalog: Arc<SqliteCatalog>,
        io: Arc<NativeScratchIo>,
        root: Arc<StateRootLease>,
    ) -> Self {
        Self {
            state: Mutex::new(RetirementState {
                catalog: Some(catalog),
                io: Some(io),
                root: Some(root),
                completed: false,
            }),
        }
    }
}

impl RuntimeRetirementPort for NativeCatalogRetirement {
    fn retire(&self) -> Result<(), CoreError> {
        let mut state = self.state.lock().map_err(|error| {
            CoreError::Storage(format!("native Catalog retirement state poisoned: {error}"))
        })?;
        if state.completed {
            return Ok(());
        }
        if let Some(catalog) = state.catalog.as_mut() {
            let catalog = Arc::get_mut(catalog).ok_or_else(|| {
                CoreError::NotReady("native Catalog consumers remain after child drain".into())
            })?;
            // On error, Catalog restores its original connection. The lease,
            // VFS owner and root remain in this state for the next attempt.
            catalog.close_checked()?;
        }
        if let Some(io) = state.io.as_ref() {
            io.retire_checked()?;
        }
        drop(state.catalog.take());
        drop(state.io.take());
        drop(state.root.take());
        state.completed = true;
        drop(state);
        Ok(())
    }
}

#[cfg(test)]
#[path = "native_catalog_retirement/tests.rs"]
mod tests;

impl Drop for NativeCatalogRetirement {
    fn drop(&mut self) {
        if self.retire().is_ok() {
            return;
        }
        // No caller remains to retry. Preserve the exact connection, its
        // retained cache charge, VFS funding and root lease in the process
        // rather than reporting a clean close or releasing an uncertain FD.
        let state = match self.state.get_mut() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state
            .catalog
            .as_mut()
            .is_some_and(|catalog| Arc::get_mut(catalog).is_none())
        {
            // Another Catalog owner still exists. Its connection owns the
            // cache lease and I/O admission; normal Arc destruction retains
            // them until that owner closes or performs its funded fallback.
            return;
        }
        if let Some(catalog) = state.catalog.take() {
            std::mem::forget(catalog);
        }
        if let Some(io) = state.io.take() {
            std::mem::forget(io);
        }
        if let Some(root) = state.root.take() {
            std::mem::forget(root);
        }
    }
}
