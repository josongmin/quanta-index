//! Explicit filesystem fixture for qualified routing and search parity.
//! Product accounting belongs to the Runtime backend tests.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tantivy::TantivyError;
use tantivy::directory::error::{LockError, OpenReadError, OpenWriteError};

use quanta_index_contract::lex::CommitSha;
use quanta_index_contract::{
    AuxEpochV1, LqExpr, LqLeaf, LqOptions, ManifestGeneration, RepoId, RevisionId,
};
use quanta_index_core::publication_admission::{
    PublicationControlFileMutation, PublicationObjectReader,
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, HistoryTextBuildV1, HistoryTextDocKeyV1, HistoryTextDocV1,
    HistoryTextEpochStatusV1, HistoryTextIndexPort, HistoryTextKindV1, HistoryTextQueryV1,
    RequestBudgetV1,
};
use quanta_index_core::{
    HistoryTextDirectoryEntry, HistoryTextFileAdmissionFactory, HistoryTextFileIoPort,
    HistoryTextPhysicalLock,
};

use super::{HistoryTextIndexAdapter, engine_error};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn expired_budget() -> Result<RequestBudgetV1, &'static str> {
    let deadline = Instant::now()
        .checked_sub(Duration::from_millis(1))
        .ok_or("test monotonic clock cannot represent the expired deadline")?;
    Ok(RequestBudgetV1::until(deadline))
}

#[test]
fn expired_or_cancelled_publish_refuses_before_root_birth_or_parent_reservation() -> TestResult {
    let directory = tempfile::tempdir()?;
    let qualified_root = directory.path().join("qualified");
    let raw_root = directory.path().join("raw");
    let factory = fixture_factory(qualified_root.clone(), false);
    let qualified =
        HistoryTextIndexAdapter::with_file_admission(qualified_root.clone(), factory.clone())?;
    let raw = HistoryTextIndexAdapter::with_root(raw_root.clone())?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("rev")?,
        generation: ManifestGeneration::new(3),
    };
    let build = HistoryTextBuildV1::Full {
        docs: vec![document(1, "needle")],
    };
    let cancelled = RequestBudgetV1::unbounded();
    cancelled.cancel_handle().cancel();
    for (budget, expected) in [
        (
            expired_budget()?,
            quanta_index_core::REQUEST_DEADLINE_EXCEEDED_CODE,
        ),
        (cancelled, quanta_index_core::REQUEST_CANCELLED_CODE),
    ] {
        for adapter in [&qualified, &raw] {
            if !matches!(
                adapter.publish_epoch(&generation, AuxEpochV1::new(1), build.clone(), &budget),
                Err(CoreError::Typed { code, .. }) if code == expected
            ) {
                return Err(
                    "interrupted fresh history publish lost its typed request cause".into(),
                );
            }
        }
    }
    if qualified_root.exists()
        || raw_root.exists()
        || factory.publications.load(Ordering::SeqCst) != 0
        || factory.io.writes.load(Ordering::SeqCst) != 0
    {
        return Err("interrupted history publish created a file or reserved a parent".into());
    }
    Ok(())
}

#[test]
fn cancelled_raw_retry_retires_old_staging_before_refusing_fresh_work() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("raw");
    let adapter = HistoryTextIndexAdapter::with_root(root.clone())?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("rev")?,
        generation: ManifestGeneration::new(3),
    };
    let epoch = AuxEpochV1::new(1);
    let staging = super::layout::staging_dir(&root, &generation, epoch);
    fs::create_dir_all(&staging)?;
    fs::write(staging.join("incomplete"), b"old")?;
    let cancelled = RequestBudgetV1::unbounded();
    cancelled.cancel_handle().cancel();
    if !matches!(
        adapter.publish_epoch(
            &generation,
            epoch,
            HistoryTextBuildV1::Full {
                docs: vec![document(1, "new")],
            },
            &cancelled,
        ),
        Err(CoreError::Typed { code, .. }) if code == quanta_index_core::REQUEST_CANCELLED_CODE
    ) || staging.exists()
        || super::layout::epoch_dir(&root, &generation, epoch).exists()
    {
        return Err("cancelled raw retry left old staging or began a new epoch".into());
    }
    Ok(())
}

