//! Target factory for offline refusals that must precede staging.

use std::path::Path;

use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_searchd::app::state_migration::{
    OfflineSourceSessionV1, OfflineTargetFactoryV1, OfflineTargetSessionV1,
};

pub(super) struct UnreachedTargetFactoryV1;

impl OfflineTargetFactoryV1 for UnreachedTargetFactoryV1 {
    fn begin_target(
        &self,
        _source: &OfflineSourceSessionV1,
        destination: &Path,
        _budget: &RequestBudgetV1,
    ) -> Result<Box<dyn OfflineTargetSessionV1>, CoreError> {
        Err(CoreError::InvalidContract(format!(
            "target creation must not be reached for {}",
            destination.display()
        )))
    }
}
