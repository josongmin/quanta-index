//! Offline `backup-state` / `restore-state` / `verify-state`
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
use std::process::Command;
use std::time::Duration;

use quanta_index_catalog::{SqliteCatalog, live_catalog_receipt, verify_snapshot};
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::state_format::{
    LEGACY_AUXILIARY_SNAPSHOT_RELATIVES, NoStateMigrationFaultsV1, OfflineRootRoleV1,
    STATE_ROOT_MANIFEST_FILE_NAME, StateMigrationFaultPointV1, StateMigrationFaultPort,
    StateRootFormatV1, StateRootManifestV1, atomic_cutover_v1, detect_state_root_format_v1,
    inventory_state_root_v1, read_root_manifest_v1, refuse_broad_offline_target_v1,
    refuse_legacy_state_root_v1, refuse_non_empty_destination_v1, staging_directory_for_v1,
    write_root_manifest_last_v1,
};
use quanta_index_searchd::app::state_migration::{
    CatalogFreezeV1, CatalogSnapshotPort, CatalogSnapshotV1, OfflineSourceSessionV1,
    OfflineStateCommandV1, OfflineStateOperationV1, OfflineStateVerificationV1,
    SourceFreezeReceiptV1, StateRootDeepOpenPort, StateRootDeepOpenReceiptV1,
    run_offline_backup_v1, run_offline_restore_v1, run_offline_verify_v1,
};
use quanta_index_searchd_harness::E2eRuntime;
use quanta_index_searchd_runtime::state_migration::{
    render_offline_outcome_v1, run_offline_state_command_with_v1,
};
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BUSY: Duration = Duration::from_secs(2);
const INCARNATION: &str = "activations/.activation-root-incarnation-v1";

fn signed_manifest_fixture(body: &str) -> String {
    format!("{body}root-digest {:x}\n", Sha256::digest(body.as_bytes()))
}

fn canonical_manifest_fixture() -> String {
    format!(
        "quanta-index-state-root-manifest\nformat-version 1\nroot-format current-v1\ncatalog-digest {}\ncatalog-rows 0\n",
        "0".repeat(64)
    )
}

#[test]
fn audit_manifest_decode_refuses_noncanonical_authority() -> TestResult {
    let body = canonical_manifest_fixture();
    let canonical = signed_manifest_fixture(&body);
    let parsed = StateRootManifestV1::decode(&canonical)?;
    assert_eq!(
        parsed.encode(),
        canonical,
        "the independent golden round-trips exactly"
    );

    let mut counterexamples = Vec::new();
    for line in [
        "format-version 1",
        "root-format current-v1",
        "catalog-rows 0",
    ] {
        counterexamples.push(signed_manifest_fixture(
            &body.replace(&format!("{line}\n"), &format!("{line}\n{line}\n")),
        ));
    }
    let digest_line = format!("catalog-digest {}\n", "0".repeat(64));
    counterexamples.push(signed_manifest_fixture(
        &body.replace(&digest_line, &digest_line.repeat(2)),
    ));
    counterexamples.push(signed_manifest_fixture(&body.replace(
        "format-version 1\nroot-format current-v1\n",
        "root-format current-v1\nformat-version 1\n",
    )));
    counterexamples.push(signed_manifest_fixture(
        &body.replace("catalog-rows 0", "catalog-rows 00"),
    ));
    counterexamples.push(canonical.trim_end_matches('\n').to_string());
    // Existing decoding normalizes CRLF before digest comparison. A signed
    // LF body with CRLF transport must not acquire the same byte authority.
    counterexamples.push(canonical.replace('\n', "\r\n"));
    for bytes in counterexamples {
        let _error = StateRootManifestV1::decode(&bytes)
            .expect_err("noncanonical or duplicate manifest authority must be refused");
    }
    Ok(())
}

#[test]
fn audit_manifest_decode_refuses_invalid_digests_and_ambiguous_paths() -> TestResult {
    let body = canonical_manifest_fixture();
    let _valid = StateRootManifestV1::decode(&signed_manifest_fixture(&body))?;
    for digest in [
        String::new(),
        "g".repeat(64),
        "A".repeat(64),
        "0".repeat(63),
    ] {
        let forged = body.replace(&"0".repeat(64), &digest);
        let _error = StateRootManifestV1::decode(&signed_manifest_fixture(&forged))
            .expect_err("a self-signed manifest still requires a canonical SHA-256 catalog digest");
    }
    for payload in [
        format!(
            "object {} 1 item\nobject {} 2 item\n",
            "0".repeat(64),
            "1".repeat(64)
        ),
        format!("object {} 1 item\ndir item\n", "0".repeat(64)),
        "object malformed 1 item\n".to_string(),
        format!("object {} 1 item\tname\n", "0".repeat(64)),
    ] {
        let _error =
            StateRootManifestV1::decode(&signed_manifest_fixture(&format!("{body}{payload}")))
                .expect_err("an object path or digest cannot have ambiguous authority");
    }
    Ok(())
}

