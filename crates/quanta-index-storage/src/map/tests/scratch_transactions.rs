//! One process-global VFS fixture for real disposable scratch COMMIT refusals.
//!
//! This standalone allocation tests `SQLite` error/custody ordering, not the
//! production managed physical authority or its aggregate disk admission.

use std::{
    collections::BTreeSet,
    ffi::CString,
    fmt,
    num::NonZeroU64,
    os::unix::ffi::OsStrExt as _,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use quanta_index_core::index::{
    ScratchAllocation, ScratchIoBinding, ScratchIoScope, ScratchNamespaceLease, ScratchRequest,
    ScratchSpacePort,
};
use rusqlite::ffi::quanta_capacity_vfs::{
    self, Admission, Event, EventKind, Refusal, RegisteredVfs, ScopeGuard,
};

use super::*;

const DRIVER: &str = "quanta-map-tx-test";
const SCOPE: NonZeroU64 = NonZeroU64::MIN;

#[derive(Default)]
struct FaultState {
    paths: Mutex<BTreeSet<Vec<u8>>>,
    opened_handles: Mutex<BTreeSet<u64>>,
    armed: AtomicBool,
    refused_writes: AtomicU64,
    foreign_events: AtomicU64,
}

impl FaultState {
    fn admit_path(&self, path: &Path) -> Result<(), IndexError> {
        let bytes = path.as_os_str().as_bytes().to_vec();
        let inserted = self
            .paths
            .lock()
            .map_err(|_error| IndexError::Storage("test VFS path lock poisoned".into()))?
            .insert(bytes);
        if !inserted {
            return Err(IndexError::BackendViolation(
                "test VFS scratch path was registered twice".into(),
            ));
        }
        Ok(())
    }

    fn arm(&self) -> Result<(), IndexError> {
        if self.armed.swap(true, Ordering::SeqCst) {
            return Err(IndexError::BackendViolation(
                "test VFS write refusal was already armed".into(),
            ));
        }
        Ok(())
    }

    fn disarm(&self) {
        self.armed.store(false, Ordering::SeqCst);
    }

    fn paths(&self) -> Result<Vec<Vec<u8>>, IndexError> {
        Ok(self
            .paths
            .lock()
            .map_err(|_error| IndexError::Storage("test VFS path lock poisoned".into()))?
            .iter()
            .cloned()
            .collect())
    }
}

struct CommitFaultAdmission(Arc<FaultState>);

impl Admission for CommitFaultAdmission {
    fn on_event(&self, event: Event<'_>) -> Result<Option<CString>, Refusal> {
        if event.scope_id != SCOPE {
            let _previous = self.0.foreign_events.fetch_add(1, Ordering::SeqCst);
            return Err(Refusal::Io);
        }
        let path_is_owned = event.path.is_some_and(|path| {
            self.0
                .paths
                .lock()
                .is_ok_and(|paths| paths.contains(path.to_bytes()))
        });
        let authorized = match event.kind {
            EventKind::BeforeOpen | EventKind::BeforeNativeOpen | EventKind::OpenAbortedNoFd => {
                path_is_owned
            }
            EventKind::AfterOpen if path_is_owned => {
                if event.sqlite_result == 0 {
                    self.0
                        .opened_handles
                        .lock()
                        .is_ok_and(|mut handles| handles.insert(event.handle_id))
                } else {
                    true
                }
            }
            EventKind::FdRetired if event.operation == 0 => self
                .0
                .opened_handles
                .lock()
                .is_ok_and(|mut handles| handles.remove(&event.handle_id)),
            EventKind::AfterClose if path_is_owned => {
                self.0.opened_handles.lock().is_ok_and(|mut handles| {
                    if event.fd_retired {
                        handles.remove(&event.handle_id)
                    } else {
                        handles.contains(&event.handle_id)
                    }
                })
            }
            EventKind::AfterOpen
            | EventKind::BeforeWrite
            | EventKind::AfterWrite
            | EventKind::BeforeTruncate
            | EventKind::AfterTruncate
            | EventKind::BeforeSync
            | EventKind::AfterSync
            | EventKind::BeforeShmMap
            | EventKind::AfterShmMap
            | EventKind::AfterShmUnmap
            | EventKind::BeforeDelete
            | EventKind::AfterDelete
            | EventKind::AfterClose
            | EventKind::BeforeFileControl
            | EventKind::AfterFileControl
            | EventKind::FdRetired
            | EventKind::BeforeVerificationRead => {
                path_is_owned
                    && self
                        .0
                        .opened_handles
                        .lock()
                        .is_ok_and(|handles| handles.contains(&event.handle_id))
            }
        };
        if !authorized {
            let _previous = self.0.foreign_events.fetch_add(1, Ordering::SeqCst);
            return Err(Refusal::Io);
        }
        if event.kind == EventKind::BeforeWrite && self.0.armed.swap(false, Ordering::SeqCst) {
            let _previous = self.0.refused_writes.fetch_add(1, Ordering::SeqCst);
            return Err(Refusal::Io);
        }
        Ok(None)
    }
}

struct TestScope(ScopeGuard);

impl ScratchIoScope for TestScope {
    fn is_capacity_refusal(&self, code: i32) -> bool {
        self.0.is_capacity_extended_code(code)
    }
}

struct FaultBinding {
    vfs: &'static RegisteredVfs,
}

impl fmt::Debug for FaultBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FaultBinding")
            .finish_non_exhaustive()
    }
}

