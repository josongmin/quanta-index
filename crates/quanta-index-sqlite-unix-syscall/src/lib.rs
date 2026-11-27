//! Unix file custody for SQLite.
//!
//! The Linux protected reader uses a named, FD-only VFS without changing the
//! Unix syscall table. The separate mutation-hook installer does change that
//! process-global table and requires an unsafe exclusive-process permit.
//! Its wrapper releases the logical owner acquired by xOpen after xClose.

#![cfg(unix)]

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("SQLite Unix syscall custody is only implemented on Linux and macOS");

#[cfg(target_os = "linux")]
mod protected_lock;
#[cfg(target_os = "linux")]
mod protected_read;
#[cfg(target_os = "linux")]
pub use protected_read::{
    ProtectedFileIdentity, ProtectedReadError, ProtectedReadExpectation, ProtectedReadLimits,
    with_protected_read,
};

use std::{
    alloc::{Layout, alloc, dealloc},
    cell::Cell,
    ffi::{CStr, c_char, c_int, c_void},
    mem::MaybeUninit,
    num::{NonZeroU64, NonZeroUsize},
    os::fd::RawFd,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
};

use quanta_index_sqlite_fd_ledger::{
    CloseReport, FdLedger, FdToken, FileIdentity, LedgerError, OpenDisposition, OpenError,
    RetirementReceipt, SharedOwnerToken,
};
pub use quanta_sqlite_retained_grant::RetainedMemoryGrant;

type Open = unsafe extern "C" fn(*const c_char, c_int, c_int) -> c_int;
type Close = unsafe extern "C" fn(c_int) -> c_int;
type Pwrite = unsafe extern "C" fn(c_int, *const c_void, usize, libc::off_t) -> isize;
type Ftruncate = unsafe extern "C" fn(c_int, libc::off_t) -> c_int;

/// The caller must own every SQLite connection in this process. Because
/// xSetSystemCall affects the global Unix table, another SQLite consumer must
/// run in a different process.
pub struct ExclusiveProcessPermit(());

impl ExclusiveProcessPermit {
    /// # Safety
    /// This method cannot verify exclusivity. The caller must launch a
    /// dedicated SQLite process, establish that no other library has installed
    /// Unix syscall overrides, and call this before any SQLite connection or
    /// concurrent SQLite worker exists. A named VFS does not isolate the
    /// process-global Unix syscall table.
    pub unsafe fn assume_exclusive_sqlite_process() -> Self {
        Self(())
    }
}

pub enum OpenDecision {
    /// One file was admitted and charged before its physical open.
    Admit {
        root: NonZeroU64,
        charge: NonZeroU64,
    },
    /// An admitted SHM FD opened from xShmMap outside xOpen capture.
    /// The named VFS binds its initial and subsequent owners to one node.
    AdmitShared {
        root: NonZeroU64,
        charge: NonZeroU64,
    },
    /// Explicitly allow a non-state SQLite auxiliary path such as urandom.
    Auxiliary,
    /// Fail closed. Only positive errno values are valid.
    Deny(c_int),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileMutationKind {
    Write,
    Truncate,
}

/// The implementation must classify every pathname in the dedicated process.
/// It must reject aliases or unexpected paths that could reach a managed root.
/// Callback implementations must not call SQLite or unwind.
pub trait OpenAuthority: Send + Sync + 'static {
    fn decide(&self, path: &CStr) -> OpenDecision;
    /// Open an admitted file relative to a held root FD. This callback runs
    /// under the FD ledger lock and must not release its charge on failure.
    fn open_admitted(&self, path: &CStr, flags: c_int, mode: c_int) -> Result<RawFd, c_int>;
    fn opened(&self, path: &CStr, token: FdToken);
    /// Undo a pre-open charge only when no physical FD was obtained.
    fn open_refused(&self, charge: NonZeroU64);
    /// Retain the charge for reconciliation; the FD was opened without a
    /// usable identity or physical close was uncertain.
    fn quarantined(&self, charge: NonZeroU64);
    fn retired(&self, receipt: RetirementReceipt);
    /// Reserve the incremental SHM extent before the Unix VFS changes the
    /// file. The token identifies the live SHM descriptor, not the DB file.
    /// The reservation must be atomic with the root's other file allowances.
    /// `shm_growth_settled` releases only the unused portion; the actual
    /// growth remains charged until this FD's physical retirement receipt.
    fn reserve_shm_growth(
        &self,
        token: FdToken,
        previous_len: u64,
        requested_len: u64,
    ) -> Result<NonZeroU64, c_int>;
    /// Settle a reservation against the actual length after the syscall.
    /// Failure may leave a partially grown file, so `observed_len` is never
    /// inferred solely from the syscall return value. An unknown final size
    /// is reported as `requested_len` and must remain funded conservatively.
    /// The authority must key retained growth by FD generation, so a reused
    /// numeric descriptor cannot retire another generation's allowance.
    fn shm_growth_settled(&self, reservation: NonZeroU64, observed_len: u64);
    /// Reserve an ordinary SQLite FD mutation before the lower VFS can write.
    /// A zero-growth overwrite still needs a ticket for durability tracking.
    fn reserve_file_mutation(
        &self,
        token: FdToken,
        kind: FileMutationKind,
        previous_len: u64,
        requested_len: u64,
    ) -> Result<NonZeroU64, c_int>;
    /// The lower operation may partially change the file even on failure.
    /// An unknown final length is reported as the larger of old/requested.
    fn file_mutation_settled(
        &self,
        reservation: NonZeroU64,
        observed_len: u64,
        sqlite_rc: c_int,
    ) -> Result<(), c_int>;
    /// Called after the lower xSync, including failures. A successful lower
    /// sync must not be reported to SQLite if the authority cannot settle it.
    fn file_synced(
        &self,
        token: FdToken,
        observed_len: Option<u64>,
        sqlite_rc: c_int,
    ) -> Result<(), c_int>;
    /// Authorize a pathname deletion before the lower VFS unlinks it.
    fn reserve_delete(&self, path: &CStr) -> Result<NonZeroU64, c_int>;
    /// A failed lower deletion must retain ambiguous charges for reconciliation.
    fn delete_settled(&self, reservation: NonZeroU64, sqlite_rc: c_int) -> Result<(), c_int>;
}

struct HookState {
    original_open: Open,
    original_close: Close,
    original_pwrite: Pwrite,
    original_ftruncate: Ftruncate,
    ledger: Arc<FdLedger>,
    authority: Arc<dyn OpenAuthority>,
    physical_closes: AtomicU64,
}

static HOOK: OnceLock<HookState> = OnceLock::new();
static INSTALL_STATE: AtomicU8 = AtomicU8::new(0);
static OWNER_VFS_STATE: AtomicU8 = AtomicU8::new(0);
static LOWER_VFS: AtomicUsize = AtomicUsize::new(0);
static FILES: OnceLock<Mutex<OwnerTable>> = OnceLock::new();
static SHM_SERIAL: Mutex<()> = Mutex::new(());
static FILE_MUTATION_SERIAL: Mutex<()> = Mutex::new(());
thread_local! {
    static OPEN_SLOT: Cell<Option<usize>> = const { Cell::new(None) };
    static SHM_SLOT: Cell<Option<usize>> = const { Cell::new(None) };
}
const INSTALLING: u8 = 1;
const INSTALLED: u8 = 2;
const FAILED: u8 = 3;

#[derive(Clone, Copy)]
pub struct InstalledHook(());

#[derive(Debug, Eq, PartialEq)]
pub enum InstallError {
    AlreadyInstalled,
    InstallInProgress,
    PreviousFailure,
    UnixVfsUnavailable,
    UnsupportedVersion,
    MissingSyscall,
    OverrideRejected,
    MissingOpenMethod,
    RegisterRejected,
    InvalidGrant,
    MemoryUnavailable,
}

fn verify_grant(required: u64, grant: &dyn RetainedMemoryGrant) -> Result<(), InstallError> {
    if grant.bytes() == required {
        Ok(())
    } else {
        Err(InstallError::InvalidGrant)
    }
}

#[derive(Clone, Copy)]
pub struct OwnerLimits {
    pub max_files: NonZeroUsize,
}

const MAX_TOKENS_PER_OPEN: usize = 8;

#[derive(Clone, Copy, Eq, PartialEq)]
enum SlotState {
    Free,
    Opening(usize),
    Live(usize),
    Closing(usize),
    Quarantined(usize),
}

