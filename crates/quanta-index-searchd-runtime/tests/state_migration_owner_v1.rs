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
    CatalogFreezeV1, CatalogSnapshotPort, CatalogSnapshotV1, LegacyImportOutcomeV1,
    LegacyStateImportPort, OfflineSourceSessionV1, OfflineStateCommandV1, OfflineStateOperationV1,
    OfflineStateVerificationV1, SourceFreezeReceiptV1, StateRootDeepOpenPort,
    StateRootDeepOpenReceiptV1, run_offline_backup_v1, run_offline_migrate_v1,
    run_offline_verify_v1,
};
use quanta_index_searchd_harness::E2eRuntime;
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

/// Custody for a current root: acquire the daemon lease, then freeze. The
/// setup lease that pre-seeds the lock file (daemon-ran-here fixture state)
/// must be dropped before this runs.
fn current_session(root: &Path) -> Result<OfflineSourceSessionV1, Box<dyn std::error::Error>> {
    let lease = StateRootLease::acquire(root)?;
    Ok(OfflineSourceSessionV1::open_current(lease)?)
}

/// Custody for a legacy root: read-only, creating nothing.
fn legacy_session(root: &Path) -> Result<OfflineSourceSessionV1, Box<dyn std::error::Error>> {
    Ok(OfflineSourceSessionV1::open_legacy_read_only(root)?)
}

/// Custody for a produced backup root: read-only, creating nothing.
fn backup_session(root: &Path) -> Result<OfflineSourceSessionV1, Box<dyn std::error::Error>> {
    Ok(OfflineSourceSessionV1::open_produced_backup(root)?)
}

/// The frozen inventory of a current root, as the drift gate compares it.
fn freeze_current(root: &Path) -> Result<SourceFreezeReceiptV1, Box<dyn std::error::Error>> {
    Ok(current_session(root)?.before().clone())
}

/// The frozen inventory of a legacy root: the whole tree, no exclusions.
fn freeze_legacy(root: &Path) -> Result<SourceFreezeReceiptV1, Box<dyn std::error::Error>> {
    Ok(legacy_session(root)?.before().clone())
}

/// The frozen inventory of a produced backup root.
fn freeze_backup(root: &Path) -> Result<SourceFreezeReceiptV1, Box<dyn std::error::Error>> {
    Ok(backup_session(root)?.before().clone())
}

/// Verify a produced current root through its lease-bound session.
fn verify_current(root: &Path) -> Result<OfflineStateVerificationV1, Box<dyn std::error::Error>> {
    let session = current_session(root)?;
    Ok(run_offline_verify_v1(&session, &CatalogVerifierV1)?)
}

/// Verify a backup root through its read-only session.
fn verify_backup(root: &Path) -> Result<OfflineStateVerificationV1, Box<dyn std::error::Error>> {
    let session = backup_session(root)?;
    Ok(run_offline_verify_v1(&session, &CatalogVerifierV1)?)
}

/// Every file and directory name under `root`, sorted. The catalog subtree
/// is excluded: the engine's backup API necessarily opens the live catalog,
/// and directory mtimes there are vendor bookkeeping, not source state.
/// Everywhere else the name set must be exactly stable across an operation.
fn tree_names_outside_catalog(root: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut names = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|error| format!("relativize {}: {error}", path.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "catalog" || relative.starts_with("catalog/") {
                continue;
            }
            names.push(relative);
            if entry.file_type()?.is_dir() {
                stack.push(path);
            }
        }
    }
    names.sort();
    Ok(names)
}

/// No migration marker, migration receipt or produced-current manifest may
/// appear inside a source root: those authorities live in staging/destination
/// only. Track-local files such as `indexes/.../manifest.cbor` are ordinary
/// source payload and must not be rejected by a substring match. Lock files
/// are covered separately: the daemon's own pre-existing lock may stand, but
/// the name set ([`tree_names_outside_catalog`]) must prove no lock was
/// added, and callers compare its bytes directly.
fn assert_no_source_markers(root: &Path) -> TestResult {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(
                name != "MIGRATED"
                    && !name.starts_with("MIGRATED.")
                    && !name.starts_with(".MIGRATED.")
                    && name != STATE_ROOT_MANIFEST_FILE_NAME
                    && name != STATE_MIGRATION_RECEIPT_FILE_NAME,
                "source root {} holds the authority file {}",
                root.display(),
                path.display()
            );
            if entry.file_type()?.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(())
}

