use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quanta_index_core::publication_admission::PublicationObjectLifetimePort as _;
use quanta_index_core::publication_capacity::PublicationCapacity;
use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, RequestBudgetV1};
use quanta_index_searchd::app::runtime::StateRootLease;

use super::PublicationLexicalLifetime;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn Error>>;

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    lexical_root: PathBuf,
    object_shard: PathBuf,
    authority: PublicationCapacity,
    binder: Arc<PublicationFileCapacity>,
    lifetime: PublicationLexicalLifetime,
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
    fixture_with_boot_objects(&[], 64, 16)
}

fn fixture_with_boot_objects(
    objects: &[(&str, &[u8])],
    max_tracked_entries: usize,
    max_open_handles: usize,
) -> Result<Fixture, Box<dyn Error>> {
    fixture_with_boot_objects_and_gate(objects, max_tracked_entries, max_open_handles, None)
}

fn fixture_with_boot_objects_and_gate(
    objects: &[(&str, &[u8])],
    max_tracked_entries: usize,
    max_open_handles: usize,
    gate_bytes: Option<&[u8]>,
) -> Result<Fixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let lexical_root = root.join("indexes").join("lexical");
    let object_shard = lexical_root
        .join("lexical-object-pool")
        .join("objects")
        .join("ab");
    fs::create_dir_all(&object_shard)?;
    if let Some(bytes) = gate_bytes {
        fs::write(
            lexical_root.join("lexical-object-pool/.lifecycle-gate"),
            bytes,
        )?;
    }
    for &(name, bytes) in objects {
        fs::write(object_shard.join(name), bytes)?;
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
    let binder = PublicationFileCapacity::new(
        lease,
        authority.clone(),
        max_open_handles,
        max_tracked_entries,
        256,
    )?;
    let lifetime = PublicationLexicalLifetime::new(Arc::clone(&binder));
    Ok(Fixture {
        _directory: directory,
        root,
        lexical_root,
        object_shard,
        authority,
        binder,
        lifetime,
    })
}

fn publish_file(binder: &PublicationFileCapacity, target: &Path, bytes: &[u8]) -> TestResult {
    let mut ticket = binder.begin_create(target)?;
    let staging = ticket.allocate_private_temp_path()?;
    ticket.reserve_end(u64::try_from(bytes.len())?)?;
    ticket.creation_started()?;
    let mut writer = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&staging)?;
    ticket.created(&writer, &staging)?;
    writer.write_all(bytes)?;
    ticket.before_replace(target)?;
    fs::rename(&staging, target)?;
    let _retained = ticket.replaced_and_synced(&writer, target)?;
    ticket.confirmed_safe_writer_fd_retired(writer)?;
    let lease = ticket.take_owned_lease()?;
    drop(ticket);
    lease.retire()?;
    Ok(())
}

#[test]
fn owned_reader_keeps_deleted_three_byte_inode_until_checked_retirement() -> TestResult {
    let fixture = fixture()?;
    let baseline = fixture.authority.snapshot()?;
    let path = fixture.object_shard.join("object-abc");
    publish_file(&fixture.binder, &path, b"abc")?;
    let charged = fixture.authority.snapshot()?;
    if charged.used_bytes != baseline.used_bytes.checked_add(3).ok_or("byte overflow")?
        || charged.used_files != baseline.used_files.checked_add(1).ok_or("file overflow")?
    {
        return Err("three-byte object did not consume one retained inode".into());
    }
    fixture
        .lifetime
        .validate_state_root(&fixture.lexical_root)?;
    let opening = fixture.lifetime.begin_reader_open(&path)?;
    let mut reader = opening.opened_owned(File::open(&path)?)?;
    if reader.metadata()?.len() != 3 {
        return Err("reader observed a different encoded width".into());
    }
    reader.sync_posix()?;
    let mut delete = fixture.lifetime.begin_delete(&path)?;
    let (source, parent) = delete.source_metadata()?;
    if source.len() != 3 || !parent.is_dir() {
        return Err("delete ticket did not retain source and parent metadata".into());
    }
    delete.deletion_started()?;
    fs::remove_file(&path)?;
    delete.unlinked_and_synced()?;
    if !delete.is_settled() || fixture.authority.snapshot()? != charged {
        return Err("last-reader pin did not retain the unlinked object's charge".into());
    }
    drop(delete);
    let position = reader.seek(SeekFrom::Start(0))?;
    if position != 0 {
        return Err("pinned reader did not rewind to the beginning".into());
    }
    let mut observed = [0_u8; 3];
    reader.read_exact(&mut observed)?;
    if observed != *b"abc" {
        return Err("pinned reader lost the original object's bytes".into());
    }
    reader.retire()?;
    if fixture.authority.snapshot()? != baseline || !fixture.binder.is_quiescent()? {
        return Err("last reader retirement did not refund the physical inode".into());
    }
    Ok(())
}

