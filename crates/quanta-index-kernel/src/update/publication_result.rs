use quanta_index_core::index::{CatalogView, IndexError, OperationKey, WorkBudget};

use super::{ControlCatalog, Kernel};

impl<C: ControlCatalog + 'static> Kernel<C> {
    /// Return fixed metadata from the Catalog's exact Published operation read.
    /// This does not create a read pin or fetch immutable object bodies.
    pub fn published_view_metadata(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<CatalogView, IndexError> {
        budget.checkpoint()?;
        let metadata = self.services.catalog.published_view_metadata(key, budget)?;
        budget.checkpoint()?;
        if metadata.identity.workspace != key.workspace || metadata.identity.epoch != key.epoch {
            return Err(IndexError::DigestConflict);
        }
        Ok(metadata)
    }
}
