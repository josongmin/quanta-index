//! P10 state migration owner target (SEP-21 S21-11): the offline
//! `migrate-state` / `backup-state` / `restore-state` / `verify-state`
//! workflow over disposable state roots.
//!
//! One test per frozen fixture: a legacy root refused at boot, a backup-API
//! catalog freeze that a raw file copy cannot reproduce, a manifest-last
//! interruption that converges, a crash between the cutover rename and its
//! parent fsync that leaves the complete new root, the refusal matrix, and
//! the tamper/missing/extra object matrix of `verify-state`. Every root here
//! is a `tempfile` directory: no real or shared state root is ever touched.

#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "the owner matrix asserts fixture and refusal invariants with `assert!`; a violated fixture invariant is not a propagatable error"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use quanta_index_catalog::{SqliteCatalog, live_catalog_receipt, verify_snapshot};
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::state_format::{
    NoStateMigrationFaultsV1, OfflineRootRoleV1, STATE_MIGRATION_RECEIPT_FILE_NAME,
    STATE_ROOT_MANIFEST_FILE_NAME, StateMigrationFaultPointV1, StateMigrationFaultPort,
    StateRootFormatV1, atomic_cutover_v1, detect_state_root_format_v1, read_root_manifest_v1,
    refuse_broad_offline_target_v1, refuse_legacy_state_root_v1, refuse_non_empty_destination_v1,
    staging_directory_for_v1,
};
use quanta_index_searchd::app::state_migration::{
    CatalogFreezeV1, CatalogSnapshotPort, CatalogSnapshotV1, OfflineStateCommandV1,
    OfflineStateOperationV1, OfflineStateRequestV1, run_offline_verify_v1,
};
use quanta_index_searchd_runtime::state_migration::{
    render_offline_outcome_v1, run_offline_state_command_with_v1,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BUSY: Duration = Duration::from_secs(2);

/// A fault port that fails exactly one boundary, the way a full disk or a
/// torn write would.
struct ScriptedFaultV1 {
    point: StateMigrationFaultPointV1,
}

impl StateMigrationFaultPort for ScriptedFaultV1 {
    fn reach(&self, point: StateMigrationFaultPointV1) -> Result<(), CoreError> {
        if point == self.point {
            return Err(CoreError::Storage(
                "simulated offline write failure (ENOSPC)".to_string(),
            ));
        }
        Ok(())
    }
}

/// The verify-side catalog port, wired the way the composition root wires it.
struct CatalogVerifierV1;

impl CatalogSnapshotPort for CatalogVerifierV1 {
    fn snapshot_into(
        &self,
        _live_root: &Path,
        _destination_file: &Path,
    ) -> Result<CatalogFreezeV1, CoreError> {
        Err(CoreError::InvalidContract(
            "the verification port only re-reads produced snapshots".to_string(),
        ))
    }

    fn verify_snapshot_at(&self, snapshot_file: &Path) -> Result<CatalogSnapshotV1, CoreError> {
        verify_snapshot(snapshot_file).map(|receipt| CatalogSnapshotV1 {
            content_digest_hex: receipt.content_digest_hex,
            byte_size: receipt.byte_size,
            table_rows: receipt.table_rows,
        })
    }
}

/// The deep-open port stub the engine-level refusal test needs: it is never
/// reached, because the refusal happens while planning the targets.
struct UnreachedDeepOpenV1;

impl quanta_index_searchd::app::state_migration::StateRootDeepOpenPort for UnreachedDeepOpenV1 {
    fn deep_open(
        &self,
        root: &Path,
    ) -> Result<quanta_index_searchd::app::state_migration::StateRootDeepOpenReceiptV1, CoreError>
    {
        Err(CoreError::InvalidContract(format!(
            "the deep open must not be reached for {}",
            root.display()
        )))
    }
}

fn typed_code(error: &CoreError) -> Option<SearchPlaneErrorCodeV2> {
    match error {
        CoreError::Typed { code, .. } => Some(*code),
        CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_) => None,
    }
}

