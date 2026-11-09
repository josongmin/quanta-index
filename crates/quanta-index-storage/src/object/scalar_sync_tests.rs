//! Managed scalar durability and physical-ticket regression tests.

use super::*;
use std::{fs::File, path::Path};

#[derive(Debug, Default)]
struct ScalarTicketState {
    pending: Mutex<Vec<File>>,
    settled: AtomicU64,
}

#[derive(Debug)]
struct ScalarWriteIo(Arc<ScalarTicketState>);

struct ScalarTicket {
    state: Arc<ScalarTicketState>,
    pin: Option<File>,
    retired: bool,
    settled: bool,
}

impl quanta_index_core::index::ObjectWriteIoPort for ScalarWriteIo {
    fn recover_retired_visible_create(
        &self,
        _target: &Path,
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        Ok(())
    }

    fn begin(
        &self,
        _target: &Path,
        _temporary: &Path,
        _max_bytes: u64,
        _budget: &mut WorkBudget,
    ) -> Result<Box<dyn quanta_index_core::index::ObjectWriteTicket>, IndexError> {
        Ok(Box::new(ScalarTicket {
            state: Arc::clone(&self.0),
            pin: None,
            retired: false,
            settled: false,
        }))
    }
}

impl quanta_index_core::index::ObjectWriteTicket for ScalarTicket {
    fn creation_started(&mut self) -> Result<(), IndexError> {
        Ok(())
    }

    fn created(&mut self, writer: &File) -> Result<(), IndexError> {
        self.pin = Some(writer.try_clone().map_err(|error| storage_error(&error))?);
        Ok(())
    }

    fn uncreated(&mut self) -> Result<(), IndexError> {
        self.settled = true;
        Ok(())
    }

    fn retire_writer(&mut self, writer: File) -> Result<(), IndexError> {
        drop(writer);
        self.retired = true;
        Ok(())
    }

    fn cancel(&mut self, writer: File) -> Result<(), IndexError> {
        drop(writer);
        self.pin = None;
        self.settled = true;
        Ok(())
    }

    fn synced(
        mut self: Box<Self>,
        _directories: quanta_index_core::index::ObjectWriteSyncDirectories,
    ) -> Result<(), IndexError> {
        self.pin = None;
        self.settled = true;
        let _prior = self.state.settled.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for ScalarTicket {
    fn drop(&mut self) {
        if self.retired
            && !self.settled
            && let Some(pin) = self.pin.take()
            && let Ok(mut pending) = self.state.pending.lock()
        {
            pending.push(pin);
        }
    }
}

struct ScalarFixture {
    store: FileObjectStore,
    ticket: Arc<ScalarTicketState>,
    _directory: tempfile::TempDir,
    observer: Arc<ObjectIoObservation>,
    sink: ManagedSink,
    registration: RegisteredObject,
}

impl ScalarFixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let scratch = Arc::new(FileScratchSpace::open_standalone(
            directory.path().join("scratch"),
            1024 * 1024,
        )?);
        let authority: Arc<dyn ManagedObjectAuthority> = Arc::new(TraceAuthority {
            fail_confirmation: false,
            inputs: 1,
            pair: None,
        });
        let ticket = Arc::new(ScalarTicketState::default());
        let observer = Arc::new(ObjectIoObservation::default());
        let store = FileObjectStore::open_managed_with_scratch(
            directory.path().join("store"),
            scratch,
            authority,
        )?
        .with_write_io(Arc::new(ScalarWriteIo(Arc::clone(&ticket))))?
        .with_io_observation(Arc::clone(&observer));
        let mut identity_budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        let registration = store.identify(b"scalar-sync", &[], &mut identity_budget)?;
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let sink = ManagedSink(ObjectInstallScope::Bootstrap(BootstrapRef {
            workspace,
            epoch: 1,
            fence: 1,
            profile: registration.reference,
        }));
        Ok(Self {
            store,
            ticket,
            _directory: directory,
            observer,
            sink,
            registration,
        })
    }

