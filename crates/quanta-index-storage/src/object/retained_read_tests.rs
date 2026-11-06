//! Existing-object batch replay must retain its admitted read owner through durability.

use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _, symlink},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_core::{
    CancelHandleV1,
    index::{
        IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
        ObjectReadFile, ObjectReadIoPort, ObjectWriteIoPort, ObjectWriteTicket, RegisteredObject,
        WorkBudget, WorkLimits,
    },
};

use super::{FileObjectStore, ObjectWriteSink, PreparedObject, nofollow_flag, storage_error};

macro_rules! check {
    ($condition:expr, $message:literal $(,)?) => {
        if !($condition) {
            return Err(IndexError::BackendViolation($message.into()));
        }
    };
    ($condition:expr $(,)?) => {
        if !($condition) {
            return Err(IndexError::BackendViolation(format!(
                "check failed: {}",
                stringify!($condition)
            )));
        }
    };
}

macro_rules! check_eq {
    ($actual:expr, $expected:expr $(,)?) => {{
        let actual = &$actual;
        let expected = &$expected;
        if actual != expected {
            return Err(IndexError::BackendViolation(format!(
                "{} != {}: {actual:?} != {expected:?}",
                stringify!($actual),
                stringify!($expected)
            )));
        }
    }};
}

fn budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

struct Sink {
    expected: usize,
    reserves: AtomicU64,
    confirms: AtomicU64,
}

impl Default for Sink {
    fn default() -> Self {
        Self {
            expected: 1,
            reserves: AtomicU64::new(0),
            confirms: AtomicU64::new(0),
        }
    }
}

impl ObjectWriteSink for Sink {
    fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected scalar reserve".into(),
        ))
    }

    fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected scalar confirm".into(),
        ))
    }

    fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        check_eq!(objects.len(), self.expected);
        let _prior = self.reserves.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        check_eq!(objects.len(), self.expected);
        let _prior = self.confirms.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Debug, Default)]
struct MemoryCounter(AtomicU64);

impl MemoryLeaseOwner for MemoryCounter {
    fn release(&self, bytes: u64) {
        let prior = self.0.fetch_sub(bytes, Ordering::SeqCst);
        assert!(prior >= bytes);
    }
}

struct CountedMemory(Arc<MemoryCounter>);

impl MemoryScopePort for CountedMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _prior = self.0.0.fetch_add(bytes, Ordering::SeqCst);
        let counter = Arc::clone(&self.0);
        let owner: Arc<dyn MemoryLeaseOwner> = counter;
        Ok(MemoryLease::new(bytes, owner))
    }
}

#[derive(Debug)]
enum AfterRead {
    None,
    ReplaceSameWire,
    ReplaceWithSymlink,
    Cancel(CancelHandleV1),
    DenyOpen,
}

#[derive(Debug)]
struct Probe {
    path: PathBuf,
    wire: Vec<u8>,
    action: AfterRead,
    fired: AtomicBool,
    opens: AtomicU64,
    live: AtomicU64,
    drops: AtomicU64,
    first_file: Mutex<Option<(u64, u64)>>,
    memory: Arc<MemoryCounter>,
}

impl Probe {
    fn new(
        path: PathBuf,
        wire: Vec<u8>,
        action: AfterRead,
        memory: Arc<MemoryCounter>,
    ) -> Arc<Self> {
        Arc::new(Self {
            path,
            wire,
            action,
            fired: AtomicBool::new(false),
            opens: AtomicU64::new(0),
            live: AtomicU64::new(0),
            drops: AtomicU64::new(0),
            first_file: Mutex::new(None),
            memory,
        })
    }

    fn fire(&self) -> Result<(), IndexError> {
        if self.fired.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        match &self.action {
            AfterRead::None | AfterRead::DenyOpen => Ok(()),
            AfterRead::Cancel(handle) => {
                handle.cancel();
                Ok(())
            }
            AfterRead::ReplaceSameWire | AfterRead::ReplaceWithSymlink => {
                let replacement = self.path.with_extension("replacement");
                fs::write(&replacement, &self.wire).map_err(|error| storage_error(&error))?;
                fs::set_permissions(&replacement, Permissions::from_mode(0o600))
                    .map_err(|error| storage_error(&error))?;
                if matches!(&self.action, AfterRead::ReplaceSameWire) {
                    fs::rename(&replacement, &self.path).map_err(|error| storage_error(&error))?;
                } else {
                    fs::remove_file(&self.path).map_err(|error| storage_error(&error))?;
                    symlink(&replacement, &self.path).map_err(|error| storage_error(&error))?;
                }
                Ok(())
            }
        }
    }

