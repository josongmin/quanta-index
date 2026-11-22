//! Offline command receipts and refusals against the pinned old-Q executable.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneErrorCodeV2, SourceFileKey, SourcePublicationEvent, TextQueryResponse,
    lex::LanguageCode,
};
use quanta_index_core::{CoreError, GenerationStorageKeyV1};
use quanta_index_sdk::{FileOwnershipBatch, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::state_format::{
    NoStateMigrationFaultsV1, STATE_BACKUP_MANIFEST_FILE_NAME, STATE_ROOT_LEASE_FILE_NAME,
    STATE_ROOT_MANIFEST_FILE_NAME, StateRootFormatV1, StateRootManifestV1,
    detect_state_root_format_v1, inventory_state_root_v1, read_root_manifest_v1,
    refuse_existing_destination_v1, refuse_legacy_state_root_v1, staging_directory_for_v1,
    write_root_manifest_last_v1,
};
use quanta_index_searchd::app::state_migration::{
    CatalogSnapshotPort, CatalogSnapshotV1, OfflineSourceSessionV1, run_offline_verify_v1,
};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, LegacySearchdCommand, legacy_source_batch, publish_legacy_source,
    publish_legacy_source_batch, request_when_ready,
};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1, private_tempdir as private_root,
    semantic_source_scopes_for_chunk_records,
};
use sha2::{Digest as _, Sha256};

#[path = "common/offline_catalog_verify.rs"]
mod offline_catalog_verify;
use offline_catalog_verify::CatalogVerifierV1;

use crate::fail_closed_wait::{RealTicker, wait_for};
use crate::offline_lexical_pool::pooled_object_of_kind;
use crate::offline_live_root::build_live_root;
use crate::offline_tree_inventory::tree_names_outside_catalog;
use crate::{legacy_start_probe, offline_legacy_root::build_legacy_root};

type TestResult = Result<(), Box<dyn Error>>;
const INCARNATION: &str = "activations/.activation-root-incarnation-v1";
const MIGRATION_PATH: &str = "src/migration.rs";
const MIGRATION_NEEDLE: &str = "migration_needle_47";
const MIGRATION_BODY: &str = "pub fn migration_needle_47() -> bool { true }\n";
// The Source fixture appends one framing newline to every chunk.
const MIGRATION_SOURCE: &[u8] = b"pub fn migration_needle_47() -> bool { true }\n\n";
const MIGRATION_CHUNK_ID: &str = "e2e-1-src/migration.rs";
// SHA-256 of the CodeSearch file domain and length-framed fixture repo/path.
const MIGRATION_FILE_ID: &str =
    "file:3c95b3152f276384c83c74ed7f113e9eb0caaa7f944d588e26d6360fb6c7e66a";
const RESTORED_PATH: &str = "src/post_restore.rs";
const RESTORED_NEEDLE: &str = "restored_fresh_needle_89";
const RESTORED_CHUNK_ID: &str = "restored-fresh-chunk";
const RESTORED_FILE_ID: &str =
    "file:ce5f197dbf2c55b121f3c6ef865ba26dd0fa5df2e91a35b55c14db6266005204";

fn assert_restore_catalog_image(
    backup: &Path,
    restored: &Path,
    backup_manifest: &StateRootManifestV1,
    restored_manifest: &StateRootManifestV1,
) -> TestResult {
    const CATALOG: &str = "catalog/catalog-v1.sqlite";
    let original = fs::read(backup.join(CATALOG))?;
    let produced = fs::read(restored.join(CATALOG))?;
    assert_eq!(produced.len(), original.len());
    // SQLite may update only these two database-header counters while
    // normalizing the copied image from WAL to DELETE mode.
    for (offset, (before, after)) in original.iter().zip(&produced).enumerate() {
        if !(24..28).contains(&offset) && !(92..96).contains(&offset) {
            assert_eq!(after, before, "unexpected Catalog byte change at {offset}");
        }
    }
    for (manifest, bytes) in [(backup_manifest, &original), (restored_manifest, &produced)] {
        let entry = manifest
            .objects
            .iter()
            .find(|entry| entry.relative_path == CATALOG)
            .ok_or("manifest has no Catalog image")?;
        assert_eq!(entry.byte_size, u64::try_from(bytes.len())?);
        assert_eq!(entry.digest_hex, format!("{:x}", Sha256::digest(bytes)));
    }
    assert_eq!(
        restored_manifest.catalog_digest_hex,
        backup_manifest.catalog_digest_hex
    );
    assert_eq!(restored_manifest.catalog_rows, backup_manifest.catalog_rows);
    Ok(())
}

pub(super) fn offline_command<'binary>(
    binary: &'binary LegacySearchdBinary,
    operation: &str,
    source: &Path,
    destination: Option<&Path>,
) -> Result<LegacySearchdCommand<'binary>, Box<dyn Error>> {
    let mut command = binary.command()?;
    configure_offline_command(&mut command, operation, source, destination);
    Ok(command)
}

pub(super) fn configure_offline_command(
    command: &mut LegacySearchdCommand<'_>,
    operation: &str,
    source: &Path,
    destination: Option<&Path>,
) {
    // Every offline case supplies its own policy and opts into faults explicitly.
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"QUANTA_INDEX_") {
            let _configured = command.env_remove(key);
        }
    }
    let _configured = command.arg(operation).arg("--source").arg(source);
    if let Some(destination) = destination {
        let _configured = command.arg("--destination").arg(destination);
    }
    let _configured = command.apply_searchd_env(8);
}

fn run_cli(
    binary: &LegacySearchdBinary,
    operation: &str,
    source: &Path,
    destination: Option<&Path>,
) -> Result<Output, Box<dyn Error>> {
    legacy_start_probe::wait_for_command_exit(
        binary,
        offline_command(binary, operation, source, destination)?,
    )
}