#[test]
fn foreign_parent_and_root_are_rejected_before_physical_work() -> TestResult {
    let local = fixture()?;
    let foreign = fixture()?;
    let local_baseline = local.authority.snapshot()?;
    let foreign_baseline = foreign.authority.snapshot()?;
    local.lifetime.validate_state_root(&local.lexical_root)?;
    if local.lifetime.validate_state_root(&local.root).is_ok()
        || local
            .lifetime
            .validate_state_root(&foreign.lexical_root)
            .is_ok()
    {
        return Err("foreign root was accepted".into());
    }
    if local
        .lifetime
        .begin_reader_open(&local.root.join("outside"))
        .is_ok()
        || local
            .lifetime
            .begin_delete(&local.lexical_root.join("outside"))
            .is_ok()
        || local
            .lifetime
            .begin_reader_open(&local.object_shard.join("..").join("outside"))
            .is_ok()
    {
        return Err("object path outside the canonical pool was accepted".into());
    }
    let foreign_parent = foreign.binder.reserve_publication_allowance(4, 1)?;
    if local.lifetime.validate_parent(&foreign_parent).is_ok() {
        return Err("foreign work parent was accepted".into());
    }
    let local_parent = local.binder.reserve_publication_allowance(4, 1)?;
    local.lifetime.validate_parent(&local_parent)?;
    drop(local_parent);
    drop(foreign_parent);
    if local.authority.snapshot()? != local_baseline
        || foreign.authority.snapshot()? != foreign_baseline
    {
        return Err("root or parent validation changed physical capacity".into());
    }
    Ok(())
}

#[test]
fn started_delete_ticket_can_retry_with_the_same_owned_observer() -> TestResult {
    let fixture = fixture()?;
    let baseline = fixture.authority.snapshot()?;
    let path = fixture.object_shard.join("retry-abc");
    publish_file(&fixture.binder, &path, b"abc")?;
    let charged = fixture.authority.snapshot()?;
    let mut delete = fixture.lifetime.begin_delete(&path)?;
    delete.deletion_started()?;
    if delete.unlinked_and_synced().is_ok()
        || !delete.has_started()
        || delete.is_settled()
        || fixture.authority.snapshot()? != charged
    {
        return Err("failed pre-unlink settlement lost started delete custody".into());
    }
    fs::remove_file(&path)?;
    delete.unlinked_and_synced()?;
    if !delete.is_settled() || fixture.authority.snapshot()? != baseline {
        return Err("retried started delete did not settle the exact inode".into());
    }
    drop(delete);
    if !fixture.binder.is_quiescent()? {
        return Err("settled delete left a path or reader claim".into());
    }
    Ok(())
}