#[test]
fn tantivy_facade_retains_nested_admission_and_real_io_causes() -> TestResult {
    let typed = CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: "fixed admission refusal".into(),
    };
    let nested = io::Error::other(io::Error::other(typed));
    let read = TantivyError::OpenReadError(OpenReadError::wrap_io_error(
        nested,
        PathBuf::from("meta.json"),
    ));
    if !matches!(
        engine_error("history read", &read),
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        }
    ) {
        return Err("nested typed admission cause was lost by Tantivy read facade".into());
    }

    let enospc = rustix::io::Errno::NOSPC.raw_os_error();
    let write = TantivyError::OpenWriteError(OpenWriteError::wrap_io_error(
        io::Error::other(io::Error::from_raw_os_error(enospc)),
        PathBuf::from("segment"),
    ));
    if !matches!(
        engine_error("history write", &write),
        CoreError::StorageIo {
            kind: io::ErrorKind::StorageFull,
            raw_os_error: Some(code),
            ..
        } if code == enospc
    ) {
        return Err("raw ENOSPC cause was lost by Tantivy write facade".into());
    }

    let lookalike = TantivyError::LockFailure(
        LockError::wrap_io_error(io::Error::other(
            "IngestResourceBudgetExceeded: fixed lookalike",
        )),
        None,
    );
    let observed = engine_error("history lock", &lookalike);
    if !matches!(
        &observed,
        CoreError::StorageIo {
            kind: io::ErrorKind::Other,
            raw_os_error: None,
            message,
        } if message.contains("history lock")
            && message.contains("IngestResourceBudgetExceeded: fixed lookalike")
    ) {
        return Err(format!("text lookalike lost its original I/O cause: {observed:?}").into());
    }
    Ok(())
}

fn storage(context: &str, error: &io::Error) -> CoreError {
    CoreError::storage_io(context, error)
}

struct FixtureReader {
    file: File,
    retired: Arc<AtomicUsize>,
}

impl Read for FixtureReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.file.read(output)
    }
}
impl Seek for FixtureReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}
impl PublicationObjectReader for FixtureReader {
    fn metadata(&self) -> Result<Metadata, CoreError> {
        self.file
            .metadata()
            .map_err(|error| storage("fixture reader metadata", &error))
    }
    fn sync_posix(&self) -> Result<(), CoreError> {
        self.file
            .sync_all()
            .map_err(|error| storage("fixture reader sync", &error))
    }
    fn retire(self: Box<Self>) -> Result<(), CoreError> {
        let _prior = self.retired.fetch_add(1, Ordering::SeqCst);
        drop(self);
        Ok(())
    }
}

struct FixtureWriter(File);

impl Read for FixtureWriter {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.0.read(output)
    }
}
impl Write for FixtureWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        self.0.write(input)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
impl Seek for FixtureWriter {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.seek(position)
    }
}
impl PublicationControlFileMutation for FixtureWriter {
    fn metadata(&self) -> Result<Metadata, CoreError> {
        self.0
            .metadata()
            .map_err(|error| storage("fixture writer metadata", &error))
    }
    fn set_len(&mut self, length: u64) -> Result<(), CoreError> {
        self.0
            .set_len(length)
            .map_err(|error| storage("fixture writer length", &error))
    }
    fn sync_and_settle(self: Box<Self>) -> Result<(), CoreError> {
        self.0
            .sync_all()
            .map_err(|error| storage("fixture writer sync", &error))
    }
    fn retire_preserving_pending(self: Box<Self>) -> Result<(), CoreError> {
        drop(self);
        Ok(())
    }
}

struct FixtureLock {
    retired: Arc<AtomicUsize>,
}
impl HistoryTextPhysicalLock for FixtureLock {
    fn retire(self: Box<Self>) -> Result<(), CoreError> {
        let _prior = self.retired.fetch_add(1, Ordering::SeqCst);
        drop(self);
        Ok(())
    }
}

struct FixtureIo {
    root: PathBuf,
    writes: AtomicUsize,
    refuse_reader: AtomicBool,
    locks_retired: Arc<AtomicUsize>,
    readers_retired: Arc<AtomicUsize>,
    fail_after_move_once: AtomicBool,
    pending_move: Mutex<Option<(PathBuf, PathBuf, u64, u64)>>,
    fail_after_delete_once: AtomicBool,
    pending_delete: Mutex<Option<(PathBuf, u64)>>,
}

