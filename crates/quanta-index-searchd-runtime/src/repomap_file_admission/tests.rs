use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quanta_index_contract::{CandidateObjectDigestV1, QuarantinePayloadDigestV1};
use quanta_index_core::publication_admission::{
    PublicationOutputClass, PublicationOutputReservation,
};
use quanta_index_core::publication_capacity::PublicationCapacity;
use quanta_index_core::{
    CoreError, RepoMapImmutablePutOutcome, RepoMapPhysicalFileClass, RepoMapPhysicalFilePort,
    RequestBudgetV1,
};
use quanta_index_repomap::{CandidateObjectAddressV1, QuarantinePayloadAddressV1};
use quanta_index_searchd::app::runtime::StateRootLease;
use rustix::fs::{Mode, OFlags, openat};

use super::CanonicalRepoMapFileAdmission;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
use crate::publication_sidecar::recoverable_sidecar_output_permit;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn require(condition: bool, message: &'static str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn require_eq<T: std::fmt::Debug + PartialEq>(actual: &T, expected: &T) -> io::Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "actual={actual:?}, expected={expected:?}"
        )))
    }
}

type Fixture = (
    tempfile::TempDir,
    PathBuf,
    PublicationCapacity,
    Arc<PublicationFileCapacity>,
    CanonicalRepoMapFileAdmission,
);

type SeededFixture = (
    tempfile::TempDir,
    PathBuf,
    PublicationCapacity,
    Arc<PublicationFileCapacity>,
    Arc<PublicationFileDeletions>,
    StateRootLease,
);

fn seeded_fixture(staged: &[(&str, &[u8])]) -> Result<SeededFixture, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let requested_root = directory.path().join("state");
    fs::create_dir(&requested_root)?;
    fs::set_permissions(&requested_root, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested_root)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    for (name, bytes) in staged {
        let path = root.join(name);
        fs::write(&path, bytes)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 1024,
            max_files: 32,
            max_pending: 32,
            max_entries: 64,
            max_depth: 12,
            max_pending_frames: 16,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let binder = PublicationFileCapacity::new(lease.clone(), authority.clone(), 16, 64, 256)?;
    let deletions = Arc::new(PublicationFileDeletions::new(Arc::clone(&binder)));
    Ok((directory, root, authority, binder, deletions, lease))
}

fn fixture(control_max: u64, candidate_max: u64) -> Result<Fixture, Box<dyn std::error::Error>> {
    let (directory, root, authority, binder, deletions, lease) = seeded_fixture(&[])?;
    let admission = CanonicalRepoMapFileAdmission::new(
        deletions,
        lease.try_clone_root_directory_v1()?,
        control_max,
        candidate_max,
        64, // Same explicit boot-entry ceiling as this fixture's inventory.
    )?;
    Ok((directory, root, authority, binder, admission))
}

fn create_projection_parents(
    admission: &CanonicalRepoMapFileAdmission,
    root: &Path,
    path: &Path,
) -> TestResult {
    let layout = root.join("repo-map");
    admission.validate_root_and_reconcile(&layout)?;
    admission.ensure_private_directory(&layout)?;
    let relative = path
        .parent()
        .ok_or_else(|| io::Error::other("projection has no parent"))?
        .strip_prefix(&layout)?;
    let mut directory = layout;
    for component in relative.components() {
        directory.push(component.as_os_str());
        admission.ensure_private_directory(&directory)?;
    }
    Ok(())
}

fn candidate(root: &Path) -> PathBuf {
    root.join("repo-map").join(
        CandidateObjectAddressV1::new(CandidateObjectDigestV1::from_bytes([0xab; 32]))
            .relative_path(),
    )
}

fn physical_inventory(root: &Path) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let mut directories = vec![root.to_path_buf()];
    let mut seen = BTreeSet::new();
    let mut bytes = 0_u64;
    let mut files = 0_u64;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                directories.push(entry.path());
            } else if metadata.is_file() && seen.insert((metadata.dev(), metadata.ino())) {
                bytes = bytes.checked_add(metadata.len()).ok_or("byte overflow")?;
                files = files.checked_add(1).ok_or("file overflow")?;
            }
        }
    }
    Ok((bytes, files))
}

