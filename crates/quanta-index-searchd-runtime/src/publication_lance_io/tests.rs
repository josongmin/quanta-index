use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lance_io::object_store::file_admission::{AdmittedFileStoreProvider, FileAdmission};
use lance_io::object_store::{
    ChainedWrappingObjectStore, ObjectStore, ObjectStoreParams, ObjectStoreRegistry, uri_to_url,
};
use object_store::{ObjectStoreExt as _, PutMode, PutMultipartOptions, PutPayload};
use quanta_index_core::publication_capacity::{CapacitySnapshot, PublicationCapacity};
use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE, RequestBudgetV1};
use quanta_index_searchd::app::runtime::StateRootLease;
use tokio::io::AsyncWriteExt as _;

use super::PublicationLanceAdmission;
use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;
use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

type TestResult = Result<(), Box<dyn Error>>;
type Fixture = (
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
fn run_test(future: impl core::future::Future<Output = TestResult>) -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(future)
}

fn require(condition: bool, message: &'static str) -> io::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message))
    }
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
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

fn qualified_registry(admission: Arc<dyn FileAdmission>) -> Arc<ObjectStoreRegistry> {
    let registry = Arc::new(ObjectStoreRegistry::default());
    registry.insert("file", Arc::new(AdmittedFileStoreProvider::new(admission)));
    registry
}

fn deletion_owner(binder: &Arc<PublicationFileCapacity>) -> Arc<PublicationFileDeletions> {
    Arc::new(PublicationFileDeletions::new(Arc::clone(binder)))
}

fn root_uri(root: &Path) -> io::Result<&str> {
    root.to_str()
        .ok_or_else(|| io::Error::other("test state root is not UTF-8"))
}

fn contains_budget_cause(error: &(dyn Error + 'static)) -> bool {
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
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let charged = authority.snapshot()?;
        let admission: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            deletion_owner(&binder),
            Arc::clone(&parent),
        )?);
        let registry = qualified_registry(admission);
        let (store, base_path) = ObjectStore::from_uri_and_params(
            registry,
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        require(
            store.is_local(),
            "qualified store changed local-file semantics",
        )?;
        require(
            store.has_file_admission(),
            "qualified store lost its admission",
        )?;
        let target = root.join("data.lance");
        let path = base_path.join("data.lance");
        let mut writer = store.create(&path).await?;
        writer.write_all(b"abc").await?;
        writer.flush().await?;
        let before_refusal = authority.snapshot()?;
        let refusal = writer
            .write_all(b"d")
            .await
            .err()
            .ok_or_else(|| io::Error::other("writer exceeded held byte parent"))?;
        require(
            contains_budget_cause(&refusal),
            "writer refusal lost its typed budget source",
        )?;
        require(!target.exists(), "failed writer published a final file")?;
        require(
            authority.snapshot()? == before_refusal,
            "failed growth changed the existing physical charge",
        )?;
        require(
            before_refusal.used_bytes == charged.used_bytes
                && before_refusal.used_files == charged.used_files,
            "admitted child double charged its held parent",
        )?;
        let mut staged = 0_u64;
        for item in fs::read_dir(&root)? {
            let item = item?;
            if item
                .file_name()
                .to_string_lossy()
                .starts_with(".lance-admitted-")
            {
                staged = staged
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("staged file count overflow"))?;
                require(
                    item.metadata()?.len() == 3,
                    "refused append touched staged bytes",
                )?;
            }
        }
        require(staged == 1, "expected one actual admitted staging file")?;
        drop(writer);
        Ok(())
    })
}

#[test]
fn real_local_writer_reader_and_unlink_retain_exact_inode_charge() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let lock = root.join(".searchd-state-root.lock");
        require(
            lock.metadata()?.len() == 0
                && boot.used_bytes == 0
                && boot.used_files == 1
                && boot.retained_inodes == 1
                && boot.active_allowances == 0
                && boot.active_tickets == 0,
            "boot inventory did not count exactly the empty state-root lock",
        )?;
        let committed_bytes = boot
            .used_bytes
            .checked_add(3)
            .ok_or_else(|| io::Error::other("three-byte charge overflow"))?;
        let committed_files = boot
            .used_files
            .checked_add(1)
            .ok_or_else(|| io::Error::other("one-file charge overflow"))?;
        let committed_inodes = boot
            .retained_inodes
            .checked_add(1)
            .ok_or_else(|| io::Error::other("one-inode charge overflow"))?;

        // Reserve slack deliberately: after every unused work-parent owner is
        // dropped, only the lock and three-byte output remain charged.
        let parent = Arc::new(authority.reserve_publication(8, 1)?);
        let deletions = deletion_owner(&binder);
        let work: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            Arc::clone(&deletions),
            Arc::clone(&parent),
        )?);
        let work_registry = qualified_registry(work);
        let (work_store, base_path) = ObjectStore::from_uri_and_params(
            Arc::clone(&work_registry),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let path = base_path.join("retained.lance");
        let physical = root.join("retained.lance");
        let mut writer = work_store.create(&path).await?;
        writer.write_all(b"abc").await?;
        let result = lance_io::traits::Writer::shutdown(writer.as_mut()).await?;
        require(result.size == 3, "vendor finish reported the wrong width")?;
        drop(writer);
        require(
            physical.metadata()?.len() == 3,
            "vendor finish published the wrong physical width",
        )?;

        let reader = work_store.open(&path).await?;
        require(
            reader.get_range(0..3).await?.as_ref() == b"abc",
            "admitted reader returned the wrong payload",
        )?;
        drop(work_store);
        drop(work_registry);
        drop(parent);
        let committed = authority.snapshot()?;
        require(
            committed.used_bytes == committed_bytes
                && committed.used_files == committed_files
                && committed.retained_inodes == committed_inodes
                && committed.active_allowances == boot.active_allowances,
            "committed inode kept unused work-parent slack or lost its charge",
        )?;

        let retirement: Arc<dyn FileAdmission> = Arc::new(
            PublicationLanceAdmission::retirement_only(Arc::clone(&deletions)),
        );
        let retirement_registry = qualified_registry(retirement);
        let (retirement_store, _retirement_base) = ObjectStore::from_uri_and_params(
            Arc::clone(&retirement_registry),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        retirement_store.delete(&path).await?;
        require(!physical.exists(), "delete left the pathname visible")?;
        require(
            reader.get_range(0..3).await?.as_ref() == b"abc",
            "unlinked admitted reader lost its original inode",
        )?;
        let pinned = authority.snapshot()?;
        require(
            pinned.used_bytes == committed_bytes
                && pinned.used_files == committed_files
                && pinned.retained_inodes == committed_inodes,
            "unlink refunded a still-open reader inode",
        )?;

        drop(reader);
        binder.sweep_retired()?;
        drop(retirement_store);
        drop(retirement_registry);
        require(
            authority.snapshot()? == boot,
            "last reader retirement did not restore the boot capacity baseline",
        )?;
        Ok(())
    })
}