    fn first_identity(&self) -> Result<(u64, u64), IndexError> {
        self.first_file
            .lock()
            .map_err(|_error| IndexError::Conflict)?
            .ok_or_else(|| IndexError::BackendViolation("read owner was not opened".into()))
    }
}

#[derive(Debug)]
struct Reads(Vec<Arc<Probe>>);

#[derive(Debug, Default)]
struct LosingWriterState {
    began: AtomicU64,
    created: AtomicU64,
    cancelled: AtomicU64,
    recovered: AtomicU64,
    winner_identity: Mutex<Option<(u64, u64)>>,
    target: Mutex<Option<PathBuf>>,
    temporary: Mutex<Option<PathBuf>>,
}

#[derive(Debug)]
struct LosingWriterPort {
    wire: Vec<u8>,
    state: Arc<LosingWriterState>,
}

struct LosingWriterTicket {
    target: PathBuf,
    temporary: PathBuf,
    wire: Vec<u8>,
    state: Arc<LosingWriterState>,
    pin: Option<File>,
}

impl ObjectWriteIoPort for LosingWriterPort {
    fn recover_retired_visible_create(
        &self,
        target: &Path,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.state.cancelled.load(Ordering::SeqCst) != 1
            || self
                .state
                .target
                .lock()
                .map_err(|_error| IndexError::Conflict)?
                .as_deref()
                != Some(target)
        {
            return Err(IndexError::BackendViolation(
                "recovery did not follow the declared loser cancellation".into(),
            ));
        }
        let winner_identity = self
            .state
            .winner_identity
            .lock()
            .map_err(|_error| IndexError::Conflict)?
            .ok_or_else(|| IndexError::BackendViolation("winner inode not recorded".into()))?;
        let linked = fs::symlink_metadata(target).map_err(|error| storage_error(&error))?;
        if !linked.file_type().is_file()
            || (linked.dev(), linked.ino()) != winner_identity
            || linked.mode() & 0o7777 != 0o600
            || linked.nlink() != 1
        {
            return Err(IndexError::BackendViolation(
                "recovery target differs from the matching winner".into(),
            ));
        }
        let prior = self.state.recovered.fetch_add(1, Ordering::SeqCst);
        if prior != 0 {
            return Err(IndexError::BackendViolation(
                "visible-create recovery ran more than once".into(),
            ));
        }
        Ok(())
    }

    fn begin(
        &self,
        target: &Path,
        temporary: &Path,
        _max_bytes: u64,
        _budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectWriteTicket>, IndexError> {
        if target.exists() {
            return Err(IndexError::BackendViolation(
                "loser test did not start from an absent final name".into(),
            ));
        }
        let _prior = self.state.began.fetch_add(1, Ordering::SeqCst);
        *self
            .state
            .target
            .lock()
            .map_err(|_error| IndexError::Conflict)? = Some(target.to_path_buf());
        *self
            .state
            .temporary
            .lock()
            .map_err(|_error| IndexError::Conflict)? = Some(temporary.to_path_buf());
        Ok(Box::new(LosingWriterTicket {
            target: target.to_path_buf(),
            temporary: temporary.to_path_buf(),
            wire: self.wire.clone(),
            state: Arc::clone(&self.state),
            pin: None,
        }))
    }
}

impl ObjectWriteTicket for LosingWriterTicket {
    fn creation_started(&mut self) -> Result<(), IndexError> {
        Ok(())
    }

