//! Offline `migrate-state`, `backup-state`, `restore-state` and
//! `verify-state` (SEP-21 P10 / S21-11).
//!
//! Every operation here is offline: it takes the daemon's own
//! [`StateRootLease`] on the root it reads, refuses anything that is not a
//! private, owned, single-link root, and never mutates its source. There is
//! no live fallback and no boot-time branch — the daemon refuses a legacy
//! root typed (see [`super::state_format::refuse_legacy_state_root_v1`]) and
//! the operator runs `migrate-state` explicitly.
//!
//! The ordering rules are the whole point, so they are structural rather
//! than documented-only:
//!
//! * a destination is fully prepared in a sibling staging directory, and
//!   the root manifest is the **last** thing written there, after every data
//!   object is fsynced and the staging root has been deep-opened;
//! * the switch is one `rename` plus a parent-directory fsync, so an
//!   interruption leaves either the old root or the complete new one;
//! * a stale staging directory from an interrupted run is removed only
//!   after proving it advertises no manifest, which is exactly the state
//!   "interrupted before the manifest" leaves.
//!
//! Concrete adapters are named by the composition root only: this module
//! talks to [`CatalogSnapshotPort`], [`LegacyStateImportPort`] and
//! [`StateRootDeepOpenPort`].

use std::fs;
use std::path::{Path, PathBuf};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;

use super::runtime::{StateRootAccessV1, StateRootLease};
use super::state_format::{
    OfflineRootRoleV1, STAGING_DIRECTORY_SUFFIX, STATE_BACKUP_CATALOG_FILE_NAME,
    STATE_BACKUP_MANIFEST_FILE_NAME, STATE_CATALOG_DIRECTORY, STATE_MIGRATION_RECEIPT_FILE_NAME,
    STATE_MIGRATION_RECEIPT_FORMAT_VERSION, STATE_ROOT_MANIFEST_FILE_NAME,
    STATE_ROOT_MANIFEST_FORMAT_VERSION, StateMigrationFaultPointV1, StateMigrationFaultPort,
    StateObjectEntryV1, StateRootFormatV1, StateRootManifestV1, atomic_cutover_v1,
    detect_state_root_format_v1, fsync_directory_v1, fsync_file_v1, inventory_state_directories_v1,
    inventory_state_root_v1, is_canonical_relative_path, legacy_state_root_markers_v1,
    read_root_manifest_v1, refuse_broad_offline_target_v1, refuse_legacy_state_root_v1,
    refuse_non_empty_destination_v1, refuse_non_private_source_root_v1, sha256_hex,
    staging_directory_for_v1, verify_root_against_manifest_v1, write_root_manifest_last_v1,
};

/// The daemon's state-root lock file: never part of an inventory.
pub const STATE_ROOT_LOCK_FILE_NAME: &str = ".searchd-state-root.lock";

/// Exclusions for an inventory taken over a **live** root.
///
/// The lease file, any manifest, and the whole catalog directory: the live
/// catalog's bytes are not a snapshot, so the backup API carries it instead
/// and the manifest's `catalog-digest` covers its logical content.
const LIVE_ROOT_EXCLUSIONS: [&str; 4] = [
    STATE_ROOT_LOCK_FILE_NAME,
    STATE_ROOT_MANIFEST_FILE_NAME,
    STATE_BACKUP_MANIFEST_FILE_NAME,
    STATE_CATALOG_DIRECTORY,
];

/// Exclusions for an inventory taken over a **produced** (staging or
/// destination) root: only the manifest files themselves.
const PRODUCED_ROOT_EXCLUSIONS: [&str; 2] = [
    STATE_ROOT_MANIFEST_FILE_NAME,
    STATE_BACKUP_MANIFEST_FILE_NAME,
];

fn typed(code: SearchPlaneErrorCodeV2, message: String) -> CoreError {
    CoreError::Typed { code, message }
}

fn storage(action: &str, path: &Path, error: &dyn std::fmt::Display) -> CoreError {
    CoreError::Storage(format!(
        "state migration: {action} {}: {error}",
        path.display()
    ))
}

/// One catalog's canonical logical content receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogSnapshotV1 {
    /// Canonical logical digest (schema plus every user table's rows).
    pub content_digest_hex: String,
    pub byte_size: u64,
    /// `(table, rows)` in schema order.
    pub table_rows: Vec<(String, u64)>,
}