#[test]
#[expect(
    deprecated,
    reason = "test exercises the pinned legacy custom inner-store bypass field"
)]
fn qualified_session_rejects_explicit_inner_store_bypass() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let parent = Arc::new(authority.reserve_publication(8, 1)?);
        let admission: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            deletion_owner(&binder),
            parent,
        )?);
        let registry = qualified_registry(admission);
        let (store, _path) = ObjectStore::from_uri_and_params(
            Arc::clone(&registry),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        require(
            store.is_local(),
            "qualified store changed local-file semantics",
        )?;
        require(
            store.has_file_admission(),
            "qualified store lost its admission",
        )?;
        let before = authority.snapshot()?;
        let params = ObjectStoreParams {
            object_store: Some((ObjectStore::memory().inner, uri_to_url("memory:///")?)),
            ..ObjectStoreParams::default()
        };
        let bypass =
            ObjectStore::from_uri_and_params(Arc::clone(&registry), root_uri(&root)?, &params)
                .await;
        require(
            bypass.is_err(),
            "qualified provider accepted explicit inner store",
        )?;
        let wrapped_params = ObjectStoreParams {
            object_store_wrapper: Some(Arc::new(ChainedWrappingObjectStore::new(Vec::new()))),
            ..ObjectStoreParams::default()
        };
        let wrapped =
            ObjectStore::from_uri_and_params(registry, root_uri(&root)?, &wrapped_params).await;
        require(
            wrapped.is_err(),
            "qualified provider accepted a store wrapper",
        )?;
        require(
            authority.snapshot()? == before,
            "rejected inner store changed physical charges",
        )?;
        Ok(())
    })
}

#[test]
fn readonly_and_foreign_parent_refuse_before_namespace_creation() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let read_only: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::read_only(
            Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
        ));
        let registry = qualified_registry(read_only);
        let (store, base_path) = ObjectStore::from_uri_and_params(
            registry,
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        require(
            store.is_local(),
            "qualified store changed local-file semantics",
        )?;
        require(
            store.has_file_admission(),
            "qualified store lost its admission",
        )?;
        let before = authority.snapshot()?;
        let target = root.join("not-created/deep/data.lance");
        let path = base_path
            .join("not-created")
            .join("deep")
            .join("data.lance");
        require(
            store.create(&path).await.is_err(),
            "read-only store created a writer",
        )?;
        require(
            !root.join("not-created").exists(),
            "read-only admission created a parent directory",
        )?;
        require(
            !target.exists(),
            "read-only admission created its target file",
        )?;
        require(
            authority.snapshot()? == before,
            "read-only path changed charges",
        )?;

        let (_other_directory, _other_root, _other_lease, foreign, _other_binder) = fixture()?;
        let foreign_parent = Arc::new(foreign.reserve_publication(1, 1)?);
        let foreign_before = foreign.snapshot()?;
        require(
            PublicationLanceAdmission::for_work(
                deletion_owner(&binder),
                Arc::clone(&foreign_parent),
            )
            .is_err(),
            "foreign work parent entered local-file admission",
        )?;
        require(
            authority.snapshot()? == before,
            "foreign parent charged local ledger",
        )?;
        require(
            foreign.snapshot()? == foreign_before,
            "foreign parent changed after refusal",
        )?;
        require(
            !root.join("not-created").exists(),
            "foreign parent created a namespace before validation",
        )?;
        Ok(())
    })
}

