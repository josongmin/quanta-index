use super::*;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{FileExt as _, OpenOptionsExt as _, PermissionsExt as _, symlink};

use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
use quanta_index_core::RequestBudgetV1;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Fixture = (
    tempfile::TempDir,
    PathBuf,
    StateRootLease,
    PublicationCapacity,
    Arc<PublicationFileCapacity>,
);

macro_rules! require {
    ($condition:expr) => {{
        if $condition {
            Ok::<(), std::io::Error>(())
        } else {
            Err(std::io::Error::other(concat!(
                "requirement failed: ",
                stringify!($condition)
            )))
        }
    }};
}

macro_rules! require_eq {
    ($actual:expr, $expected:expr) => {{
        let actual = $actual;
        let expected = $expected;
        if actual == expected {
            Ok::<(), std::io::Error>(())
        } else {
            Err(std::io::Error::other(format!(
                "{}: actual={actual:?}, expected={expected:?}",
                stringify!($actual)
            )))
        }
    }};
}

#[test]
fn safe_reader_lease_owns_actual_opened_fd_until_retirement() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let path = root.join("owned-reader");
    publish_file(&common, &path, b"four")?;
    let baseline = authority.snapshot()?;
    let guard = common.begin_reader_open(&path)?;
    let actual = File::open(&path)?;
    let descriptor = actual.as_raw_fd();
    let lease = guard.opened_owned(actual)?;
    require_eq!(lease.file()?.as_raw_fd(), descriptor)?;
    let mut deletion = common.begin_delete(&path)?;
    deletion.perform_and_sync()?;
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes)?;
    require!(!common.is_quiescent()?)?;
    lease.retire()?;
    common.sweep_retired()?;
    require_eq!(authority.snapshot()?.used_bytes, 0)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn safe_reader_rejects_wrong_actual_fd_without_open_guard_leak() -> TestResult {
    let (dir, root, _lease, _authority, common) = fixture()?;
    let path = root.join("owned-reader");
    publish_file(&common, &path, b"four")?;
    let wrong_path = dir.path().join("wrong-reader");
    fs::write(&wrong_path, b"other")?;
    let guard = common.begin_reader_open(&path)?;
    require!(guard.opened_owned(File::open(&wrong_path)?).is_err())?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn missing_reader_paths_are_typed_not_found_without_a_pin() -> TestResult {
    let (dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    require!(matches!(
        common.begin_reader_open(&root.join("missing-leaf")),
        Err(CoreError::NotFound(_))
    ))?;
    require!(matches!(
        common.begin_reader_open(&root.join("missing-parent/leaf")),
        Err(CoreError::NotFound(_))
    ))?;
    let external = dir.path().join("external-reader-parent");
    fs::create_dir(&external)?;
    symlink(&external, root.join("reader-symlink"))?;
    require!(matches!(
        common.begin_reader_open(&root.join("reader-symlink/leaf")),
        Err(CoreError::StorageIo {
            kind,
            raw_os_error: Some(errno),
            ..
        }) if (errno == rustix::io::Errno::LOOP.raw_os_error()
            || errno == rustix::io::Errno::NOTDIR.raw_os_error())
            && kind == std::io::Error::from_raw_os_error(errno).kind()
    ))?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn namespace_source_classification_is_rooted_nofollow_and_read_only() -> TestResult {
    let (dir, root, _lease, authority, common) = fixture()?;
    let regular = root.join("regular-source");
    publish_file(&common, &regular, b"abc")?;
    let directory = root.join("directory-source");
    fs::create_dir(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    symlink(&regular, root.join("symlink-source"))?;
    symlink(&directory, root.join("symlink-parent"))?;
    let foreign = dir.path().join("foreign-source");
    fs::write(&foreign, b"abc")?;
    let baseline = authority.snapshot()?;

    require!(!common.namespace_source_is_directory(&regular)?)?;
    require!(common.namespace_source_is_directory(&directory)?)?;
    require!(matches!(
        common.namespace_source_is_directory(&root.join("symlink-source")),
        Err(CoreError::InvalidContract(_))
    ))?;
    require!(
        common
            .namespace_source_is_directory(&root.join("symlink-parent/child"))
            .is_err()
    )?;
    require!(matches!(
        common.namespace_source_is_directory(&foreign),
        Err(CoreError::InvalidContract(_))
    ))?;
    require!(matches!(
        common.namespace_source_is_directory(&root.join("missing-source")),
        Err(CoreError::NotFound(_))
    ))?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn list_scope_accepts_root_and_absent_prefix_but_rejects_foreign_namespace() -> TestResult {
    let (dir, root, _lease, authority, common) = fixture()?;
    let inside = root.join("list-inside");
    let regular = inside.join("regular");
    publish_file(&common, &regular, b"abc")?;
    symlink(&regular, root.join("list-leaf-link"))?;
    symlink(&inside, root.join("list-parent-link"))?;
    let insecure = root.join("list-insecure-parent");
    fs::create_dir(&insecure)?;
    fs::set_permissions(&insecure, fs::Permissions::from_mode(0o777))?;
    let outside = dir.path().join("outside-list-prefix");
    fs::create_dir(&outside)?;
    let baseline = authority.snapshot()?;

    common.validate_list_scope(&root)?;
    common.validate_list_scope(&inside)?;
    common.validate_list_scope(&regular)?;
    common.validate_list_scope(&inside.join("missing-leaf"))?;
    common.validate_list_scope(&root.join("missing-parent/missing-leaf"))?;
    require!(matches!(
        common.validate_list_scope(&outside),
        Err(CoreError::InvalidContract(_))
    ))?;
    require!(matches!(
        common.validate_list_scope(&root.join("list-leaf-link")),
        Err(CoreError::InvalidContract(_))
    ))?;
    require!(matches!(
        common.validate_list_scope(&root.join("list-parent-link/leaf")),
        Err(CoreError::InvalidContract(_))
    ))?;
    require!(matches!(
        common.validate_list_scope(&insecure.join("leaf")),
        Err(CoreError::InvalidContract(_))
    ))?;
    require!(matches!(
        common.validate_list_scope(&root.join("list-inside/../list-inside")),
        Err(CoreError::InvalidContract(_))
    ))?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn root_handoff_freezes_new_publication_and_reader_admission() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    require!(common.is_handoff_ready_after_retirement()?)?;
    let before = authority.snapshot()?;
    require!(common.refill_maintenance_reserve().is_err())?;
    require_eq!(authority.snapshot()?, before)?;
    require!(common.begin_create(&root.join("new-file")).is_err())?;
    require!(
        common
            .begin_reader_open(&root.join("existing-file"))
            .is_err()
    )?;
    require!(
        common
            .begin_directory_delete(&root.join("directory"), &mut || Ok(()))
            .is_err()
    )?;
    Ok(())
}

#[test]
fn supplied_parent_is_subdivided_without_second_global_charge() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let parent = Arc::new(authority.reserve_publication(12, 1)?);
    let charged = authority.snapshot()?;
    let child = common.begin_create_with_allowance(&root.join("child"), 12, Arc::clone(&parent))?;
    let during = authority.snapshot()?;
    require_eq!(during.used_bytes, charged.used_bytes)?;
    require_eq!(during.used_files, charged.used_files)?;
    drop(child);
    drop(parent);
    require_eq!(authority.snapshot()?.used_bytes, boot.used_bytes)?;
    require_eq!(authority.snapshot()?.used_files, boot.used_files)?;
    Ok(())
}

#[test]
fn supplied_parent_from_foreign_authority_is_rejected() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let (_foreign_dir, _foreign_root, _foreign_lease, foreign, _foreign_common) = fixture()?;
    let parent = Arc::new(foreign.reserve_publication(12, 1)?);
    let before = authority.snapshot()?;
    require!(
        common
            .begin_create_with_allowance(&root.join("foreign"), 12, parent)
            .is_err()
    )?;
    require_eq!(authority.snapshot()?.used_bytes, before.used_bytes)?;
    require_eq!(authority.snapshot()?.used_files, before.used_files)?;
    require!(!root.join("foreign").exists())?;
    Ok(())
}

fn visible_writer(
    common: &PublicationFileCapacity,
    path: &Path,
    parent: Arc<PublicationAllowance>,
) -> Result<(FileMutation, File), Box<dyn std::error::Error>> {
    let mut ticket = common.begin_visible_create_with_allowance(path, parent)?;
    ticket.creation_started()?;
    let writer = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    ticket.created_visible(&writer)?;
    Ok((ticket, writer))
}

#[test]
fn visible_create_rejects_existing_name_without_child_charge() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let target = root.join("existing-visible");
    publish_file(&common, &target, b"abc")?;
    let boot = authority.snapshot()?;
    let parent = common.reserve_publication_allowance(4, 1)?;
    require!(matches!(
        common.begin_visible_create_with_allowance(&target, Arc::clone(&parent)),
        Err(CoreError::StorageIo {
            kind: std::io::ErrorKind::AlreadyExists,
            ..
        })
    ))?;
    drop(parent);
    require_eq!(fs::read(&target)?, b"abc")?;
    require_eq!(authority.snapshot()?, boot)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn visible_original_fd_growth_settles_only_after_reserved_extent_and_checked_close() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("visible-abc");
    let parent = common.reserve_publication_allowance(4, 1)?;
    let (mut ticket, mut writer) = visible_writer(&common, &target, Arc::clone(&parent))?;
    ticket.reserve_end(3)?;
    writer.write_all(b"abc")?;
    let _retained = ticket.settle_after_file_and_parent_sync(&writer)?;
    require!(!common.is_quiescent()?)?;
    require!(common.begin_delete(&target).is_err())?;
    ticket.confirmed_safe_writer_fd_retired(writer)?;
    ticket.confirmed_safe_pin_retired()?;
    drop(ticket);
    drop(parent);
    let charged = authority.snapshot()?;
    require_eq!(
        charged.used_bytes,
        boot.used_bytes.checked_add(3).ok_or("byte overflow")?
    )?;
    require_eq!(
        charged.used_files,
        boot.used_files.checked_add(1).ok_or("file overflow")?
    )?;
    require_eq!(fs::read(&target)?, b"abc")?;
    let mut deletion = common.begin_delete(&target)?;
    deletion.perform_and_sync()?;
    drop(deletion);
    require_eq!(authority.snapshot()?, boot)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn visible_create_cancellation_unlinks_only_owned_original_and_refunds() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("cancel-visible");
    let parent = common.reserve_publication_allowance(4, 1)?;
    let (mut ticket, mut writer) = visible_writer(&common, &target, Arc::clone(&parent))?;
    ticket.reserve_end(3)?;
    writer.write_all(b"abc")?;
    ticket.cancel_visible_create(writer)?;
    drop(ticket);
    drop(parent);
    require!(!target.exists())?;
    require_eq!(authority.snapshot()?, boot)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn visible_create_hardlink_refuses_false_absence_and_keeps_pending_charge() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("hardlinked-visible");
    let alias = root.join("hardlinked-alias");
    let parent = common.reserve_publication_allowance(4, 1)?;
    let (mut ticket, mut writer) = visible_writer(&common, &target, Arc::clone(&parent))?;
    ticket.reserve_end(3)?;
    writer.write_all(b"abc")?;
    fs::hard_link(&target, &alias)?;
    require!(ticket.cancel_visible_create(writer).is_err())?;
    drop(ticket);
    drop(parent);
    require_eq!(fs::read(&target)?, b"abc")?;
    require_eq!(fs::read(&alias)?, b"abc")?;
    require!(
        authority.snapshot()?.used_bytes
            >= boot.used_bytes.checked_add(3).ok_or("byte overflow")?
    )?;
    require!(!common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn visible_create_clone_cannot_cancel_original_but_original_can_refund() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("clone-visible");
    let parent = common.reserve_publication_allowance(4, 1)?;
    let (mut ticket, mut writer) = visible_writer(&common, &target, Arc::clone(&parent))?;
    ticket.reserve_end(3)?;
    writer.write_all(b"abc")?;
    let clone = writer.try_clone()?;
    require!(ticket.cancel_visible_create(clone).is_err())?;
    require_eq!(fs::read(&target)?, b"abc")?;
    ticket.cancel_visible_create(writer)?;
    drop(ticket);
    drop(parent);
    require!(!target.exists())?;
    require_eq!(authority.snapshot()?, boot)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn visible_create_replaced_path_cannot_unlink_winner_or_refund_original() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("replaced-visible");
    let moved = root.join("moved-original");
    let parent = common.reserve_publication_allowance(4, 1)?;
    let (mut ticket, mut writer) = visible_writer(&common, &target, Arc::clone(&parent))?;
    ticket.reserve_end(3)?;
    writer.write_all(b"abc")?;
    fs::rename(&target, &moved)?;
    fs::write(&target, b"new")?;
    require!(ticket.cancel_visible_create(writer).is_err())?;
    drop(ticket);
    drop(parent);
    require_eq!(fs::read(&target)?, b"new")?;
    require_eq!(fs::read(&moved)?, b"abc")?;
    require!(
        authority.snapshot()?.used_bytes
            >= boot.used_bytes.checked_add(3).ok_or("byte overflow")?
    )?;
    require!(!common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_visible_writer_reconciles_from_actual_anchored_fd() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("recover-visible");
    let parent = common.reserve_publication_allowance(4, 1)?;
    let (mut ticket, mut writer) = visible_writer(&common, &target, Arc::clone(&parent))?;
    ticket.reserve_end(3)?;
    writer.write_all(b"abc")?;
    ticket.confirmed_safe_writer_fd_retired(writer)?;
    drop(ticket);
    require!(common.has_retired_safe_visible_create_pending(&target)?)?;
    require!(!common.is_quiescent()?)?;
    let actual = File::open(&target)?;
    let _retained = common.recover_pending_with_fd(&target, &target, actual)?;
    drop(parent);
    require_eq!(fs::read(&target)?, b"abc")?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        boot.used_bytes.checked_add(3).ok_or("byte overflow")?
    )?;
    let mut deletion = common.begin_delete(&target)?;
    deletion.perform_and_sync()?;
    drop(deletion);
    require_eq!(authority.snapshot()?, boot)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_visible_recovery_rechecks_leaf_and_parent_after_sync() -> TestResult {
    for replace_parent in [false, true] {
        let (_dir, root, _lease, authority, common) = fixture()?;
        let shard = root.join("recovery-shard");
        fs::create_dir(&shard)?;
        fs::set_permissions(&shard, fs::Permissions::from_mode(0o700))?;
        let target = shard.join("object");
        let parent = common.reserve_publication_allowance(4, 1)?;
        let (mut ticket, mut writer) = visible_writer(&common, &target, Arc::clone(&parent))?;
        ticket.reserve_end(3)?;
        writer.write_all(b"abc")?;
        ticket.confirmed_safe_writer_fd_retired(writer)?;
        drop(ticket);
        let pending = authority.snapshot()?;
        require_eq!(pending.uncertain_pending, 1)?;
        let held = common.anchor(&target)?;
        let actual = File::open(&target)?;
        let observed = InodeObservation::from_file(&actual)?;
        actual.sync_all()?;
        held.sync_parent()?;
        common.revalidate_recovery_witness_after_sync(&held, &target, &actual, observed)?;
        if replace_parent {
            fs::rename(&shard, root.join("old-recovery-shard"))?;
            fs::create_dir(&shard)?;
            fs::set_permissions(&shard, fs::Permissions::from_mode(0o700))?;
        } else {
            fs::rename(&target, shard.join("old-object"))?;
        }
        let mut replacement = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&target)?;
        replacement.write_all(b"new")?;
        drop(replacement);
        require!(
            common
                .revalidate_recovery_witness_after_sync(&held, &target, &actual, observed)
                .is_err()
        )?;
        require_eq!(authority.snapshot()?, pending)?;
        require!(common.has_retired_safe_visible_create_pending(&target)?)?;
        drop(parent);
    }
    Ok(())
}

#[test]
fn reserved_output_rejects_other_parent_and_started_ticket_before_path_claim() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let parent = Arc::new(authority.reserve_publication(4, 1)?);
    let other_parent = Arc::new(authority.reserve_publication(4, 1)?);
    let path = root.join("reserved-output-rejected");
    let wrong_parent_ticket = parent.reserve_output(4)?;
    require!(
        common
            .begin_create_with_reserved_output(
                &path,
                4,
                Arc::clone(&other_parent),
                wrong_parent_ticket,
            )
            .is_err()
    )?;
    require!(!path.exists())?;
    require_eq!(authority.snapshot()?.uncertain_pending, 0)?;
    let other_child = other_parent.reserve_output(4)?;
    drop(other_child);
    // The rejected reserved ticket refunds only its own unstarted child.
    let started = parent.reserve_output(4)?;
    started.creation_started()?;
    let pending_id = started.id();
    let pending = authority.snapshot()?;
    require!(
        common
            .begin_create_with_reserved_output(&path, 4, Arc::clone(&parent), started)
            .is_err()
    )?;
    require!(!path.exists())?;
    require_eq!(authority.snapshot()?.used_bytes, pending.used_bytes)?;
    require_eq!(authority.snapshot()?.used_files, pending.used_files)?;
    require_eq!(authority.snapshot()?.uncertain_pending, 1)?;
    std::fs::File::open(&root)?.sync_all()?;
    authority.confirmed_pending_absent_and_synced(pending_id)?;
    drop(parent);
    drop(other_parent);
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes)?;
    require_eq!(authority.snapshot()?.used_files, baseline.used_files)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn supplied_copy_and_resize_use_parent_without_top_up() -> TestResult {
    let (dir, root, _lease, authority, common) = fixture()?;
    let source_path = dir.path().join("source");
    fs::write(&source_path, b"four")?;
    let source = File::open(&source_path)?;
    let copy_parent = Arc::new(authority.reserve_publication(4, 1)?);
    let charged = authority.snapshot()?;
    let copy = common.begin_byte_copy_with_allowance(
        &source,
        &root.join("copy"),
        Arc::clone(&copy_parent),
    )?;
    require_eq!(authority.snapshot()?.used_bytes, charged.used_bytes)?;
    drop(copy);
    drop(copy_parent);

    let path = root.join("existing");
    publish_file(&common, &path, b"four")?;
    let opened = OpenOptions::new().read(true).write(true).open(&path)?;
    let resize_parent = Arc::new(authority.reserve_publication(8, 0)?);
    let charged = authority.snapshot()?;
    let resize =
        common.begin_resize_with_allowance(&path, &opened, 12, Arc::clone(&resize_parent))?;
    require_eq!(authority.snapshot()?.used_bytes, charged.used_bytes)?;
    drop(resize);
    rusqlite::ffi::quanta_capacity_vfs::close_owned_file(opened)?;
    drop(resize_parent);
    require_eq!(authority.snapshot()?.used_bytes, 4)?;
    Ok(())
}

fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with_tracking_limit(64)
}

fn fixture_with_tracking_limit(
    max_tracked_entries: usize,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let requested_root = dir.path().join("state");
    fs::create_dir(&requested_root)?;
    fs::set_permissions(&requested_root, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested_root)?;
    // macOS may give tempdir() a /var alias while the lease owns /private/var.
    // Build all admitted paths from the lease's canonical root identity.
    let root = lease.state_root_identity_v1().to_path_buf();
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
    let common = PublicationFileCapacity::new(
        lease.clone(),
        authority.clone(),
        16,
        max_tracked_entries,
        256,
    )?;
    Ok((dir, root, lease, authority, common))
}

#[test]
fn retired_control_create_reconciles_with_only_one_tracked_path_slot() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture_with_tracking_limit(1)?;
    let boot = authority.snapshot()?;
    let (target, temporary, _born) = retired_control_create(&common, &root)?;
    fs::rename(&temporary, &target)?;
    require!(matches!(
        common.recover_retired_control_create(&target)?,
        ControlCreateRecovery::RetainedTarget
    ))?;
    require_eq!(fs::read(&target)?, b"four")?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        boot.used_bytes.checked_add(4).ok_or("byte overflow")?
    )?;
    require_eq!(
        authority.snapshot()?.uncertain_pending,
        boot.uncertain_pending
    )?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_control_replacement_before_rename_retains_old_and_temporary() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("control-journal");
    publish_file(&common, &target, b"old")?;
    let (target, temporary, born) = retired_control_create(&common, &root)?;
    require!(matches!(
        common.recover_retired_control_create(&target)?,
        ControlCreateRecovery::RetainedTemporary { path, inode, bytes }
            if path == temporary && inode == born.inode && bytes == 4
    ))?;
    require_eq!(fs::read(&target)?, b"old")?;
    require_eq!(fs::read(&temporary)?, b"four")?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        boot.used_bytes.checked_add(7).ok_or("byte overflow")?
    )?;
    require_eq!(
        authority.snapshot()?.used_files,
        boot.used_files.checked_add(2).ok_or("file overflow")?
    )?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_control_replacement_after_rename_waits_for_old_reader() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    common.sweep_retired()?;
    let boot = authority.snapshot()?;
    let target = root.join("control-journal");
    publish_file(&common, &target, b"old")?;
    let reader = common
        .begin_reader_open(&target)?
        .opened_owned(File::open(&target)?)?;
    let (target, temporary, _born) = retired_control_create(&common, &root)?;
    fs::rename(&temporary, &target)?;
    require!(matches!(
        common.recover_retired_control_create(&target)?,
        ControlCreateRecovery::RetainedTarget
    ))?;
    let mut old_bytes = [0_u8; 3];
    require_eq!(reader.file()?.read_at(&mut old_bytes, 0)?, 3)?;
    require_eq!(old_bytes, *b"old")?;
    require_eq!(fs::read(&target)?, b"four")?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        boot.used_bytes.checked_add(7).ok_or("byte overflow")?
    )?;
    reader.retire()?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        boot.used_bytes.checked_add(4).ok_or("byte overflow")?
    )?;
    require_eq!(
        authority.snapshot()?.used_files,
        boot.used_files.checked_add(1).ok_or("file overflow")?
    )?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_control_replacement_preserves_old_hardlink_until_alias_delete() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("control-journal");
    let alias = root.join("old-alias");
    publish_file(&common, &target, b"old")?;
    common.begin_link(&target, &alias)?.perform_and_sync()?;
    let (target, temporary, _born) = retired_control_create(&common, &root)?;
    fs::rename(&temporary, &target)?;
    require!(matches!(
        common.recover_retired_control_create(&target)?,
        ControlCreateRecovery::RetainedTarget
    ))?;
    require_eq!(fs::read(&alias)?, b"old")?;
    require_eq!(fs::read(&target)?, b"four")?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        boot.used_bytes.checked_add(7).ok_or("byte overflow")?
    )?;
    common.begin_delete(&alias)?.perform_and_sync()?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        boot.used_bytes.checked_add(4).ok_or("byte overflow")?
    )?;
    require_eq!(
        authority.snapshot()?.used_files,
        boot.used_files.checked_add(1).ok_or("file overflow")?
    )?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_control_replacement_absent_born_refunds_only_new_child() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("control-journal");
    publish_file(&common, &target, b"old")?;
    let old_charge = authority.snapshot()?;
    let (target, temporary, _born) = retired_control_create(&common, &root)?;
    fs::remove_file(&temporary)?;
    require!(matches!(
        common.recover_retired_control_create(&target)?,
        ControlCreateRecovery::Absent
    ))?;
    require_eq!(fs::read(&target)?, b"old")?;
    require_eq!(authority.snapshot()?, old_charge)?;
    require_eq!(
        old_charge.used_bytes,
        boot.used_bytes.checked_add(3).ok_or("byte overflow")?
    )?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_control_replacement_rejects_foreign_target_without_releasing_old() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let target = root.join("control-journal");
    publish_file(&common, &target, b"old")?;
    let (target, temporary, _born) = retired_control_create(&common, &root)?;
    let pending = authority.snapshot()?;
    fs::rename(&target, root.join("old-displaced"))?;
    fs::write(&target, b"evil")?;
    require!(common.recover_retired_control_create(&target).is_err())?;
    require_eq!(authority.snapshot()?, pending)?;
    require_eq!(fs::read(&target)?, b"evil")?;
    require_eq!(fs::read(&temporary)?, b"four")?;
    require!(!common.is_quiescent()?)?;
    Ok(())
}