/// The typed code behind a composition-root error, which crosses the crate
/// boundary as `anyhow::Error`.
fn command_code(error: &anyhow::Error) -> Option<SearchPlaneErrorCodeV2> {
    error.downcast_ref::<CoreError>().and_then(typed_code)
}

fn private_root() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
    }
    Ok(root)
}

/// A disposable *current-format* root with real catalog rows, one object per
/// track, and the directories the current layout owns.
fn build_live_root(root: &Path) -> TestResult {
    for directory in [
        "indexes/lexical/generation-v1-aa/g1",
        "indexes/semantic",
        "authorities",
        "activations",
        // The content-addressed fanout the sealed candidates below name:
        // a real seal always leaves its object directory behind, and the
        // `RepoMap` store's open fsyncs it.
        "repo-map/objects/sha256/08/08",
        "repo-map/objects/sha256/12/12",
        "repo-map/objects/sha256/1c/1c",
    ] {
        fs::create_dir_all(root.join(directory))?;
    }
    fs::write(
        root.join("indexes/lexical/generation-v1-aa/g1/manifest.cbor"),
        b"lexical-generation-1",
    )?;
    fs::write(root.join("authorities/history.cbor"), b"history-authority")?;
    let catalog = SqliteCatalog::open(root, BUSY)?;
    let _sealed = catalog.seal_repomap_candidate(
        "repo-a",
        "rev-a",
        1,
        &[7_u8; 32],
        &[8_u8; 32],
        &[9_u8; 32],
        128,
        "projection-a",
    )?;
    let _sealed = catalog.seal_repomap_candidate(
        "repo-a",
        "rev-a",
        2,
        &[17_u8; 32],
        &[18_u8; 32],
        &[19_u8; 32],
        256,
        "projection-b",
    )?;
    drop(catalog);
    Ok(())
}

/// A disposable legacy root: the two `RepoMap` layout directories the
/// current store refuses to mutate.
fn build_legacy_root(root: &Path) -> TestResult {
    fs::create_dir_all(root.join("repo-map/activations"))?;
    fs::create_dir_all(root.join("repo-map/snapshots"))?;
    fs::write(
        root.join("repo-map/activations/repo-a--rev-a.json"),
        b"{\"legacy\":\"activation\"}",
    )?;
    fs::write(
        root.join("repo-map/snapshots/repo-a--rev-a.json"),
        b"{\"legacy\":\"snapshot\"}",
    )?;
    Ok(())
}

fn backup_command(source: &Path, destination: &Path) -> OfflineStateCommandV1 {
    OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Backup,
        source_root: source.to_path_buf(),
        destination_root: Some(destination.to_path_buf()),
    }
}

fn make_backup(source: &Path, destination: &Path) -> TestResult {
    let _outcome = run_offline_state_command_with_v1(
        &backup_command(source, destination),
        &NoStateMigrationFaultsV1,
    )?;
    Ok(())
}

fn socket_paths() -> (PathBuf, PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let dir = std::env::temp_dir();
    (
        dir.join(format!("qi-p10-q-{pid}-{nanos}.sock")),
        dir.join(format!("qi-p10-c-{pid}-{nanos}.sock")),
        dir.join(format!("qi-p10-i-{pid}-{nanos}.sock")),
    )
}

// ---------------------------------------------------------------------------
// Legacy layout: refused at boot, never migrated on the hot path
// ---------------------------------------------------------------------------

#[test]
fn legacy_root_is_detected_and_refused_typed() -> TestResult {
    let root = private_root()?;
    build_legacy_root(root.path())?;
    assert_eq!(
        detect_state_root_format_v1(root.path())?,
        StateRootFormatV1::LegacyV1
    );
    let error =
        refuse_legacy_state_root_v1(root.path()).expect_err("a legacy root must be refused");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported)
    );
    assert!(
        format!("{error}").contains("migrate-state"),
        "the refusal must point at the offline command: {error}"
    );
    Ok(())
}