async fn real_local_put_opts_mode(overwrite: bool) -> TestResult {
    let (_directory, root, lease, authority, binder) = fixture()?;
    let boot = authority.snapshot()?;
    let lock = root.join(".searchd-state-root.lock");
    require(
        lock.metadata()?.len() == 0
            && boot.used_bytes == 0
            && boot.used_files == 1
            && boot.retained_inodes == 1
            && boot.active_allowances == 0
            && boot.active_tickets == 0,
        "put boot inventory did not count exactly the empty state-root lock",
    )?;
    let committed_bytes = boot
        .used_bytes
        .checked_add(3)
        .ok_or_else(|| io::Error::other("three-byte charge overflow"))?;
    let committed_files = boot
        .used_files
        .checked_add(1)
        .ok_or_else(|| io::Error::other("one-file charge overflow"))?;
    let committed_inodes = boot
        .retained_inodes
        .checked_add(1)
        .ok_or_else(|| io::Error::other("one-inode charge overflow"))?;

    // An overwrite temporarily owns both the old and new physical inodes.
    let parent = Arc::new(authority.reserve_publication(8, if overwrite { 2 } else { 1 })?);
    let deletions = deletion_owner(&binder);
    let work: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
        Arc::clone(&deletions),
        Arc::clone(&parent),
    )?);
    let work_registry = qualified_registry(work);
    let (work_store, base_path) = ObjectStore::from_uri_and_params(
        Arc::clone(&work_registry),
        root_uri(&root)?,
        &ObjectStoreParams::default(),
    )
    .await?;
    require(
        work_store.is_local() && work_store.has_file_admission(),
        "put session lost its qualified local-file provider",
    )?;
    let path = base_path.join("put-opts.lance");
    let physical = root.join("put-opts.lance");
    if overwrite {
        let _seed = work_store
            .inner
            .put_opts(
                &path,
                PutPayload::from_static(b"old"),
                PutMode::Create.into(),
            )
            .await?;
        require(
            fs::read(&physical)? == b"old",
            "overwrite seed was not the expected three-byte inode",
        )?;
    }
    let mode = if overwrite {
        PutMode::Overwrite
    } else {
        PutMode::Create
    };
    let _published = work_store
        .inner
        .put_opts(&path, PutPayload::from_static(b"abc"), mode.into())
        .await?;
    require(
        fs::read(&physical)? == b"abc" && physical.metadata()?.len() == 3,
        "put_opts did not publish its exact three-byte payload",
    )?;
    let reader = work_store.open(&path).await?;
    require(
        reader.get_range(0..3).await?.as_ref() == b"abc",
        "admitted put reader returned the wrong bytes",
    )?;
    drop(work_store);
    drop(work_registry);
    drop(parent);
    let committed = authority.snapshot()?;
    require(
        committed.used_bytes == committed_bytes
            && committed.used_files == committed_files
            && committed.retained_inodes == committed_inodes
            && committed.active_allowances == boot.active_allowances
            && committed.active_tickets == boot.active_tickets,
        "put commit retained old inode, unused parent slack, or an unsettled ticket",
    )?;

    let retirement: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::retirement_only(
        Arc::clone(&deletions),
    ));
    let retirement_registry = qualified_registry(retirement);
    let (retirement_store, _retirement_base) = ObjectStore::from_uri_and_params(
        Arc::clone(&retirement_registry),
        root_uri(&root)?,
        &ObjectStoreParams::default(),
    )
    .await?;
    retirement_store.delete(&path).await?;
    require(!physical.exists(), "put delete left the pathname visible")?;
    require(
        reader.get_range(0..3).await?.as_ref() == b"abc",
        "unlinked put reader lost its original inode",
    )?;
    let pinned = authority.snapshot()?;
    require(
        pinned.used_bytes == committed_bytes
            && pinned.used_files == committed_files
            && pinned.retained_inodes == committed_inodes,
        "put unlink refunded a still-open reader inode",
    )?;

    drop(reader);
    binder.sweep_retired()?;
    drop(retirement_store);
    drop(retirement_registry);
    require(
        authority.snapshot()? == boot,
        "put reader retirement did not restore exact boot capacity",
    )?;
    drop(deletions);
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
        "put handoff failed to recover exact boot inventory",
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

#[test]
fn real_local_put_opts_preserves_typed_budget_refusal() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let work: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            deletion_owner(&binder),
            parent,
        )?);
        let registry = qualified_registry(work);
        let (store, base_path) = ObjectStore::from_uri_and_params(
            registry,
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let path = base_path.join("put-over-budget.lance");
        let physical = root.join("put-over-budget.lance");
        let error = store
            .inner
            .put_opts(
                &path,
                PutPayload::from_static(b"abcd"),
                PutMode::Create.into(),
            )
            .await
            .err()
            .ok_or_else(|| io::Error::other("put_opts exceeded the held byte parent"))?;
        require(
            contains_budget_cause(&error),
            "put_opts refusal lost its typed budget source",
        )?;
        require(!physical.exists(), "over-budget put published a final path")?;
        Ok(())
    })
}

async fn wait_for_boot_refund(
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

fn require_only_boot_lock(root: &Path) -> TestResult {
    let mut entries = fs::read_dir(root)?;
    let only = entries
        .next()
        .transpose()?
        .ok_or_else(|| io::Error::other("boot lock missing after cleanup"))?;
    require(
        only.file_name().to_string_lossy() == ".searchd-state-root.lock",
        "cleanup left a non-lock entry in the private root",
    )?;
    require(
        entries.next().transpose()?.is_none(),
        "cleanup left extra private-root entries",
    )?;
    Ok(())
}

fn require_one_staged_payload(root: &Path, expected: &[u8]) -> TestResult {
    let mut staged = 0_u64;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy() == ".searchd-state-root.lock" {
            continue;
        }
        require(
            entry.file_type()?.is_file() && fs::read(entry.path())? == expected,
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
fn real_local_put_budget_refusal_refunds_and_retries() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        require(
            boot.used_bytes == 0 && boot.used_files == 1 && boot.retained_inodes == 1,
            "retry boot baseline was not the empty lock inode",
        )?;
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let deletions = deletion_owner(&binder);
        let work: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            Arc::clone(&deletions),
            Arc::clone(&parent),
        )?);
        let registry = qualified_registry(work);
        let (store, base_path) = ObjectStore::from_uri_and_params(
            Arc::clone(&registry),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let path = base_path.join("retry.lance");
        let physical = root.join("retry.lance");
        let error = store
            .inner
            .put_opts(
                &path,
                PutPayload::from_static(b"abcd"),
                PutMode::Create.into(),
            )
            .await
            .err()
            .ok_or_else(|| io::Error::other("over-budget put unexpectedly succeeded"))?;
        require(
            contains_budget_cause(&error),
            "over-budget put lost the typed budget cause",
        )?;
        require(!physical.exists(), "over-budget put published a final file")?;
        drop(store);
        drop(registry);
        drop(parent);
        require(
            authority.snapshot()? == boot,
            "failed put retained a charge after authenticated writer retirement",
        )?;
        require_only_boot_lock(&root)?;

        let retry_parent = Arc::new(authority.reserve_publication(3, 1)?);
        let retry_work: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            Arc::clone(&deletions),
            Arc::clone(&retry_parent),
        )?);
        let retry_registry = qualified_registry(retry_work);
        let (retry_store, retry_base) = ObjectStore::from_uri_and_params(
            Arc::clone(&retry_registry),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let retry_path = retry_base.join("retry.lance");
        let _published = retry_store
            .inner
            .put_opts(
                &retry_path,
                PutPayload::from_static(b"abc"),
                PutMode::Create.into(),
            )
            .await?;
        require(fs::read(&physical)? == b"abc", "retry did not publish abc")?;
        drop(retry_store);
        drop(retry_registry);
        drop(retry_parent);
        let committed = authority.snapshot()?;
        require(
            committed.used_bytes == 3
                && committed.used_files == 2
                && committed.retained_inodes == 2,
            "successful retry had an incorrect physical charge",
        )?;
        let retirement: Arc<dyn FileAdmission> = Arc::new(
            PublicationLanceAdmission::retirement_only(Arc::clone(&deletions)),
        );
        let retirement_registry = qualified_registry(retirement);
        let (retirement_store, _base) = ObjectStore::from_uri_and_params(
            Arc::clone(&retirement_registry),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        retirement_store.delete(&retry_path).await?;
        drop(retirement_store);
        drop(retirement_registry);
        binder.sweep_retired()?;
        require(
            authority.snapshot()? == boot,
            "retry cleanup missed boot baseline",
        )?;
        require_only_boot_lock(&root)?;
        Ok(())
    })
}

