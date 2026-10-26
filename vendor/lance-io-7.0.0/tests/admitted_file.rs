use std::fs::File;
use std::io;
use std::path::Path as StdPath;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::StreamExt as _;
use lance_io::object_store::file_admission::{
    AdmittedFileStoreProvider, FileAdmission, FileDirectoryDeleteTicket, FileDirectoryPathTicket,
    FileMoveMode, FilePathTicket, FileReaderLease, FileReaderOpenTicket, FileWriteTicket,
};
use lance_io::object_store::{ObjectStore, ObjectStoreRegistry};
use lance_io::traits::Writer;
use object_store::ObjectStore as _;
use object_store::ObjectStoreExt;
use object_store::path::Path;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Default)]
struct Counts {
    bytes: u64,
    files: u64,
    readers: u64,
}

#[derive(Debug)]
struct Capped {
    root: PathBuf,
    counts: Arc<Mutex<Counts>>,
    byte_limit: u64,
    file_limit: u64,
    deny_reconcile: AtomicBool,
}

impl Capped {
    fn new(bytes: u64, files: u64, root: &StdPath) -> Self {
        Self {
            root: root.to_owned(),
            counts: Arc::new(Mutex::new(Counts::default())),
            byte_limit: bytes,
            file_limit: files,
            deny_reconcile: AtomicBool::new(false),
        }
    }
}

#[derive(Debug)]
struct WriteTicket {
    counts: Arc<Mutex<Counts>>,
    byte_limit: u64,
    reserved_end: u64,
    declared: bool,
    started: bool,
}

impl FileWriteTicket for WriteTicket {
    fn declare_temp_path(&mut self, _: &StdPath) -> io::Result<()> {
        if self.started {
            return Err(io::Error::other("temp declared after creation started"));
        }
        self.declared = true;
        Ok(())
    }

    fn creation_started(&mut self) -> io::Result<()> {
        if !self.declared {
            return Err(io::Error::other("creation started before temp declaration"));
        }
        self.started = true;
        Ok(())
    }

    fn created(&mut self, _: &File, _: &StdPath) -> io::Result<()> {
        if !self.started {
            return Err(io::Error::other("created before creation_started"));
        }
        Ok(())
    }

    fn reserve_end(&mut self, candidate_end: u64) -> io::Result<()> {
        if candidate_end <= self.reserved_end {
            return Ok(());
        }
        let mut state = self
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?;
        let delta = candidate_end - self.reserved_end;
        let next = state
            .bytes
            .checked_add(delta)
            .ok_or_else(|| io::Error::other("overflow"))?;
        if next > self.byte_limit {
            return Err(io::Error::other("capacity refused before growth"));
        }
        state.bytes = next;
        self.reserved_end = candidate_end;
        Ok(())
    }