#[test]
fn post_unlink_registry_refusal_preserves_observer_for_same_ticket_retry() -> TestResult {
    let other_names = [
        "other-0", "other-1", "other-2", "other-3", "other-4", "other-5", "other-6", "other-7",
    ];
    let boot_objects: [(&str, &[u8]); 9] = [
        ("other-0", b"xyz"),
        ("other-1", b"xyz"),
        ("other-2", b"xyz"),
        ("other-3", b"xyz"),
        ("other-4", b"xyz"),
        ("other-5", b"xyz"),
        ("other-6", b"xyz"),
        ("other-7", b"xyz"),
        ("target", b"abc"),
    ];
    // The canonical lexical pathname has six components below the leased
    // root. Eight retained inode pins fill the smallest usable registry cap.
    let fixture = fixture_with_boot_objects(&boot_objects, 8, 64)?;
    let baseline = fixture.authority.snapshot()?;
    let target = fixture.object_shard.join("target");
    let mut readers = Vec::new();
    for name in other_names {
        let other = fixture.object_shard.join(name);
        let opening = fixture.lifetime.begin_reader_open(&other)?;
        let reader = opening.opened_owned(File::open(&other)?)?;
        let mut other_delete = fixture.lifetime.begin_delete(&other)?;
        other_delete.deletion_started()?;
        fs::remove_file(&other)?;
        other_delete.unlinked_and_synced()?;
        if !other_delete.is_settled() || fixture.authority.snapshot()? != baseline {
            return Err("pinned boot inode was refunded before its last reader retired".into());
        }
        drop(other_delete);
        readers.push(reader);
    }

    let mut target_delete = fixture.lifetime.begin_delete(&target)?;
    target_delete.deletion_started()?;
    fs::remove_file(&target)?;
    if !matches!(
        target_delete.unlinked_and_synced(),
        Err(CoreError::Typed { code, message })
            if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
                && message == "publication file capacity: tracked inode ceiling exceeded"
    ) {
        return Err("full tracked-inode registry did not refuse target settlement".into());
    }
    let (source, parent) = target_delete.source_metadata()?;
    if source.len() != 3
        || source.nlink() != 0
        || !parent.is_dir()
        || !target_delete.has_started()
        || target_delete.is_settled()
        || fixture.authority.snapshot()? != baseline
    {
        return Err("failed post-unlink settlement lost the original source FD or charge".into());
    }

    readers.pop().ok_or("pinned reader missing")?.retire()?;
    let after_reader = fixture.authority.snapshot()?;
    if after_reader.used_bytes != baseline.used_bytes.checked_sub(3).ok_or("byte underflow")?
        || after_reader.used_files != baseline.used_files.checked_sub(1).ok_or("file underflow")?
    {
        return Err("retiring the old reader did not free the tracked-inode slot".into());
    }
    target_delete.unlinked_and_synced()?;
    if !target_delete.is_settled() {
        return Err("same target ticket did not settle after the slot was freed".into());
    }
    drop(target_delete);
    let after_target = fixture.authority.snapshot()?;
    if after_target.used_bytes != baseline.used_bytes.checked_sub(6).ok_or("byte underflow")?
        || after_target.used_files != baseline.used_files.checked_sub(2).ok_or("file underflow")?
    {
        return Err("same-ticket retry did not refund the exact target inode".into());
    }
    for reader in readers {
        reader.retire()?;
    }
    let settled = fixture.authority.snapshot()?;
    if settled.used_bytes
        != baseline
            .used_bytes
            .checked_sub(27)
            .ok_or("byte underflow")?
        || settled.used_files != baseline.used_files.checked_sub(9).ok_or("file underflow")?
        || !fixture.binder.is_quiescent()?
    {
        return Err("all nine boot inodes did not refund and release root custody".into());
    }
    Ok(())
}

