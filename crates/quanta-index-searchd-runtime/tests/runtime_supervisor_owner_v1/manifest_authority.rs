//! NoFeatures manifest decode and write authority oracles.
//! The tests use disposable roots and do not construct a search engine.

#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "the fixed manifest fixtures assert independent decode and refusal invariants"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use quanta_index_catalog::{SqliteCatalog, live_catalog_receipt, verify_snapshot};
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::state_migration::OfflineSourceSessionV1;
use quanta_index_searchd_harness::private_tempdir as private_root;

#[path = "../common/offline_legacy_root.rs"]
mod offline_legacy_root;
use offline_legacy_root::build_legacy_root;

#[path = "../common/offline_target_guard.rs"]
mod offline_target_guard;
use offline_target_guard::UnreachedTargetFactoryV1;

use super::offline_live_root::{BUSY, build_live_root};
use super::offline_state_fixture::current_session;
use quanta_index_core::CoreError;
#[cfg(unix)]
use quanta_index_searchd::app::state_format::LEGACY_AUXILIARY_SNAPSHOT_RELATIVES;
use quanta_index_searchd::app::state_format::{
    NoStateMigrationFaultsV1, OfflineRootRoleV1, STATE_ROOT_MANIFEST_FILE_NAME, StateRootFormatV1,
    StateRootManifestV1, atomic_cutover_v1, detect_state_root_format_v1, inventory_state_root_v1,
    refuse_broad_offline_target_v1, refuse_legacy_state_root_v1, staging_directory_for_v1,
    write_root_manifest_last_v1,
};
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn migrate_state_cli_is_rejected_before_opening_a_root() -> TestResult {
    let source = quanta_index_searchd_harness::private_tempdir()?;
    let parent = quanta_index_searchd_harness::private_tempdir()?;
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

fn typed_code(error: &CoreError) -> Option<SearchPlaneErrorCodeV2> {
    match error {
        CoreError::Typed { code, .. } => Some(*code),
        CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)
        | CoreError::StorageIo { .. } => None,
    }
}

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
    let parent = quanta_index_searchd_harness::private_tempdir()?;
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
        if !matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::InvalidRequest,
                ..
            }
        ) {
            return Err(format!("manifest path refusal had wrong typed error: {error:?}").into());
        }
    }
    assert!(!outside.exists());
    assert_eq!(
        fs::read_dir(&root)?.count(),
        0,
        "refusal must leave no files"
    );
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
    assert!(matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            ..
        }
    ));
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let error = write_root_manifest_last_v1(
        root.path(),
        STATE_ROOT_MANIFEST_FILE_NAME,
        &manifest,
        &NoStateMigrationFaultsV1,
    )
    .expect_err("an old-format manifest must not be written");
    assert!(matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            ..
        }
    ));
    assert!(!root.path().join(STATE_ROOT_MANIFEST_FILE_NAME).exists());
    Ok(())
}

#[test]
fn old_migration_receipts_and_dangling_journals_refuse_mixed_roots() -> TestResult {
    let root = quanta_index_searchd_harness::private_tempdir()?;
    fs::create_dir_all(root.path().join("catalog"))?;
    fs::write(root.path().join("state-migration-receipt-v1.txt"), b"old")?;
    assert_eq!(
        detect_state_root_format_v1(root.path())?,
        StateRootFormatV1::LegacyV1
    );
    let error = refuse_legacy_state_root_v1(root.path()).expect_err("old receipt must refuse");
    assert!(matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            ..
        }
    ));
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

#[cfg(unix)]
#[test]
fn dangling_legacy_auxiliary_snapshot_is_not_classified_as_current() -> TestResult {
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let snapshot = root.path().join(LEGACY_AUXILIARY_SNAPSHOT_RELATIVES[0]);
    fs::create_dir_all(snapshot.parent().ok_or("snapshot has no parent")?)?;
    std::os::unix::fs::symlink("missing-snapshot", &snapshot)?;
    assert_eq!(
        detect_state_root_format_v1(root.path())?,
        StateRootFormatV1::LegacyV1
    );
    let error = refuse_legacy_state_root_v1(root.path()).expect_err("boot must refuse");
    assert!(matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            ..
        }
    ));
    Ok(())
}

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
    let real = quanta_index_searchd_harness::private_tempdir()?;
    let alias_parent = quanta_index_searchd_harness::private_tempdir()?;
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

#[test]
fn atomic_cutover_never_replaces_an_existing_root() -> TestResult {
    let destination_parent = private_root()?;
    let staging = destination_parent.path().join("staging");
    fs::create_dir(&staging)?;
    let destination = destination_parent.path().join("occupied");
    fs::create_dir_all(&destination)?;
    fs::write(destination.join("pre-existing"), b"do not clobber")?;
    let error = atomic_cutover_v1(&staging, &destination)
        .expect_err("a cutover must refuse an existing destination");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    assert_eq!(
        fs::read(destination.join("pre-existing"))?,
        b"do not clobber"
    );
    assert!(staging.is_dir(), "refusal preserves the original staging");
    Ok(())
}

#[test]
fn atomic_cutover_preserves_existing_destination_symlink_and_its_target() -> TestResult {
    use std::os::unix::fs::symlink;

    let parent = private_root()?;
    let staging = parent.path().join("staging");
    fs::create_dir(&staging)?;
    fs::write(staging.join("new-data"), b"new")?;
    let foreign = parent.path().join("foreign");
    fs::create_dir(&foreign)?;
    fs::write(foreign.join("old-data"), b"old")?;
    let destination = parent.path().join("destination");
    symlink(&foreign, &destination)?;
    let error = atomic_cutover_v1(&staging, &destination)
        .expect_err("an existing symlink is never replaced");
    assert_eq!(
        typed_code(&error),
        Some(SearchPlaneErrorCodeV2::InvalidRequest)
    );
    assert_eq!(fs::read_link(&destination)?, foreign);
    assert_eq!(fs::read(foreign.join("old-data"))?, b"old");
    assert_eq!(fs::read(staging.join("new-data"))?, b"new");
    Ok(())
}

#[test]
fn atomic_cutover_rejects_a_symlink_staging_without_publishing() -> TestResult {
    use std::os::unix::fs::symlink;

    let parent = private_root()?;
    let foreign = parent.path().join("foreign");
    fs::create_dir(&foreign)?;
    fs::write(foreign.join("old-data"), b"old")?;
    let staging = parent.path().join("staging");
    let destination = parent.path().join("destination");
    symlink(&foreign, &staging)?;
    let _error = atomic_cutover_v1(&staging, &destination)
        .expect_err("staging must be a held directory rather than a symlink");
    assert!(!destination.exists());
    assert_eq!(fs::read_link(&staging)?, foreign);
    assert_eq!(fs::read(foreign.join("old-data"))?, b"old");
    Ok(())
}

#[test]
fn atomic_cutover_publishes_the_original_staging_inode() -> TestResult {
    use std::os::unix::fs::MetadataExt as _;

    let parent = private_root()?;
    let staging = parent.path().join("staging");
    let destination = parent.path().join("destination");
    fs::create_dir(&staging)?;
    fs::write(staging.join("data"), b"fixed-data")?;
    let before = fs::metadata(&staging)?;
    atomic_cutover_v1(&staging, &destination)?;
    let after = fs::metadata(&destination)?;
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert!(!staging.exists());
    assert_eq!(fs::read(destination.join("data"))?, b"fixed-data");
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
        &UnreachedTargetFactoryV1,
        &NoStateMigrationFaultsV1,
        &quanta_index_core::RequestBudgetV1::unbounded(),
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
