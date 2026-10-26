//! Common, safe filesystem binder for sealed publication capacity.
//!
//! Every public transition keeps the `StateRootLease` and nofollow directory custody.

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("publication_file_capacity currently supports macOS/Linux only");

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read as _;
use std::os::fd::{AsFd as _, AsRawFd as _, RawFd};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::publication_capacity::{
    InodeKey, OutputTicket, PendingId, PublicationAllowance, PublicationCapacity, ResizeTicket,
    RetainedId,
};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticLaneV1, DiagnosticStageV1, DiagnosticTerminalV1,
};
use quanta_index_searchd::app::runtime::{StateRootDirectoryV1, StateRootLease};
use rusqlite::ffi::quanta_capacity_vfs::close_owned_file;
use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, RenameFlags, linkat, mkdirat, openat, renameat_with, statat,
    unlinkat,
};

fn storage(context: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("publication file capacity {context}: {error}"))
}
fn storage_io(context: &str, error: impl Into<std::io::Error>) -> CoreError {
    let error = error.into();
    CoreError::storage_io(&format!("publication file capacity {context}"), &error)
}
fn append_failure(mut primary: CoreError, context: &str, secondary: &CoreError) -> CoreError {
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. }
        | CoreError::Typed { message, .. } => message,
    };
    let _format_result = write!(message, "; {context}: {secondary}");
    primary
}
fn existing_target(context: &str) -> CoreError {
    CoreError::StorageIo {
        kind: std::io::ErrorKind::AlreadyExists,
        raw_os_error: None,
        message: format!("publication file capacity {context}: target already exists"),
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum MissingDisposition {
    Storage,
    NotFound,
}
fn path_error(context: &str, error: rustix::io::Errno, missing: MissingDisposition) -> CoreError {
    if missing == MissingDisposition::NotFound && error == rustix::io::Errno::NOENT {
        CoreError::NotFound(format!("publication file capacity {context}: {error}"))
    } else {
        storage_io(context, error)
    }
}
fn contract(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("publication file capacity: {message}"))
}
fn capacity(message: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: format!("publication file capacity: {message}"),
    }
}
fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, CoreError> {
    mutex.lock().map_err(|error| storage("lock", error))
}

// Register active roots before I/O so an ambiguous owned-FD close always has
// a bounded process-lifetime slot in which to keep the original root lease.
static ROOT_CUSTODY: OnceLock<Mutex<RootCustody>> = OnceLock::new();
struct RootCustody {
    limit: usize,
    roots: BTreeMap<PathBuf, RootCustodyEntry>,
}
struct RootCustodyEntry {
    active: usize,
    quarantine: Option<StateRootLease>,
}
fn register_root_custody(root: &Path, limit: usize) -> Result<(), CoreError> {
    if limit == 0 {
        return Err(contract("process root-custody bound must be positive"));
    }
    let registry = ROOT_CUSTODY.get_or_init(|| {
        Mutex::new(RootCustody {
            limit,
            roots: BTreeMap::new(),
        })
    });
    let mut state = lock(registry)?;
    if state.limit != limit {
        return Err(contract(
            "process root-custody bound differs from registered bound",
        ));
    }
    if let Some(entry) = state.roots.get(root) {
        if entry.quarantine.is_some() {
            return Err(contract("state root has ambiguous-close quarantine"));
        }
        return Err(contract(
            "state root already has an active publication binder",
        ));
    }
    if state.roots.len() >= state.limit {
        return Err(capacity("process root-custody registry ceiling exceeded"));
    }
    if state
        .roots
        .insert(
            root.to_path_buf(),
            RootCustodyEntry {
                active: 1,
                quarantine: None,
            },
        )
        .is_some()
    {
        return Err(contract("state root registry replaced an existing owner"));
    }
    drop(state);
    Ok(())
}
fn custody_registry_or_abort() -> &'static Mutex<RootCustody> {
    let Some(registry) = ROOT_CUSTODY.get() else {
        std::process::abort();
    };
    registry
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InodeObservation {
    pub inode: InodeKey,
    pub bytes: u64,
    pub links: u64,
}
impl InodeObservation {
    pub fn from_file(file: &File) -> Result<Self, CoreError> {
        let metadata = file
            .metadata()
            .map_err(|error| storage_io("fstat", error))?;
        if !metadata.is_file() {
            return Err(contract("owned FD is not a regular file"));
        }
        Ok(Self {
            inode: InodeKey {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            bytes: metadata.len(),
            links: metadata.nlink(),
        })
    }
}

struct Anchor {
    shared: Arc<Shared>,
    parent: File,
    name: OsString,
    relative: PathBuf,
}
struct HeldPath {
    parent: File,
    name: OsString,
    relative: PathBuf,
}
impl HeldPath {
    fn from_anchor(anchor: &Anchor) -> Result<Self, CoreError> {
        Ok(Self {
            parent: anchor
                .parent
                .try_clone()
                .map_err(|error| storage_io("clone Control create parent", error))?,
            name: anchor.name.clone(),
            relative: anchor.relative.clone(),
        })
    }
    fn current_anchor(&self, binder: &PublicationFileCapacity) -> Result<Anchor, CoreError> {
        let current = binder.anchor(&self.relative)?;
        let held = self
            .parent
            .metadata()
            .map_err(|error| storage_io("stat held Control parent", error))?;
        let actual = current
            .parent
            .metadata()
            .map_err(|error| storage_io("stat current Control parent", error))?;
        if held.dev() != actual.dev() || held.ino() != actual.ino() || self.name != current.name {
            return Err(contract("Control create parent incarnation changed"));
        }
        Ok(current)
    }
}
impl Anchor {
    fn stat(&self) -> Result<Option<InodeObservation>, CoreError> {
        match statat(
            &self.parent,
            Path::new(&self.name),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(stat) => {
                if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                    return Err(contract("anchored entry is not a regular file"));
                }
                if stat.st_uid != rustix::process::getuid().as_raw() || stat.st_mode & 0o022 != 0 {
                    return Err(contract(
                        "anchored regular file has insecure owner or write mode",
                    ));
                }
                Ok(Some(InodeObservation {
                    inode: InodeKey {
                        device: crate::publication_inventory::checked_device_id(stat.st_dev)
                            .map_err(|error| storage("stat device", error))?,
                        inode: stat.st_ino,
                    },
                    bytes: u64::try_from(stat.st_size)
                        .map_err(|error| storage("stat width", error))?,
                    links: u64::from(stat.st_nlink),
                }))
            }
            Err(rustix::io::Errno::NOENT) => Ok(None),
            Err(error) => Err(storage_io("stat anchored entry", error)),
        }
    }
    fn open(&self) -> Result<File, CoreError> {
        self.open_with_missing(MissingDisposition::Storage)
    }
    fn open_reader(&self) -> Result<File, CoreError> {
        self.open_with_missing(MissingDisposition::NotFound)
    }
    fn open_with_missing(&self, missing: MissingDisposition) -> Result<File, CoreError> {
        let opened = openat(
            &self.parent,
            Path::new(&self.name),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|error| path_error("open anchored file", error, missing))?;
        let file = File::from(opened);
        let checked = (|| {
            let observed = InodeObservation::from_file(&file)?;
            if self.stat()? != Some(observed) {
                return Err(contract("open FD and anchored pathname disagree"));
            }
            Ok(())
        })();
        match checked {
            Ok(()) => Ok(file),
            Err(error) => {
                if close_owned_file(file).is_err() {
                    PublicationFileCapacity {
                        shared: Arc::clone(&self.shared),
                    }
                    .mark_close_unwitnessed();
                }
                Err(error)
            }
        }
    }
    fn sync_parent(&self) -> Result<(), CoreError> {
        self.parent
            .sync_all()
            .map_err(|error| storage_io("parent fsync", error))
    }
}

struct InodeState {
    retained: RetainedId,
    bytes: u64,
    pins: usize,
    tombstone_synced: bool,
    releasing: bool,
}
#[derive(Clone, Copy)]
enum PendingKind {
    Create,
    Resize { inode: InodeKey },
}
#[derive(Clone, Copy)]
enum ControlOldDisposition {
    StillLinked,
    Replaced,
}
struct ControlCreatePending {
    target: Arc<HeldPath>,
    temporary: Arc<HeldPath>,
    reserved_end: u64,
    replaced_old: Option<(File, RetainedId, InodeObservation)>,
    recovering: bool,
}
struct PendingMutation {
    id: PendingId,
    kind: PendingKind,
    busy_inode: Option<InodeKey>,
    // A safe writer FD clone prevents same-process root handoff until the
    // caller transfers and closes its real writer during reconciliation.
    owned_pin: Option<File>,
    reserved_inode_slot: bool,
    born_inode: Option<InodeKey>,
    native: bool,
    retired_native: Option<InodeObservation>,
    external_writer_live: bool,
    safe_writer_fd: Option<RawFd>,
    control_create: Option<ControlCreatePending>,
}

/// Result of reconciling one physically retired Control creation.
#[derive(Debug, Eq, PartialEq)]
pub enum ControlCreateRecovery {
    RetainedTarget,
    Absent,
    RetainedTemporary {
        path: PathBuf,
        inode: InodeKey,
        bytes: u64,
    },
}
#[derive(Default)]
struct Tracking {
    handoff_frozen: bool,
    close_unwitnessed: bool,
    // False only after a complete, mutex-held scan observed no tombstones.
    may_have_tombstones: bool,
    paths_busy: BTreeSet<PathBuf>,
    directory_scopes: BTreeSet<PathBuf>,
    inodes_busy: BTreeSet<InodeKey>,
    opening_readers: BTreeMap<PathBuf, usize>,
    total_opening_readers: usize,
    total_pins: usize,
    inodes: BTreeMap<InodeKey, InodeState>,
    reserved_inode_slots: usize,
    pending: BTreeMap<PathBuf, PendingMutation>,
}
fn tracking_quiescent(state: &Tracking) -> bool {
    state.paths_busy.is_empty()
        && !state.close_unwitnessed
        && state.directory_scopes.is_empty()
        && state.inodes_busy.is_empty()
        && state.opening_readers.is_empty()
        && state.total_opening_readers == 0
        && state.total_pins == 0
        && state.pending.is_empty()
        && state.reserved_inode_slots == 0
        && state
            .inodes
            .values()
            .all(|inode| inode.pins == 0 && !inode.releasing)
}
fn quiescence_error(state: &Tracking, context: &str) -> CoreError {
    CoreError::NotReady(format!(
        "{context}: busy_paths={:?}, busy_inodes={:?}, directory_scopes={:?}, opening_readers={}, pins={}, pending_paths={:?}, reserved_inodes={}, close_unwitnessed={}, releasing_inodes={}",
        state.paths_busy,
        state.inodes_busy,
        state.directory_scopes,
        state.total_opening_readers,
        state.total_pins,
        state.pending.keys().collect::<Vec<_>>(),
        state.reserved_inode_slots,
        state.close_unwitnessed,
        state
            .inodes
            .values()
            .filter(|inode| inode.releasing)
            .count(),
    ))
}

struct ControlCreateClaim {
    shared: Arc<Shared>,
    target: PathBuf,
    id: PendingId,
}
impl Drop for ControlCreateClaim {
    fn drop(&mut self) {
        let Ok(mut state) = self.shared.state.lock() else {
            std::process::abort();
        };
        if let Some(pending) = state.pending.get_mut(&self.target)
            && pending.id == self.id
            && let Some(control) = pending.control_create.as_mut()
        {
            control.recovering = false;
        }
    }
}
// Inspect only anchored directory FDs and stat the leaf. Opening a native
// SQLite DB/WAL/SHM inode here could release its process locks on close.
fn pending_path_matches(root: &StateRootDirectoryV1, relative: &Path, born: InodeKey) -> bool {
    let mut components = relative.components().peekable();
    if components.peek().is_none() {
        return false;
    }
    let Ok(owned_root) = root.as_fd().try_clone_to_owned() else {
        return false;
    };
    let mut parent = File::from(owned_root);
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return false;
        };
        if components.peek().is_none() {
            let Ok(stat) = statat(&parent, Path::new(name), AtFlags::SYMLINK_NOFOLLOW) else {
                return false;
            };
            return FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
                && stat.st_uid == rustix::process::getuid().as_raw()
                && stat.st_mode & 0o022 == 0
                && crate::publication_inventory::checked_device_id(stat.st_dev)
                    .is_ok_and(|device| device == born.device)
                && stat.st_ino == born.inode;
        }
        let Ok(next) = openat(
            &parent,
            Path::new(name),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::DIRECTORY,
            Mode::empty(),
        ) else {
            return false;
        };
        parent = File::from(next);
    }
    false
}
fn directory_scope_busy(state: &Tracking, path: &Path) -> bool {
    state
        .directory_scopes
        .iter()
        .any(|scope| path.starts_with(scope) || scope.starts_with(path))
}
fn require_publication_open(state: &Tracking) -> Result<(), CoreError> {
    if state.close_unwitnessed {
        return Err(storage("admission", "owned FD close was ambiguous"));
    }
    if state.handoff_frozen {
        return Err(contract("publication root closed for handoff"));
    }
    Ok(())
}
struct Shared {
    lease: StateRootLease,
    root: StateRootDirectoryV1,
    authority: PublicationCapacity,
    state: Mutex<Tracking>,
    max_open_handles: usize,
    max_tracked_entries: usize,
    maintenance: Mutex<Option<MaintenanceReserve>>,
}

struct MaintenanceReserve {
    parent: Arc<PublicationAllowance>,
    bytes: u64,
    files: u64,
}
impl Drop for Shared {
    fn drop(&mut self) {
        // This is the last Shared owner. A pending safe writer can still own
        // an FD inside `state`; field destruction releases `lease` and `root`
        // before `state`, so a normal File::drop is not a retirement witness.
        let handoff_ready = self.root.require_current_v1().is_ok()
            && self.state.get_mut().is_ok_and(|state| {
                let pending_inodes: BTreeSet<_> = state
                    .pending
                    .values()
                    .filter_map(|pending| pending.busy_inode)
                    .collect();
                !state.close_unwitnessed
                    && state.directory_scopes.is_empty()
                    && state.opening_readers.is_empty()
                    && state.total_opening_readers == 0
                    && state.total_pins == 0
                    && state
                        .inodes
                        .values()
                        .all(|inode| inode.pins == 0 && !inode.releasing)
                    && state.paths_busy.len() == state.pending.len()
                    && state
                        .paths_busy
                        .iter()
                        .all(|path| state.pending.contains_key(path))
                    && state.pending.iter().all(|(path, pending)| {
                        pending.owned_pin.is_none()
                            && pending
                                .control_create
                                .as_ref()
                                .is_none_or(|control| control.replaced_old.is_none())
                            && !pending.external_writer_live
                            && pending.safe_writer_fd.is_none()
                            && pending
                                .born_inode
                                .is_some_and(|born| pending_path_matches(&self.root, path, born))
                    })
                    && state.reserved_inode_slots
                        == state
                            .pending
                            .values()
                            .filter(|pending| pending.reserved_inode_slot)
                            .count()
                    && state.inodes_busy == pending_inodes
            })
            && self.root.require_current_v1().is_ok();
        let Ok(mut registry) = custody_registry_or_abort().lock() else {
            std::process::abort();
        };
        let root = self.lease.state_root_identity_v1();
        let Some(entry) = registry.roots.get_mut(root) else {
            std::process::abort()
        };
        if !handoff_ready && entry.quarantine.is_none() {
            entry.quarantine = Some(self.lease.clone());
        }
        let Some(next) = entry.active.checked_sub(1) else {
            std::process::abort()
        };
        entry.active = next;
        if next == 0 && entry.quarantine.is_none() && registry.roots.remove(root).is_none() {
            std::process::abort();
        }
        drop(registry);
    }
}
#[derive(Clone)]
pub struct PublicationFileCapacity {
    shared: Arc<Shared>,
}
impl PublicationFileCapacity {
    pub(crate) fn prepare_empty_lock(
        &self,
        path: &Path,
        create_new: bool,
    ) -> Result<(), CoreError> {
        self.prepare_empty_lock_with_parent(path, create_new, None)
    }

    pub(crate) fn prepare_empty_lock_with_parent(
        &self,
        path: &Path,
        create_new: bool,
        parent: Option<Arc<PublicationAllowance>>,
    ) -> Result<(), CoreError> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt as _;