fn retired_control_create(
    common: &PublicationFileCapacity,
    root: &Path,
) -> Result<(PathBuf, PathBuf, InodeObservation), Box<dyn std::error::Error>> {
    let target = root.join("control-journal");
    let mut ticket = common.begin_create(&target)?;
    let temporary = ticket.allocate_private_temp_path()?;
    ticket.retain_control_create_for_recovery()?;
    ticket.reserve_end(8)?;
    ticket.creation_started()?;
    let mut writer = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    ticket.created(&writer, &temporary)?;
    writer.write_all(b"four")?;
    let born = InodeObservation::from_file(&writer)?;
    ticket.confirmed_safe_writer_fd_retired(writer)?;
    drop(ticket);
    Ok((target, temporary, born))
}

#[test]
fn retired_control_create_reconciles_published_target_once() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let (target, temporary, born) = retired_control_create(&common, &root)?;
    fs::rename(&temporary, &target)?;
    require_eq!(common.next_retired_control_create()?, Some(target.clone()))?;
    require!(matches!(
        common.recover_retired_control_create(&target)?,
        ControlCreateRecovery::RetainedTarget
    ))?;
    require_eq!(fs::read(&target)?, b"four")?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        boot.used_bytes.checked_add(4).ok_or("byte overflow")?
    )?;
    require_eq!(
        authority.snapshot()?.uncertain_pending,
        boot.uncertain_pending
    )?;
    require!(authority.retained_inode(born.inode, 4)?.is_some())?;
    let settled = authority.snapshot()?;
    require_eq!(common.next_retired_control_create()?, None)?;
    require!(common.recover_retired_control_create(&target).is_err())?;
    require_eq!(authority.snapshot()?, settled)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_control_create_retains_unpublished_temporary_charge() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let (target, temporary, born) = retired_control_create(&common, &root)?;
    let recovered = common.recover_retired_control_create(&target)?;
    require!(matches!(
        recovered,
        ControlCreateRecovery::RetainedTemporary { path, inode, bytes }
            if path == temporary && inode == born.inode && bytes == 4
    ))?;
    require!(!target.exists())?;
    require_eq!(fs::read(&temporary)?, b"four")?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        boot.used_bytes.checked_add(4).ok_or("byte overflow")?
    )?;
    require_eq!(
        authority.snapshot()?.uncertain_pending,
        boot.uncertain_pending
    )?;
    require!(authority.retained_inode(born.inode, 4)?.is_some())?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_control_create_refunds_only_witnessed_zero_link_absence() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let (target, temporary, born) = retired_control_create(&common, &root)?;
    fs::remove_file(&temporary)?;
    require!(matches!(
        common.recover_retired_control_create(&target)?,
        ControlCreateRecovery::Absent
    ))?;
    require!(!target.exists() && !temporary.exists())?;
    require_eq!(authority.snapshot()?, boot)?;
    require_eq!(authority.retained_inode(born.inode, 4)?, None)?;
    require_eq!(common.next_retired_control_create()?, None)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_control_create_rejects_foreign_replacement_and_hardlink_alias() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let (target, temporary, _born) = retired_control_create(&common, &root)?;
    let pending = authority.snapshot()?;
    fs::write(&target, b"evil")?;
    require!(common.recover_retired_control_create(&target).is_err())?;
    require_eq!(authority.snapshot()?, pending)?;
    require_eq!(fs::read(&target)?, b"evil")?;
    fs::remove_file(&target)?;
    fs::hard_link(&temporary, &target)?;
    require!(common.recover_retired_control_create(&target).is_err())?;
    require_eq!(authority.snapshot()?, pending)?;
    require_eq!(fs::read(&temporary)?, b"four")?;
    require_eq!(fs::read(&target)?, b"four")?;
    Ok(())
}