struct FileSlot {
    state: SlotState,
    lower_methods: usize,
    methods: MaybeUninit<libsqlite3_sys::sqlite3_io_methods>,
    tokens: [Option<FdToken>; MAX_TOKENS_PER_OPEN],
    flags: c_int,
    pending_shm: Option<FdToken>,
    shm_owner: Option<ShmOwner>,
    shm_key: Option<DbKey>,
    shm_shared_locks: u16,
    shm_exclusive_locks: u16,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct DbKey {
    root: NonZeroU64,
    identity: FileIdentity,
}

#[derive(Clone, Copy)]
enum ShmOwner {
    Initial(FdToken),
    Shared(SharedOwnerToken),
}

#[derive(Clone, Copy)]
struct ShmNode {
    key: DbKey,
    token: FdToken,
    owners: usize,
}

impl FileSlot {
    fn empty() -> Self {
        Self {
            state: SlotState::Free,
            lower_methods: 0,
            methods: MaybeUninit::uninit(),
            tokens: [None; MAX_TOKENS_PER_OPEN],
            flags: 0,
            pending_shm: None,
            shm_owner: None,
            shm_key: None,
            shm_shared_locks: 0,
            shm_exclusive_locks: 0,
        }
    }
}

struct OwnerTable {
    slots: Box<[FileSlot]>,
    shm_nodes: Box<[Option<ShmNode>]>,
    _grant: Arc<dyn RetainedMemoryGrant>,
}

impl OwnerTable {
    fn reserve(&mut self, file: usize) -> Option<usize> {
        if self
            .slots
            .iter()
            .any(|slot| slot.state != SlotState::Free && slot_file(slot.state) == file)
        {
            return None;
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.state == SlotState::Free)?;
        self.slots[index].state = SlotState::Opening(file);
        Some(index)
    }

    fn push(&mut self, index: usize, token: FdToken) -> bool {
        let slot = &mut self.slots[index];
        if !matches!(slot.state, SlotState::Opening(_)) {
            std::process::abort();
        }
        let Some(empty) = slot.tokens.iter_mut().find(|entry| entry.is_none()) else {
            return false;
        };
        *empty = Some(token);
        true
    }

    fn push_shm(&mut self, index: usize, token: FdToken) -> bool {
        let slot = &mut self.slots[index];
        if !matches!(slot.state, SlotState::Live(_))
            || slot.shm_owner.is_some()
            || slot.pending_shm.is_some()
        {
            return false;
        }
        slot.pending_shm = Some(token);
        true
    }

    fn free(&mut self, index: usize) {
        if self.slots[index].shm_owner.is_some() {
            std::process::abort();
        }
        self.slots[index] = FileSlot::empty();
    }
}

fn slot_file(state: SlotState) -> usize {
    match state {
        SlotState::Free => 0,
        SlotState::Opening(file)
        | SlotState::Live(file)
        | SlotState::Closing(file)
        | SlotState::Quarantined(file) => file,
    }
}

struct InstallAttempt<'a> {
    state: &'a AtomicU8,
    complete: bool,
}

impl<'a> InstallAttempt<'a> {
    fn begin(state: &'a AtomicU8) -> Result<Self, InstallError> {
        match state.compare_exchange(0, INSTALLING, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(Self {
                state,
                complete: false,
            }),
            Err(INSTALLING) => Err(InstallError::InstallInProgress),
            Err(INSTALLED) => Err(InstallError::AlreadyInstalled),
            Err(_) => Err(InstallError::PreviousFailure),
        }
    }

    fn succeed(&mut self) {
        self.state.store(INSTALLED, Ordering::Release);
        self.complete = true;
    }
}

impl Drop for InstallAttempt<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.state.store(FAILED, Ordering::Release);
        }
    }
}

impl InstalledHook {
    /// Number of SQLite files whose named-VFS xClose has not completed.
    pub fn active_owner_files(self) -> usize {
        files()
            .lock()
            .expect("owner VFS mutex")
            .slots
            .iter()
            .filter(|slot| slot.state != SlotState::Free)
            .count()
    }

    pub fn active_shm_owners(self) -> usize {
        files()
            .lock()
            .expect("owner VFS mutex")
            .slots
            .iter()
            .filter(|slot| slot.shm_owner.is_some())
            .count()
    }

    pub fn active_shm_nodes(self) -> usize {
        files()
            .lock()
            .expect("owner VFS mutex")
            .shm_nodes
            .iter()
            .flatten()
            .count()
    }

    pub fn active_shm_locks(self) -> usize {
        files()
            .lock()
            .expect("owner VFS mutex")
            .slots
            .iter()
            .map(|slot| (slot.shm_shared_locks | slot.shm_exclusive_locks).count_ones() as usize)
            .sum()
    }

    /// Exact requested backing bytes for the process-lifetime VFS and file
    /// slots. The caller must retain a hard memory grant for this amount.
    pub fn owner_vfs_backing_bytes(limits: OwnerLimits) -> Result<u64, InstallError> {
        let slots = Layout::array::<FileSlot>(limits.max_files.get())
            .map_err(|_error| InstallError::MemoryUnavailable)?;
        let shm_nodes = Layout::array::<Option<ShmNode>>(limits.max_files.get())
            .map_err(|_error| InstallError::MemoryUnavailable)?;
        let total = slots
            .size()
            .checked_add(shm_nodes.size())
            .ok_or(InstallError::MemoryUnavailable)?
            .checked_add(Layout::new::<libsqlite3_sys::sqlite3_vfs>().size())
            .ok_or(InstallError::MemoryUnavailable)?;
        u64::try_from(total).map_err(|_error| InstallError::MemoryUnavailable)
    }

    /// Register a named VFS that releases xOpen owners at xClose and shared
    /// SHM owners after xShmUnmap or successful xClose. Call only while the
    /// exclusive-process invariant used by install still holds.
    pub fn register_owner_vfs(
        self,
        limits: OwnerLimits,
        grant: Arc<dyn RetainedMemoryGrant>,
    ) -> Result<&'static CStr, InstallError> {
        let required = Self::owner_vfs_backing_bytes(limits)?;
        verify_grant(required, grant.as_ref())?;
        let lower = unsafe { libsqlite3_sys::sqlite3_vfs_find(c"unix".as_ptr()) };
        if lower.is_null() {
            return Err(InstallError::UnixVfsUnavailable);
        }
        // SAFETY: The registry Unix VFS remains registered for this process.
        let mut wrapper = unsafe { *lower };
        if wrapper.xOpen.is_none() || wrapper.xDelete.is_none() {
            return Err(InstallError::MissingOpenMethod);
        }
        wrapper.pNext = std::ptr::null_mut();
        wrapper.zName = c"quanta-unix-owner".as_ptr();
        wrapper.xOpen = Some(owner_x_open);
        wrapper.xDelete = Some(owner_x_delete);
        let slots = allocate_slots(limits.max_files.get())?;
        let shm_nodes = allocate_shm_nodes(limits.max_files.get())?;
        let layout = Layout::new::<libsqlite3_sys::sqlite3_vfs>();
        // SAFETY: An exact-layout allocation is made only after its grant.
        let wrapper_ptr = unsafe { alloc(layout) as *mut libsqlite3_sys::sqlite3_vfs };
        if wrapper_ptr.is_null() {
            return Err(InstallError::MemoryUnavailable);
        }
        let mut attempt = match InstallAttempt::begin(&OWNER_VFS_STATE) {
            Ok(attempt) => attempt,
            Err(error) => {
                // SAFETY: Registration has not observed this exact-layout allocation.
                unsafe { dealloc(wrapper_ptr.cast::<u8>(), layout) };
                return Err(error);
            }
        };
        let table = OwnerTable {
            slots,
            shm_nodes,
            _grant: grant,
        };
        if FILES.set(Mutex::new(table)).is_err() {
            // SAFETY: Registration has not observed this allocation.
            unsafe { dealloc(wrapper_ptr.cast::<u8>(), layout) };
            return Err(InstallError::PreviousFailure);
        }
        LOWER_VFS.store(lower as usize, Ordering::Release);
        // SAFETY: wrapper_ptr has the alignment and size of sqlite3_vfs.
        unsafe { wrapper_ptr.write(wrapper) };
        // SAFETY: The wrapper and its static name remain live for the process.
        let rc = unsafe { libsqlite3_sys::sqlite3_vfs_register(wrapper_ptr, 0) };
        if rc != libsqlite3_sys::SQLITE_OK {
            // SAFETY: Failed registration did not transfer this allocation.
            unsafe { dealloc(wrapper_ptr.cast::<u8>(), layout) };
            return Err(InstallError::RegisterRejected);
        }
        attempt.succeed();
        Ok(c"quanta-unix-owner")
    }

    /// Count successful physical closes of admitted descriptors.
    pub fn physical_closes(self) -> Result<u64, LedgerError> {
        let state = HOOK.get().ok_or(LedgerError::NotCurrent)?;
        Ok(state.physical_closes.load(Ordering::Relaxed))
    }

    /// A product VFS wrapper calls this once for the initial xOpen owner.
    /// Shared SHM owners must be released independently by the wrapper.
    pub fn release_owner(self, token: FdToken) -> Result<(), LedgerError> {
        let state = HOOK.get().ok_or(LedgerError::NotCurrent)?;
        if let Some(receipt) = state.ledger.release_owner(token)? {
            publish_retirement(state, receipt);
        }
        Ok(())
    }

    fn release_shared_owner(self, owner: SharedOwnerToken) -> Result<(), LedgerError> {
        let state = HOOK.get().ok_or(LedgerError::NotCurrent)?;
        if let Some(receipt) = state.ledger.release_shared_owner(owner)? {
            publish_retirement(state, receipt);
        }
        Ok(())
    }
}