        self.ensure_private_parent_directories(path)?;
        if self.path_observation(path)?.is_some() {
            if create_new {
                return Err(existing_target("exclusive empty lock create"));
            }
            let mut opening = self.begin_native_reader_open(path)?;
            let file = File::open(path).map_err(|error| storage_io("open scratch lock", error))?;
            let reader = match InodeObservation::from_file(&file)
                .and_then(|observed| opening.opened_witness(observed))
            {
                Ok(reader) => reader,
                Err(error) => {
                    if close_owned_file(file).is_err() {
                        self.mark_close_unwitnessed();
                    }
                    return Err(error);
                }
            };
            if let Err(error) = close_owned_file(file) {
                self.mark_close_unwitnessed();
                return Err(storage_io("close scratch lock", error));
            }
            reader.retire()?;
            return Ok(());
        }
        // This empty regular file still consumes one inode slot. Admission
        // precedes create; the actual original FD supplies the final witness.
        let mut ticket = match parent {
            Some(parent) => self.begin_native_create_with_allowance(path, parent)?,
            None => self.begin_native_create(path)?,
        };
        ticket.creation_started()?;
        let nofollow = i32::try_from(OFlags::NOFOLLOW.bits()).map_err(|error| {
            CoreError::InvalidContract(format!("scratch nofollow flag: {error}"))
        })?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(nofollow)
            .open(path)
            .map_err(|error| storage_io("create scratch lock", error))?;
        let settled = (|| {
            ticket.observe_native_fd(InodeObservation::from_file(&file)?)?;
            file.sync_all()
                .map_err(|error| storage_io("sync scratch lock", error))?;
            let _retained =
                ticket.settle_after_native_sync(InodeObservation::from_file(&file)?, None)?;
            Ok::<(), CoreError>(())
        })();
        self.retire_empty_lock(&mut ticket, file, settled)
    }

    fn retire_empty_lock(
        &self,
        ticket: &mut FileMutation,
        file: File,
        settled: Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let observed = InodeObservation::from_file(&file);
        let closed = close_owned_file(file);
        if let Err(error) = closed {
            self.mark_close_unwitnessed();
            return Err(storage_io("close created scratch lock", error));
        }
        match settled {
            Ok(()) => ticket.confirmed_native_fd_retired(),
            Err(error) => {
                // Physical close is independent from durability. Retain the
                // full uncertain charge, but let bounded scratch cleanup use
                // this original FD's verified retirement.
                let retired = observed
                    .and_then(|observed| ticket.confirmed_unsynced_native_fd_retired(observed));
                if let Err(retirement_error) = retired {
                    self.mark_close_unwitnessed();
                    return Err(match error {
                        CoreError::StorageIo {
                            kind,
                            raw_os_error,
                            mut message,
                        } => {
                            write!(&mut message, "; retirement witness: {retirement_error}")
                                .map_err(|fmt_error| {
                                    storage("format retirement witness", fmt_error)
                                })?;
                            CoreError::StorageIo {
                                kind,
                                raw_os_error,
                                message,
                            }
                        }
                        other @ (CoreError::InvalidContract(_)
                        | CoreError::Typed { .. }
                        | CoreError::NotReady(_)
                        | CoreError::NotImplemented(_)
                        | CoreError::NotFound(_)
                        | CoreError::Storage(_)) => CoreError::Storage(format!(
                            "created scratch lock failed: {other}; retirement witness: {retirement_error}"
                        )),
                    });
                }
                Err(error)
            }
        }
    }

    pub fn new(
        lease: StateRootLease,
        authority: PublicationCapacity,
        max_open_handles: usize,
        max_tracked_entries: usize,
        max_process_roots: usize,
    ) -> Result<Arc<Self>, CoreError> {
        Self::new_internal(
            lease,
            authority,
            max_open_handles,
            max_tracked_entries,
            max_process_roots,
            None,
        )
    }

    pub(crate) fn new_with_maintenance(
        lease: StateRootLease,
        authority: PublicationCapacity,
        max_open_handles: usize,
        max_tracked_entries: usize,
        max_process_roots: usize,
        bytes: u64,
        files: u64,
    ) -> Result<Arc<Self>, CoreError> {
        if bytes == 0 || files == 0 {
            return Err(contract("maintenance reserve must be positive"));
        }
        Self::new_internal(
            lease,
            authority,
            max_open_handles,
            max_tracked_entries,
            max_process_roots,
            Some((bytes, files)),
        )
    }

    fn new_internal(
        lease: StateRootLease,
        authority: PublicationCapacity,
        max_open_handles: usize,
        max_tracked_entries: usize,
        max_process_roots: usize,
        reserve: Option<(u64, u64)>,
    ) -> Result<Arc<Self>, CoreError> {
        if max_open_handles == 0 || max_tracked_entries == 0 {
            return Err(contract("registry bounds must be positive"));
        }
        let root = lease.try_clone_root_directory_v1()?;
        let maintenance = reserve
            .map(|(bytes, files)| {
                authority
                    .reserve_publication(bytes, files)
                    .map(|parent| MaintenanceReserve {
                        parent: Arc::new(parent),
                        bytes,
                        files,
                    })
            })
            .transpose()?;
        register_root_custody(lease.state_root_identity_v1(), max_process_roots)?;
        Ok(Arc::new(Self {
            shared: Arc::new(Shared {
                lease,
                root,
                authority,
                state: Mutex::new(Tracking {
                    may_have_tombstones: true,
                    ..Tracking::default()
                }),
                max_open_handles,
                max_tracked_entries,
                maintenance: Mutex::new(maintenance),
            }),
        }))
    }

    pub(crate) fn maintenance_parent(
        &self,
    ) -> Result<Option<Arc<PublicationAllowance>>, CoreError> {
        self.shared.root.require_current_v1()?;
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        let maintenance = lock(&self.shared.maintenance)?;
        Ok(maintenance
            .as_ref()
            .map(|reserve| Arc::clone(&reserve.parent)))
    }

    pub(crate) fn refresh_maintenance_reserve(&self) -> Result<(), CoreError> {
        self.shared.root.require_current_v1()?;
        let state = lock(&self.shared.state)?;
        self.refresh_maintenance_reserve_locked(&state)
    }

    fn refresh_maintenance_reserve_locked(&self, state: &Tracking) -> Result<(), CoreError> {
        require_publication_open(state)?;
        let maintenance = lock(&self.shared.maintenance)?;
        maintenance.as_ref().map_or(Ok(()), |reserve| {
            reserve
                .parent
                .resize_available(reserve.bytes, reserve.files)
        })
    }

    pub(crate) fn refill_maintenance_reserve(&self) -> Result<(), CoreError> {
        self.shared.root.require_current_v1()?;
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        let maintenance = lock(&self.shared.maintenance)?;
        let result = maintenance.as_ref().map_or(Ok(()), |reserve| {
            reserve
                .parent
                .refill_available_up_to(reserve.bytes, reserve.files)
        });
        drop(maintenance);
        drop(state);
        result
    }

    pub(crate) fn maintenance_available(&self) -> Result<(u64, u64), CoreError> {
        self.shared.root.require_current_v1()?;
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        let maintenance = lock(&self.shared.maintenance)?;
        maintenance
            .as_ref()
            .map_or(Ok((0, 0)), |reserve| reserve.parent.available_capacity())
    }

    /// Retire the original maintenance parent only after the entire binder and
    /// Core authority have no other live or uncertain custody. New admission
    /// is frozen under the namespace lock before the last parent is dropped.
    pub fn retire_maintenance_reserve_checked(&self) -> Result<(), CoreError> {
        self.shared.root.require_current_v1()?;
        let mut state = lock(&self.shared.state)?;
        if !tracking_quiescent(&state) {
            return Err(quiescence_error(
                &state,
                "publication binder is not quiescent for maintenance retirement",
            ));
        }
        let mut maintenance = lock(&self.shared.maintenance)?;
        if maintenance.as_ref().is_some_and(|reserve| {
            Arc::strong_count(&reserve.parent) != 1 || Arc::weak_count(&reserve.parent) != 0
        }) {
            return Err(CoreError::NotReady(
                "maintenance parent still has an external owner".into(),
            ));
        }
        let snapshot = self.shared.authority.snapshot()?;
        let expected_allowances = u64::from(maintenance.is_some());
        if snapshot.active_allowances != expected_allowances
            || snapshot.active_tickets != 0
            || snapshot.uncertain_pending != 0
            || snapshot.poisoned
        {
            return Err(CoreError::NotReady(format!(
                "publication authority has live or uncertain custody: expected_allowances={expected_allowances}, capacity={snapshot:?}"
            )));
        }
        self.shared.root.require_current_v1()?;
        state.handoff_frozen = true;
        drop(maintenance.take());
        drop(maintenance);
        drop(state);
        Ok(())
    }
    #[must_use]
    pub fn max_open_handles(&self) -> usize {
        self.shared.max_open_handles
    }
    /// Logical regular-file charge and pending state of this exact authority.
    pub fn capacity_snapshot(
        &self,
    ) -> Result<quanta_index_core::publication_capacity::CapacitySnapshot, CoreError> {
        self.shared.authority.snapshot()
    }
    #[must_use]
    pub fn state_root(&self) -> &Path {
        self.shared.lease.state_root_identity_v1()
    }
    /// Authenticate the held state-root lease without creating a child path.
    pub fn validate_current_state_root(&self) -> Result<(), CoreError> {
        self.shared.root.require_current_v1()?;
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)
    }
    /// Authenticate a supplied parent before creating any publication path.
    /// A zero top-up also rejects a closed or unhealthy parent without charging it.
    pub fn validate_held_allowance(
        &self,
        allowance: &PublicationAllowance,
    ) -> Result<(), CoreError> {
        self.shared.root.require_current_v1()?;
        if !allowance.belongs_to(&self.shared.authority) {
            return Err(contract("supplied allowance belongs to another authority"));
        }
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        allowance.top_up(0, 0)?;
        drop(state);
        Ok(())
    }
    /// Reserve one explicitly bounded parent from this binder's sealed ledger.
    /// The caller must subdivide it through supplied-parent writer paths;
    /// this method does not estimate an output or grant a growth top-up.
    pub fn reserve_publication_allowance(
        &self,
        max_bytes: u64,
        max_files: u64,
    ) -> Result<Arc<PublicationAllowance>, CoreError> {
        if max_bytes == 0 || max_files == 0 {
            return Err(contract(
                "publication allowance bytes and files must be positive",
            ));
        }
        self.shared.root.require_current_v1()?;
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        self.refresh_maintenance_reserve_locked(&state)?;
        let allowance = self
            .shared
            .authority
            .reserve_publication(max_bytes, max_files)
            .map(Arc::new)?;
        drop(state);
        Ok(allowance)
    }

    /// Reserve only uncommitted growth of a boot-inventoried file. Its inode
    /// slot is already charged, so claiming another slot would double count it.
    pub fn reserve_existing_growth_allowance(
        &self,
        max_growth_bytes: u64,
    ) -> Result<Arc<PublicationAllowance>, CoreError> {
        if max_growth_bytes == 0 {
            return Err(contract("existing file growth allowance must be positive"));
        }
        self.shared.root.require_current_v1()?;
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        self.refresh_maintenance_reserve_locked(&state)?;
        let allowance = self
            .shared
            .authority
            .reserve_publication(max_growth_bytes, 0)
            .map(Arc::new)?;
        drop(state);
        Ok(allowance)
    }

    /// Reserve one maintenance output from this binder's sealed authority.
    pub fn reserve_maintenance_allowance(
        &self,
        max_bytes: u64,
    ) -> Result<Arc<PublicationAllowance>, CoreError> {
        self.reserve_publication_allowance(max_bytes, 1)
    }
    /// Create only missing ancestors below the held private root. Directory
    /// blocks are outside the regular-file byte/slot contract; every created
    /// directory and its parent are synchronized before a child publication.
    pub fn ensure_private_parent_directories(&self, path: &Path) -> Result<(), CoreError> {
        self.shared.root.require_current_v1()?;
        let relative = self.relative(path)?;
        let namespace_guard = lock(&self.shared.state)?;
        require_publication_open(&namespace_guard)?;
        if directory_scope_busy(&namespace_guard, &relative) {
            return Err(capacity(
                "parent directory namespace is being moved or deleted",
            ));
        }
        let mut parent = File::from(
            self.shared
                .root
                .as_fd()
                .try_clone_to_owned()
                .map_err(|error| storage_io("clone root directory FD", error))?,
        );
        let mut parts = relative.components().peekable();
        while let Some(component) = parts.next() {
            if parts.peek().is_none() {
                break;
            }
            let Component::Normal(name) = component else {
                return Err(contract("invalid directory component"));
            };
            let child_name = Path::new(name);
            let mut created = false;
            let child = match openat(
                &parent,
                child_name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::DIRECTORY,
                Mode::empty(),
            ) {
                Ok(opened) => File::from(opened),
                Err(rustix::io::Errno::NOENT) => {
                    match mkdirat(&parent, child_name, Mode::from_raw_mode(0o700)) {
                        Ok(()) => created = true,
                        Err(rustix::io::Errno::EXIST) => {}
                        Err(error) => return Err(storage_io("mkdirat private parent", error)),
                    }
                    File::from(
                        openat(
                            &parent,
                            child_name,
                            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::DIRECTORY,
                            Mode::empty(),
                        )
                        .map_err(|error| storage_io("open created parent", error))?,
                    )
                }
                Err(error) => return Err(storage_io("open private parent", error)),
            };
            let metadata = child
                .metadata()
                .map_err(|error| storage_io("fstat private parent", error))?;
            if !metadata.is_dir()
                || metadata.uid() != rustix::process::getuid().as_raw()
                || metadata.mode() & 0o022 != 0
            {
                return Err(contract(
                    "private parent directory has insecure identity or mode",
                ));
            }
            if created {
                child
                    .sync_all()
                    .map_err(|error| storage_io("new parent fsync", error))?;
                parent
                    .sync_all()
                    .map_err(|error| storage_io("parent of new directory fsync", error))?;
            }
            parent = child;
        }
        self.shared.root.require_current_v1()?;
        // Hold the namespace mutex through all parent creation and fsyncs.
        drop(namespace_guard);
        Ok(())
    }
    /// A retired service may release root custody only after every mutation,
    /// open guard, and FD pin has reached a physical completion witness.
    pub fn is_quiescent(&self) -> Result<bool, CoreError> {
        let state = lock(&self.shared.state)?;
        Ok(tracking_quiescent(&state))
    }

    /// Refuse a lifecycle transition with the original outstanding custody.
    pub(crate) fn require_quiescent(&self, context: &str) -> Result<(), CoreError> {
        let state = lock(&self.shared.state)?;
        if !tracking_quiescent(&state) {
            return Err(quiescence_error(&state, context));
        }
        drop(state);
        Ok(())
    }

    /// Permit a closed service to surrender the root to a new boot inventory
    /// after every live FD/task guard is gone. Only dropped, started Core
    /// tickets may remain. The new root owner re-inventories their physical
    /// files; this does not refund their old in-memory charges.
    pub fn is_handoff_ready_after_retirement(&self) -> Result<bool, CoreError> {
        self.shared.root.require_current_v1()?;
        let mut state = lock(&self.shared.state)?;
        let pending_inodes: BTreeSet<_> = state
            .pending
            .values()
            .filter_map(|pending| pending.busy_inode)
            .collect();
        let ready = state.directory_scopes.is_empty()
            && !state.close_unwitnessed
            && state.opening_readers.is_empty()
            && state.total_opening_readers == 0
            && state.total_pins == 0
            && state
                .inodes
                .values()
                .all(|inode| inode.pins == 0 && !inode.releasing)
            && state.paths_busy.len() == state.pending.len()
            && state
                .paths_busy
                .iter()
                .all(|path| state.pending.contains_key(path))
            && state.pending.iter().all(|(path, pending)| {
                pending.owned_pin.is_none()
                    && !pending.external_writer_live
                    && pending.safe_writer_fd.is_none()
                    && pending
                        .born_inode
                        .is_some_and(|born| pending_path_matches(&self.shared.root, path, born))
            })
            && state.reserved_inode_slots
                == state
                    .pending
                    .values()
                    .filter(|pending| pending.reserved_inode_slot)
                    .count()
            && state.inodes_busy == pending_inodes;
        if ready {
            // Close the race between a quiescent snapshot and the router's
            // release of the root lease. All begin operations must refuse.
            state.handoff_frozen = true;
        }
        drop(state);
        if !ready {
            return Ok(false);
        }
        self.shared.root.require_current_v1()?;
        Ok(true)
    }

    fn relative(&self, path: &Path) -> Result<PathBuf, CoreError> {
        let relative = if path.is_absolute() {
            path.strip_prefix(self.state_root()).map_err(|error| {
                CoreError::InvalidContract(format!(
                    "publication file capacity: absolute pathname escapes leased root: {error}"
                ))
            })?
        } else {
            path
        };
        let mut clean = PathBuf::new();
        let mut count = 0_usize;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(contract("non-normal relative path component"));
            };
            count = count
                .checked_add(1)
                .ok_or_else(|| capacity("path component overflow"))?;
            if count > self.shared.max_tracked_entries {
                return Err(capacity("path depth exceeds tracked-entry bound"));
            }
            clean.push(name);
        }
        if count == 0 {
            return Err(contract("empty file pathname"));
        }
        Ok(clean)
    }
    /// Validate a list prefix against this live root without following any
    /// existing component. An absent suffix is still a valid empty prefix.
    /// This is a point-in-time namespace check, not aggregate listing admission.
    pub fn validate_list_scope(&self, path: &Path) -> Result<(), CoreError> {
        self.shared.root.require_current_v1()?;
        if path.as_os_str() == self.state_root().as_os_str() {
            return Ok(());
        }
        let relative = self.relative(path)?;
        let canonical = if path.is_absolute() {
            self.state_root().join(&relative)
        } else {
            relative.clone()
        };
        if path.as_os_str() != canonical.as_os_str() {
            return Err(contract("list prefix has noncanonical components"));
        }
        let mut held = File::from(
            self.shared
                .root
                .as_fd()
                .try_clone_to_owned()
                .map_err(|error| storage_io("clone list root FD", error))?,
        );
        let result = (|| {
            let mut components = relative.components().peekable();
            while let Some(component) = components.next() {
                let Component::Normal(name) = component else {
                    return Err(contract("list prefix has non-normal component"));
                };
                let stat = match statat(&held, Path::new(name), AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(stat) => stat,
                    Err(rustix::io::Errno::NOENT) => {
                        return self.shared.root.require_current_v1();
                    }
                    Err(error) => return Err(storage_io("stat list prefix component", error)),
                };
                let kind = FileType::from_raw_mode(stat.st_mode);
                let ancestor = components.peek().is_some();
                if kind != FileType::Directory && (ancestor || kind != FileType::RegularFile) {
                    return Err(contract(
                        "list prefix contains a non-directory or unsafe leaf",
                    ));
                }
                if stat.st_uid != rustix::process::getuid().as_raw() || stat.st_mode & 0o022 != 0 {
                    return Err(contract("list prefix component has insecure owner or mode"));
                }
                if ancestor {
                    let next = File::from(
                        openat(
                            &held,
                            Path::new(name),
                            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::DIRECTORY,
                            Mode::empty(),
                        )
                        .map_err(|error| storage_io("open list prefix directory", error))?,
                    );
                    let checked = (|| {
                        let metadata = next
                            .metadata()
                            .map_err(|error| storage_io("stat opened list directory", error))?;
                        if !metadata.is_dir()
                            || metadata.dev()
                                != crate::publication_inventory::checked_device_id(stat.st_dev)
                                    .map_err(|error| storage("list directory device", error))?
                            || metadata.ino() != stat.st_ino
                            || metadata.uid() != rustix::process::getuid().as_raw()
                            || metadata.mode() & 0o022 != 0
                        {
                            return Err(contract("opened list directory identity or mode changed"));
                        }
                        Ok(())
                    })();
                    if let Err(error) = checked {
                        if let Err(close_error) = close_owned_file(next) {
                            self.mark_close_unwitnessed();
                            return Err(storage_io("close rejected list directory", close_error));
                        }
                        return Err(error);
                    }
                    let previous = std::mem::replace(&mut held, next);
                    if let Err(error) = close_owned_file(previous) {
                        self.mark_close_unwitnessed();
                        return Err(storage_io("close previous list directory", error));
                    }
                }
            }
            self.shared.root.require_current_v1()
        })();
        if let Err(error) = close_owned_file(held) {
            self.mark_close_unwitnessed();
            return Err(storage_io("close list directory", error));
        }
        result
    }
    fn anchor(&self, path: &Path) -> Result<Anchor, CoreError> {
        self.anchor_with_missing(path, MissingDisposition::Storage)
    }
    fn anchor_with_missing(
        &self,
        path: &Path,
        missing: MissingDisposition,
    ) -> Result<Anchor, CoreError> {
        self.shared.root.require_current_v1()?;
        let relative = self.relative(path)?;
        let mut components = relative.components();
        let mut parent = File::from(
            self.shared
                .root
                .as_fd()
                .try_clone_to_owned()
                .map_err(|error| storage_io("clone leased root FD", error))?,
        );
        let mut name = None;
        while let Some(component) = components.next() {
            let Component::Normal(part) = component else {
                return Err(contract("invalid component"));
            };
            if components.as_path().as_os_str().is_empty() {
                name = Some(part.to_os_string());
                break;
            }
            let child = openat(
                &parent,
                Path::new(part),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::DIRECTORY,
                Mode::empty(),
            )
            .map_err(|error| path_error("open parent nofollow", error, missing))?;
            parent = File::from(child);
        }
        self.shared.root.require_current_v1()?;
        Ok(Anchor {
            shared: Arc::clone(&self.shared),
            parent,
            name: name.ok_or_else(|| contract("missing basename"))?,
            relative,
        })
    }
    fn claim_path(
        &self,
        path: &Path,
        supplied: Option<Arc<PublicationAllowance>>,
    ) -> Result<(Anchor, Arc<PublicationAllowance>), CoreError> {
        if let Some(allowance) = supplied.as_ref() {
            self.validate_held_allowance(allowance)?;
        }
        let anchor = self.anchor(path)?;
        {
            let mut state = lock(&self.shared.state)?;
            require_publication_open(&state)?;
            if state.paths_busy.contains(&anchor.relative)
                || state.opening_readers.contains_key(&anchor.relative)
                || directory_scope_busy(&state, &anchor.relative)
            {
                return Err(capacity("path has an in-flight operation or reader open"));
            }
            if state.paths_busy.len() >= self.shared.max_tracked_entries {
                return Err(capacity("tracked path ceiling exceeded"));
            }
            if supplied.is_none() {
                self.refresh_maintenance_reserve_locked(&state)?;
            }
            if !state.paths_busy.insert(anchor.relative.clone()) {
                return Err(contract("claimed path was already busy"));
            }
        }
        let allocation = supplied.map_or_else(
            || {
                self.shared
                    .authority
                    .reserve_publication(0, 0)
                    .map(Arc::new)
            },
            Ok,
        );
        match allocation {
            Ok(allowance) => Ok((anchor, allowance)),
            Err(error) => {
                if !lock(&self.shared.state)?
                    .paths_busy
                    .remove(&anchor.relative)
                {
                    return Err(contract("failed allocation lost its path claim"));
                }
                Err(error)
            }
        }
    }
    fn release_path(&self, relative: &Path) {
        let removed = self
            .shared
            .state
            .lock()
            .is_ok_and(|mut state| state.paths_busy.remove(relative));
        if !removed {
            self.mark_close_unwitnessed();
        }
    }
    fn release_busy_inode(&self, inode: InodeKey) {
        let removed = self
            .shared
            .state
            .lock()
            .is_ok_and(|mut state| state.inodes_busy.remove(&inode));
        if !removed {
            self.mark_close_unwitnessed();
        }
    }
    fn reserve_inode_slot(&self) -> Result<(), CoreError> {
        let mut state = lock(&self.shared.state)?;
        if state
            .inodes
            .len()
            .saturating_add(state.reserved_inode_slots)
            >= self.shared.max_tracked_entries
        {
            return Err(capacity("retirement inode registry ceiling exceeded"));
        }
        state.reserved_inode_slots = state
            .reserved_inode_slots
            .checked_add(1)
            .ok_or_else(|| capacity("retirement inode slot overflow"))?;
        drop(state);
        Ok(())
    }
    fn release_inode_slot(&self) {
        let released = self.shared.state.lock().is_ok_and(|mut state| {
            state
                .reserved_inode_slots
                .checked_sub(1)
                .is_some_and(|next| {
                    state.reserved_inode_slots = next;
                    true
                })
        });
        if !released {
            self.mark_close_unwitnessed();
        }
    }
    fn release_control_temp_scope(&self, temporary: &Path) {
        let released = self
            .shared
            .state
            .lock()
            .is_ok_and(|mut state| state.directory_scopes.remove(temporary));
        if !released {
            self.mark_close_unwitnessed();
        }
    }
    pub(crate) fn mark_close_unwitnessed(&self) {
        {
            let Ok(mut registry) = custody_registry_or_abort().lock() else {
                std::process::abort();
            };
            let root = self.shared.lease.state_root_identity_v1();
            let Some(entry) = registry.roots.get_mut(root) else {
                std::process::abort()
            };
            if entry.quarantine.is_none() {
                entry.quarantine = Some(self.shared.lease.clone());
            }
            drop(registry);
        }
        if let Ok(mut state) = self.shared.state.lock() {
            state.close_unwitnessed = true;
            state.handoff_frozen = true;
        }
    }
    fn track_retained(
        &self,
        retained: RetainedId,
        observed: InodeObservation,
    ) -> Result<(), CoreError> {
        self.track_retained_inner(retained, observed, false)
    }
    fn track_retained_reserved(
        &self,
        retained: RetainedId,
        observed: InodeObservation,
    ) -> Result<(), CoreError> {
        self.track_retained_inner(retained, observed, true)
    }
    fn track_retained_inner(
        &self,
        retained: RetainedId,
        observed: InodeObservation,
        reserved: bool,
    ) -> Result<(), CoreError> {
        if retained.inode() != observed.inode {
            return Err(contract("retained inode mismatch"));
        }
        let mut state = lock(&self.shared.state)?;
        if reserved && state.reserved_inode_slots == 0 {
            return Err(contract("retirement inode slot reservation missing"));
        }
        if let Some(existing) = state.inodes.get_mut(&observed.inode) {
            if existing.tombstone_synced || existing.releasing {
                return Err(contract("retained inode is retiring"));
            }
            existing.retained = retained;
            existing.bytes = observed.bytes;
        } else {
            if !reserved
                && state
                    .inodes
                    .len()
                    .saturating_add(state.reserved_inode_slots)
                    >= self.shared.max_tracked_entries
            {
                return Err(capacity("tracked inode ceiling exceeded"));
            }
            if state
                .inodes
                .insert(
                    observed.inode,
                    InodeState {
                        retained,
                        bytes: observed.bytes,
                        pins: 0,
                        tombstone_synced: false,
                        releasing: false,
                    },
                )
                .is_some()
            {
                return Err(contract("tracked inode was replaced"));
            }
        }
        if reserved {
            state.reserved_inode_slots = state
                .reserved_inode_slots
                .checked_sub(1)
                .ok_or_else(|| contract("retirement inode slot underflow"))?;
        }
        drop(state);
        Ok(())
    }
    fn pin(&self, observed: InodeObservation, file: File) -> Result<ReaderLease, CoreError> {
        let mut file = Some(file);
        let result = (|| {
            let retained = self
                .shared
                .authority
                .retained_inode(observed.inode, observed.bytes)?
                .ok_or_else(|| contract("opened FD inode absent from sealed authority"))?;
            self.track_retained(retained, observed)?;
            let mut state = lock(&self.shared.state)?;
            if state.total_pins >= self.shared.max_open_handles {
                return Err(capacity("owned FD pin ceiling exceeded"));
            }
            let inode = state
                .inodes
                .get_mut(&observed.inode)
                .ok_or_else(|| contract("pin inode disappeared"))?;
            inode.pins = inode
                .pins
                .checked_add(1)
                .ok_or_else(|| capacity("pin count overflow"))?;
            state.total_pins = state
                .total_pins
                .checked_add(1)
                .ok_or_else(|| capacity("total pin count overflow"))?;
            drop(state);
            Ok(ReaderLease {
                shared: Arc::clone(&self.shared),
                inode: observed.inode,
                file: file.take(),
                actual: None,
                live: true,
            })
        })();
        if result.is_err()
            && let Some(file) = file.take()
            && close_owned_file(file).is_err()
        {
            self.mark_close_unwitnessed();
        }
        result
    }
    pub fn path_observation(&self, path: &Path) -> Result<Option<InodeObservation>, CoreError> {
        self.anchor(path)?.stat()
    }
    /// Classify one existing namespace source without following its final component.
    /// The subsequent move admission must authenticate the source again.
    pub(crate) fn namespace_source_is_directory(&self, path: &Path) -> Result<bool, CoreError> {
        let anchor = self.anchor_with_missing(path, MissingDisposition::NotFound)?;
        let result = (|| {
            let stat = statat(
                &anchor.parent,
                Path::new(&anchor.name),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|error| {
                path_error("stat namespace source", error, MissingDisposition::NotFound)
            })?;
            let kind = FileType::from_raw_mode(stat.st_mode);
            if kind != FileType::RegularFile && kind != FileType::Directory {
                return Err(contract(
                    "namespace source is neither regular file nor directory",
                ));
            }
            if stat.st_uid != rustix::process::getuid().as_raw() || stat.st_mode & 0o022 != 0 {
                return Err(contract(
                    "namespace source has insecure owner or write mode",
                ));
            }
            self.shared.root.require_current_v1()?;
            Ok(kind == FileType::Directory)
        })();
        if let Err(error) = close_owned_file(anchor.parent) {
            self.mark_close_unwitnessed();
            return Err(storage_io("close namespace source parent", error));
        }
        result
    }
    /// Sync an absent pathname without refunding or discarding an owned operation.
    pub fn sync_confirmed_absence(
        &self,
        path: &Path,
        expected_parent: InodeKey,
    ) -> Result<(), CoreError> {
        let anchor = self.anchor(path)?;
        let parent = anchor
            .parent
            .metadata()
            .map_err(|error| storage_io("absence parent metadata", error))?;
        if expected_parent
            != (InodeKey {
                device: parent.dev(),
                inode: parent.ino(),
            })
        {
            return Err(contract("absence parent incarnation differs"));
        }
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        if state.paths_busy.contains(&anchor.relative)
            || directory_scope_busy(&state, &anchor.relative)
        {
            return Err(capacity("absence has an owned physical operation"));
        }
        drop(state);
        if anchor.stat()?.is_some() {
            return Err(contract("absence target still exists"));
        }
        anchor.sync_parent()?;
        self.shared.root.require_current_v1()?;
        if anchor.stat()?.is_some() {
            return Err(contract("absence target appeared during sync"));
        }
        Ok(())
    }
    fn pin_native_witness(
        &self,
        path: &Path,
        observed: InodeObservation,
    ) -> Result<NativeReaderLease, CoreError> {
        let anchor = self.anchor(path)?;
        if anchor.stat()? != Some(observed) {
            return Err(contract("native pin witness and anchored path differ"));
        }
        let retained = self
            .shared
            .authority
            .retained_inode(observed.inode, observed.bytes)?
            .ok_or_else(|| contract("native pin inode absent from sealed authority"))?;
        self.track_retained(retained, observed)?;
        let mut state = lock(&self.shared.state)?;
        if state.total_pins >= self.shared.max_open_handles {
            return Err(capacity("native pin ceiling exceeded"));
        }
        let entry = state
            .inodes
            .get_mut(&observed.inode)
            .ok_or_else(|| contract("native pin inode disappeared"))?;
        entry.pins = entry
            .pins
            .checked_add(1)
            .ok_or_else(|| capacity("native pin overflow"))?;
        state.total_pins = state
            .total_pins
            .checked_add(1)
            .ok_or_else(|| capacity("total native pin count overflow"))?;
        drop(state);
        Ok(NativeReaderLease {
            shared: Arc::clone(&self.shared),
            inode: observed.inode,
            live: true,
        })
    }
    pub fn begin_native_reader_open(&self, path: &Path) -> Result<NativeOpenGuard, CoreError> {
        let anchor = self.anchor(path)?;
        {
            let mut state = lock(&self.shared.state)?;
            require_publication_open(&state)?;
            if state.paths_busy.contains(&anchor.relative)
                || directory_scope_busy(&state, &anchor.relative)
                || state
                    .total_pins
                    .checked_add(state.total_opening_readers)
                    .is_none_or(|handles| handles >= self.shared.max_open_handles)
                || (state.opening_readers.len() >= self.shared.max_tracked_entries
                    && !state.opening_readers.contains_key(&anchor.relative))
            {
                return Err(capacity("native reader-open registry busy or full"));
            }
            let count = state
                .opening_readers
                .entry(anchor.relative.clone())
                .or_insert(0);
            *count = count
                .checked_add(1)
                .ok_or_else(|| capacity("native reader-open overflow"))?;
            state.total_opening_readers = state
                .total_opening_readers
                .checked_add(1)
                .ok_or_else(|| capacity("native opening-reader count overflow"))?;
        }
        let expected = (|| {
            let observed = anchor
                .stat()?
                .ok_or_else(|| contract("native reader pathname absent"))?;
            let retained = self
                .shared
                .authority
                .retained_inode(observed.inode, observed.bytes)?
                .ok_or_else(|| {
                    contract(&format!(
                        "native reader inode absent from sealed authority: path={}, observed={observed:?}",
                        anchor.relative.display()
                    ))
                })?;
            self.track_retained(retained, observed)?;
            Ok(observed)
        })();
        let expected = match expected {
            Ok(expected) => expected,
            Err(error) => {
                self.release_opening(&anchor.relative);
                return Err(error);
            }
        };
        Ok(NativeOpenGuard {
            binder: self.clone(),
            anchor,
            expected,
            finished: false,
        })
    }
    /// Lower VFS delete-on-close may unlink without calling xDelete. Only an
    /// actual retained FD plus an absent anchored path and zero nlink can
    /// witness that last pathname reference before retirement.
    pub fn confirm_native_unlinked_lease(
        &self,
        path: &Path,
        lease: &NativeReaderLease,
        observed: InodeObservation,
    ) -> Result<(), CoreError> {
        let anchor = self.anchor(path)?;
        if anchor.stat()?.is_some() {
            return Err(contract("native delete-on-close pathname remains"));
        }
        if observed.inode != lease.inode() || observed.links != 0 {
            return Err(contract("native unlink lacks zero-link FD witness"));
        }
        anchor.sync_parent()?;
        self.shared.root.require_current_v1()?;
        let mut state = lock(&self.shared.state)?;
        let entry = state
            .inodes
            .get_mut(&observed.inode)
            .ok_or_else(|| contract("native unlinked inode is not tracked"))?;
        if entry.bytes != observed.bytes || entry.releasing {
            return Err(contract("native unlinked inode width changed"));
        }
        entry.tombstone_synced = true;
        state.may_have_tombstones = true;
        drop(state);
        Ok(())
    }
    pub fn begin_reader_open(&self, path: &Path) -> Result<OpenGuard, CoreError> {
        let anchor = self.anchor_with_missing(path, MissingDisposition::NotFound)?;
        let mut state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        if state.paths_busy.contains(&anchor.relative)
            || directory_scope_busy(&state, &anchor.relative)
        {
            return Err(capacity("path mutation in progress"));
        }
        if state.opening_readers.len() >= self.shared.max_tracked_entries
            && !state.opening_readers.contains_key(&anchor.relative)
        {
            return Err(capacity("reader-open registry ceiling exceeded"));
        }
        if state
            .total_pins
            .checked_add(state.total_opening_readers)
            .is_none_or(|handles| handles >= self.shared.max_open_handles)
        {
            return Err(capacity("reader-open ceiling exceeded"));
        }
        let entry = state
            .opening_readers
            .entry(anchor.relative.clone())
            .or_insert(0);
        *entry = entry
            .checked_add(1)
            .ok_or_else(|| capacity("reader-open count overflow"))?;
        state.total_opening_readers = state
            .total_opening_readers
            .checked_add(1)
            .ok_or_else(|| capacity("opening-reader count overflow"))?;
        drop(state);
        let mut candidate = None;
        let result = (|| {
            candidate = Some(anchor.open_reader()?);
            let expected = InodeObservation::from_file(
                candidate
                    .as_ref()
                    .ok_or_else(|| contract("reader FD absent"))?,
            )?;
            let retained = self
                .shared
                .authority
                .retained_inode(expected.inode, expected.bytes)?
                .ok_or_else(|| contract("reader inode absent from sealed authority"))?;
            self.track_retained(retained, expected)?;
            Ok(OpenGuard {
                binder: self.clone(),
                anchor,
                expected,
                file: candidate.take(),
                finished: false,
            })
        })();
        if result.is_err() {
            if let Some(file) = candidate.take()
                && let Err(error) = close_owned_file(file)
            {
                self.mark_close_unwitnessed();
                self.release_opening(path);
                return Err(storage_io("close rejected opening FD", error));
            }
            self.release_opening(path);
        }
        result
    }
    fn release_opening(&self, path: &Path) {
        let released = (|| {
            let relative = self.relative(path)?;
            let mut state = lock(&self.shared.state)?;
            let count = state
                .opening_readers
                .get_mut(&relative)
                .ok_or_else(|| contract("opening reader path missing"))?;
            *count = count
                .checked_sub(1)
                .ok_or_else(|| contract("opening reader count underflow"))?;
            let remove = *count == 0;
            state.total_opening_readers = state
                .total_opening_readers
                .checked_sub(1)
                .ok_or_else(|| contract("total opening reader count underflow"))?;
            if remove && state.opening_readers.remove(&relative).is_none() {
                return Err(contract("opening reader path disappeared"));
            }
            drop(state);
            Ok::<(), CoreError>(())
        })();
        if released.is_err() {
            self.mark_close_unwitnessed();
        }
    }
    pub fn begin_create(&self, final_path: &Path) -> Result<FileMutation, CoreError> {
        self.begin_create_with_bound(final_path, 1, false, None, None)
    }
    pub fn begin_native_create(&self, final_path: &Path) -> Result<FileMutation, CoreError> {
        self.begin_create_with_bound(final_path, 1, true, None, None)
    }
    /// Subdivide an already-held parent before `SQLite` creates a native file.
    /// Further native growth remains bounded by this same parent.
    pub fn begin_native_create_with_allowance(
        &self,
        final_path: &Path,
        allowance: Arc<PublicationAllowance>,
    ) -> Result<FileMutation, CoreError> {
        self.begin_create_with_bound(final_path, 1, true, Some(allowance), None)
    }
    /// Subdivide an already charged publication allowance. The caller keeps
    /// its Arc until all child files have settled; this method never top-ups.
    pub fn begin_create_with_allowance(
        &self,
        final_path: &Path,
        max_bytes: u64,
        allowance: Arc<PublicationAllowance>,
    ) -> Result<FileMutation, CoreError> {
        self.begin_create_with_bound(final_path, max_bytes, false, Some(allowance), None)
    }
    /// Reserve a safe create-once file that is visible before termination.
    ///
    /// The caller must use anchored `O_EXCL` and register its original FD with
    /// `created_visible` before payload writes. Growth subdivides this parent.
    pub fn begin_visible_create_with_allowance(
        &self,
        final_path: &Path,
        allowance: Arc<PublicationAllowance>,
    ) -> Result<FileMutation, CoreError> {
        let mutation = self.begin_create_with_bound(final_path, 1, false, Some(allowance), None)?;
        if mutation.replaced_old.is_some() {
            return Err(existing_target("visible create"));
        }
        Ok(mutation)
    }
    /// Bind an already-subdivided child to this file's physical lifecycle.
    /// The exact child is authenticated before any path is claimed or mutated.
    pub fn begin_create_with_reserved_output(
        &self,
        final_path: &Path,
        max_bytes: u64,
        allowance: Arc<PublicationAllowance>,
        ticket: OutputTicket,
    ) -> Result<FileMutation, CoreError> {
        self.begin_create_with_bound(final_path, max_bytes, false, Some(allowance), Some(ticket))
    }
    fn begin_create_with_bound(
        &self,
        final_path: &Path,
        bound: u64,
        native: bool,
        supplied: Option<Arc<PublicationAllowance>>,
        mut reserved_ticket: Option<OutputTicket>,
    ) -> Result<FileMutation, CoreError> {
        if bound == 0 {
            return Err(contract("creation bound must be positive"));
        }
        if let Some(ticket) = reserved_ticket.as_ref() {
            let allowance = supplied
                .as_ref()
                .ok_or_else(|| contract("reserved output requires its supplied parent"))?;
            self.validate_held_allowance(allowance)?;
            ticket.authenticate_reserved_for_parent(allowance, bound)?;
        }
        let supplied_funding = supplied.is_some();
        let (anchor, allowance) = self.claim_path(final_path, supplied)?;
        let relative = anchor.relative.clone();
        let mut claimed_inode = None;
        let mut reserved_new = false;
        let mut old_file_on_error = None;
        let result = (|| {
            let replaced_old_meta = if let Some(observed) = anchor.stat()? {
                if native {
                    return Err(existing_target("native create"));
                }
                let file = anchor.open()?;
                old_file_on_error = Some(file);
                let retained = self
                    .shared
                    .authority
                    .retained_inode(observed.inode, observed.bytes)?
                    .ok_or_else(|| contract("replacement target is not retained"))?;
                self.track_retained(retained, observed)?;
                {
                    let mut state = lock(&self.shared.state)?;
                    if !state.inodes_busy.insert(observed.inode) {
                        return Err(capacity("replacement inode has concurrent mutation"));
                    }
                }
                claimed_inode = Some(observed.inode);
                Some((retained, observed))
            } else {
                None
            };
            self.reserve_inode_slot()?;
            reserved_new = true;
            if !supplied_funding {
                allowance.top_up(bound, 1)?;
            }
            let ticket = if let Some(ticket) = reserved_ticket.take() {
                ticket
            } else {
                allowance.reserve_output(bound)?
            };
            Ok(FileMutation {
                binder: self.clone(),
                anchor,
                allowance,
                funding: if supplied_funding {
                    FundingMode::Supplied
                } else {
                    FundingMode::Local
                },
                ticket: Some(MutationTicket::Create(ticket)),
                reserved_end: bound,
                base_bytes: 0,
                observed: None,
                pin: None,
                declared_temp: None,
                control_create: None,
                replaced_old: match replaced_old_meta {
                    Some((retained, observed)) => Some((
                        old_file_on_error
                            .take()
                            .ok_or_else(|| contract("replacement FD disappeared"))?,
                        retained,
                        observed,
                    )),
                    None => None,
                },
                busy_inode: claimed_inode,
                phase: MutationPhase::Reserved,
                mode: if native {
                    MutationMode::Native
                } else {
                    MutationMode::Safe
                },
                native_covered: false,
                external_writer_live: false,
                safe_writer_fd: None,
                reserved_inode_slot: true,
            })
        })();
        if result.is_err() {
            if let Some(file) = old_file_on_error.take()
                && close_owned_file(file).is_err()
            {
                self.mark_close_unwitnessed();
            }
            if reserved_new {
                self.release_inode_slot();
            }
            self.release_path(&relative);
            if let Some(inode) = claimed_inode {
                self.release_busy_inode(inode);
            }
        }
        result
    }
    pub fn begin_byte_copy(
        &self,
        source: &File,
        final_path: &Path,
    ) -> Result<FileMutation, CoreError> {
        let source = InodeObservation::from_file(source)?;
        self.begin_create_with_bound(final_path, source.bytes.max(1), false, None, None)
    }
    pub fn begin_byte_copy_with_allowance(
        &self,
        source: &File,
        final_path: &Path,
        allowance: Arc<PublicationAllowance>,
    ) -> Result<FileMutation, CoreError> {
        let source = InodeObservation::from_file(source)?;
        self.begin_create_with_bound(
            final_path,
            source.bytes.max(1),
            false,
            Some(allowance),
            None,
        )
    }
    pub fn begin_resize(
        &self,
        path: &Path,
        opened: &File,
        target_end: u64,
    ) -> Result<FileMutation, CoreError> {
        self.begin_resize_with_funding(path, opened, target_end, None)
    }
    pub fn begin_resize_with_allowance(
        &self,
        path: &Path,
        opened: &File,
        target_end: u64,
        allowance: Arc<PublicationAllowance>,
    ) -> Result<FileMutation, CoreError> {
        self.begin_resize_with_funding(path, opened, target_end, Some(allowance))
    }
    fn begin_resize_with_funding(
        &self,
        path: &Path,
        opened: &File,
        target_end: u64,
        supplied: Option<Arc<PublicationAllowance>>,
    ) -> Result<FileMutation, CoreError> {
        let supplied_funding = supplied.is_some();
        let (anchor, allowance) = self.claim_path(path, supplied)?;
        let relative = anchor.relative.clone();
        let mut claimed_inode = None;
        let mut pin_on_error = None;
        let result = (|| {
            let observed = InodeObservation::from_file(opened)?;
            if anchor.stat()? != Some(observed) {
                return Err(contract("resize path and opened FD differ"));
            }
            let retained = self
                .shared
                .authority
                .retained_inode(observed.inode, observed.bytes)?
                .ok_or_else(|| contract("resize inode absent from sealed authority"))?;
            {
                let mut state = lock(&self.shared.state)?;
                if !state.inodes_busy.insert(observed.inode) {
                    return Err(capacity("inode has concurrent mutation"));
                }
            }
            claimed_inode = Some(observed.inode);
            self.track_retained(retained, observed)?;
            pin_on_error = Some(
                opened
                    .try_clone()
                    .map_err(|error| storage_io("pin writer FD", error))?,
            );
            let growth = target_end.saturating_sub(observed.bytes);
            if !supplied_funding {
                allowance.top_up(growth, 0)?;
            }
            let ticket = allowance.reserve_resize(retained, observed.bytes, target_end)?;
            Ok(FileMutation {
                binder: self.clone(),
                anchor,
                allowance,
                funding: if supplied_funding {
                    FundingMode::Supplied
                } else {
                    FundingMode::Local
                },
                ticket: Some(MutationTicket::Resize(ticket)),
                reserved_end: target_end.max(observed.bytes),
                base_bytes: observed.bytes,
                observed: Some(observed),
                pin: pin_on_error.take(),
                replaced_old: None,
                busy_inode: claimed_inode,
                declared_temp: None,
                control_create: None,
                phase: MutationPhase::Reserved,
                mode: MutationMode::Safe,
                native_covered: false,
                external_writer_live: true,
                safe_writer_fd: Some(opened.as_raw_fd()),
                reserved_inode_slot: false,
            })
        })();
        if result.is_err() {
            if let Some(pin) = pin_on_error.take()
                && close_owned_file(pin).is_err()
            {
                self.mark_close_unwitnessed();
            }
            self.release_path(&relative);
            if let Some(inode) = claimed_inode {
                self.release_busy_inode(inode);
            }
        }
        result
    }
    pub fn begin_native_resize(
        &self,
        path: &Path,
        native_witness: InodeObservation,
        target_end: u64,
    ) -> Result<FileMutation, CoreError> {
        self.begin_native_resize_inner(path, Some(native_witness), target_end, None)
    }
    /// Charge growth witnessed through a native file descriptor to a held parent.
    pub fn begin_native_resize_with_allowance(
        &self,
        path: &Path,
        native_witness: InodeObservation,
        target_end: u64,
        allowance: Arc<PublicationAllowance>,
    ) -> Result<FileMutation, CoreError> {
        self.begin_native_resize_inner(path, Some(native_witness), target_end, Some(allowance))
    }
    pub fn begin_native_resize_path(
        &self,
        path: &Path,
        target_end: u64,
    ) -> Result<FileMutation, CoreError> {
        self.begin_native_resize_inner(path, None, target_end, None)
    }
    /// Charge native SHM growth to an already-held parent after path authentication.
    pub fn begin_native_resize_path_with_allowance(
        &self,
        path: &Path,
        target_end: u64,
        allowance: Arc<PublicationAllowance>,
    ) -> Result<FileMutation, CoreError> {
        self.begin_native_resize_inner(path, None, target_end, Some(allowance))
    }
    fn begin_native_resize_inner(
        &self,
        path: &Path,
        native_witness: Option<InodeObservation>,
        target_end: u64,
        supplied: Option<Arc<PublicationAllowance>>,
    ) -> Result<FileMutation, CoreError> {
        let supplied_funding = supplied.is_some();
        let (anchor, allowance) = self.claim_path(path, supplied)?;
        let relative = anchor.relative.clone();
        let mut claimed_inode = None;
        let result = (|| {
            let observed = anchor
                .stat()?
                .ok_or_else(|| contract("native resize pathname absent"))?;
            if native_witness.is_some_and(|witness| witness != observed) {
                return Err(contract("native resize witness and pathname differ"));
            }
            let retained = self
                .shared
                .authority
                .retained_inode(observed.inode, observed.bytes)?
                .ok_or_else(|| contract("native resize inode absent from sealed authority"))?;
            {
                let mut state = lock(&self.shared.state)?;
                if !state.inodes_busy.insert(observed.inode) {
                    return Err(capacity("native inode has concurrent mutation"));
                }
            }
            claimed_inode = Some(observed.inode);
            self.track_retained(retained, observed)?;
            let growth = target_end.saturating_sub(observed.bytes);
            if !supplied_funding {
                allowance.top_up(growth, 0)?;
            }
            let ticket = allowance.reserve_resize(retained, observed.bytes, target_end)?;
            Ok(FileMutation {
                binder: self.clone(),
                anchor,
                allowance,
                funding: if supplied_funding {
                    FundingMode::Supplied
                } else {
                    FundingMode::Local
                },
                ticket: Some(MutationTicket::Resize(ticket)),
                reserved_end: target_end.max(observed.bytes),
                base_bytes: observed.bytes,
                observed: Some(observed),
                pin: None,
                replaced_old: None,
                busy_inode: claimed_inode,
                declared_temp: None,
                control_create: None,
                phase: MutationPhase::Reserved,
                mode: MutationMode::Native,
                native_covered: false,
                external_writer_live: false,
                safe_writer_fd: None,
                reserved_inode_slot: false,
            })
        })();
        if result.is_err() {
            self.release_path(&relative);
            if let Some(inode) = claimed_inode {
                self.release_busy_inode(inode);
            }
        }
        result
    }
    /// Dispose unsynced scratch children only after actual native FD retirement.
    /// This never turns a reopened file into a live native-writer witness.
    pub fn discard_retired_native_pending_children(
        &self,
        directory: &Path,
        before_child: &mut impl FnMut() -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let prefix = self.relative(directory)?;
        self.shared.root.require_current_v1()?;
        // Preserve directory deletion's private namespace fence before any
        // pending child is removed.
        let _namespace = directory::directory_observation(&self.anchor(directory)?)?;
        {
            let mut state = lock(&self.shared.state)?;
            require_publication_open(&state)?;
            if directory_scope_busy(&state, &prefix)
                || state.directory_scopes.len() >= self.shared.max_tracked_entries
                || state
                    .opening_readers
                    .keys()
                    .any(|path| path.starts_with(&prefix))
            {
                return Err(capacity("scratch disposal namespace busy"));
            }
            for path in state
                .paths_busy
                .iter()
                .filter(|path| path.starts_with(&prefix))
            {
                let pending = state
                    .pending
                    .get(path)
                    .ok_or_else(|| capacity("scratch child has a live mutation"))?;
                let retired = pending
                    .retired_native
                    .ok_or_else(|| contract("scratch child lacks native FD retirement"))?;
                if !pending.native
                    || pending.owned_pin.is_some()
                    || pending.external_writer_live
                    || pending.safe_writer_fd.is_some()
                    || retired.links != 1
                    || state
                        .inodes
                        .get(&retired.inode)
                        .is_some_and(|inode| inode.pins != 0)
                {
                    return Err(contract("scratch child is not exclusively retired"));
                }
            }
            if !state.directory_scopes.insert(prefix.clone()) {
                return Err(contract("scratch disposal scope already present"));
            }
            drop(state);
        }
        let outcome = (|| {
            for _ in 0..self.shared.max_tracked_entries {
                let next = {
                    let state = lock(&self.shared.state)?;
                    state
                        .pending
                        .keys()
                        .find(|path| path.starts_with(&prefix))
                        .cloned()
                };
                let Some(path) = next else {
                    return Ok(());
                };
                // Stop before admitting this child's irreversible unlink and
                // synced close. A completed prefix remains settled; pending
                // children retain their original charge for a later retry.
                before_child()?;
                self.discard_retired_native_pending_file(&path, false)?;
            }
            let state = lock(&self.shared.state)?;
            if state.pending.keys().any(|path| path.starts_with(&prefix)) {
                return Err(capacity("scratch disposal entry bound exceeded"));
            }
            drop(state);
            Ok(())
        })();
        let released = self
            .shared
            .state
            .lock()
            .is_ok_and(|mut state| state.directory_scopes.remove(&prefix));
        if !released {
            self.mark_close_unwitnessed();
            return Err(contract("scratch disposal scope release failed"));
        }
        outcome
    }
    /// Dispose only an unused native birth after its original FD retired.
    /// A populated file or resize cannot use this empty-sidecar cleanup path.
    pub(crate) fn discard_retired_empty_native_create(&self, path: &Path) -> Result<(), CoreError> {
        let relative = self.relative(path)?;
        self.discard_retired_native_pending_file(&relative, true)
    }

    fn discard_retired_native_pending_file(
        &self,
        path: &Path,
        empty_create_only: bool,
    ) -> Result<(), CoreError> {
        let anchor = self.anchor(path)?;
        let (id, kind, retired, reserved_inode_slot, busy_inode) = {
            let mut state = lock(&self.shared.state)?;
            require_publication_open(&state)?;
            let Some(pending) = state.pending.get(path) else {
                if empty_create_only {
                    return Ok(());
                }
                return Err(contract("retired scratch pending disappeared"));
            };
            let retired = pending
                .retired_native
                .ok_or_else(|| contract("scratch pending lacks native close witness"))?;
            if empty_create_only
                && (!matches!(pending.kind, PendingKind::Create)
                    || retired.bytes != 0
                    || !pending.native
                    || !pending.reserved_inode_slot
                    || pending.busy_inode.is_some()
                    || pending.born_inode != Some(retired.inode)
                    || pending.owned_pin.is_some()
                    || pending.external_writer_live
                    || pending.safe_writer_fd.is_some()
                    || retired.links != 1
                    || directory_scope_busy(&state, path)
                    || state
                        .inodes
                        .get(&retired.inode)
                        .is_some_and(|inode| inode.pins != 0))
            {
                return Err(contract(
                    "empty native cleanup requires an exclusively retired zero-byte birth",
                ));
            }
            let selected = (
                pending.id,
                pending.kind,
                retired,
                pending.reserved_inode_slot,
                pending.busy_inode,
            );
            if state.opening_readers.contains_key(path)
                || state
                    .total_pins
                    .checked_add(state.total_opening_readers)
                    .is_none_or(|count| count >= self.shared.max_open_handles)
                || state.opening_readers.len() >= self.shared.max_tracked_entries
            {
                return Err(capacity("scratch cleanup reader admission exhausted"));
            }
            state.total_opening_readers = state
                .total_opening_readers
                .checked_add(1)
                .ok_or_else(|| capacity("scratch cleanup reader overflow"))?;
            let _prior = state.opening_readers.insert(path.to_path_buf(), 1);
            drop(state);
            selected
        };
        let mut actual = None;
        let mut unlinked = false;
        let outcome = (|| {
            actual = Some(anchor.open()?);
            let file = actual
                .as_ref()
                .ok_or_else(|| contract("scratch cleanup FD absent"))?;
            if InodeObservation::from_file(file)? != retired || anchor.stat()? != Some(retired) {
                return Err(contract("retired scratch file changed before unlink"));
            }
            self.shared.root.require_current_v1()?;
            unlinkat(&anchor.parent, Path::new(&anchor.name), AtFlags::empty())
                .map_err(|error| storage_io("unlink retired scratch pending", error))?;
            unlinked = true;
            let after = InodeObservation::from_file(file)?;
            if after.inode != retired.inode || after.bytes != retired.bytes || after.links != 0 {
                return Err(contract("scratch disposal lacks zero-link FD witness"));
            }
            anchor.sync_parent()?;
            close_owned_file(
                actual
                    .take()
                    .ok_or_else(|| contract("scratch cleanup FD absent"))?,
            )
            .map_err(|error| storage_io("close retired scratch witness", error))?;
            self.shared.root.require_current_v1()?;
            let mut state = lock(&self.shared.state)?;
            require_publication_open(&state)?;
            if state
                .pending
                .get(path)
                .is_none_or(|pending| pending.id != id)
            {
                return Err(contract("scratch disposal ticket changed"));
            }
            match kind {
                PendingKind::Create => self
                    .shared
                    .authority
                    .confirmed_pending_absent_and_synced(id)?,
                PendingKind::Resize { inode } if inode == retired.inode => self
                    .shared
                    .authority
                    .confirmed_pending_resize_last_reference_removed_and_synced(id)?,
                PendingKind::Resize { .. } => {
                    return Err(contract("scratch pending resize inode changed"));
                }
            }
            let removed = state.pending.remove(path);
            if removed.is_none()
                || !state.paths_busy.remove(path)
                || busy_inode.is_some_and(|inode| !state.inodes_busy.remove(&inode))
            {
                return Err(contract("scratch disposal tracking disappeared"));
            }
            let _retired = state.inodes.remove(&retired.inode);
            if reserved_inode_slot {
                state.reserved_inode_slots = state
                    .reserved_inode_slots
                    .checked_sub(1)
                    .ok_or_else(|| contract("scratch reserved inode slot underflow"))?;
            }
            drop(state);
            Ok(())
        })();
        if let Some(file) = actual
            && let Err(error) = close_owned_file(file)
        {
            self.mark_close_unwitnessed();
            self.release_opening(path);
            return Err(storage_io("close rejected scratch witness", error));
        }
        if outcome.is_err() && unlinked {
            // An interrupted destructive prefix retains its charge and blocks
            // this root until explicit process restart and boot inventory.
            self.mark_close_unwitnessed();
        }
        self.release_opening(path);
        outcome
    }
    /// Inspect only a physically retired safe resize eligible for recovery.
    /// This grants no settlement; recovery still reauthenticates its owned FD.
    pub fn has_retired_safe_resize_pending(&self, path: &Path) -> Result<bool, CoreError> {
        self.shared.root.require_current_v1()?;
        let relative = self.relative(path)?;
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        Ok(state.pending.get(&relative).is_some_and(|pending| {
            matches!(pending.kind, PendingKind::Resize { .. })
                && !pending.native
                && !pending.external_writer_live
                && pending.safe_writer_fd.is_none()
                && pending.owned_pin.is_none()
        }))
    }

    /// Discover a retired visible create; settlement still requires an actual FD.
    pub fn has_retired_safe_visible_create_pending(&self, path: &Path) -> Result<bool, CoreError> {
        self.shared.root.require_current_v1()?;
        let relative = self.relative(path)?;
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        let pending = state.pending.get(&relative).is_some_and(|pending| {
            matches!(pending.kind, PendingKind::Create)
                && !pending.native
                && !pending.external_writer_live
                && pending.safe_writer_fd.is_none()
                && pending.control_create.is_none()
                && pending.born_inode.is_some()
        });
        drop(state);
        Ok(pending)
    }

    /// Return one bounded, opted-in Control create whose original writer has
    /// been physically retired. This is discovery only, not a success proof.
    pub fn next_retired_control_create(&self) -> Result<Option<PathBuf>, CoreError> {
        self.shared.root.require_current_v1()?;
        let state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        let next = state.pending.iter().find_map(|(path, pending)| {
            pending.control_create.as_ref().and_then(|control| {
                (!control.recovering
                    && !pending.external_writer_live
                    && pending.safe_writer_fd.is_none())
                .then(|| self.state_root().join(path))
            })
        });
        drop(state);
        Ok(next)
    }

    /// Reconcile only the original target and declared temp of a retired
    /// Control creation. An absent result requires the born FD's zero-link
    /// witness, both anchored parent syncs, and checked final FD retirement.
    pub fn recover_retired_control_create(
        &self,
        target: &Path,
    ) -> Result<ControlCreateRecovery, CoreError> {
        self.shared.root.require_current_v1()?;
        let relative = self.relative(target)?;
        let (claim, target_held, temp_held, witness_fd, born, reserved_end, old) = {
            let mut state = lock(&self.shared.state)?;
            require_publication_open(&state)?;
            let pending = state
                .pending
                .get(&relative)
                .ok_or_else(|| contract("Control create pending target absent"))?;
            let control = pending
                .control_create
                .as_ref()
                .ok_or_else(|| contract("pending create lacks Control opt-in"))?;
            if !matches!(pending.kind, PendingKind::Create)
                || pending.native
                || pending.external_writer_live
                || pending.safe_writer_fd.is_some()
                || control.recovering
                || pending.busy_inode
                    != control
                        .replaced_old
                        .as_ref()
                        .map(|(_, _, prior)| prior.inode)
                || !pending.reserved_inode_slot
                || !state.paths_busy.contains(&relative)
            {
                return Err(contract(
                    "Control create is not retired or is already recovering",
                ));
            }
            let born = pending
                .born_inode
                .ok_or_else(|| contract("Control create lacks born inode witness"))?;
            let old = control
                .replaced_old
                .as_ref()
                .map(|(file, _, prior)| {
                    InodeObservation::from_file(file).map(|current| (*prior, current))
                })
                .transpose()?;
            let witness_fd = pending
                .owned_pin
                .as_ref()
                .ok_or_else(|| contract("Control create lacks original owned pin"))?
                .try_clone()
                .map_err(|error| storage_io("clone Control create pin", error))?;
            let target_held = Arc::clone(&control.target);
            let temp_held = Arc::clone(&control.temporary);
            if target_held.relative != relative
                || state.paths_busy.contains(&temp_held.relative)
                || state.opening_readers.contains_key(&temp_held.relative)
                || !state.directory_scopes.contains(&temp_held.relative)
            {
                drop(state);
                if close_owned_file(witness_fd).is_err() {
                    self.mark_close_unwitnessed();
                    return Err(storage("close rejected Control observer", "close failed"));
                }
                return Err(capacity("Control create recovery path is busy"));
            }
            let id = pending.id;
            let reserved_end = control.reserved_end;
            let pending = state
                .pending
                .get_mut(&relative)
                .ok_or_else(|| contract("Control pending disappeared during claim"))?;
            let control = pending
                .control_create
                .as_mut()
                .ok_or_else(|| contract("Control pending opt-in disappeared"))?;
            control.recovering = true;
            drop(state);
            (
                ControlCreateClaim {
                    shared: Arc::clone(&self.shared),
                    target: relative.clone(),
                    id,
                },
                target_held,
                temp_held,
                witness_fd,
                born,
                reserved_end,
                old,
            )
        };
        let mut witness_fd = Some(witness_fd);
        let result = (|| {
            let target_anchor = target_held.current_anchor(self)?;
            let temp_anchor = temp_held.current_anchor(self)?;
            let file = witness_fd
                .as_ref()
                .ok_or_else(|| contract("Control recovery observer absent"))?;
            let observed = InodeObservation::from_file(file)?;
            if observed.inode != born || observed.bytes > reserved_end {
                return Err(contract("Control recovery pin differs from born inode"));
            }
            let target_seen = target_anchor.stat()?;
            let temp_seen = temp_anchor.stat()?;
            let old_disposition = match (old, target_seen, temp_seen) {
                (None, Some(target), None) if target == observed && observed.links == 1 => None,
                (None, None, Some(temp)) if temp == observed && observed.links == 1 => None,
                (None, None, None) if observed.links == 0 => None,
                (Some((prior, current)), Some(target), Some(temp))
                    if target == prior
                        && current == prior
                        && temp == observed
                        && observed.links == 1 =>
                {
                    Some(ControlOldDisposition::StillLinked)
                }
                (Some((prior, current)), Some(target), None)
                    if target == observed
                        && current.inode == prior.inode
                        && current.bytes == prior.bytes
                        && current.links == prior.links.saturating_sub(1)
                        && observed.links == 1 =>
                {
                    Some(ControlOldDisposition::Replaced)
                }
                (Some((prior, current)), Some(target), None)
                    if target == prior && current == prior && observed.links == 0 =>
                {
                    Some(ControlOldDisposition::StillLinked)
                }
                _ => return Err(contract("Control recovery path, link or inode changed")),
            };
            file.sync_all()
                .map_err(|error| storage_io("Control recovery file fsync", error))?;
            target_anchor.sync_parent()?;
            temp_anchor.sync_parent()?;
            self.shared.root.require_current_v1()?;
            if target_anchor.stat()? != target_seen
                || temp_anchor.stat()? != temp_seen
                || InodeObservation::from_file(file)? != observed
            {
                return Err(contract("Control recovery witness changed after sync"));
            }
            if target_seen == Some(observed) || temp_seen == Some(observed) {
                let current = if target_seen == Some(observed) {
                    target_anchor.relative.as_path()
                } else {
                    temp_anchor.relative.as_path()
                };
                let _retained = self.recover_pending_with_fd_inner(
                    &relative,
                    current,
                    witness_fd
                        .take()
                        .ok_or_else(|| contract("Control recovery observer absent"))?,
                    Some(claim.id),
                    old_disposition,
                )?;
                return Ok(if target_seen == Some(observed) {
                    ControlCreateRecovery::RetainedTarget
                } else {
                    ControlCreateRecovery::RetainedTemporary {
                        path: self.state_root().join(&temp_anchor.relative),
                        inode: observed.inode,
                        bytes: observed.bytes,
                    }
                });
            }
            let state = lock(&self.shared.state)?;
            if state.inodes.get(&born).is_some_and(|entry| entry.pins != 0) {
                return Err(contract("absent Control inode has tracked readers"));
            }
            drop(state);
            if let Some(disposition) = old_disposition {
                let _tombstone =
                    self.retire_control_replaced_old(&relative, claim.id, disposition, observed)?;
            }
            close_owned_file(
                witness_fd
                    .take()
                    .ok_or_else(|| contract("Control recovery observer absent"))?,
            )
            .map_err(|error| {
                self.mark_close_unwitnessed();
                storage_io("close absent Control observer", error)
            })?;
            let original = {
                let mut state = lock(&self.shared.state)?;
                let pending = state
                    .pending
                    .get_mut(&relative)
                    .ok_or_else(|| contract("absent Control pending disappeared"))?;
                if pending.id != claim.id {
                    return Err(contract("absent Control pending changed"));
                }
                let original = pending
                    .owned_pin
                    .take()
                    .ok_or_else(|| contract("absent Control original pin disappeared"))?;
                drop(state);
                original
            };
            close_owned_file(original).map_err(|error| {
                self.mark_close_unwitnessed();
                storage_io("close absent Control original pin", error)
            })?;
            self.shared
                .authority
                .confirmed_pending_absent_and_synced(claim.id)?;
            let mut state = lock(&self.shared.state)?;
            if state
                .pending
                .get(&relative)
                .and_then(|pending| pending.control_create.as_ref())
                .is_some_and(|control| control.replaced_old.is_some())
            {
                return Err(contract("absent Control old FD was not retired"));
            }
            let removed = state.pending.remove(&relative);
            if removed.is_none()
                || !state.paths_busy.remove(&relative)
                || removed
                    .as_ref()
                    .and_then(|pending| pending.busy_inode)
                    .is_some_and(|inode| !state.inodes_busy.remove(&inode))
                || removed
                    .as_ref()
                    .and_then(|pending| pending.control_create.as_ref())
                    .is_some_and(|control| {
                        !state.directory_scopes.remove(&control.temporary.relative)
                    })
            {
                return Err(contract("absent Control tracking disappeared"));
            }
            state.reserved_inode_slots = state
                .reserved_inode_slots
                .checked_sub(1)
                .ok_or_else(|| contract("absent Control inode slot underflow"))?;
            drop(state);
            Ok(ControlCreateRecovery::Absent)
        })();
        if let Some(file) = witness_fd
            && let Err(error) = close_owned_file(file)
        {
            self.mark_close_unwitnessed();
            return Err(storage_io("close failed Control recovery observer", error));
        }
        drop(claim);
        result
    }

    fn retire_control_replaced_old(
        &self,
        busy: &Path,
        id: PendingId,
        disposition: ControlOldDisposition,
        born: InodeObservation,
    ) -> Result<bool, CoreError> {
        let (target, prior, retained, current) = {
            let state = lock(&self.shared.state)?;
            let pending = state
                .pending
                .get(busy)
                .ok_or_else(|| contract("replaced Control pending disappeared"))?;
            let control = pending
                .control_create
                .as_ref()
                .ok_or_else(|| contract("replaced Control custody absent"))?;
            if pending.id != id || !control.recovering {
                return Err(contract("replaced Control recovery claim changed"));
            }
            let (file, retained, prior) = control
                .replaced_old
                .as_ref()
                .ok_or_else(|| contract("replaced Control original FD absent"))?;
            let current = InodeObservation::from_file(file)?;
            if current.inode != prior.inode
                || current.bytes != prior.bytes
                || current.links
                    != match disposition {
                        ControlOldDisposition::StillLinked => prior.links,
                        ControlOldDisposition::Replaced => prior
                            .links
                            .checked_sub(1)
                            .ok_or_else(|| contract("old Control link count underflow"))?,
                    }
            {
                return Err(contract("replaced Control original inode changed"));
            }
            let selected = (Arc::clone(&control.target), *prior, *retained, current);
            drop(state);
            selected
        };
        let anchor = target.current_anchor(self)?;
        let expected_target = match disposition {
            ControlOldDisposition::StillLinked => prior,
            ControlOldDisposition::Replaced => born,
        };
        if anchor.stat()? != Some(expected_target)
            || self
                .shared
                .authority
                .retained_inode(prior.inode, prior.bytes)?
                != Some(retained)
        {
            return Err(contract(
                "replaced Control target or retained charge changed",
            ));
        }
        self.shared.root.require_current_v1()?;
        let original = {
            let mut state = lock(&self.shared.state)?;
            let pending = state
                .pending
                .get_mut(busy)
                .ok_or_else(|| contract("replaced Control pending disappeared before close"))?;
            let control = pending
                .control_create
                .as_mut()
                .ok_or_else(|| contract("replaced Control custody disappeared before close"))?;
            if pending.id != id || !control.recovering {
                return Err(contract("replaced Control close claim changed"));
            }
            let original = control
                .replaced_old
                .take()
                .ok_or_else(|| contract("replaced Control original FD disappeared"))?
                .0;
            drop(state);
            original
        };
        if let Err(error) = close_owned_file(original) {
            self.mark_close_unwitnessed();
            return Err(storage_io("close replaced Control original FD", error));
        }
        if anchor.stat()? != Some(expected_target) {
            self.mark_close_unwitnessed();
            return Err(contract("replaced Control target changed after FD close"));
        }
        let tombstone =
            matches!(disposition, ControlOldDisposition::Replaced) && current.links == 0;
        if tombstone {
            let mut state = lock(&self.shared.state)?;
            let entry = state
                .inodes
                .get_mut(&prior.inode)
                .ok_or_else(|| contract("replaced Control tracked inode absent"))?;
            if entry.retained != retained || entry.bytes != prior.bytes {
                return Err(contract("replaced Control tracked inode changed"));
            }
            entry.tombstone_synced = true;
            state.may_have_tombstones = true;
            drop(state);
        }
        Ok(tombstone)
    }

    /// Reconcile an orphaned started ticket using a caller-owned real FD and
    /// the same nofollow path witness. No absence refund is inferred here.
    pub fn recover_pending_with_fd(
        &self,
        busy_path: &Path,
        current_path: &Path,
        file: File,
    ) -> Result<RetainedId, CoreError> {
        self.recover_pending_with_fd_inner(busy_path, current_path, file, None, None)
    }
    /// Reopen the current parent chain after sync. The held anchor alone could
    /// still name an inode in a directory detached during the sync call.
    fn revalidate_recovery_witness_after_sync(
        &self,
        held: &Anchor,
        current_path: &Path,
        file: &File,
        observed: InodeObservation,
    ) -> Result<(), CoreError> {
        self.shared.root.require_current_v1()?;
        let current = self.anchor(current_path)?;
        let held_parent = held
            .parent
            .metadata()
            .map_err(|error| storage_io("stat held recovery parent", error))?;
        let current_parent = current
            .parent
            .metadata()
            .map_err(|error| storage_io("stat current recovery parent", error))?;
        if held.relative != current.relative
            || held.name != current.name
            || held_parent.dev() != current_parent.dev()
            || held_parent.ino() != current_parent.ino()
            || held.stat()? != Some(observed)
            || current.stat()? != Some(observed)
            || InodeObservation::from_file(file)? != observed
        {
            return Err(contract("recovery path or FD changed after sync"));
        }
        self.shared.root.require_current_v1()
    }
    fn recover_pending_with_fd_inner(
        &self,
        busy_path: &Path,
        current_path: &Path,
        file: File,
        control_claim: Option<PendingId>,
        old_disposition: Option<ControlOldDisposition>,
    ) -> Result<RetainedId, CoreError> {
        let mut file = Some(file);
        let mut original_to_retire = None;
        let mut rejected_clone = false;
        let outcome = (|| {
            let busy = self.relative(busy_path)?;
            let (id, kind, busy_inode, reserved_inode_slot, born_inode) = {
                let state = lock(&self.shared.state)?;
                require_publication_open(&state)?;
                let pending = state
                    .pending
                    .get(&busy)
                    .ok_or_else(|| contract("no pending path to recover"))?;
                if pending.control_create.is_some() && control_claim != Some(pending.id) {
                    return Err(contract("Control pending requires anchored reconciliation"));
                }
                if pending.control_create.as_ref().is_some_and(|control| {
                    control.replaced_old.is_some() != old_disposition.is_some()
                }) {
                    return Err(contract("Control replacement recovery mode changed"));
                }
                if pending.native {
                    return Err(contract(
                        "native pending requires FD retirement and boot inventory",
                    ));
                }
                if pending.external_writer_live {
                    if pending.safe_writer_fd != file.as_ref().map(std::os::fd::AsRawFd::as_raw_fd)
                    {
                        rejected_clone = true;
                        return Err(contract("pending recovery requires the original writer FD"));
                    }
                    original_to_retire = Some((busy.clone(), pending.id));
                } else if pending.safe_writer_fd.is_some() {
                    return Err(contract("retired pending writer still has an FD identity"));
                }
                let kind = match pending.kind {
                    PendingKind::Create => PendingKind::Create,
                    PendingKind::Resize { inode } => PendingKind::Resize { inode },
                };
                let snapshot = (
                    pending.id,
                    kind,
                    pending.busy_inode,
                    pending.reserved_inode_slot,
                    pending.born_inode,
                );
                drop(state);
                snapshot
            };
            let anchor = self.anchor(current_path)?;
            let observed = InodeObservation::from_file(
                file.as_ref()
                    .ok_or_else(|| contract("recovery FD absent"))?,
            )?;
            if born_inode.is_some_and(|born| born != observed.inode) {
                return Err(contract("recovery FD differs from born inode"));
            }
            if anchor.stat()? != Some(observed) {
                return Err(contract("recovery FD and anchored path differ"));
            }
            file.as_ref()
                .ok_or_else(|| contract("recovery FD absent"))?
                .sync_all()
                .map_err(|error| storage_io("recovery file fsync", error))?;
            anchor.sync_parent()?;
            self.revalidate_recovery_witness_after_sync(
                &anchor,
                current_path,
                file.as_ref()
                    .ok_or_else(|| contract("recovery FD absent"))?,
                observed,
            )?;
            if let Some((original_busy, original_id)) = original_to_retire.take() {
                let mut state = lock(&self.shared.state)?;
                let pending = state
                    .pending
                    .get_mut(&original_busy)
                    .ok_or_else(|| contract("pending writer disappeared before FD close"))?;
                if pending.id != original_id || !pending.external_writer_live {
                    return Err(contract("pending writer changed before FD close"));
                }
                if let Err(error) =
                    close_owned_file(file.take().ok_or_else(|| contract("recovery FD absent"))?)
                {
                    drop(state);
                    self.mark_close_unwitnessed();
                    return Err(storage_io("close recovery writer FD", error));
                }
                pending.external_writer_live = false;
                pending.safe_writer_fd = None;
                drop(state);
            } else if let Err(error) =
                close_owned_file(file.take().ok_or_else(|| contract("recovery FD absent"))?)
            {
                self.mark_close_unwitnessed();
                return Err(storage_io("close recovery writer FD", error));
            }
            let owned_pin = {
                let mut state = lock(&self.shared.state)?;
                state
                    .pending
                    .get_mut(&busy)
                    .and_then(|entry| entry.owned_pin.take())
            };
            if let Some(pin) = owned_pin
                && let Err(error) = close_owned_file(pin)
            {
                self.mark_close_unwitnessed();
                return Err(storage_io("close pending writer pin", error));
            }
            let retained = match kind {
                PendingKind::Create => self
                    .shared
                    .authority
                    .confirmed_pending_committed_and_synced(id, observed.inode, observed.bytes)?,
                PendingKind::Resize { inode } if inode == observed.inode => self
                    .shared
                    .authority
                    .confirmed_pending_resize_settled_and_synced(
                    id,
                    observed.inode,
                    observed.bytes,
                )?,
                PendingKind::Resize { .. } => {
                    return Err(contract("recovery resize inode changed"));
                }
            };
            if reserved_inode_slot {
                self.track_retained_reserved(retained, observed)?;
            } else {
                self.track_retained(retained, observed)?;
            }
            let sweep_old = old_disposition
                .map(|disposition| {
                    self.retire_control_replaced_old(&busy, id, disposition, observed)
                })
                .transpose()?
                .unwrap_or(false);
            let mut state = lock(&self.shared.state)?;
            if state
                .pending
                .get(&busy)
                .and_then(|pending| pending.control_create.as_ref())
                .is_some_and(|control| control.replaced_old.is_some())
            {
                return Err(contract("Control old FD was not retired"));
            }
            let control_temp = state
                .pending
                .get(&busy)
                .and_then(|pending| pending.control_create.as_ref())
                .map(|control| control.temporary.relative.clone());
            if state.pending.remove(&busy).is_none()
                || !state.paths_busy.remove(&busy)
                || busy_inode.is_some_and(|inode| !state.inodes_busy.remove(&inode))
                || control_temp
                    .as_ref()
                    .is_some_and(|temporary| !state.directory_scopes.remove(temporary))
            {
                return Err(contract("recovered mutation tracking disappeared"));
            }
            drop(state);
            if sweep_old {
                self.sweep_retired()?;
            }
            Ok(retained)
        })();
        if let Some(file) = file.take() {
            if let Err(error) = close_owned_file(file) {
                self.mark_close_unwitnessed();
                return Err(storage_io("close failed-recovery FD", error));
            }
            if original_to_retire.is_some() || !rejected_clone {
                // An original may have been consumed before a valid recovery
                // witness. Its numeric descriptor must not be reused as proof.
                self.mark_close_unwitnessed();
            }
        }
        outcome
    }
    pub fn sweep_retired(&self) -> Result<(), CoreError> {
        loop {
            let candidate = {
                let mut state = lock(&self.shared.state)?;
                if state.close_unwitnessed {
                    return Err(storage("retirement", "owned FD close was ambiguous"));
                }
                if !state.may_have_tombstones {
                    return Ok(());
                }
                let mut tombstone_seen = false;
                let key = state.inodes.iter().find_map(|(key, entry)| {
                    tombstone_seen |= entry.tombstone_synced;
                    (entry.tombstone_synced && entry.pins == 0 && !entry.releasing).then_some(*key)
                });
                let Some(key) = key else {
                    // Exhaustion, not absence of eligible entries, proves emptiness.
                    // Pinned and concurrently releasing tombstones keep the hint set.
                    state.may_have_tombstones = tombstone_seen;
                    return Ok(());
                };
                let entry = state
                    .inodes
                    .get_mut(&key)
                    .ok_or_else(|| contract("retire candidate missing"))?;
                entry.releasing = true;
                let candidate = (key, entry.retained, entry.bytes);
                drop(state);
                candidate
            };
            let (key, retained, bytes) = candidate;
            if let Err(error) = self
                .shared
                .authority
                .confirmed_last_reference_removed_and_synced(retained, bytes)
            {
                if let Ok(mut state) = self.shared.state.lock()
                    && let Some(entry) = state.inodes.get_mut(&key)
                {
                    entry.releasing = false;
                }
                return Err(error);
            }
            let mut state = lock(&self.shared.state)?;
            let removed = state
                .inodes
                .remove(&key)
                .ok_or_else(|| contract("retired inode disappeared"))?;
            if removed.pins != 0 || !removed.tombstone_synced || removed.retained != retained {
                return Err(contract("retired inode changed during ledger release"));
            }
            drop(state);
        }
    }
}

