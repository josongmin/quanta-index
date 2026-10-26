//! Safe Catalog `SQLite` VFS binder for the common publication file authority.
//!
//! Native VFS/FFI stays in the pinned `libsqlite3-sys` boundary.
//! Call `scope()` around open, queries, transactions, close and retirement drain.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, OsString};
use std::fs::File;
use std::io::Read as _;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use quanta_index_catalog::{CatalogIoAdmission, CatalogIoScope, CatalogReadOnlyIoAdmission};
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::index::{
    IndexError, ObjectReadFile, ObjectReadIoPort, WorkBudget, WorkLimits,
};
use quanta_index_core::publication_admission::{
    CatalogPublicationFundingPort, CatalogPublicationFundingScope,
};
use quanta_index_core::publication_capacity::PublicationAllowance;
use quanta_index_core::{CoreError, RequestBudgetV1};
use rusqlite::ffi::quanta_capacity_vfs::{
    self, Admission, Event, EventKind, FileWitness, Refusal, RegisteredVfs, ScopeGuard,
};

use crate::publication_file_capacity::{
    FileMutation, InodeObservation, NativeDeleteTicket, NativeOpenGuard, NativeReaderLease,
    PublicationFileCapacity,
};

mod diagnostic;
mod refusal_diagnostic;
mod service;
pub use service::ServiceFundedCatalogIo;

static NEXT_SCOPE: AtomicU64 = AtomicU64::new(1);
static PROCESS_ROUTER: OnceLock<Arc<Router>> = OnceLock::new();
static REGISTER_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
enum FundingPolicy {
    Unqualified,
    HeldParent,
    LocalOrHeldParent,
}

struct CurrentFunding {
    scope: NonZeroU64,
    parent: Arc<PublicationAllowance>,
}

thread_local! {
    static CURRENT_FUNDING: RefCell<Option<CurrentFunding>> = const { RefCell::new(None) };
    static CURRENT_VERIFICATION: RefCell<Option<(NonZeroU64, RequestBudgetV1)>> = const { RefCell::new(None) };
}

struct FundedScopeGuard {
    previous: Option<CurrentFunding>,
    _root_scope: ScopeGuard,
    _not_send: PhantomData<Rc<()>>,
}

impl CatalogPublicationFundingScope for FundedScopeGuard {}

impl Drop for FundedScopeGuard {
    fn drop(&mut self) {
        CURRENT_FUNDING.with(|slot| {
            let _finished = slot.replace(self.previous.take());
        });
    }
}

fn refusal(error: CoreError) -> Refusal {
    let mapped = if matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        }
    ) {
        Refusal::Capacity
    } else {
        Refusal::Io
    };
    // Preserve only an opt-in observation of this callback-local CoreError.
    // The native callback still returns only the fixed SQLite refusal code.
    refusal_diagnostic::observe(error, mapped);
    mapped
}
fn observation(witness: FileWitness) -> InodeObservation {
    InodeObservation {
        inode: quanta_index_core::publication_capacity::InodeKey {
            device: witness.device,
            inode: witness.inode,
        },
        bytes: witness.bytes,
        links: witness.links,
    }
}
fn event_path(path: Option<&CStr>) -> Result<PathBuf, Refusal> {
    let path = path.ok_or(Refusal::CannotOpen)?;
    let path = PathBuf::from(OsString::from_vec(path.to_bytes().to_vec()));
    if !path.is_absolute() {
        return Err(Refusal::CannotOpen);
    }
    Ok(path)
}
fn shm_path(path: &Path) -> PathBuf {
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.extend_from_slice(b"-shm");
    PathBuf::from(OsString::from_vec(bytes))
}

struct ShmHandle {
    path: PathBuf,
    opening: Option<NativeOpenGuard>,
    mutation: Option<FileMutation>,
    mutation_parent: Option<Arc<PublicationAllowance>>,
    reader: Option<NativeReaderLease>,
    retired: bool,
}
struct Handle {
    path: PathBuf,
    opening: Option<NativeOpenGuard>,
    reader: Option<NativeReaderLease>,
    mutation: Option<FileMutation>,
    mutation_parent: Option<Arc<PublicationAllowance>>,
    shm: Option<ShmHandle>,
    delete_on_close: bool,
    main_retired: bool,
    // Observation only; never consulted by mutation admission or settlement.
    sync_mutation: diagnostic::MutationClass,
}
struct AdapterState {
    handles: BTreeMap<u64, Handle>,
    deletes: BTreeMap<PathBuf, NativeDeleteTicket>,
    poisoned: bool,
}
struct Adapter {
    common: Arc<PublicationFileCapacity>,
    scope: NonZeroU64,
    funding_policy: FundingPolicy,
    temp_nonce: [u8; 16],
    refusal_diagnostics: bool,
    state: Mutex<AdapterState>,
}
impl Adapter {
    fn parent_for_mutation(&self) -> Result<Option<Arc<PublicationAllowance>>, Refusal> {
        match self.funding_policy {
            FundingPolicy::Unqualified => Ok(None),
            FundingPolicy::HeldParent | FundingPolicy::LocalOrHeldParent => {
                CURRENT_FUNDING.with(|slot| {
                    let funding = slot.borrow();
                    match funding.as_ref() {
                        Some(funding) if funding.scope == self.scope => {
                            Ok(Some(Arc::clone(&funding.parent)))
                        }
                        Some(_) => Err(Refusal::Io),
                        None if matches!(self.funding_policy, FundingPolicy::LocalOrHeldParent) => {
                            Ok(None)
                        }
                        None => Err(Refusal::Capacity),
                    }
                })
            }
        }
    }

    fn require_pending_parent(
        &self,
        current: Option<&Arc<PublicationAllowance>>,
        pending: Option<&Arc<PublicationAllowance>>,
    ) -> Result<(), Refusal> {
        match self.funding_policy {
            FundingPolicy::Unqualified => Ok(()),
            FundingPolicy::LocalOrHeldParent if pending.is_none() => Ok(()),
            FundingPolicy::HeldParent | FundingPolicy::LocalOrHeldParent => {
                match (current, pending) {
                    (Some(current), Some(pending)) if Arc::ptr_eq(current, pending) => Ok(()),
                    _ => Err(Refusal::Capacity),
                }
            }
        }
    }

    fn begin_native_create(
        &self,
        path: &Path,
        parent: Option<Arc<PublicationAllowance>>,
    ) -> Result<FileMutation, Refusal> {
        parent
            .map_or_else(
                || self.common.begin_native_create(path),
                |parent| self.common.begin_native_create_with_allowance(path, parent),
            )
            .map_err(refusal)
    }