impl CatalogSnapshotV1 {
    /// Total rows across every user table.
    #[must_use]
    pub fn total_rows(&self) -> u64 {
        self.table_rows.iter().map(|(_table, rows)| rows).sum()
    }
}

/// Both sides of a catalog freeze: the live catalog and the copy taken from
/// it.
///
/// The engine refuses a freeze whose halves disagree, so a snapshot is only
/// ever accepted when it is logically equal to its source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogFreezeV1 {
    pub live: CatalogSnapshotV1,
    pub snapshot: CatalogSnapshotV1,
}

impl CatalogFreezeV1 {
    /// Whether the copy is logically equal to the catalog it came from.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.live.content_digest_hex == self.snapshot.content_digest_hex
    }
}

/// Produce a consistent catalog snapshot through the storage engine's own
/// backup API — never a byte copy of the database and its WAL.
pub trait CatalogSnapshotPort: Send + Sync {
    /// Snapshot the catalog under `live_root` into `destination_file` and
    /// report both the live and the produced content receipts.
    fn snapshot_into(
        &self,
        live_root: &Path,
        destination_file: &Path,
    ) -> Result<CatalogFreezeV1, CoreError>;

    /// Re-open and re-verify an existing snapshot file, without touching any
    /// other catalog.
    fn verify_snapshot_at(&self, snapshot_file: &Path) -> Result<CatalogSnapshotV1, CoreError>;
}

/// What an offline legacy import moved into a staging root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LegacyImportOutcomeV1 {
    pub imported_records: u64,
    /// The legacy markers the import consumed, for the receipt.
    pub consumed_markers: Vec<String>,
}

/// The offline-only legacy importer.
///
/// This is the only surface that links a legacy parser: it is handed a
/// legacy root and a staging root and must fill the staging root with
/// current-format durable state, refusing ambiguity or corruption rather
/// than choosing a winner.
pub trait LegacyStateImportPort: Send + Sync {
    fn import_legacy_into(
        &self,
        source_root: &Path,
        staging_root: &Path,
    ) -> Result<LegacyImportOutcomeV1, CoreError>;
}

/// What a deep open proved about a produced root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StateRootDeepOpenReceiptV1 {
    /// Sealed generations the two index tracks inventoried.
    pub sealed_generations: u64,
    /// `RepoMap` candidates the catalog and object projections reconciled.
    pub repomap_candidates: u64,
    /// Catalog rows the reopen read back.
    pub catalog_rows: u64,
}

/// Open a produced root the way the daemon would, before it is published.
///
/// A partial staging root must not be openable as production-ready, so this
/// is a required step of every producing operation and it runs while the
/// root is still in staging.
pub trait StateRootDeepOpenPort: Send + Sync {
    fn deep_open(&self, root: &Path) -> Result<StateRootDeepOpenReceiptV1, CoreError>;
}

/// Which offline operation a request describes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfflineStateOperationV1 {
    Migrate,
    Backup,
    Restore,
    Verify,
}

impl OfflineStateOperationV1 {
    /// The operation's stable CLI name.
    #[must_use]
    pub const fn command_name(self) -> &'static str {
        match self {
            Self::Migrate => "migrate-state",
            Self::Backup => "backup-state",
            Self::Restore => "restore-state",
            Self::Verify => "verify-state",
        }
    }
}

/// One parsed offline command, as the CLI hands it to the composition root.
///
/// `verify-state` names one root only, so `destination_root` is `None` for
/// it; the three producing operations require it and refuse the command
/// before touching anything when it is missing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfflineStateCommandV1 {
    pub operation: OfflineStateOperationV1,
    pub source_root: PathBuf,
    pub destination_root: Option<PathBuf>,
}

/// A source/destination pair for one offline operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfflineStateRequestV1 {
    pub operation: OfflineStateOperationV1,
    pub source_root: PathBuf,
    pub destination_root: PathBuf,
}

/// What an offline operation produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfflineStateOutcomeV1 {
    pub operation: OfflineStateOperationV1,
    pub destination_root: PathBuf,
    pub manifest_digest_hex: String,
    pub objects: u64,
    pub catalog_digest_hex: String,
    pub catalog_rows: u64,
    pub imported_legacy_records: u64,
    pub deep_open: StateRootDeepOpenReceiptV1,
}