enum MutationTicket {
    Create(OutputTicket),
    Resize(ResizeTicket),
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum MutationPhase {
    Reserved,
    Started,
    Settled,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum MutationMode {
    Safe,
    Native,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum FundingMode {
    Local,
    Supplied,
}
pub struct FileMutation {
    binder: PublicationFileCapacity,
    anchor: Anchor,
    allowance: Arc<PublicationAllowance>,
    funding: FundingMode,
    ticket: Option<MutationTicket>,
    reserved_end: u64,
    base_bytes: u64,
    observed: Option<InodeObservation>,
    pin: Option<File>,
    replaced_old: Option<(File, RetainedId, InodeObservation)>,
    busy_inode: Option<InodeKey>,
    declared_temp: Option<Anchor>,
    control_create: Option<(Arc<HeldPath>, Arc<HeldPath>)>,
    phase: MutationPhase,
    mode: MutationMode,
    native_covered: bool,
    external_writer_live: bool,
    safe_writer_fd: Option<RawFd>,
    reserved_inode_slot: bool,
}
impl FileMutation {
    /// Transfer the owned FD pin into a lifetime guard after settlement.
    pub fn take_owned_lease(&mut self) -> Result<ReaderLease, CoreError> {
        if !self.is_settled() || self.external_writer_live {
            return Err(contract("writer is not settled"));
        }
        let file = self
            .pin
            .as_ref()
            .ok_or_else(|| contract("writer has no owned FD pin"))?;
        let observed = InodeObservation::from_file(file)?;
        let cloned = file
            .try_clone()
            .map_err(|error| storage_io("clone settled FD for lease", error))?;
        let lease = self.binder.pin(observed, cloned)?;
        let original = self
            .pin
            .take()
            .ok_or_else(|| contract("writer pin disappeared"))?;
        if let Err(error) = close_owned_file(original) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close settled writer pin", error));
        }
        // Replacement settlement has already witnessed the old inode's last
        // link and closed its owned FD. Retire it only after this writer's
        // actual FD is closed and the new inode has a tracked reader lease.
        self.binder.sweep_retired()?;
        Ok(lease)
    }
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.phase == MutationPhase::Settled
    }
    #[must_use]
    pub fn is_started(&self) -> bool {
        self.phase != MutationPhase::Reserved
    }
    fn is_native(&self) -> bool {
        self.mode == MutationMode::Native
    }
    pub fn take_native_lease(&mut self) -> Result<NativeReaderLease, CoreError> {
        if !self.is_native() || !self.is_settled() || self.native_covered {
            return Err(contract("native writer cannot transfer inode pin"));
        }
        let observed = self
            .observed
            .ok_or_else(|| contract("native writer lacks witness"))?;
        let lease = self
            .binder
            .pin_native_witness(&self.anchor.relative, observed)?;
        self.native_covered = true;
        Ok(lease)
    }
    pub fn finish_with_existing_native_lease(
        &mut self,
        lease: &NativeReaderLease,
    ) -> Result<(), CoreError> {
        if !self.is_native()
            || !self.is_settled()
            || self.observed.map(|o| o.inode) != Some(lease.inode())
        {
            return Err(contract("native existing lease does not cover writer"));
        }
        self.native_covered = true;
        Ok(())
    }
    /// Resize on a handle already carrying a `ReaderLease`. The existing lease
    /// covers the native inode through its eventual physical FD retirement.
    pub fn finish_with_existing_lease(&mut self, lease: &ReaderLease) -> Result<(), CoreError> {
        if !self.is_settled()
            || self.external_writer_live
            || self.observed.map(|o| o.inode) != Some(lease.inode())
        {
            return Err(contract(
                "existing reader lease does not cover settled writer",
            ));
        }
        if let Some(pin) = self.pin.take()
            && let Err(error) = close_owned_file(pin)
        {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close existing-lease writer pin", error));
        }
        Ok(())
    }
    /// A native `FdRetired` event proves that the lower FD is physically gone.
    /// This permits dropping a settled writer without installing a reader pin.
    pub fn confirmed_native_fd_retired(&mut self) -> Result<(), CoreError> {
        if !self.is_native() || !self.is_settled() {
            return Err(contract("native FD retired before mutation settled"));
        }
        self.native_covered = true;
        Ok(())
    }
    /// Record actual native retirement without granting a durability settlement.
    /// Only a successful physical `FdRetired` event or the admitted empty-lock
    /// owner's checked original close may call this.
    pub fn confirmed_unsynced_native_fd_retired(
        &mut self,
        observed: InodeObservation,
    ) -> Result<(), CoreError> {
        if !self.is_native()
            || !self.is_started()
            || self.is_settled()
            || observed.bytes > self.reserved_end
        {
            return Err(contract("unsynced native retirement out of order"));
        }
        self.observe_native_fd(observed)?;
        self.native_covered = true;
        Ok(())
    }
    #[must_use]
    pub fn pending_id(&self) -> Option<PendingId> {
        self.ticket.as_ref().map(|ticket| match ticket {
            MutationTicket::Create(ticket) => ticket.id(),
            MutationTicket::Resize(ticket) => ticket.id(),
        })
    }
    /// Consume the caller's actual safe writer FD. The caller must not retain
    /// an untracked duplicate. No owned reader lease is transferable before
    /// this physical close has succeeded.
    pub fn confirmed_safe_writer_fd_retired(&mut self, writer: File) -> Result<(), CoreError> {
        let supplied_fd = writer.as_raw_fd();
        let original = self.safe_writer_fd == Some(supplied_fd);
        let observation = InodeObservation::from_file(&writer);
        if let Err(error) = close_owned_file(writer) {
            if original {
                self.safe_writer_fd = None;
            }
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close actual writer FD", error));
        }
        if self.is_native() || !self.external_writer_live || !original {
            return Err(contract("safe writer FD retirement out of order"));
        }
        self.safe_writer_fd = None;
        let observed = match observation {
            Ok(observed) => observed,
            Err(error) => {
                self.binder.mark_close_unwitnessed();
                return Err(error);
            }
        };
        if self.observed.map(|prior| prior.inode) != Some(observed.inode) {
            self.binder.mark_close_unwitnessed();
            return Err(contract("safe writer FD identity differs"));
        }
        self.external_writer_live = false;
        Ok(())
    }
    /// Physically retire an auxiliary writer handle while retaining the
    /// original writer's external-lifetime guard. Used before overwrite rename.
    pub fn confirmed_auxiliary_writer_fd_retired(&mut self, writer: File) -> Result<(), CoreError> {
        let original = self.safe_writer_fd == Some(writer.as_raw_fd());
        let observation = InodeObservation::from_file(&writer);
        if let Err(error) = close_owned_file(writer) {
            if original {
                self.safe_writer_fd = None;
            }
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close auxiliary writer FD", error));
        }
        if original {
            self.safe_writer_fd = None;
            self.binder.mark_close_unwitnessed();
            return Err(contract("original writer FD passed as auxiliary"));
        }
        if self.is_native() || !self.external_writer_live {
            return Err(contract("auxiliary writer FD retirement out of order"));
        }
        let observed = observation?;
        if self.observed.map(|prior| prior.inode) != Some(observed.inode) {
            return Err(contract("auxiliary writer FD identity differs"));
        }
        Ok(())
    }
    pub fn reserve_end(&mut self, target_end: u64) -> Result<(), CoreError> {
        self.binder.validate_held_allowance(&self.allowance)?;
        if self.ticket.is_none() {
            return Err(contract("mutation already settled"));
        }
        if target_end <= self.reserved_end {
            return Ok(());
        }
        let additional = target_end.saturating_sub(self.reserved_end.max(self.base_bytes));
        if self.funding == FundingMode::Local {
            self.binder.refresh_maintenance_reserve()?;
            self.allowance.top_up(additional, 0)?;
        }
        match self
            .ticket
            .as_ref()
            .ok_or_else(|| contract("mutation already settled"))?
        {
            MutationTicket::Create(ticket) => ticket.reserve_growth(target_end)?,
            MutationTicket::Resize(ticket) => ticket.reserve_growth(target_end)?,
        }
        self.reserved_end = target_end;
        Ok(())
    }
    pub fn creation_started(&mut self) -> Result<(), CoreError> {
        self.binder.validate_held_allowance(&self.allowance)?;
        match self.ticket.as_ref() {
            Some(MutationTicket::Create(ticket)) if !self.is_started() => {
                ticket.creation_started()?;
                self.phase = MutationPhase::Started;
                Ok(())
            }
            Some(MutationTicket::Create(_)) => Ok(()),
            _ => Err(contract("not a creation ticket")),
        }
    }
    pub fn mutation_started(&mut self) -> Result<(), CoreError> {
        self.binder.validate_held_allowance(&self.allowance)?;
        match self.ticket.as_ref() {
            Some(MutationTicket::Resize(ticket)) if !self.is_started() => {
                ticket.mutation_started()?;
                self.phase = MutationPhase::Started;
                Ok(())
            }
            Some(MutationTicket::Resize(_)) => Ok(()),
            _ => Err(contract("not a resize ticket")),
        }
    }
    pub fn start(&mut self) -> Result<(), CoreError> {
        match self.ticket.as_ref() {
            Some(MutationTicket::Create(_)) => self.creation_started(),
            Some(MutationTicket::Resize(_)) => self.mutation_started(),
            None => Err(contract("mutation already settled")),
        }
    }
    /// Opt this safe, declared Control creation into same-process pending
    /// recovery before the first create syscall. Other producers keep their
    /// existing orphan behavior.
    pub fn retain_control_create_for_recovery(&mut self) -> Result<(), CoreError> {
        if self.is_native()
            || self.phase != MutationPhase::Reserved
            || !matches!(self.ticket, Some(MutationTicket::Create(_)))
            || self.control_create.is_some()
            || self.observed.is_some()
        {
            return Err(contract("Control create recovery opt-in is out of order"));
        }
        let temporary = self
            .declared_temp
            .as_ref()
            .ok_or_else(|| contract("Control create recovery requires declared temp"))?;
        let target_observation = self.anchor.stat()?;
        if target_observation != self.replaced_old.as_ref().map(|(_, _, observed)| *observed)
            || temporary.stat()?.is_some()
        {
            return Err(contract(
                "Control create recovery requires expected target and absent temp",
            ));
        }
        let target = Arc::new(HeldPath::from_anchor(&self.anchor)?);
        let temporary = Arc::new(HeldPath::from_anchor(temporary)?);
        {
            let mut state = lock(&self.binder.shared.state)?;
            require_publication_open(&state)?;
            if state.directory_scopes.len() >= self.binder.shared.max_tracked_entries
                || state.paths_busy.contains(&temporary.relative)
                || state.opening_readers.contains_key(&temporary.relative)
                || directory_scope_busy(&state, &temporary.relative)
            {
                return Err(capacity("Control create temporary scope is busy"));
            }
            if !state.directory_scopes.insert(temporary.relative.clone()) {
                return Err(contract("Control create temporary scope already claimed"));
            }
        }
        self.control_create = Some((target, temporary));
        Ok(())
    }
    /// Declare the private temp pathname before its create syscall.
    pub fn declare_temp_path(&mut self, temp_path: &Path) -> Result<(), CoreError> {
        if !matches!(self.ticket, Some(MutationTicket::Create(_))) || self.control_create.is_some()
        {
            return Err(contract("temp path requires creation ticket"));
        }
        let temp = self.binder.anchor(temp_path)?;
        if temp.relative == self.anchor.relative || temp.stat()?.is_some() {
            return Err(contract(
                "temp path collides with final path or existing entry",
            ));
        }
        self.declared_temp = Some(temp);
        Ok(())
    }
    /// Select an exact private pathname before the caller's `O_EXCL` create.
    /// This method does not create the file. The owned ticket must cross into
    /// the blocking I/O task before calling `creation_started` and create.
    pub fn allocate_private_temp_path(&mut self) -> Result<PathBuf, CoreError> {
        let mut nonce = [0_u8; 16];
        File::open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut nonce))
            .map_err(|error| storage_io("private temp nonce", error))?;
        let mut hex = String::with_capacity(32);
        for byte in nonce {
            use std::fmt::Write as _;
            write!(&mut hex, "{byte:02x}").map_err(|error| storage("temp nonce format", error))?;
        }
        let path = self.binder.state_root().join(format!(".qiv-pub-{hex}"));
        self.declare_temp_path(&path)?;
        Ok(path)
    }
    pub fn created(&mut self, file: &File, temp_path: &Path) -> Result<(), CoreError> {
        if !self.is_started() {
            return Err(contract("creation has not started"));
        }
        if self.observed.is_some() || self.pin.is_some() || self.external_writer_live {
            return Err(contract("creation FD already witnessed"));
        }
        if !matches!(self.ticket, Some(MutationTicket::Create(_))) {
            return Err(contract("created on resize"));
        }
        let temp = self
            .declared_temp
            .as_ref()
            .ok_or_else(|| contract("temp path was not declared before create"))?;
        if self.binder.relative(temp_path)? != temp.relative {
            return Err(contract("created temp path differs from declaration"));
        }
        let observed = InodeObservation::from_file(file)?;
        if temp.stat()? != Some(observed) {
            return Err(contract("temp path and created FD differ"));
        }
        let pin = file
            .try_clone()
            .map_err(|error| storage_io("pin created FD", error))?;
        self.observed = Some(observed);
        self.pin = Some(pin);
        self.safe_writer_fd = Some(file.as_raw_fd());
        self.external_writer_live = true;
        Ok(())
    }

    /// Authenticate the sole safe writer of a visible, non-replacing birth.
    pub fn created_visible(&mut self, file: &File) -> Result<(), CoreError> {
        if self.is_native()
            || !self.is_started()
            || !matches!(self.ticket, Some(MutationTicket::Create(_)))
            || self.replaced_old.is_some()
            || self.declared_temp.is_some()
            || self.observed.is_some()
            || self.pin.is_some()
            || self.external_writer_live
        {
            return Err(contract("visible create FD registration out of order"));
        }
        let observed = InodeObservation::from_file(file)?;
        if observed.links != 1
            || observed.bytes > self.reserved_end
            || self.anchor.stat()? != Some(observed)
        {
            return Err(contract("visible create path and original FD differ"));
        }
        let pin = file
            .try_clone()
            .map_err(|error| storage_io("pin visible create FD", error))?;
        self.observed = Some(observed);
        self.pin = Some(pin);
        self.safe_writer_fd = Some(file.as_raw_fd());
        self.external_writer_live = true;
        Ok(())
    }

    /// Cancel only this original visible birth, never a preexisting file.
    pub fn cancel_visible_create(&mut self, writer: File) -> Result<(), CoreError> {
        let cleanup = (|| {
            if self.is_native()
                || !matches!(self.ticket, Some(MutationTicket::Create(_)))
                || self.replaced_old.is_some()
                || self.declared_temp.is_some()
                || self.safe_writer_fd != Some(writer.as_raw_fd())
            {
                return Err(contract(
                    "visible cancellation lacks original safe create FD",
                ));
            }
            let observed = InodeObservation::from_file(&writer)?;
            if self.observed.map(|prior| prior.inode) != Some(observed.inode)
                || observed.links != 1
                || self.anchor.stat()? != Some(observed)
            {
                return Err(contract("visible cancellation path or inode changed"));
            }
            let held = HeldPath::from_anchor(&self.anchor)?;
            let current = held.current_anchor(&self.binder)?;
            let result = (|| {
                if current.stat()? != Some(observed) {
                    return Err(contract("visible cancellation parent changed"));
                }
                unlinkat(&current.parent, Path::new(&current.name), AtFlags::empty())
                    .map_err(|error| storage_io("unlink cancelled visible file", error))?;
                current.sync_parent()?;
                self.binder.shared.root.require_current_v1()
            })();
            let close = close_owned_file(current.parent).map_err(|error| {
                self.binder.mark_close_unwitnessed();
                storage_io("close visible cancellation parent observer", error)
            });
            match (result, close) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(primary), Ok(())) | (Ok(()), Err(primary)) => Err(primary),
                (Err(primary), Err(close)) => {
                    Err(append_failure(primary, "parent retirement", &close))
                }
            }
        })();
        match cleanup {
            Ok(()) => self.confirmed_safe_absent_after_writer_close(writer),
            Err(primary) => match self.confirmed_safe_writer_fd_retired(writer) {
                Ok(()) => Err(primary),
                Err(close) => Err(append_failure(primary, "visible writer retirement", &close)),
            },
        }
    }

    /// Settle an exact born-but-unregistered temporary only after actual close.
    pub fn cancel_rejected_created_temp(&mut self, writer: File) -> Result<(), CoreError> {
        if self.safe_writer_fd == Some(writer.as_raw_fd()) {
            return self.cancel_declared_temp(writer);
        }
        let cleanup = (|| {
            if self.is_native()
                || !self.is_started()
                || !matches!(self.ticket, Some(MutationTicket::Create(_)))
                || self.observed.is_some()
                || self.pin.is_some()
                || self.external_writer_live
                || self.safe_writer_fd.is_some()
            {
                return Err(contract("rejected create cancellation is out of order"));
            }
            let observed = InodeObservation::from_file(&writer)?;
            let temp = self
                .declared_temp
                .as_ref()
                .ok_or_else(|| contract("rejected create has no declared temporary"))?;
            if observed.links != 1
                || observed.bytes > self.reserved_end
                || temp.stat()? != Some(observed)
            {
                return Err(contract(
                    "rejected create is not the declared original inode",
                ));
            }
            let held = HeldPath::from_anchor(temp)?;
            let current = held.current_anchor(&self.binder)?;
            let result = (|| {
                if current.stat()? != Some(observed) {
                    return Err(contract("rejected create parent or inode changed"));
                }
                unlinkat(&current.parent, Path::new(&current.name), AtFlags::empty())
                    .map_err(|error| storage_io("unlink rejected staged birth", error))?;
                current.sync_parent()?;
                let absent = InodeObservation::from_file(&writer)?;
                if absent.inode != observed.inode || absent.links != 0 {
                    return Err(contract("rejected birth lacks original zero-link proof"));
                }
                self.binder.shared.root.require_current_v1()
            })();
            let close = close_owned_file(current.parent).map_err(|error| {
                self.binder.mark_close_unwitnessed();
                storage_io("close rejected-create parent observer", error)
            });
            match (result, close) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(primary), Ok(())) | (Ok(()), Err(primary)) => Err(primary),
                (Err(primary), Err(close)) => {
                    Err(append_failure(primary, "parent retirement", &close))
                }
            }
        })();
        let close = close_owned_file(writer).map_err(|error| {
            self.binder.mark_close_unwitnessed();
            storage_io("close rejected original writer", error)
        });
        match (cleanup, close) {
            (Ok(()), Ok(())) => self.confirmed_absent_inner(),
            (Err(primary), Ok(())) | (Ok(()), Err(primary)) => Err(primary),
            (Err(primary), Err(close)) => Err(append_failure(primary, "writer retirement", &close)),
        }
    }
    /// Only for an instrumented native delete-on-close temp FD opened under
    /// the leased private root with `SQLITE_OPEN_NOFOLLOW`.
    pub fn observe_unlinked_native_fd(
        &mut self,
        observed: InodeObservation,
    ) -> Result<(), CoreError> {
        if !self.is_started() {
            return Err(contract("creation has not started"));
        }
        if !self.is_native()
            || !matches!(self.ticket, Some(MutationTicket::Create(_)))
            || observed.links != 0
            || self.anchor.stat()?.is_some()
        {
            return Err(contract("unlinked native FD lacks private-temp proof"));
        }
        self.observed = Some(observed);
        Ok(())
    }
    pub fn settle_from_anchored_path(&mut self) -> Result<RetainedId, CoreError> {
        if self.is_native() {
            return Err(contract("native SQLite file must not be reopened"));
        }
        let file = self.anchor.open()?;
        let result = self.settle_after_file_and_parent_sync(&file);
        if let Err(error) = close_owned_file(file) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close settlement witness FD", error));
        }
        result
    }
    pub fn observe_native_fd(&mut self, observed: InodeObservation) -> Result<(), CoreError> {
        if !self.is_native() || !self.is_started() {
            return Err(contract("mutation has not started"));
        }
        if self
            .observed
            .is_some_and(|prior| prior.inode != observed.inode)
        {
            return Err(contract("native FD inode changed"));
        }
        if self.anchor.stat()? != Some(observed) {
            return Err(contract("native FD witness and anchored path differ"));
        }
        self.observed = Some(observed);
        Ok(())
    }
    pub fn before_replace(&self, final_path: &Path) -> Result<(), CoreError> {
        let final_anchor = self.binder.anchor(final_path)?;
        if final_anchor.relative != self.anchor.relative {
            return Err(contract("final path changed"));
        }
        if let Some((_, _, prior)) = &self.replaced_old {
            if final_anchor.stat()? != Some(*prior) {
                return Err(contract("replacement target changed"));
            }
        } else if final_anchor.stat()?.is_some() {
            return Err(contract("new final path unexpectedly exists"));
        }
        self.binder.shared.root.require_current_v1()?;
        Ok(())
    }
    pub fn settle_after_file_and_parent_sync(
        &mut self,
        file: &File,
    ) -> Result<RetainedId, CoreError> {
        if self.is_native() {
            return Err(contract(
                "native SQLite writer requires same-FD xSync witness",
            ));
        }
        file.sync_all()
            .map_err(|error| storage_io("file fsync", error))?;
        let observed = InodeObservation::from_file(file)?;
        self.settle_after_native_sync(observed, None)
    }
    pub fn settle_after_native_sync(
        &mut self,
        observed: InodeObservation,
        diagnostics: Option<&DiagnosticContextV1>,
    ) -> Result<RetainedId, CoreError> {
        self.settle_observed_sync(observed, true, diagnostics)
    }
    /// Settle after the selected file store's completed sync protocol.
    ///
    /// The original writer must be physically closed first;
    /// the binder-owned original pin supplies the remaining actual witness.
    /// This authenticates and commits extent without adding per-file fsyncs.
    pub fn settle_after_external_file_and_parent_sync(&mut self) -> Result<RetainedId, CoreError> {
        if self.is_native() || self.external_writer_live {
            return Err(contract("external sync requires retired safe writer"));
        }
        let observed = InodeObservation::from_file(
            self.pin
                .as_ref()
                .ok_or_else(|| contract("external sync pin absent"))?,
        )?;
        self.settle_observed_sync(observed, false, None)
    }
    /// Close the remaining safe writer pin after retirement and settlement.
    ///
    /// A linked immutable inode needs no artificial read lease.
    pub fn confirmed_safe_pin_retired(&mut self) -> Result<(), CoreError> {
        if self.is_native() || !self.is_settled() || self.external_writer_live {
            return Err(contract("safe pin retirement before writer settlement"));
        }
        let pin = self.pin.take().ok_or_else(|| contract("safe pin absent"))?;
        if let Err(error) = close_owned_file(pin) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close settled safe pin", error));
        }
        Ok(())
    }
    /// Cancel only the declared original temporary while its writer is available.
    ///
    /// Anchored inode authentication precedes unlink; a
    /// canonical name is never removed by this operation.
    pub fn cancel_declared_temp(&mut self, writer: File) -> Result<(), CoreError> {
        let cleanup = (|| {
            if self.is_native() || self.safe_writer_fd != Some(writer.as_raw_fd()) {
                return Err(contract("temporary cancellation lacks original safe FD"));
            }
            let observed = InodeObservation::from_file(&writer)?;
            if self.observed.map(|prior| prior.inode) != Some(observed.inode) {
                return Err(contract("temporary cancellation inode changed"));
            }
            let temp = self
                .declared_temp
                .as_ref()
                .ok_or_else(|| contract("temporary absent"))?;
            // A held directory FD alone does not prove that this parent is
            // still linked in the leased namespace.
            let current = self.binder.anchor(&temp.relative)?;
            let held_parent = temp
                .parent
                .metadata()
                .map_err(|error| storage_io("stat temporary parent", error))?;
            let current_parent = current
                .parent
                .metadata()
                .map_err(|error| storage_io("stat current temporary parent", error))?;
            if held_parent.dev() != current_parent.dev()
                || held_parent.ino() != current_parent.ino()
            {
                return Err(contract("temporary cancellation parent changed"));
            }
            if temp.stat()? != Some(observed) {
                return Err(contract("temporary cancellation path and FD differ"));
            }
            unlinkat(&temp.parent, Path::new(&temp.name), AtFlags::empty())
                .map_err(|error| storage_io("unlink cancelled temporary", error))?;
            temp.sync_parent()?;
            Ok(())
        })();
        if let Err(error) = cleanup {
            // The actual original FD is consumed even on a failed cleanup;
            // retained pending custody must not pretend that Drop closed it.
            let closed = self.confirmed_safe_writer_fd_retired(writer);
            closed?;
            return Err(error);
        }
        self.confirmed_safe_absent_after_writer_close_inner(writer, true)
    }
    fn settle_observed_sync(
        &mut self,
        observed: InodeObservation,
        sync_parent: bool,
        diagnostics: Option<&DiagnosticContextV1>,
    ) -> Result<RetainedId, CoreError> {
        if !self.is_started() {
            return Err(contract("mutation has not started"));
        }
        if self.is_native() {
            // Preserve the original AfterSync observer's inode, anchored-path
            // and witness-retention contract with one anchored stat.
            self.observe_native_fd(observed)?;
        } else {
            let prior = self
                .observed
                .ok_or_else(|| contract("missing FD witness"))?;
            if prior.inode != observed.inode {
                return Err(contract("settlement witness inode differs"));
            }
            if observed.bytes > self.reserved_end {
                return Err(contract("settlement width exceeds reservation"));
            }
            if self.anchor.stat()? != Some(observed) {
                return Err(contract("settled FD and pathname differ"));
            }
        }
        if observed.bytes > self.reserved_end {
            return Err(contract("settlement width exceeds reservation"));
        }
        if sync_parent {
            let span = diagnostics.and_then(|context| {
                crate::runtime_diagnostic_span(
                    context,
                    context.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(21)),
                )
            });
            let result = self.anchor.sync_parent();
            if let Some(span) = span {
                let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
            }
            result?;
        }
        self.binder.shared.root.require_current_v1()?;
        let ticket = self
            .ticket
            .as_mut()
            .ok_or_else(|| contract("mutation ticket absent"))?;
        let retained = match ticket {
            MutationTicket::Create(ticket) => ticket.committed(observed.inode, observed.bytes)?,
            MutationTicket::Resize(ticket) => ticket.settled(observed.inode, observed.bytes)?,
        };
        if self.reserved_inode_slot {
            self.binder.track_retained_reserved(retained, observed)?;
            self.reserved_inode_slot = false;
        } else {
            self.binder.track_retained(retained, observed)?;
        }
        self.ticket = None;
        if let Some((old_file, old_retained, old_prior)) = self.replaced_old.take() {
            let observation_result = (|| {
                let old_after = InodeObservation::from_file(&old_file)?;
                if old_after.inode != old_prior.inode || old_after.bytes != old_prior.bytes {
                    return Err(contract("replaced old inode changed"));
                }
                if old_after.links == 0 {
                    self.binder.track_retained(old_retained, old_after)?;
                }
                Ok(old_after.links == 0)
            })();
            let close_result = close_owned_file(old_file);
            if let Err(error) = close_result {
                self.binder.mark_close_unwitnessed();
                return Err(storage_io("close replaced inode FD", error));
            }
            if observation_result? {
                // The replacement observer FD is not a reader pin. Publish
                // sweep eligibility only after its checked physical close.
                let mut state = lock(&self.binder.shared.state)?;
                let entry = state
                    .inodes
                    .get_mut(&old_prior.inode)
                    .ok_or_else(|| contract("replaced inode tracking missing"))?;
                entry.tombstone_synced = true;
                state.may_have_tombstones = true;
                drop(state);
            }
        }
        self.phase = MutationPhase::Settled;
        Ok(retained)
    }
    pub fn replaced_and_synced(
        &mut self,
        file: &File,
        final_path: &Path,
    ) -> Result<RetainedId, CoreError> {
        if self.binder.relative(final_path)? != self.anchor.relative {
            return Err(contract("replaced final pathname differs"));
        }
        self.settle_after_file_and_parent_sync(file)
    }
    /// A failed create that never produced an FD. The caller must have the
    /// create syscall result and this ticket must have no born inode witness.
    pub fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError> {
        if self.is_native() || self.observed.is_some() || self.pin.is_some() {
            return Err(contract("uncreated absence has a born FD witness"));
        }
        self.confirmed_absent_inner()
    }
    /// Only a native AfterShmMap/no-FD witness may call this when the native
    /// operation did not leave any inode behind. The caller must have proved
    /// that `SQLite` holds no SHM descriptor for this handle.
    pub fn confirmed_native_uncreated_absent_after_no_fd(&mut self) -> Result<(), CoreError> {
        if !self.is_native() || self.observed.is_some() || self.pin.is_some() {
            return Err(contract("native no-FD absence has a born inode witness"));
        }
        self.confirmed_absent_inner()?;
        self.native_covered = true;
        Ok(())
    }
    /// The caller transfers its sole actual safe writer FD after removing its
    /// last link. This closes both that FD and the binder clone before refund.
    /// The caller must not retain untracked duplicates of the same writer.
    pub fn confirmed_safe_absent_after_writer_close(
        &mut self,
        writer: File,
    ) -> Result<(), CoreError> {
        self.confirmed_safe_absent_after_writer_close_inner(writer, false)
    }
    fn confirmed_safe_absent_after_writer_close_inner(
        &mut self,
        writer: File,
        allow_retained_winner: bool,
    ) -> Result<(), CoreError> {
        let observed = InodeObservation::from_file(&writer);
        self.confirmed_safe_writer_fd_retired(writer)?;
        let observed = observed?;
        if observed.links != 0 {
            return Err(contract(
                "safe writer absence lacks zero-link inode witness",
            ));
        }
        let pin = self
            .pin
            .take()
            .ok_or_else(|| contract("safe writer pin missing"))?;
        if let Err(error) = close_owned_file(pin) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close writer pin FD", error));
        }
        self.require_no_tracked_pins(observed.inode)?;
        self.confirmed_absent_with_winner(allow_retained_winner)
    }
    /// Only a C `FdRetired` event (successful native `osClose`) may call this.
    pub fn confirmed_native_absent_after_retirement(
        &mut self,
        observed: InodeObservation,
    ) -> Result<(), CoreError> {
        if !self.is_native()
            || observed.links != 0
            || self.observed.map(|o| o.inode) != Some(observed.inode)
        {
            return Err(contract(
                "native absence lacks retired zero-link FD witness",
            ));
        }
        self.native_covered = true;
        self.require_no_tracked_pins(observed.inode)?;
        self.confirmed_absent_inner()
    }
    fn require_no_tracked_pins(&self, inode: InodeKey) -> Result<(), CoreError> {
        let state = lock(&self.binder.shared.state)?;
        if state
            .inodes
            .get(&inode)
            .is_some_and(|entry| entry.pins != 0)
        {
            return Err(contract("absence inode has live tracked readers"));
        }
        drop(state);
        Ok(())
    }
    fn confirmed_absent_inner(&mut self) -> Result<(), CoreError> {
        self.confirmed_absent_with_winner(false)
    }
    fn confirmed_absent_with_winner(
        &mut self,
        allow_retained_winner: bool,
    ) -> Result<(), CoreError> {
        if !self.is_started() {
            return Err(contract("creation has not started"));
        }
        if let Some(temp) = &self.declared_temp {
            if temp.stat()?.is_some() {
                return Err(contract("temp output still exists"));
            }
            temp.sync_parent()?;
        }
        match &self.replaced_old {
            Some((_, _, old)) if self.anchor.stat()? == Some(*old) => {}
            None if self.anchor.stat()?.is_none() => {}
            None if allow_retained_winner => {
                let winner = self
                    .anchor
                    .stat()?
                    .ok_or_else(|| contract("winner absent"))?;
                if self
                    .binder
                    .shared
                    .authority
                    .retained_inode(winner.inode, winner.bytes)?
                    .is_none()
                {
                    return Err(contract(
                        "no-replace winner is not charged to this authority",
                    ));
                }
            }
            _ => return Err(contract("final pathname changed during failed creation")),
        }
        self.anchor.sync_parent()?;
        match self.ticket.as_mut() {
            Some(MutationTicket::Create(ticket)) => ticket.confirmed_absent_and_synced()?,
            _ => return Err(contract("absence proof requires creation ticket")),
        }
        self.ticket = None;
        self.phase = MutationPhase::Settled;
        if self.reserved_inode_slot {
            self.binder.release_inode_slot();
            self.reserved_inode_slot = false;
        }
        Ok(())
    }
}
impl Drop for FileMutation {
    fn drop(&mut self) {
        if (!self.is_started() || self.is_settled())
            && let Some((_, temporary)) = self.control_create.take()
        {
            self.binder.release_control_temp_scope(&temporary.relative);
        }
        if (self.control_create.is_none() || !self.is_started() || self.is_settled())
            && let Some((old_file, _, _)) = self.replaced_old.take()
            && close_owned_file(old_file).is_err()
        {
            self.binder.mark_close_unwitnessed();
        }
        if !self.is_started() {
            // Core's reserved ticket Drop refunds unused capacity.
            if let Some(pin) = self.pin.take()
                && close_owned_file(pin).is_err()
            {
                self.binder.mark_close_unwitnessed();
            }
            if self.reserved_inode_slot {
                self.binder.release_inode_slot();
                self.reserved_inode_slot = false;
            }
            self.binder.release_path(&self.anchor.relative);
            if let Some(inode) = self.busy_inode.take() {
                self.binder.release_busy_inode(inode);
            }
            return;
        }
        if self.is_settled() {
            // The path may be released only after a tracked ReaderLease has
            // been installed, or an existing lease was proved. If a caller
            // drops a settled writer without either, retain custody.
            if let Some(pin) = self.pin.take() {
                if close_owned_file(pin).is_err() {
                    self.binder.mark_close_unwitnessed();
                }
                return;
            }
            if self.external_writer_live || (self.is_native() && !self.native_covered) {
                return;
            }
            self.binder.release_path(&self.anchor.relative);
            if let Some(inode) = self.busy_inode.take() {
                self.binder.release_busy_inode(inode);
            }
            return;
        }
        // A failed batch may leave a published prefix whose original writer
        // was physically retired, but whose shared barrier never settled.
        // Close our independent pin without settling or refunding the output.
        // The pending born inode and full charge still govern boot inventory;
        // an actual close failure continues to quarantine this root.
        if !self.is_native()
            && self.control_create.is_none()
            && !self.external_writer_live
            && self.safe_writer_fd.is_none()
            && let Some(pin) = self.pin.take()
            && close_owned_file(pin).is_err()
        {
            self.binder.mark_close_unwitnessed();
        }
        // A started Core ticket becomes an uncertain orphan. Keep the path
        // blocked and index its pending ID for anchored recovery.
        if let Some(ticket) = &self.ticket {
            let id = match ticket {
                MutationTicket::Create(t) => t.id(),
                MutationTicket::Resize(t) => t.id(),
            };
            let kind = match ticket {
                MutationTicket::Create(_) => Some(PendingKind::Create),
                MutationTicket::Resize(_) => self.observed.map(|observed| PendingKind::Resize {
                    inode: observed.inode,
                }),
            };
            let inserted = if let Some(kind) = kind {
                if let Ok(mut state) = self.binder.shared.state.lock() {
                    match state.pending.entry(self.anchor.relative.clone()) {
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            let _pending = entry.insert(PendingMutation {
                                id,
                                kind,
                                busy_inode: self.busy_inode,
                                owned_pin: self.pin.take(),
                                reserved_inode_slot: self.reserved_inode_slot,
                                born_inode: self.observed.map(|entry| entry.inode),
                                native: self.is_native(),
                                retired_native: (self.is_native() && self.native_covered)
                                    .then_some(self.observed)
                                    .flatten(),
                                external_writer_live: self.external_writer_live,
                                safe_writer_fd: self.safe_writer_fd,
                                control_create: self.control_create.take().map(
                                    |(target, temporary)| ControlCreatePending {
                                        target,
                                        temporary,
                                        reserved_end: self.reserved_end,
                                        replaced_old: self.replaced_old.take(),
                                        recovering: false,
                                    },
                                ),
                            });
                            true
                        }
                        std::collections::btree_map::Entry::Occupied(_) => false,
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if !inserted {
                self.binder.mark_close_unwitnessed();
                if let Some((old_file, _, _)) = self.replaced_old.take()
                    && close_owned_file(old_file).is_err()
                {
                    self.binder.mark_close_unwitnessed();
                }
                if let Some(pin) = self.pin.take()
                    && close_owned_file(pin).is_err()
                {
                    self.binder.mark_close_unwitnessed();
                }
            }
        }
    }
}

pub struct OpenGuard {
    binder: PublicationFileCapacity,
    anchor: Anchor,
    expected: InodeObservation,
    file: Option<File>,
    finished: bool,
}
impl OpenGuard {
    /// Transfer the actual caller-opened FD into this lease. A borrowed
    /// witness would let the caller silently close its original FD later.
    pub fn opened_owned(mut self, opened: File) -> Result<ReaderLease, CoreError> {
        let mut actual = Some(opened);
        let result = (|| {
            let observed = InodeObservation::from_file(
                actual
                    .as_ref()
                    .ok_or_else(|| contract("reader FD absent"))?,
            )?;
            if observed != self.expected || self.anchor.stat()? != Some(observed) {
                return Err(contract("reader open changed inode or width"));
            }
            let file = self
                .file
                .take()
                .ok_or_else(|| contract("reader pin absent"))?;
            let mut lease = self.binder.pin(observed, file)?;
            lease.actual = actual.take();
            self.finished = true;
            self.binder.release_opening(&self.anchor.relative);
            Ok(lease)
        })();
        if let Some(opened) = actual.take()
            && let Err(error) = close_owned_file(opened)
        {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close rejected reader FD", error));
        }
        result
    }
}
impl Drop for OpenGuard {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(file) = self.file.take()
                && close_owned_file(file).is_err()
            {
                self.binder.mark_close_unwitnessed();
            }
            self.binder.release_opening(&self.anchor.relative);
        }
    }
}