#[test]
fn real_local_multipart_abort_refunds_and_releases_root() -> TestResult {
    run_test(async {
        let (_directory, root, lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let work: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            deletion_owner(&binder),
            Arc::clone(&parent),
        )?);
        let registry = qualified_registry(work);
        let (store, base_path) = ObjectStore::from_uri_and_params(
            Arc::clone(&registry),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let path = base_path.join("aborted.lance");
        let mut upload = store
            .inner
            .put_multipart_opts(&path, PutMultipartOptions::default())
            .await?;
        upload.put_part(PutPayload::from_static(b"abc")).await?;
        require_one_staged_payload(&root, b"abc")?;
        require(
            !root.join("aborted.lance").exists(),
            "multipart part published the final path",
        )?;
        upload.abort().await?;
        drop(upload);
        drop(store);
        drop(registry);
        drop(parent);
        require(
            authority.snapshot()? == boot,
            "multipart abort missed boot baseline",
        )?;
        require_only_boot_lock(&root)?;
        drop(binder);
        drop(authority);
        drop(lease);
        let reacquired = StateRootLease::acquire(&root)?;
        reacquired.require_state_root_v1(&root)?;
        Ok(())
    })
}

#[test]
fn real_local_multipart_drop_eventually_refunds() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let work: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            deletion_owner(&binder),
            Arc::clone(&parent),
        )?);
        let registry = qualified_registry(work);
        let (store, base_path) = ObjectStore::from_uri_and_params(
            Arc::clone(&registry),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let path = base_path.join("dropped-upload.lance");
        let mut upload = store
            .inner
            .put_multipart_opts(&path, PutMultipartOptions::default())
            .await?;
        upload.put_part(PutPayload::from_static(b"abc")).await?;
        require_one_staged_payload(&root, b"abc")?;
        drop(upload);
        drop(store);
        drop(registry);
        drop(parent);
        wait_for_boot_refund(&authority, boot).await?;
        require_only_boot_lock(&root)?;
        drop(binder);
        Ok(())
    })
}

#[test]
fn real_local_dropped_lance_writer_eventually_refunds() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let work: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            deletion_owner(&binder),
            Arc::clone(&parent),
        )?);
        let registry = qualified_registry(work);
        let (store, base_path) = ObjectStore::from_uri_and_params(
            Arc::clone(&registry),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let path = base_path.join("dropped-writer.lance");
        let mut writer = store.create(&path).await?;
        writer.write_all(b"abc").await?;
        drop(writer);
        drop(store);
        drop(registry);
        drop(parent);
        wait_for_boot_refund(&authority, boot).await?;
        require_only_boot_lock(&root)?;
        drop(binder);
        Ok(())
    })
}

fn fixture_with_limits(max_bytes: u64, max_files: u64) -> Result<Fixture, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("state");
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
fn admitted_reader_preserves_absence_and_refuses_symlinks_without_charge() -> TestResult {
    use std::os::unix::fs::symlink;

    let (_directory, root, _lease, authority, binder) = fixture()?;
    let boot = authority.snapshot()?;
    let admission = PublicationLanceAdmission::read_only(Arc::new(PublicationFileDeletions::new(
        Arc::clone(&binder),
    )));
    for relative in ["missing", "missing-parent/receipt.cbor"] {
        let error = admission
            .begin_reader_open(&root.join(relative))
            .err()
            .ok_or("absent reader unexpectedly opened")?;
        require(
            error.kind() == io::ErrorKind::NotFound
                && error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<CoreError>())
                    .is_some_and(|source| matches!(source, CoreError::NotFound(_))),
            "absent reader lost its typed NotFound cause",
        )?;
    }
    symlink(root.join("missing"), root.join("unsafe-receipt"))?;
    let error = admission
        .begin_reader_open(&root.join("unsafe-receipt"))
        .err()
        .ok_or("symlink reader unexpectedly opened")?;
    require(
        error.kind() != io::ErrorKind::NotFound,
        "unsafe symlink was accepted as an optional absent receipt",
    )?;
    let lookalike = super::io_error(CoreError::Storage(
        "open anchored file: No such file or directory".into(),
    ));
    require(
        lookalike.kind() != io::ErrorKind::NotFound,
        "Storage text was classified as typed absence",
    )?;
    require(
        authority.snapshot()? == boot && binder.is_quiescent()?,
        "failed reader open changed the capacity or retained a pin",
    )?;
    Ok(())
}

