use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, RequestBudgetV1};
use quanta_index_searchd::app::runtime::StateRootLease;

use super::super::{PendingOperation, PublicationFileDeletions};
use super::NamespaceOperation;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn Error>>;

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    owner: Arc<PublicationFileDeletions>,
    binder: Arc<PublicationFileCapacity>,
    authority: quanta_index_core::publication_capacity::PublicationCapacity,
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
    fixture_with_tree(false)
}

fn fixture_with_tree(include_tree: bool) -> Result<Fixture, Box<dyn Error>> {
    fixture_with_layout(include_tree, false, false)
}

fn fixture_with_staged_parent() -> Result<Fixture, Box<dyn Error>> {
    fixture_with_layout(false, true, false)
}

fn fixture_with_semantic_tree() -> Result<Fixture, Box<dyn Error>> {
    fixture_with_layout(false, false, true)
}

fn fixture_with_layout(
    include_tree: bool,
    include_staged_target: bool,
    include_semantic_tree: bool,
) -> Result<Fixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    if include_semantic_tree {
        let generation = requested.join("indexes/semantic/family/g3");
        fs::create_dir_all(&generation)?;
        fs::write(generation.join("child"), b"abc")?;
    } else {
        fs::write(requested.join("source"), b"abc")?;
        fs::write(requested.join("other"), b"xyz")?;
    }
    if include_tree {
        fs::create_dir(requested.join("tree"))?;
        fs::write(requested.join("tree/child"), b"old")?;
    }
    if include_staged_target {
        fs::create_dir(requested.join("target-parent"))?;
        fs::write(requested.join("target-parent/target"), b"prior")?;
    }
    let lease = StateRootLease::acquire(&requested)?;
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
    let binder = PublicationFileCapacity::new(lease, authority.clone(), 16, 32, 64)?;
    let owner = Arc::new(PublicationFileDeletions::new(Arc::clone(&binder)));
    Ok(Fixture {
        _directory: directory,
        root,
        owner,
        binder,
        authority,
    })
}