#[test]
fn live_control_create_writer_cannot_be_recovered_or_refunded() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let boot = authority.snapshot()?;
    let target = root.join("control-live");
    let mut ticket = common.begin_create(&target)?;
    let temporary = ticket.allocate_private_temp_path()?;
    ticket.retain_control_create_for_recovery()?;
    ticket.reserve_end(8)?;
    ticket.creation_started()?;
    let mut writer = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    ticket.created(&writer, &temporary)?;
    writer.write_all(b"four")?;
    drop(ticket);
    require_eq!(common.next_retired_control_create()?, None)?;
    require!(common.recover_retired_control_create(&target).is_err())?;
    require_eq!(writer.write_at(b"F", 0)?, 1)?;
    require_eq!(fs::read(&temporary)?, b"Four")?;
    require_eq!(
        authority.snapshot()?.uncertain_pending,
        boot.uncertain_pending
            .checked_add(1)
            .ok_or("pending overflow")?
    )?;
    require!(!common.is_quiescent()?)?;
    rusqlite::ffi::quanta_capacity_vfs::close_owned_file(writer)?;
    Ok(())
}

#[test]
fn empty_lock_sync_failure_keeps_charge_but_records_actual_close_for_disposal() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let directory = root.join("scratch-lock-failure");
    let path = directory.join(".lease");
    common.ensure_private_parent_directories(&path)?;
    let mut ticket = common.begin_native_create(&path)?;
    ticket.creation_started()?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    file.try_lock()?;
    ticket.observe_native_fd(InodeObservation::from_file(&file)?)?;
    let result = common.retire_empty_lock(
        &mut ticket,
        file,
        Err(CoreError::Storage(
            "injected empty-lock sync failure".into(),
        )),
    );
    require!(
        matches!(result, Err(CoreError::Storage(message)) if message == "injected empty-lock sync failure")
    )?;
    drop(ticket);
    // This is a synthetic sync-result failure, with an actual original close.
    // Independent re-lock and fixed charges check both sides of that boundary.
    let reopened = OpenOptions::new().read(true).write(true).open(&path)?;
    reopened.try_lock()?;
    require_eq!(reopened.metadata()?.len(), 0)?;
    close_owned_file(reopened)?;
    let pending = authority.snapshot()?;
    require_eq!(pending.used_bytes, baseline.used_bytes + 1)?;
    require_eq!(pending.used_files, baseline.used_files + 1)?;
    require_eq!(pending.uncertain_pending, 1)?;
    common.discard_retired_native_pending_children(&directory, &mut || Ok(()))?;
    require!(!path.exists())?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_scratch_pending_disposal_stops_before_next_child_and_retries() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let directory = root.join("scratch-pending-prefix");
    let first = directory.join("a");
    let second = directory.join("b");
    for path in [&first, &second] {
        common.ensure_private_parent_directories(path)?;
        let mut ticket = common.begin_native_create(path)?;
        ticket.creation_started()?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        ticket.observe_native_fd(InodeObservation::from_file(&file)?)?;
        let result = common.retire_empty_lock(
            &mut ticket,
            file,
            Err(CoreError::Storage("injected pending close result".into())),
        );
        require!(result.is_err())?;
        drop(ticket);
    }
    let pending = authority.snapshot()?;
    require_eq!(pending.uncertain_pending, 2)?;
    let mut denied = || Err(CoreError::InvalidContract("stop before child".into()));
    require!(
        common
            .discard_retired_native_pending_children(&directory, &mut denied)
            .is_err()
    )?;
    require!(first.exists() && second.exists())?;
    require_eq!(authority.snapshot()?, pending)?;

    let mut visits = 0usize;
    let mut stop_after_prefix = || {
        visits = visits.saturating_add(1);
        if visits == 2 {
            Err(CoreError::InvalidContract(
                "stop after settled prefix".into(),
            ))
        } else {
            Ok(())
        }
    };
    require!(
        common
            .discard_retired_native_pending_children(&directory, &mut stop_after_prefix)
            .is_err()
    )?;
    require_eq!(visits, 2)?;
    require!(!first.exists())?;
    require!(second.exists())?;
    require_eq!(authority.snapshot()?.uncertain_pending, 1)?;
    common.discard_retired_native_pending_children(&directory, &mut || Ok(()))?;
    require!(!second.exists())?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn directory_delete_refusal_before_destructive_phase_preserves_children() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let directory = root.join("scratch-delete-admission");
    let first = directory.join("first");
    let second = directory.join("second");
    publish_file(&common, &first, b"one")?;
    publish_file(&common, &second, b"two")?;
    let original = authority.snapshot()?;
    let mut scan_visits = 0usize;
    let mut stop_scan = || {
        scan_visits = scan_visits.saturating_add(1);
        if scan_visits == 2 {
            Err(CoreError::InvalidContract("stop directory scan".into()))
        } else {
            Ok(())
        }
    };
    require!(
        common
            .begin_directory_delete(&directory, &mut stop_scan)
            .is_err()
    )?;
    require_eq!(fs::read(&first)?, b"one")?;
    require_eq!(fs::read(&second)?, b"two")?;
    require_eq!(authority.snapshot()?, original)?;

    let mut ticket = common.begin_directory_delete(&directory, &mut || Ok(()))?;
    let mut stop_preflight = || Err(CoreError::InvalidContract("stop before unlink".into()));
    require!(ticket.perform_and_sync(&mut stop_preflight).is_err())?;
    drop(ticket);
    require_eq!(fs::read(&first)?, b"one")?;
    require_eq!(fs::read(&second)?, b"two")?;
    require_eq!(authority.snapshot()?, original)?;
    let mut retry = common.begin_directory_delete(&directory, &mut || Ok(()))?;
    retry.perform_and_sync(&mut || Ok(()))?;
    require!(!directory.exists())?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn provisional_create_drop_refunds_without_creating_file() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let final_path = root.join("artifact");
    let ticket = common.begin_create(&final_path)?;
    require!(
        authority.snapshot()?.used_bytes
            >= baseline
                .used_bytes
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("expected byte growth overflow"))?
    )?;
    drop(ticket);
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes)?;
    require_eq!(authority.snapshot()?.used_files, baseline.used_files)?;
    require!(!final_path.exists())?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn real_create_delete_retains_inode_until_reader_fd_retires() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    common.sweep_retired()?;
    let baseline = authority.snapshot()?;
    let final_path = root.join("artifact");
    let mut ticket = common.begin_create(&final_path)?;
    let temp_path = ticket.allocate_private_temp_path()?;
    ticket.creation_started()?;
    let mut file = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&temp_path)?;
    ticket.created(&file, &temp_path)?;
    ticket.reserve_end(4)?;
    file.write_all(b"four")?;
    ticket.before_replace(&final_path)?;
    fs::rename(&temp_path, &final_path)?;
    let _retained = ticket.replaced_and_synced(&file, &final_path)?;
    ticket.confirmed_safe_writer_fd_retired(file)?;
    let reader = ticket.take_owned_lease()?;
    drop(ticket);
    require_eq!(fs::metadata(&final_path)?.len(), 4)?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        baseline
            .used_bytes
            .checked_add(4)
            .ok_or_else(|| std::io::Error::other("expected byte growth overflow"))?
    )?;
    let mut delete = common.begin_delete(&final_path)?;
    delete.perform_and_sync()?;
    require!(!final_path.exists())?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        baseline
            .used_bytes
            .checked_add(4)
            .ok_or_else(|| std::io::Error::other("expected byte growth overflow"))?
    )?;
    reader.retire()?;
    common.sweep_retired()?;
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn overwrite_refunds_unpinned_old_inode_after_writer_transfer() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let path = root.join("overwrite");
    publish_file(&common, &path, b"old")?;
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes + 3)?;

    publish_file(&common, &path, b"newer")?;
    require_eq!(fs::read(&path)?, b"newer")?;
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes + 5)?;
    require_eq!(authority.snapshot()?.used_files, baseline.used_files + 1)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn overwrite_keeps_old_inode_until_its_reader_retires() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    common.sweep_retired()?;
    let baseline = authority.snapshot()?;
    let path = root.join("overwrite-pinned");
    publish_file(&common, &path, b"old")?;
    let guard = common.begin_reader_open(&path)?;
    let old_reader = guard.opened_owned(File::open(&path)?)?;

    publish_file(&common, &path, b"newer")?;
    let mut old_bytes = [0_u8; 3];
    require_eq!(old_reader.file()?.read_at(&mut old_bytes, 0)?, 3)?;
    require_eq!(old_bytes, *b"old")?;
    require_eq!(fs::read(&path)?, b"newer")?;
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes + 8)?;
    require_eq!(authority.snapshot()?.used_files, baseline.used_files + 2)?;

    drop(old_reader);
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes + 5)?;
    require_eq!(authority.snapshot()?.used_files, baseline.used_files + 1)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn symlink_parent_is_rejected_before_ticket_or_external_write() -> TestResult {
    let (dir, root, _lease, authority, common) = fixture()?;
    let external = dir.path().join("external");
    fs::create_dir(&external)?;
    symlink(&external, root.join("escape"))?;
    let baseline = authority.snapshot()?;
    require!(common.begin_create(&root.join("escape/file")).is_err())?;
    require!(!external.join("file").exists())?;
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes)?;
    Ok(())
}