#[test]
fn local_get_preserves_not_found_and_refuses_unsafe_optional_files() -> TestResult {
    run_test(async {
        use std::os::unix::fs::symlink;

        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let admission: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::read_only(
            Arc::new(PublicationFileDeletions::new(Arc::clone(&binder))),
        ));
        let (store, base_path) = ObjectStore::from_uri_and_params(
            qualified_registry(admission),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let missing = store
            .inner
            .get(&base_path.clone().join("missing-receipt"))
            .await;
        require(
            matches!(missing, Err(object_store::Error::NotFound { .. })),
            "qualified get lost the public NotFound outcome",
        )?;
        symlink(root.join("missing-receipt"), root.join("unsafe-receipt"))?;
        let unsafe_file = store.inner.get(&base_path.join("unsafe-receipt")).await;
        require(
            matches!(unsafe_file, Err(object_store::Error::Generic { .. })),
            "qualified get accepted unsafe optional content as absent",
        )?;
        require(
            authority.snapshot()? == boot && binder.is_quiescent()?,
            "refused qualified get retained capacity or a reader pin",
        )?;
        Ok(())
    })
}

// The qualified Semantic build below uses the canonical admission port.
fn regular_file_census(root: &Path) -> io::Result<(u64, u64)> {
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

#[test]
fn managed_semantic_build_reopen_and_reclaim_matches_disk_census() -> TestResult {
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        ChunkId, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    };
    use quanta_index_core::publication_admission::{
        PublicationAdmissionRequest, PublicationBuildWork,
    };
    use quanta_index_core::{
        IngestResourcePolicy, PublicationValidationOwner, ResidentScopeSource,
        SealedGenerationReclaimPort as _, SemanticIndexOpenPort as _, SemanticIngestHeaderV1,
        SemanticScopeStreamBuildPort as _, SemanticStreamWindowPolicy,
    };
    use quanta_index_searchd_harness::{
        SourceCorpusFixture, semantic_source_scopes_for_chunk_records,
    };
    use quanta_index_semantic::{
        SemanticAdapter, legacy_chunk_embedding_record_v1, sealed_replace_batch_v1,
        semantic_state_root,
    };

    let (_directory, root, _lease, authority, binder) = fixture_with_limits(16 * 1024 * 1024, 256)?;
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
    let source_scopes = semantic_source_scopes_for_chunk_records(std::slice::from_ref(&chunk));
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
    let maintenance = Arc::new(
        crate::semantic_file_factory::CanonicalSemanticMaintenanceFileFactory::new(Arc::clone(
            &deletions,
        )),
    );
    let adapter = SemanticAdapter::with_state_root(semantic_state_root(&root))?
        .with_qualified_file_admission(factory.read_only(), factory)
        .with_maintenance_file_admission(maintenance);
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

    let searcher = adapter.open(&repo, &revision, generation)?;
    let hits = searcher.search_hits(&[1.0, 0.0, 0.0], 1, &RequestBudgetV1::unbounded())?;
    require(
        hits.len() == 1
            && hits.first().is_some_and(|hit| {
                hit.record_id == "record-managed-record" && hit.owner_id == "owner-managed-record"
            }),
        "cold reopened semantic generation returned wrong fixed record",
    )?;
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

// Real local-file regression for one failed pre-publication callback. The
// injected error is a fixed test oracle, not a filesystem fault.
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};

use lance_io::object_store::file_admission::{
    FileDirectoryDeleteTicket, FileDirectoryPathTicket, FileMoveMode, FilePathTicket,
    FileReaderOpenTicket, FileWriteTicket,
};

const BEFORE_REPLACE_REFUSAL: &str = "injected once before_replace refusal";

#[derive(Debug)]
struct RefuseBeforeReplaceOnce {
    delegate: Arc<dyn FileAdmission>,
    fired: Arc<AtomicBool>,
}

impl FileAdmission for RefuseBeforeReplaceOnce {
    fn validate_list_scope(&self, path: &Path) -> io::Result<()> {
        self.delegate.validate_list_scope(path)
    }
    fn reconcile_pending_namespace(&self) -> io::Result<()> {
        self.delegate.reconcile_pending_namespace()
    }
    fn prepare_parent_dirs(&self, final_path: &Path) -> io::Result<()> {
        self.delegate.prepare_parent_dirs(final_path)
    }

    fn begin_create(&self, final_path: &Path) -> io::Result<Box<dyn FileWriteTicket>> {
        Ok(Box::new(RefuseBeforeReplaceOnceTicket {
            delegate: self.delegate.begin_create(final_path)?,
            fired: Arc::clone(&self.fired),
        }))
    }

    fn begin_byte_copy(
        &self,
        source: &File,
        final_path: &Path,
    ) -> io::Result<Box<dyn FileWriteTicket>> {
        Ok(Box::new(RefuseBeforeReplaceOnceTicket {
            delegate: self.delegate.begin_byte_copy(source, final_path)?,
            fired: Arc::clone(&self.fired),
        }))
    }

    fn begin_link(&self, source: &Path, target: &Path) -> io::Result<Box<dyn FilePathTicket>> {
        self.delegate.begin_link(source, target)
    }

    fn begin_move(
        &self,
        source: &Path,
        target: &Path,
        mode: FileMoveMode,
    ) -> io::Result<Box<dyn FilePathTicket>> {
        self.delegate.begin_move(source, target, mode)
    }

    fn delete_file_and_sync(&self, path: &Path) -> io::Result<()> {
        self.delegate.delete_file_and_sync(path)
    }