fn require(condition: bool, message: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

#[test]
fn namespace_link_uses_one_slot_and_keeps_one_charged_inode() -> TestResult {
    let fixture = fixture()?;
    let source = fixture.root.join("source");
    let linked = fixture.root.join("linked");
    let before = fixture.authority.snapshot()?;
    let mut ticket = fixture.owner.begin_link(&source, &linked)?;
    require(
        matches!(
            fixture.owner.delete_file_and_sync(&fixture.root.join("other")),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "file deletion bypassed a live namespace ticket",
    )?;
    ticket.perform_and_sync()?;
    require(fs::read(&linked)? == b"abc", "hard link contents changed")?;
    require(
        fs::metadata(&source)?.ino() == fs::metadata(&linked)?.ino(),
        "hard link did not preserve inode identity",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "hard link changed the physical charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "namespace claim remained busy",
    )?;
    Ok(())
}

#[test]
fn unstarted_drop_releases_claim_without_mutating_namespace() -> TestResult {
    let fixture = fixture()?;
    let source = fixture.root.join("source");
    let moved = fixture.root.join("moved");
    let before = fixture.authority.snapshot()?;
    drop(fixture.owner.begin_move(&source, &moved)?);
    require(!moved.exists(), "unstarted move created destination")?;
    let mut retry = fixture.owner.begin_move(&source, &moved)?;
    retry.perform_and_sync()?;
    require(
        fs::read(&moved)? == b"abc",
        "retry did not move original file",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "unstarted claim or move changed physical charge",
    )?;
    require(fixture.binder.is_quiescent()?, "move claim remained busy")?;
    Ok(())
}

#[test]
fn started_external_move_replays_original_ticket_without_second_rename() -> TestResult {
    let fixture = fixture()?;
    let source = fixture.root.join("source");
    let moved = fixture.root.join("moved");
    let before = fixture.authority.snapshot()?;
    let mut ticket = fixture.owner.begin_move(&source, &moved)?;
    ticket.change_started()?;
    fs::rename(&source, &moved)?;
    drop(ticket);
    let mut replay = fixture.owner.begin_move(&source, &moved)?;
    replay.perform_and_sync()?;
    require(!source.exists(), "replay restored moved source")?;
    require(fs::read(&moved)? == b"abc", "replay altered destination")?;
    require(
        fixture.authority.snapshot()? == before,
        "reconciled move changed physical charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "replay left path claim busy",
    )?;
    Ok(())
}

#[test]
fn started_unchanged_external_move_returns_unreported_outcome_once() -> TestResult {
    let fixture = fixture()?;
    let source = fixture.root.join("source");
    let moved = fixture.root.join("moved");
    let mut ticket = fixture.owner.begin_move(&source, &moved)?;
    ticket.change_started()?;
    drop(ticket);
    let replay = fixture.owner.begin_move(&source, &moved);
    require(
        matches!(replay, Err(CoreError::Storage(message)) if message.contains("not reported")),
        "unchanged external move invented an OS failure or success",
    )?;
    let mut fresh = fixture.owner.begin_move(&source, &moved)?;
    fresh.perform_and_sync()?;
    require(fs::read(&moved)? == b"abc", "fresh move failed after proof")?;
    require(
        fixture.binder.is_quiescent()?,
        "unchanged move retained claim",
    )?;
    Ok(())
}

#[test]
fn directory_delete_uses_shared_slot_and_retires_children() -> TestResult {
    let fixture = fixture()?;
    let tree = fixture.root.join("tree");
    fs::create_dir(&tree)?;
    fs::hard_link(fixture.root.join("source"), tree.join("child"))?;
    let before = fixture.authority.snapshot()?;
    let mut ticket = fixture.owner.begin_directory_delete(&tree)?;
    ticket.perform_and_sync()?;
    require(!tree.exists(), "directory delete left its root")?;
    require(
        fs::read(fixture.root.join("source"))? == b"abc",
        "alias lost",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "directory alias deletion changed physical inode charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "directory claim remained busy",
    )?;
    Ok(())
}

#[test]
fn failed_namespace_slot_blocks_other_delete_before_mutation() -> TestResult {
    let fixture = fixture()?;
    let source = fixture.root.join("source");
    let moved = fixture.root.join("moved");
    let mut ticket = fixture.owner.begin_move(&source, &moved)?;
    ticket.change_started()?;
    drop(ticket);
    let other = fixture.root.join("other");
    require(
        matches!(
            fixture.owner.delete_file_and_sync(&other),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "failed namespace slot admitted another deletion",
    )?;
    require(
        fs::read(&other)? == b"xyz",
        "blocked deletion mutated other file",
    )?;
    let original = fixture.owner.reconcile_pending_namespace();
    require(
        matches!(original, Err(CoreError::Storage(message)) if message.contains("not reported")),
        "unchanged external outcome was not surfaced",
    )?;
    require(fixture.binder.is_quiescent()?, "reconcile left claim busy")?;
    Ok(())
}

#[test]
fn directory_delete_permission_failure_retains_original_ticket_and_resumes() -> TestResult {
    let fixture = fixture_with_tree(true)?;
    let tree = fixture.root.join("tree");
    let child = tree.join("child");
    let before = fixture.authority.snapshot()?;
    let old_inode = fs::metadata(&child)?.ino();
    let mut ticket = fixture.owner.begin_directory_delete(&tree)?;
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o500))?;
    let failure = ticket.perform_and_sync();
    require(failure.is_err(), "read-only directory allowed unlink")?;
    require(
        fs::metadata(&child)?.ino() == old_inode,
        "failed unlink changed child",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "failed delete refunded a still-linked child",
    )?;
    require(
        matches!(
            fixture.owner.delete_file_and_sync(&fixture.root.join("other")),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "file deletion bypassed the failed directory ticket",
    )?;
    drop(ticket);
    fs::set_permissions(&tree, fs::Permissions::from_mode(0o700))?;
    let mut replay = fixture.owner.begin_directory_delete(&tree)?;
    replay.perform_and_sync()?;
    let after = fixture.authority.snapshot()?;
    require(!tree.exists(), "directory replay kept the old root")?;
    require(
        after.used_bytes == before.used_bytes.checked_sub(3).ok_or("byte underflow")?,
        "original child width was not refunded exactly",
    )?;
    require(
        after.used_files == before.used_files.checked_sub(1).ok_or("file underflow")?,
        "original child file was not refunded exactly",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "replayed directory ticket retained its root claim",
    )?;
    Ok(())
}

#[test]
fn foreign_move_destination_retains_original_witness_on_repeated_reconcile() -> TestResult {
    let fixture = fixture()?;
    let source = fixture.root.join("source");
    let other = fixture.root.join("other");
    let moved = fixture.root.join("moved");
    let before = fixture.authority.snapshot()?;
    let mut ticket = fixture.owner.begin_move(&source, &moved)?;
    ticket.change_started()?;
    fs::rename(&source, &moved)?;
    fs::remove_file(&moved)?;
    fs::hard_link(&other, &moved)?;
    drop(ticket);
    for _attempt in 0..2 {
        require(
            matches!(
                fixture.owner.reconcile_pending_namespace(),
                Err(CoreError::InvalidContract(_))
            ),
            "foreign destination settled the original move",
        )?;
        require(
            fs::read(&moved)? == b"xyz",
            "foreign destination was changed",
        )?;
        require(
            fixture.authority.snapshot()? == before,
            "foreign destination refunded the original inode",
        )?;
    }
    require(
        !fixture.binder.is_quiescent()?,
        "foreign namespace state released the pending claim",
    )?;
    Ok(())
}

#[test]
fn first_started_syscall_error_survives_a_second_handle_call() -> TestResult {
    let fixture = fixture()?;
    let source = fixture.root.join("source");
    let parent = fixture.root.join("target-parent");
    fs::create_dir(&parent)?;
    let target = parent.join("linked");
    let before = fixture.authority.snapshot()?;
    let mut ticket = fixture.owner.begin_link(&source, &target)?;
    let expected_errno = std::io::Error::from(rustix::io::Errno::ACCESS)
        .raw_os_error()
        .ok_or("EACCES lacks raw errno")?;
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o500))?;
    let first_message = match ticket.perform_and_sync() {
        Ok(()) => return Err("read-only parent allowed hard link".into()),
        Err(CoreError::StorageIo {
            kind: std::io::ErrorKind::PermissionDenied,
            raw_os_error: Some(errno),
            message,
        }) if errno == expected_errno => message,
        Err(error) => return Err(format!("first refusal lost EACCES: {error:?}").into()),
    };
    require(
        first_message.contains("linkat"),
        "first refusal was not the actual link syscall error",
    )?;
    let second = ticket.perform_and_sync();
    require(second.is_err(), "started ticket repeated its link syscall")?;
    drop(ticket);
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
    let replay = fixture.owner.begin_link(&source, &target);
    require(
        matches!(replay, Err(CoreError::StorageIo {
            kind: std::io::ErrorKind::PermissionDenied,
            raw_os_error: Some(errno),
            message,
        }) if errno == expected_errno && message == first_message),
        "same-key unchanged replay lost the first syscall error",
    )?;
    require(!target.exists(), "failed link unexpectedly created target")?;
    require(
        fixture.authority.snapshot()? == before,
        "failed link altered the physical charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "proved unchanged link retained its claim",
    )?;
    Ok(())
}