/// The logical catalog digest of a live root, for the before/after proof
/// across the catalog subtree the byte freeze excludes.
fn live_catalog_digest(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(live_catalog_receipt(root, BUSY)?.content_digest_hex)
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

    // The boot refusal runs through the harness-owned runtime (TOPT-03):
    // sockets and retention come from the harness builder, and `start`
    // surfaces the refusal synchronously instead of a client observing it.
    let mut runtime = E2eRuntime::boot_in(root.path())?;
    let error = match runtime.start() {
        Ok(()) => {
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
    let verified = verify_current(&restored)?;
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
    let expected_parent = private_root()?;
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
    assert!(
        !staging_directory_for_v1(&destination).exists(),
        "no staging residue may survive the cutover"
    );

    // The expected root: the same source backed up with no fault. Both
    // manifests describe identical bytes, so completeness is digest
    // equivalence — not "the object list is non-empty".
    let expected_root = expected_parent.path().join("expected");
    make_backup(source.path(), &expected_root)?;
    let expected = read_root_manifest_v1(&expected_root.join("state-backup-manifest-v1.txt"))?;
    let manifest = read_root_manifest_v1(&destination.join("state-backup-manifest-v1.txt"))?;
    assert_eq!(
        manifest.format_version, expected.format_version,
        "post-cutover format version must match the clean backup"
    );
    assert_eq!(
        manifest.root_format, expected.root_format,
        "post-cutover root format must match the clean backup"
    );
    assert_eq!(
        manifest.catalog_rows, expected.catalog_rows,
        "post-cutover catalog row count must match the clean backup"
    );

    // Object equivalence modulo the deep-open reconciliation outputs.
    // The backup's deep open reconciles the staged root before the
    // manifest is written — a real, run-specific mutation (fresh root
    // uuid, quarantine incidents, rewritten catalog snapshot bytes) — so
    // those entries cannot be byte-identical across runs. Every other
    // entry is a pure copy of the source and must match the clean backup
    // in identity, size, and digest exactly; the reconciliation outputs
    // must match in shape (same count, same stable names).
    let (mut stable, reconciled): (Vec<_>, Vec<_>) = manifest
        .objects
        .iter()
        .partition(|entry| !is_reconciliation_output(&entry.relative_path));
    let (mut expected_stable, expected_reconciled): (Vec<_>, Vec<_>) = expected
        .objects
        .iter()
        .partition(|entry| !is_reconciliation_output(&entry.relative_path));
    stable.sort();
    expected_stable.sort();
    assert_eq!(
        stable, expected_stable,
        "every copied object must match the clean backup in path, size, and digest"
    );
    assert_eq!(
        reconciled.len(),
        expected_reconciled.len(),
        "the reconciliation must publish the same object shape on both runs"
    );
    let mut reconciled_names: Vec<&str> = reconciled
        .iter()
        .map(|entry| entry.relative_path.as_str())
        .collect();
    let mut expected_reconciled_names: Vec<&str> = expected_reconciled
        .iter()
        .map(|entry| entry.relative_path.as_str())
        .collect();
    reconciled_names.sort_unstable();
    expected_reconciled_names.sort_unstable();
    // Stable reconciliation names (the catalog snapshot, the root uuid)
    // are present on both sides; quarantine incident names embed
    // run-specific hashes and are compared by count, not spelling.
    for name in ["catalog/catalog-v1.sqlite", "repo-map/root-uuid.bin"] {
        assert!(
            reconciled_names.contains(&name),
            "post-cutover reconciliation must publish {name}"
        );
        assert!(
            expected_reconciled_names.contains(&name),
            "clean-backup reconciliation must publish {name}"
        );
    }
    let incidents = reconciled_names
        .iter()
        .filter(|name| name.starts_with("repo-map/quarantine/"))
        .count();
    let expected_incidents = expected_reconciled_names
        .iter()
        .filter(|name| name.starts_with("repo-map/quarantine/"))
        .count();
    assert_eq!(
        incidents, expected_incidents,
        "both runs must quarantine the same incident count"
    );

    // Directories likewise, modulo the run-specific incident leaves.
    let mut directories: Vec<&str> = manifest
        .directories
        .iter()
        .map(String::as_str)
        .filter(|dir| !is_incident_dir(dir))
        .collect();
    let mut expected_directories: Vec<&str> = expected
        .directories
        .iter()
        .map(String::as_str)
        .filter(|dir| !is_incident_dir(dir))
        .collect();
    directories.sort_unstable();
    expected_directories.sort_unstable();
    assert_eq!(
        directories, expected_directories,
        "post-cutover directories must match the clean backup outside incident leaves"
    );

    // The production offline verifier re-proves the published root
    // against its own manifest and catalog: every advertised object
    // exists with the exact size and digest, nothing extra exists, and
    // the catalog snapshot digests to the manifest identity.
    let verified = verify_backup(&destination)?;
    assert_eq!(
        verified.catalog_digest_hex, manifest.catalog_digest_hex,
        "the verifier must re-prove the published catalog identity"
    );
    assert_eq!(verified.catalog_rows, manifest.catalog_rows);
    assert_eq!(
        verified.manifest_digest_hex,
        manifest.manifest_digest_hex(),
        "the verifier must re-prove the published manifest digest"
    );
    assert_eq!(
        verified.objects,
        u64::try_from(manifest.objects.len()).map_or(u64::MAX, |count| count),
        "the verifier must account for every published object"
    );
    Ok(())
}

/// Entries the backup's deep open rewrites per run: the reconciled
///
/// catalog snapshot bytes, the fresh root uuid, and quarantine
/// incidents. Exempt from cross-run digest equality; still covered by
/// the verifier's internal consistency proof.
fn is_reconciliation_output(relative_path: &str) -> bool {
    relative_path.starts_with("catalog/")
        || relative_path == "repo-map/root-uuid.bin"
        || relative_path.starts_with("repo-map/quarantine/")
}

/// Run-specific quarantine incident leaves under the stable
/// `repo-map/quarantine/incidents/sha256/` fanout.
fn is_incident_dir(dir: &str) -> bool {
    const FANOUT: &str = "repo-map/quarantine/incidents/sha256/";
    dir.len() > FANOUT.len() && dir.starts_with(FANOUT)
}

/// Mutation control for the completeness oracle: deleting or corrupting a
///
/// single published object must fail production verification, and
/// restoring it must pass again — proving the failure names the mutation,
/// not the fixture.
#[test]
fn deleting_or_corrupting_one_published_object_fails_verification() -> TestResult {
    let source = private_root()?;
    let destination_parent = private_root()?;
    build_live_root(source.path())?;
    let destination = destination_parent.path().join("published");
    make_backup(source.path(), &destination)?;
    let _verified = verify_backup(&destination)?;

    let manifest = read_root_manifest_v1(&destination.join("state-backup-manifest-v1.txt"))?;
    let victim = manifest
        .objects
        .first()
        .ok_or("the fixture publishes at least one object")?;
    let victim_path = destination.join(&victim.relative_path);
    let original = fs::read(&victim_path)?;

    // Delete: the verifier must report the missing manifest object.
    fs::remove_file(&victim_path)?;
    let session = backup_session(&destination)?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
        .expect_err("a deleted object must fail verification");
    assert!(
        format!("{error:?}").contains(&victim.relative_path),
        "the failure must name the deleted object: {error:?}"
    );
    fs::write(&victim_path, &original)?;
    let _verified = verify_backup(&destination)?;

    // Corrupt: the verifier must report the digest mismatch.
    let mut corrupted = original.clone();
    let last = corrupted.len().saturating_sub(1);
    if let Some(tail) = corrupted.get_mut(last) {
        *tail ^= 0xFF;
    }
    fs::write(&victim_path, &corrupted)?;
    let session = backup_session(&destination)?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
        .expect_err("a corrupted object must fail verification");
    assert!(
        format!("{error:?}").contains(&victim.relative_path),
        "the failure must name the corrupted object: {error:?}"
    );
    fs::write(&victim_path, &original)?;
    let _verified = verify_backup(&destination)?;
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
    let _verified = verify_current(&restored)?;
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
    let session = backup_session(&frozen)?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
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
    let session = backup_session(&frozen)?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
        .expect_err("a missing advertised object must be refused");
    assert_eq!(typed_code(&error), Some(SearchPlaneErrorCodeV2::NotFound));
    fs::write(&victim, &bytes)?;

    // Tampered manifest: the self-digest no longer covers the body.
    let manifest_path = frozen.join("state-backup-manifest-v1.txt");
    let manifest = fs::read_to_string(&manifest_path)?;
    let tampered = manifest.replace("catalog-rows ", "catalog-rows 9");
    assert_ne!(tampered, manifest, "the fixture must change the body");
    fs::write(&manifest_path, &tampered)?;
    let error = OfflineSourceSessionV1::open_produced_backup(&frozen)
        .expect_err("a tampered manifest must be refused at source admission");
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
    // Through the command surface: the manifest peek refuses `NotFound`
    // before any session opens, so nothing is created inside the source.
    let before = tree_names_outside_catalog(source.path())?;
    let verify = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Verify,
        source_root: source.path().to_path_buf(),
        destination_root: None,
    };
    let error = run_offline_state_command_with_v1(&verify, &NoStateMigrationFaultsV1)
        .expect_err("a root without a manifest is not verifiable");
    assert_eq!(command_code(&error), Some(SearchPlaneErrorCodeV2::NotFound));
    assert_eq!(
        tree_names_outside_catalog(source.path())?,
        before,
        "the refused verify must not create anything inside the source"
    );
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
    let session = backup_session(&frozen)?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
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
    let _verified = verify_current(&migrated)?;
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
    let _verified = verify_current(&migrated)?;
    Ok(())
}

#[test]
fn the_engine_refuses_a_self_target_before_any_lease() -> TestResult {
    // Migrated to the session contract (W10 R3): the request struct is gone;
    // the engine takes the session plus a destination. A legacy session used
    // as its own backup destination is refused before any staging: wrong
    // custody first, then the self-target planning refusal with a matching
    // current session below.
    let legacy = private_root()?;
    build_legacy_root(legacy.path())?;
    let session = legacy_session(legacy.path())?;
    let error = quanta_index_searchd::app::state_migration::run_offline_backup_v1(
        &session,
        legacy.path(),
        &CatalogVerifierV1,
        &UnreachedDeepOpenV1,
        &NoStateMigrationFaultsV1,
    )
    .expect_err("a legacy session is not backup custody");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );

    let current = private_root()?;
    build_live_root(current.path())?;
    let setup = StateRootLease::acquire(current.path())?;
    drop(setup);
    let session = current_session(current.path())?;
    let error = quanta_index_searchd::app::state_migration::run_offline_backup_v1(
        &session,
        current.path(),
        &CatalogVerifierV1,
        &UnreachedDeepOpenV1,
        &NoStateMigrationFaultsV1,
    )
    .expect_err("engine-level self-target refusal");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    assert!(
        !staging_directory_for_v1(current.path()).exists(),
        "no staging may be prepared for a self target"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// R3 immutable-source custody: freeze, drift gate, read-only sessions
// ---------------------------------------------------------------------------

/// Stub catalog port for engine-level drift tests: a fixed logical freeze
/// that never touches the filesystem.
struct StubCatalogV1;

fn stub_snapshot() -> CatalogSnapshotV1 {
    CatalogSnapshotV1 {
        content_digest_hex: "stub-catalog-digest".to_string(),
        byte_size: 8,
        table_rows: vec![("stub-table".to_string(), 3)],
    }
}

fn stub_freeze() -> CatalogFreezeV1 {
    CatalogFreezeV1 {
        live: stub_snapshot(),
        snapshot: stub_snapshot(),
    }
}

impl CatalogSnapshotPort for StubCatalogV1 {
    fn snapshot_into(
        &self,
        _live_root: &Path,
        _destination_file: &Path,
    ) -> Result<CatalogFreezeV1, CoreError> {
        Ok(stub_freeze())
    }

    fn verify_snapshot_at(&self, _snapshot_file: &Path) -> Result<CatalogSnapshotV1, CoreError> {
        Ok(stub_snapshot())
    }
}

/// Stub deep open for engine-level drift tests: the gate under test is the
/// source freeze, not the produced root.
struct StubDeepOpenV1;

impl StateRootDeepOpenPort for StubDeepOpenV1 {
    fn deep_open(&self, _root: &Path) -> Result<StateRootDeepOpenReceiptV1, CoreError> {
        Ok(StateRootDeepOpenReceiptV1::default())
    }
}

/// Stub legacy importer for the engine-level migrate drift test: carries one
/// marker file into staging and nothing else.
struct StubImporterV1;

impl LegacyStateImportPort for StubImporterV1 {
    fn import_legacy_into(
        &self,
        _source_root: &Path,
        staging_root: &Path,
    ) -> Result<LegacyImportOutcomeV1, CoreError> {
        fs::write(staging_root.join("imported.bin"), b"stub-import")
            .map_err(|error| CoreError::Storage(format!("stub import write: {error}")))?;
        Ok(LegacyImportOutcomeV1 {
            imported_records: 1,
            consumed_markers: Vec::new(),
        })
    }
}

/// A current root's byte fingerprint is identical before and after a
/// successful backup, and no marker, receipt or manifest appears inside the
/// source. Custody (the daemon lock) is established as fixture setup, before
/// the fingerprint window opens.
#[test]
fn backup_keeps_the_source_fingerprint_bit_identical() -> TestResult {
    let source = private_root()?;
    let destination_parent = private_root()?;
    build_live_root(source.path())?;
    let setup = StateRootLease::acquire(source.path())?;
    drop(setup);
    let lock_before = fs::read(source.path().join(".searchd-state-root.lock"))?;

    let before = freeze_current(source.path())?;
    let names_before = tree_names_outside_catalog(source.path())?;
    let catalog_before = live_catalog_digest(source.path())?;
    assert_no_source_markers(source.path())?;

    let destination = destination_parent.path().join("frozen");
    make_backup(source.path(), &destination)?;

    assert_eq!(
        freeze_current(source.path())?,
        before,
        "inode/mtime/content freeze must be identical after backup"
    );
    assert_eq!(
        tree_names_outside_catalog(source.path())?,
        names_before,
        "zero new files may appear inside the source"
    );
    assert_eq!(
        live_catalog_digest(source.path())?,
        catalog_before,
        "the live catalog's logical content must be unchanged"
    );
    assert_eq!(
        fs::read(source.path().join(".searchd-state-root.lock"))?,
        lock_before,
        "the daemon lock bytes must be untouched"
    );
    assert_no_source_markers(source.path())?;
    Ok(())
}

/// The same fingerprint proof across failure and interruption: a scripted
/// write failure and a pre-manifest crash both leave the source identical
/// with no authority files and no published destination.
#[test]
fn backup_failure_and_interruption_keep_the_source_identical() -> TestResult {
    for point in [
        StateMigrationFaultPointV1::AfterDataSync,
        StateMigrationFaultPointV1::BeforeManifestSync,
        StateMigrationFaultPointV1::BeforeCutoverRename,
    ] {
        let source = private_root()?;
        let destination_parent = private_root()?;
        build_live_root(source.path())?;
        let setup = StateRootLease::acquire(source.path())?;
        drop(setup);

        let before = freeze_current(source.path())?;
        let names_before = tree_names_outside_catalog(source.path())?;
        let destination = destination_parent.path().join("frozen");
        let fault = ScriptedFaultV1 { point };
        let _error =
            run_offline_state_command_with_v1(&backup_command(source.path(), &destination), &fault)
                .expect_err("the scripted failure must surface");
        assert!(
            !destination.exists(),
            "no cutover may have happened for {point:?}"
        );
        assert_eq!(
            freeze_current(source.path())?,
            before,
            "the freeze must be identical after a {point:?} failure"
        );
        assert_eq!(
            tree_names_outside_catalog(source.path())?,
            names_before,
            "zero new files may appear after a {point:?} failure"
        );
        assert_no_source_markers(source.path())?;
    }
    Ok(())
}

/// A legacy root's full-tree fingerprint is identical before and after a
/// successful migration — legacy custody has no exclusions — and the source
/// carries no receipt, lock, or manifest afterwards.
#[test]
fn migrate_keeps_the_legacy_source_bit_identical() -> TestResult {
    let legacy = private_root()?;
    let parent = private_root()?;
    build_legacy_root(legacy.path())?;
    let before = freeze_legacy(legacy.path())?;
    assert!(
        !before.entries.is_empty(),
        "the fixture must freeze a non-empty legacy tree"
    );
    let names_before = tree_names_outside_catalog(legacy.path())?;

    let migrated = parent.path().join("migrated");
    let migrate = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Migrate,
        source_root: legacy.path().to_path_buf(),
        destination_root: Some(migrated.clone()),
    };
    let _outcome = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)?;

    assert_eq!(
        freeze_legacy(legacy.path())?,
        before,
        "the legacy tree must be byte-identical after migration"
    );
    assert_eq!(
        tree_names_outside_catalog(legacy.path())?,
        names_before,
        "zero new files may appear inside the legacy source"
    );
    assert_no_source_markers(legacy.path())?;
    assert!(
        !legacy.path().join("semantic").exists()
            || legacy.path().join("semantic/MIGRATED").exists() == false,
        "no source-side migration receipt may exist"
    );
    assert!(
        migrated.join(STATE_MIGRATION_RECEIPT_FILE_NAME).is_file(),
        "the receipt authority lives in the produced root only"
    );
    Ok(())
}

