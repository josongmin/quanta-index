//! Controlled native VFS boundary, behind the private `quanta-capacity-vfs`
//! feature. SQLite FFI and physical handle enforcement stay in this vendor.

use std::boxed::Box;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs::File;
use std::marker::PhantomData;
use std::num::NonZeroU64;
#[cfg(feature = "quanta-capacity-vfs-test-hooks")]
use std::os::fd::AsRawFd as _;
use std::os::fd::IntoRawFd as _;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

const SQLITE_OK: c_int = 0;
const SQLITE_BUSY: c_int = 5;
const SQLITE_NOMEM: c_int = 7;
const SQLITE_IOERR: c_int = 10;
const SQLITE_FULL: c_int = 13;
/// Private extended SQLITE_FULL returned only by a configured quota refusal.
/// A collapsed primary SQLITE_FULL is deliberately not classified as capacity.
pub const CAPACITY_EXTENDED_CODE: c_int = SQLITE_FULL | (0x51 << 8);
const SQLITE_CANTOPEN: c_int = 14;
const SQLITE_NOTFOUND: c_int = 12;
const ABI_VERSION: u32 = 3;
const REDIRECT_MAX: usize = 4096;

#[expect(
    clippy::struct_excessive_bools,
    reason = "these independent flags mirror the native diagnostic record and do not configure VFS behavior"
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeSyncObservation {
    pub observed: bool,
    pub same_native_fd: bool,
    pub ambiguous: bool,
    pub full_requested: bool,
    pub full_attempted: bool,
    pub full_default_syscall: bool,
    pub full_result: c_int,
    pub full_errno: c_int,
    pub posix_attempted: bool,
    pub posix_result: c_int,
    pub posix_errno: c_int,
}

#[repr(C)]
struct RawNativeSyncObservation {
    observed: c_int,
    same_native_fd: c_int,
    ambiguous: c_int,
    full_requested: c_int,
    full_attempted: c_int,
    full_default_syscall: c_int,
    full_result: c_int,
    full_errno: c_int,
    posix_attempted: c_int,
    posix_result: c_int,
    posix_errno: c_int,
}

impl From<&RawNativeSyncObservation> for NativeSyncObservation {
    fn from(raw: &RawNativeSyncObservation) -> Self {
        Self {
            observed: raw.observed != 0,
            same_native_fd: raw.same_native_fd != 0,
            ambiguous: raw.ambiguous != 0,
            full_requested: raw.full_requested != 0,
            full_attempted: raw.full_attempted != 0,
            full_default_syscall: raw.full_default_syscall != 0,
            full_result: raw.full_result,
            full_errno: raw.full_errno,
            posix_attempted: raw.posix_attempted != 0,
            posix_result: raw.posix_result,
            posix_errno: raw.posix_errno,
        }
    }
}

#[repr(C)]
struct RawEvent {
    abi_version: u32,
    kind: u32,
    scope_id: u64,
    handle_id: u64,
    open_flags: c_int,
    sqlite_result: c_int,
    operation: c_int,
    observed_size_valid: c_int,
    offset: i64,
    amount: i64,
    target_size: i64,
    observed_size: i64,
    witness_valid: c_int,
    fd_retired: c_int,
    first_shm_fd: c_int,
    witness_device: u64,
    witness_inode: u64,
    witness_bytes: u64,
    witness_links: u64,
    path: *const c_char,
    redirect_len: u32,
    redirect: [c_char; REDIRECT_MAX],
    native_sync_opt_in: c_int,
    native_sync: RawNativeSyncObservation,
}