fn allocate_slots(count: usize) -> Result<Box<[FileSlot]>, InstallError> {
    let layout =
        Layout::array::<FileSlot>(count).map_err(|_error| InstallError::MemoryUnavailable)?;
    // SAFETY: The exact requested layout is nonzero because count is nonzero.
    let ptr = unsafe { alloc(layout) as *mut FileSlot };
    if ptr.is_null() {
        return Err(InstallError::MemoryUnavailable);
    }
    for index in 0..count {
        // SAFETY: Every index is within the allocated layout and initialized once.
        unsafe { ptr.add(index).write(FileSlot::empty()) };
    }
    // SAFETY: Every element is initialized and Box owns the exact layout.
    Ok(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, count)) })
}

fn allocate_shm_nodes(count: usize) -> Result<Box<[Option<ShmNode>]>, InstallError> {
    let layout = Layout::array::<Option<ShmNode>>(count)
        .map_err(|_error| InstallError::MemoryUnavailable)?;
    // SAFETY: The exact requested layout is nonzero because count is nonzero.
    let ptr = unsafe { alloc(layout) as *mut Option<ShmNode> };
    if ptr.is_null() {
        return Err(InstallError::MemoryUnavailable);
    }
    for index in 0..count {
        // SAFETY: Every index is within the allocated layout and initialized once.
        unsafe { ptr.add(index).write(None) };
    }
    // SAFETY: Every element is initialized and Box owns the exact layout.
    Ok(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, count)) })
}

/// Install once, before any SQLite connection exists. The hook remains in
/// place for the process lifetime; an unsafe process permit is consumed.
pub fn install(
    _permit: ExclusiveProcessPermit,
    ledger: Arc<FdLedger>,
    authority: Arc<dyn OpenAuthority>,
) -> Result<InstalledHook, InstallError> {
    let mut attempt = InstallAttempt::begin(&INSTALL_STATE)?;
    // SAFETY: sqlite3_vfs_find returns the process-owned Unix VFS pointer.
    let vfs = unsafe { libsqlite3_sys::sqlite3_vfs_find(c"unix".as_ptr()) };
    if vfs.is_null() {
        return Err(InstallError::UnixVfsUnavailable);
    }
    // SAFETY: The returned VFS remains registered for the process lifetime.
    let (get, set) = unsafe {
        let vfs = &*vfs;
        if vfs.iVersion < 3 {
            return Err(InstallError::UnsupportedVersion);
        }
        (
            vfs.xGetSystemCall.ok_or(InstallError::MissingSyscall)?,
            vfs.xSetSystemCall.ok_or(InstallError::MissingSyscall)?,
        )
    };
    // SAFETY: The pinned Unix table documents these exact C signatures.
    let original_open =
        unsafe { get(vfs, c"open".as_ptr()) }.ok_or(InstallError::MissingSyscall)?;
    let original_close =
        unsafe { get(vfs, c"close".as_ptr()) }.ok_or(InstallError::MissingSyscall)?;
    let original_pwrite =
        unsafe { get(vfs, c"pwrite".as_ptr()) }.ok_or(InstallError::MissingSyscall)?;
    let original_ftruncate =
        unsafe { get(vfs, c"ftruncate".as_ptr()) }.ok_or(InstallError::MissingSyscall)?;
    // SAFETY: The table entries were checked above and have the Unix signatures.
    let state = HookState {
        original_open: unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), Open>(original_open)
        },
        original_close: unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), Close>(original_close)
        },
        original_pwrite: unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), Pwrite>(original_pwrite)
        },
        original_ftruncate: unsafe {
            std::mem::transmute::<unsafe extern "C" fn(), Ftruncate>(original_ftruncate)
        },
        ledger,
        authority,
        physical_closes: AtomicU64::new(0),
    };
    HOOK.set(state)
        .map_err(|_state| InstallError::AlreadyInstalled)?;
    // SAFETY: The callbacks and originals match the pinned Unix table signatures.
    let close_callback =
        unsafe { std::mem::transmute::<Close, unsafe extern "C" fn()>(hook_close) };
    // SAFETY: The callbacks and originals match the pinned Unix table signatures.
    let open_callback = unsafe { std::mem::transmute::<Open, unsafe extern "C" fn()>(hook_open) };
    install_syscalls(
        |name, callback| {
            // SAFETY: vfs remains registered and SQLite owns its syscall table.
            unsafe { set(vfs, name.as_ptr(), callback) }
        },
        [
            (c"close", Some(original_close), close_callback),
            (c"open", Some(original_open), open_callback),
            (c"pwrite", Some(original_pwrite), unsafe {
                std::mem::transmute::<Pwrite, unsafe extern "C" fn()>(hook_pwrite)
            }),
            (c"ftruncate", Some(original_ftruncate), unsafe {
                std::mem::transmute::<Ftruncate, unsafe extern "C" fn()>(hook_ftruncate)
            }),
        ],
    )?;
    attempt.succeed();
    Ok(InstalledHook(()))
}

type ErasedCall = libsqlite3_sys::sqlite3_syscall_ptr;

fn install_syscalls(
    mut set: impl FnMut(&CStr, ErasedCall) -> c_int,
    entries: [(&CStr, ErasedCall, unsafe extern "C" fn()); 4],
) -> Result<(), InstallError> {
    for (name, _original, replacement) in &entries {
        if set(name, Some(*replacement)) != libsqlite3_sys::SQLITE_OK {
            // A rejected setter may already have mutated the table.
            for (restore_name, original, _replacement) in &entries {
                if set(restore_name, *original) != libsqlite3_sys::SQLITE_OK {
                    std::process::abort();
                }
            }
            return Err(InstallError::OverrideRejected);
        }
    }
    Ok(())
}

/// # Safety
/// SQLite's pinned Unix table passes the libc pwrite ABI.
unsafe extern "C" fn hook_pwrite(
    fd: c_int,
    buffer: *const c_void,
    count: usize,
    offset: libc::off_t,
) -> isize {
    match catch_unwind(AssertUnwindSafe(|| {
        let state = HOOK.get().expect("installed hook");
        let Some(_index) = SHM_SLOT.with(Cell::get) else {
            // SAFETY: The original callback has the same Unix ABI.
            return unsafe { (state.original_pwrite)(fd, buffer, count, offset) };
        };
        let Some(token) = shm_token_for_fd(state, fd) else {
            set_errno(libc::EIO);
            return -1;
        };
        let Some(target) = checked_pwrite_end(offset, count) else {
            set_errno(libc::EFBIG);
            return -1;
        };
        match run_shm_growth(state.authority.as_ref(), token, fd, target, || {
            // SAFETY: The original callback has the same Unix ABI.
            unsafe { (state.original_pwrite)(fd, buffer, count, offset) }
        }) {
            Ok(result) => result,
            Err(errno) => {
                set_errno(errno);
                -1
            }
        }
    })) {
        Ok(result) => result,
        Err(_) => std::process::abort(),
    }
}

fn checked_pwrite_end(offset: libc::off_t, count: usize) -> Option<u64> {
    let offset = match u64::try_from(offset) {
        Ok(offset) => offset,
        Err(_) => return None,
    };
    let count = match u64::try_from(count) {
        Ok(count) => count,
        Err(_) => return None,
    };
    offset.checked_add(count)
}

/// # Safety
/// SQLite's pinned Unix table passes the libc ftruncate ABI.
unsafe extern "C" fn hook_ftruncate(fd: c_int, length: libc::off_t) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| {
        let state = HOOK.get().expect("installed hook");
        let Some(_index) = SHM_SLOT.with(Cell::get) else {
            // SAFETY: The original callback has the same Unix ABI.
            return unsafe { (state.original_ftruncate)(fd, length) };
        };
        let Some(token) = shm_token_for_fd(state, fd) else {
            set_errno(libc::EIO);
            return -1;
        };
        let Ok(target) = u64::try_from(length) else {
            set_errno(libc::EINVAL);
            return -1;
        };
        match run_shm_growth(state.authority.as_ref(), token, fd, target, || {
            // SAFETY: The original callback has the same Unix ABI.
            unsafe { (state.original_ftruncate)(fd, length) }
        }) {
            Ok(result) => result,
            Err(errno) => {
                set_errno(errno);
                -1
            }
        }
    })) {
        Ok(result) => result,
        Err(_) => std::process::abort(),
    }
}