#[test]
fn root_uuid_requires_its_control_bound_before_create_and_reopens_under_pin() -> TestResult {
    let (_directory, root, authority, binder, too_small) = fixture(15, 64)?;
    let layout = root.join("repo-map");
    let uuid = layout.join("root-uuid.bin");
    let before = authority.snapshot()?;
    require(
        matches!(
            too_small.put_immutable(
                &uuid,
                b"0123456789abcdef",
                RepoMapPhysicalFileClass::RootUuid
            ),
            Err(CoreError::Typed { .. })
        ),
        "15-byte Control bound accepted 16-byte root UUID",
    )?;
    require(!uuid.exists(), "refusal created root UUID")?;
    require_eq(&(authority.snapshot()?), &(before))?;
    require(binder.is_quiescent()?, "UUID refusal retained a mutation")?;

    let (_directory, root, authority, binder, admitted) = fixture(16, 64)?;
    let layout = root.join("repo-map");
    let uuid = layout.join("root-uuid.bin");
    admitted.ensure_private_directory(&layout)?;
    let before = authority.snapshot()?;
    require_eq(
        &(admitted.put_immutable(
            &uuid,
            b"0123456789abcdef",
            RepoMapPhysicalFileClass::RootUuid,
        )?),
        &(RepoMapImmutablePutOutcome::Created),
    )?;
    let mut reader = admitted.open_reader(&uuid)?.ok_or("UUID absent")?;
    let mut bytes = Vec::new();
    let _read = reader.read_to_end(&mut bytes)?;
    require_eq(&(bytes), &(b"0123456789abcdef".to_vec()))?;
    reader.retire()?;
    let charged = authority.snapshot()?;
    require_eq(
        &(charged.used_bytes),
        &(before.used_bytes.checked_add(16).ok_or("byte overflow")?),
    )?;
    require_eq(
        &(charged.used_files),
        &(before.used_files.checked_add(1).ok_or("file overflow")?),
    )?;
    require_eq(
        &(physical_inventory(&root)?),
        &(charged.used_bytes, charged.used_files),
    )?;
    require(binder.is_quiescent()?, "UUID left a live claim")?;
    Ok(())
}

