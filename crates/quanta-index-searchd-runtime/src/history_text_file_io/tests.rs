use std::error::Error;
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::RequestBudgetV1;
use quanta_index_core::publication_capacity::PublicationCapacity;
use quanta_index_core::{CoreError, HistoryTextFileAdmissionFactory as _};
use quanta_index_searchd::app::runtime::StateRootLease;

use super::CanonicalHistoryTextFileAdmissionFactory;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn Error>>;

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    history: PathBuf,
    authority: PublicationCapacity,
    binder: Arc<PublicationFileCapacity>,
    factory: CanonicalHistoryTextFileAdmissionFactory,
}

fn fixture(bytes: u64, files: u64) -> Result<Fixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let held = lease.try_clone_root_directory_v1()?;
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 4096,
            max_files: 64,
            max_pending: 64,
            max_entries: 128,
            max_depth: 10,
            max_pending_frames: 32,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let binder = PublicationFileCapacity::new(lease, authority.clone(), 32, 128, 256)?;
    let deletions = Arc::new(PublicationFileDeletions::new(Arc::clone(&binder)));
    let factory = CanonicalHistoryTextFileAdmissionFactory::new(deletions, held, bytes, files, 64)?;
    let history = root.join("authorities").join("history-text");
    Ok(Fixture {
        _directory: directory,
        root,
        history,
        authority,
        binder,
        factory,
    })
}

#[test]
fn interrupted_publication_refuses_before_parent_or_file_birth() -> TestResult {
    let fixture = fixture(8, 2)?;
    let boot = fixture.authority.snapshot()?;
    let expired_at = Instant::now()
        .checked_sub(Duration::from_millis(1))
        .ok_or("test clock cannot represent an expired deadline")?;
    let expired = RequestBudgetV1::until(expired_at);
    if !matches!(
        fixture.factory.for_publication(&expired),
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
            ..
        })
    ) || fixture.authority.snapshot()? != boot
        || fixture.history.exists()
    {
        return Err("expired publication reserved capacity or created a file".into());
    }
    let cancelled = RequestBudgetV1::unbounded();
    cancelled.cancel_handle().cancel();
    if !matches!(
        fixture.factory.for_publication(&cancelled),
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        })
    ) || fixture.authority.snapshot()? != boot
    {
        return Err("cancelled publication reserved capacity".into());
    }
    let live = RequestBudgetV1::unbounded();
    let work = fixture.factory.for_publication(&live)?;
    if work.original_interruption()?.is_some() {
        return Err("fresh publication reported an interruption".into());
    }
    live.cancel_handle().cancel();
    if work.original_interruption()?.is_some() {
        return Err("publication inferred interruption from a later budget probe".into());
    }
    let path = fixture.history.join("never-born.idx");
    let observed = work
        .create_writer(&path)
        .err()
        .ok_or("cancelled scope admitted a writer")?;
    if !matches!(
        &observed,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        }
    ) || work.original_interruption()? != Some(observed)
        || path.exists()
    {
        return Err("cancelled scope lacked its original I/O interruption witness".into());
    }
    drop(work);
    if fixture.authority.snapshot()? != boot || !fixture.binder.is_quiescent()? {
        return Err("interrupted publication retained a charge or custody".into());
    }
    if fixture
        .factory
        .for_retirement()?
        .original_interruption()?
        .is_some()
        || fixture
            .factory
            .for_read()?
            .original_interruption()?
            .is_some()
        || fixture
            .factory
            .for_publication(&RequestBudgetV1::unbounded())?
            .original_interruption()?
            .is_some()
    {
        return Err("one publication's interruption leaked into another scope".into());
    }
    Ok(())
}