impl FixtureIo {
    fn check(&self, path: &Path) -> Result<(), CoreError> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err(CoreError::InvalidContract("foreign history path".into()))
        }
    }
}

impl HistoryTextFileIoPort for FixtureIo {
    fn original_interruption(&self) -> Result<Option<CoreError>, CoreError> {
        Ok(None)
    }

    fn validate_root(&self, root: &Path) -> Result<(), CoreError> {
        if root == self.root {
            Ok(())
        } else {
            Err(CoreError::InvalidContract("foreign history root".into()))
        }
    }
    fn metadata(&self, path: &Path) -> Result<Option<Metadata>, CoreError> {
        self.check(path)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(storage("fixture metadata", &error)),
        }
    }
    fn list_directory(&self, path: &Path) -> Result<Vec<HistoryTextDirectoryEntry>, CoreError> {
        self.check(path)?;
        fs::read_dir(path)
            .map_err(|error| storage("fixture list", &error))?
            .map(|entry| {
                let entry = entry.map_err(|error| storage("fixture entry", &error))?;
                Ok(HistoryTextDirectoryEntry {
                    name: entry.file_name(),
                    metadata: entry
                        .metadata()
                        .map_err(|error| storage("fixture entry metadata", &error))?,
                })
            })
            .collect()
    }
    fn ensure_directory(&self, path: &Path) -> Result<(), CoreError> {
        self.check(path)?;
        fs::create_dir_all(path).map_err(|error| storage("fixture directory", &error))
    }
    fn open_reader(
        &self,
        path: &Path,
    ) -> Result<Option<Box<dyn PublicationObjectReader>>, CoreError> {
        self.check(path)?;
        if self.refuse_reader.load(Ordering::SeqCst) {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                message: "fixture read admission refused".into(),
            });
        }
        match File::open(path) {
            Ok(file) => Ok(Some(Box::new(FixtureReader {
                file,
                retired: Arc::clone(&self.readers_retired),
            }))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(storage("fixture open reader", &error)),
        }
    }
    fn create_writer(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError> {
        self.check(path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| storage("fixture create writer", &error))?;
        let _prior = self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FixtureWriter(file)))
    }
    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
        self.check(path)?;
        let temporary = path.with_extension("fixture-tmp");
        fs::write(&temporary, bytes).map_err(|error| storage("fixture atomic stage", &error))?;
        File::open(&temporary)
            .and_then(|file| file.sync_all())
            .map_err(|error| storage("fixture atomic sync", &error))?;
        fs::rename(&temporary, path).map_err(|error| storage("fixture atomic rename", &error))?;
        self.sync_directory(
            path.parent()
                .ok_or_else(|| CoreError::InvalidContract("fixture parent missing".into()))?,
        )?;
        let _prior = self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn delete_file(&self, path: &Path) -> Result<(), CoreError> {
        self.check(path)?;
        fs::remove_file(path).map_err(|error| storage("fixture delete", &error))
    }
    fn link_file(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        self.check(source)?;
        self.check(target)?;
        fs::hard_link(source, target).map_err(|error| storage("fixture link", &error))
    }
    fn copy_file(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        self.check(source)?;
        self.check(target)?;
        let _copied = fs::copy(source, target).map_err(|error| storage("fixture copy", &error))?;
        Ok(())
    }
    fn move_directory_if_absent(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        self.check(source)?;
        self.check(target)?;
        if target.exists() {
            return Err(CoreError::InvalidContract(
                "fixture target already exists".into(),
            ));
        }
        fs::rename(source, target).map_err(|error| storage("fixture move", &error))?;
        if self.fail_after_move_once.swap(false, Ordering::SeqCst) {
            let moved = fs::symlink_metadata(target)
                .map_err(|error| storage("fixture moved target stat", &error))?;
            *self.pending_move.lock().map_err(|error| {
                CoreError::Storage(format!("fixture move custody poisoned: {error}"))
            })? = Some((
                source.to_path_buf(),
                target.to_path_buf(),
                moved.dev(),
                moved.ino(),
            ));
            return Err(CoreError::Storage(
                "injected after actual history move".into(),
            ));
        }
        Ok(())
    }
    fn remove_tree(&self, path: &Path) -> Result<Option<u64>, CoreError> {
        self.check(path)?;
        if !path.exists() {
            return Ok(None);
        }
        let bytes = super::layout::tree_bytes(path)?;
        fs::remove_dir_all(path).map_err(|error| storage("fixture remove tree", &error))?;
        if self.fail_after_delete_once.swap(false, Ordering::SeqCst) {
            *self.pending_delete.lock().map_err(|error| {
                CoreError::Storage(format!("fixture delete custody poisoned: {error}"))
            })? = Some((path.to_path_buf(), bytes));
            return Err(CoreError::Storage(
                "injected after actual history unlink".into(),
            ));
        }
        Ok(Some(bytes))
    }
    fn recover_directory_move(&self, source: &Path, target: &Path) -> Result<bool, CoreError> {
        self.check(source)?;
        self.check(target)?;
        let mut pending = self.pending_move.lock().map_err(|error| {
            CoreError::Storage(format!("fixture move custody poisoned: {error}"))
        })?;
        let Some((original_source, original_target, original_dev, original_ino)) = pending.as_ref()
        else {
            return Ok(false);
        };
        let observed = fs::symlink_metadata(target)
            .map_err(|error| storage("fixture replay target stat", &error))?;
        if original_source != source
            || original_target != target
            || source.exists()
            || !observed.is_dir()
            || observed.dev() != *original_dev
            || observed.ino() != *original_ino
        {
            return Err(CoreError::InvalidContract(
                "fixture move replay witness differs".into(),
            ));
        }
        *pending = None;
        drop(pending);
        Ok(true)
    }
    fn recover_tree_delete(&self, path: &Path) -> Result<Option<u64>, CoreError> {
        self.check(path)?;
        let mut pending = self.pending_delete.lock().map_err(|error| {
            CoreError::Storage(format!("fixture delete custody poisoned: {error}"))
        })?;
        let Some((original, bytes)) = pending.as_ref() else {
            return Ok(None);
        };
        if original != path || path.exists() {
            return Err(CoreError::InvalidContract(
                "fixture delete replay witness differs".into(),
            ));
        }
        let receipt = *bytes;
        *pending = None;
        drop(pending);
        Ok(Some(receipt))
    }
    fn sync_directory(&self, path: &Path) -> Result<(), CoreError> {
        self.check(path)?;
        File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|error| storage("fixture directory sync", &error))
    }
    fn acquire_lock(
        &self,
        path: &Path,
        _blocking: bool,
    ) -> Result<Box<dyn HistoryTextPhysicalLock>, CoreError> {
        self.check(path)?;
        Ok(Box::new(FixtureLock {
            retired: Arc::clone(&self.locks_retired),
        }))
    }
}

