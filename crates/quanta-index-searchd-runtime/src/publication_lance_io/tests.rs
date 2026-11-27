use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lance_io::object_store::{ObjectStoreParams, uri_to_url};
use object_store::ObjectStore as _;
use object_store::{ObjectStoreExt as _, PutMode, PutMultipartOptions, PutPayload};
use quanta_index_core::publication_capacity::{CapacitySnapshot, PublicationCapacity};
use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, RequestBudgetV1};
use quanta_index_searchd::app::runtime::StateRootLease;
use tokio::io::AsyncWriteExt as _;

use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

pub(super) type TestResult = Result<(), Box<dyn Error>>;
pub(super) type Fixture = (
    tempfile::TempDir,
    PathBuf,
    StateRootLease,
    PublicationCapacity,
    Arc<PublicationFileCapacity>,
);

#[expect(
    clippy::disallowed_methods,
    reason = "test-only Lance futures run on a current-thread runtime owned by this helper"
)]
pub(super) fn run_test(future: impl core::future::Future<Output = TestResult>) -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(future)
}

pub(super) fn require(condition: bool, message: &'static str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

pub(super) fn fixture() -> Result<Fixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&root)?;
    // The lease authenticates the canonical path (`/private/var` on macOS).
    // Pass that exact prefix to the binder and Lance file URI.
    let root = lease.state_root_identity_v1().to_path_buf();
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 4096,
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
    Ok((directory, root, lease, authority, binder))
}

pub(super) fn deletion_owner(
    binder: &Arc<PublicationFileCapacity>,
) -> Arc<PublicationFileDeletions> {
    Arc::new(PublicationFileDeletions::new(Arc::clone(binder)))
}

pub(super) fn root_uri(root: &Path) -> io::Result<&str> {
    root.to_str()
        .ok_or_else(|| io::Error::other("test state root is not UTF-8"))
}

pub(super) fn memory_uri(root: &Path) -> io::Result<String> {
    Ok(format!("memory://{}", root_uri(root)?))
}

pub(super) fn contains_budget_cause(error: &(dyn Error + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(current) = source {
        if matches!(
            current.downcast_ref::<CoreError>(),
            Some(CoreError::Typed { code, .. }) if *code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
        ) {
            return true;
        }
        source = current
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .map(|inner| -> &(dyn Error + 'static) { inner })
            .or_else(|| current.source());
    }
    false
}

#[test]
fn real_local_writer_refuses_growth_beyond_held_work_parent() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let disk = regular_file_census(&root)?;
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let (store, staging) =
            candidate_staged_store(&root, deletion_owner(&binder), Arc::clone(&parent), None)?;
        let physical = staging.join("table.lance/data/overgrowth.lance");
        let path = object_store::path::Path::from_absolute_path(&physical)?;
        let mut writer = store.put_multipart(&path).await?;
        writer.put_part(PutPayload::from_static(b"abc")).await?;
        require_one_staged_payload(&root, b"abc")?;
        let admitted = authority.snapshot()?;
        let refusal = writer.put_part(PutPayload::from_static(b"x")).await;
        require(
            matches!(refusal, Err(ref error) if contains_budget_cause(error))
                && !physical.exists()
                && authority.snapshot()? == admitted,
            "product writer growth changed capacity or exposed the final path",
        )?;
        require_one_staged_payload(&root, b"abc")?;
        writer.abort().await?;
        drop(writer);
        drop(store);
        drop(parent);
        binder.sweep_retired()?;
        require(
            regular_file_census(&root)? == disk
                && authority.snapshot()? == boot
                && binder.is_quiescent()?,
            "refused product writer left a staged file, charge, or pin",
        )?;
        Ok(())
    })
}

fn candidate_staged_store(
    root: &Path,
    deletions: Arc<PublicationFileDeletions>,
    held: Arc<quanta_index_core::publication_capacity::PublicationAllowance>,
    base_generation: Option<u64>,
) -> Result<(quanta_index_semantic::AdmittedMemoryWorkStore, PathBuf), Box<dyn Error>> {
    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
    use quanta_index_semantic::legacy::layout::{DATASET_STAGING_DIR_NAME, generation_dir};
    use quanta_index_semantic::{
        AdmittedMemoryWorkStore, SemanticFileAdmissionFactory as _, semantic_state_root,
    };

    let generation = generation_dir(
        &semantic_state_root(root),
        &RepoId::new("repo-staging-write").map_err(io::Error::other)?,
        &RevisionId::new("rev-staging-write").map_err(io::Error::other)?,
        ManifestGeneration::new(3),
    );
    let base = base_generation.map(|number| generation.with_file_name(format!("g{number}")));
    let staging = generation.join(DATASET_STAGING_DIR_NAME);
    let factory =
        crate::semantic_file_factory::CanonicalSemanticFileAdmissionFactory::new(deletions);
    let work = factory.for_work(held, &generation, base.as_deref())?;
    Ok((AdmittedMemoryWorkStore::new(work), staging))
}

#[cfg(feature = "qualified-memory-uri-candidate")]
fn candidate_published_dataset_file(
    root: &Path,
    deletions: Arc<PublicationFileDeletions>,
    held: Arc<quanta_index_core::publication_capacity::PublicationAllowance>,
    generation_number: u64,
    relative: &Path,
    bytes: &[u8],
) -> Result<PathBuf, Box<dyn Error>> {
    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
    use quanta_index_semantic::legacy::layout::{DATASET_STAGING_DIR_NAME, generation_dir};
    use quanta_index_semantic::{SemanticFileAdmissionFactory as _, SemanticStagingTargetMode};

    let generation = generation_dir(
        &quanta_index_semantic::semantic_state_root(root),
        &RepoId::new("repo-staging-write").map_err(io::Error::other)?,
        &RevisionId::new("rev-staging-write").map_err(io::Error::other)?,
        ManifestGeneration::new(generation_number),
    );
    let staging = generation.join(DATASET_STAGING_DIR_NAME);
    let dataset = staging.with_file_name("dataset");
    let factory =
        crate::semantic_file_factory::CanonicalSemanticFileAdmissionFactory::new(deletions);
    let work = factory.for_work(held, &generation, None)?;
    require(
        work.put_staging_file(
            &staging.join(relative),
            SemanticStagingTargetMode::Create,
            &[bytes],
        )?
        .is_some(),
        "product fixture did not create its staging file",
    )?;
    work.begin_move(&staging, &dataset)?.perform_and_sync()?;
    Ok(dataset.join(relative))
}

#[cfg(feature = "qualified-memory-uri-candidate")]
#[test]
fn candidate_staged_put_and_delete_preserve_scope_and_inode_charge() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let outside = root
            .parent()
            .ok_or_else(|| io::Error::other("state root has no parent"))?
            .join("foreign-literal%2Fname");
        fs::write(&outside, b"outside")?;
        let deletions = deletion_owner(&binder);
        let parent = Arc::new(authority.reserve_publication(9, 3)?);
        let (store, staging) =
            candidate_staged_store(&root, Arc::clone(&deletions), Arc::clone(&parent), None)?;
        let physical = staging.join("table.lance/_transactions/literal%2Fname");
        let protected = candidate_published_dataset_file(
            &root,
            Arc::clone(&deletions),
            Arc::clone(&parent),
            3,
            Path::new("table.lance/_transactions/literal%2Fname"),
            b"abc",
        )?;
        require(
            protected
                == staging
                    .with_file_name("dataset")
                    .join("table.lance/_transactions/literal%2Fname"),
            "product fixture published outside the expected generation dataset",
        )?;
        let encoded = object_store::path::Path::from_absolute_path(&physical)?;
        let foreign = object_store::path::Path::from_absolute_path(&outside)?;
        let published = object_store::path::Path::from_absolute_path(&protected)?;
        let created = store
            .put_opts(
                &encoded,
                PutPayload::from_static(b"abc"),
                PutMode::Create.into(),
            )
            .await?;
        require(
            fs::read(&physical)? == b"abc" && fs::read(&outside)? == b"outside",
            "encoded path selected another physical file",
        )?;
        require(
            created.e_tag.is_some() && created.e_tag == store.head(&encoded).await?.e_tag,
            "created ETag differs from the staged inode's HEAD",
        )?;
        let created_snapshot = authority.snapshot()?;
        require(
            matches!(
                store
                    .put_opts(
                        &encoded,
                        PutPayload::from_static(b"bad"),
                        PutMode::Create.into()
                    )
                    .await,
                Err(object_store::Error::AlreadyExists { .. })
            ) && authority.snapshot()? == created_snapshot
                && fs::read(&physical)? == b"abc",
            "create conflict mutated the staging file or capacity",
        )?;
        require(
            store
                .put_opts(
                    &published,
                    PutPayload::from_static(b"bad"),
                    PutMode::Overwrite.into(),
                )
                .await
                .is_err()
                && authority.snapshot()? == created_snapshot
                && fs::read(&protected)? == b"abc",
            "candidate staging PUT crossed into the published dataset",
        )?;
        let replaced = store
            .put_opts(
                &encoded,
                PutPayload::from_static(b"new"),
                PutMode::Overwrite.into(),
            )
            .await?;
        require(
            replaced.e_tag != created.e_tag
                && replaced.e_tag == store.head(&encoded).await?.e_tag
                && fs::read(&physical)? == b"new",
            "overwrite did not atomically install a new tagged inode",
        )?;
        let committed = authority.snapshot()?;
        require(
            store.delete(&foreign).await.is_err()
                && fs::read(&outside)? == b"outside"
                && authority.snapshot()? == committed,
            "candidate delete crossed the admitted state root",
        )?;
        require(
            store.delete(&published).await.is_err()
                && fs::read(&protected)? == b"abc"
                && authority.snapshot()? == committed,
            "candidate staging delete crossed into the published dataset",
        )?;
        store.delete(&encoded).await?;
        require(
            matches!(
                store.delete(&encoded).await,
                Err(object_store::Error::NotFound { .. })
            ),
            "missing staging file lost its typed NotFound",
        )?;
        drop(store);
        drop(parent);
        deletions.delete_file_and_sync(&protected)?;
        binder.sweep_retired()?;
        require(
            !physical.exists() && authority.snapshot()? == boot,
            "candidate delete did not restore the admitted baseline",
        )?;
        Ok(())
    })
}