pub struct ReaderLease {
    shared: Arc<Shared>,
    inode: InodeKey,
    /// Binder-owned pin, distinct from the caller's actual opened FD.
    file: Option<File>,
    actual: Option<File>,
    live: bool,
}
impl ReaderLease {
    pub fn file(&self) -> Result<&File, CoreError> {
        self.actual
            .as_ref()
            .or(self.file.as_ref())
            .ok_or_else(|| contract("reader FD already retired"))
    }
    #[must_use]
    pub fn inode(&self) -> InodeKey {
        self.inode
    }
    pub fn retire(mut self) -> Result<(), CoreError> {
        self.release()?;
        Ok(())
    }
    fn release(&mut self) -> Result<(), CoreError> {
        if !self.live {
            return Ok(());
        }
        let had_pin = self.file.is_some();
        let actual_close = self.actual.take().map(close_owned_file).transpose();
        let pin_close = self.file.take().map(close_owned_file).transpose();
        self.live = false;
        let binder = PublicationFileCapacity {
            shared: Arc::clone(&self.shared),
        };
        if let Some(error) = actual_close.err().or_else(|| pin_close.err()) {
            binder.mark_close_unwitnessed();
            return Err(storage_io("close reader FD", error));
        }
        if !had_pin {
            binder.mark_close_unwitnessed();
            return Err(contract("reader lease lacked a binder pin"));
        }
        let should_sweep = (|| {
            let mut state = lock(&self.shared.state)?;
            let entry = state
                .inodes
                .get_mut(&self.inode)
                .ok_or_else(|| contract("reader inode missing"))?;
            entry.pins = entry
                .pins
                .checked_sub(1)
                .ok_or_else(|| contract("reader pin underflow"))?;
            let should_sweep = entry.tombstone_synced && entry.pins == 0;
            state.total_pins = state
                .total_pins
                .checked_sub(1)
                .ok_or_else(|| contract("total pin underflow"))?;
            drop(state);
            Ok(should_sweep)
        })();
        match should_sweep {
            Ok(true) => binder.sweep_retired(),
            Ok(false) => Ok(()),
            Err(error) => {
                binder.mark_close_unwitnessed();
                Err(error)
            }
        }
    }
}
impl Drop for ReaderLease {
    fn drop(&mut self) {
        let _release_result = self.release();
    }
}