    fn before_replace(&mut self, _: &StdPath) -> io::Result<()> {
        Ok(())
    }
    fn replaced_and_synced(&mut self, _: &File, _: &StdPath) -> io::Result<()> {
        Ok(())
    }
    fn confirmed_writer_fd_retired(&mut self, writer: File) -> io::Result<()> {
        drop(writer);
        Ok(())
    }
    fn confirmed_auxiliary_writer_fd_retired(&mut self, writer: File) -> io::Result<()> {
        drop(writer);
        Ok(())
    }
    fn confirmed_uncreated_absent_and_synced(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn confirmed_safe_absent_after_writer_close(&mut self, writer: File) -> io::Result<()> {
        drop(writer);
        Ok(())
    }
}

#[derive(Debug)]
struct PathTicket {
    from: std::path::PathBuf,
    to: std::path::PathBuf,
    link: bool,
    create_only: bool,
}
impl FilePathTicket for PathTicket {
    fn perform_and_sync(&mut self) -> io::Result<()> {
        if self.link {
            std::fs::hard_link(&self.from, &self.to)?;
        } else if self.create_only {
            // The mock supports file-only create rename. The real owner uses
            // its authenticated atomic no-replace directory/file operation.
            std::fs::hard_link(&self.from, &self.to)?;
            std::fs::remove_file(&self.from)?;
        } else {
            std::fs::rename(&self.from, &self.to)?;
        }
        for path in [&self.from, &self.to] {
            let parent = path
                .parent()
                .ok_or_else(|| io::Error::other("missing parent"))?;
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ReaderLease(File, Arc<Mutex<Counts>>);
impl FileReaderLease for ReaderLease {
    fn file(&self) -> io::Result<&File> {
        Ok(&self.0)
    }
}
impl Drop for ReaderLease {
    fn drop(&mut self) {
        let mut state = self.1.lock().expect("reader count lock");
        state.readers -= 1;
    }
}

#[derive(Debug)]
struct ReaderOpen(Arc<Mutex<Counts>>);
impl FileReaderOpenTicket for ReaderOpen {
    fn opened_owned(&mut self, file: File) -> io::Result<Arc<dyn FileReaderLease>> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?;
        state.readers += 1;
        Ok(Arc::new(ReaderLease(file, Arc::clone(&self.0))))
    }
}

impl FileAdmission for Capped {
    fn validate_list_scope(&self, path: &StdPath) -> io::Result<()> {
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            io::Error::new(io::ErrorKind::PermissionDenied, "list outside held root")
        })?;
        if relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "list has non-normal path component",
            ))
        }
    }
    fn reconcile_pending_namespace(&self) -> io::Result<()> {
        if self.deny_reconcile.load(Ordering::SeqCst) {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "pending namespace reconciliation refused",
            ))
        } else {
            Ok(())
        }
    }
    fn prepare_parent_dirs(&self, final_path: &StdPath) -> io::Result<()> {
        let parent = final_path
            .parent()
            .ok_or_else(|| io::Error::other("admitted target missing parent"))?;
        std::fs::create_dir_all(parent)
    }

    fn begin_create(&self, _: &StdPath) -> io::Result<Box<dyn FileWriteTicket>> {
        let mut state = self
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?;
        let next = state
            .files
            .checked_add(1)
            .ok_or_else(|| io::Error::other("overflow"))?;
        if next > self.file_limit {
            return Err(io::Error::other("file capacity refused before create"));
        }
        state.files = next;
        Ok(Box::new(WriteTicket {
            counts: Arc::clone(&self.counts),
            byte_limit: self.byte_limit,
            reserved_end: 0,
            declared: false,
            started: false,
        }))
    }

    fn begin_byte_copy(&self, _: &File, target: &StdPath) -> io::Result<Box<dyn FileWriteTicket>> {
        self.begin_create(target)
    }

    fn begin_link(&self, from: &StdPath, to: &StdPath) -> io::Result<Box<dyn FilePathTicket>> {
        Ok(Box::new(PathTicket {
            from: from.to_owned(),
            to: to.to_owned(),
            link: true,
            create_only: false,
        }))
    }
    fn begin_move(
        &self,
        from: &StdPath,
        to: &StdPath,
        mode: FileMoveMode,
    ) -> io::Result<Box<dyn FilePathTicket>> {
        Ok(Box::new(PathTicket {
            from: from.to_owned(),
            to: to.to_owned(),
            link: false,
            create_only: matches!(mode, FileMoveMode::Create),
        }))
    }
    fn delete_file_and_sync(&self, path: &StdPath) -> io::Result<()> {
        std::fs::remove_file(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("missing parent"))?;
        File::open(parent)?.sync_all()
    }
    fn begin_directory_move(
        &self,
        _: &StdPath,
        _: &StdPath,
    ) -> io::Result<Box<dyn FileDirectoryPathTicket>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file-cap mock has no directory custody",
        ))
    }
    fn begin_directory_delete(
        &self,
        _: &StdPath,
    ) -> io::Result<Box<dyn FileDirectoryDeleteTicket>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file-cap mock has no directory custody",
        ))
    }
    fn begin_reader_open(&self, _: &StdPath) -> io::Result<Box<dyn FileReaderOpenTicket>> {
        Ok(Box::new(ReaderOpen(Arc::clone(&self.counts))))
    }
}

async fn store(
    root: &StdPath,
    admission: Arc<dyn FileAdmission>,
) -> Result<Arc<ObjectStore>, Box<dyn std::error::Error>> {
    let registry = Arc::new(ObjectStoreRegistry::default());
    registry.insert("file", Arc::new(AdmittedFileStoreProvider::new(admission)));
    let (store, _) = ObjectStore::from_uri_and_params(
        registry,
        root.to_str()
            .ok_or_else(|| io::Error::other("non utf8 temp root"))?,
        &Default::default(),
    )
    .await?;
    assert!(store.is_local());
    Ok(store)
}

fn object_path(root: &StdPath, name: &str) -> Path {
    Path::from_absolute_path(root.join(name)).expect("absolute object path")
}

#[tokio::test]
async fn local_writer_refuses_growth_before_touching_old_output()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let path = object_path(root.path(), "data.lance");
    let capped = Arc::new(Capped::new(3, 1, root.path()));
    let store = store(root.path(), capped.clone()).await?;
    let mut writer = store.create(&path).await?;
    writer.write_all(b"abc").await?;
    assert!(writer.write_all(b"d").await.is_err());
    drop(writer);
    assert!(!root.path().join("data.lance").exists());
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .bytes,
        3
    );
    Ok(())
}