#[test]
fn audit_manifest_write_refuses_invalid_authority_before_creation() -> TestResult {
    let parent = private_root()?;
    let root = parent.path().join("staging");
    fs::create_dir(&root)?;
    let manifest =
        StateRootManifestV1::decode(&signed_manifest_fixture(&canonical_manifest_fixture()))?;
    let mut wrong_version = manifest.clone();
    wrong_version.format_version = 2;
    let mut wrong_digest = manifest.clone();
    wrong_digest.catalog_digest_hex = "invalid".to_string();
    let mut wrong_directory = manifest.clone();
    wrong_directory
        .directories
        .push("injected\ndir authority".to_string());
    for invalid in [wrong_version, wrong_digest, wrong_directory] {
        let _error = write_root_manifest_last_v1(
            &root,
            STATE_ROOT_MANIFEST_FILE_NAME,
            &invalid,
            &NoStateMigrationFaultsV1,
        )
        .expect_err("an unreadable manifest cannot be published");
        assert!(!root.join(STATE_ROOT_MANIFEST_FILE_NAME).exists());
    }
    let outside = parent.path().join("escaped-manifest");
    for name in [
        "../escaped-manifest".to_string(),
        outside.display().to_string(),
        "other.txt".to_string(),
    ] {
        let error = write_root_manifest_last_v1(&root, &name, &manifest, &NoStateMigrationFaultsV1)
            .expect_err("manifest publication must use an owned top-level manifest name");
        assert_eq!(
            typed_code(&error),
            Some(SearchPlaneErrorCodeV2::InvalidRequest)
        );
    }
    assert!(!outside.exists());
    assert_eq!(
        fs::read_dir(&root)?.count(),
        0,
        "refusal must leave no files"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn audit_inventory_and_custody_refuse_noncanonical_names() -> TestResult {
    for name in ["bad\\separator", "bad\nline"] {
        let root = private_root()?;
        build_live_root(root.path())?;
        fs::write(
            root.path().join("authorities").join(name),
            b"cannot be relabeled",
        )?;
        let error = inventory_state_root_v1(root.path(), &[])
            .expect_err("object inventory cannot normalize an unrepresentable path");
        assert_eq!(
            typed_code(&error),
            Some(SearchPlaneErrorCodeV2::StateRootInsecure)
        );
        let error = quanta_index_searchd::app::state_format::inventory_state_directories_v1(
            root.path(),
            &[],
        )
        .expect_err("directory inventory must use the same path admission");
        assert_eq!(
            typed_code(&error),
            Some(SearchPlaneErrorCodeV2::StateRootInsecure)
        );
        let lease = StateRootLease::acquire(root.path())?;
        let error = OfflineSourceSessionV1::open_current(lease)
            .expect_err("source custody cannot normalize a filesystem identity");
        assert_eq!(
            typed_code(&error),
            Some(SearchPlaneErrorCodeV2::StateRootInsecure)
        );
    }
    Ok(())
}

#[test]
fn audit_verify_current_refuses_catalog_changed_after_catalog_read() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    make_backup(source.path(), &backup)?;
    let restored = parent.path().join("restored");
    let restore = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Restore,
        source_root: backup,
        destination_root: Some(restored.clone()),
    };
    let _outcome = run_offline_state_command_with_v1(&restore, &NoStateMigrationFaultsV1)?;
    let session = current_session(&restored)?;
    let path = restored.join("catalog/catalog-v1.sqlite");
    let mut bytes = fs::read(&path)?;
    bytes.push(0xFF);
    let error = run_offline_verify_v1(&session, &PostVerifyMutationV1 { path, bytes })
        .expect_err("a current-root catalog change after its read must not verify successfully");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    Ok(())
}