    fn begin_directory_move(
        &self,
        source: &Path,
        target: &Path,
    ) -> io::Result<Box<dyn FileDirectoryPathTicket>> {
        self.delegate.begin_directory_move(source, target)
    }

    fn begin_directory_delete(
        &self,
        path: &Path,
    ) -> io::Result<Box<dyn FileDirectoryDeleteTicket>> {
        self.delegate.begin_directory_delete(path)
    }

    fn begin_reader_open(&self, path: &Path) -> io::Result<Box<dyn FileReaderOpenTicket>> {
        self.delegate.begin_reader_open(path)
    }
}

#[derive(Debug)]
struct RefuseBeforeReplaceOnceTicket {
    delegate: Box<dyn FileWriteTicket>,
    fired: Arc<AtomicBool>,
}

impl FileWriteTicket for RefuseBeforeReplaceOnceTicket {
    fn declare_temp_path(&mut self, path: &Path) -> io::Result<()> {
        self.delegate.declare_temp_path(path)
    }

    fn creation_started(&mut self) -> io::Result<()> {
        self.delegate.creation_started()
    }

    fn created(&mut self, file: &File, temporary_path: &Path) -> io::Result<()> {
        self.delegate.created(file, temporary_path)
    }

    fn reserve_end(&mut self, candidate_end: u64) -> io::Result<()> {
        self.delegate.reserve_end(candidate_end)
    }

    fn before_replace(&mut self, final_path: &Path) -> io::Result<()> {
        if !self.fired.swap(true, Ordering::SeqCst) {
            return Err(io::Error::other(BEFORE_REPLACE_REFUSAL));
        }
        self.delegate.before_replace(final_path)
    }

    fn replaced_and_synced(&mut self, file: &File, final_path: &Path) -> io::Result<()> {
        self.delegate.replaced_and_synced(file, final_path)
    }

    fn confirmed_writer_fd_retired(&mut self, writer: File) -> io::Result<()> {
        self.delegate.confirmed_writer_fd_retired(writer)
    }

    fn confirmed_auxiliary_writer_fd_retired(&mut self, writer: File) -> io::Result<()> {
        self.delegate.confirmed_auxiliary_writer_fd_retired(writer)
    }

    fn confirmed_uncreated_absent_and_synced(&mut self) -> io::Result<()> {
        self.delegate.confirmed_uncreated_absent_and_synced()
    }

    fn confirmed_safe_absent_after_writer_close(&mut self, writer: File) -> io::Result<()> {
        self.delegate
            .confirmed_safe_absent_after_writer_close(writer)
    }
}

fn contains_before_replace_refusal(error: &(dyn Error + 'static)) -> bool {
    let mut source = Some(error);
    while let Some(current) = source {
        if current
            .downcast_ref::<io::Error>()
            .is_some_and(|io_error| io_error.to_string() == BEFORE_REPLACE_REFUSAL)
        {
            return true;
        }
        source = current.source();
    }
    false
}

fn fail_once_registry(
    deletions: Arc<PublicationFileDeletions>,
    parent: Arc<quanta_index_core::publication_capacity::PublicationAllowance>,
    fired: Arc<AtomicBool>,
) -> Result<Arc<ObjectStoreRegistry>, Box<dyn Error>> {
    let delegate: Arc<dyn FileAdmission> =
        Arc::new(PublicationLanceAdmission::for_work(deletions, parent)?);
    Ok(qualified_registry(Arc::new(RefuseBeforeReplaceOnce {
        delegate,
        fired,
    })))
}

#[derive(Clone, Copy)]
enum LocalPublishMode {
    Multipart,
    Put,
}

async fn publish_abc(
    store: &ObjectStore,
    path: &object_store::path::Path,
    mode: LocalPublishMode,
    staged_root: Option<&Path>,
) -> TestResult {
    match mode {
        LocalPublishMode::Multipart => {
            let mut upload = store
                .inner
                .put_multipart_opts(path, PutMultipartOptions::default())
                .await?;
            upload.put_part(PutPayload::from_static(b"abc")).await?;
            if let Some(root) = staged_root {
                require_one_staged_payload(root, b"abc")?;
            }
            let _published = upload.complete().await?;
        }
        LocalPublishMode::Put => {
            let _published = store
                .inner
                .put_opts(
                    path,
                    PutPayload::from_static(b"abc"),
                    PutMode::Create.into(),
                )
                .await?;
        }
    }
    Ok(())
}

#[test]
fn real_local_rename_replays_retained_move_before_missing_source_probe() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let owner = deletion_owner(&binder);
        let parent = Arc::new(authority.reserve_publication(3, 1)?);
        let admission: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            Arc::clone(&owner),
            Arc::clone(&parent),
        )?);
        let (store, base) = ObjectStore::from_uri_and_params(
            qualified_registry(admission),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let source = root.join("source.lance");
        let destination = root.join("moved.lance");
        let source_object = base.clone().join("source.lance");
        let destination_object = base.join("moved.lance");
        publish_abc(&store, &source_object, LocalPublishMode::Put, None).await?;
        let before = authority.snapshot()?;
        let mut original = owner.begin_move(&source, &destination)?;
        original.change_started()?;
        fs::rename(&source, &destination)?;
        drop(original);
        store
            .inner
            .rename_opts(
                &source_object,
                &destination_object,
                object_store::RenameOptions::default(),
            )
            .await?;
        require(!source.exists(), "replay restored the removed source")?;
        require(
            fs::read(&destination)? == b"abc",
            "replay changed destination bytes",
        )?;
        require(
            authority.snapshot()? == before,
            "replay changed the original charge",
        )?;
        require(binder.is_quiescent()?, "replay retained namespace claims")?;
        Ok(())
    })
}