#[cfg(feature = "qualified-memory-uri-candidate")]
#[test]
fn candidate_staging_multipart_orders_parts_and_recovers_aborted_writes() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let baseline = authority.snapshot()?;
        let disk = regular_file_census(&root)?;
        let parent = Arc::new(authority.reserve_publication(10, 3)?);
        let (store, staging) =
            candidate_staged_store(&root, deletion_owner(&binder), Arc::clone(&parent), None)?;
        let target = staging.join("table.lance/data/committed.lance");
        let path = object_store::path::Path::from_absolute_path(&target)?;
        let mut upload = store.put_multipart(&path).await?;
        let first = upload.put_part(PutPayload::from_static(b"one"));
        let second = upload.put_part(PutPayload::from_static(b"two"));
        second.await?;
        require(!target.exists(), "part write exposed the final name")?;
        first.await?;
        let committed = upload.complete().await?;
        require(
            fs::read(&target)? == b"onetwo"
                && committed.e_tag.is_some()
                && committed.e_tag == store.head(&path).await?.e_tag,
            "out-of-order part writes changed payload order or final ETag",
        )?;

        let cancelled = staging.join("table.lance/data/cancelled.lance");
        let cancelled_path = object_store::path::Path::from_absolute_path(&cancelled)?;
        let mut upload = store.put_multipart(&cancelled_path).await?;
        let denied = upload
            .put_part(PutPayload::from_static(b"over-budget"))
            .await;
        require(
            matches!(denied, Err(ref error) if contains_budget_cause(error)),
            "multipart growth escaped held Work or lost the typed refusal",
        )?;
        let waiting = upload.put_part(PutPayload::from_static(b"one"));
        upload.abort().await?;
        require(
            waiting.await.is_err() && !cancelled.exists(),
            "queued part wrote after abort or exposed its target",
        )?;

        let mut incomplete = store.put_multipart(&cancelled_path).await?;
        let unpolled = incomplete.put_part(PutPayload::from_static(b"one"));
        require(
            incomplete.complete().await.is_err() && unpolled.await.is_err() && !cancelled.exists(),
            "completion published a reservation whose payload never ran",
        )?;
        store.delete(&path).await?;
        let mut abandoned = store.put_multipart(&cancelled_path).await?;
        abandoned.put_part(PutPayload::from_static(b"one")).await?;
        let staged_disk = regular_file_census(&root)?;
        require(
            !cancelled.exists()
                && disk.0.checked_add(3) == Some(staged_disk.0)
                && disk.1.checked_add(1) == Some(staged_disk.1),
            "written part did not remain in one private staging inode before drop",
        )?;
        let late = abandoned.put_part(PutPayload::from_static(b"x"));
        drop(abandoned);
        drop(store);
        drop(parent);
        wait_for_boot_refund(&authority, baseline).await?;
        binder.sweep_retired()?;
        require(
            late.await.is_err()
                && !cancelled.exists()
                && !target.exists()
                && authority.snapshot()? == baseline
                && regular_file_census(&root)? == disk
                && binder.is_quiescent()?,
            "multipart completion or abort failed to restore capacity",
        )
        .map_err(Into::into)
    })
}

#[cfg(feature = "qualified-memory-uri-candidate")]
#[test]
fn candidate_staging_copy_and_rename_preserve_scope_and_shared_inode() -> TestResult {
    use std::os::unix::fs::MetadataExt as _;

    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let parent = Arc::new(authority.reserve_publication(7, 2)?);
        let (store, staging) =
            candidate_staged_store(&root, deletion_owner(&binder), Arc::clone(&parent), None)?;
        let source = staging.join("table.lance/data/source.lance");
        let target = staging.join("table.lance/data/target.lance");
        let alias = staging.join("table.lance/data/alias.lance");
        let moved = staging.join("table.lance/data/moved.lance");
        let out_of_scope = staging.with_file_name("dataset").join("escape.lance");
        let source_path = object_store::path::Path::from_absolute_path(&source)?;
        let target_path = object_store::path::Path::from_absolute_path(&target)?;
        let alias_path = object_store::path::Path::from_absolute_path(&alias)?;
        let moved_path = object_store::path::Path::from_absolute_path(&moved)?;
        let foreign_path = object_store::path::Path::from_absolute_path(&out_of_scope)?;
        let _source_put = store
            .put(&source_path, PutPayload::from_static(b"new"))
            .await?;
        let _target_put = store
            .put(&target_path, PutPayload::from_static(b"old!"))
            .await?;
        let both = authority.snapshot()?;

        require(
            matches!(
                store.copy_if_not_exists(&source_path, &target_path).await,
                Err(object_store::Error::AlreadyExists { .. })
            ) && fs::read(&target)? == b"old!"
                && authority.snapshot()? == both,
            "create-only copy replaced the destination or changed the charge",
        )?;
        store.copy_if_not_exists(&source_path, &alias_path).await?;
        require(
            authority.snapshot()? == both
                && fs::metadata(&source)?.ino() == fs::metadata(&alias)?.ino(),
            "copy allocated a separate inode or charge",
        )?;
        store.copy(&source_path, &target_path).await?;
        require(
            fs::read(&target)? == b"new"
                && fs::metadata(&source)?.ino() == fs::metadata(&target)?.ino()
                && fs::metadata(&source)?.nlink() == 3,
            "overwrite copy changed the source or left a staging alias",
        )?;
        require(
            matches!(
                store.rename_if_not_exists(&alias_path, &target_path).await,
                Err(object_store::Error::AlreadyExists { .. })
            ),
            "create-only rename did not report its existing inode alias",
        )?;
        require(
            store.rename(&alias_path, &foreign_path).await.is_err()
                && store.copy(&source_path, &foreign_path).await.is_err()
                && !out_of_scope.exists(),
            "staging namespace mutation reached the sealed dataset scope",
        )?;
        store.rename_if_not_exists(&alias_path, &moved_path).await?;
        require(
            !alias.exists() && fs::metadata(&moved)?.ino() == fs::metadata(&source)?.ino(),
            "rename did not move the original alias",
        )?;
        require(
            matches!(
                store.copy(&alias_path, &target_path).await,
                Err(object_store::Error::NotFound { .. })
            ),
            "missing staging source lost its typed NotFound",
        )?;
        for path in [&source_path, &target_path, &moved_path] {
            store.delete(path).await?;
        }
        drop(store);
        drop(parent);
        binder.sweep_retired()?;
        require(
            authority.snapshot()? == boot,
            "namespace mutations leaked a retained inode",
        )?;
        Ok(())
    })
}

#[cfg(feature = "qualified-memory-uri-candidate")]
async fn collect_candidate_work_list(
    store: &quanta_index_semantic::AdmittedMemoryWorkStore,
    prefix: &object_store::path::Path,
    offset: Option<&object_store::path::Path>,
) -> Result<Vec<object_store::ObjectMeta>, object_store::Error> {
    let mut entries = match offset {
        Some(offset) => store.list_with_offset(Some(prefix), offset),
        None => store.list(Some(prefix)),
    };
    let mut objects = Vec::new();
    while let Some(entry) =
        std::future::poll_fn(|context| entries.as_mut().poll_next(context)).await
    {
        objects.push(entry?);
    }
    Ok(objects)
}