#[test]
fn a_restored_current_root_with_a_manifest_is_not_refused() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;
    let target = restore_parent.path().join("restored");
    let _outcome = run_offline_state_command_with_v1(
        &OfflineStateCommandV1 {
            operation: OfflineStateOperationV1::Restore,
            source_root: backup,
            destination_root: Some(target.clone()),
        },
        &NoStateMigrationFaultsV1,
    )?;
    assert_eq!(
        detect_state_root_format_v1(&target)?,
        StateRootFormatV1::CurrentV1 { manifest: true }
    );
    refuse_legacy_state_root_v1(&target)?;
    Ok(())
}

#[test]
fn production_boot_refuses_a_legacy_root_instead_of_migrating() -> TestResult {
    let root = private_root()?;
    build_legacy_root(root.path())?;
    // A legacy semantic journal is the artifact the retired boot path used to
    // migrate; it must now be a typed refusal before any adapter opens.
    fs::create_dir_all(root.path().join("semantic"))?;
    fs::write(root.path().join("semantic/journal.cbor"), b"legacy-journal")?;

    let (query_socket, control_socket, ingest_socket) = socket_paths();
    let mut config =
        quanta_index_searchd::app::SearchdConfig::from_state_root(root.path().to_path_buf())
            .try_with_search_corpus_history_retention_limits(
                8,
                16 * 1024 * 1024,
                128,
                256 * 1024 * 1024,
            )
            .expect("valid test retention policy");
    config = quanta_index_searchd::app::SearchdConfig::with_socket_overrides(
        config,
        query_socket,
        control_socket,
    );
    config = quanta_index_searchd::app::SearchdConfig::with_ingest_socket_override(
        config,
        ingest_socket,
    );
    let error = match quanta_index_searchd_runtime::build_runtime(config) {
        Ok(_runtime) => {
            return Err("boot must refuse a legacy state root".into());
        }
        Err(error) => error,
    };
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported),
        "boot must refuse typed: {error}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Backup and restore: one freeze boundary, verified
// ---------------------------------------------------------------------------

#[test]
fn backup_then_restore_round_trip_matches_the_manifest_exactly() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    let restored = restore_parent.path().join("restored-root");

    let _outcome = run_offline_state_command_with_v1(
        &backup_command(source.path(), &backup),
        &NoStateMigrationFaultsV1,
    )?;
    let backup_manifest = read_root_manifest_v1(&backup.join("state-backup-manifest-v1.txt"))?;

    let restore = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Restore,
        source_root: backup,
        destination_root: Some(restored.clone()),
    };
    let _outcome = run_offline_state_command_with_v1(&restore, &NoStateMigrationFaultsV1)?;

    let restored_manifest = read_root_manifest_v1(&restored.join(STATE_ROOT_MANIFEST_FILE_NAME))?;
    assert_eq!(
        restored_manifest.objects, backup_manifest.objects,
        "the restored object inventory must equal the backup's exactly"
    );
    assert_eq!(
        restored_manifest.catalog_digest_hex, backup_manifest.catalog_digest_hex,
        "the restored catalog identity must equal the backup's exactly"
    );
    assert!(restored_manifest.catalog_rows > 0);
    let verified = run_offline_verify_v1(&restored, &CatalogVerifierV1)?;
    assert_eq!(
        verified.catalog_digest_hex,
        backup_manifest.catalog_digest_hex
    );
    assert_eq!(verified.catalog_rows, backup_manifest.catalog_rows);
    Ok(())
}