    fn created(&mut self, writer: &File) -> Result<(), IndexError> {
        self.pin = Some(writer.try_clone().map_err(|error| storage_error(&error))?);
        // The store has already created its private temporary output. Publish
        // a matching winner at the canonical name before its no-replace rename.
        let mut winner = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(nofollow_flag()?)
            .open(&self.target)
            .map_err(|error| storage_error(&error))?;
        winner
            .write_all(&self.wire)
            .map_err(|error| storage_error(&error))?;
        winner.sync_all().map_err(|error| storage_error(&error))?;
        let metadata = winner.metadata().map_err(|error| storage_error(&error))?;
        *self
            .state
            .winner_identity
            .lock()
            .map_err(|_error| IndexError::Conflict)? = Some((metadata.dev(), metadata.ino()));
        let _prior = self.state.created.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn uncreated(&mut self) -> Result<(), IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected uncreated writer".into(),
        ))
    }

    fn retire_writer(&mut self, writer: File) -> Result<(), IndexError> {
        drop(writer);
        Err(IndexError::BackendViolation(
            "losing writer unexpectedly published".into(),
        ))
    }

    fn cancel(&mut self, writer: File) -> Result<(), IndexError> {
        drop(writer);
        drop(self.pin.take());
        fs::remove_file(&self.temporary).map_err(|error| storage_error(&error))?;
        let parent = self
            .temporary
            .parent()
            .ok_or_else(|| IndexError::BackendViolation("temporary has no parent".into()))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| storage_error(&error))?;
        let _prior = self.state.cancelled.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn synced(self: Box<Self>) -> Result<(), IndexError> {
        Err(IndexError::BackendViolation(
            "losing writer unexpectedly settled".into(),
        ))
    }
}

struct Held {
    file: Option<File>,
    memory: Option<MemoryLease>,
    probe: Arc<Probe>,
}

impl ObjectReadFile for Held {
    fn file(&self) -> Result<&File, IndexError> {
        let file = self
            .file
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("read owner was already closed".into()))?;
        // The first borrow precedes the wire read. A later borrow at EOF is an
        // externally observable post-read point, independent of helper names.
        let offset = rustix::fs::tell(file)
            .map_err(|error| IndexError::Storage(format!("read offset: {error}")))?;
        let wire_len =
            u64::try_from(self.probe.wire.len()).map_err(|_error| IndexError::ResourceLimited)?;
        if offset >= wire_len {
            self.probe.fire()?;
        }
        Ok(file)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        drop(self.file.take());
        drop(self.memory.take());
        let prior = self.probe.live.fetch_sub(1, Ordering::SeqCst);
        assert!(prior > 0);
        let _prior = self.probe.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl ObjectReadIoPort for Reads {
    fn open(
        &self,
        path: &Path,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        let probe = self
            .0
            .iter()
            .find(|candidate| candidate.path == path)
            .ok_or_else(|| IndexError::BackendViolation("unexpected read path".into()))?;
        let previous_opens = probe.opens.fetch_add(1, Ordering::SeqCst);
        // The pre-repair implementation dropped the validated owner and opened
        // again. Inject at that attempted reopen as well as the retained path.
        if previous_opens > 0 {
            probe.fire()?;
        }
        budget.checkpoint()?;
        if matches!(&probe.action, AfterRead::DenyOpen) {
            return Err(IndexError::ResourceLimited);
        }
        let memory = budget.reserve_memory(64, MemoryLifetime::Temporary)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(nofollow_flag()?)
            .open(path)
            .map_err(|error| storage_error(&error))?;
        let metadata = file.metadata().map_err(|error| storage_error(&error))?;
        let mut first = probe
            .first_file
            .lock()
            .map_err(|_error| IndexError::Conflict)?;
        if first.is_none() {
            *first = Some((metadata.dev(), metadata.ino()));
        }
        drop(first);
        let _prior = probe.live.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Held {
            file: Some(file),
            memory,
            probe: Arc::clone(probe),
        }))
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    store: FileObjectStore,
    object: PreparedObject,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self, IndexError> {
        let root = tempfile::tempdir().map_err(|error| storage_error(&error))?;
        fs::set_permissions(root.path(), Permissions::from_mode(0o700))
            .map_err(|error| storage_error(&error))?;
        let store = FileObjectStore::open(root.path())?;
        let mut work = budget()?;
        let object = store.prepare_object(b"existing immutable object", &[], &mut work)?;
        let path = store.path_for(object.registration.reference);
        store.install_prepared_batch_with_barrier(
            &[&object],
            &Sink::default(),
            &mut work,
            |_file| Ok(()),
        )?;
        Ok(Self {
            _root: root,
            store,
            object,
            path,
        })
    }

