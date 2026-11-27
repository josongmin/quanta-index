//! SQLite reads pinned catalog descriptors through its public VFS v1 API.
//! The host owns the SQLite-compatible lock, so the pager's logical locks do
//! not acquire or release OS locks. The WAL index stays in SQLite's heap.

use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs::{File, Metadata},
    io,
    os::{
        fd::{AsRawFd as _, FromRawFd as _, IntoRawFd as _},
        unix::fs::MetadataExt as _,
    },
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::{Arc, OnceLock},
};

use libsqlite3_sys as sqlite;
use rusqlite::{Connection, OpenFlags, config::DbConfig};

use super::protected_lock::lock_sqlite_source;

const MAIN: &CStr = c"/quanta-protected-catalog";
const WAL: &CStr = c"/quanta-protected-catalog-wal";
const VFS: &CStr = c"quanta-protected-catalog-fd";

#[derive(Clone, Copy, Debug)]
pub struct ProtectedReadLimits {
    pub main_bytes: u64,
    pub wal_bytes: u64,
    pub expected: Option<ProtectedReadExpectation>,
}

/// The preflight witness used for Work admission in the caller's namespace.
#[derive(Clone, Copy, Debug)]
pub struct ProtectedReadExpectation {
    pub main: ProtectedFileIdentity,
    pub wal: Option<ProtectedFileIdentity>,
}

#[derive(Debug)]
pub enum ProtectedReadError {
    Cancelled,
    Contended,
    UnsafeInput,
    TruncatedMain,
    Changed,
    ByteLimit,
    RollbackJournal,
    Io(io::Error),
    Sqlite(rusqlite::Error),
    SqliteClose(rusqlite::Error),
    VfsUnavailable,
    Reentrant,
    Unretired,
    Close(io::Error),
}