#[test]
fn backup_carries_committed_rows_a_raw_file_copy_cannot_reproduce() -> TestResult {
    let source = private_root()?;
    let destination = private_root()?;
    build_live_root(source.path())?;
    // A second writer session leaves committed-but-uncheckpointed frames in
    // the WAL, which is exactly the state a hand copy gets wrong.
    let catalog = SqliteCatalog::open(source.path(), BUSY)?;
    let _sealed = catalog.seal_repomap_candidate(
        "repo-a",
        "rev-a",
        3,
        &[27_u8; 32],
        &[28_u8; 32],
        &[29_u8; 32],
        512,
        "projection-c",
    )?;
    let live = live_catalog_receipt(source.path(), BUSY)?;

    // A hand copy of the main database file is not a snapshot.
    let bare = destination.path().join("bare-copy.sqlite");
    let _copied = fs::copy(source.path().join("catalog/catalog-v1.sqlite"), &bare)?;
    let bare_digest = match verify_snapshot(&bare) {
        Ok(receipt) => Some(receipt.content_digest_hex),
        Err(_not_a_snapshot) => None,
    };
    assert_ne!(
        bare_digest.as_deref(),
        Some(live.content_digest_hex.as_str()),
        "a raw main-file copy must never reproduce the live logical digest"
    );

    // The engine's backup API does reproduce it, exactly.
    let produced = destination.path().join("snapshot.sqlite");
    let receipt = quanta_index_catalog::snapshot_catalog_file(
        &source.path().join("catalog/catalog-v1.sqlite"),
        &produced,
        BUSY,
    )?;
    assert_eq!(
        receipt.content_digest_hex, live.content_digest_hex,
        "the backup-API snapshot must be logically equal to the live catalog"
    );
    assert_eq!(receipt.table_rows, live.table_rows);
    drop(catalog);
    Ok(())
}

// ---------------------------------------------------------------------------
// Manifest-last ordering and interrupted operations
// ---------------------------------------------------------------------------

#[test]
fn an_interruption_before_the_manifest_leaves_no_manifest_and_converges_on_retry() -> TestResult {
    let source = private_root()?;
    let destination_parent = private_root()?;
    build_live_root(source.path())?;
    let destination = destination_parent.path().join("frozen");
    let staging = staging_directory_for_v1(&destination);

    let fault = ScriptedFaultV1 {
        point: StateMigrationFaultPointV1::BeforeManifestSync,
    };
    let error =
        run_offline_state_command_with_v1(&backup_command(source.path(), &destination), &fault)
            .expect_err("the scripted write failure must fail the backup");
    assert!(
        matches!(
            error.downcast_ref::<CoreError>(),
            Some(CoreError::Storage(_))
        ),
        "got {error:?}"
    );
    assert!(!destination.exists(), "no cutover may have happened");
    assert!(
        staging.is_dir(),
        "the interrupted preparation is still there"
    );
    assert!(
        !staging.join(STATE_ROOT_MANIFEST_FILE_NAME).exists(),
        "a crash before the manifest write must leave no manifest to mislead"
    );

    // The retry converges: the stale staging directory is not an authority.
    let _outcome = run_offline_state_command_with_v1(
        &backup_command(source.path(), &destination),
        &NoStateMigrationFaultsV1,
    )?;
    assert!(destination.join("state-backup-manifest-v1.txt").is_file());
    assert!(
        !staging.exists(),
        "the staging directory is consumed by the cutover"
    );
    Ok(())
}

#[test]
fn a_crash_after_the_cutover_rename_leaves_the_complete_new_root() -> TestResult {
    let source = private_root()?;
    let destination_parent = private_root()?;
    build_live_root(source.path())?;
    let destination = destination_parent.path().join("frozen");

    let fault = ScriptedFaultV1 {
        point: StateMigrationFaultPointV1::AfterCutoverRename,
    };
    let _error =
        run_offline_state_command_with_v1(&backup_command(source.path(), &destination), &fault)
            .expect_err("the scripted post-rename failure must surface");
    assert!(
        destination.join("state-backup-manifest-v1.txt").is_file(),
        "the rename publishes the complete new root, never a mixture"
    );
    let manifest = read_root_manifest_v1(&destination.join("state-backup-manifest-v1.txt"))?;
    assert!(!manifest.objects.is_empty());
    assert!(!staging_directory_for_v1(&destination).exists());
    Ok(())
}