#[test]
fn audit_verify_current_refuses_root_replaced_after_custody() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    make_backup(source.path(), &backup)?;
    let restored = parent.path().join("restored");
    let restore = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Restore,
        source_root: backup,
        destination_root: Some(restored.clone()),
    };
    let _first = run_offline_state_command_with_v1(&restore, &NoStateMigrationFaultsV1)?;
    let session = current_session(&restored)?;
    fs::rename(&restored, parent.path().join("old-root-with-held-lease"))?;
    let _replacement = run_offline_state_command_with_v1(&restore, &NoStateMigrationFaultsV1)?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
        .expect_err("a lease on the replaced inode is not custody of the new root");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    Ok(())
}

#[test]
fn audit_verify_current_does_not_reset_non_catalog_custody() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    make_backup(source.path(), &backup)?;
    let restored = parent.path().join("restored");
    let _outcome = run_offline_state_command_with_v1(
        &OfflineStateCommandV1 {
            operation: OfflineStateOperationV1::Restore,
            source_root: backup,
            destination_root: Some(restored.clone()),
        },
        &NoStateMigrationFaultsV1,
    )?;
    let session = current_session(&restored)?;
    let relative = "authorities/history.cbor";
    let bytes = b"changed after the pinned custody";
    fs::write(restored.join(relative), bytes)?;
    // Self-sign the changed payload: manifest/object consistency alone is
    // not evidence that the original session's identities are unchanged.
    let manifest_path = restored.join(STATE_ROOT_MANIFEST_FILE_NAME);
    let mut manifest = read_root_manifest_v1(&manifest_path)?;
    let object = manifest
        .objects
        .iter_mut()
        .find(|entry| entry.relative_path == relative)
        .ok_or("the authority fixture must be advertised")?;
    object.byte_size = u64::try_from(bytes.len())?;
    object.digest_hex = format!("{:x}", Sha256::digest(bytes));
    fs::write(&manifest_path, manifest.encode())?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
        .expect_err("a new verification freeze cannot reset existing source custody");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    Ok(())
}

#[test]
fn audit_backup_preserves_non_catalog_sidecar_names_and_subtrees() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let paths = [
        "authorities/ordinary.sqlite-wal",
        "authorities/ordinary.sqlite-shm",
        "authorities/ordinary.sqlite-journal",
        "authorities/subtree.sqlite-wal/payload",
    ];
    fs::create_dir_all(source.path().join("authorities/subtree.sqlite-wal"))?;
    for path in paths {
        fs::write(source.path().join(path), path.as_bytes())?;
    }
    let backup = parent.path().join("backup");
    make_backup(source.path(), &backup)?;
    let manifest = read_root_manifest_v1(&backup.join("state-backup-manifest-v1.txt"))?;
    for path in paths {
        assert!(
            manifest
                .objects
                .iter()
                .any(|entry| entry.relative_path == path),
            "a suffix is not authority to omit {path}"
        );
        assert_eq!(fs::read(backup.join(path))?, path.as_bytes());
    }
    let _verified = verify_backup(&backup)?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn audit_backup_custody_refuses_linked_owned_sidecars() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    make_backup(source.path(), &backup)?;
    let alias = parent.path().join("aliased-bytes");
    fs::write(&alias, b"must not be adopted as SQLite bookkeeping")?;
    let sidecar = backup.join("catalog/catalog-v1.sqlite-shm");
    // The deep-open fixture may already have created this disposable vendor
    // file. Replace only that exact fixture path before injecting the link.
    if sidecar.try_exists()? {
        fs::remove_file(&sidecar)?;
    }
    fs::hard_link(&alias, &sidecar)?;
    let error = OfflineSourceSessionV1::open_produced_backup(&backup)
        .expect_err("a vendor filename cannot grant custody of a hard-linked file");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    fs::remove_file(&sidecar)?;
    std::os::unix::fs::symlink(&alias, &sidecar)?;
    let error = OfflineSourceSessionV1::open_produced_backup(&backup)
        .expect_err("a vendor filename cannot grant custody of a symlink");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    assert_eq!(
        fs::read(&alias)?,
        b"must not be adopted as SQLite bookkeeping"
    );
    Ok(())
}

#[test]
fn migrate_state_cli_is_rejected_before_opening_a_root() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    let destination = parent.path().join("destination");
    let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
        .arg("migrate-state")
        .arg("--source")
        .arg(source.path())
        .arg("--destination")
        .arg(&destination)
        .output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown argument: migrate-state"));
    assert!(!destination.exists());
    Ok(())
}

