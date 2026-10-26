use std::error::Error;
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt as _;

use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, RequestBudgetV1};
use quanta_index_searchd::app::runtime::StateRootLease;

use super::PublicationFileDeletions;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn retained_post_unlink_ticket_refuses_other_path_and_retries_after_reader_close() -> TestResult {
    let directory = tempfile::tempdir()?;
    let requested = directory.path().join("state");
    fs::create_dir(&requested)?;
    fs::set_permissions(&requested, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&requested)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let blocker = root.join("blocker");
    let target = root.join("target");
    let replacement = root.join("replacement");
    fs::write(&blocker, b"bbb")?;
    fs::write(&target, b"abc")?;
    fs::write(&replacement, b"xyz")?;
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
    let baseline = authority.snapshot()?;
    let binder = PublicationFileCapacity::new(lease, authority.clone(), 16, 1, 256)?;
    let deletions = PublicationFileDeletions::new(binder.clone());

    let opening = binder.begin_reader_open(&blocker)?;
    let reader = opening.opened_owned(File::open(&blocker)?)?;
    deletions.delete_file_and_sync(&blocker)?;
    if authority.snapshot()? != baseline || blocker.exists() {
        return Err("pinned blocker refunded before actual reader close".into());
    }
    let refusal = deletions.delete_file_and_sync(&target);
    if !matches!(
        refusal,
        Err(CoreError::Typed { code, message })
            if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
                && message == "publication file capacity: tracked inode ceiling exceeded"
    ) || target.exists()
        || authority.snapshot()? != baseline
    {
        return Err("post-unlink registry refusal lost the target charge".into());
    }
    if !matches!(
        deletions.delete_file_and_sync(&replacement),
        Err(CoreError::Typed { code, .. }) if code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
    ) || fs::read(&replacement)? != b"xyz"
    {
        return Err("another path bypassed the retained one-operation slot".into());
    }

    // An external same-path replacement is deliberately outside admission.
    // The retained original ticket must refuse it without refunding either.
    fs::hard_link(&replacement, &target)?;
    if !matches!(
        deletions.delete_file_and_sync(&target),
        Err(CoreError::InvalidContract(_))
    ) || authority.snapshot()? != baseline
        || fs::read(&target)? != b"xyz"
    {
        return Err("same-path replacement consumed the pending delete ticket".into());
    }
    fs::remove_file(&target)?;
    reader.retire()?;
    // A subsequent constructor cannot rediscover the absent target by scanning.
    // Drain the retained ticket directly without another unlink or pathname.
    deletions.retry_pending_deletion()?;
    let settled = authority.snapshot()?;
    if settled.used_bytes != baseline.used_bytes.checked_sub(6).ok_or("byte underflow")?
        || settled.used_files != baseline.used_files.checked_sub(2).ok_or("file underflow")?
        || fs::read(&replacement)? != b"xyz"
        || !binder.is_quiescent()?
    {
        return Err(
            "same-ticket retry did not retain the replacement and refund two inodes".into(),
        );
    }
    Ok(())
}