/// An interrupted migration retries by converging staging only: the legacy
/// source stays identical, the destination stays absent until the retry
/// publishes it, and no staging residue survives the cutover.
#[test]
fn interrupted_migration_retry_cleans_staging_only() -> TestResult {
    let legacy = private_root()?;
    let parent = private_root()?;
    build_legacy_root(legacy.path())?;
    let before = freeze_legacy(legacy.path())?;
    let migrated = parent.path().join("migrated");
    let staging = staging_directory_for_v1(&migrated);
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
    assert!(staging.is_dir(), "the interrupted staging is still there");
    assert_eq!(
        freeze_legacy(legacy.path())?,
        before,
        "the legacy source must be identical after the interruption"
    );

    let _outcome = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)?;
    assert!(migrated.join(STATE_ROOT_MANIFEST_FILE_NAME).is_file());
    assert!(
        !staging.exists(),
        "the staging directory is consumed by the cutover"
    );
    assert_eq!(
        freeze_legacy(legacy.path())?,
        before,
        "the legacy source must be identical after the retry"
    );
    assert_no_source_markers(legacy.path())?;
    let _verified = verify_current(&migrated)?;
    Ok(())
}

/// A backup source is read-only for restore: the full-tree freeze is
/// identical before and after, modulo the manifest bytes the test itself
/// does not touch.
#[test]
fn restore_keeps_the_backup_source_bit_identical() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;

    let before = freeze_backup(&backup)?;
    let names_before = tree_names_outside_catalog(&backup)?;
    let restored = restore_parent.path().join("restored");
    let restore = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Restore,
        source_root: backup.clone(),
        destination_root: Some(restored.clone()),
    };
    let _outcome = run_offline_state_command_with_v1(&restore, &NoStateMigrationFaultsV1)?;

    assert_eq!(
        freeze_backup(&backup)?,
        before,
        "the backup source must be byte-identical after restore"
    );
    assert_eq!(
        tree_names_outside_catalog(&backup)?,
        names_before,
        "zero new files may appear inside the backup source"
    );
    assert_no_source_markers(&backup)?;
    let _verified = verify_current(&restored)?;
    drop(before);
    Ok(())
}

