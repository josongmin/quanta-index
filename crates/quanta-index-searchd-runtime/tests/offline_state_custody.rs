//! Offline backup and restore custody through the public Legacy command and
//! the engine-independent verification and admission ports.

#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "offline regressions assert custody and filesystem invariants"
)]

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use crate::legacy_offline_refusal::run_cli_success;
use crate::legacy_offline_refusal::{
    configure_offline_command, offline_command, require_cli_refusal, require_cli_refusal_output,
};
use crate::legacy_start_probe::{wait_for_command_exit, wait_for_refusal};
use crate::offline_live_root::{BUSY, build_live_root};
use crate::offline_tree_inventory::{
    assert_no_source_markers, freeze_current, tree_names_outside_catalog,
};
use quanta_index_catalog::{SqliteCatalog, live_catalog_receipt, verify_snapshot};
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::state_format::{
    NoStateMigrationFaultsV1, STATE_BACKUP_MANIFEST_FILE_NAME, STATE_ROOT_LEASE_FILE_NAME,
    STATE_ROOT_MANIFEST_FILE_NAME, StateMigrationFaultPointV1, read_root_manifest_v1,
    staging_directory_for_v1,
};
use quanta_index_searchd::app::state_format::{
    STATE_MIGRATION_CRASH_ENV, STATE_MIGRATION_CRASH_EXIT_CODE,
};
use quanta_index_searchd::app::state_migration::{
    OfflineSourceSessionV1, OfflineStateOperationV1, OfflineStateVerificationV1,
    SourceFreezeReceiptV1, run_offline_backup_v1, run_offline_restore_v1, run_offline_verify_v1,
};
use quanta_index_searchd_harness::legacy_reference::legacy_physical_env;
use quanta_index_searchd_harness::{
    legacy_reference::LegacySearchdBinary, private_tempdir as private_root,
};

#[path = "common/offline_catalog_verify.rs"]
mod offline_catalog_verify;
use offline_catalog_verify::CatalogVerifierV1;

#[path = "common/offline_cancelled_command.rs"]
mod offline_cancelled_command;

#[path = "common/offline_state_fixture.rs"]
mod offline_state_fixture;
use offline_state_fixture::current_session;

#[path = "common/offline_target_guard.rs"]
mod offline_target_guard;
use offline_target_guard::UnreachedTargetFactoryV1;

#[path = "common/physical_enospc.rs"]
mod physical_enospc;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn offline_cancelled_budget_refuses_before_target_namespace_birth() -> TestResult {
    use quanta_index_searchd::app::state_migration::run_offline_state_command_v1;

    offline_cancelled_command::require_cancelled_before_staging(|command, config, budget| {
        Ok(run_offline_state_command_v1(
            command,
            config,
            budget,
            &NoStateMigrationFaultsV1,
            &CatalogVerifierV1,
            || -> Result<UnreachedTargetFactoryV1, CoreError> {
                Err(CoreError::InvalidContract(
                    "cancelled offline command resolved the target factory".into(),
                ))
            },
        )?)
    })
}

fn offline_test_binary() -> Result<LegacySearchdBinary, Box<dyn std::error::Error>> {
    LegacySearchdBinary::from_environment()
}

fn make_backup(binary: &LegacySearchdBinary, source: &Path, destination: &Path) -> TestResult {
    let _receipt = run_cli_success(
        binary,
        OfflineStateOperationV1::Backup.command_name(),
        source,
        Some(destination),
    )?;
    Ok(())
}

