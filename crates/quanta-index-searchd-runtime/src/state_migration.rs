//! Concrete wiring for the offline `backup-state` / `restore-state` /
//! `verify-state` commands.
//!
//! This is the composition root for the offline surface: it is the only
//! place that names [`SqliteCatalog`], the lexical
//! inventory and [`RepoMapGenerationStore`], and it hands them to the engine
//! as the two ports `quanta_index_searchd::app::state_migration` declares.
//! The engine itself never names an adapter, so its ordering and refusal
//! rules are provable without a storage engine.
//!
//! The daemon's boot path refuses a legacy root typed.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use quanta_index_catalog::{
    CatalogSnapshotReceiptV1, SqliteCatalog, live_catalog_receipt, snapshot_catalog_file,
    verify_snapshot,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::RepoMapGenerationStore;
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::state_format::{
    EnvironmentStateMigrationFaultV1, StateMigrationFaultPort, refuse_legacy_state_root_v1,
};
use quanta_index_searchd::app::state_migration::{
    CatalogFreezeV1, CatalogSnapshotPort, CatalogSnapshotV1, OfflineSourceSessionV1,
    OfflineStateCommandV1, OfflineStateOperationV1, OfflineStateOutcomeV1,
    OfflineStateVerificationV1, StateRootDeepOpenPort, StateRootDeepOpenReceiptV1,
    VerifyManifestKindV1, peek_verify_manifest_v1, run_offline_backup_v1, run_offline_restore_v1,
    run_offline_verify_v1,
};

/// How long an offline catalog open waits on a held lock before answering
/// typed; the daemon uses the same budget for the same reason.
const OFFLINE_CATALOG_BUSY_BUDGET: Duration = Duration::from_secs(2);

/// The catalog port: the engine's backup API, with both halves of the freeze
/// returned so the engine can refuse a snapshot that is not equal to its
/// source.
struct CatalogSnapshotAdapter;

impl CatalogSnapshotPort for CatalogSnapshotAdapter {
    fn snapshot_into(
        &self,
        live_root: &Path,
        destination_file: &Path,
    ) -> Result<CatalogFreezeV1, CoreError> {
        let live = live_catalog_receipt(live_root, OFFLINE_CATALOG_BUSY_BUDGET)?;
        let snapshot = snapshot_catalog_file(
            &live.destination,
            destination_file,
            OFFLINE_CATALOG_BUSY_BUDGET,
        )?;
        Ok(CatalogFreezeV1 {
            live: map_receipt(&live),
            snapshot: map_receipt(&snapshot),
        })
    }

    fn verify_snapshot_at(&self, snapshot_file: &Path) -> Result<CatalogSnapshotV1, CoreError> {
        verify_snapshot(snapshot_file).map(|receipt| map_receipt(&receipt))
    }
}

fn map_receipt(receipt: &CatalogSnapshotReceiptV1) -> CatalogSnapshotV1 {
    CatalogSnapshotV1 {
        content_digest_hex: receipt.content_digest_hex.clone(),
        byte_size: receipt.byte_size,
        table_rows: receipt.table_rows.clone(),
    }
}

/// The deep open a produced root must survive before it is published.
///
/// The catalog's own open (crash recovery against unfinished journal rows,
/// stale durable-lease release, and sequence reconciliation), the `RepoMap`
/// store's catalog-versus-object reconciliation, and both index tracks'
/// sealed-generation inventories.
struct RootDeepOpenAdapter;

impl StateRootDeepOpenPort for RootDeepOpenAdapter {
    fn deep_open(&self, root: &Path) -> Result<StateRootDeepOpenReceiptV1, CoreError> {
        let catalog = SqliteCatalog::open(root, OFFLINE_CATALOG_BUSY_BUDGET)?;
        let candidates = catalog.repomap_candidate_rows()?;
        let repomap_candidates = u64::try_from(candidates.len()).map_or(u64::MAX, |count| count);
        let _repomap = RepoMapGenerationStore::open(root.join("repo-map"), Arc::new(catalog))?;
        let lexical = quanta_index_lexical::inventory_sealed_generations(
            &root.join("indexes").join("lexical"),
        )?;
        let semantic = quanta_index_semantic::inventory_persisted_generations(
            &quanta_index_semantic::semantic_state_root(root),
        )?;
        Ok(StateRootDeepOpenReceiptV1 {
            sealed_generations: u64::try_from(
                lexical.sealed.len().saturating_add(semantic.sealed.len()),
            )
            .map_or(u64::MAX, |count| count),
            repomap_candidates,
            catalog_rows: repomap_candidates,
        })
    }
}

/// What one offline command produced, ready to print.
#[derive(Clone, Debug)]
pub enum OfflineStateCommandOutcomeV1 {
    Produced(Box<OfflineStateOutcomeV1>),
    Verified(OfflineStateVerificationV1),
}