#[test]
fn started_short_write_keeps_charge_until_anchored_recovery() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let final_path = root.join("artifact");
    let mut ticket = common.begin_create(&final_path)?;
    let temp_path = ticket.allocate_private_temp_path()?;
    ticket.reserve_end(8)?;
    ticket.creation_started()?;
    let mut file = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&temp_path)?;
    ticket.created(&file, &temp_path)?;
    file.write_all(b"four")?;
    // Inject a short-write outcome at the caller boundary. No settlement is
    // claimed; the real file has four bytes but eight remain reserved.
    let injected = std::io::Error::from_raw_os_error(28); // ENOSPC on macOS/Linux
    require_eq!(injected.raw_os_error(), Some(28))?;
    drop(ticket);
    require!(
        authority.snapshot()?.used_bytes
            >= baseline
                .used_bytes
                .checked_add(8)
                .ok_or_else(|| std::io::Error::other("expected byte growth overflow"))?
    )?;
    require!(!common.is_quiescent()?)?;
    require!(!common.is_handoff_ready_after_retirement()?)?;
    let recovered = common.recover_pending_with_fd(&final_path, &temp_path, file)?;
    let reopened = File::open(&temp_path)?;
    let observed = InodeObservation::from_file(&reopened)?;
    rusqlite::ffi::quanta_capacity_vfs::close_owned_file(reopened)?;
    require_eq!(
        authority.retained_inode(observed.inode, observed.bytes)?,
        Some(recovered)
    )?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        baseline
            .used_bytes
            .checked_add(4)
            .ok_or_else(|| std::io::Error::other("expected byte growth overflow"))?
    )?;
    require!(common.is_handoff_ready_after_retirement()?)?;
    require_eq!(fs::metadata(&temp_path)?.len(), 4)?;
    Ok(())
}