fn run_shm_growth<R>(
    authority: &dyn OpenAuthority,
    token: FdToken,
    fd: RawFd,
    target: u64,
    syscall: impl FnOnce() -> R,
) -> Result<R, c_int> {
    let previous = fd_length(fd).ok_or(libc::EIO)?;
    let reservation = if target > previous {
        Some(
            authority
                .reserve_shm_growth(token, previous, target)
                .map_err(|errno| if errno > 0 { errno } else { libc::ENOSPC })?,
        )
    } else {
        None
    };
    let result = syscall();
    let errno = get_errno();
    if let Some(reservation) = reservation {
        authority.shm_growth_settled(reservation, fd_length(fd).unwrap_or(target));
    }
    set_errno(errno);
    Ok(result)
}

fn shm_token_for_fd(state: &HookState, fd: RawFd) -> Option<FdToken> {
    let index = SHM_SLOT.with(Cell::get)?;
    let token = {
        let table = files().lock().expect("owner VFS mutex");
        let slot = table.slots.get(index)?;
        slot.pending_shm.or_else(|| {
            let key = slot.shm_key?;
            table
                .shm_nodes
                .iter()
                .flatten()
                .find(|node| node.key == key)
                .map(|node| node.token)
        })?
    };
    if token.fd() != fd
        || !matches!(state.ledger.current(fd), Ok(Some(current)) if current == token)
    {
        return None;
    }
    Some(token)
}

fn fd_length(fd: RawFd) -> Option<u64> {
    // SAFETY: fstat writes the supplied stat structure for a live descriptor.
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: The descriptor is validated by the ledger before this call.
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return None;
    }
    if stat.st_size < 0 {
        None
    } else {
        Some(stat.st_size.unsigned_abs())
    }
}

/// # Safety
/// SQLite must pass a null-terminated Unix pathname and use the pinned open ABI.
unsafe extern "C" fn hook_open(path: *const c_char, flags: c_int, mode: c_int) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| open_inner(path, flags, mode))) {
        Ok(fd) => fd,
        Err(_) => std::process::abort(),
    }
}

fn open_inner(path: *const c_char, flags: c_int, mode: c_int) -> c_int {
    let state = HOOK.get().expect("installed hook state");
    if path.is_null() {
        set_errno(libc::EACCES);
        return -1;
    }
    // SAFETY: SQLite passes a null-terminated pathname to its Unix open syscall.
    let path = unsafe { CStr::from_ptr(path) };
    match state.authority.decide(path) {
        OpenDecision::Deny(errno) => {
            set_errno(if errno > 0 { errno } else { libc::EACCES });
            -1
        }
        OpenDecision::Auxiliary => {
            // SAFETY: The original Unix open callback was captured at install.
            unsafe { (state.original_open)(path.as_ptr(), flags, mode) }
        }
        OpenDecision::Admit { root, charge } => {
            admitted_open(state, path, flags, mode, root, charge, true)
        }
        OpenDecision::AdmitShared { root, charge } => {
            admitted_open(state, path, flags, mode, root, charge, false)
        }
    }
}

fn admitted_open(
    state: &HookState,
    path: &CStr,
    flags: c_int,
    mode: c_int,
    root: NonZeroU64,
    charge: NonZeroU64,
    capture_owner: bool,
) -> c_int {
    let slot = OPEN_SLOT.with(Cell::get);
    if capture_owner && slot.is_none() {
        state.authority.open_refused(charge);
        set_errno(libc::EACCES);
        return -1;
    }
    let shm_slot = if capture_owner {
        None
    } else {
        SHM_SLOT.with(Cell::get)
    };
    if !capture_owner && shm_slot.is_none() {
        state.authority.open_refused(charge);
        set_errno(libc::EACCES);
        return -1;
    }
    let result = state.ledger.open_with(
        root,
        charge,
        || state.authority.open_admitted(path, flags, mode),
        witness,
    );
    match result {
        Ok(OpenDisposition::Ready(token)) => {
            let recorded = if let Some(slot) = slot.filter(|_| capture_owner) {
                files().lock().expect("owner VFS mutex").push(slot, token)
            } else if let Some(slot) = shm_slot {
                files()
                    .lock()
                    .expect("owner VFS mutex")
                    .push_shm(slot, token)
            } else {
                false
            };
            if !recorded {
                // The FD was already opened. Close it under custody and
                // settle its initial owner before refusing this open.
                let close_rc = close_inner(token.fd());
                if (InstalledHook(())).release_owner(token).is_err() {
                    std::process::abort();
                }
                // An uncertain close keeps the charge quarantined. It must
                // never be retried by its numeric FD.
                set_errno(if close_rc == 0 {
                    libc::EMFILE
                } else {
                    libc::EIO
                });
                return -1;
            }
            state.authority.opened(path, token);
            token.fd()
        }
        Ok(OpenDisposition::Quarantined(token)) => {
            state.authority.quarantined(charge);
            // Even if close succeeds, the missing witness forbids a refund.
            let _closed = close_inner(token.fd());
            set_errno(libc::EIO);
            -1
        }
        Err(OpenError::Syscall(errno)) => {
            state.authority.open_refused(charge);
            set_errno(errno);
            -1
        }
        Err(OpenError::Ledger(error)) => {
            state.authority.open_refused(charge);
            set_errno(if error == LedgerError::Full {
                libc::EMFILE
            } else {
                libc::EIO
            });
            -1
        }
    }
}

/// # Safety
/// SQLite passes a valid wrapper VFS and file storage of szOsFile bytes.
unsafe extern "C" fn owner_x_open(
    _vfs: *mut libsqlite3_sys::sqlite3_vfs,
    name: *const c_char,
    file: *mut libsqlite3_sys::sqlite3_file,
    flags: c_int,
    out_flags: *mut c_int,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| {
        owner_open_inner(name, file, flags, out_flags)
    })) {
        Ok(rc) => rc,
        Err(_) => std::process::abort(),
    }
}

fn owner_open_inner(
    name: *const c_char,
    file: *mut libsqlite3_sys::sqlite3_file,
    flags: c_int,
    out_flags: *mut c_int,
) -> c_int {
    if OPEN_SLOT.with(Cell::get).is_some() {
        return refuse_before_lower(file);
    }
    let lower = LOWER_VFS.load(Ordering::Acquire) as *mut libsqlite3_sys::sqlite3_vfs;
    if lower.is_null() || file.is_null() {
        std::process::abort();
    }
    let Some(index) = files()
        .lock()
        .expect("owner VFS mutex")
        .reserve(file as usize)
    else {
        return refuse_before_lower(file);
    };
    OPEN_SLOT.with(|slot| slot.set(Some(index)));
    // SAFETY: The lower VFS and its xOpen were checked at registration.
    let rc = unsafe { ((*lower).xOpen.expect("lower xOpen"))(lower, name, file, flags, out_flags) };
    OPEN_SLOT.with(|slot| slot.set(None));
    // SQLite may call xClose after failed xOpen when pMethods is non-null.
    // SAFETY: SQLite supplied the file buffer and lower xOpen initialized it.
    let lower_methods = unsafe { (*file).pMethods };
    if lower_methods.is_null() {
        if rc == libsqlite3_sys::SQLITE_OK {
            std::process::abort();
        }
        let mut table = files().lock().expect("owner VFS mutex");
        let tokens = table.slots[index].tokens;
        table.free(index);
        drop(table);
        release_tokens(tokens);
        return rc;
    }
    // SAFETY: pMethods is valid while the lower file is open.
    let mut methods = unsafe { *lower_methods };
    if methods.xClose.is_none() {
        std::process::abort();
    }
    methods.xClose = Some(owner_x_close);
    if methods.xWrite.is_none() || methods.xTruncate.is_none() || methods.xSync.is_none() {
        std::process::abort();
    }
    methods.xWrite = Some(owner_x_write);
    methods.xTruncate = Some(owner_x_truncate);
    methods.xSync = Some(owner_x_sync);
    if methods.xShmMap.is_some() {
        methods.xShmMap = Some(owner_x_shm_map);
    }
    if methods.xShmUnmap.is_some() {
        methods.xShmUnmap = Some(owner_x_shm_unmap);
    }
    if methods.xShmLock.is_some() {
        methods.xShmLock = Some(owner_x_shm_lock);
    }
    let mut table = files().lock().expect("owner VFS mutex");
    let slot = &mut table.slots[index];
    if slot.state != SlotState::Opening(file as usize) {
        std::process::abort();
    }
    slot.lower_methods = lower_methods as usize;
    slot.methods.write(methods);
    slot.flags = flags;
    slot.state = SlotState::Live(file as usize);
    let methods_ptr = slot.methods.as_ptr();
    drop(table);
    // SAFETY: The cloned method table stays owned by FILES until xClose.
    unsafe { (*file).pMethods = methods_ptr };
    rc
}