    fn begin_native_resize(
        &self,
        path: &Path,
        witness: InodeObservation,
        target_end: u64,
        parent: Option<Arc<PublicationAllowance>>,
    ) -> Result<FileMutation, Refusal> {
        parent
            .map_or_else(
                || self.common.begin_native_resize(path, witness, target_end),
                |parent| {
                    self.common
                        .begin_native_resize_with_allowance(path, witness, target_end, parent)
                },
            )
            .map_err(refusal)
    }

    fn begin_native_resize_path(
        &self,
        path: &Path,
        target_end: u64,
        parent: Option<Arc<PublicationAllowance>>,
    ) -> Result<FileMutation, Refusal> {
        parent
            .map_or_else(
                || self.common.begin_native_resize_path(path, target_end),
                |parent| {
                    self.common
                        .begin_native_resize_path_with_allowance(path, target_end, parent)
                },
            )
            .map_err(refusal)
    }

    fn is_retirement_ready(&self) -> Result<bool, CoreError> {
        let state = self.state.lock().map_err(|error| {
            CoreError::Storage(format!("SQLite adapter mutex poisoned: {error}"))
        })?;
        let ready = !state.poisoned && state.handles.is_empty() && state.deletes.is_empty();
        drop(state);
        // Every remaining common reader or mutation owns the same Shared Arc
        // and therefore the root lease. Retiring this native scope must not
        // freeze another subsystem's still-live common authority.
        Ok(ready)
    }
    fn temp_path(&self, handle_id: u64) -> PathBuf {
        let nonce = format!("{:032x}", u128::from_be_bytes(self.temp_nonce));
        self.common
            .state_root()
            .join(format!(".qiv-tmp-{nonce}-{handle_id:016x}"))
    }
    fn ensure_handle_capacity(&self, state: &AdapterState) -> Result<(), Refusal> {
        if state.handles.len() >= self.common.max_open_handles() {
            Err(Refusal::Capacity)
        } else {
            Ok(())
        }
    }
    fn with_state<T>(
        &self,
        update: impl FnOnce(&mut AdapterState) -> Result<T, Refusal>,
    ) -> Result<T, Refusal> {
        let mut state = self.state.lock().map_err(|_error| Refusal::Io)?;
        let result = update(&mut state);
        drop(state);
        result
    }
    fn before_open(&self, event: &Event<'_>) -> Result<Option<CString>, Refusal> {
        self.with_state(|state| {
            self.ensure_handle_capacity(state)?;
            if state.handles.contains_key(&event.handle_id) {
                return Err(Refusal::Io);
            }
            let temp = event.path.is_none();
            let path = if temp {
                self.temp_path(event.handle_id)
            } else {
                event_path(event.path)?
            };
            let (opening, mutation, mutation_parent) = if self
                .common
                .path_observation(&path)
                .map_err(refusal)?
                .is_some()
            {
                if event.open_flags & rusqlite::ffi::SQLITE_OPEN_EXCLUSIVE != 0 {
                    return Err(Refusal::CannotOpen);
                }
                (
                    Some(
                        self.common
                            .begin_native_reader_open(&path)
                            .map_err(refusal)?,
                    ),
                    None,
                    None,
                )
            } else {
                // New native files need an OutputTicket before lower xOpen. An
                // attempted read of an absent path is refused before native I/O.
                if event.open_flags
                    & (rusqlite::ffi::SQLITE_OPEN_CREATE | rusqlite::ffi::SQLITE_OPEN_DELETEONCLOSE)
                    == 0
                {
                    return Err(Refusal::CannotOpen);
                }
                let parent = self.parent_for_mutation()?;
                let ticket = self.begin_native_create(&path, parent.clone())?;
                (None, Some(ticket), parent)
            };
            let redirect = if temp {
                Some(
                    CString::new(path.as_os_str().as_bytes())
                        .map_err(|_error| Refusal::CannotOpen)?,
                )
            } else {
                None
            };
            let sync_mutation = if mutation.is_some() {
                // Only an absent-path create ticket takes this branch; CREATE
                // flags on an existing file do not imply a new directory entry.
                diagnostic::MutationClass::Create
            } else {
                diagnostic::MutationClass::NoMutation
            };
            let previous = state.handles.insert(
                event.handle_id,
                Handle {
                    path,
                    opening,
                    reader: None,
                    mutation,
                    mutation_parent,
                    shm: None,
                    delete_on_close: event.open_flags & rusqlite::ffi::SQLITE_OPEN_DELETEONCLOSE
                        != 0,
                    main_retired: false,
                    sync_mutation,
                },
            );
            if previous.is_some() {
                return Err(Refusal::Io);
            }
            Ok(redirect)
        })
    }
    fn before_native_open(&self, event: &Event<'_>) -> Result<(), Refusal> {
        self.with_state(|state| {
            let handle = state.handles.get_mut(&event.handle_id).ok_or(Refusal::Io)?;
            if let Some(ticket) = handle.mutation.as_mut() {
                ticket.creation_started().map_err(refusal)?;
            }
            Ok(())
        })
    }
    fn open_aborted_no_fd(&self, event: &Event<'_>) -> Result<(), Refusal> {
        if !event.fd_retired {
            return Err(Refusal::Io);
        }
        self.with_state(|state| {
            let handle = state.handles.get(&event.handle_id).ok_or(Refusal::Io)?;
            if handle
                .mutation
                .as_ref()
                .is_some_and(FileMutation::is_started)
            {
                return Err(Refusal::Io);
            }
            let removed = state.handles.remove(&event.handle_id);
            if removed.is_none() {
                return Err(Refusal::Io);
            }
            Ok(())
        })
    }
    fn after_open(&self, event: &Event<'_>) -> Result<(), Refusal> {
        self.with_state(|state| {
            let handle = state.handles.get_mut(&event.handle_id).ok_or(Refusal::Io)?;
            if event.sqlite_result != rusqlite::ffi::SQLITE_OK {
                // qiv_open may still call lower xClose after this callback. Keep
                // its handle and root custody until the real close witness.
                if handle
                    .mutation
                    .as_ref()
                    .is_some_and(FileMutation::is_started)
                {
                    // Without a successful native FD witness, a created inode
                    // may lie outside the anchored root after parent retarget.
                    state.poisoned = true;
                }
                return Ok(());
            }
            let observed = observation(event.witness.ok_or(Refusal::Io)?);
            if let Some(opening) = handle.opening.as_mut() {
                let reader = opening.opened_witness(observed).map_err(refusal)?;
                handle.reader = Some(reader);
                handle.opening = None;
            } else if let Some(mutation) = handle.mutation.as_mut() {
                if handle.delete_on_close && observed.links == 0 {
                    mutation
                        .observe_unlinked_native_fd(observed)
                        .map_err(refusal)?;
                } else {
                    mutation.observe_native_fd(observed).map_err(refusal)?;
                }
            } else {
                return Err(Refusal::Io);
            }
            Ok(())
        })
    }
    fn before_mutation(&self, event: &Event<'_>) -> Result<(), Refusal> {
        self.with_state(|state| {
            let handle = state.handles.get_mut(&event.handle_id).ok_or(Refusal::Io)?;
            let parent = self.parent_for_mutation()?;
            if handle.mutation.is_none() {
                if handle.reader.is_none() {
                    return Err(Refusal::Io);
                }
                handle.mutation = Some(self.begin_native_resize(
                    &handle.path,
                    observation(event.witness.ok_or(Refusal::Io)?),
                    u64::try_from(event.target_size).map_err(|_error| Refusal::Io)?,
                    parent.clone(),
                )?);
                handle.mutation_parent = parent;
            } else {
                self.require_pending_parent(parent.as_ref(), handle.mutation_parent.as_ref())?;
            }
            let mutation = handle.mutation.as_mut().ok_or(Refusal::Io)?;
            let target_end = u64::try_from(event.target_size).map_err(|_error| Refusal::Io)?;
            mutation.reserve_end(target_end).map_err(refusal)?;
            // reserve_end authenticates root, publication state and allowance
            // on every write, including overwrites. Start only the first
            // mutation; repeating start would authenticate the same authority
            // again without any intervening native IO or phase transition.
            if !mutation.is_started() {
                mutation.start().map_err(refusal)?;
            }
            if event.kind == EventKind::BeforeTruncate {
                handle.sync_mutation.truncate();
            } else {
                handle
                    .sync_mutation
                    .write(event.witness.map(|witness| witness.bytes), Some(target_end));
            }
            Ok(())
        })
    }
    fn after_mutation(&self, event: &Event<'_>) -> Result<(), Refusal> {
        self.with_state(|state| {
            let handle = state.handles.get_mut(&event.handle_id).ok_or(Refusal::Io)?;
            if let (Some(mutation), Some(witness)) = (handle.mutation.as_mut(), event.witness) {
                let observed = observation(witness);
                if !handle.delete_on_close || observed.links != 0 {
                    mutation.observe_native_fd(observed).map_err(refusal)?;
                }
            }
            // Even on a native short-write/ENOSPC the started ticket stays held.
            Ok(())
        })
    }
    fn after_sync(&self, event: &Event<'_>) -> Result<(), Refusal> {
        if event.sqlite_result != rusqlite::ffi::SQLITE_OK {
            return Ok(());
        }
        let diagnostics = diagnostic::context(self.scope);
        self.with_state(|state| {
            let handle = state.handles.get_mut(&event.handle_id).ok_or(Refusal::Io)?;
            if let Some(mutation) = handle.mutation.as_mut() {
                let observed = observation(event.witness.ok_or(Refusal::Io)?);
                if !handle.delete_on_close || observed.links != 0 {
                    let _retained_id = mutation
                        .settle_after_native_sync(observed, diagnostics.as_ref())
                        .map_err(refusal)?;
                    if let Some(existing) = handle.reader.as_ref() {
                        mutation
                            .finish_with_existing_native_lease(existing)
                            .map_err(refusal)?;
                    } else {
                        handle.reader = Some(mutation.take_native_lease().map_err(refusal)?);
                    }
                    handle.mutation = None;
                    handle.mutation_parent = None;
                    handle.sync_mutation = diagnostic::MutationClass::NoMutation;
                }
            }
            Ok(())
        })
    }
    fn before_shm_map(&self, event: &Event<'_>) -> Result<(), Refusal> {
        self.with_state(|state| {
            let handle = state.handles.get_mut(&event.handle_id).ok_or(Refusal::Io)?;
            let had_shm = handle.shm.is_some();
            let path = shm_path(&handle.path);
            let mut shm = handle.shm.take().unwrap_or_else(|| ShmHandle {
                path: path.clone(),
                opening: None,
                mutation: None,
                mutation_parent: None,
                reader: None,
                // No lower SHM operation has run for this new handle.
                retired: true,
            });
            let was_retired = shm.retired;
            let had_mutation = shm.mutation.is_some();
            let result = self.prepare_shm_map(&mut shm, &path, event);
            if result.is_ok() {
                // The lower VFS can now attach a physical SHM descriptor.
                shm.retired = false;
                handle.shm = Some(shm);
            } else {
                // A refused BEFORE callback prevents every lower SHM syscall.
                // Refund only the newly acquired, provably unstarted ticket;
                // an earlier started write and its FD custody remain held.
                if !had_mutation
                    && shm
                        .mutation
                        .as_ref()
                        .is_some_and(|mutation| !mutation.is_started())
                {
                    shm.mutation = None;
                    shm.mutation_parent = None;
                }
                shm.retired = was_retired;
                if had_shm || shm.mutation.is_some() {
                    handle.shm = Some(shm);
                }
            }
            result
        })
    }
    fn prepare_shm_map(
        &self,
        shm: &mut ShmHandle,
        path: &Path,
        event: &Event<'_>,
    ) -> Result<(), Refusal> {
        let extending = event.operation != 0;
        let first_attach = event.first_shm_fd;
        let target = if extending {
            u64::try_from(event.target_size).map_err(|_error| Refusal::Io)?
        } else {
            0
        }
        .max(if first_attach { 3 } else { 0 });
        let parent = if shm.mutation.is_some() || extending || first_attach {
            self.parent_for_mutation()?
        } else {
            None
        };
        if shm.mutation.is_none() {
            let existing = self
                .common
                .path_observation(path)
                .map_err(refusal)?
                .is_some();
            if existing && !extending && !first_attach {
                if shm.reader.is_none() && shm.opening.is_none() {
                    shm.opening = Some(
                        self.common
                            .begin_native_reader_open(path)
                            .map_err(refusal)?,
                    );
                }
                return Ok(());
            }
            if existing {
                shm.mutation = Some(self.begin_native_resize_path(path, target, parent.clone())?);
            } else {
                if !first_attach {
                    return Err(Refusal::Io);
                }
                shm.mutation = Some(self.begin_native_create(path, parent.clone())?);
            }
            shm.mutation_parent = parent;
        } else {
            self.require_pending_parent(parent.as_ref(), shm.mutation_parent.as_ref())?;
        }
        let mutation = shm.mutation.as_mut().ok_or(Refusal::Io)?;
        mutation.reserve_end(target).map_err(refusal)?;
        mutation.start().map_err(refusal)
    }
    fn after_shm_map(&self, event: &Event<'_>) -> Result<(), Refusal> {
        self.with_state(|state| {
            let handle = state.handles.get_mut(&event.handle_id).ok_or(Refusal::Io)?;
            let shm = handle.shm.as_mut().ok_or(Refusal::Io)?;
            if let Some(opening) = shm.opening.as_mut() {
                if let Some(witness) = event.witness {
                    let reader = opening
                        .opened_witness(observation(witness))
                        .map_err(refusal)?;
                    shm.reader = Some(reader);
                    shm.opening = None;
                } else if event.fd_retired {
                    shm.opening = None;
                } else {
                    return Err(Refusal::Io);
                }
            }
            if event.witness.is_none() {
                if event.fd_retired {
                    shm.retired = true;
                    if let Some(mutation) = shm.mutation.as_mut()
                        && self
                            .common
                            .path_observation(&shm.path)
                            .map_err(refusal)?
                            .is_none()
                    {
                        mutation
                            .confirmed_native_uncreated_absent_after_no_fd()
                            .map_err(refusal)?;
                        shm.mutation = None;
                        shm.mutation_parent = None;
                    }
                }
                if shm.mutation.is_some() || !event.fd_retired {
                    return Err(Refusal::Io);
                }
                return Ok(());
            }
            if let (Some(mutation), Some(witness)) = (shm.mutation.as_mut(), event.witness) {
                let observed = observation(witness);
                mutation.observe_native_fd(observed).map_err(refusal)?;
                if event.sqlite_result == rusqlite::ffi::SQLITE_OK {
                    let _retained_id = mutation
                        .settle_after_native_sync(observed, None)
                        .map_err(refusal)?;
                    if let Some(existing) = shm.reader.as_ref() {
                        mutation
                            .finish_with_existing_native_lease(existing)
                            .map_err(refusal)?;
                    } else {
                        shm.reader = Some(mutation.take_native_lease().map_err(refusal)?);
                    }
                    shm.mutation = None;
                    shm.mutation_parent = None;
                }
            }
            Ok(())
        })
    }
    fn after_shm_unmap(&self, event: &Event<'_>) -> Result<(), Refusal> {
        if event.fd_retired {
            self.retire_shm(event)
        } else {
            Ok(())
        }
    }
    fn before_delete(&self, event: &Event<'_>) -> Result<(), Refusal> {
        let path = event_path(event.path)?;
        self.with_state(|state| {
            if state.deletes.len() >= self.common.max_open_handles() {
                return Err(Refusal::Capacity);
            }
            if state.deletes.contains_key(&path) {
                return Err(Refusal::Busy);
            }
            if self
                .common
                .path_observation(&path)
                .map_err(refusal)?
                .is_some()
            {
                let mut ticket = self.common.begin_native_delete(&path).map_err(refusal)?;
                ticket.deletion_started().map_err(refusal)?;
                let previous = state.deletes.insert(path.clone(), ticket);
                if previous.is_some() {
                    return Err(Refusal::Io);
                }
            }
            Ok(())
        })
    }
    fn after_delete(&self, event: &Event<'_>) -> Result<(), Refusal> {
        let path = event_path(event.path)?;
        self.with_state(|state| {
            if event.sqlite_result == rusqlite::ffi::SQLITE_OK {
                if let Some(ticket) = state.deletes.get_mut(&path) {
                    ticket.unlinked_and_synced().map_err(refusal)?;
                    let removed = state.deletes.remove(&path);
                    if removed.is_none() {
                        return Err(Refusal::Io);
                    }
                }
            } else if let Some(ticket) = state.deletes.get_mut(&path) {
                ticket
                    .confirmed_unchanged_after_failure()
                    .map_err(refusal)?;
                let removed = state.deletes.remove(&path);
                if removed.is_none() {
                    return Err(Refusal::Io);
                }
            }
            Ok(())
        })
    }
    fn retire_main(&self, event: &Event<'_>) -> Result<(), Refusal> {
        self.with_state(|state| {
            let handle = state.handles.get_mut(&event.handle_id).ok_or(Refusal::Io)?;
            handle.main_retired = true;
            handle.opening = None;
            if handle.delete_on_close
                && let Some(reader) = handle.reader.as_ref()
            {
                self.common
                    .confirm_native_unlinked_lease(
                        &handle.path,
                        reader,
                        observation(event.witness.ok_or(Refusal::Io)?),
                    )
                    .map_err(refusal)?;
            }
            if let Some(reader) = handle.reader.take() {
                reader.retire().map_err(refusal)?;
            }
            if let Some(mutation) = handle.mutation.as_mut() {
                if mutation.is_settled() {
                    mutation.confirmed_native_fd_retired().map_err(refusal)?;
                    handle.mutation = None;
                    handle.mutation_parent = None;
                } else if handle.delete_on_close {
                    mutation
                        .confirmed_native_absent_after_retirement(observation(
                            event.witness.ok_or(Refusal::Io)?,
                        ))
                        .map_err(refusal)?;
                    mutation.confirmed_native_fd_retired().map_err(refusal)?;
                    handle.mutation = None;
                    handle.mutation_parent = None;
                } else if self
                    .common
                    .path_observation(&handle.path)
                    .map_err(refusal)?
                    .is_some()
                {
                    // Keep its charge pending, but preserve the actual close
                    // witness for explicit scratch disposal or boot inventory.
                    mutation
                        .confirmed_unsynced_native_fd_retired(observation(
                            event.witness.ok_or(Refusal::Io)?,
                        ))
                        .map_err(refusal)?;
                }
            }
            self.common.sweep_retired().map_err(refusal)?;
            if handle.shm.as_ref().is_none_or(|shm| shm.retired) {
                let removed = state.handles.remove(&event.handle_id);
                if removed.is_none() {
                    return Err(Refusal::Io);
                }
            }
            Ok(())
        })
    }
    fn retire_shm(&self, event: &Event<'_>) -> Result<(), Refusal> {
        self.with_state(|state| {
            let handle = state.handles.get_mut(&event.handle_id).ok_or(Refusal::Io)?;
            if let Some(shm) = handle.shm.as_mut() {
                shm.retired = true;
                // An unreadable fstat witness left the pre-open namespace guard
                // in place. Physical native retirement is the release witness.
                shm.opening = None;
                if let Some(reader) = shm.reader.as_ref()
                    && self
                        .common
                        .path_observation(&shm.path)
                        .map_err(refusal)?
                        .is_none()
                {
                    self.common
                        .confirm_native_unlinked_lease(
                            &shm.path,
                            reader,
                            observation(event.witness.ok_or(Refusal::Io)?),
                        )
                        .map_err(refusal)?;
                }
                if let Some(reader) = shm.reader.take() {
                    reader.retire().map_err(refusal)?;
                }
                if let Some(mutation) = shm.mutation.as_mut() {
                    if mutation.is_settled() {
                        mutation.confirmed_native_fd_retired().map_err(refusal)?;
                        shm.mutation = None;
                        shm.mutation_parent = None;
                    } else if self
                        .common
                        .path_observation(&shm.path)
                        .map_err(refusal)?
                        .is_some()
                    {
                        // Failed SHM map or same-FD fsync remains conservative.
                    } else {
                        mutation
                            .confirmed_native_absent_after_retirement(observation(
                                event.witness.ok_or(Refusal::Io)?,
                            ))
                            .map_err(refusal)?;
                        mutation.confirmed_native_fd_retired().map_err(refusal)?;
                        shm.mutation = None;
                        shm.mutation_parent = None;
                    }
                }
            }
            self.common.sweep_retired().map_err(refusal)?;
            if handle.main_retired {
                let removed = state.handles.remove(&event.handle_id);
                if removed.is_none() {
                    return Err(Refusal::Io);
                }
            }
            Ok(())
        })
    }
}
impl Adapter {
    fn dispatch(&self, event: &Event<'_>) -> Result<Option<CString>, Refusal> {
        match event.kind {
            EventKind::BeforeVerificationRead => {
                CURRENT_VERIFICATION.with(|slot| {
                    let current = slot.borrow();
                    match current.as_ref() {
                        Some((scope, request)) if *scope == self.scope => request
                            .checkpoint("native Catalog verification read")
                            .map_err(refusal),
                        _ => Err(Refusal::Io),
                    }
                })?;
                Ok(None)
            }
            EventKind::BeforeOpen => self.before_open(event),
            EventKind::BeforeNativeOpen => {
                self.before_native_open(event)?;
                Ok(None)
            }
            EventKind::OpenAbortedNoFd => {
                self.open_aborted_no_fd(event)?;
                Ok(None)
            }
            EventKind::AfterOpen => {
                self.after_open(event)?;
                Ok(None)
            }
            EventKind::BeforeWrite | EventKind::BeforeTruncate => {
                self.before_mutation(event)?;
                Ok(None)
            }
            EventKind::AfterWrite | EventKind::AfterTruncate => {
                self.after_mutation(event)?;
                Ok(None)
            }
            EventKind::AfterSync => {
                self.after_sync(event)?;
                Ok(None)
            }
            EventKind::BeforeShmMap => {
                self.before_shm_map(event)?;
                Ok(None)
            }
            EventKind::AfterShmMap => {
                self.after_shm_map(event)?;
                Ok(None)
            }
            EventKind::AfterShmUnmap => {
                self.after_shm_unmap(event)?;
                Ok(None)
            }
            EventKind::BeforeDelete => {
                self.before_delete(event)?;
                Ok(None)
            }
            EventKind::AfterDelete => {
                self.after_delete(event)?;
                Ok(None)
            }
            EventKind::AfterClose => {
                if event.fd_retired {
                    self.retire_main(event)?;
                }
                Ok(None)
            }
            EventKind::FdRetired => {
                if event.operation == 0 {
                    self.retire_main(event)?;
                } else if event.operation == 1 {
                    self.retire_shm(event)?;
                } else {
                    return Err(Refusal::Io);
                }
                Ok(None)
            }
            EventKind::BeforeSync | EventKind::BeforeFileControl | EventKind::AfterFileControl => {
                Ok(None)
            }
        }
    }
}
impl Admission for Adapter {
    fn on_event(&self, event: Event<'_>) -> Result<Option<CString>, Refusal> {
        if event.scope_id != self.scope {
            return Err(Refusal::Io);
        }
        let kind = event.kind;
        if matches!(
            kind,
            EventKind::BeforeOpen
                | EventKind::BeforeNativeOpen
                | EventKind::BeforeWrite
                | EventKind::BeforeTruncate
                | EventKind::BeforeSync
                | EventKind::BeforeShmMap
                | EventKind::BeforeDelete
                | EventKind::BeforeFileControl
                | EventKind::BeforeVerificationRead
        ) {
            self.with_state(|state| {
                if state.poisoned {
                    Err(Refusal::Io)
                } else {
                    Ok(())
                }
            })?;
        }
        let settlement = if kind == EventKind::AfterSync {
            diagnostic::after(
                self.scope,
                event.handle_id,
                event.sqlite_result,
                event.native_sync,
                event.witness.is_some_and(|witness| witness.links != 0),
            );
            diagnostic::settlement(self.scope, event.open_flags)
        } else {
            None
        };
        let result =
            diagnostic::finish_settlement(settlement, event.sqlite_result, self.dispatch(&event));
        if kind == EventKind::BeforeSync && result.is_ok() {
            diagnostic::before(self.scope, event.handle_id, event.open_flags, || {
                let state = match self.state.lock() {
                    Ok(state) => state,
                    Err(_poisoned) => return diagnostic::MutationClass::Unclassified,
                };
                state
                    .handles
                    .get(&event.handle_id)
                    .map_or(diagnostic::MutationClass::Unclassified, |handle| {
                        handle.sync_mutation
                    })
            });
        }
        if result.is_err()
            && matches!(
                kind,
                EventKind::OpenAbortedNoFd
                    | EventKind::AfterOpen
                    | EventKind::AfterWrite
                    | EventKind::AfterTruncate
                    | EventKind::AfterSync
                    | EventKind::AfterShmMap
                    | EventKind::AfterShmUnmap
                    | EventKind::AfterDelete
                    | EventKind::AfterClose
                    | EventKind::FdRetired
            )
            && let Ok(mut state) = self.state.lock()
        {
            state.poisoned = true;
        }
        result
    }
}