#[test]
fn kernel_file_limit_interrupts_offline_first_copy() -> TestResult {
    const FIRST_OBJECT: &str = "activations/000-first.bin";
    let binary = offline_test_binary()?;
    let source = private_root()?;
    let destination_parent = private_root()?;
    let destination = destination_parent.path().join("short-write-backup");
    let staging = staging_directory_for_v1(&destination);
    let source_object = source.path().join(FIRST_OBJECT);
    let lease = source.path().join(STATE_ROOT_LEASE_FILE_NAME);
    build_live_root(source.path())?;
    let object = vec![b'Z'; 2 * 1024 * 1024];
    fs::write(&source_object, &object)?;
    drop(StateRootLease::acquire(source.path())?);
    let frozen = freeze_current(source.path())?;
    let names = tree_names_outside_catalog(source.path())?;
    let catalog = live_catalog_receipt(source.path(), BUSY)?.content_digest_hex;
    let lock = fs::read(&lease)?;
    assert_no_source_markers(source.path())?;

    let mut command = binary.command_with_one_block_file_limit()?;
    configure_offline_command(
        &mut command,
        "backup-state",
        source.path(),
        Some(&destination),
    );
    // Pipe stdout and stderr so the file limit applies to the staged copy, not
    // the daemon's failure report.
    let refusal = command.output()?;
    let stderr = String::from_utf8(refusal.stderr)?;
    let mut failures = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("searchd: backup-state failed: "));
    let expected = format!(
        "storage failure: offline target file I/O write staged object: {}",
        std::io::Error::from_raw_os_error(rustix::io::Errno::FBIG.raw_os_error())
    );
    if refusal.status.code() != Some(70)
        || !refusal.stdout.is_empty()
        || failures.next() != Some(expected.as_str())
        || failures.next().is_some()
    {
        return Err(format!(
            "first staged copy did not report kernel EFBIG: exit={:?}, stdout={:?}, stderr={stderr:?}",
            refusal.status.code(),
            refusal.stdout
        )
        .into());
    }
    assert!(!destination.try_exists()?, "short write published a target");
    assert!(fs::symlink_metadata(&staging)?.is_dir(), "staging was lost");
    for entry in [
        FIRST_OBJECT,
        STATE_ROOT_MANIFEST_FILE_NAME,
        STATE_BACKUP_MANIFEST_FILE_NAME,
    ] {
        assert!(
            !staging.join(entry).try_exists()?,
            "completed entry {entry}"
        );
    }
    assert_eq!(freeze_current(source.path())?, frozen);
    assert_eq!(tree_names_outside_catalog(source.path())?, names);
    assert_eq!(
        live_catalog_receipt(source.path(), BUSY)?.content_digest_hex,
        catalog
    );
    assert_eq!(fs::read(&lease)?, lock);
    assert_eq!(fs::read(&source_object)?, object);
    assert_no_source_markers(source.path())?;

    drop(StateRootLease::acquire(&staging)?);
    fs::remove_dir_all(&staging)?;
    make_backup(&binary, source.path(), &destination)?;
    let _verified = verify_backup(&destination)?;
    assert_eq!(fs::read(destination.join(FIRST_OBJECT))?, object);
    assert!(!staging.try_exists()?);
    Ok(())
}

fn fill_private_volume(directory: &Path) -> TestResult {
    const TARGET_FREE: u64 = 8 * 1024 * 1024;
    const MIN_INITIAL_FREE: u64 = 24 * 1024 * 1024;
    let path = directory.join("owned-space-pressure.bin");
    let mut filler = OpenOptions::new().write(true).create_new(true).open(path)?;
    let device = filler.metadata()?.dev();
    let mut random = File::open("/dev/urandom")?;
    let mut chunk = vec![0_u8; 1024 * 1024];
    let mut initial = true;
    for _ in 0..128 {
        let volume = rustix::fs::fstatvfs(&filler)?;
        let total = volume
            .f_blocks
            .checked_mul(volume.f_frsize)
            .ok_or("bounded volume capacity overflow while filling")?;
        let available = volume
            .f_bavail
            .checked_mul(volume.f_frsize)
            .ok_or("bounded volume free-space overflow while filling")?;
        if total > 128 * 1024 * 1024 || filler.metadata()?.dev() != device {
            return Err("ENOSPC filler lost its bounded filesystem identity".into());
        }
        if initial && available < MIN_INITIAL_FREE {
            return Err("bounded filesystem lacked the required clean setup space".into());
        }
        initial = false;
        if available <= TARGET_FREE {
            return Ok(());
        }
        let remaining = available
            .checked_sub(TARGET_FREE)
            .ok_or("bounded volume free-space window underflow")?;
        let amount = usize::try_from(remaining.min(u64::try_from(chunk.len())?))?;
        let bytes = chunk
            .get_mut(..amount)
            .ok_or("ENOSPC filler exceeded its supplied buffer")?;
        random.read_exact(bytes)?;
        filler.write_all(bytes)?;
        filler.sync_all()?;
    }
    Err("bounded filesystem did not reach the required free-space window".into())
}