fn refuse_before_lower(file: *mut libsqlite3_sys::sqlite3_file) -> c_int {
    if !file.is_null() {
        // SAFETY: SQLite supplied writable sqlite3_file storage. A failed
        // xOpen with pMethods non-null may be followed by xClose.
        unsafe { (*file).pMethods = std::ptr::null() };
    }
    libsqlite3_sys::SQLITE_CANTOPEN
}

fn ordinary_file_context(file: *mut libsqlite3_sys::sqlite3_file) -> Option<(usize, FdToken)> {
    let table = files().lock().expect("owner VFS mutex");
    let index = table
        .slots
        .iter()
        .position(|slot| slot.state == SlotState::Live(file as usize))?;
    let token = table.slots[index].tokens[0]?;
    let state = HOOK.get().expect("installed hook");
    if !matches!(state.ledger.current(token.fd()), Ok(Some(current)) if current == token) {
        return None;
    }
    Some((table.slots[index].lower_methods, token))
}

fn admission_sqlite_error(errno: c_int, io_error: c_int) -> c_int {
    if errno == libc::ENOSPC || errno == libc::EDQUOT {
        libsqlite3_sys::SQLITE_FULL
    } else {
        io_error
    }
}

fn mutate_ordinary_file(
    file: *mut libsqlite3_sys::sqlite3_file,
    kind: FileMutationKind,
    requested_len: Option<u64>,
    io_error: c_int,
    lower: impl FnOnce(*const libsqlite3_sys::sqlite3_io_methods) -> c_int,
) -> c_int {
    let _serial = FILE_MUTATION_SERIAL.lock().expect("file mutation mutex");
    let Some((methods, token)) = ordinary_file_context(file) else {
        return io_error;
    };
    let Some(previous_len) = fd_length(token.fd()) else {
        return io_error;
    };
    let Some(requested_len) = requested_len else {
        return io_error;
    };
    let state = HOOK.get().expect("installed hook");
    let reservation =
        match state
            .authority
            .reserve_file_mutation(token, kind, previous_len, requested_len)
        {
            Ok(reservation) => reservation,
            Err(errno) => return admission_sqlite_error(errno, io_error),
        };
    let rc = lower(methods as *const libsqlite3_sys::sqlite3_io_methods);
    let observed_len = fd_length(token.fd()).unwrap_or(previous_len.max(requested_len));
    let settled = state
        .authority
        .file_mutation_settled(reservation, observed_len, rc);
    if rc != libsqlite3_sys::SQLITE_OK {
        rc
    } else {
        settled.map_or_else(|errno| admission_sqlite_error(errno, io_error), |_| rc)
    }
}

/// # Safety
/// SQLite supplies the live file and buffer under the pinned xWrite ABI.
unsafe extern "C" fn owner_x_write(
    file: *mut libsqlite3_sys::sqlite3_file,
    buffer: *const c_void,
    amount: c_int,
    offset: libsqlite3_sys::sqlite3_int64,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| {
        let end = match (u64::try_from(offset), u64::try_from(amount)) {
            (Ok(start), Ok(amount)) => start.checked_add(amount),
            _ => None,
        };
        mutate_ordinary_file(
            file,
            FileMutationKind::Write,
            end,
            libsqlite3_sys::SQLITE_IOERR_WRITE,
            |methods| {
                // SAFETY: The lower table belongs to this live SQLite file.
                unsafe { ((*methods).xWrite.expect("lower xWrite"))(file, buffer, amount, offset) }
            },
        )
    })) {
        Ok(rc) => rc,
        Err(_) => std::process::abort(),
    }
}

/// # Safety
/// SQLite supplies the live file under the pinned xTruncate ABI.
unsafe extern "C" fn owner_x_truncate(
    file: *mut libsqlite3_sys::sqlite3_file,
    size: libsqlite3_sys::sqlite3_int64,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| {
        mutate_ordinary_file(
            file,
            FileMutationKind::Truncate,
            (size >= 0).then(|| size.unsigned_abs()),
            libsqlite3_sys::SQLITE_IOERR_TRUNCATE,
            |methods| {
                // SAFETY: The lower table belongs to this live SQLite file.
                unsafe { ((*methods).xTruncate.expect("lower xTruncate"))(file, size) }
            },
        )
    })) {
        Ok(rc) => rc,
        Err(_) => std::process::abort(),
    }
}

/// # Safety
/// SQLite supplies the live file under the pinned xSync ABI.
unsafe extern "C" fn owner_x_sync(file: *mut libsqlite3_sys::sqlite3_file, flags: c_int) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| {
        let _serial = FILE_MUTATION_SERIAL.lock().expect("file mutation mutex");
        let Some((methods, token)) = ordinary_file_context(file) else {
            return libsqlite3_sys::SQLITE_IOERR_FSYNC;
        };
        // SAFETY: The lower table belongs to this live SQLite file.
        let rc = unsafe {
            ((*((methods) as *const libsqlite3_sys::sqlite3_io_methods))
                .xSync
                .expect("lower xSync"))(file, flags)
        };
        let observed_len = fd_length(token.fd());
        let settled =
            HOOK.get()
                .expect("installed hook")
                .authority
                .file_synced(token, observed_len, rc);
        if rc != libsqlite3_sys::SQLITE_OK {
            rc
        } else if observed_len.is_none() {
            libsqlite3_sys::SQLITE_IOERR_FSYNC
        } else {
            settled.map_or_else(
                |errno| admission_sqlite_error(errno, libsqlite3_sys::SQLITE_IOERR_FSYNC),
                |_| rc,
            )
        }
    })) {
        Ok(rc) => rc,
        Err(_) => std::process::abort(),
    }
}

/// # Safety
/// SQLite supplies the registered VFS and a valid pathname under xDelete ABI.
unsafe extern "C" fn owner_x_delete(
    _vfs: *mut libsqlite3_sys::sqlite3_vfs,
    name: *const c_char,
    sync_dir: c_int,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| {
        let _serial = FILE_MUTATION_SERIAL.lock().expect("file mutation mutex");
        if name.is_null() {
            return libsqlite3_sys::SQLITE_IOERR_DELETE;
        }
        // SAFETY: SQLite passes a null-terminated pathname to xDelete.
        let path = unsafe { CStr::from_ptr(name) };
        let state = HOOK.get().expect("installed hook");
        let reservation = match state.authority.reserve_delete(path) {
            Ok(reservation) => reservation,
            Err(errno) => {
                return admission_sqlite_error(errno, libsqlite3_sys::SQLITE_IOERR_DELETE);
            }
        };
        let lower = LOWER_VFS.load(Ordering::Acquire) as *mut libsqlite3_sys::sqlite3_vfs;
        // SAFETY: The registered lower Unix VFS remains live for this process.
        let rc = unsafe { ((*lower).xDelete.expect("lower xDelete"))(lower, name, sync_dir) };
        let settled = state.authority.delete_settled(reservation, rc);
        if rc != libsqlite3_sys::SQLITE_OK {
            rc
        } else {
            settled.map_or_else(
                |errno| admission_sqlite_error(errno, libsqlite3_sys::SQLITE_IOERR_DELETE),
                |_| rc,
            )
        }
    })) {
        Ok(rc) => rc,
        Err(_) => std::process::abort(),
    }
}

/// # Safety
/// SQLite calls this only for a live named-VFS database file.
unsafe extern "C" fn owner_x_shm_map(
    file: *mut libsqlite3_sys::sqlite3_file,
    page: c_int,
    page_size: c_int,
    extend: c_int,
    out: *mut *mut c_void,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| {
        owner_shm_map_inner(file, page, page_size, extend, out)
    })) {
        Ok(rc) => rc,
        Err(_) => std::process::abort(),
    }
}