/// What `verify-state` proved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfflineStateVerificationV1 {
    pub root: PathBuf,
    pub manifest_digest_hex: String,
    pub objects: u64,
    pub catalog_digest_hex: String,
    pub catalog_rows: u64,
}

/// Resolve and refuse both ends of an offline operation before any mutation.
fn plan_offline_targets_v1(
    request: &OfflineStateRequestV1,
) -> Result<(PathBuf, PathBuf), CoreError> {
    let source = refuse_broad_offline_target_v1(&request.source_root, OfflineRootRoleV1::Source)?;
    let destination =
        refuse_broad_offline_target_v1(&request.destination_root, OfflineRootRoleV1::Destination)?;
    refuse_non_empty_destination_v1(&destination, &source)?;
    if source == destination {
        return Err(typed(
            SearchPlaneErrorCodeV2::InvalidRequest,
            format!(
                "offline {} refuses a source that is its own destination ({})",
                request.operation.command_name(),
                source.display()
            ),
        ));
    }
    Ok((source, destination))
}

/// Take the daemon's own lease on a root an offline operation is about to
/// read, so an offline run and a live daemon can never overlap.
///
/// The lease is the daemon's check owner, reused verbatim: a live owner is
/// `STATE_ROOT_IN_USE`, a foreign owner or a non-private root is
/// `STATE_ROOT_INSECURE`.
fn lease_offline_source_v1(source: &Path) -> Result<StateRootLease, CoreError> {
    if !source.is_dir() {
        return Err(typed(
            SearchPlaneErrorCodeV2::NotFound,
            format!(
                "offline state source root {} does not exist",
                source.display()
            ),
        ));
    }
    refuse_non_private_source_root_v1(source)?;
    StateRootLease::acquire_with_access(source, StateRootAccessV1::Private)
}

/// Refuse a source that is not a legacy root, and vice versa.
fn require_format_v1(source: &Path, expected: StateRootFormatV1) -> Result<(), CoreError> {
    let observed = detect_state_root_format_v1(source)?;
    let matches = matches!(
        (observed, expected),
        (StateRootFormatV1::LegacyV1, StateRootFormatV1::LegacyV1)
            | (
                StateRootFormatV1::CurrentV1 { .. },
                StateRootFormatV1::CurrentV1 { .. }
            )
            | (StateRootFormatV1::Absent, StateRootFormatV1::Absent)
    );
    if matches {
        return Ok(());
    }
    Err(typed(
        SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
        format!(
            "offline state source root {} is {observed:?}; this operation requires {expected:?}",
            source.display()
        ),
    ))
}

/// Create the staging directory, converging an interrupted run.
///
/// A staging directory left by an interruption is never an authority: the
/// destination it was preparing does not exist (proven before this point),
/// only the single cutover rename publishes it, and its name is derived from
/// the destination, so removing it cannot touch anything else. Whatever it
/// holds — including a manifest the interruption wrote before the rename —
/// is preparation, not a root anyone can open.
fn prepare_staging_v1(destination: &Path) -> Result<PathBuf, CoreError> {
    let staging = staging_directory_for_v1(destination);
    if staging.exists() {
        remove_directory_tree_v1(&staging)?;
    }
    create_private_directory_v1(&staging)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("/"));
    fsync_directory_v1(parent)?;
    Ok(staging)
}

fn create_private_directory_v1(directory: &Path) -> Result<(), CoreError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        let _mode = builder.mode(0o700);
    }
    builder
        .create(directory)
        .map_err(|error| storage("create directory", directory, &error))
}

fn remove_directory_tree_v1(directory: &Path) -> Result<(), CoreError> {
    let entries =
        fs::read_dir(directory).map_err(|error| storage("read staging", directory, &error))?;
    for entry in entries {
        let entry = entry.map_err(|error| storage("read staging entry", directory, &error))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| storage("inspect staging entry", &path, &error))?;
        if metadata.is_dir() {
            remove_directory_tree_v1(&path)?;
        } else {
            fs::remove_file(&path)
                .map_err(|error| storage("remove staging file", &path, &error))?;
        }
    }
    fs::remove_dir(directory)
        .map_err(|error| storage("remove staging directory", directory, &error))
}