    fn attach(&mut self, work: &mut WorkBudget, action: AfterRead) -> Arc<Probe> {
        let probe = Probe::new(
            self.path.clone(),
            self.object.wire.clone(),
            action,
            Arc::new(MemoryCounter::default()),
        );
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedMemory(Arc::clone(
            &probe.memory,
        ))))));
        self.store.read_io = Some(Arc::new(Reads(vec![Arc::clone(&probe)])));
        probe
    }

    fn replay(
        &self,
        work: &mut WorkBudget,
        sink: &Sink,
        barrier: impl FnOnce(&File) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        self.store
            .install_prepared_batch_with_barrier(&[&self.object], sink, work, barrier)
    }
}

fn check_closed(probe: &Probe, expected_opens: u64) -> Result<(), IndexError> {
    check_eq!(probe.opens.load(Ordering::SeqCst), expected_opens);
    check_eq!(probe.live.load(Ordering::SeqCst), 0);
    check_eq!(probe.drops.load(Ordering::SeqCst), expected_opens);
    check_eq!(probe.memory.0.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn existing_two_identity_batch_opens_each_admitted_owner_once() -> Result<(), IndexError> {
    let root = tempfile::tempdir().map_err(|error| storage_error(&error))?;
    fs::set_permissions(root.path(), Permissions::from_mode(0o700))
        .map_err(|error| storage_error(&error))?;
    let mut store = FileObjectStore::open(root.path())?;
    let mut preparing = budget()?;
    let first = store.prepare_object(b"first existing identity", &[], &mut preparing)?;
    let second = store.prepare_object(b"second existing identity", &[], &mut preparing)?;
    let seed_sink = Sink {
        expected: 2,
        ..Sink::default()
    };
    store.install_prepared_batch_with_barrier(
        &[&first, &second],
        &seed_sink,
        &mut preparing,
        |_file| Ok(()),
    )?;
    let memory = Arc::new(MemoryCounter::default());
    let first_probe = Probe::new(
        store.path_for(first.registration.reference),
        first.wire.clone(),
        AfterRead::None,
        Arc::clone(&memory),
    );
    let second_probe = Probe::new(
        store.path_for(second.registration.reference),
        second.wire.clone(),
        AfterRead::None,
        Arc::clone(&memory),
    );
    store.read_io = Some(Arc::new(Reads(vec![
        Arc::clone(&first_probe),
        Arc::clone(&second_probe),
    ])));
    let mut work = budget()?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedMemory(Arc::clone(
        &memory,
    ))))));
    let replay_sink = Sink {
        expected: 2,
        ..Sink::default()
    };
    let barrier_calls = AtomicU64::new(0);
    store.install_prepared_batch_with_barrier(
        &[&first, &second],
        &replay_sink,
        &mut work,
        |file| {
            let metadata = file.metadata().map_err(|error| storage_error(&error))?;
            check_eq!(
                (metadata.dev(), metadata.ino()),
                first_probe.first_identity()?
            );
            check_eq!(first_probe.opens.load(Ordering::SeqCst), 1);
            check_eq!(first_probe.live.load(Ordering::SeqCst), 1);
            check_eq!(first_probe.drops.load(Ordering::SeqCst), 0);
            check_eq!(second_probe.opens.load(Ordering::SeqCst), 1);
            check_eq!(second_probe.live.load(Ordering::SeqCst), 0);
            check_eq!(second_probe.drops.load(Ordering::SeqCst), 1);
            check!(memory.0.load(Ordering::SeqCst) >= 64);
            let _prior = barrier_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )?;
    check_eq!(barrier_calls.load(Ordering::SeqCst), 1);
    check_closed(&first_probe, 1)?;
    check_closed(&second_probe, 1)?;
    check_eq!(replay_sink.reserves.load(Ordering::SeqCst), 1);
    check_eq!(replay_sink.confirms.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn concurrent_matching_winner_uses_the_actual_batch_loser_branch_and_retained_read()
-> Result<(), IndexError> {
    for fail_barrier in [false, true] {
        let root = tempfile::tempdir().map_err(|error| storage_error(&error))?;
        fs::set_permissions(root.path(), Permissions::from_mode(0o700))
            .map_err(|error| storage_error(&error))?;
        let mut store = FileObjectStore::open(root.path())?;
        let mut work = budget()?;
        let object = store.prepare_object(b"matching race winner", &[], &mut work)?;
        let path = store.path_for(object.registration.reference);
        let writer_state = Arc::new(LosingWriterState::default());
        store.write_io = Some(Arc::new(LosingWriterPort {
            wire: object.wire.clone(),
            state: Arc::clone(&writer_state),
        }));
        let memory = Arc::new(MemoryCounter::default());
        let probe = Probe::new(
            path.clone(),
            object.wire.clone(),
            AfterRead::None,
            Arc::clone(&memory),
        );
        store.read_io = Some(Arc::new(Reads(vec![Arc::clone(&probe)])));
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(CountedMemory(Arc::clone(
            &memory,
        ))))));
        let sink = Sink::default();
        let barrier_calls = AtomicU64::new(0);
        let result =
            store.install_prepared_batch_with_barrier(&[&object], &sink, &mut work, |file| {
                let metadata = file.metadata().map_err(|error| storage_error(&error))?;
                check_eq!((metadata.dev(), metadata.ino()), probe.first_identity()?);
                check_eq!(probe.live.load(Ordering::SeqCst), 1);
                check!(memory.0.load(Ordering::SeqCst) >= 64);
                let _prior = barrier_calls.fetch_add(1, Ordering::SeqCst);
                if fail_barrier {
                    Err(IndexError::Storage("injected loser barrier refusal".into()))
                } else {
                    Ok(())
                }
            });
        if fail_barrier {
            check!(
                matches!(result, Err(IndexError::Storage(message)) if message == "injected loser barrier refusal")
            );
        } else {
            result?;
        }
        check_eq!(writer_state.began.load(Ordering::SeqCst), 1);
        check_eq!(writer_state.created.load(Ordering::SeqCst), 1);
        check_eq!(writer_state.cancelled.load(Ordering::SeqCst), 1);
        check_eq!(writer_state.recovered.load(Ordering::SeqCst), 1);
        check_eq!(barrier_calls.load(Ordering::SeqCst), 1);
        check_eq!(sink.reserves.load(Ordering::SeqCst), 1);
        check_eq!(
            sink.confirms.load(Ordering::SeqCst),
            u64::from(!fail_barrier)
        );
        check_closed(&probe, 1)?;
        let winner_identity = writer_state
            .winner_identity
            .lock()
            .map_err(|_error| IndexError::Conflict)?
            .ok_or_else(|| IndexError::BackendViolation("winner inode not recorded".into()))?;
        let surviving = fs::symlink_metadata(&path).map_err(|error| storage_error(&error))?;
        check_eq!((surviving.dev(), surviving.ino()), winner_identity);
        check_eq!(
            fs::read(&path).map_err(|error| storage_error(&error))?,
            object.wire
        );
        let temporary = writer_state
            .temporary
            .lock()
            .map_err(|_error| IndexError::Conflict)?
            .clone()
            .ok_or_else(|| IndexError::BackendViolation("loser temporary not recorded".into()))?;
        check!(!temporary.exists());
    }
    Ok(())
}