fn owner_shm_map_inner(
    file: *mut libsqlite3_sys::sqlite3_file,
    page: c_int,
    page_size: c_int,
    extend: c_int,
    out: *mut *mut c_void,
) -> c_int {
    let _serial = SHM_SERIAL.lock().expect("SHM serial mutex");
    let state = HOOK.get().expect("installed hook");
    let (index, lower_methods, db_token, already_owned) = {
        let table = files().lock().expect("owner VFS mutex");
        let Some(index) = table
            .slots
            .iter()
            .position(|slot| slot.state == SlotState::Live(file as usize))
        else {
            return libsqlite3_sys::SQLITE_IOERR_SHMMAP;
        };
        let slot = &table.slots[index];
        if slot.flags & libsqlite3_sys::SQLITE_OPEN_MAIN_DB == 0 {
            return libsqlite3_sys::SQLITE_IOERR_SHMMAP;
        }
        (
            index,
            slot.lower_methods,
            slot.tokens[0],
            slot.shm_owner.is_some(),
        )
    };
    let Some(db_token) = db_token else {
        return libsqlite3_sys::SQLITE_IOERR_SHMMAP;
    };
    let db_witness = match state.ledger.live_witness(db_token) {
        Ok(witness) => witness,
        Err(_) => return libsqlite3_sys::SQLITE_IOERR_SHMMAP,
    };
    let Some((root, identity)) = db_witness else {
        return libsqlite3_sys::SQLITE_IOERR_SHMMAP;
    };
    let key = DbKey { root, identity };
    if !already_owned {
        let node = {
            let table = files().lock().expect("owner VFS mutex");
            table
                .shm_nodes
                .iter()
                .flatten()
                .find(|node| node.key == key)
                .copied()
        };
        if let Some(node) = node {
            if node.owners == 0 {
                return libsqlite3_sys::SQLITE_IOERR_SHMMAP;
            }
            let owner = match state.ledger.retain_owner(node.token) {
                Ok(owner) => owner,
                Err(_) => return libsqlite3_sys::SQLITE_NOMEM,
            };
            let mut table = files().lock().expect("owner VFS mutex");
            let slot = &mut table.slots[index];
            slot.shm_owner = Some(ShmOwner::Shared(owner));
            slot.shm_key = Some(key);
            let Some(node) = table
                .shm_nodes
                .iter_mut()
                .flatten()
                .find(|node| node.key == key)
            else {
                std::process::abort();
            };
            node.owners = node
                .owners
                .checked_add(1)
                .unwrap_or_else(|| std::process::abort());
        } else if !files()
            .lock()
            .expect("owner VFS mutex")
            .shm_nodes
            .iter()
            .any(Option::is_none)
        {
            return libsqlite3_sys::SQLITE_NOMEM;
        }
    }
    SHM_SLOT.with(|slot| slot.set(Some(index)));
    // SAFETY: The lower method table belongs to this live SQLite file.
    let rc = unsafe {
        ((*((lower_methods) as *const libsqlite3_sys::sqlite3_io_methods))
            .xShmMap
            .expect("lower xShmMap"))(file, page, page_size, extend, out)
    };
    SHM_SLOT.with(|slot| slot.set(None));
    let pending = files().lock().expect("owner VFS mutex").slots[index]
        .pending_shm
        .take();
    if let Some(token) = pending {
        if already_owned
            || files().lock().expect("owner VFS mutex").slots[index]
                .shm_owner
                .is_some()
        {
            std::process::abort();
        }
        let live = match state.ledger.live_witness(token) {
            Ok(witness) => witness,
            Err(_) => std::process::abort(),
        };
        if live.is_none() {
            if (InstalledHook(())).release_owner(token).is_err() {
                std::process::abort();
            }
            return if rc == libsqlite3_sys::SQLITE_OK {
                libsqlite3_sys::SQLITE_IOERR_SHMMAP
            } else {
                rc
            };
        }
        if live.is_some_and(|(shm_root, _)| shm_root != root) {
            std::process::abort();
        }
        let mut table = files().lock().expect("owner VFS mutex");
        let Some(empty) = table.shm_nodes.iter_mut().find(|node| node.is_none()) else {
            std::process::abort();
        };
        *empty = Some(ShmNode {
            key,
            token,
            owners: 1,
        });
        let slot = &mut table.slots[index];
        slot.shm_owner = Some(ShmOwner::Initial(token));
        slot.shm_key = Some(key);
    } else if !already_owned
        && files().lock().expect("owner VFS mutex").slots[index]
            .shm_owner
            .is_none()
        && rc == libsqlite3_sys::SQLITE_OK
    {
        // The Unix VFS used SHM without an observable physical descriptor.
        return libsqlite3_sys::SQLITE_IOERR_SHMMAP;
    }
    rc
}

/// # Safety
/// SQLite calls this only for a live named-VFS database file.
unsafe extern "C" fn owner_x_shm_unmap(
    file: *mut libsqlite3_sys::sqlite3_file,
    delete: c_int,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| owner_shm_unmap_inner(file, delete))) {
        Ok(rc) => rc,
        Err(_) => std::process::abort(),
    }
}

fn owner_shm_unmap_inner(file: *mut libsqlite3_sys::sqlite3_file, delete: c_int) -> c_int {
    let _serial = SHM_SERIAL.lock().expect("SHM serial mutex");
    let (index, lower_methods) = {
        let table = files().lock().expect("owner VFS mutex");
        let Some(index) = table
            .slots
            .iter()
            .position(|slot| slot.state == SlotState::Live(file as usize))
        else {
            return libsqlite3_sys::SQLITE_IOERR_SHMMAP;
        };
        (index, table.slots[index].lower_methods)
    };
    // SAFETY: The lower method table belongs to this live SQLite file.
    let rc = unsafe {
        ((*((lower_methods) as *const libsqlite3_sys::sqlite3_io_methods))
            .xShmUnmap
            .expect("lower xShmUnmap"))(file, delete)
    };
    if rc == libsqlite3_sys::SQLITE_OK && !release_shm_owner(index) {
        return libsqlite3_sys::SQLITE_IOERR_SHMMAP;
    }
    rc
}

/// # Safety
/// SQLite calls this only for a live named-VFS database file.
unsafe extern "C" fn owner_x_shm_lock(
    file: *mut libsqlite3_sys::sqlite3_file,
    offset: c_int,
    count: c_int,
    flags: c_int,
) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| {
        owner_shm_lock_inner(file, offset, count, flags)
    })) {
        Ok(rc) => rc,
        Err(_) => std::process::abort(),
    }
}

fn owner_shm_lock_inner(
    file: *mut libsqlite3_sys::sqlite3_file,
    offset: c_int,
    count: c_int,
    flags: c_int,
) -> c_int {
    let _serial = SHM_SERIAL.lock().expect("SHM serial mutex");
    if offset < 0 || count <= 0 || offset.checked_add(count).is_none_or(|end| end > 16) {
        return libsqlite3_sys::SQLITE_IOERR_SHMLOCK;
    }
    let (index, lower_methods) = {
        let table = files().lock().expect("owner VFS mutex");
        let Some(index) = table
            .slots
            .iter()
            .position(|slot| slot.state == SlotState::Live(file as usize))
        else {
            return libsqlite3_sys::SQLITE_IOERR_SHMLOCK;
        };
        (index, table.slots[index].lower_methods)
    };
    // SAFETY: The lower method table belongs to this live SQLite file.
    let rc = unsafe {
        ((*((lower_methods) as *const libsqlite3_sys::sqlite3_io_methods))
            .xShmLock
            .expect("lower xShmLock"))(file, offset, count, flags)
    };
    if rc == libsqlite3_sys::SQLITE_OK {
        let mask = ((1_u32 << count) - 1) << offset;
        let mut table = files().lock().expect("owner VFS mutex");
        let lock_bits = if flags & libsqlite3_sys::SQLITE_SHM_SHARED != 0 {
            &mut table.slots[index].shm_shared_locks
        } else if flags & libsqlite3_sys::SQLITE_SHM_EXCLUSIVE != 0 {
            &mut table.slots[index].shm_exclusive_locks
        } else {
            std::process::abort();
        };
        if flags & libsqlite3_sys::SQLITE_SHM_LOCK != 0 {
            *lock_bits |= mask as u16;
        } else if flags & libsqlite3_sys::SQLITE_SHM_UNLOCK != 0 {
            *lock_bits &= !(mask as u16);
        } else {
            std::process::abort();
        }
    }
    rc
}

/// Called only while SHM_SERIAL is held. A failed lower operation or active
/// lock keeps the shared charge until process-level reconciliation.
fn release_shm_owner(index: usize) -> bool {
    let (owner, key) = {
        let mut table = files().lock().expect("owner VFS mutex");
        let slot = &mut table.slots[index];
        if slot.shm_shared_locks != 0 || slot.shm_exclusive_locks != 0 {
            return false;
        }
        (slot.shm_owner.take(), slot.shm_key.take())
    };
    let Some(owner) = owner else {
        return true;
    };
    let Some(key) = key else {
        std::process::abort()
    };
    let token = {
        let table = files().lock().expect("owner VFS mutex");
        table
            .shm_nodes
            .iter()
            .flatten()
            .find(|node| node.key == key)
            .map(|node| node.token)
    };
    let Some(token) = token else {
        std::process::abort()
    };
    let result = match owner {
        ShmOwner::Initial(token) => (InstalledHook(())).release_owner(token),
        ShmOwner::Shared(owner) => (InstalledHook(())).release_shared_owner(owner),
    };
    if result.is_err() {
        std::process::abort();
    }
    let physical_live = match HOOK
        .get()
        .expect("installed hook")
        .ledger
        .live_witness(token)
    {
        Ok(witness) => witness.is_some(),
        Err(_) => std::process::abort(),
    };
    let mut table = files().lock().expect("owner VFS mutex");
    let Some(node) = table
        .shm_nodes
        .iter_mut()
        .find(|node| node.is_some_and(|node| node.key == key))
    else {
        std::process::abort()
    };
    let Some(current) = node.as_mut() else {
        std::process::abort()
    };
    current.owners -= 1;
    if current.owners == 0 && !physical_live {
        *node = None;
    }
    true
}