#[test]
fn atomic_cutover_never_replaces_an_existing_root() -> TestResult {
    let staging = private_root()?;
    let destination_parent = private_root()?;
    let destination = destination_parent.path().join("occupied");
    fs::create_dir_all(&destination)?;
    fs::write(destination.join("pre-existing"), b"do not clobber")?;
    let error = atomic_cutover_v1(staging.path(), &destination)
        .expect_err("a cutover must refuse an existing destination");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    assert_eq!(
        fs::read(destination.join("pre-existing"))?,
        b"do not clobber"
    );
    Ok(())
}

#[test]
fn a_restore_interrupted_before_the_manifest_leaves_the_destination_absent() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;
    let restored = restore_parent.path().join("restored");

    let fault = ScriptedFaultV1 {
        point: StateMigrationFaultPointV1::BeforeCutoverRename,
    };
    let restore = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Restore,
        source_root: backup,
        destination_root: Some(restored.clone()),
    };
    let _error = run_offline_state_command_with_v1(&restore, &fault)
        .expect_err("the scripted pre-rename failure must surface");
    assert!(!restored.exists(), "no cutover may have happened");

    let _outcome = run_offline_state_command_with_v1(&restore, &NoStateMigrationFaultsV1)?;
    assert!(restored.join(STATE_ROOT_MANIFEST_FILE_NAME).is_file());
    let _verified = run_offline_verify_v1(&restored, &CatalogVerifierV1)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Refusal matrix: before any mutation
// ---------------------------------------------------------------------------

#[test]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the owner matrix keeps one `TestResult` signature so a future assertion can propagate"
)]
fn broad_relative_and_traversing_targets_are_refused() -> TestResult {
    for path in [
        PathBuf::from("/"),
        PathBuf::from("/usr"),
        PathBuf::from("/etc"),
        PathBuf::from("relative/state"),
        PathBuf::from("/tmp/../etc/state"),
    ] {
        let error = refuse_broad_offline_target_v1(&path, OfflineRootRoleV1::Destination)
            .expect_err("a broad, relative or traversing target must be refused");
        assert_eq!(
            typed_code(&error),
            Some(SearchPlaneErrorCodeV2::InvalidRequest),
            "for {}",
            path.display()
        );
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        let error = refuse_broad_offline_target_v1(Path::new(&home), OfflineRootRoleV1::Source)
            .expect_err("the home directory is never a state root");
        assert_eq!(
            typed_code(&error),
            Some(SearchPlaneErrorCodeV2::InvalidRequest)
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_symlinked_destination_is_refused() -> TestResult {
    let real = private_root()?;
    let alias_parent = private_root()?;
    let alias = alias_parent.path().join("alias");
    std::os::unix::fs::symlink(real.path(), &alias)?;
    let error = refuse_broad_offline_target_v1(&alias, OfflineRootRoleV1::Destination)
        .expect_err("a symlinked target must be refused");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_non_private_source_root_is_refused() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let source = private_root()?;
    build_live_root(source.path())?;
    fs::set_permissions(source.path(), fs::Permissions::from_mode(0o755))?;
    let destination_parent = private_root()?;
    let error = run_offline_state_command_with_v1(
        &backup_command(source.path(), &destination_parent.path().join("frozen")),
        &NoStateMigrationFaultsV1,
    )
    .expect_err("a root others can read must be refused");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    fs::set_permissions(source.path(), fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[test]
fn a_live_lease_owner_excludes_an_offline_operation() -> TestResult {
    let source = private_root()?;
    let destination_parent = private_root()?;
    build_live_root(source.path())?;
    let lease = StateRootLease::acquire(source.path())?;
    let error = run_offline_state_command_with_v1(
        &backup_command(source.path(), &destination_parent.path().join("frozen")),
        &NoStateMigrationFaultsV1,
    )
    .expect_err("a live lease owner must exclude the offline operation");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInUse)
    );
    drop(lease);
    Ok(())
}

#[test]
fn a_non_empty_destination_and_a_self_destination_are_refused() -> TestResult {
    let source = private_root()?;
    build_live_root(source.path())?;
    let populated = private_root()?;
    fs::write(populated.path().join("occupied"), b"x")?;
    let error = refuse_non_empty_destination_v1(populated.path(), source.path())
        .expect_err("a populated destination must be refused");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    let error = refuse_non_empty_destination_v1(source.path(), source.path())
        .expect_err("a source that is its own destination must be refused");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    Ok(())
}

#[test]
fn a_legacy_source_is_refused_by_backup_and_required_by_migrate() -> TestResult {
    let legacy = private_root()?;
    let current = private_root()?;
    let parent = private_root()?;
    build_legacy_root(legacy.path())?;
    build_live_root(current.path())?;

    let error = run_offline_state_command_with_v1(
        &backup_command(legacy.path(), &parent.path().join("frozen")),
        &NoStateMigrationFaultsV1,
    )
    .expect_err("backup-state refuses a legacy root");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported)
    );

    let migrate = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Migrate,
        source_root: current.path().to_path_buf(),
        destination_root: Some(parent.path().join("migrated")),
    };
    let error = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)
        .expect_err("migrate-state requires a legacy root");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported)
    );
    Ok(())
}