pub(super) fn run_cli_success(
    binary: &LegacySearchdBinary,
    operation: &str,
    source: &Path,
    destination: Option<&Path>,
) -> Result<String, Box<dyn Error>> {
    let output = run_cli(binary, operation, source, destination)?;
    if !output.status.success() {
        return Err(format!(
            "old-Q {operation} failed: exit={:?}, stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    if !stdout.starts_with(&format!("{operation}: ")) {
        return Err(format!("old-Q {operation} did not return its receipt: {stdout:?}").into());
    }
    Ok(stdout)
}

pub(super) fn require_cli_refusal(
    binary: &LegacySearchdBinary,
    operation: &str,
    source: &Path,
    destination: Option<&Path>,
    expected: SearchPlaneErrorCodeV2,
) -> TestResult {
    let output = run_cli(binary, operation, source, destination)?;
    require_cli_refusal_output(&output, operation, &[expected])
}

pub(super) fn require_cli_refusal_output(
    output: &Output,
    operation: &str,
    expected: &[SearchPlaneErrorCodeV2],
) -> TestResult {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let prefix = format!("searchd: {operation} failed: ");
    let mut error_lines = stderr
        .lines()
        .filter_map(|line| line.strip_prefix(prefix.as_str()));
    let reported_code = error_lines
        .next()
        .and_then(|error| error.strip_prefix("typed failure "))
        .and_then(|error| error.split_once(": ").map(|(code, _)| code));
    if output.status.code() != Some(70)
        || !output.stdout.is_empty()
        || error_lines.next().is_some()
        || !expected
            .iter()
            .any(|code| reported_code == Some(code.as_wire_str()))
    {
        return Err(format!(
            "old-Q {operation} did not report one {expected:?} refusal with empty stdout: exit={:?}, stdout={:?}, stderr={stderr}",
            output.status.code(),
            output.stdout
        )
        .into());
    }
    Ok(())
}

#[test]
fn offline_refusal_code_must_come_from_the_command_error() {
    use std::os::unix::process::ExitStatusExt as _;
    use std::process::ExitStatus;

    let expected = [SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded];
    let mut output = Output {
        status: ExitStatus::from_raw(70 << 8),
        stdout: Vec::new(),
        stderr: b"searchd: backup-state failed: typed failure INVALID_REQUEST: mentions INGEST_RESOURCE_BUDGET_EXCEEDED\n".to_vec(),
    };
    assert!(require_cli_refusal_output(&output, "backup-state", &expected).is_err());

    output.stderr =
        b"searchd: backup-state failed: typed failure INGEST_RESOURCE_BUDGET_EXCEEDED: first object exceeds capacity\n".to_vec();
    assert!(require_cli_refusal_output(&output, "backup-state", &expected).is_ok());

    output.stderr = concat!(
        "searchd: backup-state failed: typed failure INVALID_REQUEST: invalid path\n",
        "searchd: backup-state failed: typed failure INGEST_RESOURCE_BUDGET_EXCEEDED: second error\n",
    )
    .as_bytes()
    .to_vec();
    assert!(require_cli_refusal_output(&output, "backup-state", &expected).is_err());
}

fn require_manifest_receipt(receipt: &str, digest: &str) -> TestResult {
    let expected = format!("manifest={digest}");
    let fields = receipt
        .split_ascii_whitespace()
        .filter(|field| field.starts_with("manifest="));
    if !fields.eq([expected.as_str()]) {
        return Err(
            format!("old-Q receipt did not name exactly one {expected}: {receipt:?}").into(),
        );
    }
    Ok(())
}

/// Mutate a root after the verifier has walked the objects and read the catalog.
struct PostVerifyMutationV1 {
    path: PathBuf,
    bytes: Vec<u8>,
}

impl CatalogSnapshotPort for PostVerifyMutationV1 {
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

#[test]
fn audit_verify_current_refuses_catalog_changed_after_catalog_read() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let restored = parent.path().join("restored");
    let _restore_receipt = run_cli_success(&binary, "restore-state", &backup, Some(&restored))?;
    let session = OfflineSourceSessionV1::open_current(StateRootLease::acquire(&restored)?)?;
    let path = restored.join("catalog/catalog-v1.sqlite");
    let mut bytes = fs::read(&path)?;
    bytes.push(0xFF);
    let error = run_offline_verify_v1(&session, &PostVerifyMutationV1 { path, bytes })
        .expect_err("a current-root catalog change after its read must not verify successfully");
    assert!(
        matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootInsecure,
                ..
            }
        ),
        "wrong custody refusal: {error}"
    );
    Ok(())
}

#[test]
fn audit_verify_current_refuses_root_replaced_after_custody() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let restored = parent.path().join("restored");
    let _first_receipt = run_cli_success(&binary, "restore-state", &backup, Some(&restored))?;
    let session = OfflineSourceSessionV1::open_current(StateRootLease::acquire(&restored)?)?;
    fs::rename(&restored, parent.path().join("old-root-with-held-lease"))?;
    let _replacement_receipt = run_cli_success(&binary, "restore-state", &backup, Some(&restored))?;
    let error = run_offline_verify_v1(&session, &CatalogVerifierV1)
        .expect_err("a lease on the replaced inode is not custody of the new root");
    assert!(
        matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootInsecure,
                ..
            }
        ),
        "wrong custody refusal: {error}"
    );
    Ok(())
}

#[test]
fn audit_verify_current_does_not_reset_non_catalog_custody() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let restored = parent.path().join("restored");
    let _restore_receipt = run_cli_success(&binary, "restore-state", &backup, Some(&restored))?;
    let session = OfflineSourceSessionV1::open_current(StateRootLease::acquire(&restored)?)?;
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
    assert!(
        matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootInsecure,
                ..
            }
        ),
        "wrong custody refusal: {error}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn audit_backup_custody_refuses_linked_owned_sidecars() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
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
    assert!(
        matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootInsecure,
                ..
            }
        ),
        "wrong custody refusal: {error}"
    );
    fs::remove_file(&sidecar)?;
    std::os::unix::fs::symlink(&alias, &sidecar)?;
    let error = OfflineSourceSessionV1::open_produced_backup(&backup)
        .expect_err("a vendor filename cannot grant custody of a symlink");
    assert!(
        matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootInsecure,
                ..
            }
        ),
        "wrong custody refusal: {error}"
    );
    assert_eq!(
        fs::read(&alias)?,
        b"must not be adopted as SQLite bookkeeping"
    );
    Ok(())
}

#[test]
fn verify_state_refuses_object_changed_after_catalog_verification() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let backup = parent.path().join("backup");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let session = OfflineSourceSessionV1::open_produced_backup(&backup)?;
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
        match &error {
            CoreError::Typed { code, .. } => Some(*code),
            _ => None,
        },
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
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let session = OfflineSourceSessionV1::open_produced_backup(&backup)?;
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
        match &error {
            CoreError::Typed { code, .. } => Some(*code),
            _ => None,
        },
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
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let session = OfflineSourceSessionV1::open_produced_backup(&backup)?;
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
        match &error {
            CoreError::Typed { code, .. } => Some(*code),
            _ => None,
        },
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
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
    let binary = LegacySearchdBinary::from_environment()?;
    require_cli_refusal(
        &binary,
        "backup-state",
        source.path(),
        Some(&destination),
        SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
    )?;
    if destination.exists() || inventory_state_root_v1(source.path(), &[])? != before {
        return Err("refused backup created a destination or changed its source".into());
    }
    Ok(())
}