/// Run one offline state command end to end.
///
/// Every command uses the daemon's own state-root lease on the root it reads
/// and the fault port that lets the crash matrix interrupt it at a named
/// boundary. The production port is inert unless
/// `QUANTA_INDEX_STATE_MIGRATION_CRASH` names a boundary.
pub fn run_offline_state_command_v1(
    command: &OfflineStateCommandV1,
) -> Result<OfflineStateCommandOutcomeV1> {
    run_offline_state_command_with_v1(command, &EnvironmentStateMigrationFaultV1)
}

/// The fault-injectable body of [`run_offline_state_command_v1`].
///
/// Custody is established here, once per command, before the engine runs:
/// a current root's session binds the daemon's own lease (a live owner
/// fails that handover with the existing `STATE_ROOT_IN_USE`), while
/// backup roots open under read-only custody that creates nothing
/// inside the source.
pub fn run_offline_state_command_with_v1(
    command: &OfflineStateCommandV1,
    fault: &dyn StateMigrationFaultPort,
) -> Result<OfflineStateCommandOutcomeV1> {
    let catalog = CatalogSnapshotAdapter;
    let produced = match command.operation {
        OfflineStateOperationV1::Backup => {
            let (session, destination) = backup_session_v1(command)?;
            run_offline_backup_v1(
                &session,
                &destination,
                &catalog,
                &RootDeepOpenAdapter,
                fault,
            )?
        }
        OfflineStateOperationV1::Restore => {
            let (session, destination) = restore_session_v1(command)?;
            run_offline_restore_v1(
                &session,
                &destination,
                &catalog,
                &RootDeepOpenAdapter,
                fault,
            )?
        }
        // `verify-state` names one root; its arm returns the verified
        // outcome directly rather than through a produced-root receipt, and
        // so it never requires a destination.
        OfflineStateOperationV1::Verify => {
            return Ok(OfflineStateCommandOutcomeV1::Verified(
                run_offline_verify_v1(&verify_session_v1(command)?, &catalog)?,
            ));
        }
    };
    Ok(OfflineStateCommandOutcomeV1::Produced(Box::new(produced)))
}

/// The destination one *producing* offline command requires.
///
/// Only the two producing operations reach this, so a missing destination
/// is refused typed here rather than reaching the engine.
fn producing_destination_v1(
    command: &OfflineStateCommandV1,
) -> Result<std::path::PathBuf, CoreError> {
    command
        .destination_root
        .clone()
        .ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest,
            message: format!(
                "{} requires a destination root",
                command.operation.command_name()
            ),
        })
}

/// Custody for `backup-state`: the daemon's own lease on the source.
///
/// The existence precheck runs first so a missing source is `NotFound`
/// without creating anything; the acquisition itself is the daemon's lease
/// handover, reused verbatim — a live owner is the existing
/// `STATE_ROOT_IN_USE`.
fn backup_session_v1(
    command: &OfflineStateCommandV1,
) -> Result<(OfflineSourceSessionV1, std::path::PathBuf), CoreError> {
    let destination = producing_destination_v1(command)?;
    if !command.source_root.is_dir() {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::NotFound,
            message: format!(
                "offline state source root {} does not exist",
                command.source_root.display()
            ),
        });
    }
    refuse_legacy_state_root_v1(&command.source_root)?;
    let lease = StateRootLease::acquire(&command.source_root)?;
    let session = OfflineSourceSessionV1::open_current(lease)?;
    Ok((session, destination))
}

/// Custody for `restore-state`: read-only custody of the backup source.
/// Nothing is created inside it.
fn restore_session_v1(
    command: &OfflineStateCommandV1,
) -> Result<(OfflineSourceSessionV1, std::path::PathBuf), CoreError> {
    let destination = producing_destination_v1(command)?;
    let session = OfflineSourceSessionV1::open_produced_backup(&command.source_root)?;
    Ok((session, destination))
}

/// Custody for `verify-state`, routed by the advertised manifest.
///
/// A backup root never takes a lease; a missing or ambiguous root never
/// creates anything. A current root's verification binds the daemon's
/// lease — a live owner fails that handover with the existing
/// `STATE_ROOT_IN_USE`.
fn verify_session_v1(command: &OfflineStateCommandV1) -> Result<OfflineSourceSessionV1, CoreError> {
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

/// Render an outcome as the operator-facing line the CLI prints.
#[must_use]
pub fn render_offline_outcome_v1(
    command: &OfflineStateCommandV1,
    outcome: &OfflineStateCommandOutcomeV1,
) -> String {
    match outcome {
        OfflineStateCommandOutcomeV1::Produced(produced) => format!(
            "{}: {} -> {} objects={} catalog-rows={} manifest={} sealed-generations={}",
            command.operation.command_name(),
            command.source_root.display(),
            produced.destination_root.display(),
            produced.objects,
            produced.catalog_rows,
            produced.manifest_digest_hex,
            produced.deep_open.sealed_generations,
        ),
        OfflineStateCommandOutcomeV1::Verified(verified) => format!(
            "verify-state: {} objects={} catalog-rows={} manifest={}",
            verified.root.display(),
            verified.objects,
            verified.catalog_rows,
            verified.manifest_digest_hex,
        ),
    }
}
