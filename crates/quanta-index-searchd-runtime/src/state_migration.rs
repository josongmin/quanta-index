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
use quanta_index_searchd::app::LegacySemanticJournalStore;
use quanta_index_searchd::app::semantic_boot;
use quanta_index_searchd::app::state_format::{
    EnvironmentStateMigrationFaultV1, LEGACY_SEMANTIC_JOURNAL_RELATIVE, StateMigrationFaultPort,
};
use quanta_index_searchd::app::state_migration::{
    CatalogFreezeV1, CatalogSnapshotPort, CatalogSnapshotV1, LegacyImportOutcomeV1,
    LegacyStateImportPort, OfflineStateCommandV1, OfflineStateOperationV1, OfflineStateOutcomeV1,
    OfflineStateRequestV1, OfflineStateVerificationV1, StateRootDeepOpenPort,
    StateRootDeepOpenReceiptV1, run_offline_backup_v1, run_offline_migrate_v1,
    run_offline_restore_v1, run_offline_verify_v1,
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

        // The legacy `RepoMap` layout is migration input, never opened as a
        // store: every current open path refuses it typed. Its bytes are
        // carried verbatim into a namespace current adapters do not read, so
        // the import neither loses them nor lets them become a serving
        // authority again.
        for name in quanta_index_searchd::app::state_format::LEGACY_REPOMAP_DIRECTORY_NAMES {
            let legacy = source_root.join("repo-map").join(name);
            if !legacy.is_dir() {
                continue;
            }
            let destination = staging_root.join("legacy-import").join(name);
            imported_records =
                imported_records.saturating_add(carry_legacy_tree_v1(&legacy, &destination)?);
            consumed_markers.push(format!("repo-map/{name}"));
        }

        if source_root.join(LEGACY_SEMANTIC_JOURNAL_RELATIVE).exists() {
            let semantic_root = quanta_index_semantic::semantic_state_root(staging_root);
            let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
            let store = LegacySemanticJournalStore::open(source_root.join("semantic"))?;
            let outcome = semantic_boot::migrate_legacy_semantic_journal(
                &store,
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

/// Copy one legacy tree into a staging destination, refusing a symlink or a
/// special file rather than guessing what it meant. Returns the file count.
fn carry_legacy_tree_v1(source: &Path, destination: &Path) -> Result<u64, CoreError> {
    std::fs::create_dir_all(destination).map_err(|error| {
        CoreError::Storage(format!(
            "state migration: create legacy import directory {}: {error}",
            destination.display()
        ))
    })?;
    let mut carried: u64 = 0;
    let entries = std::fs::read_dir(source).map_err(|error| {
        CoreError::Storage(format!(
            "state migration: read legacy directory {}: {error}",
            source.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!(
                "state migration: read legacy directory entry in {}: {error}",
                source.display()
            ))
        })?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            CoreError::Storage(format!(
                "state migration: inspect legacy entry {}: {error}",
                path.display()
            ))
        })?;
        let name = entry.file_name();
        let target = destination.join(&name);
        if metadata.file_type().is_symlink() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
                message: format!(
                    "state migration: legacy root holds the symlink {}; the offline import refuses to follow it",
                    path.display()
                ),
            });
        }
        if metadata.is_dir() {
            carried = carried.saturating_add(carry_legacy_tree_v1(&path, &target)?);
            continue;
        }
        if !metadata.is_file() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
                message: format!(
                    "state migration: legacy root holds the non-regular entry {}",
                    path.display()
                ),
            });
        }
        let _copied = std::fs::copy(&path, &target).map_err(|error| {
            CoreError::Storage(format!(
                "state migration: carry legacy object {}: {error}",
                path.display()
            ))
        })?;
        carried = carried.saturating_add(1);
    }
    Ok(carried)
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
pub fn run_offline_state_command_with_v1(
    command: &OfflineStateCommandV1,
    fault: &dyn StateMigrationFaultPort,
) -> Result<OfflineStateCommandOutcomeV1> {
    let catalog = CatalogSnapshotAdapter;
    let produced = match command.operation {
        OfflineStateOperationV1::Backup => run_offline_backup_v1(
            &producing_request_v1(command)?,
            &catalog,
            &RootDeepOpenAdapter,
            fault,
        )?,
        OfflineStateOperationV1::Restore => run_offline_restore_v1(
            &producing_request_v1(command)?,
            &catalog,
            &RootDeepOpenAdapter,
            fault,
        )?,
        OfflineStateOperationV1::Migrate => run_offline_migrate_v1(
            &producing_request_v1(command)?,
            &LegacyStateImporterV1,
            &catalog,
            &RootDeepOpenAdapter,
            fault,
        )?,
        // `verify-state` names one root; its arm returns the verified
        // outcome directly rather than through a produced-root receipt, and
        // so it never requires a destination.
        OfflineStateOperationV1::Verify => {
            return Ok(OfflineStateCommandOutcomeV1::Verified(
                run_offline_verify_v1(&command.source_root, &catalog)?,
            ));
        }
    };
    Ok(OfflineStateCommandOutcomeV1::Produced(Box::new(produced)))
}

/// The engine request for one *producing* offline command.
///
/// Only the three producing operations reach this, so a missing destination
/// is refused typed here rather than reaching the engine.
fn producing_request_v1(
    command: &OfflineStateCommandV1,
) -> Result<OfflineStateRequestV1, CoreError> {
    let destination = command
        .destination_root
        .clone()
        .ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest,
            message: format!(
                "{} requires a destination root",
                command.operation.command_name()
            ),
        })?;
    Ok(OfflineStateRequestV1 {
        operation: command.operation,
        source_root: command.source_root.clone(),
        destination_root: destination,
    })
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