#[test]
fn verify_state_refuses_a_root_that_advertises_no_manifest() -> TestResult {
    let source = private_root()?;
    build_live_root(source.path())?;
    let before = tree_names_outside_catalog(source.path())?;
    let binary = LegacySearchdBinary::from_environment()?;
    require_cli_refusal(
        &binary,
        "verify-state",
        source.path(),
        None,
        SearchPlaneErrorCodeV2::NotFound,
    )?;
    if tree_names_outside_catalog(source.path())? != before {
        return Err("refused verify created a file outside Catalog in the source".into());
    }
    Ok(())
}

#[test]
fn restore_refuses_a_backup_root_without_a_backup_manifest() -> TestResult {
    let backup = private_root()?;
    let parent = private_root()?;
    fs::write(backup.path().join("stray.bin"), b"no manifest here")?;
    let destination = parent.path().join("restored");
    let binary = LegacySearchdBinary::from_environment()?;
    require_cli_refusal(
        &binary,
        "restore-state",
        backup.path(),
        Some(&destination),
        SearchPlaneErrorCodeV2::NotFound,
    )?;
    if destination.exists() {
        return Err("refused restore created a destination".into());
    }
    Ok(())
}

#[test]
fn a_non_empty_destination_and_a_self_destination_are_refused() -> TestResult {
    let source = private_root()?;
    build_live_root(source.path())?;
    let binary = LegacySearchdBinary::from_environment()?;

    let populated = private_root()?;
    fs::write(populated.path().join("occupied"), b"x")?;
    let error = refuse_existing_destination_v1(populated.path(), source.path())
        .expect_err("a populated destination must be refused");
    if !matches!(
        &error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::InvalidRequest,
            ..
        }
    ) {
        return Err(format!("populated destination was not refused typed: {error}").into());
    }

    let empty = private_root()?;
    let error = refuse_existing_destination_v1(empty.path(), source.path())
        .expect_err("an existing empty destination cannot be the rename target");
    if !matches!(
        &error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::InvalidRequest,
            ..
        }
    ) {
        return Err(format!("empty destination was not refused typed: {error}").into());
    }
    require_cli_refusal(
        &binary,
        "backup-state",
        source.path(),
        Some(empty.path()),
        SearchPlaneErrorCodeV2::InvalidRequest,
    )?;
    if staging_directory_for_v1(empty.path()).exists()
        || fs::read_dir(empty.path())?.next().is_some()
    {
        return Err("refused backup changed the existing empty destination or staging".into());
    }

    let error = refuse_existing_destination_v1(source.path(), source.path())
        .expect_err("a source that is its own destination must be refused");
    if !matches!(
        &error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::InvalidRequest,
            ..
        }
    ) {
        return Err(format!("self destination was not refused typed: {error}").into());
    }
    require_cli_refusal(
        &binary,
        "backup-state",
        source.path(),
        Some(source.path()),
        SearchPlaneErrorCodeV2::InvalidRequest,
    )?;
    if staging_directory_for_v1(source.path()).exists()
        || fs::read(populated.path().join("occupied"))? != b"x"
    {
        return Err("refused backup created staging or changed the populated fixture".into());
    }
    Ok(())
}

#[test]
fn a_restored_current_root_with_a_manifest_is_not_refused() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let binary = LegacySearchdBinary::from_environment()?;
    let backup = backup_parent.path().join("backup-root");
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let target = restore_parent.path().join("restored");
    let restore_receipt = run_cli_success(&binary, "restore-state", &backup, Some(&target))?;
    if detect_state_root_format_v1(&target)? != (StateRootFormatV1::CurrentV1 { manifest: true }) {
        return Err("old-Q restore did not produce a current root with a manifest".into());
    }
    refuse_legacy_state_root_v1(&target)?;
    let manifest = read_root_manifest_v1(&target.join(STATE_ROOT_MANIFEST_FILE_NAME))?;
    require_manifest_receipt(&restore_receipt, &manifest.manifest_digest_hex())
}

#[test]
fn verify_state_runs_from_the_command_surface_without_a_destination() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let verify_receipt = run_cli_success(&binary, "verify-state", &backup, None)?;
    let manifest = read_root_manifest_v1(&backup.join("state-backup-manifest-v1.txt"))?;
    require_manifest_receipt(&verify_receipt, &manifest.manifest_digest_hex())
}

#[test]
fn the_backup_root_carries_the_backup_manifest_name_only() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    if !backup.join("state-backup-manifest-v1.txt").is_file()
        || backup.join(STATE_ROOT_MANIFEST_FILE_NAME).exists()
    {
        return Err("old-Q backup used the wrong manifest name".into());
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
    let binary = LegacySearchdBinary::from_environment()?;

    let backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let backup_manifest = read_root_manifest_v1(&backup.join("state-backup-manifest-v1.txt"))?;
    require_manifest_receipt(&backup_receipt, &backup_manifest.manifest_digest_hex())?;
    let restore_receipt = run_cli_success(&binary, "restore-state", &backup, Some(&restored))?;
    let restored_manifest = read_root_manifest_v1(&restored.join(STATE_ROOT_MANIFEST_FILE_NAME))?;
    require_manifest_receipt(&restore_receipt, &restored_manifest.manifest_digest_hex())?;
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
        .filter(|entry| {
            entry.relative_path != INCARNATION && entry.relative_path != "catalog/catalog-v1.sqlite"
        })
        .collect::<Vec<_>>();
    let backup_other = backup_manifest
        .objects
        .iter()
        .filter(|entry| entry.relative_path != "catalog/catalog-v1.sqlite")
        .collect::<Vec<_>>();
    assert_eq!(
        restored_other, backup_other,
        "non-Catalog objects must remain bit identical"
    );
    assert_restore_catalog_image(&backup, &restored, &backup_manifest, &restored_manifest)?;
    assert_eq!(
        restored_manifest.catalog_digest_hex, backup_manifest.catalog_digest_hex,
        "the restored catalog identity must equal the backup's exactly"
    );
    assert!(restored_manifest.catalog_rows > 0);
    let verify_receipt = run_cli_success(&binary, "verify-state", &restored, None)?;
    require_manifest_receipt(&verify_receipt, &restored_manifest.manifest_digest_hex())?;
    if !verify_receipt.contains(&format!("catalog-rows={}", backup_manifest.catalog_rows)) {
        return Err("old-Q verify receipt differs from the backup Catalog rows".into());
    }
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
    let binary = LegacySearchdBinary::from_environment()?;

    let backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    assert_eq!(fs::read(backup.join(INCARNATION))?, original);
    let restore_receipt = run_cli_success(&binary, "restore-state", &backup, Some(&restored))?;
    let rotated = fs::read(restored.join(INCARNATION))?;
    assert_eq!(rotated.len(), original.len());
    assert_ne!(rotated, original);
    let backup_manifest = read_root_manifest_v1(&backup.join("state-backup-manifest-v1.txt"))?;
    let restored_manifest = read_root_manifest_v1(&restored.join(STATE_ROOT_MANIFEST_FILE_NAME))?;
    require_manifest_receipt(&backup_receipt, &backup_manifest.manifest_digest_hex())?;
    require_manifest_receipt(&restore_receipt, &restored_manifest.manifest_digest_hex())?;
    let backup_other = backup_manifest
        .objects
        .iter()
        .filter(|entry| {
            entry.relative_path != INCARNATION && entry.relative_path != "catalog/catalog-v1.sqlite"
        })
        .collect::<Vec<_>>();
    let restored_other = restored_manifest
        .objects
        .iter()
        .filter(|entry| {
            entry.relative_path != INCARNATION && entry.relative_path != "catalog/catalog-v1.sqlite"
        })
        .collect::<Vec<_>>();
    assert_eq!(restored_other, backup_other);
    assert_restore_catalog_image(&backup, &restored, &backup_manifest, &restored_manifest)?;
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
    let verify_receipt = run_cli_success(&binary, "verify-state", &restored, None)?;
    require_manifest_receipt(&verify_receipt, &restored_manifest.manifest_digest_hex())?;
    Ok(())
}

