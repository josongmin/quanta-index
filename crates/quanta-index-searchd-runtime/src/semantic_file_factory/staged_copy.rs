//! Immutable dataset copy under the product's physical file authority.

use std::io::{self, Read as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::sync::Arc;

use quanta_index_core::CoreError;
use quanta_index_core::publication_capacity::PublicationAllowance;
use quanta_index_semantic::SemanticStagingTargetMode;

use super::staged_file;
use crate::publication_file_capacity::{PublicationFileCapacity, ReaderLease, append_failure};

fn copy_error(context: &str, error: io::Error) -> CoreError {
    CoreError::storage_io(&format!("semantic staged copy {context}"), &error)
}

/// Copy one selected, charged dataset file into the caller's private staging
/// target. The caller authenticates both path scopes and the target parent.
pub(super) fn copy_staged_file(
    binder: &PublicationFileCapacity,
    source: &Path,
    target: &Path,
    held_work: Arc<PublicationAllowance>,
) -> Result<(), CoreError> {
    binder.validate_held_allowance(&held_work)?;
    let source_lease = binder.begin_reader_open(source)?.into_held_reader()?;
    let result = copy_from_held_reader(binder, &source_lease, target, held_work);
    let retired = source_lease.retire();
    match (result, retired) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Ok(())) | (Ok(()), Err(primary)) => Err(primary),
        (Err(primary), Err(close)) => {
            Err(append_failure(primary, "source reader retirement", &close))
        }
    }
}

