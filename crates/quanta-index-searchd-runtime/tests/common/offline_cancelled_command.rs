//! The same preflight refusal and source custody oracle in both build profiles.

use std::error::Error;
use std::fs;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_searchd::app::config::SearchdConfig;
use quanta_index_searchd::app::state_format::{
    STATE_ROOT_LEASE_FILE_NAME, staging_directory_for_v1,
};
use quanta_index_searchd::app::state_migration::{
    OfflineStateCommandOutcomeV1, OfflineStateCommandV1, OfflineStateOperationV1,
};
use quanta_index_searchd_harness::private_tempdir;

use crate::offline_live_root::build_live_root;

pub(super) fn require_cancelled_before_staging(
    run: impl FnOnce(
        &OfflineStateCommandV1,
        &SearchdConfig,
        &RequestBudgetV1,
    ) -> anyhow::Result<OfflineStateCommandOutcomeV1>,
) -> Result<(), Box<dyn Error>> {
    let source = private_tempdir()?;
    let parent = private_tempdir()?;
    build_live_root(source.path())?;
    let authority = source.path().join("authorities/history.cbor");
    let original = fs::read(&authority)?;
    let lease = source.path().join(STATE_ROOT_LEASE_FILE_NAME);
    let lease_existed = lease.try_exists()?;
    let destination = parent.path().join("cancelled-backup");
    let staging = staging_directory_for_v1(&destination);
    let command = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Backup,
        source_root: source.path().to_path_buf(),
        destination_root: Some(destination.clone()),
    };
    let config = SearchdConfig::from_test_state_root(command.source_root.clone());
    let budget = RequestBudgetV1::unbounded();
    budget.cancel_handle().cancel();
    let error = run(&command, &config, &budget)
        .err()
        .ok_or("cancelled offline request completed")?;
    if !matches!(
        error.downcast_ref::<CoreError>(),
        Some(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            message,
        }) if message.contains("checkpoint `offline state command preflight`")
    ) || destination.try_exists()?
        || staging.try_exists()?
        || lease.try_exists()? != lease_existed
        || fs::read(&authority)? != original
    {
        return Err(format!("cancelled request changed source or target: {error:#}").into());
    }
    Ok(())
}