#[test]
fn generic_move_replays_a_started_directory_move_before_source_probe() -> TestResult {
    let fixture = fixture_with_tree(true)?;
    let source = fixture.root.join("tree");
    let destination = fixture.root.join("moved-tree");
    let before = fixture.authority.snapshot()?;
    let ticket = fixture.owner.begin_directory_move(&source, &destination)?;
    {
        let mut pending = fixture
            .owner
            .pending
            .lock()
            .map_err(|_error| io::Error::other("namespace slot lock poisoned"))?;
        let Some(PendingOperation::Namespace(prior)) = pending.as_mut() else {
            return Err("directory move slot was absent".into());
        };
        let NamespaceOperation::DirectoryMove(original) = &mut prior.operation else {
            return Err("directory move lost its original ticket".into());
        };
        original.change_started()?;
        drop(pending);
    }
    fs::rename(&source, &destination)?;
    drop(ticket);
    require(
        matches!(
            fixture.owner.begin_move(&source, &destination),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "overwrite intent consumed a create-only directory move",
    )?;
    let mut replay = fixture.owner.begin_move_if_absent(&source, &destination)?;
    replay.perform_and_sync()?;
    require(!source.exists(), "directory replay restored old source")?;
    require(
        fs::read(destination.join("child"))? == b"old",
        "directory replay changed child bytes",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "directory move replay changed physical charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "directory move replay retained its scope",
    )?;
    Ok(())
}

fn start_directory_move_without_rename(
    fixture: &Fixture,
    source: &Path,
    target: &Path,
) -> TestResult {
    let ticket = fixture.owner.begin_directory_move(source, target)?;
    {
        let mut pending = fixture
            .owner
            .pending
            .lock()
            .map_err(|_error| io::Error::other("namespace slot lock poisoned"))?;
        let Some(PendingOperation::Namespace(prior)) = pending.as_mut() else {
            return Err("directory move slot was absent".into());
        };
        let NamespaceOperation::DirectoryMove(original) = &mut prior.operation else {
            return Err("directory move lost its original ticket".into());
        };
        original.change_started()?;
        drop(pending);
    }
    drop(ticket);
    Ok(())
}

fn start_directory_delete_without_unlink(fixture: &Fixture, source: &Path) -> TestResult {
    let ticket = fixture.owner.begin_directory_delete(source)?;
    {
        let mut pending = fixture
            .owner
            .pending
            .lock()
            .map_err(|_error| io::Error::other("namespace slot lock poisoned"))?;
        let Some(PendingOperation::Namespace(prior)) = pending.as_mut() else {
            return Err("directory delete slot was absent".into());
        };
        let NamespaceOperation::DirectoryDelete(original) = &mut prior.operation else {
            return Err("directory delete lost its original ticket".into());
        };
        original.deletion_started()?;
        drop(pending);
    }
    drop(ticket);
    Ok(())
}

#[test]
fn directory_move_recovery_requires_the_original_idle_key_and_never_renames_twice() -> TestResult {
    let fixture = fixture_with_tree(true)?;
    let source = fixture.root.join("tree");
    let target = fixture.root.join("moved-tree");
    let other = fixture.root.join("other");
    let before = fixture.authority.snapshot()?;
    require(
        !fixture
            .owner
            .recover_directory_move_if_pending(&source, &target)?,
        "missing directory move invented a recovery",
    )?;
    let ticket = fixture.owner.begin_directory_move(&source, &target)?;
    require(
        matches!(
            fixture.owner.recover_directory_move_if_pending(&source, &target),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "active directory move was recovered behind its handle",
    )?;
    drop(ticket);
    start_directory_move_without_rename(&fixture, &source, &target)?;
    require(
        matches!(
            fixture.owner.recover_directory_move_if_pending(&source, &other),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "different target consumed the original directory move",
    )?;
    fs::rename(&source, &target)?;
    require(
        fixture
            .owner
            .recover_directory_move_if_pending(&source, &target)?,
        "original directory move did not settle after actual rename",
    )?;
    require(!source.exists(), "recovery restored the moved source")?;
    require(
        fs::read(target.join("child"))? == b"old",
        "moved child changed",
    )?;
    require(
        !fixture
            .owner
            .recover_directory_move_if_pending(&source, &target)?,
        "settled directory move was reported twice",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "directory move recovery changed physical charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "directory move claim remained",
    )?;
    Ok(())
}

#[test]
fn directory_move_recovery_preserves_original_errno_and_foreign_replacement() -> TestResult {
    let fixture = fixture_with_tree(true)?;
    let source = fixture.root.join("tree");
    let parent = fixture.root.join("target-parent");
    fs::create_dir(&parent)?;
    let target = parent.join("moved-tree");
    let original_inode = fs::metadata(&source)?.ino();
    let mut ticket = fixture.owner.begin_directory_move(&source, &target)?;
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o500))?;
    let expected_errno = io::Error::from(rustix::io::Errno::ACCESS)
        .raw_os_error()
        .ok_or("EACCES lacks raw errno")?;
    let first_message = match ticket.perform_and_sync() {
        Err(CoreError::StorageIo {
            kind: io::ErrorKind::PermissionDenied,
            raw_os_error: Some(errno),
            message,
        }) if errno == expected_errno => message,
        other => return Err(format!("directory rename did not produce EACCES: {other:?}").into()),
    };
    drop(ticket);
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
    let retry = fixture
        .owner
        .recover_directory_move_if_pending(&source, &target);
    require(
        matches!(retry, Err(CoreError::StorageIo {
            kind: io::ErrorKind::PermissionDenied,
            raw_os_error: Some(errno),
            message,
        }) if errno == expected_errno && message == first_message),
        "unchanged directory move lost its original syscall error",
    )?;
    require(
        fs::metadata(&source)?.ino() == original_inode,
        "source changed",
    )?;
    require(!target.exists(), "failed rename created a target")?;
    require(fixture.binder.is_quiescent()?, "unchanged claim remained")?;

    start_directory_move_without_rename(&fixture, &source, &target)?;
    fs::rename(&source, &target)?;
    let held = fixture.root.join("held-tree");
    fs::rename(&target, &held)?;
    fs::create_dir(&target)?;
    fs::write(target.join("child"), b"foreign")?;
    let before = fixture.authority.snapshot()?;
    require(
        matches!(
            fixture
                .owner
                .recover_directory_move_if_pending(&source, &target),
            Err(CoreError::InvalidContract(_))
        ),
        "foreign replacement settled the original directory move",
    )?;
    require(
        fs::read(target.join("child"))? == b"foreign",
        "foreign tree was changed",
    )?;
    require(
        fs::read(held.join("child"))? == b"old",
        "original tree was changed",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "foreign replacement changed the original charge",
    )?;
    require(
        !fixture.binder.is_quiescent()?,
        "foreign state released the original claim",
    )?;
    Ok(())
}

#[test]
fn directory_delete_recovery_keeps_original_receipt_and_one_shot_refund() -> TestResult {
    let fixture = fixture_with_tree(true)?;
    let tree = fixture.root.join("tree");
    let before = fixture.authority.snapshot()?;
    require(
        fixture
            .owner
            .recover_directory_delete_if_pending(&tree)?
            .is_none(),
        "missing directory delete invented a receipt",
    )?;
    let ticket = fixture.owner.begin_directory_delete(&tree)?;
    require(
        matches!(
            fixture.owner.recover_directory_delete_if_pending(&tree),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "active directory delete was recovered behind its handle",
    )?;
    drop(ticket);
    start_directory_delete_without_unlink(&fixture, &tree)?;
    require(
        matches!(
            fixture.owner.recover_directory_delete_if_pending(&fixture.root.join("other")),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "different path consumed the original directory delete",
    )?;
    require(
        fixture.owner.recover_directory_delete_if_pending(&tree)? == Some(3),
        "recovered delete lost the original three-byte receipt",
    )?;
    require(!tree.exists(), "recovered delete kept its original tree")?;
    let after = fixture.authority.snapshot()?;
    require(
        after.used_bytes == before.used_bytes.checked_sub(3).ok_or("byte underflow")?
            && after.used_files == before.used_files.checked_sub(1).ok_or("file underflow")?,
        "recovered deletion did not refund exactly one three-byte inode",
    )?;
    require(
        fixture
            .owner
            .recover_directory_delete_if_pending(&tree)?
            .is_none(),
        "settled deletion repeated its receipt",
    )?;
    fs::create_dir(&tree)?;
    fs::write(tree.join("child"), b"foreign")?;
    require(
        fixture
            .owner
            .recover_directory_delete_if_pending(&tree)?
            .is_none(),
        "late replacement reused the original deletion receipt",
    )?;
    require(
        fs::read(tree.join("child"))? == b"foreign",
        "late tree was removed",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "directory delete claim remained",
    )?;
    Ok(())
}

#[test]
fn move_preflight_drains_only_the_original_staging_delete() -> TestResult {
    let fixture = fixture_with_tree(true)?;
    let staging = fixture.root.join("tree");
    let published = fixture.root.join("published-tree");
    let before = fixture.authority.snapshot()?;
    start_directory_delete_without_unlink(&fixture, &staging)?;
    require(
        !fixture
            .owner
            .recover_directory_move_if_pending(&staging, &published)?,
        "completed staging cleanup was misreported as a move",
    )?;
    require(
        !staging.exists(),
        "staging cleanup did not remove its original tree",
    )?;
    require(
        !published.exists(),
        "staging cleanup created the publication target",
    )?;
    require(
        fixture
            .owner
            .recover_directory_delete_if_pending(&staging)?
            .is_none(),
        "move preflight left the original staging delete pending",
    )?;
    let after = fixture.authority.snapshot()?;
    require(
        after.used_bytes == before.used_bytes.checked_sub(3).ok_or("byte underflow")?
            && after.used_files == before.used_files.checked_sub(1).ok_or("file underflow")?,
        "staging cleanup failed to refund its one three-byte inode",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "staging cleanup retained a scope",
    )?;
    Ok(())
}

#[test]
fn control_create_reconcile_preserves_directory_move_and_delete_receipts() -> TestResult {
    let fixture = fixture_with_tree(true)?;
    let source = fixture.root.join("tree");
    let target = fixture.root.join("published-tree");
    let boot = fixture.authority.snapshot()?;

    start_directory_move_without_rename(&fixture, &source, &target)?;
    fs::rename(&source, &target)?;
    require(
        matches!(
            fixture.owner.reconcile_retired_control_creates(),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "cold Control reconciliation consumed a started directory move",
    )?;
    require(
        !source.exists() && fs::read(target.join("child"))? == b"old",
        "cold Control probe changed the moved directory",
    )?;
    require(
        fixture.authority.snapshot()? == boot,
        "cold Control probe changed move charge",
    )?;
    require(
        fixture
            .owner
            .recover_directory_move_if_pending(&source, &target)?,
        "original move receipt was lost to Control reconciliation",
    )?;

    start_directory_delete_without_unlink(&fixture, &target)?;
    require(
        matches!(
            fixture.owner.reconcile_retired_control_creates(),
            Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ),
        "cold Control reconciliation consumed a started directory delete",
    )?;
    require(
        fs::read(target.join("child"))? == b"old",
        "cold Control probe unlinked the original tree",
    )?;
    require(
        fixture.authority.snapshot()? == boot,
        "cold Control probe refunded a still-linked child",
    )?;
    require(
        fixture.owner.recover_directory_delete_if_pending(&target)? == Some(3),
        "original delete receipt was lost to Control reconciliation",
    )?;
    require(!target.exists(), "original delete kept the tree")?;
    let after = fixture.authority.snapshot()?;
    require(
        after.used_bytes == boot.used_bytes.checked_sub(3).ok_or("byte underflow")?
            && after.used_files == boot.used_files.checked_sub(1).ok_or("file underflow")?,
        "directory delete did not refund exactly one three-byte child",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "original namespace claim remained after both receipts",
    )?;
    Ok(())
}

#[test]
fn semantic_finish_interrupted_reclaim_settles_original_move_and_exact_charge() -> TestResult {
    use quanta_index_core::SealedGenerationReclaimPort as _;
    use quanta_index_semantic::{SemanticAdapter, semantic_state_root};

    let fixture = fixture_with_semantic_tree()?;
    let track_root = semantic_state_root(&fixture.root);
    let source = track_root.join("family/g3");
    let entry = quanta_index_core::reclaim_area(&track_root).join("family.g3");
    fixture.binder.ensure_private_parent_directories(&entry)?;
    let boot = fixture.authority.snapshot()?;
    require(
        boot.used_bytes == 3 && boot.used_files == 2 && boot.retained_inodes == 2,
        "semantic fixture boot did not charge its child and zero-byte root lease",
    )?;
    let ticket = fixture.owner.begin_directory_move(&source, &entry)?;
    {
        let mut pending = fixture
            .owner
            .pending
            .lock()
            .map_err(|_error| io::Error::other("namespace slot lock poisoned"))?;
        let Some(PendingOperation::Namespace(prior)) = pending.as_mut() else {
            return Err("semantic reclaim slot was absent".into());
        };
        let NamespaceOperation::DirectoryMove(original) = &mut prior.operation else {
            return Err("semantic reclaim lost its original directory ticket".into());
        };
        original.change_started()?;
        drop(pending);
    }
    fs::rename(&source, &entry)?;
    drop(ticket);
    require(
        !source.exists(),
        "interrupted reclaim kept generation source",
    )?;
    require(
        fs::read(entry.join("child"))? == b"abc",
        "interrupted reclaim changed child bytes",
    )?;

    let maintenance = Arc::new(
        crate::semantic_file_factory::CanonicalSemanticMaintenanceFileFactory::new(Arc::clone(
            &fixture.owner,
        )),
    );
    let adapter =
        SemanticAdapter::with_state_root(track_root)?.with_maintenance_file_admission(maintenance);
    let finished = adapter.finish_interrupted_reclaims(None)?;
    require(
        finished.entries == 1 && finished.bytes == 3,
        "semantic recovery did not report the one original three-byte tree",
    )?;
    fixture.binder.sweep_retired()?;
    require(
        !source.exists() && !entry.exists(),
        "semantic recovery retained source or reclaim entry",
    )?;
    let after = fixture.authority.snapshot()?;
    require(
        after.used_bytes == 0
            && after.used_files == 1
            && after.retained_inodes == 1
            && after.active_allowances == boot.active_allowances
            && after.active_tickets == boot.active_tickets
            && after.uncertain_pending == boot.uncertain_pending,
        "semantic recovery did not restore exact root-lease-only charge",
    )?;
    require(
        fixture
            .root
            .join(".searchd-state-root.lock")
            .metadata()?
            .len()
            == 0,
        "root lease changed after semantic recovery",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "semantic recovery retained original move or delete scope",
    )?;
    Ok(())
}

#[test]
fn generic_move_dispatches_a_fresh_directory_without_file_reopen() -> TestResult {
    let fixture = fixture_with_tree(true)?;
    let source = fixture.root.join("tree");
    let destination = fixture.root.join("moved-tree");
    let old_inode = fs::metadata(&source)?.ino();
    let before = fixture.authority.snapshot()?;
    let mut ticket = fixture.owner.begin_move(&source, &destination)?;
    ticket.perform_and_sync()?;
    require(!source.exists(), "fresh directory move kept old source")?;
    require(
        fs::metadata(&destination)?.ino() == old_inode,
        "fresh directory move replaced its source inode",
    )?;
    require(
        fs::read(destination.join("child"))? == b"old",
        "fresh directory move changed child bytes",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "fresh directory move changed physical charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "fresh directory move retained its scope",
    )?;
    Ok(())
}

#[test]
fn create_only_move_refuses_existing_target_without_replacement() -> TestResult {
    let fixture = fixture()?;
    let source = fixture.root.join("source");
    let target = fixture.root.join("other");
    let source_inode = fs::metadata(&source)?.ino();
    let target_inode = fs::metadata(&target)?.ino();
    let before = fixture.authority.snapshot()?;
    require(
        matches!(
            fixture.owner.begin_move_if_absent(&source, &target),
            Err(CoreError::StorageIo {
                kind: std::io::ErrorKind::AlreadyExists,
                raw_os_error: None,
                ..
            })
        ),
        "create-only move accepted an existing target",
    )?;
    require(
        fs::metadata(&source)?.ino() == source_inode,
        "source changed",
    )?;
    require(
        fs::metadata(&target)?.ino() == target_inode,
        "target changed",
    )?;
    require(fs::read(&source)? == b"abc", "source bytes changed")?;
    require(fs::read(&target)? == b"xyz", "target bytes changed")?;
    require(
        fixture.authority.snapshot()? == before,
        "refused create-only move changed physical charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "refused create-only move retained namespace claim",
    )?;
    Ok(())
}

fn start_staged_move_without_rename(fixture: &Fixture, staged: &Path, target: &Path) -> TestResult {
    let ticket = fixture.owner.begin_staged_move(staged, target)?;
    {
        let mut pending = fixture
            .owner
            .pending
            .lock()
            .map_err(|_error| io::Error::other("namespace slot lock poisoned"))?;
        let Some(PendingOperation::Namespace(prior)) = pending.as_mut() else {
            return Err("staged move slot was absent".into());
        };
        let NamespaceOperation::Path(original) = &mut prior.operation else {
            return Err("staged move lost its original file ticket".into());
        };
        original.change_started()?;
        drop(pending);
    }
    drop(ticket);
    Ok(())
}

#[test]
fn unchanged_staged_move_deletes_only_original_hardlink_alias() -> TestResult {
    let fixture = fixture_with_staged_parent()?;
    let parent = fixture.root.join("target-parent");
    let staged = parent.join("stage");
    let target = parent.join("target");
    let source = fixture.root.join("source");
    let source_inode = fs::metadata(&source)?.ino();
    let target_inode = fs::metadata(&target)?.ino();
    fs::hard_link(&source, &staged)?;
    let before = fixture.authority.snapshot()?;
    start_staged_move_without_rename(&fixture, &staged, &target)?;
    let outcome = fixture.owner.reconcile_pending_namespace();
    require(
        matches!(outcome, Err(CoreError::Storage(message)) if message.contains("not reported")),
        "unchanged staged move did not surface its unreported outcome",
    )?;
    require(!staged.exists(), "original staged alias remained")?;
    require(
        fs::metadata(&source)?.ino() == source_inode,
        "source inode changed",
    )?;
    require(fs::read(&source)? == b"abc", "source bytes changed")?;
    require(
        fs::metadata(&target)?.ino() == target_inode,
        "target inode changed",
    )?;
    require(fs::read(&target)? == b"prior", "target bytes changed")?;
    require(
        fixture.authority.snapshot()? == before,
        "staged alias cleanup changed unique-inode charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "staged cleanup retained claim",
    )?;
    fs::write(&staged, b"foreign")?;
    fixture.owner.reconcile_pending_namespace()?;
    require(
        fs::read(&staged)? == b"foreign",
        "settled staged cleanup touched a later foreign pathname",
    )?;
    Ok(())
}

#[test]
fn changed_staged_move_settles_target_without_alias_cleanup() -> TestResult {
    let fixture = fixture_with_staged_parent()?;
    let parent = fixture.root.join("target-parent");
    let staged = parent.join("stage");
    let target = parent.join("target");
    let source = fixture.root.join("source");
    let source_inode = fs::metadata(&source)?.ino();
    fs::hard_link(&source, &staged)?;
    let before = fixture.authority.snapshot()?;
    start_staged_move_without_rename(&fixture, &staged, &target)?;
    fs::rename(&staged, &target)?;
    fixture.owner.reconcile_pending_namespace()?;
    require(!staged.exists(), "changed move left staged name")?;
    require(
        fs::metadata(&target)?.ino() == source_inode,
        "changed move did not publish source inode",
    )?;
    require(fs::read(&target)? == b"abc", "published bytes changed")?;
    let after = fixture.authority.snapshot()?;
    require(
        after.used_bytes == before.used_bytes.checked_sub(5).ok_or("byte underflow")?,
        "overwritten target width was not retired",
    )?;
    require(
        after.used_files == before.used_files.checked_sub(1).ok_or("file underflow")?,
        "overwritten target inode was not retired",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "changed move retained claim",
    )?;
    Ok(())
}

#[test]
fn failed_staged_alias_unlink_retains_primary_and_original_delete_ticket() -> TestResult {
    let fixture = fixture_with_staged_parent()?;
    let parent = fixture.root.join("target-parent");
    let staged = parent.join("stage");
    let target = parent.join("target");
    let source = fixture.root.join("source");
    let old_inode = fs::metadata(&source)?.ino();
    fs::hard_link(&source, &staged)?;
    let before = fixture.authority.snapshot()?;
    start_staged_move_without_rename(&fixture, &staged, &target)?;
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o500))?;
    let first = fixture.owner.reconcile_pending_namespace();
    require(
        matches!(first, Err(CoreError::Storage(message))
            if message.contains("not reported") && message.contains("cleanup also failed")),
        "failed alias unlink lost primary or cleanup failure",
    )?;
    require(
        fs::metadata(&staged)?.ino() == old_inode,
        "failed unlink changed alias",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "failed unlink refunded original inode",
    )?;
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
    let second = fixture.owner.reconcile_pending_namespace();
    require(
        matches!(second, Err(CoreError::Storage(message))
            if message.contains("not reported") && message.contains("cleanup also failed")),
        "unchanged failed-delete proof lost the original errors",
    )?;
    require(
        fs::metadata(&staged)?.ino() == old_inode,
        "second pass unlinked alias",
    )?;
    let third = fixture.owner.reconcile_pending_namespace();
    require(
        matches!(third, Err(CoreError::Storage(message))
            if message.contains("not reported") && !message.contains("cleanup also failed")),
        "completed cleanup did not return the original move error",
    )?;
    require(!staged.exists(), "retired staged alias remained")?;
    require(
        fs::metadata(&source)?.ino() == old_inode,
        "source inode changed",
    )?;
    require(fs::read(&target)? == b"prior", "target contents changed")?;
    require(
        fixture.authority.snapshot()? == before,
        "staged cleanup changed unique-inode charge",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "staged delete claim remained",
    )?;
    Ok(())
}
