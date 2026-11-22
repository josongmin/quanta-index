//! Offline command admission and source custody before adapter I/O.

use std::path::PathBuf;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{CoreError, RequestBudgetV1};

use super::{
    CatalogSnapshotPort, OfflineSourceSessionV1, OfflineStateCommandV1, OfflineStateOperationV1,
    OfflineStateOutcomeV1, OfflineStateVerificationV1, OfflineTargetFactoryV1,
    VerifyManifestKindV1, peek_verify_manifest_v1, run_offline_backup_v1, run_offline_restore_v1,
    run_offline_verify_v1,
};
use crate::app::config::SearchdConfig;
use crate::app::runtime::StateRootLease;
use crate::app::state_format::{StateMigrationFaultPort, refuse_legacy_state_root_v1};

/// The produced-root receipt or read-only verification.
#[derive(Clone, Debug)]
pub enum OfflineStateCommandOutcomeV1 {
    Produced(Box<OfflineStateOutcomeV1>),
    Verified(OfflineStateVerificationV1),
}

/// Admit the request before resolving target adapters or taking source custody.
/// A live current-root owner fails custody handover with `STATE_ROOT_IN_USE`.
pub fn run_offline_state_command_v1<T: OfflineTargetFactoryV1>(
    command: &OfflineStateCommandV1,
    config: &SearchdConfig,
    budget: &RequestBudgetV1,
    fault: &dyn StateMigrationFaultPort,
    catalog: &dyn CatalogSnapshotPort,
    create_targets: impl FnOnce() -> Result<T, CoreError>,
) -> Result<OfflineStateCommandOutcomeV1, CoreError> {
    // Staged deep-open can mutate Catalog and RepoMap. Validate deployment
    // limits before acquiring the source lease or creating staging.
    let policy = config
        .legacy_publication_physical_policy()
        .ok_or_else(|| {
            CoreError::InvalidContract(
                "offline staged deep-open requires an explicit complete physical policy".into(),
            )
        })?
        .validate()
        .map_err(|error| CoreError::InvalidContract(format!("offline physical policy: {error}")))?;
    let source_body_max_bytes = config.source_publication_max_bytes();
    if source_body_max_bytes == 0 || source_body_max_bytes > policy.inventory.bytes {
        return Err(CoreError::InvalidContract(
            "offline source body maximum exceeds physical root byte cap".into(),
        ));
    }
    budget.checkpoint("offline state command preflight")?;
    let targets = create_targets()?;
    let produced = match command.operation {
        OfflineStateOperationV1::Backup => {
            let (session, destination) = backup_session(command)?;
            run_offline_backup_v1(&session, &destination, &targets, fault, budget)?
        }
        OfflineStateOperationV1::Restore => {
            let destination = producing_destination(command)?;
            let session = OfflineSourceSessionV1::open_produced_backup(&command.source_root)?;
            run_offline_restore_v1(&session, &destination, catalog, &targets, fault, budget)?
        }
        OfflineStateOperationV1::Verify => {
            return Ok(OfflineStateCommandOutcomeV1::Verified(
                run_offline_verify_v1(&verify_session(command)?, catalog)?,
            ));
        }
    };
    Ok(OfflineStateCommandOutcomeV1::Produced(Box::new(produced)))
}

fn producing_destination(command: &OfflineStateCommandV1) -> Result<PathBuf, CoreError> {
    command
        .destination_root
        .clone()
        .ok_or_else(|| CoreError::Typed {
            code: SearchPlaneErrorCodeV2::InvalidRequest,
            message: format!(
                "{} requires a destination root",
                command.operation.command_name()
            ),
        })
}

fn backup_session(
    command: &OfflineStateCommandV1,
) -> Result<(OfflineSourceSessionV1, PathBuf), CoreError> {
    let destination = producing_destination(command)?;
    // Refuse a missing source before the lease can create any entry.
    if !command.source_root.is_dir() {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::NotFound,
            message: format!(
                "offline state source root {} does not exist",
                command.source_root.display()
            ),
        });
    }
    refuse_legacy_state_root_v1(&command.source_root)?;
    let lease = StateRootLease::acquire(&command.source_root)?;
    Ok((OfflineSourceSessionV1::open_current(lease)?, destination))
}

fn verify_session(command: &OfflineStateCommandV1) -> Result<OfflineSourceSessionV1, CoreError> {
    refuse_legacy_state_root_v1(&command.source_root)?;
    match peek_verify_manifest_v1(&command.source_root)? {
        VerifyManifestKindV1::BackupRoot => {
            OfflineSourceSessionV1::open_produced_backup(&command.source_root)
        }
        VerifyManifestKindV1::CurrentRoot => {
            let lease = StateRootLease::acquire(&command.source_root)?;
            OfflineSourceSessionV1::open_current(lease)
        }
    }
}