#[cfg(feature = "qualified-memory-uri-candidate")]
#[test]
fn candidate_work_reads_staging_and_declared_base_without_consuming_a_move() -> TestResult {
    use std::os::unix::fs::symlink;

    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let deletions = deletion_owner(&binder);
        let held = Arc::new(authority.reserve_publication(16, 4)?);
        let (store, staging) =
            candidate_staged_store(&root, Arc::clone(&deletions), Arc::clone(&held), Some(1))?;
        let base = candidate_published_dataset_file(
            &root,
            Arc::clone(&deletions),
            Arc::clone(&held),
            1,
            Path::new("table.lance/base.lance"),
            b"base",
        )?;
        let foreign = candidate_published_dataset_file(
            &root,
            Arc::clone(&deletions),
            Arc::clone(&held),
            2,
            Path::new("table.lance/foreign.lance"),
            b"out",
        )?;
        let generation = staging
            .parent()
            .ok_or("fixture generation directory is absent")?;
        require(
            base == generation
                .with_file_name("g1")
                .join("dataset/table.lance/base.lance")
                && foreign
                    == generation
                        .with_file_name("g2")
                        .join("dataset/table.lance/foreign.lance"),
            "product fixture published outside the declared base or foreign generation",
        )?;
        let staged = staging.join("table.lance/staged.lance");
        let moved = staging.join("table.lance/moved.lance");
        let table = staging.join("table.lance");
        let nested = table.join("z-nested/data.lance");
        let empty = table.join("empty");
        let upload_temporary = table.join("payload#1");
        let base_path = object_store::path::Path::from_absolute_path(&base)?;
        let foreign_path = object_store::path::Path::from_absolute_path(&foreign)?;
        let staged_path = object_store::path::Path::from_absolute_path(&staged)?;
        let moved_path = object_store::path::Path::from_absolute_path(&moved)?;
        let table_path = object_store::path::Path::from_absolute_path(&table)?;
        let nested_path = object_store::path::Path::from_absolute_path(&nested)?;
        let empty_path = object_store::path::Path::from_absolute_path(&empty)?;

        let staged_put = store
            .put(&staged_path, PutPayload::from_static(b"stage"))
            .await?;
        let _nested_put = store
            .put(&nested_path, PutPayload::from_static(b"data"))
            .await?;
        fs::create_dir(&empty)?;
        fs::set_permissions(&empty, fs::Permissions::from_mode(0o700))?;
        fs::write(&upload_temporary, [])?;

        let delimiter = store.list_with_delimiter(Some(&table_path)).await?;
        let expected_prefixes = [empty_path, table_path.clone().join("z-nested")];
        require(
            delimiter.common_prefixes == expected_prefixes
                && delimiter.objects.as_slice() == [store.head(&staged_path).await?],
            "delimiter lost the empty directory or disclosed a temporary upload",
        )?;
        let listed = collect_candidate_work_list(&store, &table_path, None).await?;
        let after_offset =
            collect_candidate_work_list(&store, &table_path, Some(&staged_path)).await?;
        require(
            listed
                .iter()
                .map(|object| &object.location)
                .eq([&staged_path, &nested_path])
                && after_offset
                    .iter()
                    .map(|object| &object.location)
                    .eq([&nested_path]),
            "recursive list order or exclusive offset changed",
        )?;
        let missing = table_path.clone().join("absent");
        for prefix in [&staged_path, &missing] {
            require(
                collect_candidate_work_list(&store, prefix, None)
                    .await?
                    .is_empty(),
                "missing or file prefix enumerated its parent",
            )?;
        }
        let base_table = object_store::path::Path::from_absolute_path(
            base.parent().ok_or("base table absent")?,
        )?;
        let foreign_table = object_store::path::Path::from_absolute_path(
            foreign.parent().ok_or("foreign table absent")?,
        )?;
        require(
            store
                .list_with_delimiter(Some(&base_table))
                .await?
                .objects
                .as_slice()
                == [store.head(&base_path).await?]
                && store
                    .list_with_delimiter(Some(&foreign_table))
                    .await
                    .is_err()
                && store.list_with_delimiter(None).await.is_err(),
            "work listing escaped the staging or declared base scope",
        )?;
        let unsafe_entry = table.join("foreign-link");
        symlink(&foreign, &unsafe_entry)?;
        require(
            store.list_with_delimiter(Some(&table_path)).await.is_err(),
            "work listing accepted a symlink child",
        )?;
        fs::remove_file(unsafe_entry)?;
        fs::remove_file(upload_temporary)?;

        let ranges = store.get_ranges(&base_path, &[2..10, 0..2]).await?;
        let [tail, start] = ranges.as_slice() else {
            return Err("base read returned the wrong number of ranges".into());
        };
        require(
            tail.as_ref() == b"se" && start.as_ref() == b"ba",
            "base range ordering or EOF clipping changed",
        )?;
        require(
            store.head(&staged_path).await?.e_tag == staged_put.e_tag
                && store.get(&staged_path).await?.bytes().await?.as_ref() == b"stage",
            "staging HEAD or body did not use the staged inode",
        )?;
        require(
            matches!(
                store
                    .get_opts(
                        &staged_path,
                        object_store::GetOptions {
                            if_match: Some("stale-etag".to_owned()),
                            ..object_store::GetOptions::default()
                        }
                    )
                    .await,
                Err(object_store::Error::Precondition { .. })
            ),
            "staging GET accepted a stale ETag",
        )?;
        require(
            store.head(&foreign_path).await.is_err()
                && store.get_ranges(&foreign_path, &[0..1]).await.is_err()
                && store.get_ranges(&base_path, &[9..10]).await.is_err(),
            "work reader crossed generations or accepted an invalid range",
        )?;

        let mut original = deletions.begin_file_move(&staged, &moved, true)?;
        original.perform_move()?;
        drop(original);
        require(
            store.head(&moved_path).await.is_err()
                && store.list_with_delimiter(Some(&table_path)).await.is_err(),
            "work observation consumed an unfinished move's original receipt",
        )?;
        store.rename(&staged_path, &moved_path).await?;
        require(
            store.head(&moved_path).await?.e_tag == staged_put.e_tag,
            "same-path retry did not settle the original inode",
        )?;

        store.delete(&moved_path).await?;
        store.delete(&nested_path).await?;
        for path in [&base, &foreign] {
            deletions.delete_file_and_sync(path)?;
        }
        drop(store);
        drop(held);
        binder.sweep_retired()?;
        require(
            authority.snapshot()? == boot && binder.is_quiescent()?,
            "work reader left retained fixture capacity or reader custody",
        )?;
        Ok(())
    })
}

#[test]
fn readonly_and_foreign_parent_refuse_before_namespace_creation() -> TestResult {
    use quanta_index_semantic::{AdmittedMemorySealedStore, SemanticFileAdmissionFactory as _};

    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let before = authority.snapshot()?;
        let deletions = deletion_owner(&binder);
        let factory = crate::semantic_file_factory::CanonicalSemanticFileAdmissionFactory::new(
            Arc::clone(&deletions),
        );
        let dataset = root.join("not-created/deep/dataset");
        let store = AdmittedMemorySealedStore::new(
            &dataset,
            Vec::new(),
            factory.control_reader_lifetime(),
        )?;
        let physical = dataset.join("data.lance");
        let path = object_store::path::Path::from_absolute_path(&physical)?;
        for result in [
            store
                .put_opts(&path, PutPayload::from_static(b"x"), PutMode::Create.into())
                .await
                .map(|_| ()),
            store
                .put_multipart_opts(&path, PutMultipartOptions::default())
                .await
                .map(|_| ()),
            store.delete(&path).await,
        ] {
            let error = result
                .err()
                .ok_or_else(|| io::Error::other("sealed write was admitted"))?;
            require(
                matches!(error, object_store::Error::Generic { source, .. }
                    if source.downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied)),
                "sealed mutation lost its permission denial",
            )?;
        }
        require(
            !root.join("not-created").exists()
                && !physical.exists()
                && authority.snapshot()? == before,
            "sealed mutation created a namespace or changed capacity charges",
        )?;

        let (_other_directory, _other_root, _other_lease, foreign, _other_binder) = fixture()?;
        let foreign_parent = Arc::new(foreign.reserve_publication(1, 1)?);
        let foreign_before = foreign.snapshot()?;
        let refusal = candidate_staged_store(&root, deletions, Arc::clone(&foreign_parent), None)
            .err()
            .ok_or_else(|| io::Error::other("foreign work parent entered the product store"))?;
        require(
            matches!(
                refusal.downcast_ref::<CoreError>(),
                Some(CoreError::InvalidContract(_))
            ) && authority.snapshot()? == before
                && foreign.snapshot()? == foreign_before
                && !root.join("indexes").exists(),
            "foreign work parent changed a ledger or created a namespace before validation",
        )?;
        Ok(())
    })
}