/// Copy one root's data objects into `staging`, byte-for-byte, and prove the
/// copy against the source inventory.
fn copy_data_objects_v1(
    source: &Path,
    staging: &Path,
    directories: &[String],
    objects: &[StateObjectEntryV1],
) -> Result<(), CoreError> {
    // Directories first: an empty one is state a restore must reproduce, and
    // the destination's mode is the source's private mode either way.
    for directory in directories {
        create_directory_chain_v1(staging, &staging.join(directory))?;
    }
    for object in objects {
        let from = source.join(&object.relative_path);
        let to = staging.join(&object.relative_path);
        if !is_canonical_relative_path(&object.relative_path) {
            return Err(typed(
                SearchPlaneErrorCodeV2::StateRootInsecure,
                format!(
                    "source inventory named the non-canonical path {}",
                    object.relative_path
                ),
            ));
        }
        if let Some(parent) = to.parent() {
            create_directory_chain_v1(staging, parent)?;
        }
        let _copied = fs::copy(&from, &to)
            .map_err(|error| storage("copy object into staging", &from, &error))?;
        fsync_file_v1(&to)?;
        let observed =
            fs::metadata(&to).map_err(|error| storage("stat staged object", &to, &error))?;
        if observed.len() != object.byte_size {
            return Err(typed(
                SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
                format!(
                    "staged object {} is {} bytes, the source inventory says {}",
                    object.relative_path,
                    observed.len(),
                    object.byte_size
                ),
            ));
        }
    }
    Ok(())
}

fn create_directory_chain_v1(staging: &Path, target: &Path) -> Result<(), CoreError> {
    let relative = target
        .strip_prefix(staging)
        .map_err(|error| storage("relativize staging directory", target, &error))?;
    let mut cursor = staging.to_path_buf();
    for component in relative.components() {
        cursor.push(component);
        if cursor.is_dir() {
            continue;
        }
        create_private_directory_v1(&cursor)?;
    }
    Ok(())
}

/// The one place a produced root's manifest is assembled and written last.
fn publish_staging_manifest_v1(
    staging: &Path,
    manifest_file_name: &str,
    catalog: &CatalogSnapshotV1,
    fault: &dyn StateMigrationFaultPort,
) -> Result<StateRootManifestV1, CoreError> {
    fault.reach(StateMigrationFaultPointV1::AfterDataSync)?;
    let objects = inventory_state_root_v1(staging, &PRODUCED_ROOT_EXCLUSIONS)?;
    let directories = inventory_state_directories_v1(staging, &PRODUCED_ROOT_EXCLUSIONS)?;
    let manifest = StateRootManifestV1 {
        format_version: STATE_ROOT_MANIFEST_FORMAT_VERSION,
        root_format: StateRootFormatV1::CurrentV1 { manifest: true },
        catalog_digest_hex: catalog.content_digest_hex.clone(),
        catalog_rows: catalog.total_rows(),
        objects,
        directories,
    };
    let _path = write_root_manifest_last_v1(staging, manifest_file_name, &manifest, fault)?;
    Ok(manifest)
}

fn finish_offline_operation_v1(
    operation: OfflineStateOperationV1,
    staging: &Path,
    destination: &Path,
    manifest_file_name: &str,
    imported_legacy_records: u64,
    deep_open: StateRootDeepOpenReceiptV1,
    fault: &dyn StateMigrationFaultPort,
) -> Result<OfflineStateOutcomeV1, CoreError> {
    atomic_cutover_v1(staging, destination)?;
    fault.reach(StateMigrationFaultPointV1::AfterCutoverRename)?;
    let published = read_root_manifest_v1(&destination.join(manifest_file_name))?;
    verify_root_against_manifest_v1(
        destination,
        manifest_file_name,
        &published,
        &PRODUCED_ROOT_EXCLUSIONS,
    )?;
    Ok(OfflineStateOutcomeV1 {
        operation,
        destination_root: destination.to_path_buf(),
        manifest_digest_hex: published.manifest_digest_hex(),
        objects: u64::try_from(published.objects.len()).map_or(u64::MAX, |count| count),
        catalog_digest_hex: published.catalog_digest_hex.clone(),
        catalog_rows: published.catalog_rows,
        imported_legacy_records,
        deep_open,
    })
}