/// Logical pin for an externally owned FD. It never opens or duplicates that
/// FD. The instrumented VFS or admitted filesystem owner retires it only after
/// an actual physical-close witness.
pub struct NativeReaderLease {
    shared: Arc<Shared>,
    inode: InodeKey,
    live: bool,
}
impl NativeReaderLease {
    #[must_use]
    pub fn inode(&self) -> InodeKey {
        self.inode
    }
    pub fn retire(mut self) -> Result<(), CoreError> {
        self.release()
    }
    fn release(&mut self) -> Result<(), CoreError> {
        if !self.live {
            return Ok(());
        }
        let mut state = lock(&self.shared.state)?;
        let entry = state
            .inodes
            .get_mut(&self.inode)
            .ok_or_else(|| contract("native pin inode missing"))?;
        entry.pins = entry
            .pins
            .checked_sub(1)
            .ok_or_else(|| contract("native pin underflow"))?;
        state.total_pins = state
            .total_pins
            .checked_sub(1)
            .ok_or_else(|| contract("total pin underflow"))?;
        drop(state);
        self.live = false;
        Ok(())
    }
}
impl Drop for NativeReaderLease {
    fn drop(&mut self) {
        // A lost physical-close witness must never release an inode charge.
        // The adapter calls `retire` only upon FdRetired.
    }
}