#[test]
fn cancellation_after_original_write_keeps_three_bytes_until_checked_recovery() -> TestResult {
    let fixture = fixture(8, 1)?;
    let boot = fixture.authority.snapshot()?;
    let budget = RequestBudgetV1::unbounded();
    let work = fixture.factory.for_publication(&budget)?;
    work.ensure_directory(&fixture.history)?;
    let path = fixture.history.join("cancel-after-abc.idx");
    let mut writer = work.create_writer(&path)?;
    writer.write_all(b"abc")?;
    if work.original_interruption()?.is_some() {
        return Err("successful original write recorded an interruption".into());
    }
    budget.cancel_handle().cancel();
    if work.original_interruption()?.is_some() {
        return Err("publication inferred interruption before another I/O checkpoint".into());
    }
    let error = writer
        .write_all(b"d")
        .err()
        .ok_or("cancelled writer accepted later bytes")?;
    let source = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<CoreError>());
    if !matches!(
        source,
        Some(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        })
    ) || fs::read(&path)? != b"abc"
    {
        return Err("cancelled original write lost its typed cause or changed bytes".into());
    }
    if work.original_interruption()? != source.cloned() {
        return Err("scope did not retain the original writer interruption".into());
    }
    if writer.sync_and_settle().is_ok() {
        return Err("cancelled writer settled as a complete publication".into());
    }
    drop(work);
    let read = fixture.factory.for_read()?;
    let mut recovered = read
        .open_reader(&path)?
        .ok_or("partial original disappeared")?;
    let mut actual = [0_u8; 3];
    recovered.read_exact(&mut actual)?;
    if actual != *b"abc" {
        return Err("recovery changed bytes written before cancellation".into());
    }
    recovered.retire()?;
    drop(read);
    let retirement = fixture.factory.for_retirement()?;
    if retirement.original_interruption()?.is_some() {
        return Err("retirement inherited publication cancellation".into());
    }
    retirement.delete_file(&path)?;
    drop(retirement);
    if fixture.authority.snapshot()? != boot || !fixture.binder.is_quiescent()? {
        return Err("cancelled writer FD or charge was not retired".into());
    }
    if fixture
        .factory
        .for_publication(&RequestBudgetV1::unbounded())?
        .original_interruption()?
        .is_some()
    {
        return Err("new publication inherited the old interruption".into());
    }
    Ok(())
}

#[test]
fn cancelled_resize_keeps_the_original_zero_byte_file_and_cause() -> TestResult {
    let fixture = fixture(8, 1)?;
    let boot = fixture.authority.snapshot()?;
    let budget = RequestBudgetV1::unbounded();
    let work = fixture.factory.for_publication(&budget)?;
    work.ensure_directory(&fixture.history)?;
    let path = fixture.history.join("cancel-before-resize.idx");
    let mut writer = work.create_writer(&path)?;
    budget.cancel_handle().cancel();
    if work.original_interruption()?.is_some() {
        return Err("resize interruption was inferred before its I/O checkpoint".into());
    }
    let observed = writer
        .set_len(1)
        .err()
        .ok_or("cancelled resize grew file")?;
    if !matches!(
        &observed,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        }
    ) || work.original_interruption()? != Some(observed.clone())
        || fs::metadata(&path)?.len() != 0
    {
        return Err("cancelled resize changed the original file or lost its cause".into());
    }
    if writer.sync_and_settle() != Err(observed) {
        return Err("failed resize did not return its first interruption".into());
    }
    drop(work);
    let read = fixture.factory.for_read()?;
    let recovered = read
        .open_reader(&path)?
        .ok_or("zero byte file disappeared")?;
    if recovered.metadata()?.len() != 0 || read.original_interruption()?.is_some() {
        return Err("zero byte recovery or read scope changed after resize refusal".into());
    }
    recovered.retire()?;
    drop(read);
    let retirement = fixture.factory.for_retirement()?;
    retirement.delete_file(&path)?;
    if retirement.original_interruption()?.is_some()
        || fixture.authority.snapshot()? != boot
        || !fixture.binder.is_quiescent()?
    {
        return Err("cancelled resize custody or charge did not retire".into());
    }
    Ok(())
}

#[test]
fn visible_three_byte_writer_and_last_reader_refund_unlinked_inode() -> TestResult {
    let fixture = fixture(4, 1)?;
    let boot = fixture.authority.snapshot()?;
    let work = fixture
        .factory
        .for_publication(&RequestBudgetV1::unbounded())?;
    work.validate_root(&fixture.history)?;
    work.ensure_directory(&fixture.history)?;
    let path = fixture.history.join("abc.idx");
    let mut writer = work.create_writer(&path)?;
    writer.write_all(b"abc")?;
    writer.sync_and_settle()?;
    if work
        .create_writer(&fixture.history.join("second.idx"))
        .is_ok()
        || fixture.history.join("second.idx").exists()
    {
        return Err("exhausted inode slot created a second history file".into());
    }
    drop(work);
    let charged = fixture.authority.snapshot()?;
    if charged.used_bytes != boot.used_bytes.checked_add(3).ok_or("byte overflow")?
        || charged.used_files != boot.used_files.checked_add(1).ok_or("file overflow")?
    {
        return Err("fixed three-byte history file was not charged exactly".into());
    }
    let read = fixture.factory.for_read()?;
    let mut reader = read.open_reader(&path)?.ok_or("history reader absent")?;
    let retirement = fixture.factory.for_retirement()?;
    retirement.delete_file(&path)?;
    if fixture.authority.snapshot()? != charged {
        return Err("unlink refunded while an original reader FD remained open".into());
    }
    let _position = reader.seek(SeekFrom::Start(0))?;
    let mut observed = [0_u8; 3];
    reader.read_exact(&mut observed)?;
    if observed != *b"abc" {
        return Err("pinned reader lost the unlinked original inode".into());
    }
    reader.retire()?;
    drop(read);
    drop(retirement);
    if fixture.authority.snapshot()? != boot || !fixture.binder.is_quiescent()? {
        return Err("last reader retirement did not restore boot charge".into());
    }
    Ok(())
}