/// Verify-state against a current root held by a live lease refuses with
/// the existing `STATE_ROOT_IN_USE`: the composition root's lease handover
/// fails before any session opens.
#[test]
fn verify_state_with_a_live_lease_refuses_state_root_in_use() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;
    // A restored root advertises the state manifest, so verify routes to
    // the lease-bound current session — the path a live daemon blocks.
    let live = restore_parent.path().join("live");
    let restore = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Restore,
        source_root: backup,
        destination_root: Some(live.clone()),
    };
    let _outcome = run_offline_state_command_with_v1(&restore, &NoStateMigrationFaultsV1)?;

    let _daemon = StateRootLease::acquire(&live)?;
    let names_before = tree_names_outside_catalog(&live)?;
    let verify = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Verify,
        source_root: live.clone(),
        destination_root: None,
    };
    let error = run_offline_state_command_with_v1(&verify, &NoStateMigrationFaultsV1)
        .expect_err("a live lease owner must exclude verify-state");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInUse)
    );
    assert_eq!(
        tree_names_outside_catalog(&live)?,
        names_before,
        "the refused verify must not create anything inside the live root"
    );
    Ok(())
}

/// A mid-migration source byte change publishes no destination: the drift
/// gate recomputes the inventory immediately before the cutover and refuses,
/// removing the staging it prepared. Same-length mutation proves the digest
/// (not just the size) is compared.
#[test]
fn mid_migration_source_byte_change_publishes_no_destination() -> TestResult {
    let source = private_root()?;
    let destination_parent = private_root()?;
    build_live_root(source.path())?;
    let setup = StateRootLease::acquire(source.path())?;
    drop(setup);
    let session = current_session(source.path())?;

    let victim = source.path().join("authorities/history.cbor");
    let original = fs::read(&victim)?;
    let mut drifted = original.clone();
    let last = drifted.len().saturating_sub(1);
    if let Some(tail) = drifted.get_mut(last) {
        *tail ^= 0xFF;
    }
    assert_eq!(drifted.len(), original.len());
    assert_ne!(drifted, original);
    fs::write(&victim, &drifted)?;

    let destination = destination_parent.path().join("frozen");
    let staging = staging_directory_for_v1(&destination);
    let error = run_offline_backup_v1(
        &session,
        &destination,
        &StubCatalogV1,
        &StubDeepOpenV1,
        &NoStateMigrationFaultsV1,
    )
    .expect_err("source drift must refuse the publish");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    assert!(!destination.exists(), "zero destination publish on drift");
    assert!(!staging.exists(), "the drift refusal removes its staging");
    fs::write(&victim, &original)?;
    Ok(())
}