#[test]
fn migrate_state_refuses_to_overwrite_its_own_source() -> TestResult {
    let legacy = private_root()?;
    build_legacy_root(legacy.path())?;
    let migrate = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Migrate,
        source_root: legacy.path().to_path_buf(),
        destination_root: Some(legacy.path().to_path_buf()),
    };
    let error = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)
        .expect_err("a destructive self-target must be refused");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    assert_eq!(
        detect_state_root_format_v1(legacy.path())?,
        StateRootFormatV1::LegacyV1
    );
    Ok(())
}

#[test]
fn migrate_state_refuses_a_missing_destination() -> TestResult {
    let legacy = private_root()?;
    build_legacy_root(legacy.path())?;
    let migrate = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Migrate,
        source_root: legacy.path().to_path_buf(),
        destination_root: None,
    };
    let error = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)
        .expect_err("a producing operation without a destination must be refused");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Verification matrix
// ---------------------------------------------------------------------------

#[test]
fn verify_state_refuses_a_missing_unadvertised_and_tampered_object() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let frozen = parent.path().join("frozen");
    make_backup(source.path(), &frozen)?;

    // Extra object: the manifest does not name it.
    fs::write(frozen.join("unadvertised.bin"), b"extra")?;
    let error = run_offline_verify_v1(&frozen, &CatalogVerifierV1)
        .expect_err("an unadvertised object must be refused");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch)
    );
    fs::remove_file(frozen.join("unadvertised.bin"))?;

    // Missing object: the manifest names a file the root no longer has. The
    // victim is taken from the manifest itself, so the test can never pick a
    // path the freeze did not advertise.
    let backup_manifest = read_root_manifest_v1(&frozen.join("state-backup-manifest-v1.txt"))?;
    let advertised = backup_manifest
        .objects
        .iter()
        .find(|object| object.relative_path.starts_with("authorities/"))
        .ok_or("the fixture advertises an authority object")?;
    let victim = frozen.join(&advertised.relative_path);
    let bytes = fs::read(&victim)?;
    fs::remove_file(&victim)?;
    let error = run_offline_verify_v1(&frozen, &CatalogVerifierV1)
        .expect_err("a missing advertised object must be refused");
    assert_eq!(typed_code(&error), Some(SearchPlaneErrorCodeV2::NotFound));
    fs::write(&victim, &bytes)?;

    // Tampered manifest: the self-digest no longer covers the body.
    let manifest_path = frozen.join("state-backup-manifest-v1.txt");
    let manifest = fs::read_to_string(&manifest_path)?;
    let tampered = manifest.replace("catalog-rows ", "catalog-rows 9");
    assert_ne!(tampered, manifest, "the fixture must change the body");
    fs::write(&manifest_path, &tampered)?;
    let error = run_offline_verify_v1(&frozen, &CatalogVerifierV1)
        .expect_err("a tampered manifest must be refused");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch)
    );
    Ok(())
}