#[test]
#[ignore = "requires QUANTA_INDEX_PHYSICAL_ENOSPC_ROOT on an isolated bounded filesystem"]
fn actual_bounded_volume_enospc_preserves_source_and_unpublished_target() -> TestResult {
    const FIRST_OBJECT: &str = "activations/000-first.bin";
    let mount = physical_enospc::bounded_volume_root()?;
    let binary = offline_test_binary()?;
    let source = private_root()?;
    build_live_root(source.path())?;
    let source_object = source.path().join(FIRST_OBJECT);
    let original = vec![b'Z'; 16 * 1024 * 1024];
    fs::write(&source_object, &original)?;
    drop(StateRootLease::acquire(source.path())?);
    let mount_device = fs::metadata(&mount)?.dev();
    if fs::metadata(source.path())?.dev() == mount_device || binary.source_device() == mount_device
    {
        return Err("ENOSPC source or pinned executable is on the bounded volume".into());
    }
    let destination_parent = tempfile::Builder::new()
        .prefix("qidx-offline-enospc-")
        .tempdir_in(&mount)?;
    let destination = destination_parent.path().join("enospc-backup");
    let staging = staging_directory_for_v1(&destination);
    let frozen = freeze_current(source.path())?;
    let names = tree_names_outside_catalog(source.path())?;
    let catalog = live_catalog_receipt(source.path(), BUSY)?.content_digest_hex;
    let lease = source.path().join(STATE_ROOT_LEASE_FILE_NAME);
    let lock = fs::read(&lease)?;
    assert_no_source_markers(source.path())?;
    fill_private_volume(destination_parent.path())?;

    let mut command = offline_command(&binary, "backup-state", source.path(), Some(&destination))?;
    let refusal = command.output()?;
    let stderr = String::from_utf8(refusal.stderr)?;
    let failures: Vec<&str> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("searchd: backup-state failed: "))
        .collect();
    if refusal.status.code() != Some(70)
        || !refusal.stdout.is_empty()
        || failures.len() != 1
        || !failures[0].contains("offline target file I/O write staged object")
        || !failures[0].contains(
            std::io::Error::from_raw_os_error(rustix::io::Errno::NOSPC.raw_os_error())
                .to_string()
                .as_str(),
        )
    {
        return Err(format!(
            "first staged copy did not report kernel ENOSPC: exit={:?}, stdout={:?}, stderr={stderr:?}",
            refusal.status.code(),
            refusal.stdout
        )
        .into());
    }
    fs::remove_file(destination_parent.path().join("owned-space-pressure.bin"))?;
    File::open(destination_parent.path())?.sync_all()?;
    assert!(!destination.try_exists()?, "ENOSPC published a target");
    assert!(fs::symlink_metadata(&staging)?.is_dir(), "staging was lost");
    for entry in [
        FIRST_OBJECT,
        STATE_ROOT_MANIFEST_FILE_NAME,
        STATE_BACKUP_MANIFEST_FILE_NAME,
    ] {
        assert!(
            !staging.join(entry).try_exists()?,
            "completed entry {entry}"
        );
    }
    assert_eq!(freeze_current(source.path())?, frozen);
    assert_eq!(tree_names_outside_catalog(source.path())?, names);
    assert_eq!(
        live_catalog_receipt(source.path(), BUSY)?.content_digest_hex,
        catalog
    );
    assert_eq!(fs::read(&lease)?, lock);
    assert_eq!(fs::read(&source_object)?, original);
    assert_no_source_markers(source.path())?;

    drop(StateRootLease::acquire(&staging)?);
    fs::remove_dir_all(&staging)?;
    make_backup(&binary, source.path(), &destination)?;
    let _verified = verify_backup(&destination)?;
    assert_eq!(fs::read(destination.join(FIRST_OBJECT))?, original);
    assert!(!staging.try_exists()?);
    Ok(())
}

/// The frozen inventory of a produced backup root.
fn freeze_backup(root: &Path) -> Result<SourceFreezeReceiptV1, Box<dyn std::error::Error>> {
    Ok(backup_session(root)?.before().clone())
}

