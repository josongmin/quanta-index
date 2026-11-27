use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::sync::Arc;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_semantic::legacy::layout::{DATASET_STAGING_DIR_NAME, generation_dir};
use quanta_index_semantic::semantic_state_root;

use super::{
    CanonicalSemanticFileAdmissionFactory, SemanticFileAdmissionFactory, SemanticStagingTargetMode,
    staged_file,
};
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn std::error::Error>>;

// A caller that bypasses the binder can make the final name appear after
// admission. Use an already charged inode so it is safe to prove a refund.
// A new uncharged inode must keep the failed mutation under custody.
#[test]
fn staging_put_and_multipart_refund_and_retry_after_final_name_collision() -> TestResult {
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
            max_bytes: 4096,
            max_files: 32,
            max_pending: 32,
            max_entries: 128,
            max_depth: 8,
            max_pending_frames: 16,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let binder = PublicationFileCapacity::new(lease, authority.clone(), 16, 128, 256)?;
    let boot = authority.snapshot()?;
    let parent = binder.reserve_publication_allowance(64, 4)?;
    let factory = CanonicalSemanticFileAdmissionFactory::new(Arc::new(
        PublicationFileDeletions::new(Arc::clone(&binder)),
    ));
    let generation = generation_dir(
        &semantic_state_root(&root),
        &RepoId::new("repo-staging-recovery")?,
        &RevisionId::new("rev-staging-recovery")?,
        ManifestGeneration::new(1),
    );
    let files = factory.for_work(Arc::clone(&parent), &generation, None)?;
    let staging = generation.join(DATASET_STAGING_DIR_NAME);
    let donor = staging.join("existing.lance");
    let original = files
        .put_staging_file(&donor, SemanticStagingTargetMode::Create, &[b"old"])?
        .ok_or_else(|| io::Error::other("staging donor was absent after create"))?;
    let baseline = authority.snapshot()?;
    let available = parent.available_capacity()?;

    let target = staging.join("retry.lance");
    let mutation = binder.begin_create_with_allowance(&target, 3, Arc::clone(&parent))?;
    let put_failure = staged_file::publish(
        mutation,
        &target,
        SemanticStagingTargetMode::Overwrite,
        "semantic staging PUT",
        |writer| {
            writer
                .write_all(b"new")
                .map_err(|error| CoreError::storage_io("test write staged bytes", &error))?;
            fs::hard_link(&donor, &target)
                .map_err(|error| CoreError::storage_io("test create competing name", &error))
        },
    );
    assert!(matches!(
        put_failure,
        Err(CoreError::InvalidContract(message))
            if message == "publication file capacity: new final path unexpectedly exists"
    ));
    assert_eq!(fs::read(&target)?, b"old");
    assert_eq!(fs::metadata(&target)?.ino(), original.ino());
    assert_eq!(authority.snapshot()?, baseline);
    assert_eq!(parent.available_capacity()?, available);
    assert!(binder.is_quiescent()?);
    fs::remove_file(&target)?;
    let put_retry = files
        .put_staging_file(&target, SemanticStagingTargetMode::Overwrite, &[b"new"])?
        .ok_or_else(|| io::Error::other("retry did not publish the staging file"))?;
    assert_ne!(put_retry.ino(), original.ino());
    assert_eq!(fs::read(&target)?, b"new");
    files.remove_staging_file_and_sync(&target)?;
    binder.sweep_retired()?;

    let before_multipart = authority.snapshot()?;
    let multipart_available = parent.available_capacity()?;
    let mut upload = files.begin_staging_upload(&target)?;
    upload.reserve_part(3)?.write(&[b"new"])?;
    fs::hard_link(&donor, &target)?;
    let multipart_failure = upload.complete();
    assert!(matches!(
        multipart_failure,
        Err(CoreError::InvalidContract(message))
            if message == "publication file capacity: new final path unexpectedly exists"
    ));
    assert_eq!(fs::read(&target)?, b"old");
    assert_eq!(fs::metadata(&target)?.ino(), original.ino());
    assert_eq!(authority.snapshot()?, before_multipart);
    assert_eq!(parent.available_capacity()?, multipart_available);
    assert!(binder.is_quiescent()?);
    fs::remove_file(&target)?;

    let mut retry = files.begin_staging_upload(&target)?;
    retry.reserve_part(3)?.write(&[b"new"])?;
    assert_ne!(retry.complete()?.ino(), original.ino());
    assert_eq!(fs::read(&target)?, b"new");
    assert_eq!(fs::read(&donor)?, b"old");
    files.remove_staging_file_and_sync(&target)?;
    files.remove_staging_file_and_sync(&donor)?;
    drop(files);
    drop(parent);
    binder.sweep_retired()?;
    assert_eq!(authority.snapshot()?, boot);
    assert!(binder.is_quiescent()?);
    assert!(fs::read_dir(&staging)?.next().is_none());
    for entry in fs::read_dir(&root)? {
        assert!(
            !entry?
                .file_name()
                .as_encoded_bytes()
                .starts_with(b".qiv-pub-")
        );
    }
    Ok(())
}