#[test]
fn verify_state_refuses_a_root_that_advertises_no_manifest() -> TestResult {
    let source = private_root()?;
    build_live_root(source.path())?;
    let error = run_offline_verify_v1(source.path(), &CatalogVerifierV1)
        .expect_err("a root without a manifest is not verifiable");
    assert_eq!(typed_code(&error), Some(SearchPlaneErrorCodeV2::NotFound));
    Ok(())
}

#[test]
fn verify_state_refuses_a_frozen_root_whose_catalog_was_replaced() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    let other = private_root()?;
    build_live_root(source.path())?;
    let frozen = parent.path().join("frozen");
    make_backup(source.path(), &frozen)?;
    build_live_root(other.path())?;
    let _copied = fs::copy(
        other.path().join("catalog/catalog-v1.sqlite"),
        frozen.join("catalog/catalog-v1.sqlite"),
    )?;
    let error = run_offline_verify_v1(&frozen, &CatalogVerifierV1)
        .expect_err("a swapped catalog must be refused");
    assert!(typed_code(&error).is_some(), "got {error:?}");
    Ok(())
}

#[test]
fn verify_state_runs_from_the_command_surface_without_a_destination() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;
    let verify = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Verify,
        source_root: backup,
        destination_root: None,
    };
    let outcome = run_offline_state_command_with_v1(&verify, &NoStateMigrationFaultsV1)?;
    assert!(
        render_offline_outcome_v1(&verify, &outcome).starts_with("verify-state:"),
        "verify-state must be reachable from the command surface"
    );
    Ok(())
}

#[test]
fn restore_refuses_a_backup_root_without_a_backup_manifest() -> TestResult {
    let backup = private_root()?;
    let parent = private_root()?;
    fs::write(backup.path().join("stray.bin"), b"no manifest here")?;
    let restore = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Restore,
        source_root: backup.path().to_path_buf(),
        destination_root: Some(parent.path().join("restored")),
    };
    let error = run_offline_state_command_with_v1(&restore, &NoStateMigrationFaultsV1)
        .expect_err("a backup root without a manifest is not restorable");
    assert_eq!(command_code(&error), Some(SearchPlaneErrorCodeV2::NotFound));
    Ok(())
}

#[test]
fn restore_refuses_a_tampered_backup_manifest() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;
    let manifest_path = backup.join("state-backup-manifest-v1.txt");
    let manifest = fs::read_to_string(&manifest_path)?;
    fs::write(
        &manifest_path,
        manifest.replace("catalog-rows ", "catalog-rows 7"),
    )?;
    let restore = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Restore,
        source_root: backup,
        destination_root: Some(restore_parent.path().join("restored")),
    };
    let error = run_offline_state_command_with_v1(&restore, &NoStateMigrationFaultsV1)
        .expect_err("a tampered backup manifest is refused");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch)
    );
    assert!(!restore_parent.path().join("restored").exists());
    Ok(())
}

#[test]
fn the_backup_root_carries_the_backup_manifest_name_only() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;
    assert!(backup.join("state-backup-manifest-v1.txt").is_file());
    assert!(!backup.join(STATE_ROOT_MANIFEST_FILE_NAME).exists());
    Ok(())
}

// ---------------------------------------------------------------------------
// Offline migration of a legacy root
// ---------------------------------------------------------------------------