#[test]
fn verify_state_refuses_a_missing_unadvertised_and_tampered_object() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let frozen = parent.path().join("frozen");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&frozen))?;

    // A body absent from the manifest cannot acquire authority by filename.
    fs::write(frozen.join("unadvertised.bin"), b"extra")?;
    require_cli_refusal(
        &binary,
        "verify-state",
        &frozen,
        None,
        SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
    )?;
    fs::remove_file(frozen.join("unadvertised.bin"))?;

    // Select an object actually advertised by this independent manifest.
    let backup_manifest = read_root_manifest_v1(&frozen.join("state-backup-manifest-v1.txt"))?;
    let advertised = backup_manifest
        .objects
        .iter()
        .find(|object| object.relative_path.starts_with("authorities/"))
        .ok_or("the fixture advertises an authority object")?;
    let victim = frozen.join(&advertised.relative_path);
    let bytes = fs::read(&victim)?;
    fs::remove_file(&victim)?;
    require_cli_refusal(
        &binary,
        "verify-state",
        &frozen,
        None,
        SearchPlaneErrorCodeV2::NotFound,
    )?;
    fs::write(&victim, &bytes)?;

    // Retain the original source-admission oracle as well as CLI refusal.
    let manifest_path = frozen.join("state-backup-manifest-v1.txt");
    let manifest = fs::read_to_string(&manifest_path)?;
    let tampered = manifest.replace("catalog-rows ", "catalog-rows 9");
    assert_ne!(tampered, manifest, "the fixture must change the body");
    fs::write(&manifest_path, &tampered)?;
    let error = OfflineSourceSessionV1::open_produced_backup(&frozen)
        .expect_err("a tampered manifest must be refused at source admission");
    if !matches!(
        &error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
            ..
        }
    ) {
        return Err(format!("tampered manifest was not refused at admission: {error}").into());
    }
    require_cli_refusal(
        &binary,
        "verify-state",
        &frozen,
        None,
        SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
    )?;
    Ok(())
}

#[test]
fn restore_refuses_a_tampered_backup_manifest() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let manifest_path = backup.join("state-backup-manifest-v1.txt");
    let manifest = fs::read_to_string(&manifest_path)?;
    fs::write(
        &manifest_path,
        manifest.replace("catalog-rows ", "catalog-rows 7"),
    )?;
    let restored = restore_parent.path().join("restored");
    require_cli_refusal(
        &binary,
        "restore-state",
        &backup,
        Some(&restored),
        SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
    )?;
    if restored.exists() {
        return Err("tampered backup was published as a restored root".into());
    }
    Ok(())
}

#[test]
fn verify_state_with_a_live_lease_refuses_state_root_in_use() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    build_live_root(source.path())?;
    let backup = backup_parent.path().join("backup-root");
    let binary = LegacySearchdBinary::from_environment()?;
    let _backup_receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    // A restored root routes verify through the lease-bound current session.
    let live = restore_parent.path().join("live");
    let _restore_receipt = run_cli_success(&binary, "restore-state", &backup, Some(&live))?;

    let _daemon = StateRootLease::acquire(&live)?;
    let names_before = tree_names_outside_catalog(&live)?;
    let lock_path = live.join(STATE_ROOT_LEASE_FILE_NAME);
    let lock_before = fs::read(&lock_path)?;
    require_cli_refusal(
        &binary,
        "verify-state",
        &live,
        None,
        SearchPlaneErrorCodeV2::StateRootInUse,
    )?;
    if tree_names_outside_catalog(&live)? != names_before || fs::read(lock_path)? != lock_before {
        return Err("refused old-Q verify changed the live root namespace or lease bytes".into());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_non_private_source_root_is_refused() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    fs::set_permissions(source.path(), fs::Permissions::from_mode(0o755))?;
    let destination = parent.path().join("frozen");
    let binary = LegacySearchdBinary::from_environment()?;
    let refusal = require_cli_refusal(
        &binary,
        "backup-state",
        source.path(),
        Some(&destination),
        SearchPlaneErrorCodeV2::StateRootInsecure,
    );
    fs::set_permissions(source.path(), fs::Permissions::from_mode(0o700))?;
    refusal?;
    if destination.exists() || staging_directory_for_v1(&destination).exists() {
        return Err("non-private source refusal published backup or staging".into());
    }
    Ok(())
}