struct ScopedAdapter {
    adapter: Arc<Adapter>,
    closing: bool,
}
struct Router {
    vfs: OnceLock<&'static RegisteredVfs>,
    name: CString,
    global_max_open_handles: u32,
    scopes: Mutex<BTreeMap<NonZeroU64, ScopedAdapter>>,
}
impl Router {
    fn vfs(&self) -> Result<&'static RegisteredVfs, CoreError> {
        self.vfs
            .get()
            .copied()
            .ok_or_else(|| CoreError::Storage("SQLite VFS not registered".into()))
    }
    fn retire_scope_if_quiescent(&self, scope: NonZeroU64) -> Result<(), CoreError> {
        let mut scopes = self
            .scopes
            .lock()
            .map_err(|_error| CoreError::Storage("SQLite scope router poisoned".into()))?;
        let Some(entry) = scopes.get(&scope) else {
            return Ok(());
        };
        // A callback may have cloned the adapter before `closing` was set and
        // not yet registered its native handle. Its clone keeps the scope.
        if entry.closing
            && Arc::strong_count(&entry.adapter) == 1
            && entry.adapter.is_retirement_ready()?
        {
            let removed = scopes.remove(&scope);
            drop(scopes);
            drop(removed);
        }
        Ok(())
    }
    fn close_scope(&self, scope: NonZeroU64) -> Result<(), CoreError> {
        {
            let mut scopes = self
                .scopes
                .lock()
                .map_err(|_error| CoreError::Storage("SQLite scope router poisoned".into()))?;
            let entry = scopes
                .get_mut(&scope)
                .ok_or_else(|| CoreError::InvalidContract("SQLite scope missing".into()))?;
            entry.closing = true;
            drop(scopes);
        }
        self.vfs()?.drain_retirements().map_err(|error| {
            CoreError::Storage(format!("SQLite FD retirement drain: {error:?}"))
        })?;
        self.retire_scope_if_quiescent(scope)
    }

    fn retire_scope_checked(&self, scope: NonZeroU64) -> Result<(), CoreError> {
        {
            let mut scopes = self
                .scopes
                .lock()
                .map_err(|_error| CoreError::Storage("SQLite scope router poisoned".into()))?;
            let Some(entry) = scopes.get_mut(&scope) else {
                return Ok(());
            };
            entry.closing = true;
            drop(scopes);
        }
        self.vfs()?.drain_retirements().map_err(|error| {
            CoreError::Storage(format!("SQLite FD retirement drain: {error:?}"))
        })?;
        self.retire_scope_if_quiescent(scope)?;
        let scopes = self
            .scopes
            .lock()
            .map_err(|_error| CoreError::Storage("SQLite scope router poisoned".into()))?;
        let retained = if let Some(entry) = scopes.get(&scope) {
            let state = entry
                .adapter
                .state
                .lock()
                .map_err(|_error| CoreError::Storage("SQLite adapter poisoned".into()))?;
            let main_pending = state
                .handles
                .values()
                .filter(|handle| !handle.main_retired)
                .count();
            let shm_pending = state
                .handles
                .values()
                .filter(|handle| handle.shm.as_ref().is_some_and(|shm| !shm.retired))
                .count();
            let handles = state.handles.len();
            let deletes = state.deletes.len();
            let poisoned = state.poisoned;
            drop(state);
            Some(format!(
                "SQLite scope still retains native handles or uncertain retirement: scope={scope}, adapter_owners={}, handles={handles}, main_pending={main_pending}, shm_pending={shm_pending}, deletes={deletes}, poisoned={poisoned}",
                Arc::strong_count(&entry.adapter),
            ))
        } else {
            None
        };
        drop(scopes);
        if let Some(detail) = retained {
            return Err(CoreError::NotReady(detail));
        }
        Ok(())
    }
}
struct RouterCallback(Arc<Router>);
impl Admission for RouterCallback {
    fn observe_native_sync(&self, scope_id: NonZeroU64) -> bool {
        diagnostic::context(scope_id).is_some()
    }