struct FixtureFactory {
    io: Arc<FixtureIo>,
    refuse_publication: AtomicBool,
    publications: AtomicUsize,
}

impl HistoryTextFileAdmissionFactory for FixtureFactory {
    fn for_read(&self) -> Result<Arc<dyn HistoryTextFileIoPort>, CoreError> {
        Ok(self.io.clone())
    }
    fn for_publication(
        &self,
        budget: &RequestBudgetV1,
    ) -> Result<Arc<dyn HistoryTextFileIoPort>, CoreError> {
        budget.checkpoint("history text fixture: before publication allowance")?;
        let _prior = self.publications.fetch_add(1, Ordering::SeqCst);
        if self.refuse_publication.load(Ordering::SeqCst) {
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                message: "fixture publication allowance refused".into(),
            })
        } else {
            Ok(self.io.clone())
        }
    }
    fn for_retirement(&self) -> Result<Arc<dyn HistoryTextFileIoPort>, CoreError> {
        Ok(self.io.clone())
    }
}

fn document(byte: u8, text: &str) -> HistoryTextDocV1 {
    HistoryTextDocV1 {
        key: HistoryTextDocKeyV1::Commit {
            sha: CommitSha::from_bytes([byte; 20]),
        },
        committer_time_ms: u64::from(byte),
        text: text.into(),
    }
}

