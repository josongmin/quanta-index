use std::error::Error;
use std::fs::{self, File};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::publication_admission::PublicationControlFileUpdatePort as _;
use quanta_index_core::publication_capacity::{CapacitySnapshot, PublicationCapacity};
use quanta_index_core::{
    CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, PublicationValidationOwner, RequestBudgetV1,
};
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd_harness::SourceCorpusFixture;
use rustix::fs::{Mode, OFlags, openat};

use super::PublicationControlFiles;
use crate::canonical_publication_admission::{
    CanonicalPublicationAdmission, PublicationAllowanceLimit,
};
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn pool_control_temporary_names_are_exact_and_exclude_permanent_or_work_files() -> TestResult {
    for name in ["..lifecycle-gate.tmp-17-4", "..lifecycle-epoch.tmp-0-0"] {
        if !super::is_pool_control_temporary(name) {
            return Err("canonical Control temporary refused".into());
        }
    }
    for name in [
        ".lifecycle-gate",
        ".lifecycle-epoch",
        ".object.tmp-17-4",
        "..lifecycle-gate.tmp--4",
        "..lifecycle-epoch.tmp-17-",
        "..lifecycle-epoch.tmp-17-4-extra",
        "..lifecycle-gate.tmp-17/4-1",
    ] {
        if super::is_pool_control_temporary(name) {
            return Err("foreign or malformed Control temporary accepted".into());
        }
    }
    Ok(())
}

struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
    authority: PublicationCapacity,
    binder: Arc<PublicationFileCapacity>,
    admission: CanonicalPublicationAdmission,
    boot: CapacitySnapshot,
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
    fixture_with_work_object(false)
}

fn fixture_with_work_object(include_work_object: bool) -> Result<Fixture, Box<dyn Error>> {
    fixture_with_files(include_work_object, false)
}

fn fixture_with_files(
    include_work_object: bool,
    include_gate: bool,
) -> Result<Fixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let attempts = root
        .join("indexes")
        .join("lexical")
        .join("lexical-object-pool")
        .join("attempts");
    fs::create_dir_all(&attempts)?;
    let path = attempts.join("journal");
    fs::write(&path, b"abc")?;
    if include_gate {
        fs::write(
            attempts
                .parent()
                .ok_or("pool parent missing")?
                .join(".lifecycle-gate"),
            b"gate",
        )?;
    }
    if include_work_object {
        let objects = root.join("indexes/lexical/lexical-object-pool/objects/ab");
        fs::create_dir_all(&objects)?;
        fs::write(objects.join("work"), b"xyz")?;
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
    let boot = authority.snapshot()?;
    let binder = PublicationFileCapacity::new(lease, authority.clone(), 16, 64, 256)?;
    let admission = CanonicalPublicationAdmission::new(
        Arc::new(
            crate::publication_file_deletions::PublicationFileDeletions::new(Arc::clone(&binder)),
        ),
        PublicationAllowanceLimit::new(3, 1)?,
        PublicationAllowanceLimit::new(1, 1)?,
    );
    Ok(Fixture {
        _directory: directory,
        path,
        authority,
        binder,
        admission,
        boot,
    })
}

fn control(
    admission: &CanonicalPublicationAdmission,
) -> Result<
    quanta_index_core::publication_admission::PublicationControlOutputAdmission,
    Box<dyn Error>,
> {
    let mut producer = SourceCorpusFixture::new(
        RepoId::new("control-resize")?,
        RevisionId::new("revision-1")?,
        ManifestGeneration::new(1),
        "control-resize-event".into(),
    );
    let batch = producer.frozen_batch()?;
    let mut owner = PublicationValidationOwner::default();
    owner.admit_publication_control(&batch, &RequestBudgetV1::unbounded(), admission)?;
    Ok(owner
        .output_admission()
        .owned_control_admission()?
        .ok_or("Control output authority missing")?)
}