#[test]
fn a_live_lease_owner_excludes_an_offline_operation() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let _lease = StateRootLease::acquire(source.path())?;
    let names_before = tree_names_outside_catalog(source.path())?;
    let lock = source.path().join(STATE_ROOT_LEASE_FILE_NAME);
    let lock_before = fs::read(&lock)?;
    let destination = parent.path().join("frozen");
    let binary = LegacySearchdBinary::from_environment()?;
    require_cli_refusal(
        &binary,
        "backup-state",
        source.path(),
        Some(&destination),
        SearchPlaneErrorCodeV2::StateRootInUse,
    )?;
    if destination.exists()
        || staging_directory_for_v1(&destination).exists()
        || tree_names_outside_catalog(source.path())? != names_before
        || fs::read(&lock)? != lock_before
    {
        return Err("live-lease refusal changed source, lock, or destination".into());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn alias_symlink_and_hardlink_sources_are_refused() -> TestResult {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let binary = LegacySearchdBinary::from_environment()?;
    let source = private_root()?;
    build_live_root(source.path())?;
    drop(StateRootLease::acquire(source.path())?);
    let names_before = tree_names_outside_catalog(source.path())?;
    let nested = source.path().join("nested-destination");
    require_cli_refusal(
        &binary,
        "backup-state",
        source.path(),
        Some(&nested),
        SearchPlaneErrorCodeV2::InvalidRequest,
    )?;
    if tree_names_outside_catalog(source.path())? != names_before
        || staging_directory_for_v1(&nested).exists()
    {
        return Err("nested destination refusal mutated the source".into());
    }

    let outer = private_root()?;
    let real = outer.path().join("real");
    fs::create_dir(&real)?;
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700))?;
    build_live_root(&real)?;
    drop(StateRootLease::acquire(&real)?);
    let alias_names = tree_names_outside_catalog(&real)?;
    let link = outer.path().join("link");
    symlink(&real, &link)?;
    let aliased = link.join("aliased-destination");
    require_cli_refusal(
        &binary,
        "backup-state",
        &real,
        Some(&aliased),
        SearchPlaneErrorCodeV2::InvalidRequest,
    )?;
    if tree_names_outside_catalog(&real)? != alias_names
        || staging_directory_for_v1(&aliased).exists()
    {
        return Err("symlinked destination refusal mutated the source".into());
    }

    let hard = private_root()?;
    let hard_parent = private_root()?;
    build_live_root(hard.path())?;
    let object = hard.path().join("authorities/history.cbor");
    let alias = hard.path().join("authorities/history-alias.cbor");
    fs::hard_link(&object, &alias)?;
    let bytes_before = fs::read(&object)?;
    let destination = hard_parent.path().join("frozen");
    require_cli_refusal(
        &binary,
        "backup-state",
        hard.path(),
        Some(&destination),
        SearchPlaneErrorCodeV2::StateRootInsecure,
    )?;
    if destination.exists()
        || staging_directory_for_v1(&destination).exists()
        || fs::read(&object)? != bytes_before
        || fs::read(&alias)? != bytes_before
    {
        return Err("hard-linked source refusal mutated an object or target".into());
    }
    Ok(())
}