#[test]
fn legacy_root_format_manifest_is_rejected() -> TestResult {
    let manifest = StateRootManifestV1 {
        format_version: 1,
        root_format: StateRootFormatV1::LegacyV1,
        catalog_digest_hex: "0".repeat(64),
        catalog_rows: 0,
        objects: Vec::new(),
        directories: Vec::new(),
    };
    let error = StateRootManifestV1::decode(&manifest.encode())
        .expect_err("a legacy manifest cannot become verification authority");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported)
    );
    let root = private_root()?;
    let error = write_root_manifest_last_v1(
        root.path(),
        STATE_ROOT_MANIFEST_FILE_NAME,
        &manifest,
        &NoStateMigrationFaultsV1,
    )
    .expect_err("an old-format manifest must not be written");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported)
    );
    assert!(!root.path().join(STATE_ROOT_MANIFEST_FILE_NAME).exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn manifest_authority_refuses_links_and_dangling_write_target() -> TestResult {
    use std::os::unix::fs::symlink;

    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    make_backup(source.path(), &backup)?;
    let manifest_path = backup.join("state-backup-manifest-v1.txt");
    let manifest = read_root_manifest_v1(&manifest_path)?;

    let symlink_path = parent.path().join("manifest-link");
    symlink(&manifest_path, &symlink_path)?;
    let error = read_root_manifest_v1(&symlink_path)
        .expect_err("a symlink cannot serve as a root manifest");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );

    let hardlink_path = parent.path().join("manifest-hardlink");
    fs::hard_link(&manifest_path, &hardlink_path)?;
    let error = read_root_manifest_v1(&hardlink_path)
        .expect_err("a hard-linked file cannot serve as a root manifest");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    fs::remove_file(&hardlink_path)?;

    let fifo = parent.path().join("manifest-fifo");
    let fifo_status = Command::new("mkfifo").arg(&fifo).status()?;
    assert!(fifo_status.success(), "the FIFO fixture must be created");
    let error = read_root_manifest_v1(&fifo)
        .expect_err("a FIFO cannot serve as a manifest or block the offline command");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );

    let staging = private_root()?;
    let outside = parent.path().join("outside-manifest");
    let dangling = staging.path().join(STATE_ROOT_MANIFEST_FILE_NAME);
    symlink(&outside, &dangling)?;
    let error = write_root_manifest_last_v1(
        staging.path(),
        STATE_ROOT_MANIFEST_FILE_NAME,
        &manifest,
        &NoStateMigrationFaultsV1,
    )
    .expect_err("a dangling manifest symlink cannot redirect a write outside staging");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    assert!(!outside.exists(), "no write may escape the staging root");
    Ok(())
}

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

/// Change a produced root at the last verifier boundary, after the object
/// walk and the catalog read have succeeded.
struct PostVerifyMutationV1 {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl CatalogSnapshotPort for PostVerifyMutationV1 {
    fn snapshot_into(
        &self,
        live_root: &Path,
        destination_file: &Path,
    ) -> Result<CatalogFreezeV1, CoreError> {
        CatalogVerifierV1.snapshot_into(live_root, destination_file)
    }

    fn verify_snapshot_at(&self, snapshot_file: &Path) -> Result<CatalogSnapshotV1, CoreError> {
        let receipt = CatalogVerifierV1.verify_snapshot_at(snapshot_file)?;
        fs::write(&self.path, &self.bytes).map_err(|error| {
            CoreError::Storage(format!(
                "mutate produced root {} after catalog verification: {error}",
                self.path.display()
            ))
        })?;
        Ok(receipt)
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

/// Custody for a produced backup root: read-only, creating nothing.
fn backup_session(root: &Path) -> Result<OfflineSourceSessionV1, Box<dyn std::error::Error>> {
    Ok(OfflineSourceSessionV1::open_produced_backup(root)?)
}

/// The frozen inventory of a current root, as the drift gate compares it.
fn freeze_current(root: &Path) -> Result<SourceFreezeReceiptV1, Box<dyn std::error::Error>> {
    Ok(current_session(root)?.before().clone())
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

/// Return sorted names beneath `root`, excluding the catalog subtree.
///
/// The engine's backup API opens the live catalog, whose directory mtimes
/// are vendor bookkeeping rather than source state.
///
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

/// Assert migration control files never appear beneath a source root.
///
/// Migration markers, receipts, and produced-current manifests belong in
/// staging or destination only. Track-local files such as
/// `indexes/.../manifest.cbor` are ordinary source payload and must not be
/// rejected by a substring match.
///
/// Lock files are covered separately: the daemon's own pre-existing lock may stand, but
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
                    && name != "state-migration-receipt-v1.txt",
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
        format!("{error}").contains("does not support legacy state roots"),
        "the refusal must state the unsupported format: {error}"
    );
    Ok(())
}

#[test]
fn backup_refuses_legacy_source_without_publishing() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_legacy_root(source.path())?;
    let before = inventory_state_root_v1(source.path(), &[])?;
    let destination = parent.path().join("backup");
    let error = run_offline_state_command_with_v1(
        &backup_command(source.path(), &destination),
        &NoStateMigrationFaultsV1,
    )
    .expect_err("backup requires a current root");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported)
    );
    assert!(!destination.exists());
    assert_eq!(inventory_state_root_v1(source.path(), &[])?, before);
    Ok(())
}