async fn real_local_put_opts_mode(overwrite: bool) -> TestResult {
    use std::os::unix::fs::MetadataExt as _;

    let (_directory, root, lease, authority, binder) = fixture()?;
    let boot = authority.snapshot()?;
    let disk = regular_file_census(&root)?;
    let deletions = deletion_owner(&binder);
    let parent = Arc::new(authority.reserve_publication(8, if overwrite { 2 } else { 1 })?);
    let (store, staging) =
        candidate_staged_store(&root, Arc::clone(&deletions), Arc::clone(&parent), None)?;
    let physical = staging.join("table.lance/data/put-opts.lance");
    let path = object_store::path::Path::from_absolute_path(&physical)?;
    let replaced = if overwrite {
        let _seed = store
            .put_opts(
                &path,
                PutPayload::from_static(b"old"),
                PutMode::Create.into(),
            )
            .await?;
        Some((fs::metadata(&physical)?.ino(), store.get(&path).await?))
    } else {
        None
    };
    let mode = if overwrite {
        PutMode::Overwrite
    } else {
        PutMode::Create
    };
    let published = store
        .put_opts(&path, PutPayload::from_static(b"abc"), mode.into())
        .await?;
    let metadata = fs::metadata(&physical)?;
    let reader = store.get(&path).await?;
    require(
        fs::read(&physical)? == b"abc"
            && metadata.len() == 3
            && published.e_tag.is_some()
            && reader.meta.e_tag == published.e_tag,
        "product put did not publish and open the exact committed output",
    )?;
    if let Some((old_inode, old_reader)) = replaced {
        require(
            old_inode != metadata.ino() && old_reader.bytes().await?.as_ref() == b"old",
            "replacement mutated the reader's old inode or reused its pathname",
        )?;
    }
    drop(store);
    drop(parent);
    binder.sweep_retired()?;
    let committed = authority.snapshot()?;
    require(
        boot.used_bytes.checked_add(3) == Some(committed.used_bytes)
            && boot.used_files.checked_add(1) == Some(committed.used_files)
            && boot.retained_inodes.checked_add(1) == Some(committed.retained_inodes)
            && committed.active_allowances == boot.active_allowances
            && committed.active_tickets == boot.active_tickets
            && regular_file_census(&root)? == (committed.used_bytes, committed.used_files),
        "product put left a replaced inode, unused allowance, or pending ticket",
    )?;

    let retire_parent = Arc::new(authority.reserve_publication(1, 1)?);
    let (retirement, _) =
        candidate_staged_store(&root, deletions, Arc::clone(&retire_parent), None)?;
    retirement.delete(&path).await?;
    drop(retirement);
    drop(retire_parent);
    require(
        !physical.exists()
            && regular_file_census(&root)? == disk
            && authority.snapshot()? == committed
            && !binder.is_quiescent()?,
        "unlink exposed the name or refunded an active reader's inode",
    )?;
    require(
        reader.bytes().await?.as_ref() == b"abc",
        "unlinked product reader lost the original file bytes",
    )?;
    binder.sweep_retired()?;
    require(
        authority.snapshot()? == boot && binder.is_quiescent()?,
        "last product reader did not refund the inode",
    )?;
    drop(binder);
    drop(authority);
    drop(lease);

    let reacquired = StateRootLease::acquire(&root)?;
    let recovered = seed_publication_capacity(
        &reacquired,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 4096,
            max_files: 32,
            max_pending: 32,
            max_entries: 64,
            max_depth: 8,
            max_pending_frames: 16,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    require(
        recovered.snapshot()? == boot,
        "root handoff changed inventory",
    )?;
    let _recovered_binder = PublicationFileCapacity::new(reacquired, recovered, 16, 64, 256)?;
    Ok(())
}

#[test]
fn real_local_put_opts_create_keeps_original_writer_fd() -> TestResult {
    run_test(real_local_put_opts_mode(false))
}

#[test]
fn real_local_put_opts_overwrite_keeps_original_writer_fd() -> TestResult {
    run_test(real_local_put_opts_mode(true))
}

pub(super) async fn wait_for_boot_refund(
    authority: &PublicationCapacity,
    boot: CapacitySnapshot,
) -> TestResult {
    for _ in 0..500 {
        if authority.snapshot()? == boot {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(io::Error::other("detached cleanup did not restore exact boot capacity").into())
}

pub(super) fn require_one_staged_payload(root: &Path, expected: &[u8]) -> TestResult {
    let mut staged = 0_u64;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if entry.file_name() == ".searchd-state-root.lock" || kind.is_dir() {
            continue;
        }
        require(
            kind.is_file() && fs::read(entry.path())? == expected,
            "multipart staging did not contain the exact payload",
        )?;
        staged = staged
            .checked_add(1)
            .ok_or_else(|| io::Error::other("staging entry count overflow"))?;
    }
    require(staged == 1, "multipart upload had no unique staged inode")?;
    Ok(())
}

#[test]
fn candidate_memory_scheme_put_refuses_overgrowth_and_reclaims_through_binder() -> TestResult {
    use std::os::unix::fs::MetadataExt as _;

    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let deletions = deletion_owner(&binder);
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let (store, staging) =
            candidate_staged_store(&root, Arc::clone(&deletions), Arc::clone(&parent), None)?;
        let physical = staging.join("semantic.lance/_versions/selected.manifest");
        let selected = object_store::path::Path::from_absolute_path(&physical)?;
        let error = store
            .put_opts(
                &selected,
                PutPayload::from_static(b"abcd"),
                PutMode::Create.into(),
            )
            .await
            .err()
            .ok_or_else(|| io::Error::other("candidate over-budget put succeeded"))?;
        require(
            contains_budget_cause(&error) && !physical.exists(),
            "candidate refusal lost the typed budget cause or published the original target",
        )?;
        drop(store);
        drop(parent);
        require(
            authority.snapshot()? == boot
                && regular_file_census(&root)? == (boot.used_bytes, boot.used_files),
            "candidate over-budget put left a temporary or retained capacity",
        )?;

        let retry_parent = Arc::new(authority.reserve_publication(3, 1)?);
        let (retry_store, _) = candidate_staged_store(
            &root,
            Arc::clone(&deletions),
            Arc::clone(&retry_parent),
            None,
        )?;
        let _written = retry_store
            .put_opts(
                &selected,
                PutPayload::from_static(b"abc"),
                PutMode::Create.into(),
            )
            .await?;
        let inode = fs::metadata(&physical)?.ino();

        let collision_parent = Arc::new(authority.reserve_publication(3, 1)?);
        let (collision_store, _) = candidate_staged_store(
            &root,
            Arc::clone(&deletions),
            Arc::clone(&collision_parent),
            None,
        )?;
        let before_collision = authority.snapshot()?;
        let collision = collision_store
            .put_opts(
                &selected,
                PutPayload::from_static(b"xyz"),
                PutMode::Create.into(),
            )
            .await;
        require(
            matches!(collision, Err(object_store::Error::AlreadyExists { .. }))
                && fs::read(&physical)? == b"abc"
                && fs::metadata(&physical)?.ino() == inode
                && authority.snapshot()? == before_collision,
            "candidate create collision changed the payload, inode, or physical charge",
        )?;
        drop(collision_store);
        drop(collision_parent);
        drop(retry_store);
        drop(retry_parent);
        binder.sweep_retired()?;
        let committed = authority.snapshot()?;
        require(
            boot.used_bytes.checked_add(3) == Some(committed.used_bytes)
                && boot.used_files.checked_add(1) == Some(committed.used_files)
                && boot.retained_inodes.checked_add(1) == Some(committed.retained_inodes)
                && regular_file_census(&root)? == (committed.used_bytes, committed.used_files),
            "candidate retry ledger differs from physical bytes",
        )?;

        let retirement_parent = Arc::new(authority.reserve_publication(1, 1)?);
        let (retirement_store, _) =
            candidate_staged_store(&root, deletions, Arc::clone(&retirement_parent), None)?;
        retirement_store.delete(&selected).await?;
        require(
            staging.is_dir() && !physical.exists(),
            "candidate cleanup removed the staging directory or left the file visible",
        )?;
        drop(retirement_store);
        drop(retirement_parent);
        binder.sweep_retired()?;
        require(
            authority.snapshot()? == boot
                && regular_file_census(&root)? == (boot.used_bytes, boot.used_files),
            "candidate cleanup did not restore boot charge",
        )?;
        Ok(())
    })
}