#[test]
fn real_local_create_rename_collision_keeps_kind_and_original_inodes() -> TestResult {
    use std::os::unix::fs::MetadataExt as _;
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let parent = Arc::new(authority.reserve_publication(6, 2)?);
        let admission: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            deletion_owner(&binder),
            Arc::clone(&parent),
        )?);
        let (store, base) = ObjectStore::from_uri_and_params(
            qualified_registry(admission),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let source = base.clone().join("source.lance");
        let destination = base.join("target.lance");
        publish_abc(&store, &source, LocalPublishMode::Put, None).await?;
        publish_abc(&store, &destination, LocalPublishMode::Put, None).await?;
        let source_path = root.join("source.lance");
        let target_path = root.join("target.lance");
        let source_before = fs::metadata(&source_path)?;
        let target_before = fs::metadata(&target_path)?;
        let before = authority.snapshot()?;
        let error = store
            .inner
            .rename_opts(
                &source,
                &destination,
                object_store::RenameOptions::new()
                    .with_target_mode(object_store::RenameTargetMode::Create),
            )
            .await
            .err()
            .ok_or_else(|| io::Error::other("create rename replaced a target"))?;
        let object_store::Error::AlreadyExists { source: cause, .. } = error else {
            return Err(
                io::Error::other(format!("collision lost its typed kind: {error:?}")).into(),
            );
        };
        let io = cause
            .downcast_ref::<io::Error>()
            .ok_or_else(|| io::Error::other("collision lost the original IO error"))?;
        require(
            io.kind() == io::ErrorKind::AlreadyExists,
            "collision IO kind changed",
        )?;
        require(
            matches!(
                io.get_ref()
                    .and_then(|source| source.downcast_ref::<CoreError>()),
                Some(CoreError::StorageIo {
                    kind: io::ErrorKind::AlreadyExists,
                    raw_os_error: None,
                    ..
                })
            ),
            "verified collision lost its Core filesystem cause",
        )?;
        let source_after = fs::metadata(&source_path)?;
        let target_after = fs::metadata(&target_path)?;
        require(
            source_after.dev() == source_before.dev()
                && source_after.ino() == source_before.ino()
                && target_after.dev() == target_before.dev()
                && target_after.ino() == target_before.ino(),
            "create refusal replaced an original inode",
        )?;
        require(
            fs::read(&source_path)? == b"abc" && fs::read(&target_path)? == b"abc",
            "create refusal changed fixed bytes",
        )?;
        require(
            authority.snapshot()? == before && binder.is_quiescent()?,
            "create refusal changed ledger or retained claims",
        )?;
        Ok(())
    })
}

async fn before_replace_refusal_refunds_and_retries(mode: LocalPublishMode) -> TestResult {
    let (_directory, root, _lease, authority, binder) = fixture()?;
    let boot = authority.snapshot()?;
    let fired = Arc::new(AtomicBool::new(false));
    let deletions = deletion_owner(&binder);
    let parent = Arc::new(authority.reserve_publication(3, 1)?);
    let registry = fail_once_registry(
        Arc::clone(&deletions),
        Arc::clone(&parent),
        Arc::clone(&fired),
    )?;
    let (store, base_path) = ObjectStore::from_uri_and_params(
        Arc::clone(&registry),
        root_uri(&root)?,
        &ObjectStoreParams::default(),
    )
    .await?;
    let path = base_path.join("before-replace.lance");
    let physical = root.join("before-replace.lance");
    {
        let refusal = publish_abc(&store, &path, mode, Some(&root))
            .await
            .err()
            .ok_or_else(|| io::Error::other("injected publication unexpectedly succeeded"))?;
        require(
            contains_before_replace_refusal(refusal.as_ref()),
            "publication lost the original before_replace error cause",
        )?;
    }
    require(
        fired.load(Ordering::SeqCst) && !physical.exists(),
        "before_replace refusal did not precede final-path publication",
    )?;
    drop(store);
    drop(registry);
    drop(parent);
    require(
        authority.snapshot()? == boot && binder.is_quiescent()?,
        "failed publication retained charge or binder custody after cleanup",
    )?;
    require_only_boot_lock(&root)?;

    let retry_parent = Arc::new(authority.reserve_publication(3, 1)?);
    let retry_registry = fail_once_registry(
        Arc::clone(&deletions),
        Arc::clone(&retry_parent),
        Arc::clone(&fired),
    )?;
    let (retry_store, retry_base) = ObjectStore::from_uri_and_params(
        Arc::clone(&retry_registry),
        root_uri(&root)?,
        &ObjectStoreParams::default(),
    )
    .await?;
    let retry_path = retry_base.join("before-replace.lance");
    publish_abc(&retry_store, &retry_path, mode, None).await?;
    drop(retry_store);
    drop(retry_registry);
    drop(retry_parent);
    require(
        fs::read(&physical)? == b"abc",
        "same-target retry did not publish fixed abc bytes",
    )?;
    let committed = authority.snapshot()?;
    require(
        committed.used_bytes
            == boot
                .used_bytes
                .checked_add(3)
                .ok_or("test oracle addition overflow")?
            && committed.used_files
                == boot
                    .used_files
                    .checked_add(1)
                    .ok_or("test oracle addition overflow")?
            && committed.retained_inodes
                == boot
                    .retained_inodes
                    .checked_add(1)
                    .ok_or("test oracle addition overflow")?
            && committed.active_allowances == boot.active_allowances
            && committed.active_tickets == boot.active_tickets,
        "same-target retry did not hold exactly one three-byte inode",
    )?;

    let retirement: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::retirement_only(
        Arc::clone(&deletions),
    ));
    let retirement_registry = qualified_registry(retirement);
    let (retirement_store, _base) = ObjectStore::from_uri_and_params(
        Arc::clone(&retirement_registry),
        root_uri(&root)?,
        &ObjectStoreParams::default(),
    )
    .await?;
    retirement_store.delete(&retry_path).await?;
    drop(retirement_store);
    drop(retirement_registry);
    binder.sweep_retired()?;
    require(
        authority.snapshot()? == boot && binder.is_quiescent()?,
        "retry deletion did not restore exact boot baseline",
    )?;
    require_only_boot_lock(&root)?;
    Ok(())
}