    fn on_event(&self, event: Event<'_>) -> Result<Option<CString>, Refusal> {
        let scope = event.scope_id;
        if event.kind == EventKind::AfterWrite {
            // The lower xWrite has already run. Adapter settlement may still
            // refuse; neither result changes the native write observation.
            diagnostic::write(scope, event.open_flags, event.amount, event.sqlite_result);
        }
        let adapter = {
            let scopes = self.0.scopes.lock().map_err(|_error| Refusal::Io)?;
            let entry = scopes.get(&scope).ok_or(Refusal::Io)?;
            if entry.closing
                && matches!(event.kind, EventKind::BeforeOpen | EventKind::BeforeDelete)
            {
                return Err(Refusal::Busy);
            }
            let adapter = Arc::clone(&entry.adapter);
            drop(scopes);
            adapter
        };
        let frame = refusal_diagnostic::Frame::enter(
            adapter.refusal_diagnostics,
            scope,
            event.handle_id,
            event.kind,
            event.operation,
        );
        let result = adapter.on_event(event);
        drop(adapter);
        // A successful physical retirement may be the last owner of a root.
        // Errors retain the scope and its StateRootLease conservatively.
        let result = result.and_then(|payload| {
            self.0.retire_scope_if_quiescent(scope).map_err(refusal)?;
            Ok(payload)
        });
        frame.finish(&result);
        result
    }
}