/// The same drift gate on the migrate path: a changed legacy byte refuses
/// the publish with no destination and no staging residue.
#[test]
fn mid_migration_legacy_byte_change_publishes_no_destination() -> TestResult {
    let legacy = private_root()?;
    let destination_parent = private_root()?;
    build_legacy_root(legacy.path())?;
    let session = legacy_session(legacy.path())?;

    let victim = legacy
        .path()
        .join("repo-map/activations/repo-a--rev-a.json");
    let original = fs::read(&victim)?;
    let mut drifted = original.clone();
    if let Some(head) = drifted.first_mut() {
        *head ^= 0xFF;
    }
    assert_eq!(drifted.len(), original.len());
    fs::write(&victim, &drifted)?;

    let destination = destination_parent.path().join("migrated");
    let staging = staging_directory_for_v1(&destination);
    let error = run_offline_migrate_v1(
        &session,
        &destination,
        &StubImporterV1,
        &StubCatalogV1,
        &StubDeepOpenV1,
        &NoStateMigrationFaultsV1,
    )
    .expect_err("legacy drift must refuse the publish");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    assert!(!destination.exists(), "zero destination publish on drift");
    assert!(!staging.exists(), "the drift refusal removes its staging");
    fs::write(&victim, &original)?;
    Ok(())
}