impl ScratchIoBinding for FaultBinding {
    fn driver_name(&self) -> &str {
        DRIVER
    }

    fn enter_scope(&self) -> Box<dyn ScratchIoScope> {
        Box::new(TestScope(self.vfs.enter_scope(SCOPE)))
    }

    fn acquire_namespace(
        &self,
        _path: &Path,
    ) -> Result<Box<dyn ScratchNamespaceLease>, IndexError> {
        Err(IndexError::BackendViolation(
            "standalone test allocation requested managed namespace".into(),
        ))
    }

    fn retire_namespace(
        &self,
        _path: &Path,
        _budget: Option<&mut WorkBudget>,
    ) -> Result<(), IndexError> {
        Err(IndexError::BackendViolation(
            "standalone test allocation requested managed retirement".into(),
        ))
    }
}

struct FaultAllocation {
    inner: Box<dyn ScratchAllocation>,
    binding: Arc<dyn ScratchIoBinding>,
}

impl ScratchAllocation for FaultAllocation {
    fn path(&self) -> &Path {
        self.inner.path()
    }

    fn max_bytes(&self) -> u64 {
        self.inner.max_bytes()
    }

    fn reserved_bytes(&self) -> Result<u64, IndexError> {
        self.inner.reserved_bytes()
    }

    fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
        self.inner.reserve_at_least(bytes, budget)
    }

    fn io_binding(&self) -> Option<Arc<dyn ScratchIoBinding>> {
        Some(Arc::clone(&self.binding))
    }
}

struct FaultSpace {
    inner: crate::FileScratchSpace,
    binding: Arc<FaultBinding>,
    state: Arc<FaultState>,
}

impl ScratchSpacePort for FaultSpace {
    fn allocate(
        &self,
        request: ScratchRequest,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
        let inner = self.inner.allocate(request, budget)?;
        self.state
            .admit_path(&inner.path().join("scratch.sqlite"))?;
        let binding: Arc<dyn ScratchIoBinding> = self.binding.clone();
        Ok(Box::new(FaultAllocation { inner, binding }))
    }
}