#[test]
fn candidate_is_exactly_charged_replayed_and_reader_pinned_through_unlink() -> TestResult {
    let (_directory, root, authority, binder, admission) = fixture(64, 3)?;
    let path = candidate(&root);
    create_projection_parents(&admission, &root, &path)?;
    let before = authority.snapshot()?;
    require_eq(
        &(admission.put_immutable(&path, b"abc", RepoMapPhysicalFileClass::Candidate)?),
        &(RepoMapImmutablePutOutcome::Created),
    )?;
    require_eq(&(fs::read(&path)?), &(b"abc".to_vec()))?;
    require_eq(
        &(admission.put_immutable(&path, b"abc", RepoMapPhysicalFileClass::Candidate)?),
        &(RepoMapImmutablePutOutcome::ExactReplay),
    )?;
    let charged = authority.snapshot()?;
    require_eq(
        &(physical_inventory(&root)?),
        &(charged.used_bytes, charged.used_files),
    )?;
    require_eq(
        &(charged.used_bytes),
        &(before.used_bytes.checked_add(3).ok_or("byte overflow")?),
    )?;
    require_eq(
        &(charged.used_files),
        &(before.used_files.checked_add(1).ok_or("file overflow")?),
    )?;
    let mut reader = admission
        .open_reader(&path)?
        .ok_or("published projection missing")?;
    let metadata = reader.metadata()?;
    let mut actual = Vec::new();
    let _read = reader.read_to_end(&mut actual)?;
    require_eq(&(actual), &(b"abc".to_vec()))?;
    admission.delete_file_and_sync(
        &path,
        quanta_index_core::publication_capacity::InodeKey {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        metadata.len(),
    )?;
    require(!path.exists(), "delete retained a pathname")?;
    require_eq(&(authority.snapshot()?.used_bytes), &(charged.used_bytes))?;
    require_eq(&(authority.snapshot()?.used_files), &(charged.used_files))?;
    reader.retire()?;
    require_eq(&(authority.snapshot()?.used_bytes), &(before.used_bytes))?;
    require_eq(&(authority.snapshot()?.used_files), &(before.used_files))?;
    require_eq(
        &(physical_inventory(&root)?),
        &(before.used_bytes, before.used_files),
    )?;
    require(binder.is_quiescent()?, "reader or writer claim remains")?;
    Ok(())
}

#[test]
fn zero_byte_payload_retains_one_file_without_a_provisional_byte() -> TestResult {
    let (_directory, root, authority, binder, admission) = fixture(8, 8)?;
    let path = root.join("repo-map").join(
        QuarantinePayloadAddressV1::new(QuarantinePayloadDigestV1::from_bytes([0xcd; 32]))
            .relative_path(),
    );
    create_projection_parents(&admission, &root, &path)?;
    let before = authority.snapshot()?;
    require_eq(
        &(admission.put_immutable(&path, b"", RepoMapPhysicalFileClass::Payload)?),
        &(RepoMapImmutablePutOutcome::Created),
    )?;
    require_eq(&(fs::metadata(&path)?.len()), &(0))?;
    require_eq(&(authority.snapshot()?.used_bytes), &(before.used_bytes))?;
    require_eq(
        &(authority.snapshot()?.used_files),
        &(before.used_files.checked_add(1).ok_or("file overflow")?),
    )?;
    let charged = authority.snapshot()?;
    require_eq(
        &(physical_inventory(&root)?),
        &(charged.used_bytes, charged.used_files),
    )?;
    require_eq(
        &(admission.put_immutable(&path, b"", RepoMapPhysicalFileClass::Payload)?),
        &(RepoMapImmutablePutOutcome::ExactReplay),
    )?;
    let reader = admission
        .open_reader(&path)?
        .ok_or("zero payload missing")?;
    let metadata = reader.metadata()?;
    admission.delete_file_and_sync(
        &path,
        quanta_index_core::publication_capacity::InodeKey {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        0,
    )?;
    reader.retire()?;
    require_eq(&(authority.snapshot()?.used_bytes), &(before.used_bytes))?;
    require_eq(&(authority.snapshot()?.used_files), &(before.used_files))?;
    require(binder.is_quiescent()?, "zero payload retained custody")?;
    Ok(())
}

#[test]
fn class_bound_and_foreign_paths_refuse_before_physical_mutation() -> TestResult {
    let (directory, root, authority, binder, admission) = fixture(16, 2)?;
    let path = candidate(&root);
    let before = authority.snapshot()?;
    require(
        matches!(
            admission.put_immutable(&path, b"abc", RepoMapPhysicalFileClass::Candidate),
            Err(CoreError::Typed { .. })
        ),
        "oversized candidate passed configured bound",
    )?;
    require(!root.join("repo-map").exists(), "refusal created layout")?;
    for rejected in [
        directory.path().join("foreign/root-uuid.bin"),
        root.join("repo-map/../repo-map/root-uuid.bin"),
        root.join("repo-map/foreign.bin"),
    ] {
        require(
            matches!(
                admission.open_reader(&rejected),
                Err(CoreError::InvalidContract(_))
            ),
            "reader accepted noncanonical path",
        )?;
    }
    require_eq(&(authority.snapshot()?), &(before))?;
    require(binder.is_quiescent()?, "refusal retained capacity claim")?;
    Ok(())
}

#[test]
fn symlinked_layout_parent_and_foreign_class_are_refused() -> TestResult {
    let (directory, root, authority, binder, admission) = fixture(32, 32)?;
    let layout = root.join("repo-map");
    admission.ensure_private_directory(&layout)?;
    let foreign = directory.path().join("foreign");
    fs::create_dir(&foreign)?;
    symlink(&foreign, layout.join("objects"))?;
    let before = authority.snapshot()?;
    require(
        admission
            .ensure_private_directory(&layout.join("objects"))
            .is_err(),
        "symlinked directory passed nofollow admission",
    )?;
    require(
        matches!(
            admission.put_immutable(
                &layout.join("root-uuid.bin"),
                b"uuid",
                RepoMapPhysicalFileClass::Candidate
            ),
            Err(CoreError::InvalidContract(_))
        ),
        "candidate class accepted Control UUID path",
    )?;
    require_eq(&(authority.snapshot()?), &(before))?;
    require(binder.is_quiescent()?, "symlink refusal retained custody")?;
    Ok(())
}

#[test]
fn boot_orphan_scan_refuses_before_mutation_then_retires_exact_seeded_inodes() -> TestResult {
    let first = format!(".qiv-repomap-{}.tmp", "a".repeat(32));
    let second = format!(".qiv-repomap-{}.tmp", "b".repeat(32));
    let (_directory, root, authority, binder, deletions, lease) =
        seeded_fixture(&[(&first, b"abc"), (&second, b"d")])?;
    let before = authority.snapshot()?;
    require_eq(
        &(physical_inventory(&root)?),
        &(before.used_bytes, before.used_files),
    )?;
    require(
        matches!(
            CanonicalRepoMapFileAdmission::new(
                Arc::clone(&deletions),
                lease.try_clone_root_directory_v1()?,
                16,
                16,
                1,
            ),
            Err(CoreError::Typed { .. })
        ),
        "undersized boot scan admitted an incomplete inventory",
    )?;
    require(
        root.join(&first).exists() && root.join(&second).exists(),
        "bound refusal removed an orphan",
    )?;
    require_eq(&(authority.snapshot()?), &before)?;
    let _admission = CanonicalRepoMapFileAdmission::new(
        deletions,
        lease.try_clone_root_directory_v1()?,
        16,
        16,
        64,
    )?;
    require(
        !root.join(&first).exists() && !root.join(&second).exists(),
        "boot orphan survived exact cleanup",
    )?;
    let after = authority.snapshot()?;
    require_eq(
        &(after.used_bytes),
        &(before
            .used_bytes
            .checked_sub(4)
            .ok_or("orphan bytes underflow")?),
    )?;
    require_eq(
        &(after.used_files),
        &(before
            .used_files
            .checked_sub(2)
            .ok_or("orphan files underflow")?),
    )?;
    require_eq(
        &(physical_inventory(&root)?),
        &(after.used_bytes, after.used_files),
    )?;
    require(binder.is_quiescent()?, "boot cleanup retained a mutation")?;
    Ok(())
}

#[test]
fn malformed_boot_orphan_preserves_all_charge_until_exact_recovery() -> TestResult {
    let valid = format!(".qiv-repomap-{}.tmp", "c".repeat(32));
    let malformed = ".qiv-repomap-not-hex.tmp";
    let (_directory, root, authority, binder, deletions, lease) =
        seeded_fixture(&[(&valid, b"xyz"), (malformed, b"q")])?;
    let before = authority.snapshot()?;
    require(
        matches!(
            CanonicalRepoMapFileAdmission::new(
                Arc::clone(&deletions),
                lease.try_clone_root_directory_v1()?,
                16,
                16,
                64,
            ),
            Err(CoreError::InvalidContract(_))
        ),
        "malformed boot temporary passed canonical scan",
    )?;
    require(
        root.join(&valid).exists() && root.join(malformed).exists(),
        "failed constructor changed boot files",
    )?;
    require_eq(&(authority.snapshot()?), &before)?;
    let bad = fs::metadata(root.join(malformed))?;
    deletions.delete_owned_file_and_sync(
        &root.join(malformed),
        quanta_index_core::publication_capacity::InodeKey {
            device: bad.dev(),
            inode: bad.ino(),
        },
        bad.len(),
    )?;
    let _admission = CanonicalRepoMapFileAdmission::new(
        deletions,
        lease.try_clone_root_directory_v1()?,
        16,
        16,
        64,
    )?;
    require(
        !root.join(&valid).exists(),
        "retry did not retire valid orphan",
    )?;
    let after = authority.snapshot()?;
    require_eq(
        &(after.used_bytes),
        &(before
            .used_bytes
            .checked_sub(4)
            .ok_or("orphan bytes underflow")?),
    )?;
    require_eq(
        &(after.used_files),
        &(before
            .used_files
            .checked_sub(2)
            .ok_or("orphan files underflow")?),
    )?;
    require_eq(
        &(physical_inventory(&root)?),
        &(after.used_bytes, after.used_files),
    )?;
    require(binder.is_quiescent()?, "retry retained a mutation")?;
    Ok(())
}

#[test]
fn rejected_created_writer_is_closed_and_refunded_only_after_exact_zero_link_proof() -> TestResult {
    let (_directory, root, authority, binder, admission) = fixture(8, 8)?;
    let layout = root.join("repo-map");
    admission.ensure_private_directory(&layout)?;
    let target = layout.join("root-uuid.bin");
    let staging = root.join(format!(".qiv-repomap-{}.tmp", "d".repeat(32)));
    let baseline = authority.snapshot()?;
    let parent = binder.reserve_publication_allowance(3, 1)?;
    let request = PublicationOutputReservation::new(PublicationOutputClass::Control, 3, 1)?;
    let mut permit = recoverable_sidecar_output_permit(
        Arc::clone(&admission.deletions),
        Arc::clone(&parent),
        request,
    )?;
    permit.bind_output(&staging, &target)?;
    permit.creation_started()?;
    let writer = File::from(openat(
        &admission.held_root,
        Path::new(staging.file_name().ok_or("staging leaf absent")?),
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?);
    let original = permit.created(&writer, &root.join("wrong-stage"));
    require(
        matches!(original, Err(CoreError::InvalidContract(_))),
        "wrong declared staging path did not reject FD registration",
    )?;
    permit.cancel_rejected_created(writer)?;
    drop(permit);
    drop(parent);
    require(
        !staging.exists() && !target.exists(),
        "rejected original writer left a staged or final path",
    )?;
    require_eq(&(authority.snapshot()?), &baseline)?;
    require_eq(
        &(physical_inventory(&root)?),
        &(baseline.used_bytes, baseline.used_files),
    )?;
    require(
        binder.is_quiescent()?,
        "rejected writer retained a pending mutation",
    )?;
    Ok(())
}