#[test]
fn old_migration_receipts_and_dangling_journals_refuse_mixed_roots() -> TestResult {
    let root = private_root()?;
    fs::create_dir_all(root.path().join("catalog"))?;
    fs::write(root.path().join("state-migration-receipt-v1.txt"), b"old")?;
    assert_eq!(
        detect_state_root_format_v1(root.path())?,
        StateRootFormatV1::LegacyV1
    );
    assert_eq!(
        typed_code(&refuse_legacy_state_root_v1(root.path()).expect_err("old receipt must refuse")),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported)
    );
    fs::remove_file(root.path().join("state-migration-receipt-v1.txt"))?;
    #[cfg(unix)]
    {
        fs::create_dir_all(root.path().join("semantic"))?;
        std::os::unix::fs::symlink("absent", root.path().join("semantic/journal.cbor"))?;
        assert_eq!(
            detect_state_root_format_v1(root.path())?,
            StateRootFormatV1::LegacyV1
        );
    }
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
    let before = inventory_state_root_v1(root.path(), &[])?;

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
    assert_eq!(inventory_state_root_v1(root.path(), &[])?, before);
    Ok(())
}

#[test]
fn production_boot_refuses_auxiliary_snapshot_before_creating_catalog_or_lease() -> TestResult {
    let root = private_root()?;
    let snapshot = root.path().join(LEGACY_AUXILIARY_SNAPSHOT_RELATIVES[0]);
    fs::create_dir_all(snapshot.parent().ok_or("snapshot has no parent")?)?;
    fs::write(
        &snapshot,
        quanta_index_ipc::encode_cbor_payload(&serde_json::json!({"entries": {}}))?,
    )?;
    let before = inventory_state_root_v1(root.path(), &[])?;
    let mut runtime = E2eRuntime::boot_in(root.path())?;
    let error = runtime
        .start()
        .expect_err("boot must refuse pre-catalog snapshots");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported)
    );
    assert_eq!(inventory_state_root_v1(root.path(), &[])?, before);
    assert!(!root.path().join("catalog").exists());
    assert!(!root.path().join(".searchd-state-root.lock").exists());
    Ok(())
}

// ---------------------------------------------------------------------------
// Backup and restore: one freeze boundary, verified
// ---------------------------------------------------------------------------

#[test]
fn restore_refuses_backup_inventory_changed_before_custody() -> TestResult {
    for mutation in [
        "added-file",
        "changed-file",
        "missing-file",
        "added-directory",
        "missing-directory",
    ] {
        let source = private_root()?;
        let backup_parent = private_root()?;
        let restore_parent = private_root()?;
        build_live_root(source.path())?;
        fs::write(
            source.path().join("retained-note"),
            b"original backup bytes",
        )?;
        fs::create_dir(source.path().join("retained-empty"))?;
        let backup = backup_parent.path().join("backup-root");
        make_backup(source.path(), &backup)?;
        match mutation {
            "added-file" => fs::write(backup.join("unadvertised-note"), b"extra")?,
            "changed-file" => fs::write(backup.join("retained-note"), b"changed backup bytes")?,
            "missing-file" => fs::remove_file(backup.join("retained-note"))?,
            "added-directory" => fs::create_dir(backup.join("unadvertised-empty"))?,
            "missing-directory" => fs::remove_dir(backup.join("retained-empty"))?,
            _ => return Err("invalid closed mutation fixture".into()),
        }
        let restored = restore_parent.path().join("restored-root");
        let command = OfflineStateCommandV1 {
            operation: OfflineStateOperationV1::Restore,
            source_root: backup,
            destination_root: Some(restored.clone()),
        };
        let error = run_offline_state_command_with_v1(&command, &NoStateMigrationFaultsV1)
            .expect_err(
                "restore must refuse inventory that disagrees with the original backup manifest",
            );
        assert!(
            matches!(
                command_code(&error),
                Some(
                    SearchPlaneErrorCodeV2::NotFound
                        | SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch
                )
            ),
            "{mutation}: {error}"
        );
        assert!(
            !restored.exists(),
            "{mutation}: refusal must not publish a destination"
        );
        assert!(
            !staging_directory_for_v1(&restored).exists(),
            "{mutation}: source admission must precede staging creation"
        );
    }
    Ok(())
}