/// Verify a backup root through its read-only session.
fn verify_backup(root: &Path) -> Result<OfflineStateVerificationV1, Box<dyn std::error::Error>> {
    let session = backup_session(root)?;
    Ok(run_offline_verify_v1(&session, &CatalogVerifierV1)?)
}

/// Verify a produced current root through its lease-bound session.
fn verify_current(root: &Path) -> Result<OfflineStateVerificationV1, Box<dyn std::error::Error>> {
    let session = current_session(root)?;
    Ok(run_offline_verify_v1(&session, &CatalogVerifierV1)?)
}

/// Custody for a produced backup root: read-only, creating nothing.
fn backup_session(root: &Path) -> Result<OfflineSourceSessionV1, Box<dyn std::error::Error>> {
    Ok(OfflineSourceSessionV1::open_produced_backup(root)?)
}

#[test]
fn backup_manifest_replacement_after_custody_refuses_verify_and_restore() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    let binary = offline_test_binary()?;
    make_backup(&binary, source.path(), &backup)?;
    let session = backup_session(&backup)?;
    let path = backup.join(STATE_BACKUP_MANIFEST_FILE_NAME);
    let mut manifest = read_root_manifest_v1(&path)?;
    manifest.catalog_rows += 1;
    fs::write(&path, manifest.encode())?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
        .expect_err("verification must consume the authority pinned at session open");
    assert!(
        matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootInsecure,
                ..
            }
        ),
        "wrong custody refusal: {error:?}"
    );
    let restored = restore_parent.path().join("restored");
    let error = run_offline_restore_v1(
        &session,
        &restored,
        &CatalogVerifierV1,
        &UnreachedTargetFactoryV1,
        &NoStateMigrationFaultsV1,
        &RequestBudgetV1::unbounded(),
    )
    .expect_err("restore cannot re-admit a replacement authority");
    assert!(
        matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootInsecure,
                ..
            }
        ),
        "wrong custody refusal: {error:?}"
    );
    assert!(!restored.exists());
    assert!(!staging_directory_for_v1(&restored).exists());
    Ok(())
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
    let binary = offline_test_binary()?;
    make_backup(&binary, source.path(), &destination)?;
    let _verified = verify_backup(&destination)?;

    let manifest = read_root_manifest_v1(&destination.join(STATE_BACKUP_MANIFEST_FILE_NAME))?;
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
fn verify_state_refuses_a_frozen_root_whose_catalog_was_replaced() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    let other = private_root()?;
    build_live_root(source.path())?;
    let frozen = parent.path().join("frozen");
    let binary = offline_test_binary()?;
    make_backup(&binary, source.path(), &frozen)?;
    let _verified = verify_backup(&frozen)?;
    drop(SqliteCatalog::open(other.path(), BUSY)?);
    let replacement = other.path().join("catalog/catalog-v1.sqlite");
    let frozen_catalog = frozen.join("catalog/catalog-v1.sqlite");
    assert_ne!(
        verify_snapshot(&replacement)?.content_digest_hex,
        verify_snapshot(&frozen_catalog)?.content_digest_hex,
        "replacement must be a valid catalog with different logical rows"
    );
    let _copied = fs::copy(&replacement, &frozen_catalog)?;
    let session = backup_session(&frozen)?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
        .expect_err("a swapped catalog must be refused");
    assert!(
        matches!(
            &error,
            CoreError::Typed { code: SearchPlaneErrorCodeV2::NotFound, message }
                if message.contains("catalog/catalog-v1.sqlite")
        ),
        "verification must name the replaced manifest object: {error:?}"
    );
    Ok(())
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
    let lock_before = fs::read(source.path().join(STATE_ROOT_LEASE_FILE_NAME))?;

    let before = freeze_current(source.path())?;
    let names_before = tree_names_outside_catalog(source.path())?;
    let catalog_before = live_catalog_receipt(source.path(), BUSY)?.content_digest_hex;
    assert_no_source_markers(source.path())?;

    let destination = destination_parent.path().join("frozen");
    let binary = offline_test_binary()?;
    make_backup(&binary, source.path(), &destination)?;

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
        live_catalog_receipt(source.path(), BUSY)?.content_digest_hex,
        catalog_before,
        "the live catalog's logical content must be unchanged"
    );
    assert_eq!(
        fs::read(source.path().join(STATE_ROOT_LEASE_FILE_NAME))?,
        lock_before,
        "the daemon lock bytes must be untouched"
    );
    assert_no_source_markers(source.path())?;
    Ok(())
}