#[test]
fn existing_batch_uses_one_admitted_read_owner_through_success_and_failed_barrier()
-> Result<(), IndexError> {
    for fail_barrier in [false, true] {
        let mut fixture = Fixture::new()?;
        let mut work = budget()?;
        let probe = fixture.attach(&mut work, AfterRead::None);
        let sink = Sink::default();
        let result = fixture.replay(&mut work, &sink, |file| {
            let metadata = file.metadata().map_err(|error| storage_error(&error))?;
            check_eq!((metadata.dev(), metadata.ino()), probe.first_identity()?);
            check_eq!(probe.live.load(Ordering::SeqCst), 1);
            check!(probe.memory.0.load(Ordering::SeqCst) >= 64);
            if fail_barrier {
                Err(IndexError::Storage("injected barrier refusal".into()))
            } else {
                Ok(())
            }
        });
        if fail_barrier {
            check!(
                matches!(result, Err(IndexError::Storage(message)) if message == "injected barrier refusal")
            );
        } else {
            result?;
        }
        check_eq!(sink.reserves.load(Ordering::SeqCst), 1);
        check_eq!(
            sink.confirms.load(Ordering::SeqCst),
            u64::from(!fail_barrier)
        );
        check_closed(&probe, 1)?;
        check_eq!(
            fs::read(&fixture.path).map_err(|error| storage_error(&error))?,
            fixture.object.wire
        );
    }
    Ok(())
}