/// RAII runtime registration.
///
/// Drop closes the scope to new native opens and deletes. The process-global
/// router retains that scope until its actual native handles and delete tickets
/// retire. Other common readers and mutations keep their own root custody.
pub struct SqliteCapacityVfs {
    router: Arc<Router>,
    vfs: &'static RegisteredVfs,
    name: String,
    scope: NonZeroU64,
    funding_policy: FundingPolicy,
}
impl SqliteCapacityVfs {
    pub fn register(
        common: Arc<PublicationFileCapacity>,
        name: CString,
        global_max_open_handles: u32,
    ) -> Result<Self, CoreError> {
        Self::register_with_policy(
            common,
            name,
            global_max_open_handles,
            FundingPolicy::Unqualified,
        )
    }

    /// Register a VFS whose native creates and growth require an authenticated
    /// already-held parent on the `SQLite` operation's own thread.
    pub fn register_funded(
        common: Arc<PublicationFileCapacity>,
        name: CString,
        global_max_open_handles: u32,
    ) -> Result<Self, CoreError> {
        Self::register_with_policy(
            common,
            name,
            global_max_open_handles,
            FundingPolicy::HeldParent,
        )
    }

    /// Native foreground uses local funding; selected maintenance may supply
    /// its already-held parent. A foreign scope never falls back to local.
    pub(crate) fn register_with_optional_funding(
        common: Arc<PublicationFileCapacity>,
        name: CString,
        global_max_open_handles: u32,
    ) -> Result<Self, CoreError> {
        Self::register_with_policy(
            common,
            name,
            global_max_open_handles,
            FundingPolicy::LocalOrHeldParent,
        )
    }