#[test]
fn real_local_multipart_before_replace_refusal_refunds_and_retries_same_target() -> TestResult {
    run_test(before_replace_refusal_refunds_and_retries(
        LocalPublishMode::Multipart,
    ))
}

#[test]
fn real_local_put_before_replace_refusal_refunds_and_retries_same_target() -> TestResult {
    run_test(before_replace_refusal_refunds_and_retries(
        LocalPublishMode::Put,
    ))
}

#[test]
fn fresh_inode_copy_before_replace_refusal_refunds_and_allows_retry() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let parent = Arc::new(authority.reserve_publication(6, 2)?);
        let delegate: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            deletion_owner(&binder),
            Arc::clone(&parent),
        )?);
        let (store, base_path) = ObjectStore::from_uri_and_params(
            qualified_registry(Arc::clone(&delegate)),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let source = base_path.clone().join("copy-source.lance");
        publish_abc(&store, &source, LocalPublishMode::Put, None).await?;
        let target = root.join("copy-target.lance");
        let before = authority.snapshot()?;
        let disk_before = regular_file_census(&root)?;
        let fired = Arc::new(AtomicBool::new(false));
        let admission = RefuseBeforeReplaceOnce {
            delegate: Arc::clone(&delegate),
            fired: Arc::clone(&fired),
        };
        {
            let refusal = lance_io::object_store::file_admission::copy_file_admitted(
                &root.join("copy-source.lance"),
                &target,
                &admission,
            )
            .err()
            .ok_or("copy unexpectedly published before the injected refusal")?;
            require(
                contains_before_replace_refusal(&refusal),
                "copy lost the original before_replace error",
            )?;
        }
        require(
            fired.load(Ordering::SeqCst)
                && !target.exists()
                && authority.snapshot()? == before
                && regular_file_census(&root)? == disk_before
                && binder.is_quiescent()?,
            "failed fresh-inode copy left a temp file, pending charge or path claim",
        )?;
        lance_io::object_store::file_admission::copy_file_admitted(
            &root.join("copy-source.lance"),
            &target,
            &admission,
        )?;
        require(
            fs::read(&target)? == b"abc",
            "copy retry changed fixed bytes",
        )?;
        store
            .inner
            .delete(&base_path.join("copy-target.lance"))
            .await?;
        store.inner.delete(&source).await?;
        drop(admission);
        drop(delegate);
        drop(store);
        drop(parent);
        require(
            authority.snapshot()? == boot && binder.is_quiescent()?,
            "fresh-inode copy retirement did not restore boot capacity",
        )?;
        require_only_boot_lock(&root)?;
        Ok(())
    })
}

#[test]
fn overwrite_copy_busy_target_retires_staging_alias_and_allows_retry() -> TestResult {
    run_test(async {
        let (_directory, root, _lease, authority, binder) = fixture()?;
        let boot = authority.snapshot()?;
        let parent = Arc::new(authority.reserve_publication(6, 2)?);
        let admission: Arc<dyn FileAdmission> = Arc::new(PublicationLanceAdmission::for_work(
            deletion_owner(&binder),
            Arc::clone(&parent),
        )?);
        let (store, base_path) = ObjectStore::from_uri_and_params(
            qualified_registry(admission),
            root_uri(&root)?,
            &ObjectStoreParams::default(),
        )
        .await?;
        let source = base_path.clone().join("alias-source.lance");
        let target = base_path.join("alias-target.lance");
        publish_abc(&store, &source, LocalPublishMode::Put, None).await?;
        let _published = store
            .inner
            .put_opts(
                &target,
                PutPayload::from_static(b"old"),
                PutMode::Create.into(),
            )
            .await?;
        let before = authority.snapshot()?;
        let disk_before = regular_file_census(&root)?;
        let opening = binder.begin_reader_open(&root.join("alias-target.lance"))?;
        for _attempt in 0..2 {
            {
                let refusal = store
                    .inner
                    .copy_opts(&source, &target, object_store::CopyOptions::default())
                    .await
                    .err()
                    .ok_or("overwrite copy ignored the held reader-open guard")?;
                require(
                    contains_budget_cause(&refusal),
                    "overwrite copy lost the path-busy capacity refusal",
                )?;
            }
            require(
                fs::read(root.join("alias-source.lance"))? == b"abc"
                    && fs::read(root.join("alias-target.lance"))? == b"old"
                    && authority.snapshot()? == before
                    && regular_file_census(&root)? == disk_before,
                "refused overwrite copy changed bytes or retained a staging charge",
            )?;
            for entry in fs::read_dir(&root)? {
                require(
                    !entry?
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".lance-admitted-copy-"),
                    "refused overwrite copy left a staging hardlink",
                )?;
            }
        }
        drop(opening);
        require(binder.is_quiescent()?, "refused copy retained path custody")?;
        store
            .inner
            .copy_opts(&source, &target, object_store::CopyOptions::default())
            .await?;
        require(
            fs::read(root.join("alias-target.lance"))? == b"abc",
            "overwrite copy retry changed fixed bytes",
        )?;
        store.inner.delete(&target).await?;
        store.inner.delete(&source).await?;
        drop(store);
        drop(parent);
        require(
            authority.snapshot()? == boot && binder.is_quiescent()?,
            "overwrite alias retirement did not restore boot capacity",
        )?;
        require_only_boot_lock(&root)?;
        Ok(())
    })
}