fn copy_from_held_reader(
    binder: &PublicationFileCapacity,
    source: &ReaderLease,
    target: &Path,
    held_work: Arc<PublicationAllowance>,
) -> Result<(), CoreError> {
    let source_file = source.file()?;
    let source_meta = source_file
        .metadata()
        .map_err(|error| copy_error("source fstat", error))?;
    if !source_meta.is_file() {
        return Err(CoreError::InvalidContract(
            "semantic staged copy source is not a regular file".into(),
        ));
    }
    let source_width = source_meta.len();
    let mutation = binder.begin_byte_copy_with_allowance(source_file, target, held_work)?;
    staged_file::publish(mutation, target, SemanticStagingTargetMode::Overwrite, "semantic staged copy", |writer| {
        let mut bounded = source_file.take(source_width);
        let copied = io::copy(&mut bounded, writer)
            .map_err(|error| copy_error("copy bounded source", error))?;
        if copied != source_width {
            return Err(CoreError::Storage(format!(
                "semantic staged copy source shortened: expected {source_width} bytes, copied {copied}"
            )));
        }
        let after_copy = source_file
            .metadata()
            .map_err(|error| copy_error("source fstat after copy", error))?;
        if after_copy.len() != source_width
            || after_copy.mtime() != source_meta.mtime()
            || after_copy.mtime_nsec() != source_meta.mtime_nsec()
        {
            return Err(CoreError::Storage(
                "semantic staged copy source changed while copying".into(),
            ));
        }
        writer
            .set_permissions(source_meta.permissions())
            .map_err(|error| copy_error("set source permissions", error))?;
        Ok(())
    })?
    .ok_or_else(|| CoreError::InvalidContract("staged copy replacement lacked output".into()))
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs::{self, OpenOptions};
    use std::io;
    use std::io::Read as _;
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};

    use quanta_index_core::publication_capacity::PublicationCapacity;
    use quanta_index_core::{INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, RequestBudgetV1};
    use quanta_index_searchd::app::runtime::StateRootLease;

    use super::*;
    use crate::publication_file_deletions::PublicationFileDeletions;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

    type TestResult = Result<(), Box<dyn Error>>;

    fn fixture() -> Result<
        (
            tempfile::TempDir,
            PathBuf,
            PublicationCapacity,
            Arc<PublicationFileCapacity>,
        ),
        Box<dyn Error>,
    > {
        let directory = tempfile::tempdir()?;
        let requested_root = directory.path().join("state");
        fs::create_dir(&requested_root)?;
        fs::set_permissions(&requested_root, fs::Permissions::from_mode(0o700))?;
        let lease = StateRootLease::acquire(&requested_root)?;
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
        let binder = PublicationFileCapacity::new(lease, authority.clone(), 16, 64, 256)?;
        Ok((directory, root, authority, binder))
    }

    fn publish_source(binder: &PublicationFileCapacity, path: &Path, bytes: &[u8]) -> TestResult {
        binder.ensure_private_parent_directories(path)?;
        let mut mutation = binder.begin_create(path)?;
        let temp = mutation.allocate_private_temp_path()?;
        mutation.reserve_end(u64::try_from(bytes.len())?)?;
        mutation.creation_started()?;
        let mut writer = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&temp)?;
        mutation.created(&writer, &temp)?;
        writer.write_all(bytes)?;
        mutation.before_replace(path)?;
        fs::rename(&temp, path)?;
        let _retained = mutation.replaced_and_synced(&writer, path)?;
        mutation.confirmed_safe_writer_fd_retired(writer)?;
        let lease = mutation.take_owned_lease()?;
        drop(mutation);
        lease.retire()?;
        Ok(())
    }

    fn no_publication_temp(root: &Path) -> io::Result<bool> {
        Ok(fs::read_dir(root)?.all(|entry| {
            entry.is_ok_and(|entry| !entry.file_name().to_string_lossy().starts_with(".qiv-pub-"))
        }))
    }

    fn leave_renamed_copy_pending(
        binder: &PublicationFileCapacity,
        source: &Path,
        target: &Path,
    ) -> TestResult {
        binder.ensure_private_parent_directories(target)?;
        let source = binder.begin_reader_open(source)?.into_held_reader()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let mut mutation = binder.begin_byte_copy_with_allowance(source.file()?, target, held)?;
        let path = mutation.allocate_private_temp_path()?;
        mutation.retain_control_create_for_recovery()?;
        mutation.creation_started()?;
        let mut writer = mutation.open_declared_temp()?;
        mutation.created(&writer, &path)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        mutation.publish_declared_temp_replace(&writer, target)?;
        // Simulate a failed parent sync. Only the physical close is witnessed.
        mutation.confirmed_safe_writer_fd_retired(writer)?;
        drop(mutation);
        source.retire()?;
        Ok(())
    }

    #[test]
    fn immutable_copy_charges_distinct_inode_and_refuses_small_work_parent() -> TestResult {
        let (_directory, root, authority, binder) = fixture()?;
        let source = root.join("dataset/source.lance");
        let target = root.join("dataset.staging/source.lance");
        publish_source(&binder, &source, b"four")?;
        binder.ensure_private_parent_directories(&target)?;

        let before = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        copy_staged_file(&binder, &source, &target, Arc::clone(&held))?;
        drop(held);
        let copied = authority.snapshot()?;
        let target_inode = fs::metadata(&target)?.ino();
        if fs::read(&source)? != b"four"
            || fs::read(&target)? != b"four"
            || fs::metadata(&source)?.ino() == target_inode
            || fs::metadata(&source)?.permissions().mode() & 0o7777
                != fs::metadata(&target)?.permissions().mode() & 0o7777
            || copied.used_bytes != before.used_bytes + 4
            || copied.used_files != before.used_files + 1
            || !no_publication_temp(&root)?
        {
            return Err(io::Error::other("immutable copy lost physical inode or charge").into());
        }

        let too_small = binder.reserve_publication_allowance(3, 1)?;
        let refusal = copy_staged_file(&binder, &source, &target, Arc::clone(&too_small));
        drop(too_small);
        if !matches!(refusal, Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE)
            || fs::read(&target)? != b"four"
            || fs::metadata(&target)?.ino() != target_inode
            || authority.snapshot()? != copied
            || !no_publication_temp(&root)?
            || !binder.is_quiescent()?
        {
            return Err(io::Error::other("small work parent changed target or temp").into());
        }
        Ok(())
    }

    #[test]
    fn prepublication_copy_refusal_preserves_old_inode_and_allows_retry() -> TestResult {
        let (_directory, root, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let source = root.join("dataset/source.lance");
        let target = root.join("dataset.staging/target.lance");
        publish_source(&binder, &source, b"new")?;
        publish_source(&binder, &target, b"old!")?;
        let source_inode = fs::metadata(&source)?.ino();
        let old_inode = fs::metadata(&target)?.ino();
        let before = authority.snapshot()?;

        let held = binder.reserve_publication_allowance(3, 1)?;
        let reader = binder.begin_reader_open(&source)?.into_held_reader()?;
        let mutation =
            binder.begin_byte_copy_with_allowance(reader.file()?, &target, Arc::clone(&held))?;
        let refusal = {
            let mut bounded = reader.file()?.take(3);
            staged_file::publish(
                mutation,
                &target,
                SemanticStagingTargetMode::Overwrite,
                "semantic staged copy refusal test",
                |writer| {
                    let copied = io::copy(&mut bounded, &mut *writer)
                        .map_err(|error| copy_error("test copy", error))?;
                    if copied != 3 {
                        return Err(CoreError::Storage("test source shortened".into()));
                    }
                    writer
                        .sync_all()
                        .map_err(|error| copy_error("test sync", error))?;
                    Err::<(), _>(CoreError::InvalidContract(
                        "test refusal before publication".into(),
                    ))
                },
            )
            .err()
            .ok_or_else(|| io::Error::other("copy unexpectedly published"))?
        };
        reader.retire()?;
        drop(held);
        if !matches!(refusal, CoreError::InvalidContract(ref detail) if detail == "test refusal before publication")
            || fs::read(&source)? != b"new"
            || fs::read(&target)? != b"old!"
            || fs::metadata(&source)?.ino() != source_inode
            || fs::metadata(&target)?.ino() != old_inode
            || authority.snapshot()? != before
            || !no_publication_temp(target.parent().ok_or("target lacks parent")?)?
            || !binder.is_quiescent()?
        {
            return Err(
                io::Error::other("refused copy retained a temp, charge, or path claim").into(),
            );
        }

        let retry_work = binder.reserve_publication_allowance(3, 1)?;
        copy_staged_file(&binder, &source, &target, Arc::clone(&retry_work))?;
        drop(retry_work);
        let new_inode = fs::metadata(&target)?.ino();
        if fs::read(&target)? != b"new"
            || new_inode == source_inode
            || new_inode == old_inode
            || !no_publication_temp(target.parent().ok_or("target lacks parent")?)?
            || !binder.is_quiescent()?
        {
            return Err(
                io::Error::other("copy retry lost bytes, inode isolation, or custody").into(),
            );
        }
        let deletions = PublicationFileDeletions::new(Arc::clone(&binder));
        deletions.delete_file_and_sync(&target)?;
        deletions.delete_file_and_sync(&source)?;
        if authority.snapshot()? != boot || !binder.is_quiescent()? {
            return Err(io::Error::other("copy retry retirement did not refund capacity").into());
        }
        Ok(())
    }

    #[test]
    fn scoped_retired_copy_recovers_renamed_inode_without_touching_sibling() -> TestResult {
        let (_directory, root, authority, binder) = fixture()?;
        let source = root.join("dataset/hint.json");
        let staging = root.join("generation/dataset.staging");
        let sibling = root.join("generation-other/dataset.staging");
        let target = staging.join("hint.json");
        let sibling_target = sibling.join("hint.json");
        publish_source(&binder, &source, b"four")?;
        leave_renamed_copy_pending(&binder, &source, &target)?;
        leave_renamed_copy_pending(&binder, &source, &sibling_target)?;
        let target_inode = fs::metadata(&target)?.ino();
        let pending_total = authority.snapshot()?.uncertain_pending;
        let deletions = PublicationFileDeletions::new(Arc::clone(&binder));

        deletions.reconcile_retired_creates_under(&staging)?;
        if pending_total != 2
            || authority.snapshot()?.uncertain_pending != 1
            || binder.next_retired_control_create_in(Some(&sibling))?
                != Some(sibling_target.clone())
            || fs::metadata(&target)?.ino() != target_inode
            || fs::read(&target)? != b"four"
        {
            return Err(
                io::Error::other("scoped recovery changed a published or foreign inode").into(),
            );
        }

        deletions.reconcile_retired_creates_under(&sibling)?;
        if authority.snapshot()?.uncertain_pending != 0
            || fs::read(&sibling_target)? != b"four"
            || !binder.is_quiescent()?
        {
            return Err(io::Error::other("retired staging charges did not settle").into());
        }
        Ok(())
    }
}