#[test]
fn restore_refuses_self_signed_wrong_catalog_rows_before_staging() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;
    let path = backup.join("state-backup-manifest-v1.txt");
    let mut manifest = read_root_manifest_v1(&path)?;
    manifest.catalog_rows += 1;
    fs::write(&path, manifest.encode())?;
    let restored = restore_parent.path().join("restored");
    let command = OfflineStateCommandV1 {
        operation: OfflineStateOperationV1::Restore,
        source_root: backup,
        destination_root: Some(restored.clone()),
    };
    let error = run_offline_state_command_with_v1(&command, &NoStateMigrationFaultsV1)
        .expect_err("a valid self-digest cannot replace the independent catalog row count");
    assert_eq!(
        command_code(&error),
        Some(SearchPlaneErrorCodeV2::CatalogRowCorrupt)
    );
    assert!(!restored.exists());
    assert!(!staging_directory_for_v1(&restored).exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn ambiguous_dangling_or_directory_authority_refuses_verify_and_restore() -> TestResult {
    for mutation in ["dangling", "directory"] {
        let source = private_root()?;
        let backup_parent = private_root()?;
        let restore_parent = private_root()?;
        build_live_root(source.path())?;
        let backup = backup_parent.path().join("backup-root");
        make_backup(source.path(), &backup)?;
        let alternate = backup.join(STATE_ROOT_MANIFEST_FILE_NAME);
        match mutation {
            "dangling" => std::os::unix::fs::symlink("absent-authority", &alternate)?,
            "directory" => fs::create_dir(&alternate)?,
            _ => return Err("invalid closed mutation fixture".into()),
        }
        for operation in [
            OfflineStateOperationV1::Verify,
            OfflineStateOperationV1::Restore,
        ] {
            let destination = restore_parent.path().join("restored");
            let command = OfflineStateCommandV1 {
                operation,
                source_root: backup.clone(),
                destination_root: (operation == OfflineStateOperationV1::Restore)
                    .then_some(destination.clone()),
            };
            let error = run_offline_state_command_with_v1(&command, &NoStateMigrationFaultsV1)
                .expect_err("a second reserved authority entry cannot be interpreted as absent");
            assert_eq!(
                command_code(&error),
                Some(SearchPlaneErrorCodeV2::InvalidRequest)
            );
            assert!(!destination.exists());
            assert!(!staging_directory_for_v1(&destination).exists());
        }
    }
    Ok(())
}

#[test]
fn backup_manifest_replacement_after_custody_refuses_verify_and_restore() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;
    let session = backup_session(&backup)?;
    let path = backup.join("state-backup-manifest-v1.txt");
    let mut manifest = read_root_manifest_v1(&path)?;
    manifest.catalog_rows += 1;
    fs::write(&path, manifest.encode())?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
        .expect_err("verification must consume the authority pinned at session open");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    let restored = restore_parent.path().join("restored");
    let error = run_offline_restore_v1(
        &session,
        &restored,
        &CatalogVerifierV1,
        &UnreachedDeepOpenV1,
        &NoStateMigrationFaultsV1,
    )
    .expect_err("restore cannot re-admit a replacement authority");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    assert!(!restored.exists());
    assert!(!staging_directory_for_v1(&restored).exists());
    Ok(())
}

struct ChangeBackupAuthorityV1 {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}

struct CatalogReadSidecarV1;

impl CatalogSnapshotPort for CatalogReadSidecarV1 {
    fn snapshot_into(
        &self,
        live_root: &Path,
        destination_file: &Path,
    ) -> Result<CatalogFreezeV1, CoreError> {
        CatalogVerifierV1.snapshot_into(live_root, destination_file)
    }

    fn verify_snapshot_at(&self, snapshot_file: &Path) -> Result<CatalogSnapshotV1, CoreError> {
        let receipt = CatalogVerifierV1.verify_snapshot_at(snapshot_file)?;
        // A read-only WAL inspection may create/remove this disposable file.
        // Force the catalog-directory metadata change without altering payload.
        let sidecar = snapshot_file.with_extension("sqlite-shm");
        fs::write(&sidecar, b"disposable read-only sidecar")
            .map_err(|error| CoreError::Storage(error.to_string()))?;
        fs::remove_file(&sidecar).map_err(|error| CoreError::Storage(error.to_string()))?;
        Ok(receipt)
    }
}