#[tokio::test]
async fn inner_manifest_put_and_local_copy_use_distinct_inode_accounts()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let capped = Arc::new(Capped::new(8, 2, root.path()));
    let store = store(root.path(), capped.clone()).await?;
    let manifest = object_path(root.path(), "manifest");
    store.inner.put(&manifest, b"abcd".to_vec().into()).await?;
    let copy = object_path(root.path(), "copy");
    store.copy(&manifest, &copy).await?;
    assert_eq!(std::fs::read(root.path().join("copy"))?, b"abcd");
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .bytes,
        8
    );
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .files,
        2
    );
    Ok(())
}

#[tokio::test]
async fn reader_lease_outlives_local_reader_object_until_stream_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let capped = Arc::new(Capped::new(8, 1, root.path()));
    let store = store(root.path(), capped.clone()).await?;
    let path = object_path(root.path(), "data");
    let mut writer = store.create(&path).await?;
    writer.write_all(b"x").await?;
    Writer::shutdown(writer.as_mut()).await?;
    let reader = store.open(&path).await?;
    let stream = reader.get_stream().await?;
    drop(reader);
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .readers,
        1
    );
    drop(stream);
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .readers,
        0
    );
    Ok(())
}

#[tokio::test]
async fn inner_reader_lease_outlives_result_until_stream_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let capped = Arc::new(Capped::new(8, 1, root.path()));
    let store = store(root.path(), capped.clone()).await?;
    let path = object_path(root.path(), "manifest");
    store.inner.put(&path, b"x".to_vec().into()).await?;
    let result = store.inner.get(&path).await?;
    let stream = result.into_stream();
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .readers,
        1
    );
    drop(stream);
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .readers,
        0
    );
    Ok(())
}

#[tokio::test]
async fn namespace_reconciliation_is_required_for_observation_not_move_ticket()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let capped = Arc::new(Capped::new(8, 2, root.path()));
    let store = store(root.path(), capped.clone()).await?;
    let source = object_path(root.path(), "source");
    let target = object_path(root.path(), "target");
    store.inner.put(&source, b"abc".to_vec().into()).await?;

    capped.deny_reconcile.store(true, Ordering::SeqCst);
    assert!(store.inner.get(&source).await.is_err());
    let listed = store
        .inner
        .list(None)
        .next()
        .await
        .ok_or_else(|| io::Error::other("missing list refusal"))?;
    assert!(listed.is_err());
    assert!(
        store
            .inner
            .copy_if_not_exists(&source, &target)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(root.path().join("source"))?, b"abc");
    assert!(!root.path().join("target").exists());

    store.inner.rename_if_not_exists(&source, &target).await?;
    assert!(!root.path().join("source").exists());
    assert_eq!(std::fs::read(root.path().join("target"))?, b"abc");

    capped.deny_reconcile.store(false, Ordering::SeqCst);
    assert_eq!(
        store.inner.get(&target).await?.bytes().await?.as_ref(),
        b"abc"
    );
    Ok(())
}

#[tokio::test]
async fn namespace_ticket_errors_keep_object_store_not_found_and_already_exists()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let capped = Arc::new(Capped::new(8, 2, root.path()));
    let store = store(root.path(), capped).await?;
    let source = object_path(root.path(), "source");
    let target = object_path(root.path(), "target");
    let absent = object_path(root.path(), "absent");
    let missing_target = object_path(root.path(), "missing-target");
    store.inner.put(&source, b"abc".to_vec().into()).await?;
    store.inner.put(&target, b"xyz".to_vec().into()).await?;

    let exists = store.inner.copy_if_not_exists(&source, &target).await;
    assert!(matches!(
        exists,
        Err(object_store::Error::AlreadyExists { .. })
    ));
    let missing = store
        .inner
        .copy_if_not_exists(&absent, &missing_target)
        .await;
    assert!(matches!(missing, Err(object_store::Error::NotFound { .. })));
    assert_eq!(std::fs::read(root.path().join("source"))?, b"abc");
    assert_eq!(std::fs::read(root.path().join("target"))?, b"xyz");
    Ok(())
}

#[tokio::test]
async fn direct_admitted_local_routes_mutations_through_the_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let capped = Arc::new(Capped::new(8, 2, root.path()));
    let store = store(root.path(), capped.clone()).await?;
    let source = object_path(root.path(), "source");
    let target = object_path(root.path(), "target");
    let moved = object_path(root.path(), "moved");
    store.inner.put(&source, b"abc".to_vec().into()).await?;
    let direct = object_store::local::LocalFileSystem::new().with_admission(capped);

    direct.copy_if_not_exists(&source, &target).await?;
    assert_eq!(std::fs::read(root.path().join("target"))?, b"abc");
    direct.rename(&target, &moved).await?;
    assert!(!root.path().join("target").exists());
    assert_eq!(std::fs::read(root.path().join("moved"))?, b"abc");
    direct.delete(&moved).await?;
    assert!(!root.path().join("moved").exists());
    assert_eq!(std::fs::read(root.path().join("source"))?, b"abc");
    Ok(())
}