#[test]
fn existing_staging_file_and_directory_are_preserved() -> TestResult {
    let source = private_root()?;
    let parent = private_root()?;
    build_live_root(source.path())?;
    let binary = LegacySearchdBinary::from_environment()?;

    let directory_destination = parent.path().join("directory-victim");
    let directory_staging = staging_directory_for_v1(&directory_destination);
    fs::create_dir(&directory_staging)?;
    let sentinel = directory_staging.join("unrelated-sentinel");
    fs::write(&sentinel, b"leave this directory intact")?;
    require_cli_refusal(
        &binary,
        "backup-state",
        source.path(),
        Some(&directory_destination),
        SearchPlaneErrorCodeV2::InvalidRequest,
    )?;
    if fs::read(&sentinel)? != b"leave this directory intact" || directory_destination.exists() {
        return Err("staging directory refusal changed sentinel or published target".into());
    }

    let file_destination = parent.path().join("file-victim");
    let file_staging = staging_directory_for_v1(&file_destination);
    fs::write(&file_staging, b"leave this file intact")?;
    require_cli_refusal(
        &binary,
        "backup-state",
        source.path(),
        Some(&file_destination),
        SearchPlaneErrorCodeV2::InvalidRequest,
    )?;
    if fs::read(&file_staging)? != b"leave this file intact" || file_destination.exists() {
        return Err("staging file refusal changed bytes or published target".into());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn existing_staging_symlinks_preserve_external_and_source_bytes() -> TestResult {
    use std::os::unix::fs::symlink;

    let source = private_root()?;
    let parent = private_root()?;
    let external = private_root()?;
    build_live_root(source.path())?;
    drop(StateRootLease::acquire(source.path())?);
    let source_names_before = tree_names_outside_catalog(source.path())?;
    let source_object = source.path().join("authorities/history.cbor");
    let source_before = fs::read(&source_object)?;
    let sentinel = external.path().join("unrelated-sentinel");
    fs::write(&sentinel, b"leave external bytes intact")?;
    let dangling = parent.path().join("absent");
    let binary = LegacySearchdBinary::from_environment()?;

    for (name, target) in [
        ("external-link", external.path()),
        ("source-link", source.path()),
        ("dangling-link", dangling.as_path()),
    ] {
        let destination = parent.path().join(name);
        let staging = staging_directory_for_v1(&destination);
        symlink(target, &staging)?;
        require_cli_refusal(
            &binary,
            "backup-state",
            source.path(),
            Some(&destination),
            SearchPlaneErrorCodeV2::InvalidRequest,
        )?;
        if fs::read_link(&staging)?.as_path() != target
            || destination.exists()
            || fs::read(&source_object)? != source_before
            || tree_names_outside_catalog(source.path())? != source_names_before
            || fs::read(&sentinel)? != b"leave external bytes intact"
            || fs::read_dir(external.path())?.count() != 1
        {
            return Err(
                format!("staging symlink {name} refusal mutated a source or target").into(),
            );
        }
    }
    Ok(())
}

#[test]
fn restore_refuses_backup_inventory_changed_before_custody() -> TestResult {
    let binary = LegacySearchdBinary::from_environment()?;
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
        let _receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
        match mutation {
            "added-file" => fs::write(backup.join("unadvertised-note"), b"extra")?,
            "changed-file" => fs::write(backup.join("retained-note"), b"changed backup bytes")?,
            "missing-file" => fs::remove_file(backup.join("retained-note"))?,
            "added-directory" => fs::create_dir(backup.join("unadvertised-empty"))?,
            "missing-directory" => fs::remove_dir(backup.join("retained-empty"))?,
            _ => return Err("invalid closed mutation fixture".into()),
        }
        let restored = restore_parent.path().join("restored-root");
        let output = run_cli(&binary, "restore-state", &backup, Some(&restored))?;
        require_cli_refusal_output(
            &output,
            "restore-state",
            &[
                SearchPlaneErrorCodeV2::NotFound,
                SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
            ],
        )?;
        if restored.exists() || staging_directory_for_v1(&restored).exists() {
            return Err(
                format!("{mutation}: old-Q restore did not refuse before target staging").into(),
            );
        }
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
    let binary = LegacySearchdBinary::from_environment()?;
    let _receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let path = backup.join(STATE_BACKUP_MANIFEST_FILE_NAME);
    let mut manifest = read_root_manifest_v1(&path)?;
    manifest.catalog_rows = manifest
        .catalog_rows
        .checked_add(1)
        .ok_or("fixture row count exhausted")?;
    fs::write(&path, manifest.encode())?;
    let restored = restore_parent.path().join("restored");
    require_cli_refusal(
        &binary,
        "restore-state",
        &backup,
        Some(&restored),
        SearchPlaneErrorCodeV2::CatalogRowCorrupt,
    )?;
    if restored.exists() || staging_directory_for_v1(&restored).exists() {
        return Err("wrong catalog row count published a restore target".into());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn ambiguous_dangling_or_directory_authority_refuses_verify_and_restore() -> TestResult {
    let binary = LegacySearchdBinary::from_environment()?;
    for mutation in ["dangling", "directory"] {
        let source = private_root()?;
        let backup_parent = private_root()?;
        let restore_parent = private_root()?;
        build_live_root(source.path())?;
        let backup = backup_parent.path().join("backup-root");
        let _receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
        let alternate = backup.join(STATE_ROOT_MANIFEST_FILE_NAME);
        match mutation {
            "dangling" => std::os::unix::fs::symlink("absent-authority", &alternate)?,
            "directory" => fs::create_dir(&alternate)?,
            _ => return Err("invalid closed mutation fixture".into()),
        }
        for operation in ["verify-state", "restore-state"] {
            let destination = restore_parent.path().join("restored");
            require_cli_refusal(
                &binary,
                operation,
                &backup,
                (operation == "restore-state").then_some(destination.as_path()),
                SearchPlaneErrorCodeV2::InvalidRequest,
            )?;
            if destination.exists() || staging_directory_for_v1(&destination).exists() {
                return Err(format!("{mutation}: {operation} published a target").into());
            }
        }
    }
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
    let binary = LegacySearchdBinary::from_environment()?;
    let _receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let manifest_path = backup.join("state-backup-manifest-v1.txt");
    let manifest = read_root_manifest_v1(&manifest_path)?;
    let typed_code = |error: &CoreError| match error {
        CoreError::Typed { code, .. } => Some(*code),
        CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)
        | CoreError::StorageIo { .. } => None,
    };

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
    let fifo_status = std::process::Command::new("mkfifo").arg(&fifo).status()?;
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
    binary.verify()?;
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
    drop(StateRootLease::acquire(source.path())?);
    let source_names = tree_names_outside_catalog(source.path())?;
    let backup = parent.path().join("backup");
    let binary = LegacySearchdBinary::from_environment()?;
    let _receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let manifest = read_root_manifest_v1(&backup.join(STATE_BACKUP_MANIFEST_FILE_NAME))?;
    for path in paths {
        if !manifest
            .objects
            .iter()
            .any(|entry| entry.relative_path == path)
            || fs::read(backup.join(path))? != path.as_bytes()
        {
            return Err(format!("old-Q omitted or changed source sidecar {path}").into());
        }
    }
    let _verified = run_cli_success(&binary, "verify-state", &backup, None)?;
    if tree_names_outside_catalog(source.path())? != source_names {
        return Err("old-Q backup changed source sidecar namespace".into());
    }
    Ok(())
}

fn require_corpus_hit(
    answer: &TextQueryResponse,
    pin: &GenerationPin,
    path: &str,
    candidate_id: &str,
    source_sha256: [u8; 32],
) -> TestResult {
    let [row] = answer.results.as_slice() else {
        return Err(format!(
            "expected one row for {candidate_id}, got {:?}",
            answer.results
        )
        .into());
    };
    if answer.generation != *pin
        || row.candidate_id.as_str() != candidate_id
        || row.repo_relative_path.as_str() != path
        || row.source.as_ref().map(|source| source.source_sha256) != Some(source_sha256)
    {
        return Err(
            format!("unexpected generation or source row for {candidate_id}: {answer:?}").into(),
        );
    }
    Ok(())
}

fn require_migration_queries(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
    let source_sha256 = Sha256::digest(MIGRATION_SOURCE).into();
    for (query, candidate_id) in [
        (
            client.lexical().query().native(MIGRATION_NEEDLE),
            MIGRATION_CHUNK_ID,
        ),
        (
            client.lexical().query().code_search(MIGRATION_NEEDLE),
            MIGRATION_FILE_ID,
        ),
    ] {
        let answer = query.pinned(pin.clone()).top_k(10).execute()?;
        require_corpus_hit(&answer, pin, MIGRATION_PATH, candidate_id, source_sha256)?;
    }
    let ownership = request_when_ready(|| {
        client
            .lexical()
            .query()
            .sourcegraph(format!("select:file.owners {MIGRATION_NEEDLE}"))
            .pinned(pin.clone())
            .top_k(10)
            .execute()
    })?;
    require_corpus_hit(
        &ownership,
        pin,
        MIGRATION_PATH,
        MIGRATION_CHUNK_ID,
        source_sha256,
    )?;
    let Some([row]) = ownership.file_owner_rows.as_deref() else {
        return Err(format!(
            "expected one ownership row, got {:?}",
            ownership.file_owner_rows
        )
        .into());
    };
    if row.repo_relative_path.as_str() != MIGRATION_PATH || row.owners != ["@migration-owner"] {
        return Err(format!("restored ownership differs: {row:?}").into());
    }
    Ok(())
}

fn with_post_restore_file(
    batch: SearchCorpusBatch,
    pin: &GenerationPin,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let path = RepoRelativePath::new(RESTORED_PATH);
    let text = format!("pub fn {RESTORED_NEEDLE}() -> bool {{ true }}");
    let scope = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id: pin.repo_id.clone(),
            repo_relative_path: path.clone(),
        },
        pin.revision_id.clone(),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(RESTORED_CHUNK_ID),
            repo_relative_path: path,
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(text.len())?,
            start_line: 1,
            end_line: 1,
            text: text.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(pin.repo_id.clone()),
        }],
        Vec::new(),
    )?;
    let semantic_scopes = semantic_source_scopes_for_chunk_records(&pin.repo_id, &scope.chunks);
    let mut batch = batch.replace_scope(
        scope.coverage,
        scope.source_bytes,
        scope.chunks,
        scope.symbols,
    );
    for scope in semantic_scopes {
        batch = batch.replace_semantic_scope(
            scope.scope,
            scope.scope_digest,
            scope.sources,
            scope.cluster_memberships,
        );
    }
    Ok(batch)
}