#[test]
fn restore_keeps_the_backup_source_bit_identical() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    let binary = offline_test_binary()?;
    make_backup(&binary, source.path(), &backup)?;

    let before = freeze_backup(&backup)?;
    let names_before = tree_names_outside_catalog(&backup)?;
    let restored = restore_parent.path().join("restored");
    let _receipt = run_cli_success(&binary, "restore-state", &backup, Some(&restored))?;

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

/// A source byte change before backup is refused before staging creation.
/// Same-length mutation proves the digest (not just the size) is compared.
#[test]
fn pre_backup_source_byte_change_refuses_before_staging_birth() -> TestResult {
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
        &UnreachedTargetFactoryV1,
        &NoStateMigrationFaultsV1,
        &RequestBudgetV1::unbounded(),
    )
    .expect_err("source drift must refuse the publish");
    assert!(
        matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootInsecure,
                ..
            }
        ),
        "wrong custody refusal: {error:?}"
    );
    assert!(!destination.exists(), "zero destination publish on drift");
    assert!(
        !staging.exists(),
        "drift refusal must precede staging birth"
    );
    fs::write(&victim, &original)?;
    Ok(())
}

fn crash_before_cutover(
    binary: &LegacySearchdBinary,
    operation: &str,
    source: &Path,
    destination: &Path,
) -> TestResult {
    let mut command = offline_command(binary, operation, source, Some(destination))?;
    let _configured = command.env(STATE_MIGRATION_CRASH_ENV, "before-cutover-rename");
    let output = wait_for_command_exit(binary, command)?;
    if output.status.code() != Some(STATE_MIGRATION_CRASH_EXIT_CODE) || !output.stdout.is_empty() {
        return Err(format!(
            "{operation} did not crash before cutover, exit={}, stdout={:?}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

#[test]
fn offline_missing_physical_profile_refuses_before_source_or_staging_open() -> TestResult {
    const REQUIRED_PROFILE: &str =
        "offline staged deep-open requires an explicit complete physical policy";
    let parent = private_root()?;
    let source = parent.path().join("unopened-source");
    let destination = parent.path().join("unopened-destination");
    let staging = staging_directory_for_v1(&destination);

    {
        let binary = offline_test_binary()?;
        let mut command = offline_command(&binary, "backup-state", &source, Some(&destination))?;
        for (name, _) in legacy_physical_env() {
            let _configured = command.env_remove(name);
        }
        let output = wait_for_refusal(&binary, command)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let expected =
            format!("searchd: backup-state failed: invalid contract: {REQUIRED_PROFILE}");
        assert!(
            output.status.code() == Some(70)
                && output.stdout.is_empty()
                && stderr.lines().any(|line| line == expected),
            "wrong physical preflight error: {stderr}"
        );
    }

    assert!(!source.try_exists()?, "missing policy touched the source");
    assert!(
        !destination.try_exists()?,
        "missing policy published a destination"
    );
    assert!(!staging.try_exists()?, "missing policy created staging");
    Ok(())
}

#[test]
fn offline_first_copied_object_refuses_tiny_capacity_before_file_birth() -> TestResult {
    const FIRST_OBJECT_BYTES: usize = 2 * 1024 * 1024;
    const INVENTORY_BYTES: u64 = 1024 * 1024;
    const RESERVED_BYTES: u64 = 64 * 1024;
    const CANDIDATE_BYTES: u64 = 512 * 1024;
    const SOURCE_BYTES: u64 = 1024;
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let first = source.path().join("activations/000-first.bin");
    fs::write(&first, vec![b'Z'; FIRST_OBJECT_BYTES])?;
    let retained = fs::read(&first)?;
    let destination = parent.path().join("unpublished-backup");
    let staging = staging_directory_for_v1(&destination);

    {
        let binary = offline_test_binary()?;
        let mut command =
            offline_command(&binary, "backup-state", source.path(), Some(&destination))?;
        let _configured = command.envs([
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES",
                INVENTORY_BYTES.to_string(),
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_BYTES",
                RESERVED_BYTES.to_string(),
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_BYTES",
                RESERVED_BYTES.to_string(),
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_BYTES",
                RESERVED_BYTES.to_string(),
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_BYTES",
                RESERVED_BYTES.to_string(),
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_BYTES",
                RESERVED_BYTES.to_string(),
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CONTROL_FILE_MAX_BYTES",
                RESERVED_BYTES.to_string(),
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CANDIDATE_FILE_MAX_BYTES",
                CANDIDATE_BYTES.to_string(),
            ),
            (
                "QUANTA_INDEX_SOURCE_PUBLICATION_MAX_BYTES",
                SOURCE_BYTES.to_string(),
            ),
        ]);
        let output = wait_for_refusal(&binary, command)?;
        require_cli_refusal_output(
            &output,
            "backup-state",
            &[SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded],
        )?;
    }

    let mut staging_regular_files = Vec::new();
    if staging.try_exists()? {
        let mut directories = vec![staging.clone()];
        while let Some(directory) = directories.pop() {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    directories.push(entry.path());
                } else if kind.is_file() {
                    staging_regular_files.push(entry.path().strip_prefix(&staging)?.to_path_buf());
                } else {
                    return Err(format!(
                        "unexpected staging entry after capacity refusal: {}",
                        entry.path().display()
                    )
                    .into());
                }
            }
        }
    }
    staging_regular_files.sort();
    if destination.try_exists()?
        || staging_regular_files
            .iter()
            .any(|relative| relative != Path::new(STATE_ROOT_LEASE_FILE_NAME))
        || fs::read(&first)? != retained
    {
        return Err(format!(
            "first object capacity refusal lost pre-birth/source invariant; staging files {staging_regular_files:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn a_restore_interrupted_before_the_manifest_leaves_the_destination_absent() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    let binary = offline_test_binary()?;
    make_backup(&binary, source.path(), &backup)?;
    let restored = restore_parent.path().join("restored");

    crash_before_cutover(&binary, "restore-state", &backup, &restored)?;
    assert!(!restored.exists(), "no cutover may have happened");

    let staging = staging_directory_for_v1(&restored);
    let _sealed = read_root_manifest_v1(&staging.join(STATE_ROOT_MANIFEST_FILE_NAME))?;
    require_cli_refusal(
        &binary,
        "restore-state",
        &backup,
        Some(&restored),
        SearchPlaneErrorCodeV2::InvalidRequest,
    )?;
    assert!(staging.join(STATE_ROOT_MANIFEST_FILE_NAME).is_file());
    assert!(!restored.exists());
    // This fixture owns the interrupted staging tree.
    fs::remove_dir_all(&staging)?;
    let _receipt = run_cli_success(&binary, "restore-state", &backup, Some(&restored))?;
    assert!(restored.join(STATE_ROOT_MANIFEST_FILE_NAME).is_file());
    let _verified = verify_current(&restored)?;
    Ok(())
}

/// Preserve the source at every verified interruption boundary.
#[test]
fn backup_failure_and_interruption_keep_the_source_identical() -> TestResult {
    let binary = offline_test_binary()?;
    // The pinned subprocess can prove the pre-cutover boundary from its
    // sealed staging manifest. Exit 88 alone cannot distinguish earlier faults.
    let points = [StateMigrationFaultPointV1::BeforeCutoverRename];
    for point in points {
        let source = private_root()?;
        let destination_parent = private_root()?;
        build_live_root(source.path())?;
        let setup = StateRootLease::acquire(source.path())?;
        drop(setup);

        let before = freeze_current(source.path())?;
        let names_before = tree_names_outside_catalog(source.path())?;
        let destination = destination_parent.path().join("frozen");
        crash_before_cutover(&binary, "backup-state", source.path(), &destination)?;
        assert!(
            !destination.exists(),
            "no cutover may have happened for {point:?}"
        );
        let staging = staging_directory_for_v1(&destination);
        assert!(
            staging.is_dir(),
            "interrupted staging must remain for {point:?}"
        );
        if point == StateMigrationFaultPointV1::BeforeCutoverRename {
            let _verified = verify_backup(&staging)?;
        } else {
            assert!(
                !staging.join(STATE_BACKUP_MANIFEST_FILE_NAME).try_exists()?,
                "backup manifest was published before the {point:?} interruption"
            );
        }
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