fn fixture_factory(root: PathBuf, refuse_publication: bool) -> Arc<FixtureFactory> {
    Arc::new(FixtureFactory {
        io: Arc::new(FixtureIo {
            root,
            writes: AtomicUsize::new(0),
            refuse_reader: AtomicBool::new(false),
            locks_retired: Arc::new(AtomicUsize::new(0)),
            readers_retired: Arc::new(AtomicUsize::new(0)),
            fail_after_move_once: AtomicBool::new(false),
            pending_move: Mutex::new(None),
            fail_after_delete_once: AtomicBool::new(false),
            pending_delete: Mutex::new(None),
        }),
        refuse_publication: AtomicBool::new(refuse_publication),
        publications: AtomicUsize::new(0),
    })
}

#[test]
fn qualified_full_incremental_query_and_refusal_have_raw_parity() -> TestResult {
    let directory = tempfile::tempdir()?;
    let qualified_root = directory.path().join("qualified");
    let raw_root = directory.path().join("raw");
    let factory = fixture_factory(qualified_root.clone(), true);
    let qualified =
        HistoryTextIndexAdapter::with_file_admission(qualified_root.clone(), factory.clone())?;
    let raw = HistoryTextIndexAdapter::with_root(raw_root)?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("rev")?,
        generation: ManifestGeneration::new(3),
    };
    let full = HistoryTextBuildV1::Full {
        docs: vec![document(1, "needle alpha"), document(2, "beta")],
    };
    if !matches!(
        qualified.publish_epoch(
            &generation,
            AuxEpochV1::new(1),
            full.clone(),
            &RequestBudgetV1::unbounded()
        ),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        })
    ) || qualified_root.exists()
        || factory.io.writes.load(Ordering::SeqCst) != 0
    {
        return Err("publication refusal mutated the history root".into());
    }
    factory.refuse_publication.store(false, Ordering::SeqCst);
    let _qualified_receipt = qualified.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        full.clone(),
        &RequestBudgetV1::unbounded(),
    )?;
    let _raw_receipt = raw.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        full,
        &RequestBudgetV1::unbounded(),
    )?;
    let update = HistoryTextBuildV1::Incremental {
        base: AuxEpochV1::new(1),
        upserts: vec![document(2, "needle beta")],
    };
    let _qualified_receipt = qualified.publish_epoch(
        &generation,
        AuxEpochV1::new(2),
        update.clone(),
        &RequestBudgetV1::unbounded(),
    )?;
    let _raw_receipt = raw.publish_epoch(
        &generation,
        AuxEpochV1::new(2),
        update.clone(),
        &RequestBudgetV1::unbounded(),
    )?;
    if qualified.epoch_status(&generation, AuxEpochV1::new(2))?
        != HistoryTextEpochStatusV1::Servable
        || qualified.durable_epochs(&generation)? != vec![AuxEpochV1::new(1), AuxEpochV1::new(2)]
        || factory.publications.load(Ordering::SeqCst) != 3
        || factory.io.writes.load(Ordering::SeqCst) == 0
        || factory.io.locks_retired.load(Ordering::SeqCst) == 0
    {
        return Err("qualified full/incremental publication or scope ordering failed".into());
    }
    let query = HistoryTextQueryV1 {
        kind: HistoryTextKindV1::Commit,
        expr: LqExpr::Leaf(LqLeaf::Keyword("needle".into())),
        options: LqOptions::defaults(),
    };
    let qualified_searcher = qualified.open_epoch(&generation, AuxEpochV1::new(2))?;
    let raw_searcher = raw.open_epoch(&generation, AuxEpochV1::new(2))?;
    let admit: Arc<quanta_index_core::HistoryTextAdmitFn> = Arc::new(|_hit| Ok(true));
    let qualified_page = qualified_searcher.search(
        &query,
        None,
        1,
        admit.clone(),
        &RequestBudgetV1::unbounded(),
    )?;
    let raw_page = raw_searcher.search(
        &query,
        None,
        1,
        admit.clone(),
        &RequestBudgetV1::unbounded(),
    )?;
    if qualified_page != raw_page || qualified_page.hits.len() != 1 {
        return Err("qualified and raw first ranked query pages differ".into());
    }
    let qualified_next = qualified_searcher.search(
        &query,
        qualified_page.hits.first(),
        1,
        admit.clone(),
        &RequestBudgetV1::unbounded(),
    )?;
    let raw_next = raw_searcher.search(
        &query,
        raw_page.hits.first(),
        1,
        admit,
        &RequestBudgetV1::unbounded(),
    )?;
    if qualified_next != raw_next || qualified_next.hits.len() != 1 {
        return Err("qualified and raw cursor continuation or score differs".into());
    }
    factory.io.refuse_reader.store(true, Ordering::SeqCst);
    if !matches!(
        qualified.epoch_status(&generation, AuxEpochV1::new(2)),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        })
    ) {
        return Err("qualified read refusal lost its typed cause".into());
    }
    factory.io.refuse_reader.store(false, Ordering::SeqCst);
    drop(qualified_searcher);
    if factory.io.readers_retired.load(Ordering::SeqCst) == 0 {
        return Err("qualified original readers were never retired".into());
    }
    if !matches!(
        qualified.discard_epoch(&generation, AuxEpochV1::new(1), None)?,
        quanta_index_core::HistoryTextDiscardOutcomeV1::Discarded { .. }
    ) || qualified.epoch_status(&generation, AuxEpochV1::new(2))?
        != HistoryTextEpochStatusV1::Servable
    {
        return Err("qualified retired epoch altered the later epoch".into());
    }
    let publication_calls = factory.publications.load(Ordering::SeqCst);
    factory.refuse_publication.store(true, Ordering::SeqCst);
    if qualified
        .publish_epoch(
            &generation,
            AuxEpochV1::new(2),
            update,
            &RequestBudgetV1::unbounded(),
        )?
        .docs_written
        != 1
        || factory.publications.load(Ordering::SeqCst) != publication_calls
    {
        return Err("published incremental retry required its retired base or a new parent".into());
    }
    Ok(())
}