/// # Safety
/// SQLite calls this only for a file registered by owner_x_open.
unsafe extern "C" fn owner_x_close(file: *mut libsqlite3_sys::sqlite3_file) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| owner_close_inner(file))) {
        Ok(rc) => rc,
        Err(_) => std::process::abort(),
    }
}

fn owner_close_inner(file: *mut libsqlite3_sys::sqlite3_file) -> c_int {
    let _mutation_serial = FILE_MUTATION_SERIAL.lock().expect("file mutation mutex");
    let _serial = SHM_SERIAL.lock().expect("SHM serial mutex");
    let mut table = files().lock().expect("owner VFS mutex");
    let Some(index) = table
        .slots
        .iter()
        .position(|slot| slot.state == SlotState::Live(file as usize))
    else {
        return libsqlite3_sys::SQLITE_IOERR_CLOSE;
    };
    let slot = &mut table.slots[index];
    slot.state = SlotState::Closing(file as usize);
    let lower_methods = slot.lower_methods as *const libsqlite3_sys::sqlite3_io_methods;
    let tokens = slot.tokens;
    drop(table);
    // SAFETY: The lower method table was captured from this file at xOpen.
    let close = unsafe { (*lower_methods).xClose.expect("lower xClose") };
    // SAFETY: The lower xClose must see the method table it installed.
    unsafe { (*file).pMethods = lower_methods };
    // SAFETY: The file is SQLite-owned and its lower xClose is called once.
    let rc = unsafe { close(file) };
    if rc == libsqlite3_sys::SQLITE_OK {
        let shm_released = release_shm_owner(index);
        release_tokens(tokens);
        if shm_released {
            files().lock().expect("owner VFS mutex").free(index);
        } else {
            files().lock().expect("owner VFS mutex").slots[index].state =
                SlotState::Quarantined(file as usize);
        }
    } else {
        // Lower xClose did not prove retirement. Keep the preallocated slot
        // and every charge quarantined for the remainder of this process.
        files().lock().expect("owner VFS mutex").slots[index].state =
            SlotState::Quarantined(file as usize);
    }
    rc
}

fn files() -> &'static Mutex<OwnerTable> {
    FILES.get().expect("owner VFS table installed")
}

fn release_tokens(tokens: [Option<FdToken>; MAX_TOKENS_PER_OPEN]) {
    for token in tokens.into_iter().flatten() {
        if (InstalledHook(())).release_owner(token).is_err() {
            std::process::abort();
        }
    }
}

/// # Safety
/// SQLite must pass a descriptor it owns under the pinned Unix close ABI.
unsafe extern "C" fn hook_close(fd: c_int) -> c_int {
    match catch_unwind(AssertUnwindSafe(|| close_inner(fd))) {
        Ok(result) => result,
        Err(_) => std::process::abort(),
    }
}

fn close_inner(fd: RawFd) -> c_int {
    let state = HOOK.get().expect("installed hook state");
    match state.ledger.current(fd) {
        Ok(Some(token)) => match state
            .ledger
            .close_with(token, |fd| close_original(state, fd))
        {
            Ok(CloseReport::Closed(receipt)) => {
                state.physical_closes.fetch_add(1, Ordering::Relaxed);
                if let Some(receipt) = receipt {
                    publish_retirement(state, receipt);
                }
                0
            }
            Ok(CloseReport::Uncertain(errno)) => {
                set_errno(errno);
                -1
            }
            Err(_error) => {
                // A tracked uncertain generation must never be closed again.
                set_errno(libc::EIO);
                -1
            }
        },
        Ok(None) => {
            // SAFETY: This descriptor was never admitted by this authority.
            unsafe { (state.original_close)(fd) }
        }
        Err(_error) => {
            set_errno(libc::EIO);
            -1
        }
    }
}

fn close_original(state: &HookState, fd: RawFd) -> Result<(), c_int> {
    // SAFETY: The original Unix close callback was captured at install.
    if unsafe { (state.original_close)(fd) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO))
    }
}

fn publish_retirement(state: &HookState, receipt: RetirementReceipt) {
    if catch_unwind(AssertUnwindSafe(|| state.authority.retired(receipt))).is_err() {
        // The ledger has already consumed this one-shot receipt. Returning to
        // SQLite or the caller after a failed settlement would lose its charge.
        std::process::abort();
    }
}

fn witness(fd: RawFd) -> Option<FileIdentity> {
    // SAFETY: fstat fills the initialized struct for an open descriptor.
    let mut info = unsafe { std::mem::zeroed::<libc::stat>() };
    // SAFETY: info is writable and fd was just returned by the Unix open.
    if unsafe { libc::fstat(fd, &mut info) } != 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    let device = match u64::try_from(info.st_dev) {
        Ok(device) => device,
        Err(_error) => return None,
    };
    #[cfg(target_os = "linux")]
    let device = info.st_dev;
    Some(FileIdentity {
        device,
        inode: info.st_ino,
    })
}

#[cfg(target_os = "macos")]
fn set_errno(value: c_int) {
    // SAFETY: libc exposes the calling thread's errno address.
    unsafe { *libc::__error() = value };
}