#[test]
fn candidate_memory_scheme_reopens_existing_file_table_read_only() -> TestResult {
    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::SemanticIndexOpenPort as _;
    use quanta_index_semantic::{
        SemanticAdapter, build_resident_batch_v1, legacy_chunk_embedding_record_v1,
        sealed_replace_batch_v1, semantic_state_root,
    };

    #[cfg(target_os = "linux")]
    fn generation_fd_count(generation_dir: &Path) -> io::Result<usize> {
        let mut count = 0;
        for entry in fs::read_dir("/proc/self/fd")? {
            match fs::read_link(entry?.path()) {
                Ok(path) if path.starts_with(generation_dir) => count += 1,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(count)
    }

    let directory = tempfile::tempdir()?;
    let root = directory.path().join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&root)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let repo = RepoId::new("repo-file-interop").map_err(io::Error::other)?;
    let revision = RevisionId::new("rev-file-interop").map_err(io::Error::other)?;
    let generation = ManifestGeneration::new(1);
    let path = "src/file_interop.rs";
    let record = legacy_chunk_embedding_record_v1("file-interop", path, vec![1.0, 0.0, 0.0])
        .map_err(io::Error::other)?;
    let writer = SemanticAdapter::with_state_root(semantic_state_root(&root))?;
    build_resident_batch_v1(
        &writer,
        &sealed_replace_batch_v1(
            repo.clone(),
            revision.clone(),
            generation,
            path,
            vec![record],
            3,
        ),
    )?;
    drop(writer);

    #[cfg(target_os = "linux")]
    let generation_dir = quanta_index_semantic::legacy::layout::generation_dir(
        &semantic_state_root(&root),
        &repo,
        &revision,
        generation,
    );
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes: 16 * 1024 * 1024,
            max_files: 256,
            max_pending: 256,
            max_entries: 4096,
            max_depth: 16,
            max_pending_frames: 32,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let binder = PublicationFileCapacity::new(lease, authority.clone(), 512, 4096, 256)?;
    let baseline = authority.snapshot()?;
    require(
        baseline.used_files > 1
            && regular_file_census(&root)? == (baseline.used_bytes, baseline.used_files),
        "unqualified File build did not produce an inventoried sealed generation",
    )?;
    #[cfg(target_os = "linux")]
    require(
        generation_fd_count(&generation_dir)? == 0,
        "unqualified File writer retained a generation descriptor after drop",
    )?;

    let deletions = deletion_owner(&binder);
    let factory = Arc::new(
        crate::semantic_file_factory::CanonicalSemanticFileAdmissionFactory::new(Arc::clone(
            &deletions,
        )),
    );
    let maintenance = Arc::new(
        crate::semantic_file_factory::CanonicalSemanticMaintenanceFileFactory::new(deletions),
    );
    let reader = SemanticAdapter::with_state_root(semantic_state_root(&root))?
        .with_qualified_memory_uri_candidate(factory, maintenance);
    let searcher = reader.open(&repo, &revision, generation)?;
    let hits = searcher.search_hits(&[1.0, 0.0, 0.0], 1, &RequestBudgetV1::unbounded())?;
    require(
        hits.len() == 1
            && hits.first().is_some_and(|hit| {
                hit.record_id == "record-file-interop" && hit.owner_id == "owner-file-interop"
            }),
        "qualified candidate did not reopen the File-built row identity",
    )?;
    require(
        authority.snapshot()? == baseline
            && regular_file_census(&root)? == (baseline.used_bytes, baseline.used_files),
        "qualified candidate read changed the inventoried File generation",
    )?;
    drop(searcher);
    drop(reader);
    binder.sweep_retired()?;
    require(
        binder.is_quiescent()?
            && authority.snapshot()? == baseline
            && regular_file_census(&root)? == (baseline.used_bytes, baseline.used_files),
        "qualified candidate reader retained an FD pin or changed File generation charges",
    )?;
    #[cfg(target_os = "linux")]
    require(
        generation_fd_count(&generation_dir)? == 0,
        "qualified candidate retained a generation descriptor after drop",
    )?;
    Ok(())
}

#[test]
fn real_local_multipart_abort_refunds_and_releases_root() -> TestResult {
    run_test(async {
        let (_directory, root, lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let disk = regular_file_census(&root)?;
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let (store, staging) =
            candidate_staged_store(&root, deletion_owner(&binder), Arc::clone(&parent), None)?;
        let physical = staging.join("table.lance/data/aborted.lance");
        let path = object_store::path::Path::from_absolute_path(&physical)?;
        let mut upload = store.put_multipart(&path).await?;
        upload.put_part(PutPayload::from_static(b"abc")).await?;
        let written = regular_file_census(&root)?;
        require(
            !physical.exists()
                && disk.0.checked_add(3) == Some(written.0)
                && disk.1.checked_add(1) == Some(written.1),
            "multipart part was not held in exactly one private file",
        )?;
        upload.abort().await?;
        drop(upload);
        drop(store);
        drop(parent);
        binder.sweep_retired()?;
        require(
            authority.snapshot()? == boot
                && regular_file_census(&root)? == disk
                && binder.is_quiescent()?,
            "product multipart abort left a file, charge or pin",
        )?;
        drop(binder);
        drop(authority);
        drop(lease);
        let reacquired = StateRootLease::acquire(&root)?;
        reacquired.require_state_root_v1(&root)?;
        Ok(())
    })
}

#[test]
fn real_local_dropped_lance_writer_eventually_refunds() -> TestResult {
    run_test(async {
        use lance_io::object_store::ObjectStoreProvider as _;
        use lance_io::object_store::providers::local::FileStoreProvider;

        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let disk = regular_file_census(&root)?;
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let (work, staging) =
            candidate_staged_store(&root, deletion_owner(&binder), Arc::clone(&parent), None)?;
        let mut store = FileStoreProvider
            .new_store(
                uri_to_url(&memory_uri(&staging)?)?,
                &ObjectStoreParams::default(),
            )
            .await?;
        store.inner = Arc::new(work);
        let physical = staging.join("table.lance/data/dropped-writer.lance");
        let path = object_store::path::Path::from_absolute_path(&physical)?;
        let mut writer = store.create(&path).await?;
        writer.write_all(b"abc").await?;
        drop(writer);
        drop(store);
        drop(parent);
        wait_for_boot_refund(&authority, boot).await?;
        binder.sweep_retired()?;
        require(
            !physical.exists() && regular_file_census(&root)? == disk && binder.is_quiescent()?,
            "dropping the Lance writer left a final file, temporary, or pin",
        )?;
        Ok(())
    })
}

fn fixture_with_limits(max_bytes: u64, max_files: u64) -> Result<Fixture, Box<dyn Error>> {
    fixture_with_root_name("state", max_bytes, max_files)
}

fn fixture_with_root_name(
    root_name: &str,
    max_bytes: u64,
    max_files: u64,
) -> Result<Fixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join(root_name);
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let lease = StateRootLease::acquire(&root)?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let authority = seed_publication_capacity(
        &lease,
        &root,
        PublicationBootInventoryLimits {
            max_bytes,
            max_files,
            max_pending: max_files,
            max_entries: 4096,
            max_depth: 16,
            max_pending_frames: 32,
        },
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let binder = PublicationFileCapacity::new(lease.clone(), authority.clone(), 512, 4096, 256)?;
    Ok((directory, root, lease, authority, binder))
}

#[test]
fn sidecar_reservation_immediately_subdivides_and_drop_restores_parent() -> TestResult {
    use quanta_index_core::publication_admission::{
        PublicationOutputClass, PublicationOutputReservation,
    };

    let (_directory, _root, _lease, authority, binder) = fixture()?;
    let baseline = authority.snapshot()?;
    let parent = Arc::new(authority.reserve_publication(6, 1)?);
    let charged = authority.snapshot()?;
    let request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 3, 1)?;
    let permit = crate::publication_sidecar::sidecar_output_permit(
        deletion_owner(&binder),
        Arc::clone(&parent),
        request,
    )?;
    require(
        parent.reserve_output(1).is_err(),
        "unbound sidecar did not consume its parent file slot",
    )?;
    require(
        authority.snapshot()?.used_bytes == charged.used_bytes
            && authority.snapshot()?.used_files == charged.used_files,
        "sidecar child charged the global ledger twice",
    )?;
    // The root has room; only the held parent's remaining file slots refuse.
    let independent = authority.reserve_publication(1, 1)?;
    drop(independent);
    drop(permit);
    let returned = parent.reserve_output(3)?;
    drop(returned);
    drop(parent);
    require(
        authority.snapshot()? == baseline && binder.is_quiescent()?,
        "unbound sidecar drop failed to return child capacity",
    )?;
    Ok(())
}

#[test]
fn settled_sidecar_can_move_without_allowing_another_output() -> TestResult {
    use quanta_index_core::publication_admission::{
        PublicationOutputClass, PublicationOutputReservation,
    };
    use std::fs::{File, OpenOptions};
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let (_directory, root, _lease, authority, binder) = fixture()?;
    let baseline = authority.snapshot()?;
    // Leave a second file slot available so refusal proves the one-output
    // contract rather than exhaustion of the enclosing work allowance.
    let parent = Arc::new(authority.reserve_publication(32, 2)?);
    let request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 3, 1)?;
    let mut permit = crate::publication_sidecar::sidecar_output_permit(
        deletion_owner(&binder),
        Arc::clone(&parent),
        request,
    )?;
    let staging = root.join("sidecar.tmp");
    let target = root.join("sidecar.next");
    let promoted = root.join("sidecar.final");
    permit.bind_output(&staging, &target)?;
    permit.creation_started()?;
    let mut writer = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)?;
    permit.created(&writer, &staging)?;
    writer.write_all(b"abc")?;
    writer.sync_all()?;
    permit.before_replace()?;
    fs::rename(&staging, &target)?;
    File::open(&root)?.sync_all()?;
    permit.committed(writer, 3)?;
    permit.rename_started(&promoted)?;
    fs::rename(&target, &promoted)?;
    File::open(&root)?.sync_all()?;
    permit.renamed_and_synced()?;
    require(
        fs::read(&promoted)? == b"abc",
        "sidecar promotion changed bytes",
    )?;

    let before_rebind = authority.snapshot()?;
    require(
        before_rebind.retained_inodes
            == baseline
                .retained_inodes
                .checked_add(1)
                .ok_or("test oracle addition overflow")?,
        "settled sidecar did not retain its one physical inode",
    )?;
    require(
        permit
            .bind_output(&root.join("second.tmp"), &root.join("second.final"))
            .is_err(),
        "one-output sidecar permit accepted a second binding",
    )?;
    require(
        authority.snapshot()? == before_rebind && !root.join("second.tmp").exists(),
        "refused sidecar rebind changed charge or namespace",
    )?;
    // Leave the committed inode at a second name while an unrelated inode
    // occupies the same target. The permit must authenticate the inode before
    // unlinking; a matching pathname alone is insufficient.
    let saved = root.join("sidecar.saved");
    fs::rename(&promoted, &saved)?;
    fs::write(&promoted, b"xyz")?;
    File::open(&promoted)?.sync_all()?;
    File::open(&root)?.sync_all()?;
    require(
        matches!(
            permit.delete_file_and_sync(),
            Err(CoreError::InvalidContract(_))
        ) && fs::read(&promoted)? == b"xyz"
            && fs::read(&saved)? == b"abc"
            && authority.snapshot()? == before_rebind,
        "sidecar deletion accepted or changed a foreign same-path replacement",
    )?;
    fs::remove_file(&promoted)?;
    fs::rename(&saved, &promoted)?;
    File::open(&root)?.sync_all()?;
    permit.delete_file_and_sync()?;
    let after_delete = authority.snapshot()?;
    require(
        matches!(
            permit.delete_file_and_sync(),
            Err(CoreError::InvalidContract(_))
        ) && !promoted.exists()
            && authority.snapshot()? == after_delete,
        "settled sidecar accepted a second deletion",
    )?;
    require(
        !promoted.exists() && authority.snapshot()? == before_rebind,
        "live sidecar reader pin lost its unlinked inode charge",
    )?;
    drop(permit);
    drop(parent);
    require(
        authority.snapshot()? == baseline && binder.is_quiescent()?,
        "last sidecar reader retirement did not restore baseline",
    )?;
    Ok(())
}