unsafe extern "C" {
    fn qiv_register(
        name: *const c_char,
        max_open_handles: u32,
        callback: extern "C" fn(*mut c_void, *mut RawEvent) -> c_int,
        context: *mut c_void,
    ) -> c_int;
    fn qiv_enter_scope(scope_id: u64) -> u64;
    fn qiv_enter_verification(enabled: c_int) -> c_int;
    fn qiv_drain_retirements() -> c_int;
    fn qiv_event_layout(slots: *mut u32, count: usize) -> c_int;
    fn qiv_close_owned_fd(descriptor: c_int) -> c_int;
    #[cfg(feature = "quanta-capacity-vfs-test-hooks")]
    fn qiv_test_first_shm_read_map(path: *const c_char, mapping_present: *mut c_int) -> c_int;
    #[cfg(feature = "quanta-capacity-vfs-test-hooks")]
    fn qiv_test_native_sync_syscall_outcomes(descriptor: c_int) -> c_int;
}

/// Test-only oracle for the raw full-sync and POSIX outcomes. Fake fcntl calls
/// are injected per call; SQLite's process-global syscall table is untouched.
/// The caller retains ownership of the regular-file descriptor.
#[cfg(feature = "quanta-capacity-vfs-test-hooks")]
pub fn test_native_sync_syscall_outcomes(file: &File) -> Result<(), Refusal> {
    let rc = unsafe { qiv_test_native_sync_syscall_outcomes(file.as_raw_fd()) };
    if rc == SQLITE_OK {
        Ok(())
    } else {
        Err(Refusal::Io)
    }
}

#[cfg(feature = "quanta-capacity-vfs-test-hooks")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShmReadProbe {
    /// A first read-only region probe usually finds no mapped region after
    /// SQLite resets an unused SHM file to its three-byte bootstrap width.
    pub mapping_present: bool,
}

#[cfg(feature = "quanta-capacity-vfs-test-hooks")]
#[derive(Debug)]
pub enum ShmReadProbeError {
    Abi(RegisterError),
    Sqlite(c_int),
}

/// Consume an owned regular-file FD and report only a proven successful close.
/// On error the descriptor may already be closed or may remain open; callers
/// must retain its publication charge and must not retry the numeric FD.
pub fn close_owned_file(file: File) -> Result<(), std::io::Error> {
    let descriptor = file.into_raw_fd();
    let result = unsafe { qiv_close_owned_fd(descriptor) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(result))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    BeforeOpen,
    AfterOpen,
    BeforeWrite,
    AfterWrite,
    BeforeTruncate,
    AfterTruncate,
    BeforeSync,
    AfterSync,
    BeforeShmMap,
    AfterShmMap,
    AfterShmUnmap,
    BeforeDelete,
    AfterDelete,
    AfterClose,
    BeforeFileControl,
    AfterFileControl,
    FdRetired,
    BeforeNativeOpen,
    OpenAbortedNoFd,
    BeforeVerificationRead,
}