#[test]
fn completed_move_replays_only_the_original_framed_build_and_receipt() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("qualified");
    let factory = fixture_factory(root.clone(), false);
    let adapter = HistoryTextIndexAdapter::with_file_admission(root.clone(), factory.clone())?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("rev")?,
        generation: ManifestGeneration::new(3),
    };
    let first = document(1, "old duplicate");
    let last = document(1, "final duplicate");
    let original = HistoryTextBuildV1::Full {
        docs: vec![first, last.clone()],
    };
    factory
        .io
        .fail_after_move_once
        .store(true, Ordering::SeqCst);
    if !matches!(
        adapter.publish_epoch(
            &generation,
            AuxEpochV1::new(1),
            original.clone(),
            &RequestBudgetV1::unbounded()
        ),
        Err(CoreError::Storage(_))
    ) {
        return Err("injected completed move did not interrupt publication".into());
    }
    let published = super::layout::epoch_dir(&root, &generation, AuxEpochV1::new(1));
    let manifest_path = published.join("history-text-manifest.cbor");
    let original_manifest = fs::read(&manifest_path)?;
    let writes = factory.io.writes.load(Ordering::SeqCst);
    let publication_calls = factory.publications.load(Ordering::SeqCst);
    // Recovery must succeed even when no new publication parent can be held.
    factory.refuse_publication.store(true, Ordering::SeqCst);
    let changed = HistoryTextBuildV1::Full { docs: vec![last] };
    if !matches!(
        adapter.publish_epoch(
            &generation,
            AuxEpochV1::new(1),
            changed,
            &RequestBudgetV1::unbounded()
        ),
        Err(CoreError::InvalidContract(_))
    ) || fs::read(&manifest_path)? != original_manifest
        || factory.io.writes.load(Ordering::SeqCst) != writes
        || factory.publications.load(Ordering::SeqCst) != publication_calls
    {
        return Err("different original build replaced a completed epoch".into());
    }
    let cancelled_replay = RequestBudgetV1::unbounded();
    cancelled_replay.cancel_handle().cancel();
    let receipt =
        adapter.publish_epoch(&generation, AuxEpochV1::new(1), original, &cancelled_replay)?;
    if receipt.docs_written != 1
        || fs::read(&manifest_path)? != original_manifest
        || factory.io.writes.load(Ordering::SeqCst) != writes
        || factory.publications.load(Ordering::SeqCst) != publication_calls
        || factory
            .io
            .pending_move
            .lock()
            .map_err(|error| error.to_string())?
            .is_some()
    {
        return Err("same original build did not replay its fixed receipt without writes".into());
    }
    Ok(())
}