/// A corrupt and a truncated legacy journal both fail closed: no
/// destination, and the source tree is byte-identical afterwards.
#[test]
fn corrupt_and_truncated_legacy_journal_fail_closed() -> TestResult {
    for (label, bytes) in [
        ("corrupt", b"not a journal at all".to_vec()),
        ("truncated", b"\x9f\x84ao".to_vec()),
    ] {
        let legacy = private_root()?;
        let parent = private_root()?;
        build_legacy_root(legacy.path())?;
        fs::create_dir_all(legacy.path().join("semantic"))?;
        fs::write(legacy.path().join("semantic/journal.cbor"), &bytes)?;
        let before = freeze_legacy(legacy.path())?;

        let migrated = parent.path().join("migrated");
        let migrate = OfflineStateCommandV1 {
            operation: OfflineStateOperationV1::Migrate,
            source_root: legacy.path().to_path_buf(),
            destination_root: Some(migrated.clone()),
        };
        let error = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)
            .expect_err(&format!("a {label} journal must fail"));
        assert_eq!(
            command_code(&error),
            Some(SearchPlaneErrorCodeV2::LegacySemanticJournalCorrupt),
            "{label} journal must fail with the journal-corrupt authority: {error:?}"
        );
        assert!(
            !migrated.exists(),
            "a {label} journal must publish no destination"
        );
        assert_eq!(
            freeze_legacy(legacy.path())?,
            before,
            "a {label} journal must leave the source identical"
        );
        assert_no_source_markers(legacy.path())?;
    }
    Ok(())
}