    fn register_with_policy(
        common: Arc<PublicationFileCapacity>,
        name: CString,
        global_max_open_handles: u32,
        funding_policy: FundingPolicy,
    ) -> Result<Self, CoreError> {
        Self::register_with_policy_and_diagnostics(
            common,
            name,
            global_max_open_handles,
            funding_policy,
            refusal_diagnostic::enabled_at_registration(),
        )
    }

    fn register_with_policy_and_diagnostics(
        common: Arc<PublicationFileCapacity>,
        name: CString,
        global_max_open_handles: u32,
        funding_policy: FundingPolicy,
        refusal_diagnostics: bool,
    ) -> Result<Self, CoreError> {
        let validated_name = name
            .to_str()
            .map_err(|error| CoreError::InvalidContract(format!("SQLite VFS name UTF-8: {error}")))?
            .to_owned();
        let max_open_handles = usize::try_from(global_max_open_handles).map_err(|_error| {
            CoreError::InvalidContract("SQLite handle ceiling exceeds usize".into())
        })?;
        if global_max_open_handles == 0 || max_open_handles < common.max_open_handles() {
            return Err(CoreError::InvalidContract(
                "global SQLite handle ceiling below root ceiling".into(),
            ));
        }
        let number = NEXT_SCOPE
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_error| CoreError::InvalidContract("SQLite scope IDs exhausted".into()))?;
        let scope = NonZeroU64::new(number)
            .ok_or_else(|| CoreError::InvalidContract("zero SQLite scope".into()))?;
        let mut nonce = [0_u8; 16];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut nonce))
            .map_err(|error| CoreError::Storage(format!("SQLite private temp nonce: {error}")))?;
        let adapter = Arc::new(Adapter {
            common,
            scope,
            funding_policy,
            temp_nonce: nonce,
            refusal_diagnostics,
            state: Mutex::new(AdapterState {
                handles: BTreeMap::new(),
                deletes: BTreeMap::new(),
                poisoned: false,
            }),
        });
        let registration_guard = REGISTER_LOCK
            .lock()
            .map_err(|_error| CoreError::Storage("SQLite registration mutex poisoned".into()))?;
        let router = if let Some(router) = PROCESS_ROUTER.get() {
            if router.name.as_c_str() != name.as_c_str()
                || router.global_max_open_handles != global_max_open_handles
            {
                return Err(CoreError::InvalidContract("SQLite VFS process registration conflicts with existing name or global handle ceiling".into()));
            }
            Arc::clone(router)
        } else {
            let router = Arc::new(Router {
                vfs: OnceLock::new(),
                name: name.clone(),
                global_max_open_handles,
                scopes: Mutex::new(BTreeMap::new()),
            });
            let vfs = quanta_capacity_vfs::register(
                name,
                global_max_open_handles,
                Box::new(RouterCallback(Arc::clone(&router))),
            )
            .map_err(|error| CoreError::Storage(format!("SQLite VFS register: {error:?}")))?;
            router
                .vfs
                .set(vfs)
                .map_err(|_error| CoreError::Storage("SQLite VFS duplicate registration".into()))?;
            PROCESS_ROUTER
                .set(Arc::clone(&router))
                .map_err(|_error| CoreError::Storage("SQLite VFS router duplicate".into()))?;
            router
        };
        let mut scopes = router
            .scopes
            .lock()
            .map_err(|_error| CoreError::Storage("SQLite scope router poisoned".into()))?;
        if scopes.len() >= max_open_handles {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                message: "SQLite active/retiring scope registry ceiling exceeded".into(),
            });
        }
        if scopes
            .insert(
                scope,
                ScopedAdapter {
                    adapter,
                    closing: false,
                },
            )
            .is_some()
        {
            return Err(CoreError::Storage("SQLite scope ID reused".into()));
        }
        drop(scopes);
        let vfs = router.vfs()?;
        drop(registration_guard);
        Ok(Self {
            router,
            vfs,
            name: validated_name,
            scope,
            funding_policy,
        })
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    fn common_for_scope(&self) -> Result<Arc<PublicationFileCapacity>, CoreError> {
        let scopes = self.router.scopes.lock().map_err(|error| {
            CoreError::Storage(format!("SQLite scope router poisoned: {error}"))
        })?;
        let entry = scopes
            .get(&self.scope)
            .ok_or_else(|| CoreError::InvalidContract("SQLite scope has retired".into()))?;
        if entry.closing {
            return Err(CoreError::InvalidContract("SQLite scope is closing".into()));
        }
        let common = Arc::clone(&entry.adapter.common);
        drop(scopes);
        Ok(common)
    }
    #[must_use]
    pub fn scope(&self) -> ScopeGuard {
        self.vfs.enter_scope(self.scope)
    }

    /// Bind the current `SQLite` operation to a held parent from this VFS's
    /// capacity authority. The returned guard is local to this thread.
    pub fn enter_funded_scope(
        &self,
        parent: Arc<PublicationAllowance>,
    ) -> Result<Box<dyn CatalogPublicationFundingScope>, CoreError> {
        if matches!(self.funding_policy, FundingPolicy::Unqualified) {
            return Err(CoreError::InvalidContract(
                "SQLite VFS was registered for unqualified local funding".into(),
            ));
        }
        let common = self.common_for_scope()?;
        common.validate_held_allowance(&parent)?;
        Ok(self.bind_funding_scope(parent))
    }

    fn bind_funding_scope(
        &self,
        parent: Arc<PublicationAllowance>,
    ) -> Box<dyn CatalogPublicationFundingScope> {
        let root_scope = self.scope();
        let previous = CURRENT_FUNDING.with(|slot| {
            slot.replace(Some(CurrentFunding {
                scope: self.scope,
                parent,
            }))
        });
        Box::new(FundedScopeGuard {
            previous,
            _root_scope: root_scope,
            _not_send: PhantomData,
        })
    }

    /// Constructor-authenticated close funding stays alive through native close.
    /// Every create/growth callback still authenticates its supplied parent.
    pub(crate) fn close_scope_with_parent(
        &self,
        parent: Arc<PublicationAllowance>,
    ) -> Box<dyn CatalogIoScope> {
        let funding = self.bind_funding_scope(parent);
        Box::new(CatalogSqliteScope {
            scope: self.scope(),
            _diagnostics: diagnostic::Scope::enter(self.scope, None),
            _funding: Some(funding),
        })
    }
    pub fn drain_retirements(&self) -> Result<(), CoreError> {
        self.vfs.drain_retirements().map_err(|error| {
            CoreError::Storage(format!("SQLite FD retirement drain: {error:?}"))
        })?;
        self.router.retire_scope_if_quiescent(self.scope)
    }
    /// Close this scope to new native operations and require actual retirement.
    /// A live handle or uncertain close remains owned by the router on error.
    pub fn retire_checked(&self) -> Result<(), CoreError> {
        self.router.retire_scope_checked(self.scope)
    }
    pub fn close(self) -> Result<(), CoreError> {
        self.router.close_scope(self.scope)
    }
}
impl CatalogPublicationFundingPort for SqliteCapacityVfs {
    fn enter_funded_scope(
        &self,
        parent: Arc<PublicationAllowance>,
    ) -> Result<Box<dyn CatalogPublicationFundingScope>, CoreError> {
        SqliteCapacityVfs::enter_funded_scope(self, parent)
    }
}
impl Drop for SqliteCapacityVfs {
    fn drop(&mut self) {
        // A failed retirement keeps the adapter and its root lease in the
        // process router. Drop must not release that conservative custody.
        let _retirement_result = self.router.close_scope(self.scope);
    }
}