#[test]
fn real_commit_io_refusal_precedes_physical_install_and_discards_scratch()
-> Result<(), Box<dyn std::error::Error>> {
    // quanta_capacity_vfs::register is once per process. Keep all cases here.
    let state = Arc::new(FaultState::default());
    let vfs = quanta_capacity_vfs::register(
        CString::new(DRIVER)?,
        64,
        Box::new(CommitFaultAdmission(Arc::clone(&state))),
    )
    .map_err(|error| assertion_error(format!("register test VFS: {error:?}")))?;
    check_eq!(vfs.name().to_bytes(), DRIVER.as_bytes());
    let directory = tempfile::tempdir()?;
    let space = crate::FileScratchSpace::open_standalone(
        directory.path().join("scratch"),
        4 * 1024 * 1024,
    )?;
    let fault_space: Arc<dyn ScratchSpacePort> = Arc::new(FaultSpace {
        inner: space,
        binding: Arc::new(FaultBinding { vfs }),
        state: Arc::clone(&state),
    });
    let store = FileObjectStore::open_with_scratch(directory.path().join("objects"), fault_space)?;
    let sink = RecordingSink::default();
    let mut work = budget()?;

    // One tiny admitted row remains dirty until a real SQLite COMMIT write.
    let scratch = MapScratch::open(&store, &mut work)?;
    scratch.begin_batch(&work)?;
    scratch.insert_edit(
        &OrdinalMapEdit::unscoped(
            0,
            MapEdit {
                key: b"commit-fault".to_vec(),
                expected: None,
                next: None,
            },
        ),
        &mut work,
    )?;
    let before = state.refused_writes.load(Ordering::SeqCst);
    state.arm()?;
    let failed = scratch.commit_batch(&work);
    state.disarm();
    check!(matches!(failed, Err(IndexError::Storage(_))));
    check_eq!(
        state.refused_writes.load(Ordering::SeqCst),
        before
            .checked_add(1)
            .ok_or_else(|| assertion_error("test refusal counter overflow".into()))?
    );
    check_eq!(sink.1.load(Ordering::SeqCst), 0);
    check_eq!(sink.2.load(Ordering::SeqCst), 0);
    drop(scratch); // journal_mode=OFF forbids rollback-and-reuse.
    vfs.drain_retirements()
        .map_err(|error| assertion_error(format!("drain test VFS: {error:?}")))?;

    // A successful earlier group survives a later group's actual COMMIT failure.
    let first = store.prepare_object(b"first committed group", &[], &mut work)?;
    let first_reference = first.registration.reference;
    let second = store.prepare_object(b"second final group", &[], &mut work)?;
    let second_reference = second.registration.reference;
    let third = store.prepare_object(b"third refused group", &[], &mut work)?;
    let third_reference = third.registration.reference;
    let scratch = MapScratch::open(&store, &mut work)?;
    scratch.begin_batch(&work)?;
    scratch.set_mark(first_reference, 1, &mut work)?;
    // Exercise the commit-only primitive with an already committed next mark.
    // The installer separately completes its own admitted group before commit.
    scratch.set_mark(second_reference, 2, &mut work)?;
    let mut first_chunk = MapBuffer::new(MAX_INSTALL_BATCH_INPUTS);
    first_chunk.push(first, &work)?;
    flush_reachable_chunk(
        &store,
        &mut first_chunk,
        &sink,
        &mut work,
        |_objects, budget| scratch.commit_batch(budget),
    )?;
    check!(first_chunk.is_empty());
    check_eq!(sink.1.load(Ordering::SeqCst), 1);
    check_eq!(sink.2.load(Ordering::SeqCst), 1);
    check!(store.path_for(first_reference).is_file());

    // A no-active commit remains a true no-op for the primitive, even if a
    // subsequent explicit flush has not introduced new traversal marks.
    let mut second_chunk = MapBuffer::new(MAX_INSTALL_BATCH_INPUTS);
    second_chunk.push(second, &work)?;
    let committed = state.refused_writes.load(Ordering::SeqCst);
    state.arm()?;
    flush_reachable_chunk(
        &store,
        &mut second_chunk,
        &sink,
        &mut work,
        |_objects, budget| scratch.commit_batch(budget),
    )?;
    check!(state.armed.load(Ordering::SeqCst));
    check_eq!(state.refused_writes.load(Ordering::SeqCst), committed);
    check!(second_chunk.is_empty());
    check_eq!(sink.1.load(Ordering::SeqCst), 2);
    check_eq!(sink.2.load(Ordering::SeqCst), 2);
    check!(store.path_for(second_reference).is_file());
    state.disarm();

    scratch.begin_batch(&work)?;
    scratch.set_mark(third_reference, 1, &mut work)?;
    let mut third_chunk = MapBuffer::new(MAX_INSTALL_BATCH_INPUTS);
    third_chunk.push(third, &work)?;
    let before = state.refused_writes.load(Ordering::SeqCst);
    state.arm()?;
    let refused = flush_reachable_chunk(
        &store,
        &mut third_chunk,
        &sink,
        &mut work,
        |_objects, budget| scratch.commit_batch(budget),
    );
    state.disarm();
    check!(matches!(refused, Err(IndexError::Storage(_))));
    check_eq!(
        state.refused_writes.load(Ordering::SeqCst),
        before
            .checked_add(1)
            .ok_or_else(|| assertion_error("test refusal counter overflow".into()))?
    );
    check_eq!(third_chunk.len(), 1);
    check_eq!(sink.1.load(Ordering::SeqCst), 2);
    check_eq!(sink.2.load(Ordering::SeqCst), 2);
    check!(store.path_for(first_reference).is_file());
    check!(store.path_for(second_reference).is_file());
    check!(!store.path_for(third_reference).exists());
    drop(scratch);
    vfs.drain_retirements()
        .map_err(|error| assertion_error(format!("drain test VFS: {error:?}")))?;

    for path in state.paths()? {
        let database = Path::new(std::ffi::OsStr::from_bytes(&path));
        let allocation = database
            .parent()
            .ok_or_else(|| assertion_error("missing scratch allocation parent".into()))?;
        check!(
            !allocation.exists(),
            "failed scratch allocation survived close and cleanup"
        );
    }
    check!(
        state
            .opened_handles
            .lock()
            .map_err(|_error| assertion_error("test VFS handle lock poisoned".into()))?
            .is_empty()
    );
    check_eq!(state.foreign_events.load(Ordering::SeqCst), 0);
    Ok(())
}