#[test]
fn local_get_preserves_not_found_and_refuses_unsafe_optional_files() -> TestResult {
    run_test(async {
        use std::os::unix::fs::symlink;

        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let disk = regular_file_census(&root)?;
        let held = Arc::new(authority.reserve_publication(1, 1)?);
        let (store, staging) =
            candidate_staged_store(&root, deletion_owner(&binder), Arc::clone(&held), None)?;
        let missing_path = staging.join("table.lance/missing-receipt");
        let missing_parent_path = staging.join("table.lance/absent/receipt.cbor");
        let unsafe_path = staging.join("table.lance/unsafe-receipt");
        binder.ensure_private_parent_directories(&unsafe_path)?;
        let before = authority.snapshot()?;
        for physical in [&missing_path, &missing_parent_path] {
            let path = object_store::path::Path::from_absolute_path(physical)?;
            require(
                matches!(
                    store.get(&path).await,
                    Err(object_store::Error::NotFound { .. })
                ) && matches!(
                    store.head(&path).await,
                    Err(object_store::Error::NotFound { .. })
                ) && authority.snapshot()? == before,
                "product GET or HEAD lost typed absence or changed capacity",
            )?;
        }
        symlink(&missing_path, &unsafe_path)?;
        let unsafe_object = object_store::path::Path::from_absolute_path(&unsafe_path)?;
        require(
            fs::read_link(&unsafe_path)? == missing_path
                && matches!(
                    store.get(&unsafe_object).await,
                    Err(object_store::Error::Generic { .. })
                )
                && matches!(
                    store.head(&unsafe_object).await,
                    Err(object_store::Error::Generic { .. })
                )
                && authority.snapshot()? == before
                && binder.is_quiescent()?,
            "product GET or HEAD treated an unsafe symlink as optional absence",
        )?;
        fs::remove_file(&unsafe_path)?;
        drop(store);
        drop(held);
        binder.sweep_retired()?;
        require(
            authority.snapshot()? == boot
                && regular_file_census(&root)? == disk
                && binder.is_quiescent()?,
            "refused product GET or HEAD retained capacity, a reader pin, or a physical file",
        )?;
        Ok(())
    })
}

// The qualified Semantic build below uses the canonical admission port.
pub(super) fn regular_file_census(root: &Path) -> io::Result<(u64, u64)> {
    use std::collections::BTreeMap;
    use std::os::unix::fs::MetadataExt as _;

    fn visit(path: &Path, files: &mut BTreeMap<(u64, u64), u64>) -> io::Result<()> {
        for entry in fs::read_dir(path)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                visit(&path, files)?;
            } else if metadata.is_file() {
                let old = files.insert((metadata.dev(), metadata.ino()), metadata.len());
                if old.is_some_and(|size| size != metadata.len()) {
                    return Err(io::Error::other(
                        "hardlink census found inconsistent widths",
                    ));
                }
            } else {
                return Err(io::Error::other("unexpected non-regular inventory entry"));
            }
        }
        Ok(())
    }

    let mut files = BTreeMap::new();
    visit(root, &mut files)?;
    let bytes = files.values().try_fold(0_u64, |sum, size| {
        sum.checked_add(*size)
            .ok_or_else(|| io::Error::other("independent census byte overflow"))
    })?;
    let count = u64::try_from(files.len())
        .map_err(|error| io::Error::other(format!("independent census count overflow: {error}")))?;
    Ok((bytes, count))
}

fn first_nonempty_regular_file(root: &Path) -> io::Result<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_file() && metadata.len() != 0 {
                return Ok(path);
            }
            if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "sealed dataset has no nonempty regular file for candidate reads",
    ))
}

#[test]
fn managed_semantic_build_reopen_and_reclaim_matches_disk_census() -> TestResult {
    managed_semantic_build_reopen_and_reclaim("state")
}

#[cfg(feature = "qualified-memory-uri-candidate")]
#[test]
fn managed_semantic_candidate_preserves_escaped_state_root() -> TestResult {
    managed_semantic_build_reopen_and_reclaim("state with #한글%2F?")
}

