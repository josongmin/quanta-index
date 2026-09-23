//! Concrete wiring for the offline `migrate-state` / `backup-state` /
//! `restore-state` / `verify-state` commands (SEP-21 P10 / S21-11).
//!
//! This is the composition root for the offline surface: it is the only
//! place that names [`SqliteCatalog`], [`SemanticAdapter`], the lexical
//! inventory and [`RepoMapGenerationStore`], and it hands them to the engine
//! as the three ports `quanta_index_searchd::app::state_migration` declares.
//! The engine itself never names an adapter, so its ordering and refusal
//! rules are provable without a storage engine.
//!
//! The legacy semantic parser is linked **here only**; the daemon's boot path
//! has no migrator and refuses a legacy root typed.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use quanta_index_catalog::{
    CatalogSnapshotReceiptV1, SqliteCatalog, live_catalog_receipt, normalize_catalog_journal_mode,
    snapshot_catalog_file, verify_snapshot,
};
use quanta_index_core::CoreError;
use quanta_index_repomap::RepoMapGenerationStore;
use quanta_index_searchd::app::LegacySemanticJournalReaderV1;
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::semantic_boot;
use quanta_index_searchd::app::state_format::{
    EnvironmentStateMigrationFaultV1, LEGACY_SEMANTIC_JOURNAL_RELATIVE, StateMigrationFaultPort,
};
use quanta_index_searchd::app::state_migration::{
    CatalogFreezeV1, CatalogSnapshotPort, CatalogSnapshotV1, LegacyImportOutcomeV1,
    LegacyStateImportPort, OfflineSourceSessionV1, OfflineStateCommandV1, OfflineStateOperationV1,
    OfflineStateOutcomeV1, OfflineStateVerificationV1, StateRootDeepOpenPort,
    StateRootDeepOpenReceiptV1, VerifyManifestKindV1, peek_verify_manifest_v1,
    run_offline_backup_v1, run_offline_migrate_v1, run_offline_restore_v1, run_offline_verify_v1,
};
use quanta_index_semantic::SemanticAdapter;

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

/// The offline legacy importer: the only surface that links the legacy
/// semantic journal parser and the legacy `RepoMap` layout.
struct LegacyStateImporterV1;

impl LegacyStateImportPort for LegacyStateImporterV1 {
    fn import_legacy_into(
        &self,
        source_root: &Path,
        staging_root: &Path,
    ) -> Result<LegacyImportOutcomeV1, CoreError> {
        let mut imported_records: u64 = 0;
        let mut consumed_markers: Vec<String> = Vec::new();

        // A V1 RepoMap snapshot is a materialized view, not the source graph
        // bundle required by the current generation store. Copying its bytes
        // into an inert namespace would publish a root with no active RepoMap
        // while claiming that migration succeeded. Refuse such a root until
        // the producer can replay its source bundle into current authority.
        for name in quanta_index_searchd::app::state_format::LEGACY_REPOMAP_DIRECTORY_NAMES {
            let legacy = source_root.join("repo-map").join(name);
            refuse_unconvertible_legacy_repomap_v1(&legacy)?;
        }

        if source_root.join(LEGACY_SEMANTIC_JOURNAL_RELATIVE).exists() {
            let semantic_root = quanta_index_semantic::semantic_state_root(staging_root);
            let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
            // Read-only open of the source journal: no lock, no receipt, no
            // cleanup inside the source. The receipt lands in the staging
            // semantic root only.
            let reader = LegacySemanticJournalReaderV1::open(source_root.join("semantic"))?;
            let outcome = semantic_boot::migrate_legacy_semantic_journal(
                &reader,
                &adapter,
                &semantic_root,
                adapter.window_policy(),
            )?;
            if let semantic_boot::SemanticMigrationOutcome::Migrated { imported } = outcome {
                imported_records = imported_records
                    .saturating_add(u64::try_from(imported).map_or(u64::MAX, |count| count));
            }
            consumed_markers.push(LEGACY_SEMANTIC_JOURNAL_RELATIVE.to_string());
        }

        // Staging schema migration (S21-11 step 5): the produced root gets a
        // whole catalog from its first open, and its journal mode is
        // normalized to the offline snapshot form so `verify-state` can read
        // it without a shared-memory file.
        let catalog = SqliteCatalog::open(staging_root, OFFLINE_CATALOG_BUSY_BUDGET)?;
        let catalog_path = catalog.path().to_path_buf();
        drop(catalog);
        let _receipt = normalize_catalog_journal_mode(&catalog_path)?;

        Ok(LegacyImportOutcomeV1 {
            imported_records,
            consumed_markers,
        })
    }
}

/// Empty legacy layout directories are markers, not authority. Any entry is
/// unconvertible without the original graph producer and must not be silently
/// dropped from the produced current root.
fn refuse_unconvertible_legacy_repomap_v1(source: &Path) -> Result<(), CoreError> {
    let metadata = match std::fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "state migration: inspect legacy RepoMap directory {}: {error}",
                source.display()
            )));
        }
    };
    if !metadata.is_dir() {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
            message: format!(
                "state migration: legacy RepoMap path {} is not a directory",
                source.display()
            ),
        });
    }
    let mut entries = std::fs::read_dir(source).map_err(|error| {
        CoreError::Storage(format!(
            "state migration: read legacy RepoMap directory {}: {error}",
            source.display()
        ))
    })?;
    if let Some(entry) = entries.next() {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!(
                "state migration: read legacy RepoMap entry in {}: {error}",
                source.display()
            ))
        })?;
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            message: format!(
                "state migration: legacy RepoMap authority {} cannot be converted from materialized V1 snapshots; replay the producer source bundle into a current root",
                entry.path().display()
            ),
        });
    }
    Ok(())
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
/// fails that handover with the existing `STATE_ROOT_IN_USE`), while legacy
/// and backup roots open under read-only custody that creates nothing
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
        OfflineStateOperationV1::Migrate => {
            let (session, destination) = migrate_session_v1(command)?;
            run_offline_migrate_v1(
                &session,
                &destination,
                &LegacyStateImporterV1,
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
/// Only the three producing operations reach this, so a missing destination
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
    let lease = StateRootLease::acquire(&command.source_root)?;
    let session = OfflineSourceSessionV1::open_current(lease)?;
    Ok((session, destination))
}

/// Custody for `migrate-state`: read-only custody of the legacy source.
/// Nothing is created inside it.
fn migrate_session_v1(
    command: &OfflineStateCommandV1,
) -> Result<(OfflineSourceSessionV1, std::path::PathBuf), CoreError> {
    let destination = producing_destination_v1(command)?;
    let session = OfflineSourceSessionV1::open_legacy_read_only(&command.source_root)?;
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
            "{}: {} -> {} objects={} catalog-rows={} manifest={} legacy-records={} sealed-generations={}",
            command.operation.command_name(),
            command.source_root.display(),
            produced.destination_root.display(),
            produced.objects,
            produced.catalog_rows,
            produced.manifest_digest_hex,
            produced.imported_legacy_records,
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