pub struct NativeOpenGuard {
    binder: PublicationFileCapacity,
    anchor: Anchor,
    expected: InodeObservation,
    finished: bool,
}
impl NativeOpenGuard {
    pub fn opened_witness(
        &mut self,
        observed: InodeObservation,
    ) -> Result<NativeReaderLease, CoreError> {
        if observed != self.expected {
            return Err(contract("native open inode/width changed"));
        }
        let lease = self
            .binder
            .pin_native_witness(&self.anchor.relative, observed)?;
        self.finished = true;
        self.binder.release_opening(&self.anchor.relative);
        Ok(lease)
    }
}
impl Drop for NativeOpenGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.binder.release_opening(&self.anchor.relative);
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum PathAction {
    Link,
    Move,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathRecovery {
    Changed,
    Unchanged,
}
pub struct PathTicket {
    binder: PublicationFileCapacity,
    from: Anchor,
    to: Anchor,
    source: Option<File>,
    expected: InodeObservation,
    target_before: Option<InodeObservation>,
    replaced_target: Option<(File, RetainedId)>,
    action: PathAction,
    settled: bool,
    started: bool,
}
impl PublicationFileCapacity {
    fn begin_path_change(
        &self,
        from: &Path,
        to: &Path,
        action: PathAction,
    ) -> Result<PathTicket, CoreError> {
        let from = self.anchor(from)?;
        let to = self.anchor(to)?;
        if from.relative == to.relative {
            return Err(contract("source equals destination"));
        }
        let mut source = Some(from.open()?);
        let mut target_file = None;
        let result = (|| {
            let expected = InodeObservation::from_file(
                source
                    .as_ref()
                    .ok_or_else(|| contract("path source FD absent"))?,
            )?;
            if self
                .shared
                .authority
                .retained_inode(expected.inode, expected.bytes)?
                .is_none()
            {
                return Err(contract("path source absent from authority"));
            }
            let target_before = to.stat()?;
            let replaced_retained = match target_before {
                Some(_) if matches!(action, PathAction::Link) => {
                    return Err(existing_target("link"));
                }
                Some(prior) => {
                    if prior.inode == expected.inode {
                        return Err(contract("move target aliases source inode"));
                    }
                    target_file = Some(to.open()?);
                    let opened = InodeObservation::from_file(
                        target_file
                            .as_ref()
                            .ok_or_else(|| contract("path target FD absent"))?,
                    )?;
                    if opened != prior {
                        return Err(contract("path target changed during admission"));
                    }
                    Some(
                        self.shared
                            .authority
                            .retained_inode(prior.inode, prior.bytes)?
                            .ok_or_else(|| contract("path target absent from authority"))?,
                    )
                }
                None => None,
            };
            let mut state = lock(&self.shared.state)?;
            require_publication_open(&state)?;
            if state.paths_busy.len().saturating_add(2) > self.shared.max_tracked_entries
                || state.paths_busy.contains(&from.relative)
                || state.paths_busy.contains(&to.relative)
                || directory_scope_busy(&state, &from.relative)
                || directory_scope_busy(&state, &to.relative)
                || state.opening_readers.contains_key(&from.relative)
                || state.opening_readers.contains_key(&to.relative)
                || state.inodes_busy.contains(&expected.inode)
                || target_before.is_some_and(|prior| state.inodes_busy.contains(&prior.inode))
            {
                return Err(capacity("path-change registry busy or full"));
            }
            if !state.paths_busy.insert(from.relative.clone())
                || !state.paths_busy.insert(to.relative.clone())
                || !state.inodes_busy.insert(expected.inode)
                || target_before.is_some_and(|prior| !state.inodes_busy.insert(prior.inode))
            {
                return Err(contract("path-change claim unexpectedly collided"));
            }
            drop(state);
            Ok(PathTicket {
                binder: self.clone(),
                from,
                to,
                source: Some(
                    source
                        .take()
                        .ok_or_else(|| contract("path source FD absent"))?,
                ),
                expected,
                target_before,
                replaced_target: match replaced_retained {
                    Some(retained) => Some((
                        target_file
                            .take()
                            .ok_or_else(|| contract("path target FD absent"))?,
                        retained,
                    )),
                    None => None,
                },
                action,
                settled: false,
                started: false,
            })
        })();
        if let Some(file) = source.take()
            && let Err(error) = close_owned_file(file)
        {
            self.mark_close_unwitnessed();
            return Err(storage_io("close rejected path source FD", error));
        }
        if let Some(file) = target_file.take()
            && let Err(error) = close_owned_file(file)
        {
            self.mark_close_unwitnessed();
            return Err(storage_io("close rejected path target FD", error));
        }
        result
    }
    pub fn begin_link(&self, from: &Path, to: &Path) -> Result<PathTicket, CoreError> {
        self.begin_path_change(from, to, PathAction::Link)
    }
    pub fn begin_move(&self, from: &Path, to: &Path) -> Result<PathTicket, CoreError> {
        self.begin_path_change(from, to, PathAction::Move)
    }
    /// Admit a move only when its destination is absent. The resulting ticket
    /// keeps the absent-target witness and uses `RENAME_NOREPLACE` at the syscall.
    pub fn begin_move_if_absent(&self, from: &Path, to: &Path) -> Result<PathTicket, CoreError> {
        let ticket = self.begin_path_change(from, to, PathAction::Move)?;
        if ticket.target_before.is_some() {
            return Err(existing_target("create-mode move"));
        }
        Ok(ticket)
    }
    pub fn begin_delete(&self, path: &Path) -> Result<DeleteTicket, CoreError> {
        let anchor = self.anchor(path)?;
        let mut source = Some(anchor.open()?);
        let result = (|| {
            let expected = InodeObservation::from_file(
                source
                    .as_ref()
                    .ok_or_else(|| contract("delete source FD absent"))?,
            )?;
            let retained = self
                .shared
                .authority
                .retained_inode(expected.inode, expected.bytes)?
                .ok_or_else(|| contract("delete inode absent from authority"))?;
            let mut state = lock(&self.shared.state)?;
            require_publication_open(&state)?;
            if state.paths_busy.len() >= self.shared.max_tracked_entries
                || state.paths_busy.contains(&anchor.relative)
                || directory_scope_busy(&state, &anchor.relative)
                || state.opening_readers.contains_key(&anchor.relative)
                || state.inodes_busy.contains(&expected.inode)
            {
                return Err(capacity("delete path busy or registry full"));
            }
            if !state.paths_busy.insert(anchor.relative.clone()) {
                return Err(contract("delete path claim unexpectedly collided"));
            }
            drop(state);
            Ok(DeleteTicket {
                binder: self.clone(),
                anchor,
                source: Some(
                    source
                        .take()
                        .ok_or_else(|| contract("delete source FD absent"))?,
                ),
                expected,
                retained,
                settled: false,
                started: false,
            })
        })();
        if let Some(file) = source.take()
            && let Err(error) = close_owned_file(file)
        {
            self.mark_close_unwitnessed();
            return Err(storage_io("close rejected delete source FD", error));
        }
        result
    }
    /// `SQLite` `xDelete` path: no extra FD is opened, preserving unix `fcntl`
    /// locks held by the native VFS. Namespace custody is established before
    /// native unlink; the last-link proof uses the pre-unlink nlink witness.
    pub fn begin_native_delete(&self, path: &Path) -> Result<NativeDeleteTicket, CoreError> {
        let anchor = self.anchor(path)?;
        let expected = anchor
            .stat()?
            .ok_or_else(|| contract("native delete path absent"))?;
        let retained = self
            .shared
            .authority
            .retained_inode(expected.inode, expected.bytes)?
            .ok_or_else(|| contract("native delete inode absent from sealed authority"))?;
        let mut state = lock(&self.shared.state)?;
        require_publication_open(&state)?;
        if state.paths_busy.len() >= self.shared.max_tracked_entries
            || state.paths_busy.contains(&anchor.relative)
            || directory_scope_busy(&state, &anchor.relative)
            || state.opening_readers.contains_key(&anchor.relative)
            || state.inodes_busy.contains(&expected.inode)
        {
            return Err(capacity("native delete pathname busy or registry full"));
        }
        if !state.paths_busy.insert(anchor.relative.clone()) {
            return Err(contract("native delete path claim unexpectedly collided"));
        }
        drop(state);
        Ok(NativeDeleteTicket {
            binder: self.clone(),
            anchor,
            expected,
            retained,
            started: false,
            settled: false,
        })
    }
}

pub struct NativeDeleteTicket {
    binder: PublicationFileCapacity,
    anchor: Anchor,
    expected: InodeObservation,
    retained: RetainedId,
    started: bool,
    settled: bool,
}
impl NativeDeleteTicket {
    pub fn deletion_started(&mut self) -> Result<(), CoreError> {
        if self.started || self.settled || self.anchor.stat()? != Some(self.expected) {
            return Err(contract("native delete changed before syscall"));
        }
        self.started = true;
        Ok(())
    }
    pub fn unlinked_and_synced(&mut self) -> Result<(), CoreError> {
        if !self.started || self.anchor.stat()?.is_some() {
            return Err(contract("native unlink lacks absence proof"));
        }
        self.anchor.sync_parent()?;
        self.binder.shared.root.require_current_v1()?;
        if self.expected.links == 1 {
            self.binder.track_retained(self.retained, self.expected)?;
            let mut state = lock(&self.binder.shared.state)?;
            let entry = state
                .inodes
                .get_mut(&self.expected.inode)
                .ok_or_else(|| contract("native deleted inode untracked"))?;
            entry.tombstone_synced = true;
            state.may_have_tombstones = true;
            drop(state);
        }
        self.settled = true;
        self.binder.release_path(&self.anchor.relative);
        self.binder.sweep_retired()?;
        Ok(())
    }
    pub fn confirmed_unchanged_after_failure(&mut self) -> Result<(), CoreError> {
        if !self.started || self.anchor.stat()? != Some(self.expected) {
            return Err(contract("native delete failure lacks unchanged proof"));
        }
        self.anchor.sync_parent()?;
        self.settled = true;
        self.binder.release_path(&self.anchor.relative);
        Ok(())
    }
}
impl Drop for NativeDeleteTicket {
    fn drop(&mut self) {
        if !self.started && !self.settled {
            self.binder.release_path(&self.anchor.relative);
        }
    }
}
impl PathTicket {
    #[must_use]
    pub fn has_started(&self) -> bool {
        self.started
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.settled
    }

    /// Return the original source and held parent FD witnesses only while the
    /// canonical source still matches the admitted inode and parent.
    pub fn source_metadata(&self) -> Result<(std::fs::Metadata, std::fs::Metadata), CoreError> {
        self.require_current_parent(&self.from)?;
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| contract("path source FD missing"))?
            .metadata()
            .map_err(|error| storage_io("stat path source FD", error))?;
        if !source.is_file()
            || (InodeKey {
                device: source.dev(),
                inode: source.ino(),
            }) != self.expected.inode
            || source.len() != self.expected.bytes
            || source.nlink() != self.expected.links
            || self.from.stat()? != Some(self.expected)
        {
            return Err(contract("path source FD or canonical entry changed"));
        }
        let parent = self
            .from
            .parent
            .metadata()
            .map_err(|error| storage_io("stat held path source parent", error))?;
        if !parent.is_dir() {
            return Err(contract("held path source parent is not a directory"));
        }
        Ok((source, parent))
    }

    fn require_current_parent(&self, anchor: &Anchor) -> Result<(), CoreError> {
        let current = self.binder.anchor(&anchor.relative)?;
        let checked = (|| {
            let held = anchor
                .parent
                .metadata()
                .map_err(|error| storage_io("stat held path parent", error))?;
            let actual = current
                .parent
                .metadata()
                .map_err(|error| storage_io("stat current path parent", error))?;
            if !held.is_dir()
                || !actual.is_dir()
                || held.dev() != actual.dev()
                || held.ino() != actual.ino()
                || anchor.name != current.name
                || anchor.relative != current.relative
            {
                return Err(contract("path parent incarnation changed"));
            }
            Ok(())
        })();
        if let Err(error) = close_owned_file(current.parent) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close current path parent observer", error));
        }
        checked
    }

    fn require_current_parents(&self) -> Result<(), CoreError> {
        self.require_current_parent(&self.from)?;
        self.require_current_parent(&self.to)
    }

    fn release_claims(&self) {
        self.binder.release_path(&self.from.relative);
        self.binder.release_path(&self.to.relative);
        self.binder.release_busy_inode(self.expected.inode);
        if let Some(prior) = self.target_before {
            self.binder.release_busy_inode(prior.inode);
        }
    }
    pub fn change_started(&mut self) -> Result<(), CoreError> {
        self.require_current_parents()?;
        if self.started || self.settled {
            return Err(contract("path change started twice"));
        }
        if self.from.stat()? != Some(self.expected) || self.to.stat()? != self.target_before {
            return Err(contract("path changed before operation"));
        }
        if InodeObservation::from_file(
            self.source
                .as_ref()
                .ok_or_else(|| contract("path source FD missing"))?,
        )? != self.expected
        {
            return Err(contract("path source FD changed before operation"));
        }
        if let Some(prior) = self.target_before
            && InodeObservation::from_file(
                &self
                    .replaced_target
                    .as_ref()
                    .ok_or_else(|| contract("path target FD missing"))?
                    .0,
            )? != prior
        {
            return Err(contract("path target FD changed before operation"));
        }
        self.started = true;
        Ok(())
    }
    pub fn perform_and_sync(&mut self) -> Result<(), CoreError> {
        self.change_started()?;
        match self.action {
            PathAction::Link => linkat(
                &self.from.parent,
                Path::new(&self.from.name),
                &self.to.parent,
                Path::new(&self.to.name),
                AtFlags::empty(),
            )
            .map_err(|error| storage_io("linkat", error))?,
            PathAction::Move => {
                let flags = if self.target_before.is_some() {
                    RenameFlags::empty()
                } else {
                    RenameFlags::NOREPLACE
                };
                renameat_with(
                    &self.from.parent,
                    Path::new(&self.from.name),
                    &self.to.parent,
                    Path::new(&self.to.name),
                    flags,
                )
                .map_err(|error| storage_io("renameat", error))?;
            }
        }
        self.changed_and_synced()
    }
    pub fn changed_and_synced(&mut self) -> Result<(), CoreError> {
        self.require_current_parents()?;
        if !self.started {
            return Err(contract("path change not started"));
        }
        let target = self
            .to
            .stat()?
            .ok_or_else(|| contract("target path absent"))?;
        if target.inode != self.expected.inode || target.bytes != self.expected.bytes {
            return Err(contract("target path witness differs"));
        }
        if matches!(self.action, PathAction::Move) && self.from.stat()?.is_some() {
            return Err(contract("move source remains"));
        }
        let source = InodeObservation::from_file(
            self.source
                .as_ref()
                .ok_or_else(|| contract("path source FD missing"))?,
        )?;
        if source.inode != self.expected.inode || source.bytes != self.expected.bytes {
            return Err(contract("source FD changed"));
        }
        self.from.sync_parent()?;
        self.to.sync_parent()?;
        self.require_current_parents()?;
        self.binder.shared.root.require_current_v1()?;
        let old_zero_link = if let Some((file, retained)) = self.replaced_target.as_ref() {
            let prior = self
                .target_before
                .ok_or_else(|| contract("replaced target observation absent"))?;
            let after = InodeObservation::from_file(file)?;
            if after.inode != prior.inode || after.bytes != prior.bytes {
                return Err(contract("replaced target FD changed"));
            }
            if after.links == 0 {
                self.binder.track_retained(*retained, after)?;
            }
            after.links == 0
        } else {
            false
        };
        let file = self
            .source
            .take()
            .ok_or_else(|| contract("path source FD missing"))?;
        if let Err(error) = close_owned_file(file) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close path source FD", error));
        }
        if let Some((file, _)) = self.replaced_target.take()
            && let Err(error) = close_owned_file(file)
        {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close replaced path target FD", error));
        }
        if old_zero_link {
            let prior = self
                .target_before
                .ok_or_else(|| contract("replaced target observation absent"))?;
            let mut state = lock(&self.binder.shared.state)?;
            let entry = state
                .inodes
                .get_mut(&prior.inode)
                .ok_or_else(|| contract("replaced target tracking missing"))?;
            entry.tombstone_synced = true;
            state.may_have_tombstones = true;
            drop(state);
        }
        self.settled = true;
        self.release_claims();
        if old_zero_link {
            self.binder.sweep_retired()?;
        }
        Ok(())
    }
    pub fn confirmed_unchanged_after_failure(&mut self) -> Result<(), CoreError> {
        self.require_current_parents()?;
        if !self.started
            || self.settled
            || self.from.stat()? != Some(self.expected)
            || self.to.stat()? != self.target_before
        {
            return Err(contract("failed path change lacks unchanged proof"));
        }
        if InodeObservation::from_file(
            self.source
                .as_ref()
                .ok_or_else(|| contract("path source FD missing"))?,
        )? != self.expected
        {
            return Err(contract("failed path change source FD changed"));
        }
        if let Some(prior) = self.target_before
            && InodeObservation::from_file(
                &self
                    .replaced_target
                    .as_ref()
                    .ok_or_else(|| contract("path target FD missing"))?
                    .0,
            )? != prior
        {
            return Err(contract("failed path change target FD changed"));
        }
        self.from.sync_parent()?;
        self.to.sync_parent()?;
        self.require_current_parents()?;
        self.binder.shared.root.require_current_v1()?;
        let file = self
            .source
            .take()
            .ok_or_else(|| contract("path source FD missing"))?;
        if let Err(error) = close_owned_file(file) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close failed-path source FD", error));
        }
        if let Some((file, _)) = self.replaced_target.take()
            && let Err(error) = close_owned_file(file)
        {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close failed-path target FD", error));
        }
        self.settled = true;
        self.release_claims();
        Ok(())
    }

    /// Reconcile a started path operation using the original physical FDs.
    /// A mixed or foreign namespace retains the claims and all witnesses.
    pub fn reconcile_after_failure(&mut self) -> Result<PathRecovery, CoreError> {
        if !self.started || self.settled {
            return Err(contract(
                "path operation has no started ticket to reconcile",
            ));
        }
        self.require_current_parents()?;
        let from = self.from.stat()?;
        let to = self.to.stat()?;
        let source = InodeObservation::from_file(
            self.source
                .as_ref()
                .ok_or_else(|| contract("path source FD missing"))?,
        )?;
        let prior_file = self
            .replaced_target
            .as_ref()
            .map(|(file, _retained)| InodeObservation::from_file(file))
            .transpose()?;
        let unchanged = from == Some(self.expected)
            && to == self.target_before
            && source == self.expected
            && prior_file == self.target_before;
        let changed = match self.action {
            PathAction::Link => {
                let links = self
                    .expected
                    .links
                    .checked_add(1)
                    .ok_or_else(|| contract("linked source count overflow"))?;
                let linked = InodeObservation {
                    links,
                    ..self.expected
                };
                from == Some(linked) && to == Some(linked) && source == linked
            }
            PathAction::Move => {
                let old_after = self
                    .target_before
                    .map(|prior| {
                        let links = prior
                            .links
                            .checked_sub(1)
                            .ok_or_else(|| contract("replaced target link count underflow"))?;
                        Ok::<_, CoreError>(InodeObservation { links, ..prior })
                    })
                    .transpose()?;
                from.is_none()
                    && to == Some(self.expected)
                    && source == self.expected
                    && prior_file == old_after
            }
        };
        if changed {
            self.changed_and_synced()?;
            return Ok(PathRecovery::Changed);
        }
        if unchanged {
            self.confirmed_unchanged_after_failure()?;
            return Ok(PathRecovery::Unchanged);
        }
        Err(contract(
            "started path operation has mixed or foreign witnesses",
        ))
    }
}
impl Drop for PathTicket {
    fn drop(&mut self) {
        if let Some(file) = self.source.take()
            && close_owned_file(file).is_err()
        {
            self.binder.mark_close_unwitnessed();
        }
        if let Some((file, _)) = self.replaced_target.take()
            && close_owned_file(file).is_err()
        {
            self.binder.mark_close_unwitnessed();
        }
        if self.settled || self.started {
            return;
        }
        self.release_claims();
    }
}