#[test]
fn existing_batch_refuses_same_wire_inode_replacement_and_symlink_after_read()
-> Result<(), IndexError> {
    for action in [AfterRead::ReplaceSameWire, AfterRead::ReplaceWithSymlink] {
        let mut fixture = Fixture::new()?;
        let mut work = budget()?;
        let probe = fixture.attach(&mut work, action);
        let sink = Sink::default();
        let barrier_calls = AtomicU64::new(0);
        let result = fixture.replay(&mut work, &sink, |_file| {
            let _prior = barrier_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        check!(result.is_err(), "replacement path must not be confirmed");
        check!(probe.fired.load(Ordering::SeqCst));
        check_eq!(barrier_calls.load(Ordering::SeqCst), 0);
        check_eq!(sink.reserves.load(Ordering::SeqCst), 1);
        check_eq!(sink.confirms.load(Ordering::SeqCst), 0);
        check_closed(&probe, 1)?;
    }
    Ok(())
}

#[test]
fn existing_batch_refuses_wrong_wire_and_read_capacity_denial_without_fallback()
-> Result<(), IndexError> {
    for deny_open in [false, true] {
        let mut fixture = Fixture::new()?;
        let mut work = budget()?;
        let probe = fixture.attach(
            &mut work,
            if deny_open {
                AfterRead::DenyOpen
            } else {
                AfterRead::None
            },
        );
        if !deny_open {
            fs::write(&fixture.path, b"wrong immutable wire")
                .map_err(|error| storage_error(&error))?;
        }
        let sink = Sink::default();
        let barrier_calls = AtomicU64::new(0);
        let result = fixture.replay(&mut work, &sink, |_file| {
            let _prior = barrier_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        if deny_open {
            check!(matches!(result, Err(IndexError::ResourceLimited)));
            check_eq!(probe.drops.load(Ordering::SeqCst), 0);
        } else {
            check!(matches!(result, Err(IndexError::CorruptObject(_))));
            check_eq!(probe.drops.load(Ordering::SeqCst), 1);
        }
        check_eq!(probe.opens.load(Ordering::SeqCst), 1);
        check_eq!(probe.live.load(Ordering::SeqCst), 0);
        check_eq!(probe.memory.0.load(Ordering::SeqCst), 0);
        check_eq!(barrier_calls.load(Ordering::SeqCst), 0);
        check_eq!(sink.reserves.load(Ordering::SeqCst), 1);
        check_eq!(sink.confirms.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[test]
fn existing_batch_cancellation_after_validation_drops_owner_before_confirmation()
-> Result<(), IndexError> {
    let mut fixture = Fixture::new()?;
    let mut work = budget()?;
    let cancel = work.request().cancel_handle();
    let probe = fixture.attach(&mut work, AfterRead::Cancel(cancel));
    let sink = Sink::default();
    let barrier_calls = AtomicU64::new(0);
    let result = fixture.replay(&mut work, &sink, |_file| {
        let _prior = barrier_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    check!(matches!(result, Err(IndexError::Cancelled)));
    check!(probe.fired.load(Ordering::SeqCst));
    check_eq!(barrier_calls.load(Ordering::SeqCst), 0);
    check_eq!(sink.reserves.load(Ordering::SeqCst), 1);
    check_eq!(sink.confirms.load(Ordering::SeqCst), 0);
    check_closed(&probe, 1)?;
    Ok(())
}