#[test]
fn cloned_writer_fd_cannot_retire_or_recover_the_original() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let final_path = root.join("original-fd");
    let mut ticket = common.begin_create(&final_path)?;
    let temp_path = ticket.allocate_private_temp_path()?;
    ticket.reserve_end(4)?;
    ticket.creation_started()?;
    let mut writer = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&temp_path)?;
    ticket.created(&writer, &temp_path)?;
    let clone = writer.try_clone()?;
    require!(ticket.confirmed_safe_writer_fd_retired(clone).is_err())?;
    writer.write_all(b"four")?;
    drop(ticket);
    require!(!common.is_quiescent()?)?;
    require!(authority.snapshot()?.used_bytes >= 4)?;

    let clone = writer.try_clone()?;
    require!(
        common
            .recover_pending_with_fd(&final_path, &temp_path, clone)
            .is_err()
    )?;
    require_eq!(writer.write_at(b"F", 0)?, 1)?;
    require!(!common.is_quiescent()?)?;
    require!(authority.snapshot()?.used_bytes >= 4)?;
    let recovered = common.recover_pending_with_fd(&final_path, &temp_path, writer)?;
    let reopened = File::open(&temp_path)?;
    let observed = InodeObservation::from_file(&reopened)?;
    rusqlite::ffi::quanta_capacity_vfs::close_owned_file(reopened)?;
    require_eq!(
        authority.retained_inode(observed.inode, observed.bytes)?,
        Some(recovered)
    )?;
    require_eq!(fs::read(&temp_path)?, b"Four")?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn poisoned_authority_refuses_previously_reserved_write_boundary() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let known_path = root.join("known-retained");
    publish_file(&common, &known_path, b"four")?;
    let known = File::open(&known_path)?;
    let observed = InodeObservation::from_file(&known)?;
    rusqlite::ffi::quanta_capacity_vfs::close_owned_file(known)?;

    let final_path = root.join("poisoned-write");
    let mut ticket = common.begin_create(&final_path)?;
    let temp_path = ticket.allocate_private_temp_path()?;
    ticket.reserve_end(4)?;
    ticket.creation_started()?;
    let writer = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&temp_path)?;
    ticket.created(&writer, &temp_path)?;
    require!(authority.retained_inode(observed.inode, 5).is_err())?;
    require!(authority.snapshot()?.poisoned)?;
    require!(matches!(ticket.reserve_end(4), Err(CoreError::Storage(_))))?;
    require!(matches!(
        ticket.creation_started(),
        Err(CoreError::Storage(_))
    ))?;
    require_eq!(fs::metadata(&temp_path)?.len(), 0)?;
    ticket.confirmed_safe_writer_fd_retired(writer)?;
    Ok(())
}

#[test]
fn consumed_original_as_auxiliary_freezes_precharged_write() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let final_path = root.join("auxiliary-original");
    let mut ticket = common.begin_create(&final_path)?;
    let temp_path = ticket.allocate_private_temp_path()?;
    ticket.reserve_end(4)?;
    ticket.creation_started()?;
    let writer = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&temp_path)?;
    ticket.created(&writer, &temp_path)?;
    require!(
        ticket
            .confirmed_auxiliary_writer_fd_retired(writer)
            .is_err()
    )?;
    require!(matches!(ticket.reserve_end(4), Err(CoreError::Storage(_))))?;
    require!(matches!(
        ticket.creation_started(),
        Err(CoreError::Storage(_))
    ))?;
    require!(authority.snapshot()?.used_bytes >= 4)?;
    require_eq!(fs::metadata(&temp_path)?.len(), 0)?;
    Ok(())
}

fn publish_file(common: &PublicationFileCapacity, final_path: &Path, bytes: &[u8]) -> TestResult {
    common.ensure_private_parent_directories(final_path)?;
    let mut ticket = common.begin_create(final_path)?;
    let temp_path = ticket.allocate_private_temp_path()?;
    ticket.reserve_end(bytes.len().try_into()?)?;
    ticket.creation_started()?;
    let mut file = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&temp_path)?;
    ticket.created(&file, &temp_path)?;
    file.write_all(bytes)?;
    ticket.before_replace(final_path)?;
    fs::rename(&temp_path, final_path)?;
    let _retained = ticket.replaced_and_synced(&file, final_path)?;
    ticket.confirmed_safe_writer_fd_retired(file)?;
    let lease = ticket.take_owned_lease()?;
    drop(ticket);
    lease.retire()?;
    Ok(())
}