/// New binary, old root: a source-side `MIGRATED` receipt is old-binary
/// authority and refuses the journal immutable, with no destination.
/// Old-binary residue of the other kind — a stale `MIGRATED.lock` — is
/// inert and does not block the migration.
#[test]
fn new_binary_refuses_an_old_root_with_a_source_side_receipt() -> TestResult {
    let legacy = private_root()?;
    let parent = private_root()?;
    build_legacy_root(legacy.path())?;
    fs::create_dir_all(legacy.path().join("semantic"))?;
    let journal = quanta_index_ipc::encode_cbor_payload(&(Vec::<
        quanta_index_contract::SemanticIngestBatch,
    >::new(),))?;
    fs::write(legacy.path().join("semantic/journal.cbor"), journal)?;
    fs::write(legacy.path().join("semantic/MIGRATED"), b"migrated")?;

    let migrated = parent.path().join("migrated");
    let migrate = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Migrate,
        source_root: legacy.path().to_path_buf(),
        destination_root: Some(migrated.clone()),
    };
    let error = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)
        .expect_err("an old-root source receipt must be refused");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::LegacySemanticJournalImmutableAfterMigration)
    );
    assert!(!migrated.exists());

    // The lock residue alone is inert: remove the receipt, keep the lock,
    // and the migration proceeds without adopting the lock.
    fs::remove_file(legacy.path().join("semantic/MIGRATED"))?;
    fs::write(legacy.path().join("semantic/MIGRATED.lock"), b"stale")?;
    let _outcome = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)?;
    assert!(migrated.join(STATE_ROOT_MANIFEST_FILE_NAME).is_file());
    Ok(())
}