struct CatalogSqliteScope {
    scope: ScopeGuard,
    _diagnostics: diagnostic::Scope,
    _funding: Option<Box<dyn CatalogPublicationFundingScope>>,
}
impl CatalogIoScope for CatalogSqliteScope {
    fn is_capacity_refusal(&self, error: &rusqlite::Error) -> bool {
        matches!(error,
            rusqlite::Error::SqliteFailure(failure, _)
                if self.scope.is_capacity_extended_code(failure.extended_code))
    }
}
impl CatalogIoAdmission for SqliteCapacityVfs {
    fn vfs_name(&self) -> &str {
        &self.name
    }
    fn enter_scope(
        &self,
        diagnostics: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Box<dyn CatalogIoScope> {
        Box::new(CatalogSqliteScope {
            scope: self.scope(),
            _diagnostics: diagnostic::Scope::enter(self.scope, diagnostics),
            _funding: None,
        })
    }
    fn enter_close_scope(&self) -> Box<dyn CatalogIoScope> {
        self.enter_scope(None)
    }
    fn open_lock_file(
        &self,
        path: &Path,
        create_new: bool,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError> {
        let common = self
            .common_for_scope()
            .map_err(|error| catalog_file_error(&error))?;
        if create_new {
            let parent = CURRENT_FUNDING.with(|slot| {
                let funding = slot.borrow();
                match funding.as_ref() {
                    Some(funding) if funding.scope == self.scope => {
                        Ok(Some(Arc::clone(&funding.parent)))
                    }
                    Some(_) => Err(IndexError::BackendViolation(
                        "Catalog lock funding belongs to another scope".into(),
                    )),
                    None => Ok(None),
                }
            })?;
            common
                .prepare_empty_lock_with_parent(path, true, parent)
                .map_err(|error| catalog_file_error(&error))?;
        }
        // Session creation has no caller WorkBudget. Its process-lived owner
        // is bounded by the common handle/registry ceilings, not an invented
        // per-request memory grant or a claim of allocator/RSS enforcement.
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            RequestBudgetV1::unbounded(),
        );
        crate::native_object_read_io::NativeObjectReadIo::new(common).open(path, &mut budget)
    }
    fn unlink_lock_file(&self, path: &Path, held: &File) -> Result<(), IndexError> {
        use std::os::unix::fs::MetadataExt as _;
        let common = self
            .common_for_scope()
            .map_err(|error| catalog_file_error(&error))?;
        let mut deletion = common
            .begin_delete(path)
            .map_err(|error| catalog_file_error(&error))?;
        let (source, _parent) = deletion
            .source_metadata()
            .map_err(|error| catalog_file_error(&error))?;
        let expected = held
            .metadata()
            .map_err(|error| IndexError::Storage(format!("Catalog held lock metadata: {error}")))?;
        if source.dev() != expected.dev()
            || source.ino() != expected.ino()
            || source.len() != expected.len()
            || source.nlink() != expected.nlink()
        {
            return Err(IndexError::Storage(
                "Catalog delete source differs from held lock".into(),
            ));
        }
        deletion
            .perform_and_sync()
            .map_err(|error| catalog_file_error(&error))
    }
}

impl CatalogReadOnlyIoAdmission for SqliteCapacityVfs {
    fn read_only_vfs_name(&self) -> &str {
        &self.name
    }