impl EventKind {
    fn decode(raw: u32) -> Option<Self> {
        Some(match raw {
            1 => Self::BeforeOpen,
            2 => Self::AfterOpen,
            3 => Self::BeforeWrite,
            4 => Self::AfterWrite,
            5 => Self::BeforeTruncate,
            6 => Self::AfterTruncate,
            7 => Self::BeforeSync,
            8 => Self::AfterSync,
            9 => Self::BeforeShmMap,
            10 => Self::AfterShmMap,
            11 => Self::AfterShmUnmap,
            12 => Self::BeforeDelete,
            13 => Self::AfterDelete,
            14 => Self::AfterClose,
            15 => Self::BeforeFileControl,
            16 => Self::AfterFileControl,
            17 => Self::FdRetired,
            18 => Self::BeforeNativeOpen,
            19 => Self::OpenAbortedNoFd,
            20 => Self::BeforeVerificationRead,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    Capacity,
    Io,
    OutOfMemory,
    CannotOpen,
    Busy,
    UnsupportedFileControl,
}

impl Refusal {
    const fn sqlite_code(self) -> c_int {
        match self {
            Self::Capacity => CAPACITY_EXTENDED_CODE,
            Self::Io => SQLITE_IOERR,
            Self::OutOfMemory => SQLITE_NOMEM,
            Self::CannotOpen => SQLITE_CANTOPEN,
            Self::Busy => SQLITE_BUSY,
            Self::UnsupportedFileControl => SQLITE_NOTFOUND,
        }
    }
}

pub struct Event<'a> {
    pub kind: EventKind,
    pub scope_id: NonZeroU64,
    pub handle_id: u64,
    pub path: Option<&'a CStr>,
    pub open_flags: c_int,
    pub sqlite_result: c_int,
    pub operation: c_int,
    pub offset: i64,
    pub amount: i64,
    pub target_size: i64,
    pub observed_size: Option<i64>,
    pub witness: Option<FileWitness>,
    /// On close/unmap, confirms the native FD actually retired. A false value
    /// requires retaining its inode charge after logical xClose.
    pub fd_retired: bool,
    /// True when this xShmMap may open/attach an SHM FD. The first attach can
    /// truncate an existing SHM file to 3 bytes even when extend is false.
    pub first_shm_fd: bool,
    pub native_sync: NativeSyncObservation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileWitness {
    pub device: u64,
    pub inode: u64,
    pub bytes: u64,
    pub links: u64,
}

/// Callback state is owned for the process lifetime. Implementations must
/// never call SQLite recursively or let a panic escape; this facade catches
/// panics and converts them to IOERR. Unknown roots, handles, or file kinds
/// must return a typed refusal and retain any started charge.
pub trait Admission: Send + Sync + 'static {
    /// A redirect is permitted only for a null-name delete-on-close temp open.
    /// The name must be a privately owned path inside the inventoried root.
    fn on_event(&self, event: Event<'_>) -> Result<Option<CString>, Refusal>;

    fn observe_native_sync(&self, _scope_id: NonZeroU64) -> bool {
        false
    }
}

static REGISTERING_OR_REGISTERED: AtomicBool = AtomicBool::new(false);

pub struct RegisteredVfs {
    name: CString,
}

impl RegisteredVfs {
    pub fn name(&self) -> &CStr {
        &self.name
    }

    /// Exercise a real first SHM attach with xShmMap(0, 32768, extend=0)
    /// against an existing database through this named VFS. Explicit test
    /// feature only; all callbacks run inside the supplied root scope.
    #[cfg(feature = "quanta-capacity-vfs-test-hooks")]
    pub fn test_first_shm_read_map(
        &self,
        scope: NonZeroU64,
        path: &CStr,
    ) -> Result<ShmReadProbe, ShmReadProbeError> {
        verify_event_abi().map_err(ShmReadProbeError::Abi)?;
        let _scope_guard = self.enter_scope(scope);
        let mut mapping_present = 0;
        let rc = unsafe { qiv_test_first_shm_read_map(path.as_ptr(), &mut mapping_present) };
        if rc == SQLITE_OK {
            Ok(ShmReadProbe {
                mapping_present: mapping_present != 0,
            })
        } else {
            Err(ShmReadProbeError::Sqlite(rc))
        }
    }

    /// Retry bounded final-close notifications after a SQLite operation.
    /// Failed callbacks retain the record and can be retried later.
    pub fn drain_retirements(&self) -> Result<(), Refusal> {
        let rc = unsafe { qiv_drain_retirements() };
        if rc == SQLITE_OK {
            Ok(())
        } else {
            Err(Refusal::Io)
        }
    }

    /// Enter the root scope around **every** operation on a connection
    /// opened by this VFS. The guard cannot move to another thread.
    pub fn enter_scope(&self, scope: NonZeroU64) -> ScopeGuard {
        self.enter_scope_mode(scope, false)
    }

    /// Deny source-file mutation at the native boundary. Main DB opens retain
    /// write-capable FDs solely for exclusive POSIX locks; WAL opens are RO.
    /// Callers must select exclusive locking before any database read and
    /// disable checkpoint-on-close. SHM is refused, never created or mapped.
    pub fn enter_verification_scope(&self, scope: NonZeroU64) -> ScopeGuard {
        self.enter_scope_mode(scope, true)
    }

    fn enter_scope_mode(&self, scope: NonZeroU64, verification: bool) -> ScopeGuard {
        let previous = unsafe { qiv_enter_scope(scope.get()) };
        let previous_verification = unsafe { qiv_enter_verification(c_int::from(verification)) };
        ScopeGuard {
            previous,
            previous_verification,
            _not_send: PhantomData,
        }
    }
}

pub struct ScopeGuard {
    previous: u64,
    previous_verification: c_int,
    _not_send: PhantomData<Rc<()>>,
}

impl ScopeGuard {
    pub fn is_capacity_extended_code(&self, sqlite_extended_code: c_int) -> bool {
        sqlite_extended_code == CAPACITY_EXTENDED_CODE
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        unsafe {
            qiv_enter_scope(self.previous);
            qiv_enter_verification(self.previous_verification);
        }
    }
}

#[derive(Debug)]
pub enum RegisterError {
    AlreadyRegistered,
    InvalidMaxHandles,
    LayoutMismatch,
    Sqlite(c_int),
}

/// Compare every layout segment consumed across the C callback boundary.
/// Registration fails closed before SQLite observes the named VFS.
pub fn verify_event_abi() -> Result<(), RegisterError> {
    use std::mem::{align_of, offset_of, size_of};
    let mut native = [0_u32; 37];
    let rc = unsafe { qiv_event_layout(native.as_mut_ptr(), native.len()) };
    if rc != SQLITE_OK {
        return Err(RegisterError::Sqlite(rc));
    }
    let expected = [
        size_of::<RawEvent>() as u32,
        align_of::<RawEvent>() as u32,
        offset_of!(RawEvent, abi_version) as u32,
        offset_of!(RawEvent, kind) as u32,
        offset_of!(RawEvent, scope_id) as u32,
        offset_of!(RawEvent, handle_id) as u32,
        offset_of!(RawEvent, open_flags) as u32,
        offset_of!(RawEvent, sqlite_result) as u32,
        offset_of!(RawEvent, operation) as u32,
        offset_of!(RawEvent, observed_size_valid) as u32,
        offset_of!(RawEvent, offset) as u32,
        offset_of!(RawEvent, amount) as u32,
        offset_of!(RawEvent, target_size) as u32,
        offset_of!(RawEvent, observed_size) as u32,
        offset_of!(RawEvent, witness_valid) as u32,
        offset_of!(RawEvent, fd_retired) as u32,
        offset_of!(RawEvent, first_shm_fd) as u32,
        offset_of!(RawEvent, witness_device) as u32,
        offset_of!(RawEvent, witness_inode) as u32,
        offset_of!(RawEvent, witness_bytes) as u32,
        offset_of!(RawEvent, witness_links) as u32,
        offset_of!(RawEvent, path) as u32,
        offset_of!(RawEvent, redirect_len) as u32,
        offset_of!(RawEvent, redirect) as u32,
        offset_of!(RawEvent, native_sync_opt_in) as u32,
        offset_of!(RawEvent, native_sync) as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, observed)) as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, same_native_fd))
            as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, ambiguous))
            as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, full_requested))
            as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, full_attempted))
            as u32,
        (offset_of!(RawEvent, native_sync)
            + offset_of!(RawNativeSyncObservation, full_default_syscall)) as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, full_result))
            as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, full_errno))
            as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, posix_attempted))
            as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, posix_result))
            as u32,
        (offset_of!(RawEvent, native_sync) + offset_of!(RawNativeSyncObservation, posix_errno))
            as u32,
    ];
    if native == expected {
        Ok(())
    } else {
        Err(RegisterError::LayoutMismatch)
    }
}