/// `backup-state`: freeze a live or stopped current root into a new
/// directory whose every object is enumerated, digested and manifest-last.
pub fn run_offline_backup_v1(
    request: &OfflineStateRequestV1,
    catalog: &dyn CatalogSnapshotPort,
    deep_open: &dyn StateRootDeepOpenPort,
    fault: &dyn StateMigrationFaultPort,
) -> Result<OfflineStateOutcomeV1, CoreError> {
    let (source, destination) = plan_offline_targets_v1(request)?;
    let _lease = lease_offline_source_v1(&source)?;
    refuse_legacy_state_root_v1(&source)?;
    require_format_v1(&source, StateRootFormatV1::CurrentV1 { manifest: false })?;
    let objects = inventory_state_root_v1(&source, &LIVE_ROOT_EXCLUSIONS)?;
    let directories = inventory_state_directories_v1(&source, &LIVE_ROOT_EXCLUSIONS)?;
    let staging = prepare_staging_v1(&destination)?;
    copy_data_objects_v1(&source, &staging, &directories, &objects)?;
    let catalog_dir = staging.join(STATE_CATALOG_DIRECTORY);
    create_private_directory_v1(&catalog_dir)?;
    let freeze =
        catalog.snapshot_into(&source, &catalog_dir.join(STATE_BACKUP_CATALOG_FILE_NAME))?;
    if !freeze.is_consistent() {
        return Err(typed(
            SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            format!(
                "catalog snapshot of {} digests to {} but the live catalog digests to {}; the snapshot is not a freeze of its source",
                source.display(),
                freeze.snapshot.content_digest_hex,
                freeze.live.content_digest_hex
            ),
        ));
    }
    fsync_directory_v1(&catalog_dir)?;
    // A backup root must be a root the daemon could open, so its staged copy
    // is deep-opened exactly as a migrated or restored one is: the freeze
    // boundary then covers a reconciled root, and a later restore of it is
    // a pure byte-for-byte reproduction. The catalog receipt is taken again
    // afterwards, because reconciliation is a real mutation and the manifest
    // must describe the root it actually publishes.
    let deep = deep_open.deep_open(&staging)?;
    let settled = catalog.verify_snapshot_at(&catalog_dir.join(STATE_BACKUP_CATALOG_FILE_NAME))?;
    let _manifest =
        publish_staging_manifest_v1(&staging, STATE_BACKUP_MANIFEST_FILE_NAME, &settled, fault)?;
    finish_offline_operation_v1(
        OfflineStateOperationV1::Backup,
        &staging,
        &destination,
        STATE_BACKUP_MANIFEST_FILE_NAME,
        0,
        deep,
        fault,
    )
}

/// `restore-state`: rebuild a root from a backup directory, verifying the
/// backup manifest, re-proving the catalog digest, deep-opening the staged
/// root, and cutting over atomically.
pub fn run_offline_restore_v1(
    request: &OfflineStateRequestV1,
    catalog: &dyn CatalogSnapshotPort,
    deep_open: &dyn StateRootDeepOpenPort,
    fault: &dyn StateMigrationFaultPort,
) -> Result<OfflineStateOutcomeV1, CoreError> {
    let (source, destination) = plan_offline_targets_v1(request)?;
    let backup_manifest_path = source.join(STATE_BACKUP_MANIFEST_FILE_NAME);
    let backup_manifest = read_root_manifest_v1(&backup_manifest_path)?;
    let objects = inventory_state_root_v1(&source, &PRODUCED_ROOT_EXCLUSIONS)?;
    let directories = inventory_state_directories_v1(&source, &PRODUCED_ROOT_EXCLUSIONS)?;
    let staging = prepare_staging_v1(&destination)?;
    copy_data_objects_v1(&source, &staging, &directories, &objects)?;
    let staged_catalog = staging
        .join(STATE_CATALOG_DIRECTORY)
        .join(STATE_BACKUP_CATALOG_FILE_NAME);
    let receipt = catalog.verify_snapshot_at(&staged_catalog)?;
    if receipt.content_digest_hex != backup_manifest.catalog_digest_hex {
        return Err(typed(
            SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            format!(
                "restored catalog digests to {} but the backup manifest {} records {}",
                receipt.content_digest_hex,
                backup_manifest_path.display(),
                backup_manifest.catalog_digest_hex
            ),
        ));
    }
    let deep = deep_open.deep_open(&staging)?;
    let _staged_manifest =
        publish_staging_manifest_v1(&staging, STATE_ROOT_MANIFEST_FILE_NAME, &receipt, fault)?;
    finish_offline_operation_v1(
        OfflineStateOperationV1::Restore,
        &staging,
        &destination,
        STATE_ROOT_MANIFEST_FILE_NAME,
        0,
        deep,
        fault,
    )
}

