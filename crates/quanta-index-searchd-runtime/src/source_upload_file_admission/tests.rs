use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::PathBuf;
use std::sync::Arc;

use quanta_index_contract::{
    BatchIngestMode, ManifestGeneration, RepoId, RevisionId, SearchCorpusIngestBatch,
    SearchPlaneErrorCodeV2, SourcePublicationEvent, SourcePublicationUploadIdentity,
    SourcePublicationUploadPart,
};
use quanta_index_core::publication_capacity::PublicationCapacity;
use quanta_index_core::{
    CoreError, RequestBudgetV1, SourcePublicationUploadFileAdmissionPort as _,
    SourcePublicationUploadPort as _,
};
use quanta_index_ipc::SourcePublicationUploadStore;
use quanta_index_searchd::app::runtime::StateRootLease;
use rustix::fs::{Mode, OFlags, open};
use sha2::{Digest as _, Sha256};

use super::CanonicalSourceUploadFileAdmission;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Fixture = (
    tempfile::TempDir,
    PathBuf,
    PublicationCapacity,
    Arc<PublicationFileDeletions>,
    StateRootLease,
);

fn require(condition: bool, message: &'static str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with_staging(&[])
}

fn fixture_with_boot_temp(staged: bool) -> Result<Fixture, Box<dyn std::error::Error>> {
    if staged {
        let name = format!(".qiv-source-upload-{}.tmp", "0".repeat(32));
        fixture_with_staging(&[(&name, b"x")])
    } else {
        fixture_with_staging(&[])
    }
}

fn fixture_with_staging(staged: &[(&str, &[u8])]) -> Result<Fixture, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let uploads = root.join("source-publication-uploads");
    fs::create_dir(&uploads)?;
    fs::set_permissions(&uploads, fs::Permissions::from_mode(0o700))?;
    for (name, bytes) in staged {
        let temp = root.join(name);
        fs::write(&temp, bytes)?;
        fs::set_permissions(temp, fs::Permissions::from_mode(0o600))?;
    }
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 1024,
            max_files: 32,
            max_pending: 32,
            max_entries: 64,
            max_depth: 8,
            max_pending_frames: 16,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let binder = PublicationFileCapacity::new(lease.clone(), authority.clone(), 16, 64, 256)?;
    Ok((
        directory,
        root,
        authority,
        Arc::new(PublicationFileDeletions::new(binder)),
        lease,
    ))
}

#[test]
fn boot_seeded_exact_temp_is_deleted_before_upload_admission() -> TestResult {
    let (_directory, root, authority, deletions, lease) = fixture_with_boot_temp(true)?;
    let temp = root.join(format!(".qiv-source-upload-{}.tmp", "0".repeat(32)));
    let before = authority.snapshot()?;
    require(temp.exists(), "boot fixture temp missing")?;
    let _admission = CanonicalSourceUploadFileAdmission::new(
        deletions,
        lease.try_clone_root_directory_v1()?,
        6,
        8,
        64,
    )?;
    let after = authority.snapshot()?;
    require(!temp.exists(), "seeded upload temp survived constructor")?;
    require(
        after.used_bytes + 1 == before.used_bytes,
        "exact one-byte temp charge was not retired",
    )?;
    require(
        after.used_files + 1 == before.used_files,
        "exact one temp inode slot was not retired",
    )?;
    Ok(())
}

#[test]
fn boot_scan_bound_refuses_without_partial_orphan_deletion_then_retries() -> TestResult {
    let first = format!(".qiv-source-upload-{}.tmp", "a".repeat(32));
    let second = format!(".qiv-source-upload-{}.tmp", "b".repeat(32));
    let (_directory, root, authority, deletions, lease) =
        fixture_with_staging(&[(&first, b"abc"), (&second, b"d")])?;
    let before = authority.snapshot()?;
    require(
        matches!(
            CanonicalSourceUploadFileAdmission::new(
                Arc::clone(&deletions),
                lease.try_clone_root_directory_v1()?,
                6,
                8,
                1,
            ),
            Err(CoreError::Typed { .. })
        ),
        "short boot scan admitted only a prefix of the namespace",
    )?;
    require(
        root.join(&first).exists() && root.join(&second).exists(),
        "failed boot scan removed a staged body",
    )?;
    require(
        authority.snapshot()? == before,
        "failed boot scan changed the sealed ledger",
    )?;
    let _admission = CanonicalSourceUploadFileAdmission::new(
        deletions.clone(),
        lease.try_clone_root_directory_v1()?,
        6,
        8,
        64,
    )?;
    require(
        !root.join(&first).exists() && !root.join(&second).exists(),
        "retry did not remove exact seeded orphans",
    )?;
    let after = authority.snapshot()?;
    require(
        after.used_bytes == before.used_bytes.checked_sub(4).ok_or("byte underflow")?
            && after.used_files == before.used_files.checked_sub(2).ok_or("file underflow")?,
        "exact orphan bytes or file slots were not refunded",
    )?;
    require(
        deletions.binder().is_quiescent()?,
        "orphan retry retained a live custody claim",
    )?;
    Ok(())
}