fn managed_semantic_build_reopen_and_reclaim(root_name: &str) -> TestResult {
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    };
    use quanta_index_core::publication_admission::{
        PublicationAdmissionRequest, PublicationBuildWork,
    };
    use quanta_index_core::{
        IngestResourcePolicy, PublicationValidationOwner, ResidentScopeSource,
        SealedGenerationReclaimPort as _, SemanticContentRootsPort as _,
        SemanticIndexOpenPort as _, SemanticIngestHeaderV1, SemanticScopeStreamBuildPort as _,
        SemanticStreamWindowPolicy,
    };
    use quanta_index_searchd_harness::{
        SourceCorpusFixture, semantic_source_scopes_for_chunk_records,
    };
    use quanta_index_semantic::SemanticFileAdmissionFactory as _;
    use quanta_index_semantic::{
        SemanticAdapter, legacy_chunk_embedding_record_v1, sealed_replace_batch_v1,
        semantic_state_root,
    };

    let (_directory, root, _lease, authority, binder) =
        fixture_with_root_name(root_name, 16 * 1024 * 1024, 256)?;
    let boot = authority.snapshot()?;
    require(
        boot.used_bytes == 0 && boot.used_files == 1 && boot.retained_inodes == 1,
        "boot census must include only the zero-byte state-root lock",
    )?;
    require(
        regular_file_census(&root)? == (boot.used_bytes, boot.used_files),
        "boot ledger differs from independent disk census",
    )?;

    let repo = RepoId::new("repo-managed-sem").map_err(io::Error::other)?;
    let revision = RevisionId::new("rev-managed-sem").map_err(io::Error::other)?;
    let generation = ManifestGeneration::new(1);
    let path = "src/managed.rs";
    let record_id = "managed-record";
    let chunk = ChunkRecord {
        chunk_id: ChunkId::new(record_id),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust").map_err(io::Error::other)?,
        start_byte: 0,
        end_byte: 3,
        start_line: 1,
        end_line: 2,
        text: "abc".to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    };
    let source_scopes =
        semantic_source_scopes_for_chunk_records(&repo, std::slice::from_ref(&chunk));
    let mut producer = SourceCorpusFixture::new(
        repo.clone(),
        revision.clone(),
        generation,
        "managed-semantic-event".into(),
    );
    producer.stage_fixture(vec![chunk], Vec::new(), source_scopes)?;
    let source_batch = producer.frozen_batch()?;
    let footprint = IngestResourcePolicy::DEFAULT.admit_search_corpus_batch(&source_batch, 3)?;

    // Explicit fixture ceilings, not an encoded-output estimate. The test
    // checks actual post-build physical usage separately.
    let port = crate::canonical_publication_admission::CanonicalPublicationAdmission::new(
        Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
        crate::canonical_publication_admission::PublicationAllowanceLimit::new(1, 1)?,
        crate::canonical_publication_admission::PublicationAllowanceLimit::new(
            16 * 1024 * 1024,
            128,
        )?,
    );
    let mut owner = PublicationValidationOwner::default();
    owner.admit_publication_resources(
        &PublicationAdmissionRequest {
            batch: &source_batch,
            footprint,
            work: PublicationBuildWork::Materialize {
                lexical: false,
                semantic: true,
            },
        },
        &RequestBudgetV1::unbounded(),
        &port,
    )?;

    let deletions = port.file_deletions();
    let factory = Arc::new(
        crate::semantic_file_factory::CanonicalSemanticFileAdmissionFactory::new(Arc::clone(
            &deletions,
        )),
    );
    let foreign_control = root.join("indexes/semantic/foreign/MARKER_SEALED");
    require(
        matches!(
            factory
                .control_reader_lifetime()
                .begin_control_reader_open(&foreign_control),
            Err(quanta_index_core::CoreError::InvalidContract(_))
        ),
        "semantic Control lifetime accepted a foreign generation path",
    )?;
    let foreign_dataset = root.join("indexes/semantic/foreign/dataset/data/1.lance");
    require(
        matches!(
            factory
                .control_reader_lifetime()
                .begin_reader_open(&foreign_dataset),
            Err(quanta_index_core::CoreError::InvalidContract(_))
        ),
        "semantic dataset lifetime accepted a foreign generation path",
    )?;
    let maintenance = Arc::new(
        crate::semantic_file_factory::CanonicalSemanticMaintenanceFileFactory::new(Arc::clone(
            &deletions,
        )),
    );
    let candidate_lifetime = factory.control_reader_lifetime();
    let adapter = SemanticAdapter::with_state_root(semantic_state_root(&root))?
        .with_qualified_memory_uri_candidate(factory, maintenance);
    let embedding = legacy_chunk_embedding_record_v1(record_id, path, vec![1.0, 0.0, 0.0])
        .map_err(io::Error::other)?;
    // This fixture's embedding ID and owner-document record ID are distinct.
    // Keep the expected record literal independent of the query result.
    require(
        embedding.embedding_id.as_str() == "managed-record"
            && embedding.record_id.as_ref() == "record-managed-record",
        "semantic fixture changed its fixed embedding/record identity",
    )?;
    let mut semantic_batch = sealed_replace_batch_v1(
        repo.clone(),
        revision.clone(),
        generation,
        path,
        vec![embedding],
        3,
    );
    semantic_batch.manifest_digest = source_batch.manifest_digest.clone();
    semantic_batch.batch_digest = source_batch.batch_digest.clone();
    let header = SemanticIngestHeaderV1::of_batch(&semantic_batch);
    let mut scopes = ResidentScopeSource::new(
        &semantic_batch.replace_scopes,
        SemanticStreamWindowPolicy::DEFAULT,
    )?;
    let (_tally, _stages) =
        adapter.build_stream(&header, &mut scopes, Some(owner.output_admission()), None)?;
    drop(owner);
    drop(port);
    binder.sweep_retired()?;

    let held = authority.snapshot()?;
    let sealed = adapter.sealed_generations_for_pair(&repo, &revision, None)?;
    let candidate = sealed
        .first()
        .ok_or_else(|| io::Error::other("managed semantic generation was not sealed"))?;
    let roots = adapter.sealed_content_roots(candidate)?;
    require(
        roots.is_canonical_v1(),
        "product-owned sealed Control reader returned invalid content roots",
    )?;
    let measured = adapter.measure_sealed_generations(
        &repo,
        &revision,
        &std::collections::BTreeSet::from([generation]),
        None,
    )?;
    let generation_dir = quanta_index_semantic::legacy::layout::generation_dir(
        &semantic_state_root(&root),
        &repo,
        &revision,
        generation,
    );
    require(
        measured.absent.is_empty() && measured.bytes == regular_file_census(&generation_dir)?.0,
        "qualified retention measurement differs from the generation inode census",
    )?;
    require(
        authority.snapshot()? == held,
        "sealed Control reader retained a physical pin after checked retirement",
    )?;

    let searcher = adapter.open(&repo, &revision, generation)?;
    let hits = searcher.search_hits(&[1.0, 0.0, 0.0], 1, &RequestBudgetV1::unbounded())?;
    require(
        hits.len() == 1
            && hits.first().is_some_and(|hit| {
                hit.record_id == "record-managed-record" && hit.owner_id == "owner-managed-record"
            }),
        "cold reopened semantic generation returned wrong fixed record",
    )?;
    #[cfg(feature = "qualified-memory-uri-candidate")]
    {
        let second_maintenance = Arc::new(
            crate::semantic_file_factory::CanonicalSemanticMaintenanceFileFactory::new(Arc::clone(
                &deletions,
            )),
        );
        let second_adapter = SemanticAdapter::with_state_root(semantic_state_root(&root))?
            .with_maintenance_file_admission(second_maintenance);
        let held = authority.snapshot()?;
        let disk = regular_file_census(&root)?;
        let generations = second_adapter.sealed_generations_for_pair(&repo, &revision, None)?;
        let sealed = generations
            .first()
            .ok_or_else(|| io::Error::other("candidate sealed generation is absent"))?;
        let early_file = second_adapter.reclaim_sealed_generation(sealed, None);
        let early_candidate = adapter.reclaim_sealed_generation(sealed, None);
        require(
            matches!(early_file, Err(quanta_index_core::CoreError::Storage(ref message)) if message.contains("live reader view(s)"))
                && matches!(early_candidate, Err(quanta_index_core::CoreError::Storage(ref message)) if message.contains("live reader view(s)"))
                && authority.snapshot()? == held
                && regular_file_census(&root)? == disk,
            "File or candidate adapter reclaimed a live candidate reader or changed quota on refusal",
        )?;
        // A failed proven open acquires a candidate view before checking the
        // digest; its error path must release that view for the retry below.
        let mut wrong = sealed.clone();
        wrong.manifest_digest = "wrong-candidate-digest".into();
        require(
            adapter.open_proven(&wrong).is_err(),
            "wrong candidate digest unexpectedly opened",
        )?;
    }
    drop(searcher);
    binder.sweep_retired()?;
    let built = authority.snapshot()?;
    let (disk_bytes, disk_files) = regular_file_census(&root)?;
    require(
        built.used_bytes == disk_bytes
            && built.used_files == disk_files
            && built.retained_inodes == disk_files
            && built.active_allowances == boot.active_allowances
            && built.active_tickets == boot.active_tickets
            && built.uncertain_pending == boot.uncertain_pending,
        "managed semantic build ledger differs from physical census after reader retirement",
    )?;

    let generation_dir = quanta_index_semantic::legacy::layout::generation_dir(
        &semantic_state_root(&root),
        &repo,
        &revision,
        generation,
    );
    let dataset_dir = quanta_index_semantic::legacy::layout::dataset_dir(&generation_dir);
    run_test(async {
        {
            use quanta_index_core::SealedArtifactCommitmentV1;
            use quanta_index_semantic::AdmittedMemorySealedStore;
            use sha2::Digest as _;

            let physical = first_nonempty_regular_file(&dataset_dir)?;
            let relative = physical
                .strip_prefix(&generation_dir)?
                .to_str()
                .ok_or("sealed fixture artifact name is not UTF-8")?;
            let committed = SealedArtifactCommitmentV1 {
                name: relative.to_owned(),
                bytes: fs::metadata(&physical)?.len(),
                sha256: sha2::Sha256::digest(fs::read(&physical)?).into(),
            };
            let store =
                AdmittedMemorySealedStore::new(&dataset_dir, vec![committed], candidate_lifetime)?;
            let path = object_store::path::Path::from_absolute_path(&physical)?;
            let meta = store.head(&path).await?;
            require(
                meta.size > 0 && meta.size == fs::metadata(&physical)?.len(),
                "candidate HEAD returned a different or empty physical file width",
            )?;
            let conditional = object_store::GetOptions {
                head: true,
                if_match: meta.e_tag.clone(),
                range: Some(object_store::GetRange::Suffix(1)),
                version: Some("ignored-local-version".into()),
                ..Default::default()
            };
            let result = store.get_opts(&path, conditional).await?;
            require(
                result.meta == meta && result.range == (meta.size.saturating_sub(1)..meta.size),
                "candidate HEAD changed conditional/range/version metadata behavior",
            )?;
            let invalid_range = store
                .get_opts(
                    &path,
                    object_store::GetOptions {
                        head: true,
                        range: Some(object_store::GetRange::Offset(meta.size)),
                        ..Default::default()
                    },
                )
                .await;
            require(
                matches!(
                    invalid_range,
                    Err(object_store::Error::Generic {
                        store: "LocalFileSystem",
                        ..
                    })
                ),
                "candidate HEAD changed the local store's invalid range classification",
            )?;
            let not_modified = store
                .get_opts(
                    &path,
                    object_store::GetOptions {
                        head: true,
                        if_none_match: meta.e_tag.clone(),
                        ..Default::default()
                    },
                )
                .await;
            require(
                matches!(not_modified, Err(object_store::Error::NotModified { .. })),
                "candidate HEAD lost the delegated ETag precondition",
            )?;
            let ranges = [
                meta.size - 1..meta.size.saturating_add(3),
                0..meta.size.min(3),
                0..1,
            ];
            let ordered = store.get_ranges(&path, &ranges).await?;
            require(
                ordered.len() == ranges.len(),
                "candidate returned a different number of ranges",
            )?;
            let expected = fs::File::open(&physical)?;
            for (range, bytes) in ranges.iter().zip(ordered) {
                let width = range.end.min(meta.size) - range.start;
                let mut on_disk = vec![0; usize::try_from(width)?];
                std::os::unix::fs::FileExt::read_exact_at(&expected, &mut on_disk, range.start)?;
                require(
                    bytes.as_ref() == on_disk.as_slice(),
                    "candidate returned incorrect bytes or changed the range order",
                )?;
            }
            drop(expected);
            require(
                binder.is_quiescent()?,
                "completed candidate ranges retained a reader FD",
            )?;
            let pending = store.get(&path).await?.into_stream();
            require(
                !binder.is_quiescent()?,
                "candidate body did not hold the reader FD across stream consumption",
            )?;
            drop(pending);
            require(
                binder.is_quiescent()?,
                "dropping the candidate body did not release its reader FD",
            )?;
            let control_file = generation_dir.join("MARKER_SEALED");
            require(
                control_file.is_file(),
                "sealed Control fixture is missing before candidate HEAD",
            )?;
            let control = object_store::path::Path::from_absolute_path(control_file)?;
            require(
                store.head(&control).await.is_err() && store.get(&control).await.is_err(),
                "candidate HEAD or GET bypassed product dataset path admission",
            )?;
        }
        Ok(())
    })?;
    binder.sweep_retired()?;
    require(
        authority.snapshot()? == built && regular_file_census(&root)? == (disk_bytes, disk_files),
        "candidate sealed-generation read changed physical file charges",
    )?;

    let generations = adapter.sealed_generations_for_pair(&repo, &revision, None)?;
    require(
        generations.len() == 1,
        "expected one sealed semantic generation",
    )?;
    let generation = generations
        .first()
        .ok_or_else(|| io::Error::other("sealed semantic generation is absent"))?;
    let outcome = adapter.reclaim_sealed_generation(generation, None)?;
    require(
        matches!(
            outcome,
            quanta_index_core::SealedGenerationReclaimOutcomeV1::Reclaimed { .. }
        ),
        "qualified reclaim did not report reclaimed generation",
    )?;
    binder.sweep_retired()?;
    require(
        authority.snapshot()? == boot
            && regular_file_census(&root)? == (boot.used_bytes, boot.used_files),
        "qualified reclaim did not restore exact boot ledger and disk census",
    )?;
    Ok(())
}