#[cfg(target_os = "macos")]
fn get_errno() -> c_int {
    // SAFETY: libc exposes the calling thread's errno address.
    unsafe { *libc::__error() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::OpenOptions,
        os::fd::IntoRawFd,
        sync::{Barrier, atomic::AtomicUsize},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct GrowthAuthority {
        limit: u64,
        attempts: Mutex<Vec<(u64, u64)>>,
        settled: Mutex<Vec<u64>>,
    }

    impl OpenAuthority for GrowthAuthority {
        fn decide(&self, _path: &CStr) -> OpenDecision {
            unreachable!()
        }
        fn open_admitted(&self, _path: &CStr, _flags: c_int, _mode: c_int) -> Result<RawFd, c_int> {
            unreachable!()
        }
        fn opened(&self, _path: &CStr, _token: FdToken) {
            unreachable!()
        }
        fn open_refused(&self, _charge: NonZeroU64) {
            unreachable!()
        }
        fn quarantined(&self, _charge: NonZeroU64) {
            unreachable!()
        }
        fn retired(&self, _receipt: RetirementReceipt) {
            unreachable!()
        }
        fn reserve_shm_growth(
            &self,
            _token: FdToken,
            previous_len: u64,
            requested_len: u64,
        ) -> Result<NonZeroU64, c_int> {
            self.attempts
                .lock()
                .expect("attempts")
                .push((previous_len, requested_len));
            if requested_len > self.limit {
                Err(libc::ENOSPC)
            } else {
                Ok(NonZeroU64::new(1).expect("reservation"))
            }
        }
        fn shm_growth_settled(&self, _reservation: NonZeroU64, observed_len: u64) {
            self.settled.lock().expect("settled").push(observed_len);
        }
        fn reserve_file_mutation(
            &self,
            _token: FdToken,
            _kind: FileMutationKind,
            _previous_len: u64,
            _requested_len: u64,
        ) -> Result<NonZeroU64, c_int> {
            unreachable!()
        }
        fn file_mutation_settled(
            &self,
            _reservation: NonZeroU64,
            _observed_len: u64,
            _sqlite_rc: c_int,
        ) -> Result<(), c_int> {
            unreachable!()
        }
        fn file_synced(
            &self,
            _token: FdToken,
            _observed_len: Option<u64>,
            _sqlite_rc: c_int,
        ) -> Result<(), c_int> {
            unreachable!()
        }
        fn reserve_delete(&self, _path: &CStr) -> Result<NonZeroU64, c_int> {
            unreachable!()
        }
        fn delete_settled(&self, _reservation: NonZeroU64, _sqlite_rc: c_int) -> Result<(), c_int> {
            unreachable!()
        }
    }

    #[test]
    fn shm_growth_uses_existing_length_and_settles_partial_write() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("qiv-shm-growth-{}-{nonce}", std::process::id()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("scratch SHM file");
        std::fs::remove_file(path).expect("unlink scratch SHM file");
        file.set_len(1024).expect("pre-existing SHM length");
        let fd = file.into_raw_fd();
        let ledger =
            FdLedger::with_capacity(NonZeroUsize::new(1).expect("slot")).expect("FD ledger");
        let OpenDisposition::Ready(token) = ledger
            .open_with(
                NonZeroU64::new(1).expect("root"),
                NonZeroU64::new(1).expect("charge"),
                || Ok::<RawFd, c_int>(fd),
                witness,
            )
            .expect("admit scratch FD")
        else {
            panic!("scratch FD needs an identity");
        };
        let authority = GrowthAuthority {
            limit: 8192,
            attempts: Mutex::new(Vec::new()),
            settled: Mutex::new(Vec::new()),
        };
        let result = run_shm_growth(&authority, token, fd, 8192, || {
            // SAFETY: The ledger owns this live scratch descriptor.
            assert_eq!(
                unsafe { libc::pwrite(fd, b"x".as_ptr().cast(), 1, 4095) },
                1
            );
            set_errno(libc::EIO);
            -1
        });
        assert_eq!(result, Ok(-1));
        assert_eq!(get_errno(), libc::EIO);
        assert_eq!(fd_length(fd), Some(4096));
        assert_eq!(
            *authority.attempts.lock().expect("attempts"),
            vec![(1024, 8192)]
        );
        assert_eq!(*authority.settled.lock().expect("settled"), vec![4096]);
        assert_eq!(
            run_shm_growth(&authority, token, fd, 8193, || panic!("unfunded write ran")).err(),
            Some(libc::ENOSPC)
        );
        assert_eq!(fd_length(fd), Some(4096));
        assert_eq!(run_shm_growth(&authority, token, fd, 2048, || 1), Ok(1));
        assert_eq!(authority.settled.lock().expect("settled").len(), 1);
        assert_eq!(checked_pwrite_end(-1, 1), None);
        assert_eq!(checked_pwrite_end(libc::off_t::MAX, usize::MAX), None);
        assert!(matches!(
            ledger.close_with(token, |fd| {
                // SAFETY: This is the sole physical close of the ledger's FD.
                if unsafe { libc::close(fd) } == 0 {
                    Ok(())
                } else {
                    Err(())
                }
            }),
            Ok(CloseReport::Closed(None))
        ));
        assert!(
            ledger
                .release_owner(token)
                .expect("release owner")
                .is_some()
        );
    }

    /// # Safety
    /// This fake callback is never invoked; only its address enters the test table.
    unsafe extern "C" fn original_open() {}
    /// # Safety
    /// This fake callback is never invoked; only its address enters the test table.
    unsafe extern "C" fn original_close() {}
    /// # Safety
    /// This fake callback is never invoked; only its address enters the test table.
    unsafe extern "C" fn original_pwrite() {}
    /// # Safety
    /// This fake callback is never invoked; only its address enters the test table.
    unsafe extern "C" fn original_ftruncate() {}
    /// # Safety
    /// This fake callback is never invoked; only its address enters the test table.
    unsafe extern "C" fn new_open() {}
    /// # Safety
    /// This fake callback is never invoked; only its address enters the test table.
    unsafe extern "C" fn new_close() {}
    /// # Safety
    /// This fake callback is never invoked; only its address enters the test table.
    unsafe extern "C" fn new_pwrite() {}
    /// # Safety
    /// This fake callback is never invoked; only its address enters the test table.
    unsafe extern "C" fn new_ftruncate() {}

    fn address(call: ErasedCall) -> usize {
        call.map_or(0, |call| call as usize)
    }

    #[test]
    fn rejected_setter_restores_all_syscalls() {
        for failed_call in 1..=4 {
            let mut call_count = 0;
            let mut open = Some(original_open as unsafe extern "C" fn());
            let mut close = Some(original_close as unsafe extern "C" fn());
            let mut pwrite = Some(original_pwrite as unsafe extern "C" fn());
            let mut ftruncate = Some(original_ftruncate as unsafe extern "C" fn());
            let result = install_syscalls(
                |name, callback| {
                    call_count += 1;
                    if name == c"open" {
                        open = callback;
                    } else if name == c"close" {
                        close = callback;
                    } else if name == c"pwrite" {
                        pwrite = callback;
                    } else if name == c"ftruncate" {
                        ftruncate = callback;
                    } else {
                        panic!("unknown syscall");
                    }
                    if call_count == failed_call {
                        1
                    } else {
                        libsqlite3_sys::SQLITE_OK
                    }
                },
                [
                    (c"close", Some(original_close), new_close),
                    (c"open", Some(original_open), new_open),
                    (c"pwrite", Some(original_pwrite), new_pwrite),
                    (c"ftruncate", Some(original_ftruncate), new_ftruncate),
                ],
            );
            assert_eq!(result, Err(InstallError::OverrideRejected));
            assert_eq!(address(open), address(Some(original_open)));
            assert_eq!(address(close), address(Some(original_close)));
            assert_eq!(address(pwrite), address(Some(original_pwrite)));
            assert_eq!(address(ftruncate), address(Some(original_ftruncate)));
            assert_eq!(call_count, failed_call + 4);
        }
    }

    #[test]
    fn concurrent_installers_admit_exactly_one_attempt() {
        let state = AtomicU8::new(0);
        let accepted = AtomicUsize::new(0);
        let start = Barrier::new(16);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let state = &state;
                let accepted = &accepted;
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    if let Ok(_attempt) = InstallAttempt::begin(state) {
                        accepted.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(accepted.load(Ordering::Relaxed), 1);
        assert_eq!(
            InstallAttempt::begin(&state).err(),
            Some(InstallError::PreviousFailure)
        );
    }

    #[test]
    fn nested_xopen_refuses_before_lower_or_owner_table() {
        OPEN_SLOT.with(|slot| {
            assert_eq!(slot.get(), None);
            slot.set(Some(7));
            let mut file = libsqlite3_sys::sqlite3_file {
                pMethods: std::ptr::dangling(),
            };
            assert_eq!(
                owner_open_inner(std::ptr::null(), &mut file, 0, std::ptr::null_mut()),
                libsqlite3_sys::SQLITE_CANTOPEN
            );
            assert!(file.pMethods.is_null());
            assert_eq!(slot.get(), Some(7));
            slot.set(None);
        });
    }

    struct TestGrant(u64);

    impl RetainedMemoryGrant for TestGrant {
        fn bytes(&self) -> u64 {
            self.0
        }
    }

    #[test]
    fn grant_must_match_preallocated_backing_before_registration() {
        let limits = OwnerLimits {
            max_files: NonZeroUsize::new(2).expect("nonzero"),
        };
        let bytes = InstalledHook::owner_vfs_backing_bytes(limits).expect("finite layout");
        assert!(verify_grant(bytes, &TestGrant(bytes)).is_ok());
        assert_eq!(
            verify_grant(bytes, &TestGrant(bytes - 1)),
            Err(InstallError::InvalidGrant)
        );
        let impossible = OwnerLimits {
            max_files: NonZeroUsize::new(usize::MAX).expect("nonzero maximum"),
        };
        assert_eq!(
            InstalledHook::owner_vfs_backing_bytes(impossible),
            Err(InstallError::MemoryUnavailable)
        );
    }

    #[test]
    fn fixed_owner_slots_refuse_parallel_overflow_and_reuse_after_close() {
        let slots = allocate_slots(2).expect("two slots");
        let table = Arc::new(Mutex::new(OwnerTable {
            slots,
            shm_nodes: allocate_shm_nodes(2).expect("two SHM slots"),
            _grant: Arc::new(TestGrant(1)),
        }));
        let accepted = AtomicUsize::new(0);
        let start = Barrier::new(8);
        std::thread::scope(|scope| {
            for file in 1..=8 {
                let table = Arc::clone(&table);
                let accepted = &accepted;
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    if table.lock().expect("owner table").reserve(file).is_some() {
                        accepted.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(accepted.load(Ordering::Relaxed), 2);
        let mut table = table.lock().expect("owner table");
        let index = table
            .slots
            .iter()
            .position(|slot| slot.state != SlotState::Free)
            .expect("one occupied slot");
        table.free(index);
        assert!(table.reserve(9).is_some());
    }

    #[test]
    fn per_open_token_limit_refuses_ninth_without_growing() {
        let ledger = FdLedger::with_capacity(NonZeroUsize::new(9).expect("nonzero ledger"))
            .expect("ledger backing");
        let mut table = OwnerTable {
            slots: allocate_slots(1).expect("one slot"),
            shm_nodes: allocate_shm_nodes(1).expect("one SHM slot"),
            _grant: Arc::new(TestGrant(1)),
        };
        let index = table.reserve(1).expect("free slot");
        for number in 1_u64..=9 {
            let token = match ledger.open_with(
                NonZeroU64::new(1).expect("root"),
                NonZeroU64::new(number).expect("charge"),
                || Ok::<RawFd, ()>(100 + c_int::try_from(number).expect("small FD")),
                |_fd| {
                    Some(FileIdentity {
                        device: 1,
                        inode: number,
                    })
                },
            ) {
                Ok(OpenDisposition::Ready(token)) => token,
                _ => panic!("synthetic descriptor admission"),
            };
            assert_eq!(table.push(index, token), number <= 8);
        }
        assert_eq!(table.slots[index].tokens.iter().flatten().count(), 8);
    }
}

#[cfg(target_os = "linux")]
fn set_errno(value: c_int) {
    // SAFETY: libc exposes the calling thread's errno address.
    unsafe { *libc::__errno_location() = value };
}

#[cfg(target_os = "linux")]
fn get_errno() -> c_int {
    // SAFETY: libc exposes the calling thread's errno address.
    unsafe { *libc::__errno_location() }
}