#[test]
fn migrate_state_carries_legacy_bytes_into_a_fresh_current_root() -> TestResult {
    let legacy = private_root()?;
    let parent = private_root()?;
    build_legacy_root(legacy.path())?;
    let migrated = parent.path().join("migrated");

    let migrate = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Migrate,
        source_root: legacy.path().to_path_buf(),
        destination_root: Some(migrated.clone()),
    };
    let outcome = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)?;
    let rendered = render_offline_outcome_v1(&migrate, &outcome);
    assert!(rendered.contains("migrate-state"), "{rendered}");

    assert_eq!(
        detect_state_root_format_v1(&migrated)?,
        StateRootFormatV1::CurrentV1 { manifest: true }
    );
    // The legacy bytes survived verbatim, in a namespace no adapter serves.
    assert_eq!(
        fs::read(migrated.join("legacy-import/activations/repo-a--rev-a.json"))?,
        b"{\"legacy\":\"activation\"}"
    );
    // The receipt names source and target format, and the manifest covers it.
    let receipt = fs::read_to_string(migrated.join(STATE_MIGRATION_RECEIPT_FILE_NAME))?;
    assert!(receipt.contains("format-version 1"), "{receipt}");
    assert!(receipt.contains("source-format legacy-v1"), "{receipt}");
    assert!(receipt.contains("target-format current-v1"), "{receipt}");
    assert!(
        receipt.contains("source-marker repo-map/activations"),
        "{receipt}"
    );
    assert!(
        receipt.contains("consumed-marker repo-map/snapshots"),
        "{receipt}"
    );
    let manifest = read_root_manifest_v1(&migrated.join(STATE_ROOT_MANIFEST_FILE_NAME))?;
    assert!(manifest.objects.iter().any(|object| {
        object
            .relative_path
            .ends_with(STATE_MIGRATION_RECEIPT_FILE_NAME)
    }));
    let _verified = run_offline_verify_v1(&migrated, &CatalogVerifierV1)?;
    // The legacy source is untouched: it still detects as legacy.
    assert_eq!(
        detect_state_root_format_v1(legacy.path())?,
        StateRootFormatV1::LegacyV1
    );
    Ok(())
}

#[test]
fn an_interrupted_migration_leaves_no_new_root_and_keeps_the_legacy_source() -> TestResult {
    let legacy = private_root()?;
    let parent = private_root()?;
    build_legacy_root(legacy.path())?;
    let migrated = parent.path().join("migrated");
    let migrate = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Migrate,
        source_root: legacy.path().to_path_buf(),
        destination_root: Some(migrated.clone()),
    };
    let fault = ScriptedFaultV1 {
        point: StateMigrationFaultPointV1::AfterDataSync,
    };
    let _error = run_offline_state_command_with_v1(&migrate, &fault)
        .expect_err("the scripted post-data failure must surface");
    assert!(!migrated.exists(), "no cutover may have happened");
    assert_eq!(
        detect_state_root_format_v1(legacy.path())?,
        StateRootFormatV1::LegacyV1,
        "the legacy source is never mutated"
    );
    // The retry converges on the same destination.
    let _outcome = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)?;
    assert!(migrated.join(STATE_ROOT_MANIFEST_FILE_NAME).is_file());
    let _verified = run_offline_verify_v1(&migrated, &CatalogVerifierV1)?;
    Ok(())
}

#[test]
fn the_engine_refuses_a_self_target_before_any_lease() -> TestResult {
    let legacy = private_root()?;
    build_legacy_root(legacy.path())?;
    let request = OfflineStateRequestV1 {
        operation: OfflineStateOperationV1::Backup,
        source_root: legacy.path().to_path_buf(),
        destination_root: legacy.path().to_path_buf(),
    };
    let error = quanta_index_searchd::app::state_migration::run_offline_backup_v1(
        &request,
        &CatalogVerifierV1,
        &UnreachedDeepOpenV1,
        &NoStateMigrationFaultsV1,
    )
    .expect_err("engine-level self-target refusal");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    Ok(())
}