pub struct DeleteTicket {
    binder: PublicationFileCapacity,
    anchor: Anchor,
    source: Option<File>,
    expected: InodeObservation,
    retained: RetainedId,
    settled: bool,
    started: bool,
}
impl DeleteTicket {
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.settled
    }
    #[must_use]
    pub fn has_started(&self) -> bool {
        self.started
    }
    /// Observe the actual retained source and anchored parent before deletion.
    /// The returned metadata grants no mutation or retirement authority.
    pub fn source_metadata(&self) -> Result<(std::fs::Metadata, std::fs::Metadata), CoreError> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| contract("delete source FD absent"))?;
        Ok((
            source
                .metadata()
                .map_err(|error| storage_io("delete source metadata", error))?,
            self.anchor
                .parent
                .metadata()
                .map_err(|error| storage_io("delete parent metadata", error))?,
        ))
    }

    pub fn deletion_started(&mut self) -> Result<(), CoreError> {
        if self.started || self.settled {
            return Err(contract("delete started twice"));
        }
        if self.anchor.stat()? != Some(self.expected) {
            return Err(contract("delete source changed"));
        }
        self.started = true;
        Ok(())
    }
    pub fn perform_and_sync(&mut self) -> Result<(), CoreError> {
        self.deletion_started()?;
        unlinkat(
            &self.anchor.parent,
            Path::new(&self.anchor.name),
            AtFlags::empty(),
        )
        .map_err(|error| storage_io("unlinkat", error))?;
        self.unlinked_and_synced()
    }
    pub fn unlinked_and_synced(&mut self) -> Result<(), CoreError> {
        if !self.started {
            return Err(contract("delete not started"));
        }
        if self.anchor.stat()?.is_some() {
            return Err(contract("unlink target still exists"));
        }
        self.anchor.sync_parent()?;
        self.binder.shared.root.require_current_v1()?;
        // Observation or tracking refusal is retryable with this exact ticket.
        // Keep the actual observer until those checks succeed; taking and
        // closing it on refusal strands the started path claim.
        let observation_result = (|| {
            let source = self
                .source
                .as_ref()
                .ok_or_else(|| contract("delete source FD missing"))?;
            let observed = InodeObservation::from_file(source)?;
            if observed.inode != self.expected.inode || observed.bytes != self.expected.bytes {
                return Err(contract("delete source inode/width changed"));
            }
            if observed.links == 0 {
                self.binder.track_retained(self.retained, observed)?;
            }
            Ok(observed.links == 0)
        })()?;
        let source = self
            .source
            .take()
            .ok_or_else(|| contract("delete source FD missing"))?;
        if let Err(error) = close_owned_file(source) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close deleted source FD", error));
        }
        if observation_result {
            // A concurrent last-reader Drop may sweep immediately; the
            // deletion observer must already be physically closed here.
            let mut state = lock(&self.binder.shared.state)?;
            let entry = state
                .inodes
                .get_mut(&self.expected.inode)
                .ok_or_else(|| contract("deleted inode untracked"))?;
            entry.tombstone_synced = true;
            state.may_have_tombstones = true;
            drop(state);
        }
        self.settled = true;
        self.binder.release_path(&self.anchor.relative);
        self.binder.sweep_retired()?;
        Ok(())
    }
    pub fn confirmed_unchanged_after_failure(&mut self) -> Result<(), CoreError> {
        if !self.started || self.settled || self.anchor.stat()? != Some(self.expected) {
            return Err(contract("failed delete lacks unchanged proof"));
        }
        self.anchor.sync_parent()?;
        self.binder.shared.root.require_current_v1()?;
        let source = self
            .source
            .take()
            .ok_or_else(|| contract("delete source FD missing"))?;
        if let Err(error) = close_owned_file(source) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io("close unchanged-delete FD", error));
        }
        self.settled = true;
        self.binder.release_path(&self.anchor.relative);
        Ok(())
    }
}
impl Drop for DeleteTicket {
    fn drop(&mut self) {
        if let Some(source) = self.source.take()
            && close_owned_file(source).is_err()
        {
            self.binder.mark_close_unwitnessed();
        }
        if self.settled || self.started {
            return;
        }
        self.binder.release_path(&self.anchor.relative);
    }
}

#[cfg(test)]
#[path = "publication_file_capacity/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "publication_file_capacity/maintenance_retirement_tests.rs"]
mod maintenance_retirement_tests;

#[path = "publication_directory_capacity.rs"]
mod directory;
pub use directory::{DirectoryDeleteTicket, DirectoryMoveRecovery, DirectoryPathTicket};