/// `migrate-state`: import a legacy root into a fresh current root.
///
/// The legacy parser is reached only through [`LegacyStateImportPort`], and
/// the produced root carries a migration receipt naming the source format
/// markers before the manifest is written.
pub fn run_offline_migrate_v1(
    request: &OfflineStateRequestV1,
    importer: &dyn LegacyStateImportPort,
    catalog: &dyn CatalogSnapshotPort,
    deep_open: &dyn StateRootDeepOpenPort,
    fault: &dyn StateMigrationFaultPort,
) -> Result<OfflineStateOutcomeV1, CoreError> {
    let (source, destination) = plan_offline_targets_v1(request)?;
    let _lease = lease_offline_source_v1(&source)?;
    require_format_v1(&source, StateRootFormatV1::LegacyV1)?;
    let markers = legacy_state_root_markers_v1(&source);
    let staging = prepare_staging_v1(&destination)?;
    let outcome = importer.import_legacy_into(&source, &staging)?;
    let receipt = render_migration_receipt_v1(&source, &markers, &outcome);
    let receipt_path = staging.join(STATE_MIGRATION_RECEIPT_FILE_NAME);
    fs::write(&receipt_path, receipt)
        .map_err(|error| storage("write migration receipt", &receipt_path, &error))?;
    fsync_file_v1(&receipt_path)?;
    // A migrated root that still carries a legacy artifact would be refused
    // by the very boot it exists to satisfy, so the import is proven
    // complete before anything else is written.
    assert_no_legacy_markers_v1(&staging)?;
    let deep = deep_open.deep_open(&staging)?;
    // The migrated root's catalog digest is read back from the staged
    // catalog the importer produced: nothing else may claim it.
    let staged_catalog = staging
        .join(STATE_CATALOG_DIRECTORY)
        .join(STATE_BACKUP_CATALOG_FILE_NAME);
    let catalog_receipt = catalog.verify_snapshot_at(&staged_catalog)?;
    let _manifest = publish_staging_manifest_v1(
        &staging,
        STATE_ROOT_MANIFEST_FILE_NAME,
        &catalog_receipt,
        fault,
    )?;
    finish_offline_operation_v1(
        OfflineStateOperationV1::Migrate,
        &staging,
        &destination,
        STATE_ROOT_MANIFEST_FILE_NAME,
        outcome.imported_records,
        deep,
        fault,
    )
}

/// The canonical migration receipt: source format markers, target format and
/// the receipt's own version, written before the root manifest.
#[must_use]
pub fn render_migration_receipt_v1(
    source_root: &Path,
    markers: &[String],
    outcome: &LegacyImportOutcomeV1,
) -> String {
    let mut lines: Vec<String> = vec![
        "quanta-index-state-migration-receipt".to_string(),
        format!("format-version {STATE_MIGRATION_RECEIPT_FORMAT_VERSION}"),
        "source-format legacy-v1".to_string(),
        "target-format current-v1".to_string(),
        format!(
            "source-root-digest {}",
            sha256_hex(source_root.to_string_lossy().as_bytes())
        ),
        format!("imported-records {}", outcome.imported_records),
    ];
    let mut markers = markers.to_vec();
    markers.sort();
    for marker in markers {
        lines.push(format!("source-marker {marker}"));
    }
    let mut consumed = outcome.consumed_markers.clone();
    consumed.sort();
    for marker in consumed {
        lines.push(format!("consumed-marker {marker}"));
    }
    let mut body = lines.join("\n");
    body.push('\n');
    body.push_str("receipt-digest ");
    // The digest covers the body exactly as the manifest's covers its own.
    body.push_str(&sha256_hex(body.as_bytes()));
    body.push('\n');
    body
}