#[test]
fn cold_control_gate_whitelist_locks_original_fd_until_checked_retirement() -> TestResult {
    let local = fixture_with_boot_objects_and_gate(&[], 64, 16, Some(b"gate"))?;
    let foreign = fixture_with_boot_objects_and_gate(&[], 64, 16, Some(b"other"))?;
    let gate = local
        .lexical_root
        .join("lexical-object-pool/.lifecycle-gate");
    let baseline = local.authority.snapshot()?;
    let foreign_baseline = foreign.authority.snapshot()?;
    if local
        .binder
        .path_observation(&gate)?
        .map(|entry| entry.bytes)
        != Some(4)
    {
        return Err("boot census did not retain the existing four-byte Gate".into());
    }
    let foreign_gate = foreign
        .lexical_root
        .join("lexical-object-pool/.lifecycle-gate");
    let non_control = local.object_shard.join("unlisted-object");
    let aliased = local
        .lexical_root
        .join("lexical-object-pool/objects/../.lifecycle-gate");
    for rejected in [&foreign_gate, &non_control, &aliased, &local.root] {
        if !matches!(
            local.lifetime.begin_control_reader_open(rejected),
            Err(CoreError::InvalidContract(_))
        ) {
            return Err("cold Control reader accepted a path outside its whitelist".into());
        }
    }
    if local.authority.snapshot()? != baseline || foreign.authority.snapshot()? != foreign_baseline
    {
        return Err("refused Control reader changed either physical authority".into());
    }

    let opening = local.lifetime.begin_control_reader_open(&gate)?;
    let mut reader = opening.opened_owned(File::open(&gate)?)?;
    if reader.metadata()?.len() != 4 {
        return Err("admitted Gate reader lost the inventoried width".into());
    }
    reader.lock_exclusive()?;
    if !reader.try_lock_exclusive()? {
        return Err("the original Gate FD did not retain its exclusive lock".into());
    }
    let contender_opening = local.lifetime.begin_control_reader_open(&gate)?;
    let contender = contender_opening.opened_owned(File::open(&gate)?)?;
    if contender.try_lock_exclusive()? {
        return Err("a second admitted Gate FD bypassed the original lock".into());
    }
    let mut observed = [0_u8; 4];
    reader.read_exact(&mut observed)?;
    if observed != *b"gate" {
        return Err("admitted Gate reader lost its original bytes".into());
    }
    reader.sync_posix()?;
    reader.retire()?;
    if !contender.try_lock_exclusive()? {
        return Err("checked retirement did not release the original Gate lock".into());
    }
    contender.retire()?;
    if local.authority.snapshot()? != baseline || !local.binder.is_quiescent()? {
        return Err("cold Gate readers retained a pin or altered boot charge".into());
    }
    Ok(())
}

#[test]
fn cold_control_readers_share_the_sixteen_pin_binder_ceiling() -> TestResult {
    let fixture = fixture_with_boot_objects_and_gate(&[], 64, 16, Some(b"gate"))?;
    let gate = fixture
        .lexical_root
        .join("lexical-object-pool/.lifecycle-gate");
    let baseline = fixture.authority.snapshot()?;
    let mut readers = Vec::new();
    for _ in 0..16 {
        let opening = fixture.lifetime.begin_control_reader_open(&gate)?;
        readers.push(opening.opened_owned(File::open(&gate)?)?);
    }
    if !matches!(
        fixture.lifetime.begin_control_reader_open(&gate),
        Err(CoreError::Typed { code, message })
            if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
                && message == "publication file capacity: reader-open ceiling exceeded"
    ) {
        return Err("seventeenth Control reader bypassed the shared pin ceiling".into());
    }
    if fixture.authority.snapshot()? != baseline {
        return Err("refused seventeenth Control reader changed the boot charge".into());
    }
    readers.pop().ok_or("Control reader missing")?.retire()?;
    let opening = fixture.lifetime.begin_control_reader_open(&gate)?;
    let replacement = opening.opened_owned(File::open(&gate)?)?;
    replacement.retire()?;
    for reader in readers {
        reader.retire()?;
    }
    if fixture.authority.snapshot()? != baseline || !fixture.binder.is_quiescent()? {
        return Err("retired Control readers left a physical pin or changed boot charge".into());
    }
    Ok(())
}