#[test]
fn directory_delete_retires_only_after_all_hardlinks_are_removed() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    common.sweep_retired()?;
    let baseline = authority.snapshot()?;
    let first = root.join("dataset/first");
    let second = root.join("dataset/second");
    publish_file(&common, &first, b"four")?;
    let mut link = common.begin_link(&first, &second)?;
    link.perform_and_sync()?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        baseline
            .used_bytes
            .checked_add(4)
            .ok_or_else(|| std::io::Error::other("expected byte growth overflow"))?
    )?;
    let mut delete = common.begin_directory_delete(&root.join("dataset"), &mut || Ok(()))?;
    delete.perform_and_sync(&mut || Ok(()))?;
    require!(!root.join("dataset").exists())?;
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes)?;
    require_eq!(authority.snapshot()?.used_files, baseline.used_files)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn move_overwrite_keeps_old_charge_until_last_reader_retires() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let both_bytes = baseline
        .used_bytes
        .checked_add(7)
        .ok_or_else(|| std::io::Error::other("overwrite byte expectation overflow"))?;
    let old_retired_bytes = baseline
        .used_bytes
        .checked_add(3)
        .ok_or_else(|| std::io::Error::other("retired byte expectation overflow"))?;
    let both_files = baseline
        .used_files
        .checked_add(2)
        .ok_or_else(|| std::io::Error::other("overwrite file expectation overflow"))?;
    let old_retired_files = baseline
        .used_files
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("retired file expectation overflow"))?;
    let source = root.join("move-source");
    let target = root.join("move-target");
    publish_file(&common, &source, b"new")?;
    publish_file(&common, &target, b"old!")?;
    let reader = common
        .begin_reader_open(&target)?
        .opened_owned(File::open(&target)?)?;
    let mut move_ticket = common.begin_move(&source, &target)?;
    move_ticket.perform_and_sync()?;
    drop(move_ticket);
    require!(!source.exists())?;
    require_eq!(fs::read(&target)?, b"new")?;
    let mut old = [0_u8; 4];
    require_eq!(reader.file()?.read_at(&mut old, 0)?, 4)?;
    require_eq!(old, *b"old!")?;
    require_eq!(authority.snapshot()?.used_bytes, both_bytes)?;
    require_eq!(authority.snapshot()?.used_files, both_files)?;
    drop(reader);
    require_eq!(authority.snapshot()?.used_bytes, old_retired_bytes)?;
    require_eq!(authority.snapshot()?.used_files, old_retired_files)?;
    let mut delete = common.begin_delete(&target)?;
    delete.perform_and_sync()?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn move_overwrite_retains_old_hardlink_until_alias_deletion() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let both_bytes = baseline
        .used_bytes
        .checked_add(7)
        .ok_or_else(|| std::io::Error::other("hardlink overwrite byte expectation overflow"))?;
    let old_retired_bytes = baseline
        .used_bytes
        .checked_add(3)
        .ok_or_else(|| std::io::Error::other("hardlink retired byte expectation overflow"))?;
    let both_files = baseline
        .used_files
        .checked_add(2)
        .ok_or_else(|| std::io::Error::other("hardlink overwrite file expectation overflow"))?;
    let source = root.join("move-source");
    let target = root.join("move-target");
    let alias = root.join("old-alias");
    publish_file(&common, &source, b"new")?;
    publish_file(&common, &target, b"old!")?;
    common.begin_link(&target, &alias)?.perform_and_sync()?;
    common.begin_move(&source, &target)?.perform_and_sync()?;
    require_eq!(fs::read(&alias)?, b"old!")?;
    require_eq!(fs::read(&target)?, b"new")?;
    require_eq!(authority.snapshot()?.used_bytes, both_bytes)?;
    require_eq!(authority.snapshot()?.used_files, both_files)?;
    common.begin_delete(&alias)?.perform_and_sync()?;
    require_eq!(authority.snapshot()?.used_bytes, old_retired_bytes)?;
    common.begin_delete(&target)?.perform_and_sync()?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn create_mode_move_refuses_existing_target_and_moves_into_absence() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let source = root.join("create-mode-source");
    let existing = root.join("create-mode-existing");
    let vacant = root.join("create-mode-vacant");
    publish_file(&common, &source, b"new")?;
    publish_file(&common, &existing, b"old!")?;
    let charged = authority.snapshot()?;

    require!(matches!(
        common.begin_move_if_absent(&source, &existing),
        Err(CoreError::StorageIo {
            kind: std::io::ErrorKind::AlreadyExists,
            raw_os_error: None,
            ..
        })
    ))?;
    require_eq!(fs::read(&source)?, b"new")?;
    require_eq!(fs::read(&existing)?, b"old!")?;
    require_eq!(authority.snapshot()?, charged)?;
    require!(common.is_quiescent()?)?;

    common
        .begin_move_if_absent(&source, &vacant)?
        .perform_and_sync()?;
    require!(!source.exists())?;
    require_eq!(fs::read(&vacant)?, b"new")?;
    require_eq!(fs::read(&existing)?, b"old!")?;
    require_eq!(authority.snapshot()?, charged)?;
    common.begin_delete(&vacant)?.perform_and_sync()?;
    common.begin_delete(&existing)?.perform_and_sync()?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn path_ticket_source_metadata_requires_original_file_and_current_parent() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let parent = root.join("path-metadata-parent");
    let displaced_parent = root.join("path-metadata-displaced");
    let source = parent.join("source");
    let target = parent.join("target");
    publish_file(&common, &source, b"abc")?;
    let ticket = common.begin_move_if_absent(&source, &target)?;
    let (source_fd, parent_fd) = ticket.source_metadata()?;
    require_eq!(source_fd.len(), 3)?;
    require_eq!(source_fd.ino(), fs::metadata(&source)?.ino())?;
    require_eq!(parent_fd.ino(), fs::metadata(&parent)?.ino())?;

    let displaced_source = parent.join("displaced-source");
    fs::rename(&source, &displaced_source)?;
    require!(ticket.source_metadata().is_err())?;
    fs::rename(&displaced_source, &source)?;
    require_eq!(ticket.source_metadata()?.0.len(), 3)?;

    fs::rename(&parent, &displaced_parent)?;
    fs::create_dir(&parent)?;
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
    require!(ticket.source_metadata().is_err())?;
    fs::remove_dir(&parent)?;
    fs::rename(&displaced_parent, &parent)?;
    require_eq!(ticket.source_metadata()?.1.ino(), parent_fd.ino())?;

    drop(ticket);
    require!(common.is_quiescent()?)?;
    common.begin_delete(&source)?.perform_and_sync()?;
    require_eq!(authority.snapshot()?, baseline)?;
    Ok(())
}

#[test]
fn move_overwrite_unstarted_drop_and_failed_noreplace_preserve_both_paths() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let source = root.join("move-source");
    let target = root.join("move-target");
    publish_file(&common, &source, b"new")?;
    publish_file(&common, &target, b"old!")?;
    let before = authority.snapshot()?;
    drop(common.begin_move(&source, &target)?);
    require_eq!(authority.snapshot()?, before)?;
    require!(common.is_quiescent()?)?;
    let mut ticket = common.begin_move(&source, &target)?;
    ticket.change_started()?;
    let failed = renameat_with(
        &ticket.from.parent,
        Path::new(&ticket.from.name),
        &ticket.to.parent,
        Path::new(&ticket.to.name),
        RenameFlags::NOREPLACE,
    );
    let errno = failed.err().ok_or("NOREPLACE accepted existing target")?;
    let expected_raw = std::io::Error::from(rustix::io::Errno::EXIST)
        .raw_os_error()
        .ok_or("EEXIST lacks raw errno")?;
    require!(matches!(
        storage_io("renameat", errno),
        CoreError::StorageIo {
            kind: std::io::ErrorKind::AlreadyExists,
            raw_os_error: Some(raw),
            ..
        } if raw == expected_raw
    ))?;
    require_eq!(ticket.reconcile_after_failure()?, PathRecovery::Unchanged)?;
    drop(ticket);
    require_eq!(fs::read(&source)?, b"new")?;
    require_eq!(fs::read(&target)?, b"old!")?;
    require_eq!(authority.snapshot()?, before)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn post_link_cut_reconciles_original_inode_without_a_second_link() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let source = root.join("link-source");
    let alias = root.join("link-alias");
    publish_file(&common, &source, b"abc")?;
    let charged = authority.snapshot()?;
    let mut ticket = common.begin_link(&source, &alias)?;
    ticket.change_started()?;
    fs::hard_link(&source, &alias)?;
    require!(ticket.has_started() && !ticket.is_settled())?;
    require_eq!(ticket.reconcile_after_failure()?, PathRecovery::Changed)?;
    require_eq!(fs::read(&alias)?, b"abc")?;
    require_eq!(authority.snapshot()?, charged)?;
    common.begin_delete(&alias)?.perform_and_sync()?;
    common.begin_delete(&source)?.perform_and_sync()?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn post_overwrite_cut_reconciles_old_inode_after_last_reader() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let source = root.join("cut-source");
    let target = root.join("cut-target");
    publish_file(&common, &source, b"new")?;
    publish_file(&common, &target, b"old!")?;
    let both = authority.snapshot()?;
    let reader = common
        .begin_reader_open(&target)?
        .opened_owned(File::open(&target)?)?;
    let mut ticket = common.begin_move(&source, &target)?;
    ticket.change_started()?;
    fs::rename(&source, &target)?;
    require_eq!(ticket.reconcile_after_failure()?, PathRecovery::Changed)?;
    require_eq!(fs::read(&target)?, b"new")?;
    require_eq!(authority.snapshot()?, both)?;
    let mut old = [0_u8; 4];
    require_eq!(reader.file()?.read_at(&mut old, 0)?, 4)?;
    require_eq!(old, *b"old!")?;
    reader.retire()?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        both.used_bytes
            .checked_sub(4)
            .ok_or_else(|| std::io::Error::other("old byte expectation underflow"))?
    )?;
    common.begin_delete(&target)?.perform_and_sync()?;
    require_eq!(authority.snapshot()?, baseline)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn post_start_foreign_path_cannot_reconcile_or_unlink_replacement() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let source = root.join("foreign-source");
    let target = root.join("foreign-target");
    publish_file(&common, &source, b"abc")?;
    let before = authority.snapshot()?;
    let mut ticket = common.begin_move(&source, &target)?;
    ticket.change_started()?;
    fs::write(&target, b"other")?;
    require!(ticket.reconcile_after_failure().is_err())?;
    require_eq!(fs::read(&source)?, b"abc")?;
    require_eq!(fs::read(&target)?, b"other")?;
    require_eq!(authority.snapshot()?, before)?;
    require!(ticket.has_started() && !ticket.is_settled())?;
    Ok(())
}

#[test]
fn post_start_foreign_parent_cannot_reconcile_original_path() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let original_parent = root.join("held-parent");
    let moved_parent = root.join("moved-parent");
    let source = original_parent.join("source");
    let target = root.join("new-target");
    publish_file(&common, &source, b"abc")?;
    let before = authority.snapshot()?;
    let mut ticket = common.begin_move(&source, &target)?;
    ticket.change_started()?;
    fs::rename(&original_parent, &moved_parent)?;
    fs::create_dir(&original_parent)?;
    require!(ticket.reconcile_after_failure().is_err())?;
    require_eq!(fs::read(moved_parent.join("source"))?, b"abc")?;
    require!(original_parent.is_dir())?;
    require_eq!(authority.snapshot()?, before)?;
    require!(ticket.has_started() && !ticket.is_settled())?;
    Ok(())
}