#[test]
fn growth_refusal_precedes_syscall_and_failed_writer_cannot_settle() -> TestResult {
    let fixture = fixture(4, 1)?;
    let boot = fixture.authority.snapshot()?;
    let work = fixture
        .factory
        .for_publication(&RequestBudgetV1::unbounded())?;
    work.ensure_directory(&fixture.history)?;
    let path = fixture.history.join("partial.idx");
    let mut writer = work.create_writer(&path)?;
    writer.write_all(b"abc")?;
    let refusal = writer
        .write_all(b"de")
        .err()
        .ok_or("over-budget history growth was accepted")?;
    let typed = refusal
        .get_ref()
        .and_then(|source| source.downcast_ref::<CoreError>());
    if !matches!(
        typed,
        Some(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        })
    ) || fs::metadata(&path)?.len() != 3
    {
        return Err("over-budget write reached the visible original inode".into());
    }
    if writer.sync_and_settle().is_ok() || work.create_writer(&path).is_ok() {
        return Err("failed writer settled or reused an uncertain pathname".into());
    }
    drop(work);
    let read = fixture.factory.for_read()?;
    let mut recovered = read.open_reader(&path)?.ok_or("retired writer absent")?;
    let mut observed = [0_u8; 3];
    recovered.read_exact(&mut observed)?;
    if observed != *b"abc" {
        return Err("recovery changed the three bytes written before refusal".into());
    }
    recovered.retire()?;
    drop(read);
    let retirement = fixture.factory.for_retirement()?;
    retirement.delete_file(&path)?;
    drop(retirement);
    if fixture.authority.snapshot()? != boot || !fixture.binder.is_quiescent()? {
        return Err("recovered failed writer did not restore boot charge".into());
    }
    Ok(())
}

#[test]
fn atomic_replace_retains_old_pinned_bytes_then_tree_delete_refunds_aliases() -> TestResult {
    let fixture = fixture(32, 4)?;
    let boot = fixture.authority.snapshot()?;
    let work = fixture
        .factory
        .for_publication(&RequestBudgetV1::unbounded())?;
    let epoch = fixture.history.join("epoch-1");
    work.ensure_directory(&epoch)?;
    let original = epoch.join("data.idx");
    let alias = epoch.join("data.link");
    let copied = epoch.join("data.copy");
    work.atomic_write(&original, b"abc")?;
    let old = fixture
        .factory
        .for_read()?
        .open_reader(&original)?
        .ok_or("original history file absent")?;
    work.link_file(&original, &alias)?;
    work.copy_file(&original, &copied)?;
    let original_inode = fs::metadata(&original)?.ino();
    if fs::metadata(&alias)?.ino() != original_inode
        || fs::metadata(&copied)?.ino() == original_inode
    {
        return Err("link and copy lost their distinct inode semantics".into());
    }
    work.atomic_write(&original, b"defg")?;
    if fs::read(&original)? != b"defg" || fs::read(&alias)? != b"abc" {
        return Err("replacement changed the pinned inherited inode".into());
    }
    drop(work);
    let charged = fixture.authority.snapshot()?;
    if charged.used_bytes != boot.used_bytes.checked_add(10).ok_or("byte overflow")?
        || charged.used_files != boot.used_files.checked_add(3).ok_or("file overflow")?
    {
        return Err("3B inherited inode + 3B copy + 4B replacement charge differs".into());
    }
    let retirement = fixture.factory.for_retirement()?;
    if retirement.remove_tree(&epoch)? != Some(10) {
        return Err("tree receipt did not count 4+3+3 path bytes".into());
    }
    let mut old = old;
    let _position = old.seek(SeekFrom::Start(0))?;
    let mut observed = [0_u8; 3];
    old.read_exact(&mut observed)?;
    let pinned = fixture.authority.snapshot()?;
    if observed != *b"abc"
        || pinned.used_bytes != boot.used_bytes.checked_add(3).ok_or("byte overflow")?
        || pinned.used_files != boot.used_files.checked_add(1).ok_or("file overflow")?
    {
        return Err("tree unlink lost the old pinned inode or prematurely refunded it".into());
    }
    old.retire()?;
    drop(retirement);
    if fixture.authority.snapshot()? != boot || !fixture.binder.is_quiescent()? {
        return Err("tree and last pin retirement did not restore boot charge".into());
    }
    Ok(())
}