#[tokio::test]
async fn direct_admitted_local_reader_owns_actual_fd_until_stream_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let capped = Arc::new(Capped::new(8, 1, root.path()));
    let store = store(root.path(), capped.clone()).await?;
    let source = object_path(root.path(), "source");
    store.inner.put(&source, b"abc".to_vec().into()).await?;
    let direct = object_store::local::LocalFileSystem::new().with_admission(capped.clone());
    let result = direct.get(&source).await?;
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .readers,
        1
    );
    let stream = result.into_stream();
    drop(stream);
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .readers,
        0
    );
    assert_eq!(direct.get_range(&source, 1..3).await?.as_ref(), b"bc");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn admitted_listing_skips_directory_symlinks_and_retires_file_leases()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let foreign = tempfile::tempdir()?;
    std::fs::write(foreign.path().join("secret"), b"private")?;
    let capped = Arc::new(Capped::new(8, 1, root.path()));
    let store = store(root.path(), capped.clone()).await?;
    let source = object_path(root.path(), "source");
    store.inner.put(&source, b"abc".to_vec().into()).await?;
    std::os::unix::fs::symlink(foreign.path(), root.path().join("foreign-link"))?;
    let prefix = Path::from_absolute_path(root.path())?;

    let mut listed = store.inner.list(Some(&prefix));
    let first = listed
        .next()
        .await
        .ok_or_else(|| io::Error::other("admitted source missing from listing"))??;
    assert_eq!(first.location, source);
    assert!(listed.next().await.is_none());
    let delimited = store.inner.list_with_delimiter(Some(&prefix)).await?;
    assert_eq!(delimited.objects.len(), 1);
    assert_eq!(
        delimited.objects.first().map(|entry| &entry.location),
        Some(&source)
    );
    assert!(delimited.common_prefixes.is_empty());
    assert_eq!(
        capped
            .counts
            .lock()
            .map_err(|_| io::Error::other("count lock poisoned"))?
            .readers,
        0
    );
    Ok(())
}

#[tokio::test]
async fn admitted_listing_refuses_unscoped_walk_roots_before_traversal()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let foreign = tempfile::tempdir()?;
    let capped = Arc::new(Capped::new(8, 1, root.path()));
    let direct = object_store::local::LocalFileSystem::new().with_admission(capped);
    let foreign_prefix = Path::from_absolute_path(foreign.path())?;
    let held_prefix = Path::from_absolute_path(root.path())?;

    let unscoped = direct
        .list(None)
        .next()
        .await
        .ok_or_else(|| io::Error::other("missing unscoped-list refusal"))?;
    assert!(unscoped.is_err());
    assert!(direct.list_with_delimiter(None).await.is_err());
    let foreign_result = direct
        .list(Some(&foreign_prefix))
        .next()
        .await
        .ok_or_else(|| io::Error::other("missing foreign-list refusal"))?;
    assert!(foreign_result.is_err());
    assert!(
        direct
            .list_with_delimiter(Some(&foreign_prefix))
            .await
            .is_err()
    );

    assert!(direct.list(Some(&held_prefix)).next().await.is_none());
    let held = direct.list_with_delimiter(Some(&held_prefix)).await?;
    assert!(held.objects.is_empty());
    assert!(held.common_prefixes.is_empty());
    Ok(())
}

#[tokio::test]
async fn explicit_inner_store_cannot_bypass_the_qualified_provider()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let registry = Arc::new(ObjectStoreRegistry::default());
    registry.insert(
        "file",
        Arc::new(AdmittedFileStoreProvider::new(Arc::new(Capped::new(
            8,
            1,
            root.path(),
        )))),
    );
    let mut params = lance_io::object_store::ObjectStoreParams::default();
    #[allow(deprecated)]
    {
        params.object_store = Some((
            Arc::new(object_store::local::LocalFileSystem::new()),
            url::Url::parse("file:///")?,
        ));
    }
    let uri = root
        .path()
        .to_str()
        .ok_or_else(|| io::Error::other("non utf8 temp root"))?;
    assert!(
        ObjectStore::from_uri_and_params(registry, uri, &params)
            .await
            .is_err()
    );
    Ok(())
}