/// Register once before any controlled connection. The callback is intentionally
/// retained for process lifetime because SQLite may call the VFS on close after
/// the service's logical owner has dropped. The callback must reject stale
/// scope IDs instead of accessing freed owner state.
pub fn register(
    name: CString,
    max_open_handles: u32,
    callback: Box<dyn Admission>,
) -> Result<&'static RegisteredVfs, RegisterError> {
    verify_event_abi()?;
    if max_open_handles == 0 {
        return Err(RegisterError::InvalidMaxHandles);
    }
    if REGISTERING_OR_REGISTERED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(RegisterError::AlreadyRegistered);
    }
    // Double boxing yields a stable thin C context pointer. The callback's
    // concrete type remains dynamically dispatched behind the inner Box.
    let context = Box::into_raw(Box::new(callback));
    let rc = unsafe {
        qiv_register(
            name.as_ptr(),
            max_open_handles,
            dispatch,
            context.cast::<c_void>(),
        )
    };
    if rc != SQLITE_OK {
        unsafe {
            drop(Box::from_raw(context));
        }
        REGISTERING_OR_REGISTERED.store(false, Ordering::Release);
        return Err(RegisterError::Sqlite(rc));
    }
    Ok(Box::leak(Box::new(RegisteredVfs { name })))
}