#[test]
fn real_search_corpus_backup_restore_preserves_file_and_native_queries() -> TestResult {
    let source = private_root()?;
    let backup_parent = private_root()?;
    let restore_parent = private_root()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let g1 = ManifestGeneration::new(1);
    let pin = GenerationPin::new(repo.clone(), revision.clone(), g1);
    let initial = legacy_source_batch(
        &repo,
        &revision,
        g1.get(),
        None,
        &[
            (MIGRATION_PATH, MIGRATION_BODY, 1),
            ("src/decoy.rs", "pub fn unrelated() {}\n", 2),
        ],
    )?;
    let producer = binary.start(source.path(), 8)?;
    let client = producer.connect()?;
    let request = initial.generation_reservation_request()?;
    let reservation = client.search_corpus().reserve_generation(&request)?;
    if reservation.generation != g1 || reservation.source_event != request.source_event {
        return Err(format!("unexpected G1 Source reservation: {reservation:?}").into());
    }
    let ownership = client.history().publish_file_ownership(
        &FileOwnershipBatch::new(repo.clone(), revision.clone(), g1).entry(
            repo.clone(),
            RepoRelativePath::new(MIGRATION_PATH),
            vec!["@migration-owner".into()],
        ),
    )?;
    if ownership.generation != g1 || !ownership.applied || ownership.durable_sequence == 0 {
        return Err(format!("G1 ownership was not durable: {ownership:?}").into());
    }
    let (g1_head, receipt) = publish_legacy_source_batch(&client, &initial, None)?;
    if !receipt.sealed
        || receipt.accepted_replace_scopes != 2
        || receipt.accepted_semantic_replace_scopes != 2
    {
        return Err(format!("G1 did not seal both source files: {receipt:?}").into());
    }
    require_migration_queries(&client, &pin)?;
    drop(client);
    producer.stop()?;

    let source_incarnation = fs::read(source.path().join(INCARNATION))?;
    if source_incarnation.as_slice() != reservation.root_incarnation.as_slice() {
        return Err("G1 Source allocation did not use the original incarnation".into());
    }
    // Freeze the independent disk oracle before backup opens the source.
    let mut observed_kinds = BTreeSet::new();
    let mut pooled_bodies = Vec::new();
    for relative in tree_names_outside_catalog(source.path())? {
        if !relative.starts_with("indexes/lexical/lexical-object-pool/objects/") {
            continue;
        }
        let object = source.path().join(&relative);
        if !fs::symlink_metadata(&object)?.file_type().is_file() {
            continue;
        }
        let bytes = fs::read(&object)?;
        if bytes.get(..8) == Some(b"QLSTOR01".as_slice())
            && let Some(kind @ (1 | 3)) = bytes.get(10).copied()
        {
            let _present = observed_kinds.insert(kind);
            pooled_bodies.push((relative, bytes));
        }
    }
    if observed_kinds != BTreeSet::from([1, 3]) {
        return Err(format!(
            "expected physical File and Native bodies from G1, got {observed_kinds:?}"
        )
        .into());
    }
    let backup = backup_parent.path().join("real-corpus-backup");
    let _receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
    let backup_manifest = read_root_manifest_v1(&backup.join(STATE_BACKUP_MANIFEST_FILE_NAME))?;
    for (relative, bytes) in &pooled_bodies {
        let size = u64::try_from(bytes.len())?;
        let digest = format!("{:x}", Sha256::digest(bytes));
        if !backup_manifest.objects.iter().any(|entry| {
            entry.relative_path == *relative
                && entry.byte_size == size
                && entry.digest_hex == digest
        }) || fs::read(backup.join(relative))?.as_slice() != bytes.as_slice()
        {
            return Err(format!("backup did not bind an exact copy of {relative}").into());
        }
        if fs::read(source.path().join(relative))?.as_slice() != bytes.as_slice() {
            return Err(format!("backup changed source pooled body {relative}").into());
        }
    }
    let restored = restore_parent.path().join("restored-real-corpus");
    let _receipt = run_cli_success(&binary, "restore-state", &backup, Some(&restored))?;
    let restored_incarnation = fs::read(restored.join(INCARNATION))?;
    if restored_incarnation == source_incarnation {
        return Err("restore reused the original allocation incarnation".into());
    }
    for (relative, bytes) in pooled_bodies {
        if fs::read(restored.join(&relative))? != bytes {
            return Err(format!("restore changed the pooled body {relative}").into());
        }
    }
    let reopened = binary.start(&restored, 8)?;
    let client = reopened.connect()?;
    require_migration_queries(&client, &pin)?;
    let restored_g1_head = client
        .generations()
        .active_head(repo.clone(), revision.clone())?
        .ok_or("restored root has no active G1 head")?;
    if restored_g1_head.generation != g1_head.generation
        || restored_g1_head.activation_token.activation_sequence()
            != g1_head.activation_token.activation_sequence()
        || restored_g1_head
            .activation_token
            .root_incarnation()
            .as_slice()
            != restored_incarnation.as_slice()
    {
        return Err(format!(
            "restored G1 head did not preserve identity and rotate token: {restored_g1_head:?}"
        )
        .into());
    }

    // Keep G1 as the head of its own Source stream so G3 can name that
    // real parent even after the active corpus advances to the G2 FULL.
    let g2 = ManifestGeneration::new(2);
    let full = with_post_restore_file(
        SearchCorpusBatch::replace_generation(
            repo.clone(),
            revision.clone(),
            g2,
            "post-restore-g2",
        )
        .source_event(SourcePublicationEvent {
            stream_id: "post-restore-full-bootstrap".into(),
            event_id: "post-restore-full-bootstrap-g2".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        }),
        &pin,
    )?;
    let full_reservation = client
        .search_corpus()
        .reserve_generation(&full.generation_reservation_request()?)?;
    if full_reservation.generation != g2
        || full_reservation.root_incarnation.as_slice() != restored_incarnation.as_slice()
    {
        return Err(format!(
            "G2 did not allocate in the restored incarnation: {full_reservation:?}"
        )
        .into());
    }
    let (g2_head, receipt) = publish_legacy_source_batch(&client, &full, Some(restored_g1_head))?;
    if !receipt.sealed
        || receipt.accepted_replace_scopes != 1
        || receipt.accepted_semantic_replace_scopes != 1
    {
        return Err(format!("G2 did not seal the full replacement: {receipt:?}").into());
    }
    let g2_pin = GenerationPin::new(repo.clone(), revision.clone(), g2);
    let expected_hash =
        Sha256::digest(b"pub fn restored_fresh_needle_89() -> bool { true }\n").into();
    for (fresh_query, old_query, candidate_id) in [
        (
            client.lexical().query().native(RESTORED_NEEDLE),
            client.lexical().query().native(MIGRATION_NEEDLE),
            RESTORED_CHUNK_ID,
        ),
        (
            client.lexical().query().code_search(RESTORED_NEEDLE),
            client.lexical().query().code_search(MIGRATION_NEEDLE),
            RESTORED_FILE_ID,
        ),
    ] {
        let answer = fresh_query.pinned(g2_pin.clone()).top_k(10).execute()?;
        require_corpus_hit(&answer, &g2_pin, RESTORED_PATH, candidate_id, expected_hash)?;
        let old_content = old_query.pinned(g2_pin.clone()).top_k(10).execute()?;
        if old_content.generation != g2_pin || !old_content.results.is_empty() {
            return Err(format!("G2 FULL was not isolated from G1: {old_content:?}").into());
        }
    }
    require_migration_queries(&client, &pin)?;

    let g3 = ManifestGeneration::new(3);
    let delta = with_post_restore_file(
        SearchCorpusBatch::delta(repo.clone(), revision.clone(), g3, g1, "post-restore-g3")
            .source_event(SourcePublicationEvent {
                stream_id: reservation.source_event.stream_id,
                event_id: "post-restore-old-base-delta-g3".into(),
                expected_base_event_id: Some(reservation.source_event.event_id),
                payload_sha256: [0; 32],
            }),
        &pin,
    )?;
    let refusal = client.search_corpus().publish(&delta);
    if !matches!(
        &refusal,
        Err(SdkError::Remote { code: SearchPlaneErrorCodeV2::InvalidRequest, message, .. })
            if message.contains("allocation scope")
    ) {
        return Err(format!("expected the G3 allocation-scope refusal, got {refusal:?}").into());
    }
    let g3_path = GenerationStorageKeyV1::for_repo_revision(&repo, &revision)
        .generation_dir(&restored.join("indexes/lexical"), g3);
    if g3_path.try_exists()?
        || client
            .generations()
            .active_head(repo.clone(), revision.clone())?
            != Some(g2_head)
    {
        return Err("G3 staged a lexical generation or changed the G2 active head".into());
    }
    // The pair must also be absent from retained serving authority. An
    // unchanged active head alone would allow an inactive G3 to slip through.
    let g3_pin = GenerationPin::new(repo, revision, g3);
    let lexical = client
        .lexical()
        .query()
        .native(RESTORED_NEEDLE)
        .pinned(g3_pin.clone())
        .top_k(10)
        .execute();
    let semantic = client
        .semantic()
        .query()
        .text(RESTORED_NEEDLE)
        .pinned(g3_pin)
        .top_k(10)
        .execute();
    if !matches!(
        &lexical,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::NotReady,
            message,
            ..
        }) if message.contains("requested Lexical generation 3 but materialized only up to 2")
    ) || !matches!(
        &semantic,
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::SemanticGenerationNotMaterialized,
            message,
            ..
        }) if message.contains("semantic generation 3 is not materialized")
    ) {
        return Err(format!(
            "G3 serving status differs after refusal: lexical={lexical:?}, semantic={semantic:?}"
        )
        .into());
    }
    drop(client);
    reopened.stop()?;
    binary.verify()
}