#[test]
fn path_change_refuses_same_inode_move_and_existing_link_target() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let source = root.join("same-inode-source");
    let alias = root.join("same-inode-alias");
    publish_file(&common, &source, b"data")?;
    common.begin_link(&source, &alias)?.perform_and_sync()?;
    let before = authority.snapshot()?;
    require!(common.begin_move(&source, &alias).is_err())?;
    require!(matches!(
        common.begin_link(&source, &alias),
        Err(CoreError::StorageIo {
            kind: std::io::ErrorKind::AlreadyExists,
            raw_os_error: None,
            ..
        })
    ))?;
    let duplicate_link = fs::hard_link(&source, &alias)
        .err()
        .ok_or("hard link replaced an existing target")?;
    let expected_raw = std::io::Error::from(rustix::io::Errno::EXIST)
        .raw_os_error()
        .ok_or("EEXIST lacks raw errno")?;
    require!(matches!(
        storage_io("linkat", duplicate_link),
        CoreError::StorageIo {
            kind: std::io::ErrorKind::AlreadyExists,
            raw_os_error: Some(raw),
            ..
        } if raw == expected_raw
    ))?;
    require_eq!(fs::read(&source)?, b"data")?;
    require_eq!(fs::read(&alias)?, b"data")?;
    require_eq!(authority.snapshot()?, before)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn directory_move_preserves_charge_and_blocks_parallel_child_open() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let from = root.join("old");
    let to = root.join("new");
    publish_file(&common, &from.join("child"), b"four")?;
    let mut moving = common.begin_directory_move(&from, &to)?;
    require!(common.begin_reader_open(&from.join("child")).is_err())?;
    moving.perform_and_sync()?;
    require!(!from.exists())?;
    require_eq!(fs::read(to.join("child"))?, b"four")?;
    require_eq!(
        authority.snapshot()?.used_bytes,
        baseline
            .used_bytes
            .checked_add(4)
            .ok_or_else(|| std::io::Error::other("expected byte growth overflow"))?
    )?;
    require!(common.is_quiescent()?)?;
    Ok(())
}

#[test]
fn retired_unsettled_writer_pin_preserves_charge_and_allows_inventory_handoff() -> TestResult {
    let (_dir, root, lease, authority, common) = fixture()?;
    let baseline = authority.snapshot()?;
    let final_path = root.join("partial-batch-output");
    let mut ticket = common.begin_create(&final_path)?;
    let temporary = ticket.allocate_private_temp_path()?;
    ticket.reserve_end(8)?;
    ticket.creation_started()?;
    let mut writer = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&temporary)?;
    ticket.created(&writer, &temporary)?;
    writer.write_all(b"four")?;
    writer.sync_all()?;
    fs::rename(&temporary, &final_path)?;
    // Deliberately omit the batch's directory/shared barrier and settlement.
    ticket.confirmed_safe_writer_fd_retired(writer)?;
    drop(ticket);

    let pending = authority.snapshot()?;
    require_eq!(pending.used_bytes, baseline.used_bytes + 8)?;
    require_eq!(pending.used_files, baseline.used_files + 1)?;
    require_eq!(pending.uncertain_pending, 1)?;
    require!(!pending.poisoned)?;
    require!(!common.is_quiescent()?)?;
    require!(common.is_handoff_ready_after_retirement()?)?;
    require_eq!(fs::read(&final_path)?, b"four")?;
    drop(common);
    drop(lease);
    let replacement = StateRootLease::acquire(&root)?;
    require_eq!(replacement.state_root_identity_v1(), root.as_path())?;
    let reseeded = seed_publication_capacity(
        &replacement,
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
    require_eq!(reseeded.snapshot()?.used_bytes, baseline.used_bytes + 4)?;
    require_eq!(reseeded.snapshot()?.used_files, baseline.used_files + 1)?;
    require_eq!(reseeded.snapshot()?.uncertain_pending, 0)?;
    // A new inventory did not retroactively settle/refund the old ledger.
    require_eq!(authority.snapshot()?.used_bytes, pending.used_bytes)?;
    require_eq!(authority.snapshot()?.used_files, pending.used_files)?;
    require_eq!(authority.snapshot()?.uncertain_pending, 1)?;
    drop(replacement);
    Ok(())
}

#[test]
fn final_common_drop_quarantines_pending_owned_writer_fd() -> TestResult {
    let (_dir, root, lease, authority, common) = fixture()?;
    let final_path = root.join("unfinished-writer");
    let mut ticket = common.begin_create(&final_path)?;
    let temp_path = ticket.allocate_private_temp_path()?;
    ticket.reserve_end(4)?;
    ticket.creation_started()?;
    let mut writer = OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(&temp_path)?;
    ticket.created(&writer, &temp_path)?;
    writer.write_all(b"four")?;
    writer.sync_all()?;
    fs::rename(&temp_path, &final_path)?;
    File::open(&root)?.sync_all()?;

    // The final path now matches the born inode. Only the unwitnessed retained
    // writer pin can keep this root from being handed to a new owner.
    rusqlite::ffi::quanta_capacity_vfs::close_owned_file(writer)?;
    drop(ticket);
    drop(common);
    drop(authority);
    drop(lease);

    let reacquire = StateRootLease::acquire(&root);
    require!(matches!(
        reacquire,
        Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootInUse,
            ..
        })
    ))?;
    Ok(())
}

// Append to runtime/publication_file_capacity_fs_tests.rs after integration.
#[test]
fn one_active_binder_per_root_rejects_independent_ledger() -> TestResult {
    let (_dir, root, lease, _authority, common) = fixture()?;
    let independent =
        quanta_index_core::publication_capacity::PublicationCapacityLedger::new(1024, 32, 32)?
            .seal_boot()?;

    // A second constructor would otherwise admit writes through an
    // independently capped ledger for the same held state-root lease.
    let duplicate = PublicationFileCapacity::new(lease.clone(), independent.clone(), 16, 64, 256);
    require!(matches!(duplicate, Err(CoreError::InvalidContract(_))))?;

    let shared = Arc::clone(&common);
    require!(Arc::ptr_eq(&common, &shared))?;
    let provisional = shared.begin_create(&root.join("arc-clone-still-admitted"))?;
    drop(provisional);
    drop(shared);
    drop(common);

    // Recreating after the final Shared owner retires is permitted.
    let successor = PublicationFileCapacity::new(lease, independent, 16, 64, 256)?;
    require!(successor.is_quiescent()?)?;
    Ok(())
}