// The one-row managed fixture above exercises the exact lane. This fixture
// crosses the production IVF floor and observes the real qualified candidate
// session, including its index-reader lifetime after a warm query.
#[cfg(target_os = "linux")]
#[test]
fn managed_semantic_ivf_reader_fds_retire_before_reclaim() -> TestResult {
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    };
    use quanta_index_core::publication_admission::{
        PublicationAdmissionRequest, PublicationBuildWork,
    };
    use quanta_index_core::{
        IngestResourcePolicy, MetricSourcePort as _, MetricValueV1, PublicationValidationOwner,
        ResidentScopeSource, SealedGenerationReclaimPort as _, SemanticIndexOpenPort as _,
        SemanticIngestHeaderV1, SemanticScopeStreamBuildPort as _, SemanticStreamWindowPolicy,
    };
    use quanta_index_searchd_harness::{
        SourceCorpusFixture, semantic_source_scopes_for_chunk_records,
    };
    use quanta_index_semantic::{
        SemanticAdapter, legacy_chunk_embedding_record_v1, sealed_replace_batch_v1,
        semantic_state_root,
    };

    fn generation_fd_count(generation_dir: &Path) -> io::Result<usize> {
        let mut count = 0;
        for entry in fs::read_dir("/proc/self/fd")? {
            match fs::read_link(entry?.path()) {
                Ok(path) if path.starts_with(generation_dir) => count += 1,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(count)
    }

    let (_directory, root, _lease, authority, binder) = fixture_with_limits(64 * 1024 * 1024, 512)?;
    let boot = authority.snapshot()?;
    let repo = RepoId::new("repo-managed-ivf").map_err(io::Error::other)?;
    let revision = RevisionId::new("rev-managed-ivf").map_err(io::Error::other)?;
    let generation = ManifestGeneration::new(1);
    let path = "src/ivf.rs";
    let mut chunks = Vec::with_capacity(256);
    let mut embeddings = Vec::with_capacity(256);
    for index in 0..256_u32 {
        let id = format!("ivf-row-{index:03}");
        chunks.push(ChunkRecord {
            chunk_id: ChunkId::new(id.clone()),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust").map_err(io::Error::other)?,
            start_byte: index * 3,
            end_byte: index * 3 + 3,
            start_line: index + 1,
            end_line: index + 2,
            text: "abc".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        });
        embeddings.push(
            legacy_chunk_embedding_record_v1(&id, path, vec![1.0, index as f32 / 256.0, 0.0])
                .map_err(io::Error::other)?,
        );
    }
    let source_scopes = semantic_source_scopes_for_chunk_records(&repo, &chunks);
    let mut producer = SourceCorpusFixture::new(
        repo.clone(),
        revision.clone(),
        generation,
        "managed-ivf-event".into(),
    );
    producer.stage_fixture(chunks, Vec::new(), source_scopes)?;
    let source_batch = producer.frozen_batch()?;
    let footprint = IngestResourcePolicy::DEFAULT.admit_search_corpus_batch(&source_batch, 3)?;
    let port = crate::canonical_publication_admission::CanonicalPublicationAdmission::new(
        Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
        crate::canonical_publication_admission::PublicationAllowanceLimit::new(1, 1)?,
        crate::canonical_publication_admission::PublicationAllowanceLimit::new(
            64 * 1024 * 1024,
            256,
        )?,
    );
    let mut owner = PublicationValidationOwner::default();
    owner.admit_publication_resources(
        &PublicationAdmissionRequest {
            batch: &source_batch,
            footprint,
            work: PublicationBuildWork::Materialize {
                lexical: false,
                semantic: true,
            },
        },
        &RequestBudgetV1::unbounded(),
        &port,
    )?;
    let deletions = port.file_deletions();
    let factory = Arc::new(
        crate::semantic_file_factory::CanonicalSemanticFileAdmissionFactory::new(Arc::clone(
            &deletions,
        )),
    );
    let maintenance = Arc::new(
        crate::semantic_file_factory::CanonicalSemanticMaintenanceFileFactory::new(Arc::clone(
            &deletions,
        )),
    );
    let adapter = SemanticAdapter::with_state_root(semantic_state_root(&root))?
        .with_qualified_memory_uri_candidate(factory, maintenance);
    let mut semantic_batch = sealed_replace_batch_v1(
        repo.clone(),
        revision.clone(),
        generation,
        path,
        embeddings,
        3,
    );
    semantic_batch.manifest_digest = source_batch.manifest_digest.clone();
    semantic_batch.batch_digest = source_batch.batch_digest.clone();
    let header = SemanticIngestHeaderV1::of_batch(&semantic_batch);
    let mut scopes = ResidentScopeSource::new(
        &semantic_batch.replace_scopes,
        SemanticStreamWindowPolicy::DEFAULT,
    )?;
    let (tally, _stages) =
        adapter.build_stream(&header, &mut scopes, Some(owner.output_admission()), None)?;
    require(
        tally.rows == 256,
        "managed IVF build did not consume 256 rows",
    )?;
    drop(owner);
    drop(port);
    binder.sweep_retired()?;

    let generation_dir = quanta_index_semantic::legacy::layout::generation_dir(
        &semantic_state_root(&root),
        &repo,
        &revision,
        generation,
    );
    let manifest_bytes = fs::read(quanta_index_semantic::legacy::layout::manifest_path(
        &generation_dir,
    ))?;
    let manifest: serde_json::Value = ciborium::from_reader(manifest_bytes.as_slice())?;
    require(
        manifest["row_count"] == 256
            && manifest["vector_index"]["mode"] == "ivf_hnsw_sq"
            && manifest["vector_index"]["ann"]["indexed_rows"] == 256,
        "managed fixture did not seal a trained 256-row IVF index",
    )?;
    let built = authority.snapshot()?;
    let built_disk = regular_file_census(&root)?;
    require(
        built.used_bytes == built_disk.0 && built.used_files == built_disk.1,
        "IVF build ledger differs from physical disk census",
    )?;
    require(
        generation_fd_count(&generation_dir)? == 0,
        "IVF build left an open generation descriptor",
    )?;
    for _cycle in 0..2 {
        let searcher = adapter.open(&repo, &revision, generation)?;
        for _query in 0..2 {
            let hits = searcher.search_hits(&[1.0, 0.0, 0.0], 1, &RequestBudgetV1::unbounded())?;
            require(
                hits.len() == 1
                    && hits
                        .first()
                        .is_some_and(|hit| hit.record_id.starts_with("record-ivf-row-")),
                "cold or warm IVF query did not return a sealed fixture row",
            )?;
            require(
                authority.snapshot()? == built && regular_file_census(&root)? == built_disk,
                "IVF read changed physical file charges or disk census",
            )?;
        }
        drop(searcher);
        binder.sweep_retired()?;
        require(
            binder.is_quiescent()?
                && generation_fd_count(&generation_dir)? == 0
                && authority.snapshot()? == built,
            "IVF reader drop retained an FD, product pin, or physical charge",
        )?;
    }
    require(
        adapter.scrape()?.iter().any(|point| {
            point.name == "semantic_dense_queries_ann_total"
                && point.value == MetricValueV1::Counter(4)
        }),
        "the four queries did not execute the trained ANN lane",
    )?;

    let sealed = adapter.sealed_generations_for_pair(&repo, &revision, None)?;
    require(sealed.len() == 1, "expected one sealed IVF generation")?;
    let outcome = adapter.reclaim_sealed_generation(&sealed[0], None)?;
    require(
        matches!(
            outcome,
            quanta_index_core::SealedGenerationReclaimOutcomeV1::Reclaimed { .. }
        ),
        "qualified IVF reclaim did not report reclaimed generation",
    )?;
    binder.sweep_retired()?;
    require(
        authority.snapshot()? == boot
            && regular_file_census(&root)? == (boot.used_bytes, boot.used_files)
            && generation_fd_count(&generation_dir)? == 0,
        "qualified IVF reclaim did not restore boot census or retained FDs",
    )?;
    Ok(())
}