/// Alias, symlink, hardlink and destination-inside-source refusals: none
/// creates anything inside the source, and every one is typed.
#[test]
fn alias_symlink_and_hardlink_sources_are_refused() -> TestResult {
    // A destination inside the source tree would mutate it: refused before
    // any staging directory is prepared.
    let source = private_root()?;
    let destination_parent = private_root()?;
    build_live_root(source.path())?;
    let setup = StateRootLease::acquire(source.path())?;
    drop(setup);
    let names_before = tree_names_outside_catalog(source.path())?;
    let nested = source.path().join("nested-destination");
    let error = run_offline_state_command_with_v1(
        &backup_command(source.path(), &nested),
        &NoStateMigrationFaultsV1,
    )
    .expect_err("a destination inside the source must be refused");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    assert_eq!(tree_names_outside_catalog(source.path())?, names_before);
    drop(destination_parent);

    // A destination reached through a symlinked parent that aliases back
    // into the source is the same violation under canonicalization.
    #[cfg(unix)]
    {
        let outer = private_root()?;
        let real = outer.path().join("real");
        fs::create_dir_all(&real)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&real, fs::Permissions::from_mode(0o700))?;
        }
        build_live_root(&real)?;
        let setup = StateRootLease::acquire(&real)?;
        drop(setup);
        let link = outer.path().join("link");
        std::os::unix::fs::symlink(&real, &link)?;
        let aliased = link.join("aliased-destination");
        let error = run_offline_state_command_with_v1(
            &backup_command(&real, &aliased),
            &NoStateMigrationFaultsV1,
        )
        .expect_err("an aliased destination must be refused");
        assert_eq!(
            command_code(&error),
            Some(SearchPlaneErrorCodeV2::InvalidRequest)
        );
    }

    // A symlinked source root never becomes custody.
    #[cfg(unix)]
    {
        let target = private_root()?;
        build_legacy_root(target.path())?;
        let alias_parent = private_root()?;
        let alias = alias_parent.path().join("alias");
        std::os::unix::fs::symlink(target.path(), &alias)?;
        let migrate = OfflineStateCommandV1 {
            operation: OfflineStateOperationV1::Migrate,
            source_root: alias,
            destination_root: Some(alias_parent.path().join("migrated")),
        };
        let error = run_offline_state_command_with_v1(&migrate, &NoStateMigrationFaultsV1)
            .expect_err("a symlinked source must be refused");
        assert_eq!(
            command_code(&error),
            Some(SearchPlaneErrorCodeV2::StateRootInsecure)
        );
    }

    // A hard-linked object inside the source is refused: a byte copy would
    // silently un-share its links.
    #[cfg(unix)]
    {
        let hard = private_root()?;
        let hard_parent = private_root()?;
        build_live_root(hard.path())?;
        fs::hard_link(
            hard.path().join("authorities/history.cbor"),
            hard.path().join("authorities/history-alias.cbor"),
        )?;
        let error = run_offline_state_command_with_v1(
            &backup_command(hard.path(), &hard_parent.path().join("frozen")),
            &NoStateMigrationFaultsV1,
        )
        .expect_err("a hard-linked source object must be refused");
        assert_eq!(
            command_code(&error),
            Some(SearchPlaneErrorCodeV2::StateRootInsecure)
        );
        assert!(!hard_parent.path().join("frozen").exists());
    }
    Ok(())
}

/// Wrong-custody sessions never reach the engine: backup needs the daemon
/// lease, migrate needs a legacy session, restore needs a backup session,
/// and verify refuses a legacy session. No destination is published.
#[test]
fn wrong_custody_sessions_never_reach_the_engine() -> TestResult {
    let legacy = private_root()?;
    let current = private_root()?;
    let exile = private_root()?;
    build_legacy_root(legacy.path())?;
    build_live_root(current.path())?;
    let setup = StateRootLease::acquire(current.path())?;
    drop(setup);

    let legacy_held = legacy_session(legacy.path())?;
    let current_held = current_session(current.path())?;

    for (label, error) in [
        (
            "backup with a legacy session",
            run_offline_backup_v1(
                &legacy_held,
                &exile.path().join("backup-out"),
                &StubCatalogV1,
                &StubDeepOpenV1,
                &NoStateMigrationFaultsV1,
            )
            .expect_err("backup requires current custody"),
        ),
        (
            "migrate with a current session",
            run_offline_migrate_v1(
                &current_held,
                &exile.path().join("migrate-out"),
                &StubImporterV1,
                &StubCatalogV1,
                &StubDeepOpenV1,
                &NoStateMigrationFaultsV1,
            )
            .expect_err("migrate requires legacy custody"),
        ),
        (
            "verify with a legacy session",
            run_offline_verify_v1(&legacy_held, &CatalogVerifierV1)
                .expect_err("verify refuses a legacy session"),
        ),
    ] {
        assert_eq!(
            typed_code(&error),
            Some(SearchPlaneErrorCodeV2::InvalidRequest),
            "{label}"
        );
    }
    assert!(exile.path().read_dir()?.next().is_none());
    Ok(())
}