fn open_writer(path: &Path) -> Result<File, Box<dyn Error>> {
    let parent = File::open(path.parent().ok_or("journal parent missing")?)?;
    let name = path.file_name().ok_or("journal leaf missing")?;
    Ok(File::from(openat(
        &parent,
        Path::new(name),
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?))
}

fn open_reader(path: &Path) -> Result<File, Box<dyn Error>> {
    let parent = File::open(path.parent().ok_or("Control reader parent missing")?)?;
    let name = path.file_name().ok_or("Control reader leaf missing")?;
    Ok(File::from(openat(
        &parent,
        Path::new(name),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?))
}

#[test]
fn control_gate_lock_is_held_by_the_owned_reader_until_checked_retirement() -> TestResult {
    let fixture = fixture_with_files(false, true)?;
    let gate = fixture
        .path
        .parent()
        .ok_or("attempt parent missing")?
        .parent()
        .ok_or("pool parent missing")?
        .join(".lifecycle-gate");
    let port = PublicationControlFiles::new(
        fixture.admission.file_deletions(),
        fixture.admission.object_lifetime(),
    );
    let first = port
        .begin_reader_open(&gate, control(&fixture.admission)?)?
        .opened_owned(open_reader(&gate)?)?;
    if first.metadata()?.len() != 4 {
        return Err("Control gate reader returned an unexpected physical file".into());
    }
    first.sync_posix()?;
    first.lock_exclusive()?;
    let second = port
        .begin_reader_open(&gate, control(&fixture.admission)?)?
        .opened_owned(open_reader(&gate)?)?;
    if second.try_lock_exclusive()? {
        return Err("second actual gate FD acquired a held exclusive lock".into());
    }
    first.retire()?;
    if !second.try_lock_exclusive()? {
        return Err("gate lock remained after checked first FD retirement".into());
    }
    second.retire()?;
    if fixture.authority.snapshot()? != fixture.boot || !fixture.binder.is_quiescent()? {
        return Err("gate reader lifecycle changed physical custody".into());
    }
    Ok(())
}

#[test]
fn cold_control_reader_rejects_foreign_paths_and_locks_the_original_gate_fd() -> TestResult {
    let foreign = fixture()?;
    let fixture = fixture_with_files(true, true)?;
    let lifetime = fixture.admission.object_lifetime();
    let gate = fixture
        .path
        .parent()
        .ok_or("attempt parent missing")?
        .parent()
        .ok_or("pool parent missing")?
        .join(".lifecycle-gate");
    let work = gate
        .parent()
        .ok_or("gate parent missing")?
        .join("objects/ab/work");
    let boot = fixture.authority.snapshot()?;
    let wrong_inode = lifetime
        .begin_control_reader_open(&gate)?
        .opened_owned(open_reader(&work)?)
        .err()
        .ok_or("cold Control accepted a different original FD")?;
    let work_path = lifetime
        .begin_control_reader_open(&work)
        .err()
        .ok_or("cold Control accepted a Work pathname")?;
    let foreign_path = lifetime
        .begin_control_reader_open(&foreign.path)
        .err()
        .ok_or("cold Control accepted a foreign root")?;
    let parent_component = lifetime
        .begin_control_reader_open(&gate.with_file_name("..").join("attempts/journal"))
        .err()
        .ok_or("cold Control accepted a parent component")?;
    if !matches!(wrong_inode, CoreError::InvalidContract(_))
        || !matches!(work_path, CoreError::InvalidContract(_))
        || !matches!(foreign_path, CoreError::InvalidContract(_))
        || !matches!(parent_component, CoreError::InvalidContract(_))
    {
        return Err("cold Control refusal lost its contract cause".into());
    }

    let first = lifetime
        .begin_control_reader_open(&gate)?
        .opened_owned(open_reader(&gate)?)?;
    if first.metadata()?.len() != 4 {
        return Err("cold Control reader returned a different gate".into());
    }
    first.sync_posix()?;
    first.lock_exclusive()?;
    let second = lifetime
        .begin_control_reader_open(&gate)?
        .opened_owned(open_reader(&gate)?)?;
    if second.try_lock_exclusive()? {
        return Err("cold Control second gate FD acquired the held lock".into());
    }
    first.retire()?;
    if !second.try_lock_exclusive()? {
        return Err("cold Control lock survived checked original FD retirement".into());
    }
    second.retire()?;
    if fixture.authority.snapshot()? != boot || !fixture.binder.is_quiescent()? {
        return Err("cold Control reader changed physical charge or retained custody".into());
    }
    Ok(())
}

#[test]
fn cold_control_reader_obeys_the_shared_reader_pin_ceiling() -> TestResult {
    let fixture = fixture_with_files(false, true)?;
    let lifetime = fixture.admission.object_lifetime();
    let gate = fixture
        .path
        .parent()
        .ok_or("attempt parent missing")?
        .parent()
        .ok_or("pool parent missing")?
        .join(".lifecycle-gate");
    let mut readers = Vec::new();
    readers.try_reserve_exact(16)?;
    for _ in 0..16 {
        readers.push(
            lifetime
                .begin_control_reader_open(&gate)?
                .opened_owned(open_reader(&gate)?)?,
        );
    }
    let refused = lifetime
        .begin_control_reader_open(&gate)
        .err()
        .ok_or("cold Control reader exceeded the shared pin ceiling")?;
    if !matches!(refused, CoreError::Typed { code, .. } if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE)
    {
        return Err("cold Control reader pin refusal lost its budget code".into());
    }
    readers.pop().ok_or("missing held reader")?.retire()?;
    let replacement = lifetime
        .begin_control_reader_open(&gate)?
        .opened_owned(open_reader(&gate)?)?;
    replacement.retire()?;
    for reader in readers {
        reader.retire()?;
    }
    if fixture.authority.snapshot()? != fixture.boot || !fixture.binder.is_quiescent()? {
        return Err("cold Control reader pins did not retire to boot baseline".into());
    }
    Ok(())
}

#[test]
fn unlinked_control_journal_remains_charged_until_last_reader_retirement() -> TestResult {
    let fixture = fixture()?;
    let port = PublicationControlFiles::new(
        fixture.admission.file_deletions(),
        fixture.admission.object_lifetime(),
    );
    let mut reader = port
        .begin_reader_open(&fixture.path, control(&fixture.admission)?)?
        .opened_owned(open_reader(&fixture.path)?)?;
    let mut bytes = [0_u8; 3];
    reader.read_exact(&mut bytes)?;
    if bytes != *b"abc" || reader.metadata()?.len() != 3 {
        return Err("owned Control reader did not observe fixed journal bytes".into());
    }
    port.remove_file_and_sync(&fixture.path, control(&fixture.admission)?)?;
    let pinned = fixture.authority.snapshot()?;
    if fixture.path.exists()
        || pinned.used_bytes
            != fixture
                .boot
                .used_bytes
                .checked_add(3)
                .ok_or("held Control bytes overflow")?
        || pinned.used_files
            != fixture
                .boot
                .used_files
                .checked_add(1)
                .ok_or("held Control files overflow")?
        || pinned.retained_inodes != fixture.boot.retained_inodes
        || fixture.binder.is_quiescent()?
    {
        return Err("unlinked Control journal was refunded while its reader FD lived".into());
    }
    reader.retire()?;
    let retired = fixture.authority.snapshot()?;
    if retired.used_bytes
        != fixture
            .boot
            .used_bytes
            .checked_sub(3)
            .ok_or("boot bytes underflow")?
        || retired.used_files
            != fixture
                .boot
                .used_files
                .checked_sub(1)
                .ok_or("boot files underflow")?
        || retired.retained_inodes
            != fixture
                .boot
                .retained_inodes
                .checked_sub(1)
                .ok_or("boot inode underflow")?
        || !fixture.binder.is_quiescent()?
    {
        return Err("last checked Control reader retirement did not refund one inode".into());
    }
    Ok(())
}

#[test]
fn control_reader_rejects_wrong_inode_and_unauthorized_paths_before_open() -> TestResult {
    let local = fixture_with_work_object(true)?;
    let foreign = fixture()?;
    let work = local
        .path
        .parent()
        .ok_or("attempt parent missing")?
        .parent()
        .ok_or("pool parent missing")?
        .join("objects/ab/work");
    let port = PublicationControlFiles::new(
        local.admission.file_deletions(),
        local.admission.object_lifetime(),
    );
    let boot = local.authority.snapshot()?;
    let wrong = port
        .begin_reader_open(&local.path, control(&local.admission)?)?
        .opened_owned(open_reader(&work)?)
        .err()
        .ok_or("foreign inode was adopted as Control journal")?;
    let missing = port
        .begin_reader_open(
            &local.path.with_file_name("missing"),
            control(&local.admission)?,
        )
        .err()
        .ok_or("missing Control journal was admitted")?;
    let foreign_parent = port
        .begin_reader_open(&local.path, control(&foreign.admission)?)
        .err()
        .ok_or("foreign Control capability opened local journal")?;
    let escaped = port
        .begin_reader_open(&work, control(&local.admission)?)
        .err()
        .ok_or("Control reader accepted Work object path")?;
    if !matches!(wrong, CoreError::InvalidContract(_))
        || !matches!(missing, CoreError::NotFound(_))
        || !matches!(foreign_parent, CoreError::InvalidContract(_))
        || !matches!(escaped, CoreError::InvalidContract(_))
    {
        return Err("Control reader refusal lost its typed cause".into());
    }
    let mut valid = port
        .begin_reader_open(&local.path, control(&local.admission)?)?
        .opened_owned(open_reader(&local.path)?)?;
    let mut bytes = [0_u8; 3];
    valid.read_exact(&mut bytes)?;
    valid.retire()?;
    if bytes != *b"abc"
        || fs::read(&work)? != b"xyz"
        || local.authority.snapshot()? != boot
        || foreign.authority.snapshot()? != foreign.boot
        || !local.binder.is_quiescent()?
    {
        return Err("rejected Control reader changed namespace, charge or open custody".into());
    }
    Ok(())
}

#[test]
fn exact_control_growth_settles_after_original_fd_close() -> TestResult {
    let fixture = fixture()?;
    let port = PublicationControlFiles::new(
        fixture.admission.file_deletions(),
        fixture.admission.object_lifetime(),
    );
    let mut update = port.begin_update(
        &fixture.path,
        open_writer(&fixture.path)?,
        3,
        6,
        control(&fixture.admission)?,
    )?;
    let _position = update.seek(SeekFrom::End(0))?;
    update.write_all(b"def")?;
    update.sync_and_settle()?;
    let settled = fixture.authority.snapshot()?;
    if fs::read(&fixture.path)? != b"abcdef"
        || settled.used_bytes
            != fixture
                .boot
                .used_bytes
                .checked_add(3)
                .ok_or("byte overflow")?
        || settled.used_files != fixture.boot.used_files
        || settled.uncertain_pending != fixture.boot.uncertain_pending
        || !fixture.binder.is_quiescent()?
    {
        return Err("Control resize did not settle exact six-byte journal charge".into());
    }
    Ok(())
}

#[test]
fn funded_control_file_deletion_refunds_once_and_repeated_absence_is_free() -> TestResult {
    let fixture = fixture()?;
    let port = PublicationControlFiles::new(
        fixture.admission.file_deletions(),
        fixture.admission.object_lifetime(),
    );
    let mut update = port.begin_update(
        &fixture.path,
        open_writer(&fixture.path)?,
        3,
        6,
        control(&fixture.admission)?,
    )?;
    let _position = update.seek(SeekFrom::End(0))?;
    update.write_all(b"def")?;
    update.sync_and_settle()?;
    if fs::read(&fixture.path)? != b"abcdef" {
        return Err("funded Control journal did not commit fixed bytes".into());
    }
    port.remove_file_and_sync(&fixture.path, control(&fixture.admission)?)?;
    let removed = fixture.authority.snapshot()?;
    if fixture.path.exists()
        || removed.used_bytes
            != fixture
                .boot
                .used_bytes
                .checked_sub(3)
                .ok_or("boot byte oracle underflow")?
        || removed.used_files
            != fixture
                .boot
                .used_files
                .checked_sub(1)
                .ok_or("boot file oracle underflow")?
        || removed.retained_inodes
            != fixture
                .boot
                .retained_inodes
                .checked_sub(1)
                .ok_or("boot inode oracle underflow")?
        || !fixture.binder.is_quiescent()?
    {
        return Err("Control deletion did not refund the one actual inode".into());
    }
    port.remove_file_and_sync(&fixture.path, control(&fixture.admission)?)?;
    port.reconcile_deletions()?;
    if fixture.authority.snapshot()? != removed || fixture.path.exists() {
        return Err("repeated Control absence changed physical charge".into());
    }
    Ok(())
}

#[test]
fn foreign_control_parent_cannot_remove_local_journal() -> TestResult {
    let local = fixture()?;
    let foreign = fixture()?;
    let port = PublicationControlFiles::new(
        local.admission.file_deletions(),
        local.admission.object_lifetime(),
    );
    let refusal = port
        .remove_file_and_sync(&local.path, control(&foreign.admission)?)
        .err()
        .ok_or("foreign Control capability removed local journal")?;
    if !matches!(refusal, CoreError::InvalidContract(_))
        || fs::read(&local.path)? != b"abc"
        || local.authority.snapshot()? != local.boot
        || foreign.authority.snapshot()? != foreign.boot
        || !local.binder.is_quiescent()?
    {
        return Err("foreign Control capability changed local physical custody".into());
    }
    Ok(())
}

#[test]
fn control_capability_refuses_work_and_noncanonical_paths_before_mutation() -> TestResult {
    let fixture = fixture_with_work_object(true)?;
    let port = PublicationControlFiles::new(
        fixture.admission.file_deletions(),
        fixture.admission.object_lifetime(),
    );
    let attempts = fixture.path.parent().ok_or("attempt parent missing")?;
    let pool = attempts.parent().ok_or("pool parent missing")?;
    let work = pool.join("objects/ab/work");
    let sibling = pool.join("sibling/journal");
    let nested = attempts.join("nested/journal");
    let traversed = attempts.join("../attempts/journal");
    let foreign = tempfile::tempdir()?.path().join("journal");
    let boot = fixture.authority.snapshot()?;

    for path in [&work, &sibling, &nested, &traversed, &foreign] {
        let removed = port
            .remove_file_and_sync(path, control(&fixture.admission)?)
            .err()
            .ok_or("non-Control path was accepted for deletion")?;
        let writer_path = if path == &work { &work } else { &fixture.path };
        let updated = port
            .begin_update(
                path,
                open_writer(writer_path)?,
                3,
                6,
                control(&fixture.admission)?,
            )
            .err()
            .ok_or("non-Control path was accepted for update")?;
        if !matches!(removed, CoreError::InvalidContract(_))
            || !matches!(updated, CoreError::InvalidContract(_))
            || fixture.authority.snapshot()? != boot
            || fs::read(&fixture.path)? != b"abc"
            || fs::read(&work)? != b"xyz"
            || !fixture.binder.is_quiescent()?
        {
            return Err("Control path refusal changed namespace or physical custody".into());
        }
    }
    Ok(())
}

#[test]
fn control_bound_refuses_before_physical_write_or_growth() -> TestResult {
    let fixture = fixture()?;
    let port = PublicationControlFiles::new(
        fixture.admission.file_deletions(),
        fixture.admission.object_lifetime(),
    );
    let control = control(&fixture.admission)?;
    let admitted = fixture.authority.snapshot()?;
    let beyond_parent = port
        .begin_update(
            &fixture.path,
            open_writer(&fixture.path)?,
            3,
            7,
            control.clone(),
        )
        .err()
        .ok_or("over-budget Control resize unexpectedly started")?;
    if !matches!(
        beyond_parent,
        CoreError::Typed { code, .. } if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
    ) || fs::read(&fixture.path)? != b"abc"
        || fixture.authority.snapshot()? != admitted
    {
        return Err("Control parent accepted over-budget journal growth".into());
    }
    let mut update =
        port.begin_update(&fixture.path, open_writer(&fixture.path)?, 3, 3, control)?;
    let _position = update.seek(SeekFrom::End(0))?;
    let refused = update
        .write_all(b"d")
        .err()
        .ok_or("out-of-bound write succeeded")?;
    if !matches!(
        refused
            .get_ref()
            .and_then(|source| source.downcast_ref::<CoreError>()),
            Some(CoreError::Typed { code, .. }) if *code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
    ) || fs::read(&fixture.path)? != b"abc"
    {
        return Err("Control write bound changed bytes or lost typed refusal".into());
    }
    update.sync_and_settle()?;
    if fixture.authority.snapshot()? != fixture.boot || !fixture.binder.is_quiescent()? {
        return Err("unchanged Control journal did not refund unused parent".into());
    }
    Ok(())
}

#[test]
fn stale_expected_journal_width_refuses_before_resize_claim() -> TestResult {
    let fixture = fixture()?;
    let port = PublicationControlFiles::new(
        fixture.admission.file_deletions(),
        fixture.admission.object_lifetime(),
    );
    let refusal = port
        .begin_update(
            &fixture.path,
            open_writer(&fixture.path)?,
            4,
            6,
            control(&fixture.admission)?,
        )
        .err()
        .ok_or("stale journal width was accepted")?;
    if !matches!(refusal, CoreError::InvalidContract(_))
        || fs::read(&fixture.path)? != b"abc"
        || fixture.authority.snapshot()? != fixture.boot
        || !fixture.binder.is_quiescent()?
    {
        return Err("stale journal preflight changed file or physical custody".into());
    }
    Ok(())
}

#[test]
fn dropped_partial_control_writer_recovers_on_same_path_retry() -> TestResult {
    let fixture = fixture()?;
    let port = PublicationControlFiles::new(
        fixture.admission.file_deletions(),
        fixture.admission.object_lifetime(),
    );
    let mut update = port.begin_update(
        &fixture.path,
        open_writer(&fixture.path)?,
        3,
        6,
        control(&fixture.admission)?,
    )?;
    let _position = update.seek(SeekFrom::End(0))?;
    update.write_all(b"d")?;
    drop(update);
    let pending = fixture.authority.snapshot()?;
    if fs::read(&fixture.path)? != b"abcd"
        || pending.uncertain_pending
            != fixture
                .boot
                .uncertain_pending
                .checked_add(1)
                .ok_or("pending overflow")?
        || fixture.binder.is_quiescent()?
    {
        return Err("partial Control update refunded an uncertain mutation".into());
    }
    let mut retry = port.begin_update(
        &fixture.path,
        open_writer(&fixture.path)?,
        4,
        6,
        control(&fixture.admission)?,
    )?;
    let _position = retry.seek(SeekFrom::End(0))?;
    retry.write_all(b"ef")?;
    retry.sync_and_settle()?;
    let settled = fixture.authority.snapshot()?;
    if fs::read(&fixture.path)? != b"abcdef"
        || settled.used_bytes
            != fixture
                .boot
                .used_bytes
                .checked_add(3)
                .ok_or("byte overflow")?
        || settled.used_files != fixture.boot.used_files
        || settled.uncertain_pending != fixture.boot.uncertain_pending
        || !fixture.binder.is_quiescent()?
    {
        return Err("same-path Control retry did not settle exact six bytes".into());
    }
    Ok(())
}

#[test]
fn sync_only_control_update_preserves_committed_six_byte_charge() -> TestResult {
    let fixture = fixture()?;
    let port = PublicationControlFiles::new(
        fixture.admission.file_deletions(),
        fixture.admission.object_lifetime(),
    );
    let mut growth = port.begin_update(
        &fixture.path,
        open_writer(&fixture.path)?,
        3,
        6,
        control(&fixture.admission)?,
    )?;
    let _position = growth.seek(SeekFrom::End(0))?;
    growth.write_all(b"def")?;
    growth.sync_and_settle()?;
    let committed = fixture.authority.snapshot()?;
    let sync_only = port.begin_update(
        &fixture.path,
        open_writer(&fixture.path)?,
        6,
        6,
        control(&fixture.admission)?,
    )?;
    sync_only.sync_and_settle()?;
    if fs::read(&fixture.path)? != b"abcdef"
        || fixture.authority.snapshot()? != committed
        || committed.used_bytes
            != fixture
                .boot
                .used_bytes
                .checked_add(3)
                .ok_or("byte overflow")?
        || !fixture.binder.is_quiescent()?
    {
        return Err("sync-only Control update changed bytes or inode charge".into());
    }
    Ok(())
}