#[test]
fn restore_admits_catalog_directory_change_from_read_only_sidecars() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    make_backup(source.path(), &backup)?;
    let session = backup_session(&backup)?;
    let restored = restore_parent.path().join("restored");
    let _outcome = run_offline_restore_v1(
        &session,
        &restored,
        &CatalogReadSidecarV1,
        &StubDeepOpenV1,
        &NoStateMigrationFaultsV1,
    )?;
    assert!(restored.exists());
    let _verified = verify_current(&restored)?;
    Ok(())
}

impl StateMigrationFaultPort for ChangeBackupAuthorityV1 {
    fn reach(&self, point: StateMigrationFaultPointV1) -> Result<(), CoreError> {
        if point == StateMigrationFaultPointV1::AfterDataSync {
            let result = match &self.bytes {
                Some(bytes) => fs::write(&self.path, bytes),
                None => fs::remove_file(&self.path),
            };
            result.map_err(|error| CoreError::Storage(error.to_string()))?;
        }
        Ok(())
    }
}

#[test]
fn mid_restore_backup_authority_change_discards_sealed_staging() -> TestResult {
    for mutation in ["replace", "malformed", "remove"] {
        let source = private_root()?;
        let backup_parent = private_root()?;
        let restore_parent = private_root()?;
        build_live_root(source.path())?;
        let backup = backup_parent.path().join("backup-root");
        make_backup(source.path(), &backup)?;
        let path = backup.join("state-backup-manifest-v1.txt");
        let mut manifest = read_root_manifest_v1(&path)?;
        manifest.catalog_rows += 1;
        let bytes = match mutation {
            "replace" => Some(manifest.encode().into_bytes()),
            "malformed" => Some(b"malformed authority".to_vec()),
            "remove" => None,
            _ => return Err("invalid closed mutation fixture".into()),
        };
        let restored = restore_parent.path().join("restored");
        let command = OfflineStateCommandV1 {
            operation: OfflineStateOperationV1::Restore,
            source_root: backup,
            destination_root: Some(restored.clone()),
        };
        let error =
            run_offline_state_command_with_v1(&command, &ChangeBackupAuthorityV1 { path, bytes })
                .expect_err("authority loss after copy cannot publish a restored root");
        assert!(command_code(&error).is_some(), "{mutation}: {error}");
        assert!(!restored.exists(), "{mutation}");
        assert!(!staging_directory_for_v1(&restored).exists(), "{mutation}");
    }
    Ok(())
}

#[test]
fn backup_then_restore_reseals_manifest_with_activation_incarnation() -> TestResult {
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
    assert!(
        backup_manifest
            .objects
            .iter()
            .all(|entry| entry.relative_path != INCARNATION),
        "the disposable source has not yet opened an activation catalog"
    );
    let restored_incarnation = restored_manifest
        .objects
        .iter()
        .find(|entry| entry.relative_path == INCARNATION)
        .expect("controlled restore must create a fresh activation incarnation");
    assert_eq!(restored_incarnation.byte_size, 16);
    let restored_other = restored_manifest
        .objects
        .iter()
        .filter(|entry| entry.relative_path != INCARNATION)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        restored_other, backup_manifest.objects,
        "controlled restore may add only its activation incarnation"
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
fn restore_rotates_existing_activation_incarnation_and_reseals_manifest() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let original = [0x42_u8; 16];
    fs::write(source.path().join(INCARNATION), original)?;
    let backup = backup_parent.path().join("backup-root");
    let restored = restore_parent.path().join("restored-root");
    drop(run_offline_state_command_with_v1(
        &backup_command(source.path(), &backup),
        &NoStateMigrationFaultsV1,
    )?);
    assert_eq!(fs::read(backup.join(INCARNATION))?, original);

    drop(run_offline_state_command_with_v1(
        &OfflineStateCommandV1 {
            operation: OfflineStateOperationV1::Restore,
            source_root: backup.clone(),
            destination_root: Some(restored.clone()),
        },
        &NoStateMigrationFaultsV1,
    )?);
    let rotated = fs::read(restored.join(INCARNATION))?;
    assert_eq!(rotated.len(), original.len());
    assert_ne!(rotated, original);
    let backup_manifest = read_root_manifest_v1(&backup.join("state-backup-manifest-v1.txt"))?;
    let restored_manifest = read_root_manifest_v1(&restored.join(STATE_ROOT_MANIFEST_FILE_NAME))?;
    let backup_other = backup_manifest
        .objects
        .iter()
        .filter(|entry| entry.relative_path != INCARNATION)
        .collect::<Vec<_>>();
    let restored_other = restored_manifest
        .objects
        .iter()
        .filter(|entry| entry.relative_path != INCARNATION)
        .collect::<Vec<_>>();
    assert_eq!(restored_other, backup_other);
    assert_ne!(
        restored_manifest
            .objects
            .iter()
            .find(|entry| entry.relative_path == INCARNATION)
            .expect("restored incarnation manifest entry")
            .digest_hex,
        backup_manifest
            .objects
            .iter()
            .find(|entry| entry.relative_path == INCARNATION)
            .expect("backup incarnation manifest entry")
            .digest_hex
    );
    let _verified = verify_current(&restored)?;
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
fn verify_state_refuses_object_changed_after_catalog_verification() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    make_backup(source.path(), &backup)?;
    let session = backup_session(&backup)?;
    let victim = backup.join("authorities/history.cbor");
    let mut changed = fs::read(&victim)?;
    let last = changed
        .len()
        .checked_sub(1)
        .ok_or("authority fixture is empty")?;
    *changed
        .get_mut(last)
        .ok_or("authority fixture byte is missing")? ^= 0xFF;

    let error = run_offline_verify_v1(
        &session,
        &PostVerifyMutationV1 {
            path: victim,
            bytes: changed,
        },
    )
    .expect_err("a post-walk source mutation must not verify successfully");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    Ok(())
}