/// `verify-state`: re-prove a produced root against its own manifest and the
/// catalog the manifest records.
pub fn run_offline_verify_v1(
    root: &Path,
    catalog: &dyn CatalogSnapshotPort,
) -> Result<OfflineStateVerificationV1, CoreError> {
    let root = refuse_broad_offline_target_v1(root, OfflineRootRoleV1::Source)?;
    refuse_non_private_source_root_v1(&root)?;
    // Two artifact kinds carry a manifest — a produced state root and a
    // backup root — and a root that advertises both is ambiguous, so it is
    // refused rather than resolved.
    let root_manifest = root.join(STATE_ROOT_MANIFEST_FILE_NAME);
    let backup_manifest = root.join(STATE_BACKUP_MANIFEST_FILE_NAME);
    let (manifest_file_name, manifest_path) = match (
        root_manifest.is_file(),
        backup_manifest.is_file(),
    ) {
        (true, true) => {
            return Err(typed(
                SearchPlaneErrorCodeV2::InvalidRequest,
                format!(
                    "root {} advertises both {} and {}; which manifest describes it is ambiguous",
                    root.display(),
                    STATE_ROOT_MANIFEST_FILE_NAME,
                    STATE_BACKUP_MANIFEST_FILE_NAME
                ),
            ));
        }
        (true, false) => (STATE_ROOT_MANIFEST_FILE_NAME, root_manifest),
        (false, true) => (STATE_BACKUP_MANIFEST_FILE_NAME, backup_manifest),
        (false, false) => {
            return Err(typed(
                SearchPlaneErrorCodeV2::NotFound,
                format!(
                    "root {} advertises no {STATE_ROOT_MANIFEST_FILE_NAME} and no {STATE_BACKUP_MANIFEST_FILE_NAME}",
                    root.display()
                ),
            ));
        }
    };
    let manifest = read_root_manifest_v1(&manifest_path)?;
    if manifest.root_format != (StateRootFormatV1::CurrentV1 { manifest: true }) {
        return Err(typed(
            SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            format!(
                "root {} advertises root-format {:?} in {}",
                root.display(),
                manifest.root_format,
                manifest_path.display()
            ),
        ));
    }
    verify_root_against_manifest_v1(
        &root,
        manifest_file_name,
        &manifest,
        &PRODUCED_ROOT_EXCLUSIONS,
    )?;
    let snapshot_path = root
        .join(STATE_CATALOG_DIRECTORY)
        .join(STATE_BACKUP_CATALOG_FILE_NAME);
    let receipt = catalog.verify_snapshot_at(&snapshot_path)?;
    if receipt.content_digest_hex != manifest.catalog_digest_hex {
        return Err(typed(
            SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            format!(
                "root {} catalog digests to {} but its manifest records {}",
                root.display(),
                receipt.content_digest_hex,
                manifest.catalog_digest_hex
            ),
        ));
    }
    let expected_rows = manifest.catalog_rows;
    let observed_rows = receipt.total_rows();
    if expected_rows != observed_rows {
        return Err(typed(
            SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            format!(
                "root {} catalog holds {observed_rows} rows but its manifest records {expected_rows}",
                root.display()
            ),
        ));
    }
    Ok(OfflineStateVerificationV1 {
        root,
        manifest_digest_hex: manifest.manifest_digest_hex(),
        objects: u64::try_from(manifest.objects.len()).map_or(u64::MAX, |count| count),
        catalog_digest_hex: manifest.catalog_digest_hex.clone(),
        catalog_rows: manifest.catalog_rows,
    })
}

/// The staging suffix, re-exported so an operator-facing message and the
/// cleanup helper agree on one name.
#[must_use]
pub const fn staging_suffix_v1() -> &'static str {
    STAGING_DIRECTORY_SUFFIX
}

/// Every legacy marker a staging root must no longer carry once an import
/// succeeded: a migrated root that still advertises a legacy artifact would
/// be refused by the very boot it is meant to satisfy.
pub fn assert_no_legacy_markers_v1(staging: &Path) -> Result<(), CoreError> {
    let markers = legacy_state_root_markers_v1(staging);
    if markers.is_empty() {
        return Ok(());
    }
    Err(typed(
        SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
        format!(
            "imported staging root {} still carries the legacy markers {}",
            staging.display(),
            markers.join(", ")
        ),
    ))
}