    fn budget(&self) -> Result<WorkBudget, IndexError> {
        let bytes = self
            .registration
            .bytes
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes,
                mutations: 100,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        ))
    }
}

#[test]
fn managed_new_scalar_uses_one_barrier_with_original_exact_byte_budget()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = ScalarFixture::new()?;
    let before = fixture.observer.snapshot()?;
    let mut budget = fixture.budget()?;
    let installed = fixture
        .store
        .install(b"scalar-sync", &[], &fixture.sink, &mut budget)?;
    if installed != fixture.registration
        || budget.usage()
            != (WorkUsage {
                reads: 1,
                writes: 2,
                bytes: fixture
                    .registration
                    .bytes
                    .checked_mul(2)
                    .ok_or("byte overflow")?,
                mutations: 0,
            })
    {
        return Err("managed scalar changed identity or original work billing".into());
    }
    let delta = fixture.observer.snapshot()?.checked_since(&before)?;
    if (
        delta.file_sync_calls,
        delta.directory_sync_calls,
        delta.barrier_calls,
    ) != (1, 2, 1)
        || (delta.reserve_calls, delta.confirm_calls) != (1, 1)
        || fixture.ticket.settled.load(Ordering::Relaxed) != 1
        || !fixture
            .ticket
            .pending
            .lock()
            .map_err(|_poison| "scalar ticket debt poisoned")?
            .is_empty()
    {
        return Err("managed scalar lost one-barrier custody order".into());
    }

    let replay_before = fixture.observer.snapshot()?;
    let mut replay_budget = WorkBudget::with_request(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let replay = fixture
        .store
        .install(b"scalar-sync", &[], &fixture.sink, &mut replay_budget)?;
    let replay_delta = fixture.observer.snapshot()?.checked_since(&replay_before)?;
    if replay != fixture.registration
        || (
            replay_delta.file_sync_calls,
            replay_delta.directory_sync_calls,
            replay_delta.barrier_calls,
        ) != (0, 2, 0)
        || replay_delta.confirm_calls != 1
        || fixture.ticket.settled.load(Ordering::Relaxed) != 1
    {
        return Err("managed scalar replay changed full-sync semantics".into());
    }
    Ok(())
}

#[test]
fn managed_scalar_barrier_failure_retains_ticket_without_confirmation()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = ScalarFixture::new()?;
    let before = fixture.observer.snapshot()?;
    let mut budget = fixture.budget()?;
    let final_path = fixture.store.path_for(fixture.registration.reference);
    let result = fixture.store.put_inner_with_barrier(
        b"scalar-sync",
        &[],
        Some(&fixture.sink),
        &mut budget,
        |file| {
            if !file
                .metadata()
                .map_err(|error| storage_error(&error))?
                .is_file()
                || !final_path.exists()
            {
                return Err(IndexError::BackendViolation(
                    "barrier reached before published file".into(),
                ));
            }
            Err(IndexError::Storage(
                "injected scalar full barrier failure".into(),
            ))
        },
    );
    if !matches!(result, Err(IndexError::Storage(ref message)) if message == "injected scalar full barrier failure")
    {
        return Err("scalar barrier failure did not reach the injected post-publish point".into());
    }
    let delta = fixture.observer.snapshot()?.checked_since(&before)?;
    if (
        delta.reserve_calls,
        delta.confirm_calls,
        delta.file_sync_calls,
        delta.directory_sync_calls,
        delta.barrier_calls,
        delta.failed_barriers,
    ) != (1, 0, 1, 2, 1, 1)
        || fixture.ticket.settled.load(Ordering::Relaxed) != 0
        || fixture
            .ticket
            .pending
            .lock()
            .map_err(|_poison| "scalar ticket debt poisoned")?
            .len()
            != 1
        || !final_path.exists()
    {
        return Err("failed scalar barrier confirmed or released physical debt".into());
    }
    Ok(())
}