#[test]
fn verify_state_refuses_manifest_changed_after_catalog_verification() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    make_backup(source.path(), &backup)?;
    let session = backup_session(&backup)?;
    let manifest_path = backup.join("state-backup-manifest-v1.txt");
    let mut changed = read_root_manifest_v1(&manifest_path)?;
    changed.catalog_rows = changed
        .catalog_rows
        .checked_add(1)
        .ok_or("row count overflow")?;

    let error = run_offline_verify_v1(
        &session,
        &PostVerifyMutationV1 {
            path: manifest_path,
            bytes: changed.encode().into_bytes(),
        },
    )
    .expect_err("a post-walk manifest replacement must not verify successfully");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::StateRootInsecure)
    );
    Ok(())
}

#[test]
fn verify_state_refuses_second_manifest_added_after_catalog_verification() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    make_backup(source.path(), &backup)?;
    let session = backup_session(&backup)?;
    let backup_manifest = backup.join("state-backup-manifest-v1.txt");
    let bytes = fs::read(&backup_manifest)?;

    let error = run_offline_verify_v1(
        &session,
        &PostVerifyMutationV1 {
            path: backup.join(STATE_ROOT_MANIFEST_FILE_NAME),
            bytes,
        },
    )
    .expect_err("a second manifest added after the object walk must not verify successfully");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
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
#[test]
fn dangling_legacy_auxiliary_snapshot_is_not_classified_as_current() -> TestResult {
    let root = private_root()?;
    let snapshot = root.path().join(LEGACY_AUXILIARY_SNAPSHOT_RELATIVES[0]);
    fs::create_dir_all(snapshot.parent().ok_or("snapshot has no parent")?)?;
    std::os::unix::fs::symlink("missing-snapshot", &snapshot)?;
    assert_eq!(
        detect_state_root_format_v1(root.path())?,
        StateRootFormatV1::LegacyV1
    );
    assert_eq!(
        typed_code(&refuse_legacy_state_root_v1(root.path()).expect_err("boot must refuse")),
        Some(SearchPlaneErrorCodeV2::StateRootFormatUnsupported)
    );
    Ok(())
}

#[test]
fn the_engine_refuses_a_self_target_before_any_lease() -> TestResult {
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
        content_digest_hex: "0".repeat(64),
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

/// A current root's byte fingerprint is identical before and after a
/// successful backup, and no marker, receipt or manifest appears inside the source.
///
/// Custody (the daemon lock) is established as fixture setup, before
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

/// A mid-backup source byte change publishes no destination: the drift
/// gate recomputes the inventory immediately before the cutover and refuses.
///
/// It removes the staging it prepared. Same-length mutation proves the digest
/// (not just the size) is compared.
#[test]
fn mid_backup_source_byte_change_publishes_no_destination() -> TestResult {
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