#[test]
fn empty_native_sidecar_disposal_requires_original_zero_byte_birth() -> TestResult {
    for (body, replace, resize) in [
        (b"".as_slice(), false, false),
        (b"WAL!".as_slice(), false, false),
        (b"".as_slice(), true, false),
        (b"".as_slice(), false, true),
    ] {
        let (_dir, root, _lease, authority, common) = fixture()?;
        let path = root.join("catalog-wal");
        let baseline = authority.snapshot()?;
        if resize {
            publish_file(&common, &path, b"")?;
        }
        let mut ticket = if resize {
            let file = File::open(&path)?;
            let observed = InodeObservation::from_file(&file)?;
            close_owned_file(file)?;
            common.begin_native_resize(&path, observed, 1)?
        } else {
            common.begin_native_create(&path)?
        };
        ticket.reserve_end(u64::try_from(body.len())?.max(1))?;
        ticket.start()?;
        let mut writer = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(!resize)
            .mode(0o600)
            .open(&path)?;
        writer.write_all(body)?;
        let observed = InodeObservation::from_file(&writer)?;
        ticket.observe_native_fd(observed)?;
        close_owned_file(writer)?;
        ticket.confirmed_unsynced_native_fd_retired(observed)?;
        drop(ticket);
        let pending = authority.snapshot()?;
        if replace {
            fs::rename(&path, root.join("original-retired-wal"))?;
            fs::write(&path, b"foreign")?;
        }
        let result = common.discard_retired_empty_native_create(&path);
        if body.is_empty() && !replace && !resize {
            result?;
            require!(!path.exists())?;
            require_eq!(authority.snapshot()?, baseline)?;
            require!(common.is_quiescent()?)?;
            common.discard_retired_empty_native_create(&path)?;
        } else {
            require!(result.is_err())?;
            require_eq!(
                fs::read(&path)?,
                if replace { b"foreign".as_slice() } else { body }
            )?;
            require_eq!(authority.snapshot()?, pending)?;
            require!(!common.is_quiescent()?)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
mod real_kernel_faults {
    use super::*;
    use std::fmt::Debug;

    type FaultResult<T> = Result<T, Box<dyn std::error::Error>>;

    struct FaultSwitch {
        control: PathBuf,
    }

    impl FaultSwitch {
        fn clear(&self) -> TestResult {
            fs::write(&self.control, b"off none\n")?;
            require_eq!(fs::read(&self.control)?, b"off none\n")?;
            Ok(())
        }

        fn arm(&self, operation: &str, target: &Path) -> TestResult {
            let name = target
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or("fault target basename is not UTF-8")?;
            require!(!name.chars().any(char::is_whitespace))?;
            fs::write(&self.control, format!("{operation} {name}\n"))?;
            Ok(())
        }
    }

    impl Drop for FaultSwitch {
        fn drop(&mut self) {
            let _reset_result = fs::write(&self.control, b"off none\n");
        }
    }

    fn real_fixture() -> FaultResult<(FaultSwitch, Fixture)> {
        let mount = PathBuf::from(
            std::env::var_os("QIDX_REAL_FAULT_MOUNT").ok_or("explicit fault mount is required")?,
        );
        let control = PathBuf::from(
            std::env::var_os("QIDX_REAL_FAULT_CONTROL")
                .ok_or("explicit fault control is required")?,
        );
        require!(mount.is_absolute() && control.is_absolute())?;
        require_eq!(&fs::canonicalize(&mount)?, &mount)?;
        require!(fs::symlink_metadata(&control)?.file_type().is_file())?;
        let mount_name = mount.to_str().ok_or("fault mount is not UTF-8")?;
        require!(
            fs::read_to_string("/proc/self/mountinfo")?
                .lines()
                .any(|line| {
                    line.split_whitespace().nth(4) == Some(mount_name)
                        && line.contains(" - fuse.passthrough-fault ")
                })
        )?;
        let switch = FaultSwitch { control };
        switch.clear()?;
        let fixture = fixture()?;
        let (_dir, root, _lease, _authority, _common) = &fixture;
        require!(root.starts_with(&mount))?;
        require_eq!(fs::metadata(root)?.dev(), fs::metadata(&mount)?.dev())?;
        require!(fs::metadata(root)?.dev() != fs::metadata(&switch.control)?.dev())?;
        Ok((switch, fixture))
    }

    fn require_kernel_eio<T: Debug>(
        outcome: &Result<T, CoreError>,
        switch: &FaultSwitch,
        context: &str,
    ) -> TestResult {
        // Reset independently, preserving both causes if reset also fails.
        let reset = switch.clear();
        let expected = matches!(
            outcome,
            Err(CoreError::StorageIo { raw_os_error: Some(errno), message, .. })
                if *errno == rustix::io::Errno::IO.raw_os_error() && message.contains(context)
        );
        if !expected || reset.is_err() {
            return Err(std::io::Error::other(format!(
                "real {context}: outcome={outcome:?}; fault reset={reset:?}"
            ))
            .into());
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires bounded FUSE fault backend, explicit control and TMPDIR on that mount"]
    fn original_fd_kernel_fsync_eio_keeps_charge_and_recovers_same_inode() -> TestResult {
        let (switch, (_dir, root, _lease, authority, common)) = real_fixture()?;
        let boot = authority.snapshot()?;
        let target = root.join("physical-fsync.bin");
        let parent = common.reserve_publication_allowance(8, 1)?;
        let (mut ticket, mut writer) = visible_writer(&common, &target, Arc::clone(&parent))?;
        ticket.reserve_end(8)?;
        writer.write_all(b"four")?;
        let original = InodeObservation::from_file(&writer)?;
        let descriptor = writer.as_raw_fd();
        let charged = authority.snapshot()?;
        switch.arm("fsync", &target)?;
        require_kernel_eio(
            &ticket.settle_after_file_and_parent_sync(&writer),
            &switch,
            "file fsync",
        )?;
        require_eq!(writer.as_raw_fd(), descriptor)?;
        require_eq!(InodeObservation::from_file(&writer)?, original)?;
        require_eq!(authority.snapshot()?, charged)?;
        require!(!common.is_quiescent()?)?;
        require!(!common.is_handoff_ready_after_retirement()?)?;
        let _retained = ticket.settle_after_file_and_parent_sync(&writer)?;
        ticket.confirmed_safe_writer_fd_retired(writer)?;
        ticket.confirmed_safe_pin_retired()?;
        drop(ticket);
        drop(parent);
        require_eq!(fs::read(&target)?, b"four")?;
        require_eq!(
            authority.snapshot()?.used_bytes,
            boot.used_bytes
                .checked_add(4)
                .ok_or("byte growth overflow")?
        )?;
        common.begin_delete(&target)?.perform_and_sync()?;
        common.sweep_retired()?;
        require_eq!(authority.snapshot()?, boot)?;
        require!(common.is_quiescent()?)?;
        Ok(())
    }

    #[test]
    #[ignore = "requires bounded FUSE fault backend with fsyncdir injection, explicit control and TMPDIR on that mount"]
    fn native_parent_fsyncdir_eio_retains_latest_original_fd_witness_for_retry() -> TestResult {
        let (switch, (_dir, root, _lease, authority, common)) = real_fixture()?;
        let boot = authority.snapshot()?;
        let target = root.join("native-parent-fsyncdir.bin");
        let parent = common.reserve_publication_allowance(8, 1)?;
        let mut ticket = common.begin_native_create_with_allowance(&target, Arc::clone(&parent))?;
        ticket.reserve_end(4)?;
        ticket.creation_started()?;
        let mut writer = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&target)?;
        let born = InodeObservation::from_file(&writer)?;
        ticket.observe_native_fd(born)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        let synced = InodeObservation::from_file(&writer)?;
        require_eq!(born.bytes, 0)?;
        require_eq!(synced.bytes, 4)?;
        let original_fd = writer.as_raw_fd();
        let charged = authority.snapshot()?;
        switch.arm("fsyncdir", &root)?;
        require_kernel_eio(
            &ticket.settle_after_native_sync(synced, None),
            &switch,
            "parent fsync",
        )?;
        require_eq!(ticket.observed, Some(synced))?;
        require!(!ticket.is_settled())?;
        require_eq!(writer.as_raw_fd(), original_fd)?;
        require_eq!(InodeObservation::from_file(&writer)?, synced)?;
        require_eq!(authority.snapshot()?, charged)?;
        require!(!common.is_quiescent()?)?;

        let _retained = ticket.settle_after_native_sync(synced, None)?;
        close_owned_file(writer)?;
        ticket.confirmed_native_fd_retired()?;
        drop(ticket);
        drop(parent);
        require_eq!(fs::read(&target)?, b"four")?;
        require_eq!(authority.snapshot()?.used_bytes, boot.used_bytes + 4)?;
        common.begin_delete(&target)?.perform_and_sync()?;
        common.sweep_retired()?;
        require_eq!(authority.snapshot()?, boot)?;
        require!(common.is_quiescent()?)?;
        Ok(())
    }

    #[test]
    #[ignore = "requires bounded FUSE fault backend, explicit control and TMPDIR on that mount"]
    fn original_fd_kernel_close_eio_keeps_charge_and_quarantines_root() -> TestResult {
        let (switch, (_dir, root, lease, authority, common)) = real_fixture()?;
        let boot = authority.snapshot()?;
        let target = root.join("physical-close.bin");
        let parent = common.reserve_publication_allowance(8, 1)?;
        let (mut ticket, mut writer) = visible_writer(&common, &target, Arc::clone(&parent))?;
        ticket.reserve_end(8)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        let original = InodeObservation::from_file(&writer)?;
        let charged = authority.snapshot()?;
        switch.arm("flush", &target)?;
        require_kernel_eio(
            &ticket.confirmed_safe_writer_fd_retired(writer),
            &switch,
            "close actual writer FD",
        )?;
        // The consumed numeric descriptor is never retried after close EIO.
        require_eq!(authority.snapshot()?, charged)?;
        require!(!common.is_quiescent()?)?;
        require!(!common.is_handoff_ready_after_retirement()?)?;
        require!(ticket.settle_after_external_file_and_parent_sync().is_err())?;
        require!(ticket.take_owned_lease().is_err())?;
        let observed = fs::metadata(&target)?;
        require_eq!(observed.dev(), original.inode.device)?;
        require_eq!(observed.ino(), original.inode.inode)?;
        require_eq!(fs::read(&target)?, b"four")?;
        drop(ticket);
        drop(parent);
        let pending = authority.snapshot()?;
        require_eq!(
            pending.used_bytes,
            boot.used_bytes
                .checked_add(8)
                .ok_or("byte growth overflow")?
        )?;
        require_eq!(
            pending.uncertain_pending,
            boot.uncertain_pending
                .checked_add(1)
                .ok_or("pending count overflow")?
        )?;
        drop(common);
        drop(authority);
        drop(lease);
        require!(matches!(
            StateRootLease::acquire(&root),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::StateRootInUse,
                ..
            })
        ))?;
        Ok(())
    }
}

#[test]
fn empty_retirement_hint_never_hides_an_unwitnessed_close() -> TestResult {
    let (_dir, _root, _lease, _authority, common) = fixture()?;
    common.sweep_retired()?;
    common.mark_close_unwitnessed();
    require!(matches!(common.sweep_retired(), Err(CoreError::Storage(_))))?;
    Ok(())
}

#[test]
fn sweep_keeps_releasing_tombstone_for_its_finisher() -> TestResult {
    let (_dir, root, _lease, authority, common) = fixture()?;
    common.sweep_retired()?;
    let baseline = authority.snapshot()?;
    let path = root.join("release-in-flight");
    publish_file(&common, &path, b"four")?;
    let charged = authority.snapshot()?;
    let witness = File::open(&path)?;
    let before = InodeObservation::from_file(&witness)?;
    fs::remove_file(&path)?;
    File::open(&root)?.sync_all()?;
    let after = InodeObservation::from_file(&witness)?;
    require_eq!(after.inode, before.inode)?;
    require_eq!(after.bytes, 4)?;
    require_eq!(after.links, 0)?;
    close_owned_file(witness)?;

    // Model a sweeper's mutex-protected claim after a real synced last unlink.
    // A second sweep must not forget it merely because its owner is finishing.
    {
        let mut state = lock(&common.shared.state)?;
        let entry = state
            .inodes
            .get_mut(&after.inode)
            .ok_or_else(|| std::io::Error::other("test inode missing"))?;
        require_eq!(entry.pins, 0)?;
        require!(!entry.tombstone_synced && !entry.releasing)?;
        entry.tombstone_synced = true;
        entry.releasing = true;
        state.may_have_tombstones = true;
        drop(state);
    }
    common.sweep_retired()?;
    require_eq!(authority.snapshot()?, charged)?;
    {
        let mut state = lock(&common.shared.state)?;
        state
            .inodes
            .get_mut(&after.inode)
            .ok_or_else(|| std::io::Error::other("test inode missing"))?
            .releasing = false;
        drop(state);
    }
    common.sweep_retired()?;
    require_eq!(authority.snapshot()?.used_bytes, baseline.used_bytes)?;
    require_eq!(authority.snapshot()?.used_files, baseline.used_files)?;
    require!(common.is_quiescent()?)?;
    Ok(())
}