impl From<io::Error> for ProtectedReadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for ProtectedReadError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// Inspect the exact main and WAL inodes under an already held catalog directory.
/// The caller retains its root/namespace authority and charges the admitted
/// main and WAL envelopes. The callback runs with query-only SQLite and an
/// exclusive source lock. Concurrent SQLite readers and writers are refused.
/// An existing rollback journal is outside this WAL/main-image contract.
pub fn with_protected_read<T, E>(
    directory: &File,
    basename: &CStr,
    limits: ProtectedReadLimits,
    check: Arc<dyn Fn() -> bool + Send + Sync>,
    inspect: impl FnOnce(&mut Connection) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<ProtectedReadError>,
{
    let scope = Scope::enter()?;
    let inputs = Arc::new(Inputs::open(directory, basename, limits, check)?);
    register()?;
    scope.bind(Arc::clone(&inputs));
    let main_only = inputs
        .wal
        .as_ref()
        .is_none_or(|wal| wal.identity.bytes == 0);
    let path = if main_only {
        "file:/quanta-protected-catalog?immutable=1"
    } else {
        "/quanta-protected-catalog"
    };
    let mut connection = Connection::open_with_flags_and_vfs(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | if main_only {
                OpenFlags::SQLITE_OPEN_URI
            } else {
                OpenFlags::empty()
            },
        "quanta-protected-catalog-fd",
    )
    .map_err(|error| {
        if running(&inputs.check) {
            ProtectedReadError::Sqlite(error)
        } else {
            ProtectedReadError::Cancelled
        }
    })?;
    let result = (|| {
        connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .map_err(ProtectedReadError::from)?;
        connection
            .execute_batch(
                "PRAGMA locking_mode=EXCLUSIVE; PRAGMA query_only=ON; \
                 PRAGMA temp_store=MEMORY; PRAGMA mmap_size=0;",
            )
            .map_err(ProtectedReadError::from)?;
        inspect(&mut connection)
    })();
    let closed = connection.close().map_err(|(_connection, error)| error);
    let verified = inputs.verify(directory, basename);
    drop(scope);
    Arc::try_unwrap(inputs)
        .map_err(|_live| ProtectedReadError::Unretired)?
        .close_files()?;
    verified?;
    let value = result?;
    closed.map_err(ProtectedReadError::SqliteClose)?;
    Ok(value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtectedFileIdentity {
    pub dev: u64,
    pub ino: u64,
    pub bytes: u64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
}

impl ProtectedFileIdentity {
    fn capture(metadata: Metadata, device: u64) -> Result<Self, ProtectedReadError> {
        // SAFETY: geteuid has no preconditions.
        let owner = unsafe { libc::geteuid() };
        if !metadata.is_file()
            || metadata.uid() != owner
            || metadata.nlink() != 1
            || metadata.dev() != device
            || metadata.mode() & 0o077 != 0
        {
            return Err(ProtectedReadError::UnsafeInput);
        }
        Ok(Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            bytes: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

struct Image {
    file: File,
    identity: ProtectedFileIdentity,
}

impl Image {
    fn new(file: File, device: u64, ceiling: u64) -> Result<Self, ProtectedReadError> {
        let identity = ProtectedFileIdentity::capture(file.metadata()?, device)?;
        if identity.bytes > ceiling || identity.bytes > i64::MAX as u64 {
            return Err(ProtectedReadError::ByteLimit);
        }
        Ok(Self { file, identity })
    }
}

struct Inputs {
    main: Image,
    wal: Option<Image>,
    wal_name: CString,
    journal_name: CString,
    check: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Inputs {
    fn open(
        directory: &File,
        basename: &CStr,
        limits: ProtectedReadLimits,
        check: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self, ProtectedReadError> {
        if !running(&check) {
            return Err(ProtectedReadError::Cancelled);
        }
        let name = basename.to_bytes();
        if name.is_empty()
            || name.len() > 247
            || name.contains(&b'/')
            || name == b"."
            || name == b".."
        {
            return Err(ProtectedReadError::UnsafeInput);
        }
        let directory_meta = directory.metadata()?;
        // SAFETY: geteuid has no preconditions.
        let owner = unsafe { libc::geteuid() };
        if !directory_meta.is_dir()
            || directory_meta.uid() != owner
            || directory_meta.mode() & 0o022 != 0
        {
            return Err(ProtectedReadError::UnsafeInput);
        }
        let device = directory_meta.dev();
        let _main_entry = regular_name(directory, basename)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let file = open_at(directory, basename, libc::O_RDWR)?;
        let _initial = ProtectedFileIdentity::capture(file.metadata()?, device)?;
        match lock_sqlite_source(&file) {
            Err(error) if matches!(error.raw_os_error(), Some(libc::EACCES | libc::EAGAIN)) => {
                return Err(ProtectedReadError::Contended);
            }
            result => result?,
        }
        let main = Image::new(file, device, limits.main_bytes)?;
        if limits
            .expected
            .is_some_and(|expected| expected.main != main.identity)
        {
            return Err(ProtectedReadError::Changed);
        }
        if main.identity.bytes < 100 {
            return Err(ProtectedReadError::TruncatedMain);
        }
        let wal_name = sibling(basename, b"-wal")?;
        let journal_name = sibling(basename, b"-journal")?;
        if entry_exists(directory, &journal_name)? {
            return Err(ProtectedReadError::RollbackJournal);
        }
        let wal_present = regular_name(directory, &wal_name)?.is_some();
        if limits
            .expected
            .is_some_and(|expected| expected.wal.is_some() != wal_present)
        {
            return Err(ProtectedReadError::Changed);
        }
        let wal = if wal_present {
            Some(Image::new(
                open_at(directory, &wal_name, libc::O_RDONLY)?,
                device,
                limits.wal_bytes,
            )?)
        } else {
            None
        };
        if limits
            .expected
            .is_some_and(|expected| expected.wal != wal.as_ref().map(|image| image.identity))
        {
            return Err(ProtectedReadError::Changed);
        }
        let inputs = Self {
            main,
            wal,
            wal_name,
            journal_name,
            check,
        };
        inputs.verify(directory, basename)?;
        Ok(inputs)
    }

    fn verify(&self, directory: &File, basename: &CStr) -> Result<(), ProtectedReadError> {
        if !running(&self.check) {
            return Err(ProtectedReadError::Cancelled);
        }
        let device = self.main.identity.dev;
        let main = ProtectedFileIdentity::capture(self.main.file.metadata()?, device)?;
        let wal = self
            .wal
            .as_ref()
            .map(|image| ProtectedFileIdentity::capture(image.file.metadata()?, device))
            .transpose()?;
        if main != self.main.identity
            || wal.as_ref() != self.wal.as_ref().map(|image| &image.identity)
            || !same_link(directory, basename, Some(&main))?
            || !same_link(directory, &self.wal_name, wal.as_ref())?
            || entry_exists(directory, &self.journal_name)?
        {
            return Err(ProtectedReadError::Changed);
        }
        Ok(())
    }

    fn close_files(self) -> Result<(), ProtectedReadError> {
        let wal = self.wal.map_or(Ok(()), |wal| close_source(wal.file));
        let main = close_source(self.main.file);
        wal.and(main)
    }
}

fn close_source(file: File) -> Result<(), ProtectedReadError> {
    // SAFETY: consumes one owned descriptor. A failed Linux close is never
    // retried against the same numeric descriptor.
    if unsafe { libc::close(file.into_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(ProtectedReadError::Close(io::Error::last_os_error()))
    }
}

fn running(check: &Arc<dyn Fn() -> bool + Send + Sync>) -> bool {
    catch_unwind(AssertUnwindSafe(|| check())).unwrap_or(false)
}

fn sibling(name: &CStr, suffix: &[u8]) -> Result<CString, ProtectedReadError> {
    CString::new([name.to_bytes(), suffix].concat()).map_err(|_| ProtectedReadError::UnsafeInput)
}

fn open_at(directory: &File, name: &CStr, access: c_int) -> io::Result<File> {
    // SAFETY: the directory and C string are live for this syscall. openat
    // returns ownership of any nonnegative descriptor.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            access | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat returned one newly owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn regular_name(directory: &File, name: &CStr) -> Result<Option<libc::stat>, ProtectedReadError> {
    match stat_at(directory, name)? {
        Some(stat) if stat.st_mode & libc::S_IFMT != libc::S_IFREG => {
            Err(ProtectedReadError::UnsafeInput)
        }
        result => Ok(result),
    }
}

fn stat_at(directory: &File, name: &CStr) -> io::Result<Option<libc::stat>> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstatat writes one stat to the provided storage on success.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        return Ok(Some(unsafe { stat.assume_init() }));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(error)
    }
}

fn same_link(
    directory: &File,
    name: &CStr,
    expected: Option<&ProtectedFileIdentity>,
) -> io::Result<bool> {
    Ok(match (stat_at(directory, name)?, expected) {
        (None, None) => true,
        (Some(actual), Some(expected)) => {
            actual.st_dev == expected.dev && actual.st_ino == expected.ino
        }
        _ => false,
    })
}

fn entry_exists(directory: &File, name: &CStr) -> io::Result<bool> {
    Ok(stat_at(directory, name)?.is_some())
}

thread_local! {
    static CURRENT: RefCell<Option<ScopeState>> = const { RefCell::new(None) };
}

enum ScopeState {
    Opening,
    Reading(Arc<Inputs>),
}

struct Scope;

impl Scope {
    fn enter() -> Result<Self, ProtectedReadError> {
        CURRENT.with(|slot| {
            let mut current = slot.borrow_mut();
            if current.is_some() {
                return Err(ProtectedReadError::Reentrant);
            }
            *current = Some(ScopeState::Opening);
            Ok(Self)
        })
    }

    fn bind(&self, inputs: Arc<Inputs>) {
        CURRENT.with(|slot| *slot.borrow_mut() = Some(ScopeState::Reading(inputs)));
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|slot| drop(slot.borrow_mut().take()));
    }
}

#[repr(C)]
struct SnapshotFile {
    base: sqlite::sqlite3_file,
    inputs: Arc<Inputs>,
    wal: bool,
}

impl SnapshotFile {
    fn image(&self) -> Option<&Image> {
        if self.wal {
            self.inputs.wal.as_ref()
        } else {
            Some(&self.inputs.main)
        }
    }
}

fn register() -> Result<(), ProtectedReadError> {
    static REGISTRATION: OnceLock<bool> = OnceLock::new();
    if *REGISTRATION.get_or_init(|| {
        // SAFETY: SQLite owns the registered Unix VFS and its availability
        // is checked before copying the stateless v1 utility callbacks.
        let base = unsafe { sqlite::sqlite3_vfs_find(c"unix".as_ptr()).as_ref() };
        let Some(base) = base else {
            return false;
        };
        let Ok(file_size) = c_int::try_from(std::mem::size_of::<SnapshotFile>()) else {
            return false;
        };
        let mut vfs = Box::new(sqlite::sqlite3_vfs {
            iVersion: 1,
            szOsFile: file_size,
            pNext: ptr::null_mut(),
            zName: VFS.as_ptr(),
            pAppData: ptr::null_mut(),
            xOpen: Some(open),
            xDelete: Some(delete),
            xAccess: Some(access),
            xFullPathname: Some(full_path),
            xDlOpen: Some(no_extension),
            xCurrentTimeInt64: None,
            xSetSystemCall: None,
            xGetSystemCall: None,
            xNextSystemCall: None,
            ..*base
        });
        // SAFETY: unique registration; the VFS remains valid for process
        // lifetime, and default VFS selection is intentionally untouched.
        if unsafe { sqlite::sqlite3_vfs_register(vfs.as_mut(), 0) } != sqlite::SQLITE_OK {
            return false;
        }
        Box::leak(vfs);
        true
    }) {
        Ok(())
    } else {
        Err(ProtectedReadError::VfsUnavailable)
    }
}

unsafe extern "C" fn open(
    _vfs: *mut sqlite::sqlite3_vfs,
    name: *const c_char,
    file: *mut sqlite::sqlite3_file,
    flags: c_int,
    out_flags: *mut c_int,
) -> c_int {
    // SAFETY: SQLite provides writable file storage sized by szOsFile and
    // a C string for named opens. A failed open must leave pMethods null.
    unsafe { (*file).pMethods = ptr::null() };
    let Some(name) = (!name.is_null()).then(|| unsafe { CStr::from_ptr(name) }) else {
        return sqlite::SQLITE_CANTOPEN;
    };
    let wal = name == WAL && flags & sqlite::SQLITE_OPEN_WAL != 0;
    if !wal && (name != MAIN || flags & sqlite::SQLITE_OPEN_MAIN_DB == 0) {
        return sqlite::SQLITE_CANTOPEN;
    }
    let Some(inputs) = CURRENT.with(|slot| match slot.borrow().as_ref() {
        Some(ScopeState::Reading(inputs)) => Some(Arc::clone(inputs)),
        Some(ScopeState::Opening) | None => None,
    }) else {
        return sqlite::SQLITE_CANTOPEN;
    };
    if !running(&inputs.check) || (wal && inputs.wal.is_none()) {
        return sqlite::SQLITE_CANTOPEN;
    }
    unsafe {
        file.cast::<SnapshotFile>().write(SnapshotFile {
            base: sqlite::sqlite3_file {
                pMethods: &FILE_METHODS,
            },
            inputs,
            wal,
        });
        if !out_flags.is_null() {
            *out_flags = sqlite::SQLITE_OPEN_READONLY;
        }
    }
    sqlite::SQLITE_OK
}

unsafe extern "C" fn close(file: *mut sqlite::sqlite3_file) -> c_int {
    // SAFETY: only a successful xOpen installs these methods; SQLite calls
    // xClose once per initialized file.
    unsafe { ptr::drop_in_place(file.cast::<SnapshotFile>()) };
    sqlite::SQLITE_OK
}

unsafe extern "C" fn read(
    file: *mut sqlite::sqlite3_file,
    buffer: *mut c_void,
    count: c_int,
    offset: i64,
) -> c_int {
    let (Ok(count), Ok(offset)) = (usize::try_from(count), u64::try_from(offset)) else {
        return sqlite::SQLITE_IOERR_READ;
    };
    // SAFETY: SQLite owns an initialized SnapshotFile and count writable bytes.
    let file = unsafe { &*file.cast::<SnapshotFile>() };
    let Some(image) = file.image() else {
        return sqlite::SQLITE_IOERR_READ;
    };
    if count == 0 {
        return sqlite::SQLITE_OK;
    }
    if buffer.is_null() {
        return sqlite::SQLITE_IOERR_READ;
    }
    let available = image.identity.bytes.saturating_sub(offset);
    let bounded = count.min(usize::try_from(available).unwrap_or(usize::MAX));
    let mut read = 0;
    while read < bounded {
        if !running(&file.inputs.check) {
            return sqlite::SQLITE_INTERRUPT;
        }
        // SAFETY: [read, bounded) is writable SQLite storage. It may be
        // uninitialized, so pass it directly to pread64 without a Rust slice.
        // The admitted image length bounds the offset to i64::MAX.
        let bytes = unsafe {
            libc::pread64(
                image.file.as_raw_fd(),
                buffer.cast::<u8>().add(read).cast(),
                bounded - read,
                (offset + read as u64) as i64,
            )
        };
        if bytes == 0 {
            break;
        }
        if let Ok(bytes) = usize::try_from(bytes) {
            read += bytes;
        } else if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return sqlite::SQLITE_IOERR_READ;
        }
    }
    if read == count {
        return sqlite::SQLITE_OK;
    }
    // SAFETY: SQLite requires the unread suffix of a short read to be zeroed.
    unsafe { ptr::write_bytes(buffer.cast::<u8>().add(read), 0, count - read) };
    sqlite::SQLITE_IOERR_SHORT_READ
}

unsafe extern "C" fn size(file: *mut sqlite::sqlite3_file, size: *mut i64) -> c_int {
    let file = unsafe { &*file.cast::<SnapshotFile>() };
    if !running(&file.inputs.check) {
        return sqlite::SQLITE_INTERRUPT;
    }
    let Some(image) = file.image() else {
        return sqlite::SQLITE_IOERR_FSTAT;
    };
    unsafe { *size = image.identity.bytes as i64 };
    sqlite::SQLITE_OK
}

unsafe extern "C" fn access(
    _vfs: *mut sqlite::sqlite3_vfs,
    path: *const c_char,
    flags: c_int,
    exists: *mut c_int,
) -> c_int {
    let name = unsafe { CStr::from_ptr(path) };
    let exists_at_snapshot = CURRENT.with(|slot| {
        let state = slot.borrow();
        let Some(ScopeState::Reading(inputs)) = state.as_ref() else {
            return false;
        };
        flags != sqlite::SQLITE_ACCESS_READWRITE
            && (name == MAIN || (name == WAL && inputs.wal.is_some()))
    });
    unsafe { *exists = c_int::from(exists_at_snapshot) };
    sqlite::SQLITE_OK
}

unsafe extern "C" fn full_path(
    _vfs: *mut sqlite::sqlite3_vfs,
    path: *const c_char,
    count: c_int,
    out: *mut c_char,
) -> c_int {
    let name = unsafe { CStr::from_ptr(path) };
    let bytes = name.to_bytes_with_nul();
    if name != MAIN || usize::try_from(count).map_or(true, |count| count < bytes.len()) {
        return sqlite::SQLITE_CANTOPEN;
    }
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast(), bytes.len()) };
    sqlite::SQLITE_OK
}

unsafe extern "C" fn delete(_: *mut sqlite::sqlite3_vfs, _: *const c_char, _: c_int) -> c_int {
    sqlite::SQLITE_READONLY
}
unsafe extern "C" fn no_extension(_: *mut sqlite::sqlite3_vfs, _: *const c_char) -> *mut c_void {
    ptr::null_mut()
}
unsafe extern "C" fn write(
    _: *mut sqlite::sqlite3_file,
    _: *const c_void,
    _: c_int,
    _: i64,
) -> c_int {
    sqlite::SQLITE_READONLY
}
unsafe extern "C" fn truncate(_: *mut sqlite::sqlite3_file, _: i64) -> c_int {
    sqlite::SQLITE_READONLY
}
unsafe extern "C" fn sync(_: *mut sqlite::sqlite3_file, _: c_int) -> c_int {
    sqlite::SQLITE_READONLY
}
unsafe extern "C" fn lock(_: *mut sqlite::sqlite3_file, _: c_int) -> c_int {
    sqlite::SQLITE_OK
}
unsafe extern "C" fn reserved(_: *mut sqlite::sqlite3_file, out: *mut c_int) -> c_int {
    unsafe { *out = 0 };
    sqlite::SQLITE_OK
}
unsafe extern "C" fn file_control(_: *mut sqlite::sqlite3_file, _: c_int, _: *mut c_void) -> c_int {
    sqlite::SQLITE_NOTFOUND
}
unsafe extern "C" fn sector_size(_: *mut sqlite::sqlite3_file) -> c_int {
    4096
}
unsafe extern "C" fn characteristics(_: *mut sqlite::sqlite3_file) -> c_int {
    0
}

static FILE_METHODS: sqlite::sqlite3_io_methods = sqlite::sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(close),
    xRead: Some(read),
    xWrite: Some(write),
    xTruncate: Some(truncate),
    xSync: Some(sync),
    xFileSize: Some(size),
    xLock: Some(lock),
    xUnlock: Some(lock),
    xCheckReservedLock: Some(reserved),
    xFileControl: Some(file_control),
    xSectorSize: Some(sector_size),
    xDeviceCharacteristics: Some(characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};