#[test]
fn foreign_paths_and_read_scope_mutations_refuse_before_creation() -> TestResult {
    let fixture = fixture(8, 2)?;
    let boot = fixture.authority.snapshot()?;
    let read = fixture.factory.for_read()?;
    if read.validate_root(&fixture.root).is_ok()
        || read
            .create_writer(&fixture.history.join("read-writer"))
            .is_ok()
        || read.ensure_directory(&fixture.history).is_ok()
        || read.metadata(&fixture.root.join("outside-history")).is_ok()
    {
        return Err("history scope accepted a foreign root or read-side mutation".into());
    }
    if fixture.history.exists() || fixture.authority.snapshot()? != boot {
        return Err("refused history operation changed filesystem or ledger".into());
    }
    Ok(())
}

#[test]
fn missing_nested_history_parent_is_clean_absence_without_birth_or_charge() -> TestResult {
    let fixture = fixture(8, 2)?;
    let boot = fixture.authority.snapshot()?;
    let read = fixture.factory.for_read()?;
    let missing = fixture
        .history
        .join("absent")
        .join("nested")
        .join("record.idx");
    if read.metadata(&missing)?.is_some() || read.open_reader(&missing)?.is_some() {
        return Err("missing history parent was observed as a file".into());
    }
    drop(read);
    if fixture.history.exists()
        || fixture.authority.snapshot()? != boot
        || !fixture.binder.is_quiescent()?
    {
        return Err("read of missing nested parents created entries or changed charge".into());
    }
    Ok(())
}

#[test]
fn uncertain_parent_close_result_is_not_reported_as_missing_history() -> TestResult {
    let fixture = fixture(8, 2)?;
    let boot = fixture.authority.snapshot()?;
    let read = fixture.factory.for_read()?;
    fixture
        .factory
        .shared
        .inject_next_close_eio
        .store(true, Ordering::SeqCst);
    let missing = fixture
        .history
        .join("absent")
        .join("nested")
        .join("record.idx");
    let error = read
        .metadata(&missing)
        .err()
        .ok_or("uncertain parent close was reported as clean absence")?;
    if !matches!(
        error,
        CoreError::StorageIo {
            raw_os_error: Some(code),
            ..
        } if code == rustix::io::Errno::IO.raw_os_error()
    ) || fixture.binder.is_quiescent()?
        || fixture.authority.snapshot()? != boot
        || fixture.history.exists()
    {
        return Err("uncertain close was swallowed or falsely changed history charge".into());
    }
    Ok(())
}

#[test]
fn lock_retains_original_fd_and_directory_move_keeps_exact_tree_charge() -> TestResult {
    let fixture = fixture(16, 3)?;
    let boot = fixture.authority.snapshot()?;
    let work = fixture
        .factory
        .for_publication(&RequestBudgetV1::unbounded())?;
    let staging = fixture.history.join("stage");
    let published = fixture.history.join("published");
    work.ensure_directory(&staging)?;
    let lock_path = staging.join(".tantivy-writer.lock");
    let lock = work.acquire_lock(&lock_path, false)?;
    if work.acquire_lock(&lock_path, false).is_ok() {
        return Err("second nonblocking lock acquired while original FD was held".into());
    }
    work.atomic_write(&staging.join("record.idx"), b"abc")?;
    lock.retire()?;
    work.move_directory_if_absent(&staging, &published)?;
    if staging.exists() || fs::read(published.join("record.idx"))? != b"abc" {
        return Err("admitted directory move lost the fixed record".into());
    }
    drop(work);
    let retirement = fixture.factory.for_retirement()?;
    if retirement.remove_tree(&published)? != Some(3) {
        return Err("moved tree receipt did not preserve the fixed three bytes".into());
    }
    drop(retirement);
    if fixture.authority.snapshot()? != boot || !fixture.binder.is_quiescent()? {
        return Err("directory move, lock, and tree removal did not restore boot charge".into());
    }
    Ok(())
}
