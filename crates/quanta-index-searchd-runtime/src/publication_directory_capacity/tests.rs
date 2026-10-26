use super::*;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt as _;

use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
use quanta_index_core::RequestBudgetV1;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    _lease: StateRootLease,
    authority: PublicationCapacity,
    binder: Arc<PublicationFileCapacity>,
}

fn fixture(with_file: bool) -> Result<Fixture, Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let requested = temporary.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    fs::create_dir(root.join("tree"))?;
    if with_file {
        fs::create_dir(root.join("tree/child"))?;
        fs::write(root.join("tree/child/payload"), b"abc")?;
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
    Ok(Fixture {
        _temporary: temporary,
        root,
        _lease: lease,
        authority,
        binder,
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
fn directory_move_existing_target_reports_known_collision_without_mutation() -> TestResult {
    let fixture = fixture(false)?;
    let source = fixture.root.join("tree");
    let target = fixture.root.join("existing-directory");
    fs::create_dir(&target)?;
    let source_identity = fs::metadata(&source)?;
    let target_identity = fs::metadata(&target)?;
    let before = fixture.authority.snapshot()?;
    require(
        matches!(
            fixture.binder.begin_directory_move(&source, &target),
            Err(CoreError::StorageIo {
                kind: std::io::ErrorKind::AlreadyExists,
                raw_os_error: None,
                ..
            })
        ),
        "directory move did not classify an observed target collision",
    )?;
    require(
        source.is_dir() && target.is_dir(),
        "collision changed namespace",
    )?;
    let source_after = fs::metadata(&source)?;
    let target_after = fs::metadata(&target)?;
    require(
        source_identity.dev() == source_after.dev()
            && source_identity.ino() == source_after.ino()
            && target_identity.dev() == target_after.dev()
            && target_identity.ino() == target_after.ino(),
        "collision replaced a directory incarnation",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "collision changed charge",
    )?;
    require(fixture.binder.is_quiescent()?, "collision retained a claim")?;
    Ok(())
}

#[test]
fn deleted_child_cut_resumes_with_original_inode_and_last_reader_refund() -> TestResult {
    let fixture = fixture(true)?;
    let file = fixture.root.join("tree/child/payload");
    let before = fixture.authority.snapshot()?;
    let reader = fixture
        .binder
        .begin_reader_open(&file)?
        .opened_owned(File::open(&file)?)?;
    let mut ticket = fixture
        .binder
        .begin_directory_delete(&fixture.root.join("tree"), &mut || Ok(()))?;
    let mut cut = true;
    let failed = ticket.perform_and_sync_inner(&mut || Ok(()), &mut |boundary| {
        if matches!(boundary, DeleteBoundary::EntryUnlinked) && cut {
            cut = false;
            Err(CoreError::Storage("fixed post-unlink cut".into()))
        } else {
            Ok(())
        }
    });
    require(failed.is_err(), "post-unlink cut must fail")?;
    require(ticket.has_started(), "directory deletion must have started")?;
    require(!file.exists(), "original file must already be unlinked")?;
    require(
        fixture
            .binder
            .begin_directory_delete(&fixture.root.join("tree"), &mut || Ok(()))
            .is_err(),
        "original scope must remain held",
    )?;
    ticket.perform_and_sync(&mut || Ok(()))?;
    require(ticket.is_settled(), "resumed deletion must settle")?;
    require(
        fixture.authority.snapshot()?.used_bytes == before.used_bytes,
        "last reader must retain the original three bytes",
    )?;
    reader.retire()?;
    fixture.binder.sweep_retired()?;
    let after = fixture.authority.snapshot()?;
    require(
        after.used_bytes == before.used_bytes.checked_sub(3).ok_or("byte underflow")?,
        "last reader retirement must refund the original three bytes",
    )?;
    require(
        after.used_files == before.used_files.checked_sub(1).ok_or("file underflow")?,
        "last reader retirement must refund one file",
    )?;
    require(
        fixture.binder.is_quiescent()?,
        "directory claim must release",
    )?;
    Ok(())
}

#[test]
fn deleted_root_cut_resumes_final_sync_without_a_second_unlink() -> TestResult {
    let fixture = fixture(false)?;
    let path = fixture.root.join("tree");
    let before = fixture.authority.snapshot()?;
    let mut ticket = fixture
        .binder
        .begin_directory_delete(&path, &mut || Ok(()))?;
    let mut cut = true;
    let failed = ticket.perform_and_sync_inner(&mut || Ok(()), &mut |boundary| {
        if matches!(boundary, DeleteBoundary::RootUnlinked) && cut {
            cut = false;
            Err(CoreError::Storage("fixed post-root-unlink cut".into()))
        } else {
            Ok(())
        }
    });
    require(failed.is_err(), "post-root-unlink cut must fail")?;
    require(!path.exists(), "original root must already be absent")?;
    ticket.perform_and_sync(&mut || Ok(()))?;
    require(ticket.is_settled(), "root deletion must settle")?;
    require(
        fixture.authority.snapshot()? == before,
        "directory-only deletion must not change regular-file charge",
    )?;
    require(fixture.binder.is_quiescent()?, "root claim must release")?;
    Ok(())
}

#[test]
fn directory_move_reconciles_exact_unchanged_and_moved_namespaces() -> TestResult {
    let fixture = fixture(true)?;
    let from = fixture.root.join("tree");
    let to = fixture.root.join("moved");
    let before = fixture.authority.snapshot()?;
    let mut unchanged = fixture.binder.begin_directory_move(&from, &to)?;
    unchanged.change_started()?;
    require(
        unchanged.reconcile_after_failure()? == DirectoryMoveRecovery::Unchanged,
        "unchanged move must release its original claims",
    )?;
    let mut moved = fixture.binder.begin_directory_move(&from, &to)?;
    moved.change_started()?;
    fs::rename(&from, &to)?;
    require(
        moved.reconcile_after_failure()? == DirectoryMoveRecovery::Moved,
        "moved directory must settle without another rename",
    )?;
    require(
        fs::read(to.join("child/payload"))? == b"abc",
        "moved payload must retain its original bytes",
    )?;
    require(
        fixture.authority.snapshot()? == before,
        "directory move must not change inode charge",
    )?;
    require(fixture.binder.is_quiescent()?, "move scopes must release")?;
    Ok(())
}

#[test]
fn replacement_after_child_unlink_cannot_settle_original_delete() -> TestResult {
    let fixture = fixture(true)?;
    let file = fixture.root.join("tree/child/payload");
    let mut ticket = fixture
        .binder
        .begin_directory_delete(&fixture.root.join("tree"), &mut || Ok(()))?;
    let mut cut = true;
    let failed = ticket.perform_and_sync_inner(&mut || Ok(()), &mut |boundary| {
        if matches!(boundary, DeleteBoundary::EntryUnlinked) && cut {
            cut = false;
            Err(CoreError::Storage("fixed post-unlink cut".into()))
        } else {
            Ok(())
        }
    });
    require(failed.is_err(), "post-unlink cut must fail")?;
    fs::write(&file, b"new")?;
    require(
        ticket.perform_and_sync(&mut || Ok(())).is_err(),
        "foreign replacement must block original settlement",
    )?;
    require(
        fs::read(&file)? == b"new",
        "foreign file must remain intact",
    )?;
    require(
        ticket.has_started() && !ticket.is_settled(),
        "original scope must stay held",
    )?;
    Ok(())
}

#[test]
fn replaced_parent_after_child_unlink_keeps_original_charge_and_scope() -> TestResult {
    let fixture = fixture(true)?;
    let tree = fixture.root.join("tree");
    let original_parent = tree.join("child");
    let moved_parent = tree.join("held-child");
    let before = fixture.authority.snapshot()?;
    let mut ticket = fixture
        .binder
        .begin_directory_delete(&tree, &mut || Ok(()))?;
    let mut cut = true;
    let failed = ticket.perform_and_sync_inner(&mut || Ok(()), &mut |boundary| {
        if matches!(boundary, DeleteBoundary::EntryUnlinked) && cut {
            cut = false;
            Err(CoreError::Storage("fixed post-unlink cut".into()))
        } else {
            Ok(())
        }
    });
    require(failed.is_err(), "post-unlink cut must fail")?;
    fs::rename(&original_parent, &moved_parent)?;
    fs::create_dir(&original_parent)?;
    require(
        ticket.perform_and_sync(&mut || Ok(())).is_err(),
        "new canonical parent cannot settle the held original parent",
    )?;
    require(
        original_parent.is_dir(),
        "foreign parent must remain intact",
    )?;
    require(moved_parent.is_dir(), "original parent must remain intact")?;
    require(
        fixture.authority.snapshot()? == before,
        "unsettled tree must retain original charge",
    )?;
    require(
        ticket.has_started() && !ticket.is_settled(),
        "scope must remain held",
    )?;
    Ok(())
}