extern "C" fn dispatch(context: *mut c_void, raw: *mut RawEvent) -> c_int {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if context.is_null() || raw.is_null() {
            return Err(Refusal::Io);
        }
        // SAFETY: register passes a process-lifetime Box<Box<dyn Admission>>;
        // C passes an owned, writable qiv_event for this synchronous callback.
        let callback = unsafe { &*(context.cast::<Box<dyn Admission>>()) };
        let raw = unsafe { &mut *raw };
        if raw.abi_version != ABI_VERSION {
            return Err(Refusal::Io);
        }
        let kind = EventKind::decode(raw.kind).ok_or(Refusal::Io)?;
        let scope_id = NonZeroU64::new(raw.scope_id).ok_or(Refusal::Io)?;
        let path = if raw.path.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(raw.path) })
        };
        let event = Event {
            kind,
            scope_id,
            handle_id: raw.handle_id,
            path,
            open_flags: raw.open_flags,
            sqlite_result: raw.sqlite_result,
            operation: raw.operation,
            offset: raw.offset,
            amount: raw.amount,
            target_size: raw.target_size,
            observed_size: (raw.observed_size_valid != 0).then_some(raw.observed_size),
            fd_retired: raw.fd_retired != 0,
            first_shm_fd: raw.first_shm_fd != 0,
            native_sync: NativeSyncObservation::from(&raw.native_sync),
            witness: (raw.witness_valid != 0).then_some(FileWitness {
                device: raw.witness_device,
                inode: raw.witness_inode,
                bytes: raw.witness_bytes,
                links: raw.witness_links,
            }),
        };
        let redirect = callback.on_event(event)?;
        if kind == EventKind::BeforeSync {
            raw.native_sync_opt_in = c_int::from(callback.observe_native_sync(scope_id));
        }
        if let Some(redirect) = redirect {
            if kind != EventKind::BeforeOpen || !raw.path.is_null() {
                return Err(Refusal::Io);
            }
            let bytes = redirect.as_bytes_with_nul();
            if bytes.len() > REDIRECT_MAX {
                return Err(Refusal::CannotOpen);
            }
            for (destination, byte) in raw.redirect.iter_mut().zip(bytes) {
                *destination = *byte as c_char;
            }
            raw.redirect_len =
                u32::try_from(bytes.len() - 1).map_err(|_error| Refusal::CannotOpen)?;
        }
        Ok(())
    }));
    match outcome {
        Ok(Ok(())) => SQLITE_OK,
        Ok(Err(error)) => error.sqlite_code(),
        Err(_panic) => SQLITE_IOERR,
    }
}