#[test]
fn completed_tree_delete_replays_original_receipt_after_absence() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("qualified");
    let factory = fixture_factory(root.clone(), false);
    let adapter = HistoryTextIndexAdapter::with_file_admission(root.clone(), factory.clone())?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("rev")?,
        generation: ManifestGeneration::new(3),
    };
    let _receipt = adapter.publish_epoch(
        &generation,
        AuxEpochV1::new(1),
        HistoryTextBuildV1::Full {
            docs: vec![document(1, "needle")],
        },
        &RequestBudgetV1::unbounded(),
    )?;
    let published = super::layout::epoch_dir(&root, &generation, AuxEpochV1::new(1));
    let original_bytes = super::layout::tree_bytes(&published)?;
    factory
        .io
        .fail_after_delete_once
        .store(true, Ordering::SeqCst);
    if !matches!(
        adapter.discard_epoch(&generation, AuxEpochV1::new(1), None),
        Err(CoreError::Storage(_))
    ) || published.exists()
    {
        return Err("injected completed unlink did not retain a failed receipt".into());
    }
    if adapter.discard_epoch(&generation, AuxEpochV1::new(1), None)?
        != (quanta_index_core::HistoryTextDiscardOutcomeV1::Discarded {
            bytes: original_bytes,
        })
        || adapter.discard_epoch(&generation, AuxEpochV1::new(1), None)?
            != quanta_index_core::HistoryTextDiscardOutcomeV1::Absent
    {
        return Err("post-unlink replay lost the original bytes or repeated deletion".into());
    }
    Ok(())
}

#[test]
fn staging_delete_reconciles_before_new_publication_allowance() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("qualified");
    let factory = fixture_factory(root.clone(), false);
    let adapter = HistoryTextIndexAdapter::with_file_admission(root.clone(), factory.clone())?;
    let generation = AuxiliaryGenerationKeyV1 {
        repo_id: RepoId::new("repo")?,
        revision_id: RevisionId::new("rev")?,
        generation: ManifestGeneration::new(3),
    };
    let staging = super::layout::staging_dir(&root, &generation, AuxEpochV1::new(1));
    fs::create_dir_all(&staging)?;
    fs::write(staging.join("old"), b"old")?;
    let build = HistoryTextBuildV1::Full {
        docs: vec![document(1, "needle")],
    };
    factory
        .io
        .fail_after_delete_once
        .store(true, Ordering::SeqCst);
    if !matches!(
        adapter.publish_epoch(
            &generation,
            AuxEpochV1::new(1),
            build.clone(),
            &RequestBudgetV1::unbounded()
        ),
        Err(CoreError::Storage(_))
    ) || staging.exists()
    {
        return Err("completed staging unlink did not retain its pending receipt".into());
    }
    let writes = factory.io.writes.load(Ordering::SeqCst);
    let publication_calls = factory.publications.load(Ordering::SeqCst);
    let cancelled_retry = RequestBudgetV1::unbounded();
    cancelled_retry.cancel_handle().cancel();
    if !matches!(
        adapter.publish_epoch(
            &generation,
            AuxEpochV1::new(1),
            build.clone(),
            &cancelled_retry,
        ),
        Err(CoreError::Typed { code, .. }) if code == quanta_index_core::REQUEST_CANCELLED_CODE
    ) || factory
        .io
        .pending_delete
        .lock()
        .map_err(|error| error.to_string())?
        .is_some()
        || factory.io.writes.load(Ordering::SeqCst) != writes
        || factory.publications.load(Ordering::SeqCst) != publication_calls
    {
        return Err(
            "cancelled retry did not settle the original staging deletion before refusal".into(),
        );
    }
    factory.refuse_publication.store(true, Ordering::SeqCst);
    if !matches!(
        adapter.publish_epoch(
            &generation,
            AuxEpochV1::new(1),
            build.clone(),
            &RequestBudgetV1::unbounded()
        ),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        })
    ) || factory
        .io
        .pending_delete
        .lock()
        .map_err(|error| error.to_string())?
        .is_some()
        || factory.io.writes.load(Ordering::SeqCst) != writes
    {
        return Err("pending staging unlink required a new publication parent".into());
    }
    factory.refuse_publication.store(false, Ordering::SeqCst);
    if adapter
        .publish_epoch(
            &generation,
            AuxEpochV1::new(1),
            build,
            &RequestBudgetV1::unbounded(),
        )?
        .docs_written
        != 1
    {
        return Err("reconciled staging did not permit the original new build".into());
    }
    Ok(())
}