    fn enter_read_only_scope(&self, request: &RequestBudgetV1) -> Box<dyn CatalogIoScope> {
        let previous =
            CURRENT_VERIFICATION.with(|slot| slot.replace(Some((self.scope, request.clone()))));
        Box::new(CatalogVerificationScope {
            scope: self.vfs.enter_verification_scope(self.scope),
            _diagnostics: diagnostic::Scope::enter(self.scope, None),
            previous,
        })
    }
}

struct CatalogVerificationScope {
    scope: ScopeGuard,
    _diagnostics: diagnostic::Scope,
    previous: Option<(NonZeroU64, RequestBudgetV1)>,
}

impl CatalogIoScope for CatalogVerificationScope {
    fn is_capacity_refusal(&self, error: &rusqlite::Error) -> bool {
        matches!(error, rusqlite::Error::SqliteFailure(failure, _)
            if self.scope.is_capacity_extended_code(failure.extended_code))
    }
}

impl Drop for CatalogVerificationScope {
    fn drop(&mut self) {
        CURRENT_VERIFICATION.with(|slot| {
            let _finished = slot.replace(self.previous.take());
        });
    }
}

fn catalog_file_error(error: &CoreError) -> IndexError {
    if matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        }
    ) {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("Catalog regular-file I/O: {error}"))
    }
}

#[cfg(test)]
mod tests;