#[test]
fn real_search_corpus_backup_refuses_missing_or_corrupt_pooled_native_body() -> TestResult {
    let source = private_root()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let producer = binary.start(source.path(), 8)?;
    let client = producer.connect()?;
    let (_head, receipt) = publish_legacy_source(
        &client,
        &repo,
        &revision,
        1,
        None,
        &[
            (MIGRATION_PATH, MIGRATION_BODY, 1),
            ("src/decoy.rs", "pub fn unrelated() {}\n", 2),
        ],
        None,
    )?;
    if receipt.accepted_replace_scopes != 2 || receipt.accepted_semantic_replace_scopes != 2 {
        return Err("old-Q did not publish both original source files".into());
    }
    let pin = GenerationPin::new(repo, revision, ManifestGeneration::new(1));
    let answer = wait_for(
        &RealTicker::new(),
        Duration::from_secs(20),
        Duration::from_millis(50),
        "old-Q query over the original source generation",
        || {
            client
                .lexical()
                .query()
                .native(MIGRATION_NEEDLE)
                .pinned(pin.clone())
                .top_k(10)
                .execute()
        },
        |_| true,
        |error| {
            matches!(
                error,
                SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::NotReady,
                    ..
                }
            )
        },
    )?;
    let expected_hash = Sha256::digest(MIGRATION_SOURCE).into();
    require_corpus_hit(
        &answer,
        &pin,
        MIGRATION_PATH,
        MIGRATION_CHUNK_ID,
        expected_hash,
    )?;
    drop(client);
    producer.stop()?;
    binary.verify()?;

    let body = pooled_object_of_kind(source.path(), 3)?;
    let original = fs::read(&body)?;
    for missing in [true, false] {
        let destination_parent = private_root()?;
        if missing {
            fs::remove_file(&body)?;
        } else {
            let mut bytes = original.clone();
            let last = bytes.last_mut().ok_or("empty pooled Native body")?;
            *last ^= 1;
            fs::write(&body, bytes)?;
        }
        let backup = destination_parent.path().join("refused-backup");
        require_cli_refusal(
            &binary,
            "backup-state",
            source.path(),
            Some(&backup),
            SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        )?;
        if backup.exists() {
            return Err(format!("missing={missing}: invalid pooled body published backup").into());
        }
        let staging = staging_directory_for_v1(&backup);
        if staging.join(STATE_BACKUP_MANIFEST_FILE_NAME).exists() {
            return Err(format!("missing={missing}: failed backup published a manifest").into());
        }
        fs::write(&body, &original)?;
        require_cli_refusal(
            &binary,
            "backup-state",
            source.path(),
            Some(&backup),
            SearchPlaneErrorCodeV2::InvalidRequest,
        )?;
        if !staging.is_dir() {
            return Err("retry removed the interrupted staging entry".into());
        }
        fs::remove_dir_all(&staging)?;
        let _receipt = run_cli_success(&binary, "backup-state", source.path(), Some(&backup))?;
        if staging.exists() || !backup.join(STATE_BACKUP_MANIFEST_FILE_NAME).is_file() {
            return Err("explicit cleanup did not permit a complete backup cutover".into());
        }
    }
    binary.verify()
}