fn body(root: &std::path::Path) -> PathBuf {
    root.join("source-publication-uploads")
        .join(format!("{}-0000000000000006.cbor", "a".repeat(64)))
}

#[test]
fn birth_restart_remaining_growth_reader_pin_and_exact_delete() -> TestResult {
    let (_directory, root, authority, deletions, lease) = fixture()?;
    let path = body(&root);
    let baseline = authority.snapshot()?;
    let admission = CanonicalSourceUploadFileAdmission::new(
        Arc::clone(&deletions),
        lease.try_clone_root_directory_v1()?,
        6,
        8,
        64,
    )?;
    let mut first = admission.begin_write(&path, 6, None, 3)?;
    first.write_all(b"abc")?;
    first.sync_and_settle()?;
    let after_first = authority.snapshot()?;
    require(
        after_first.used_bytes == baseline.used_bytes + 6,
        "first body parent did not hold declared bound",
    )?;
    require(
        after_first.used_files == baseline.used_files + 1,
        "first body did not hold one slot",
    )?;
    drop(admission);
    let after_restart = authority.snapshot()?;
    require(
        after_restart.used_bytes == baseline.used_bytes + 3,
        "dropping process-local parent did not retain exactly 3 bytes",
    )?;
    require(
        after_restart.used_files == baseline.used_files + 1,
        "existing inode lost its slot",
    )?;

    let admission = CanonicalSourceUploadFileAdmission::new(
        Arc::clone(&deletions),
        lease.try_clone_root_directory_v1()?,
        6,
        8,
        64,
    )?;
    let mut last = admission.begin_write(&path, 6, Some(3), 6)?;
    last.write_all(b"def")?;
    last.sync_and_settle()?;
    let complete = authority.snapshot()?;
    require(
        complete.used_bytes == baseline.used_bytes + 6,
        "restart double-counted existing bytes",
    )?;
    require(
        complete.used_files == baseline.used_files + 1,
        "restart double-counted inode slot",
    )?;
    require(
        fs::read(&path)? == b"abcdef",
        "body bytes differ from fixed golden",
    )?;

    let ticket = admission.begin_reader_open(&path)?;
    let original = File::from(open(
        &path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let mut reader = ticket.opened_owned(original)?;
    let mut bytes = [0_u8; 6];
    reader.read_exact(&mut bytes)?;
    require(bytes == *b"abcdef", "admitted reader changed bytes")?;
    let metadata = reader.metadata()?;
    admission.delete_file_and_sync(
        &path,
        quanta_index_core::publication_capacity::InodeKey {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        6,
    )?;
    require(!path.exists(), "exact deletion left the pathname")?;
    require(
        authority.snapshot()?.used_bytes == baseline.used_bytes + 6,
        "live original reader lost retained charge",
    )?;
    reader.retire()?;
    deletions.binder().sweep_retired()?;
    require(
        authority.snapshot()? == baseline,
        "reader retirement did not restore boot baseline",
    )?;
    Ok(())
}

#[test]
fn configured_bound_and_foreign_path_refuse_before_create() -> TestResult {
    let (directory, root, authority, deletions, lease) = fixture()?;
    let admission = CanonicalSourceUploadFileAdmission::new(
        Arc::clone(&deletions),
        lease.try_clone_root_directory_v1()?,
        5,
        8,
        64,
    )?;
    let body = body(&root);
    let baseline = authority.snapshot()?;
    require(
        matches!(
            admission.begin_write(&body, 6, None, 3),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                ..
            })
        ),
        "configured body bound did not return typed refusal",
    )?;
    let foreign = directory
        .path()
        .join("foreign")
        .join(body.file_name().ok_or("body leaf missing")?);
    let widened = CanonicalSourceUploadFileAdmission::new(
        deletions,
        lease.try_clone_root_directory_v1()?,
        6,
        8,
        64,
    )?;
    require(
        matches!(
            widened.begin_write(&foreign, 6, None, 3),
            Err(CoreError::InvalidContract(_))
        ),
        "foreign path reached creation",
    )?;
    require(
        !body.exists() && !foreign.exists(),
        "refusal created a body",
    )?;
    require(authority.snapshot()? == baseline, "refusal changed ledger")?;
    Ok(())
}

#[test]
fn retired_partial_append_recovers_before_overlap_reader_and_retries_exact_suffix() -> TestResult {
    let (_directory, root, authority, deletions, lease) = fixture()?;
    let path = body(&root);
    let baseline = authority.snapshot()?;
    let admission = CanonicalSourceUploadFileAdmission::new(
        Arc::clone(&deletions),
        lease.try_clone_root_directory_v1()?,
        6,
        8,
        64,
    )?;
    let mut first = admission.begin_write(&path, 6, None, 3)?;
    first.write_all(b"abc")?;
    first.sync_and_settle()?;

    let mut interrupted = admission.begin_write(&path, 6, Some(3), 6)?;
    interrupted.write_all(b"d")?;
    let in_progress = fs::metadata(&path)?;
    require(
        matches!(
            admission.delete_file_and_sync(
                &path,
                quanta_index_core::publication_capacity::InodeKey {
                    device: in_progress.dev(),
                    inode: in_progress.ino(),
                },
                in_progress.len(),
            ),
            Err(CoreError::InvalidContract(_))
        ) && path.exists(),
        "active upload writer was deleted",
    )?;
    interrupted.retire_preserving_pending()?;
    let ticket = admission.begin_reader_open(&path)?;
    let original = File::from(open(
        &path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let mut reader = ticket.opened_owned(original)?;
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix)?;
    require(
        prefix == *b"abcd",
        "retired append lost the exact durable prefix",
    )?;
    reader.retire()?;

    let mut retried = admission.begin_write(&path, 6, Some(4), 6)?;
    retried.write_all(b"ef")?;
    retried.sync_and_settle()?;
    require(
        fs::read(&path)? == b"abcdef",
        "retry changed the fixed body",
    )?;
    require(
        authority.snapshot()?.used_bytes == baseline.used_bytes + 6,
        "partial recovery charged extra bytes",
    )?;
    Ok(())
}

#[test]
fn held_upload_directory_replacement_refuses_before_body_creation() -> TestResult {
    let (_directory, root, authority, deletions, lease) = fixture()?;
    let admission = CanonicalSourceUploadFileAdmission::new(
        deletions,
        lease.try_clone_root_directory_v1()?,
        6,
        8,
        64,
    )?;
    let original = root.join("source-publication-uploads");
    let displaced = root.join("source-publication-uploads-displaced");
    let target = body(&root);
    let baseline = authority.snapshot()?;
    let _pinned = admission.held_upload_directory()?;
    fs::rename(&original, &displaced)?;
    std::os::unix::fs::symlink(&displaced, &original)?;
    require(
        admission.begin_write(&target, 6, None, 3).is_err(),
        "replaced upload directory admitted a writer",
    )?;
    require(
        !displaced
            .join(target.file_name().ok_or("missing leaf")?)
            .exists(),
        "refusal wrote into displaced directory",
    )?;
    fs::remove_file(&original)?;
    fs::create_dir(&original)?;
    fs::set_permissions(&original, fs::Permissions::from_mode(0o700))?;
    require(
        admission.begin_write(&target, 6, None, 3).is_err() && !target.exists(),
        "same-mode replacement directory admitted a writer",
    )?;
    require(
        authority.snapshot()? == baseline,
        "directory replacement changed ledger",
    )?;
    Ok(())
}

#[test]
fn canonical_consumer_ack_retry_load_discard_restores_boot_ledger() -> TestResult {
    let (_directory, root, authority, deletions, lease) = fixture()?;
    let baseline = authority.snapshot()?;
    let batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "upload-test".into(),
            event_id: "event-1".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("revision")?,
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "manifest".into(),
        batch_digest: String::new(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: Vec::new(),
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    let mut wire = Vec::new();
    ciborium::into_writer(&batch, &mut wire)?;
    let identity = SourcePublicationUploadIdentity {
        body_sha256: Sha256::digest(&wire).into(),
        body_bytes: u64::try_from(wire.len())?,
    };
    let admission = Arc::new(CanonicalSourceUploadFileAdmission::new(
        Arc::clone(&deletions),
        lease.try_clone_root_directory_v1()?,
        1024,
        8,
        64,
    )?);
    let store = SourcePublicationUploadStore::open_at_with_file_admission(
        lease.try_clone_root_directory_v1()?,
        &root,
        std::ffi::OsStr::new("source-publication-uploads"),
        1024,
        admission,
    )?;
    let split = wire
        .len()
        .checked_div(2)
        .ok_or("invalid wire split divisor")?;
    let first = SourcePublicationUploadPart {
        identity,
        offset: 0,
        bytes: wire.get(..split).ok_or("missing first part")?.to_vec(),
    };
    let second = SourcePublicationUploadPart {
        identity,
        offset: u64::try_from(split)?,
        bytes: wire.get(split..).ok_or("missing second part")?.to_vec(),
    };
    let budget = RequestBudgetV1::unbounded();
    require(
        store.stage(&first, &budget)?.next_offset == u64::try_from(split)?,
        "first ACK offset changed",
    )?;
    require(
        store.stage(&first, &budget)?.next_offset == u64::try_from(split)?,
        "duplicate ACK offset changed",
    )?;
    require(
        store.stage(&second, &budget)?.next_offset == identity.body_bytes,
        "complete ACK offset changed",
    )?;
    require(
        store.load(identity, &budget)? == batch,
        "canonical consumer changed fixed batch fields",
    )?;
    store.discard(identity)?;
    deletions.binder().sweep_retired()?;
    require(
        authority.snapshot()? == baseline,
        "consumer discard did not restore boot ledger",
    )?;
    Ok(())
}
