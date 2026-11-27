#![cfg(unix)]

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    ffi::{CStr, CString, c_int, c_void},
    fs::File,
    num::{NonZeroU64, NonZeroUsize},
    os::{
        fd::{AsRawFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
    sync::{
        Arc, Barrier, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
};

use quanta_index_sqlite_fd_ledger::{FdLedger, FdToken, RetirementReceipt};
use quanta_index_sqlite_unix_syscall::{
    ExclusiveProcessPermit, FileMutationKind, InstalledHook, OpenAuthority, OpenDecision,
    OwnerLimits, RetainedMemoryGrant, install,
};
use rusqlite::{Connection, OpenFlags};

type Mmap =
    unsafe extern "C" fn(*mut c_void, usize, c_int, c_int, c_int, libc::off_t) -> *mut c_void;
type Pwrite = unsafe extern "C" fn(c_int, *const c_void, usize, libc::off_t) -> isize;
static ORIGINAL_MMAP: OnceLock<Mmap> = OnceLock::new();
static ORIGINAL_PWRITE: OnceLock<Pwrite> = OnceLock::new();
static FAIL_NEXT_MMAP: AtomicBool = AtomicBool::new(false);
static FAIL_NEXT_PWRITE: AtomicBool = AtomicBool::new(false);
static MMAP_REFUSED: AtomicBool = AtomicBool::new(false);
static PARTIAL_PWRITE: AtomicUsize = AtomicUsize::new(0);

/// # Safety
/// The pinned Unix VFS calls this with libc pwrite's ABI and valid arguments.
unsafe extern "C" fn fail_one_pwrite_after_partial(
    fd: c_int,
    buffer: *const c_void,
    count: usize,
    offset: libc::off_t,
) -> isize {
    let original = ORIGINAL_PWRITE.get().expect("captured pwrite syscall");
    if count > 0 && FAIL_NEXT_PWRITE.swap(false, Ordering::SeqCst) {
        let partial = count.div_ceil(2);
        // SAFETY: SQLite supplied the original live FD, buffer, count and offset.
        let written = unsafe { original(fd, buffer, partial, offset) };
        if written > 0 {
            PARTIAL_PWRITE.store(written as usize, Ordering::SeqCst);
        }
        #[cfg(target_os = "macos")]
        // SAFETY: libc provides the calling thread's errno cell.
        unsafe {
            *libc::__error() = libc::EIO
        };
        #[cfg(target_os = "linux")]
        // SAFETY: libc provides the calling thread's errno cell.
        unsafe {
            *libc::__errno_location() = libc::EIO
        };
        return -1;
    }
    // SAFETY: The original pointer has libc pwrite's signature.
    unsafe { original(fd, buffer, count, offset) }
}

fn with_one_partial_pwrite<T>(run: impl FnOnce() -> T) -> Result<T, Box<dyn Error>> {
    // SAFETY: The oracle binary is the sole SQLite owner in this process.
    let vfs = unsafe { rusqlite::ffi::sqlite3_vfs_find(c"unix".as_ptr()) };
    if vfs.is_null() {
        return Err("Unix VFS unavailable".into());
    }
    // SAFETY: The registered Unix VFS remains live for the process lifetime.
    let (get, set) = unsafe {
        let vfs = &*vfs;
        (
            vfs.xGetSystemCall.ok_or("get syscall unavailable")?,
            vfs.xSetSystemCall.ok_or("set syscall unavailable")?,
        )
    };
    // SAFETY: The pinned SQLite Unix table uses libc pwrite's signature.
    let original = unsafe { get(vfs, c"pwrite".as_ptr()) }.ok_or("pwrite unavailable")?;
    let original: Pwrite = unsafe { std::mem::transmute(original) };
    ORIGINAL_PWRITE
        .set(original)
        .map_err(|_| "partial pwrite already installed")?;
    let replacement = unsafe {
        std::mem::transmute::<Pwrite, unsafe extern "C" fn()>(
            fail_one_pwrite_after_partial as Pwrite,
        )
    };
    // SAFETY: This isolated test owns every SQLite connection during override.
    if unsafe { set(vfs, c"pwrite".as_ptr(), Some(replacement)) } != rusqlite::ffi::SQLITE_OK {
        return Err("pwrite override rejected".into());
    }
    FAIL_NEXT_PWRITE.store(true, Ordering::SeqCst);
    let result = run();
    FAIL_NEXT_PWRITE.store(false, Ordering::SeqCst);
    // SAFETY: Restore the original before any later SQLite operation.
    let restored = unsafe {
        set(
            vfs,
            c"pwrite".as_ptr(),
            Some(std::mem::transmute::<Pwrite, unsafe extern "C" fn()>(
                original,
            )),
        )
    };
    if restored != rusqlite::ffi::SQLITE_OK {
        std::process::abort();
    }
    Ok(result)
}

/// # Safety
/// The pinned Unix VFS calls this with libc mmap's ABI and valid arguments.
unsafe extern "C" fn fail_one_mmap(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    flags: c_int,
    fd: c_int,
    offset: libc::off_t,
) -> *mut c_void {
    if FAIL_NEXT_MMAP.swap(false, Ordering::SeqCst) {
        MMAP_REFUSED.store(true, Ordering::SeqCst);
        #[cfg(target_os = "macos")]
        // SAFETY: libc provides the calling thread's errno cell.
        unsafe {
            *libc::__error() = libc::ENOMEM
        };
        #[cfg(target_os = "linux")]
        // SAFETY: libc provides the calling thread's errno cell.
        unsafe {
            *libc::__errno_location() = libc::ENOMEM
        };
        return libc::MAP_FAILED;
    }
    // SAFETY: install_mmap_refusal captured the live Unix VFS mmap syscall.
    unsafe {
        ORIGINAL_MMAP.get().expect("captured mmap syscall")(
            address, length, protection, flags, fd, offset,
        )
    }
}

fn with_one_mmap_refusal<T>(run: impl FnOnce() -> T) -> Result<T, Box<dyn Error>> {
    // SAFETY: The oracle binary is the sole SQLite owner in this process.
    let vfs = unsafe { rusqlite::ffi::sqlite3_vfs_find(c"unix".as_ptr()) };
    if vfs.is_null() {
        return Err("Unix VFS unavailable".into());
    }
    // SAFETY: The registered Unix VFS remains live for the process lifetime.
    let (get, set) = unsafe {
        let vfs = &*vfs;
        (
            vfs.xGetSystemCall.ok_or("get syscall unavailable")?,
            vfs.xSetSystemCall.ok_or("set syscall unavailable")?,
        )
    };
    // SAFETY: The pinned SQLite Unix table uses libc mmap's signature.
    let original = unsafe { get(vfs, c"mmap".as_ptr()) }.ok_or("mmap syscall unavailable")?;
    let original: Mmap = unsafe { std::mem::transmute(original) };
    if ORIGINAL_MMAP.set(original).is_err() {
        return Err("mmap refusal already installed".into());
    }
    // SAFETY: The replacement has the pinned Unix mmap ABI.
    let replacement =
        unsafe { std::mem::transmute::<Mmap, unsafe extern "C" fn()>(fail_one_mmap as Mmap) };
    // SAFETY: This isolated test owns every SQLite connection during override.
    if unsafe { set(vfs, c"mmap".as_ptr(), Some(replacement)) } != rusqlite::ffi::SQLITE_OK {
        return Err("mmap override rejected".into());
    }
    FAIL_NEXT_MMAP.store(true, Ordering::SeqCst);
    let result = run();
    FAIL_NEXT_MMAP.store(false, Ordering::SeqCst);
    // SAFETY: Restore the original before any later test operation.
    let restored = unsafe {
        set(
            vfs,
            c"mmap".as_ptr(),
            Some(std::mem::transmute::<Mmap, unsafe extern "C" fn()>(
                original,
            )),
        )
    };
    if restored != rusqlite::ffi::SQLITE_OK {
        std::process::abort();
    }
    Ok(result)
}

struct Authority {
    root_path: Vec<u8>,
    root_dir: File,
    next_charge: AtomicU64,
    db_opens: AtomicUsize,
    wal_opens: AtomicUsize,
    shm_opens: AtomicUsize,
    refused: AtomicUsize,
    quarantined: AtomicUsize,
    deny_shm: AtomicBool,
    shm_limit: AtomicU64,
    shm_funded: AtomicU64,
    shm_reservations: Mutex<BTreeMap<u64, (FdToken, u64, u64)>>,
    shm_retained: Mutex<BTreeMap<u64, u64>>,
    shm_growth_calls: AtomicUsize,
    shm_growth_peak: AtomicUsize,
    file_limit: AtomicU64,
    file_funded: AtomicU64,
    file_reservations: Mutex<BTreeMap<u64, (FdToken, u64, u64)>>,
    file_retained: Mutex<BTreeMap<u64, u64>>,
    write_calls: AtomicUsize,
    truncate_calls: AtomicUsize,
    sync_calls: AtomicUsize,
    delete_calls: AtomicUsize,
    delete_results: Mutex<Vec<c_int>>,
    delete_pending: Mutex<BTreeMap<u64, Vec<u8>>>,
    delete_quarantined: Mutex<Vec<u64>>,
    file_settlements: Mutex<Vec<(u64, u64, c_int, u64)>>,
    deny_next_write: AtomicBool,
    deny_next_file_settlement: AtomicBool,
    deny_next_sync: AtomicBool,
    deny_next_delete: AtomicBool,
    opened: Mutex<Vec<(Vec<u8>, FdToken)>>,
    retired: Mutex<Vec<RetirementReceipt>>,
}

struct SyntheticGrant(u64);

impl RetainedMemoryGrant for SyntheticGrant {
    fn bytes(&self) -> u64 {
        self.0
    }
}

impl OpenAuthority for Authority {
    fn decide(&self, path: &CStr) -> OpenDecision {
        let name = path.to_bytes();
        if name == b"/dev/urandom" {
            return OpenDecision::Auxiliary;
        }
        if !name.starts_with(&self.root_path) || name.get(self.root_path.len()) != Some(&b'/') {
            return OpenDecision::Deny(13);
        }
        let shared = name.ends_with(b"-shm");
        if shared && self.deny_shm.load(Ordering::Relaxed) {
            return OpenDecision::Deny(libc::EACCES);
        }
        if name.ends_with(b"upstream.sqlite") {
            self.db_opens.fetch_add(1, Ordering::Relaxed);
        } else if name.ends_with(b"-wal") {
            self.wal_opens.fetch_add(1, Ordering::Relaxed);
        } else if name.ends_with(b"-shm") {
            self.shm_opens.fetch_add(1, Ordering::Relaxed);
        }
        let charge = self.next_charge.fetch_add(1, Ordering::Relaxed);
        let root = NonZeroU64::new(1).expect("fixed root token");
        let charge = NonZeroU64::new(charge).expect("charge token has not wrapped");
        if shared {
            OpenDecision::AdmitShared { root, charge }
        } else {
            OpenDecision::Admit { root, charge }
        }
    }

    fn open_admitted(&self, path: &CStr, flags: c_int, mode: c_int) -> Result<RawFd, c_int> {
        let bytes = path.to_bytes_with_nul();
        let relative = bytes
            .get(self.root_path.len().saturating_add(1)..)
            .ok_or(libc::EACCES)?;
        let relative = CStr::from_bytes_with_nul(relative).map_err(|_error| libc::EACCES)?;
        if relative.to_bytes().is_empty()
            || relative.to_bytes() == b"."
            || relative.to_bytes() == b".."
            || relative.to_bytes().contains(&b'/')
        {
            return Err(libc::EACCES);
        }
        // SAFETY: The relative component is a validated C string and root_dir
        // remains open for the lifetime of every SQLite connection in this test.
        let fd = unsafe {
            libc::openat(
                self.root_dir.as_raw_fd(),
                relative.as_ptr(),
                flags | libc::O_NOFOLLOW,
                mode,
            )
        };
        if fd < 0 {
            let errno = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(libc::EIO);
            Err(errno)
        } else {
            Ok(fd)
        }
    }

    fn opened(&self, path: &CStr, token: FdToken) {
        self.opened
            .lock()
            .expect("open record mutex")
            .push((path.to_bytes().to_vec(), token));
    }

    fn open_refused(&self, _charge: NonZeroU64) {
        self.refused.fetch_add(1, Ordering::Relaxed);
    }

    fn quarantined(&self, _charge: NonZeroU64) {
        self.quarantined.fetch_add(1, Ordering::Relaxed);
    }

    fn retired(&self, receipt: RetirementReceipt) {
        if let Some(bytes) = self
            .file_retained
            .lock()
            .expect("file retained mutex")
            .remove(&receipt.generation.get())
        {
            self.file_funded.fetch_sub(bytes, Ordering::AcqRel);
        }
        if let Some(bytes) = self
            .shm_retained
            .lock()
            .expect("SHM retained mutex")
            .remove(&receipt.generation.get())
        {
            self.shm_funded.fetch_sub(bytes, Ordering::AcqRel);
        }
        self.retired.lock().expect("retirement mutex").push(receipt);
    }

    fn reserve_shm_growth(
        &self,
        token: FdToken,
        previous_len: u64,
        requested_len: u64,
    ) -> Result<NonZeroU64, c_int> {
        let delta = requested_len
            .checked_sub(previous_len)
            .ok_or(libc::EINVAL)?;
        self.shm_funded
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |funded| {
                funded
                    .checked_add(delta)
                    .filter(|total| *total <= self.shm_limit.load(Ordering::Acquire))
            })
            .map_err(|_| libc::ENOSPC)?;
        let id = self.next_charge.fetch_add(1, Ordering::Relaxed);
        let mut reservations = self.shm_reservations.lock().expect("SHM reservation mutex");
        reservations.insert(id, (token, previous_len, requested_len));
        self.shm_growth_calls.fetch_add(1, Ordering::Relaxed);
        self.shm_growth_peak
            .fetch_max(reservations.len(), Ordering::Relaxed);
        NonZeroU64::new(id).ok_or(libc::EIO)
    }

    fn shm_growth_settled(&self, reservation: NonZeroU64, observed_len: u64) {
        let (token, previous, requested) = self
            .shm_reservations
            .lock()
            .expect("SHM reservation mutex")
            .remove(&reservation.get())
            .expect("live SHM reservation");
        let retained = observed_len.max(previous).min(requested.max(previous)) - previous;
        *self
            .shm_retained
            .lock()
            .expect("SHM retained mutex")
            .entry(token.generation().get())
            .or_default() += retained;
        self.shm_funded
            .fetch_sub(requested - previous - retained, Ordering::AcqRel);
    }

    fn reserve_file_mutation(
        &self,
        token: FdToken,
        kind: FileMutationKind,
        previous_len: u64,
        requested_len: u64,
    ) -> Result<NonZeroU64, c_int> {
        match kind {
            FileMutationKind::Write => {
                self.write_calls.fetch_add(1, Ordering::Relaxed);
                if self.deny_next_write.swap(false, Ordering::AcqRel) {
                    return Err(libc::ENOSPC);
                }
            }
            FileMutationKind::Truncate => {
                self.truncate_calls.fetch_add(1, Ordering::Relaxed);
            }
        }
        let delta = requested_len.saturating_sub(previous_len);
        self.file_funded
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |funded| {
                funded
                    .checked_add(delta)
                    .filter(|total| *total <= self.file_limit.load(Ordering::Acquire))
            })
            .map_err(|_| libc::ENOSPC)?;
        let id = self.next_charge.fetch_add(1, Ordering::Relaxed);
        self.file_reservations
            .lock()
            .expect("file reservations mutex")
            .insert(id, (token, previous_len, requested_len));
        NonZeroU64::new(id).ok_or(libc::EIO)
    }

    fn file_mutation_settled(
        &self,
        reservation: NonZeroU64,
        observed_len: u64,
        sqlite_rc: c_int,
    ) -> Result<(), c_int> {
        let (token, previous, requested) = self
            .file_reservations
            .lock()
            .expect("file reservations mutex")
            .remove(&reservation.get())
            .expect("live file reservation");
        let retained = observed_len.max(previous).min(requested.max(previous)) - previous;
        *self
            .file_retained
            .lock()
            .expect("file retained mutex")
            .entry(token.generation().get())
            .or_default() += retained;
        self.file_funded.fetch_sub(
            requested.saturating_sub(previous) - retained,
            Ordering::AcqRel,
        );
        self.file_settlements
            .lock()
            .expect("file settlements")
            .push((
                previous,
                observed_len,
                sqlite_rc,
                self.file_funded.load(Ordering::Acquire),
            ));
        if self.deny_next_file_settlement.swap(false, Ordering::AcqRel) {
            Err(libc::EIO)
        } else {
            Ok(())
        }
    }

    fn file_synced(
        &self,
        _token: FdToken,
        observed_len: Option<u64>,
        _sqlite_rc: c_int,
    ) -> Result<(), c_int> {
        self.sync_calls.fetch_add(1, Ordering::Relaxed);
        if observed_len.is_none() {
            return Err(libc::EIO);
        }
        if self.deny_next_sync.swap(false, Ordering::AcqRel) {
            return Err(libc::EIO);
        }
        Ok(())
    }

    fn reserve_delete(&self, path: &CStr) -> Result<NonZeroU64, c_int> {
        if !path.to_bytes().starts_with(&self.root_path) {
            return Err(libc::EACCES);
        }
        self.delete_calls.fetch_add(1, Ordering::Relaxed);
        if self.deny_next_delete.swap(false, Ordering::AcqRel) {
            return Err(libc::EACCES);
        }
        let id = self.next_charge.fetch_add(1, Ordering::Relaxed);
        self.delete_pending
            .lock()
            .expect("pending deletes")
            .insert(id, path.to_bytes().to_vec());
        NonZeroU64::new(id).ok_or(libc::EIO)
    }

    fn delete_settled(&self, reservation: NonZeroU64, sqlite_rc: c_int) -> Result<(), c_int> {
        let _path = self
            .delete_pending
            .lock()
            .expect("pending deletes")
            .remove(&reservation.get())
            .expect("live deletion reservation");
        if sqlite_rc != rusqlite::ffi::SQLITE_OK {
            self.delete_quarantined
                .lock()
                .expect("quarantined deletes")
                .push(reservation.get());
        }
        self.delete_results
            .lock()
            .expect("delete results mutex")
            .push(sqlite_rc);
        Ok(())
    }
}

#[test]
fn registry_unix_syscalls_bind_real_fds_to_product_ledger() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("upstream.sqlite");
    let canonical_root = directory.path().canonicalize()?;
    let authority = Arc::new(Authority {
        root_path: canonical_root.as_os_str().as_bytes().to_vec(),
        root_dir: File::open(directory.path())?,
        next_charge: AtomicU64::new(1),
        db_opens: AtomicUsize::new(0),
        wal_opens: AtomicUsize::new(0),
        shm_opens: AtomicUsize::new(0),
        refused: AtomicUsize::new(0),
        quarantined: AtomicUsize::new(0),
        deny_shm: AtomicBool::new(false),
        shm_limit: AtomicU64::new(u64::MAX),
        shm_funded: AtomicU64::new(0),
        shm_reservations: Mutex::new(BTreeMap::new()),
        shm_retained: Mutex::new(BTreeMap::new()),
        shm_growth_calls: AtomicUsize::new(0),
        shm_growth_peak: AtomicUsize::new(0),
        file_limit: AtomicU64::new(u64::MAX),
        file_funded: AtomicU64::new(0),
        file_reservations: Mutex::new(BTreeMap::new()),
        file_retained: Mutex::new(BTreeMap::new()),
        write_calls: AtomicUsize::new(0),
        truncate_calls: AtomicUsize::new(0),
        sync_calls: AtomicUsize::new(0),
        delete_calls: AtomicUsize::new(0),
        delete_results: Mutex::new(Vec::new()),
        delete_pending: Mutex::new(BTreeMap::new()),
        delete_quarantined: Mutex::new(Vec::new()),
        file_settlements: Mutex::new(Vec::new()),
        deny_next_write: AtomicBool::new(false),
        deny_next_file_settlement: AtomicBool::new(false),
        deny_next_sync: AtomicBool::new(false),
        deny_next_delete: AtomicBool::new(false),
        opened: Mutex::new(Vec::new()),
        retired: Mutex::new(Vec::new()),
    });
    let ledger = Arc::new(FdLedger::with_capacity(
        NonZeroUsize::new(128).ok_or("zero slots")?,
    )?);
    // SAFETY: This one-test binary owns all SQLite connections in its process.
    let permit = unsafe { ExclusiveProcessPermit::assume_exclusive_sqlite_process() };
    let hook = install(
        permit,
        Arc::clone(&ledger),
        Arc::clone(&authority) as Arc<dyn OpenAuthority>,
    )
    .map_err(|error| format!("registry Unix hook install: {error:?}"))?;
    // SAFETY: The same test process is still the only SQLite owner. A second
    // install must reject without replacing the active callback table.
    let repeated_permit = unsafe { ExclusiveProcessPermit::assume_exclusive_sqlite_process() };
    assert!(matches!(
        install(
            repeated_permit,
            Arc::clone(&ledger),
            Arc::clone(&authority) as Arc<dyn OpenAuthority>,
        ),
        Err(quanta_index_sqlite_unix_syscall::InstallError::AlreadyInstalled)
    ));
    let limits = OwnerLimits {
        max_files: NonZeroUsize::new(16).ok_or("zero owner slots")?,
    };
    let grant_bytes = InstalledHook::owner_vfs_backing_bytes(limits)
        .map_err(|error| format!("owner VFS backing: {error:?}"))?;
    let grant = Arc::new(SyntheticGrant(grant_bytes));
    let vfs = hook
        .register_owner_vfs(limits, Arc::clone(&grant) as Arc<dyn RetainedMemoryGrant>)
        .map_err(|error| format!("owner VFS registration: {error:?}"))?;
    assert_eq!(Arc::strong_count(&grant), 2);
    assert!(matches!(
        hook.register_owner_vfs(limits, Arc::new(SyntheticGrant(grant_bytes))),
        Err(quanta_index_sqlite_unix_syscall::InstallError::AlreadyInstalled)
    ));
    let open = |path: &Path| {
        Connection::open_with_flags_and_vfs(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            vfs,
        )
    };

    let outside = tempfile::tempdir()?;
    let unscoped = outside.path().join("unscoped.sqlite");
    assert!(open(&unscoped).is_err());
    assert!(!unscoped.exists());
    assert_eq!(hook.active_owner_files(), 0);
    let outside_target = outside.path().join("target.sqlite");
    std::fs::write(&outside_target, b"unchanged")?;
    let alias = directory.path().join("alias.sqlite");
    std::os::unix::fs::symlink(&outside_target, &alias)?;
    assert!(open(&alias).is_err());
    assert_eq!(hook.active_owner_files(), 0);
    let alias_name = CString::new(canonical_root.join("alias.sqlite").as_os_str().as_bytes())?;
    assert!(
        authority
            .open_admitted(alias_name.as_c_str(), libc::O_RDWR, 0)
            .is_err(),
    );
    assert_eq!(std::fs::read(&outside_target)?, b"unchanged");

    let writer = open(&database)?;
    writer.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; \
         CREATE TABLE gold(n INTEGER PRIMARY KEY); INSERT INTO gold VALUES(7);",
    )?;
    let reader = open(&database)?;
    reader.execute_batch("BEGIN")?;
    let row: i64 = reader.query_row("SELECT n FROM gold", [], |row| row.get(0))?;
    assert_eq!(row, 7);
    assert_eq!(hook.active_shm_owners(), 2);
    assert!(hook.active_shm_locks() > 0);
    writer.execute_batch("INSERT INTO gold VALUES(8)")?;
    let snapshot: i64 = reader.query_row("SELECT COUNT(*) FROM gold", [], |row| row.get(0))?;
    assert_eq!(snapshot, 1);
    assert_eq!(hook.active_shm_owners(), 2);
    drop(reader);
    assert_eq!(hook.active_shm_owners(), 1);
    drop(writer);
    assert_eq!(hook.active_shm_owners(), 0);
    assert_eq!(hook.active_shm_nodes(), 0);
    assert_eq!(hook.active_shm_locks(), 0);

    let failed_shm_db = directory.path().join("failed-shm.sqlite");
    let initial = open(&failed_shm_db)?;
    initial.execute_batch("CREATE TABLE gold(n INTEGER); PRAGMA journal_mode=WAL;")?;
    drop(initial);
    authority.deny_shm.store(true, Ordering::Relaxed);
    let denied = open(&failed_shm_db)?;
    assert!(denied.execute_batch("INSERT INTO gold VALUES(1)").is_err());
    drop(denied);
    assert_eq!(hook.active_shm_owners(), 0);
    assert_eq!(hook.active_shm_nodes(), 0);
    authority.deny_shm.store(false, Ordering::Relaxed);
    let retry = open(&failed_shm_db)?;
    retry.execute_batch("INSERT INTO gold VALUES(2)")?;
    drop(retry);
    assert_eq!(hook.active_shm_owners(), 0);
    assert_eq!(hook.active_shm_nodes(), 0);

    let capacity_db = directory.path().join("capacity-shm.sqlite");
    let initial = open(&capacity_db)?;
    initial.execute_batch("CREATE TABLE gold(n INTEGER); PRAGMA journal_mode=WAL;")?;
    drop(initial);
    let funded_before = authority.shm_funded.load(Ordering::Acquire);
    authority.shm_limit.store(
        funded_before.checked_add(3).ok_or("SHM limit overflow")?,
        Ordering::Release,
    );
    let denied = open(&capacity_db)?;
    let failed_write = denied.execute_batch("INSERT INTO gold VALUES(1)");
    let Err(rusqlite::Error::SqliteFailure(error, _)) = failed_write else {
        panic!("unfunded SHM growth must reach the SQLite caller");
    };
    assert_eq!(error.extended_code, rusqlite::ffi::SQLITE_IOERR_SHMSIZE);
    assert_eq!(
        std::fs::metadata(capacity_db.with_extension("sqlite-shm"))?.len(),
        3
    );
    assert_eq!(
        authority.shm_funded.load(Ordering::Acquire),
        funded_before + 3
    );
    assert!(
        authority
            .shm_reservations
            .lock()
            .expect("reservations")
            .is_empty()
    );
    drop(denied);
    assert_eq!(authority.shm_funded.load(Ordering::Acquire), funded_before);
    authority.shm_limit.store(u64::MAX, Ordering::Release);
    let retry = open(&capacity_db)?;
    retry.execute_batch("INSERT INTO gold VALUES(2)")?;
    drop(retry);
    assert_eq!(authority.shm_funded.load(Ordering::Acquire), funded_before);

    let concurrent_db = directory.path().join("concurrent-shm.sqlite");
    let initial = open(&concurrent_db)?;
    initial.execute_batch("CREATE TABLE gold(n INTEGER); PRAGMA journal_mode=WAL;")?;
    drop(initial);
    let growth_before = authority.shm_growth_calls.load(Ordering::Relaxed);
    let ready = Barrier::new(3);
    let release = Barrier::new(3);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let ready = &ready;
            let release = &release;
            let path = concurrent_db.clone();
            scope.spawn(move || {
                let connection = open(&path).expect("concurrent WAL connection");
                let count: i64 = connection
                    .query_row("SELECT COUNT(*) FROM gold", [], |row| row.get(0))
                    .expect("concurrent SHM map");
                assert_eq!(count, 0);
                ready.wait();
                release.wait();
            });
        }
        ready.wait();
        assert_eq!(hook.active_shm_nodes(), 1);
        assert_eq!(hook.active_shm_owners(), 2);
        release.wait();
    });
    assert!(authority.shm_growth_calls.load(Ordering::Relaxed) > growth_before);
    assert_eq!(authority.shm_growth_peak.load(Ordering::Relaxed), 1);
    assert_eq!(authority.shm_funded.load(Ordering::Acquire), funded_before);

    let failed_map_db = directory.path().join("failed-map.sqlite");
    let initial = open(&failed_map_db)?;
    initial.execute_batch("CREATE TABLE gold(n INTEGER); PRAGMA journal_mode=WAL;")?;
    drop(initial);
    let failed_map = open(&failed_map_db)?;
    failed_map.execute_batch("PRAGMA mmap_size=0")?;
    let shm_opens_before = authority.shm_opens.load(Ordering::Relaxed);
    let failed_write =
        with_one_mmap_refusal(|| failed_map.execute_batch("INSERT INTO gold VALUES(1)"))?;
    assert!(MMAP_REFUSED.load(Ordering::SeqCst));
    assert!(authority.shm_opens.load(Ordering::Relaxed) > shm_opens_before);
    let Err(rusqlite::Error::SqliteFailure(error, _)) = failed_write else {
        panic!("SHM mmap failure must reach the caller as a SQLite error");
    };
    assert_eq!(error.extended_code, rusqlite::ffi::SQLITE_IOERR_SHMMAP);
    drop(failed_map);
    assert_eq!(hook.active_shm_owners(), 0);
    assert_eq!(hook.active_shm_nodes(), 0);
    assert_eq!(hook.active_shm_locks(), 0);
    let retry = open(&failed_map_db)?;
    retry.execute_batch("INSERT INTO gold VALUES(2)")?;
    drop(retry);
    assert_eq!(hook.active_shm_owners(), 0);
    assert_eq!(hook.active_shm_nodes(), 0);

    assert!(authority.db_opens.load(Ordering::Relaxed) >= 2);
    assert!(authority.wal_opens.load(Ordering::Relaxed) >= 1);
    assert!(authority.shm_opens.load(Ordering::Relaxed) >= 1);

    let rollback_database = directory.path().join("rollback.sqlite");
    let owner = open(&rollback_database)?;
    owner.execute_batch("CREATE TABLE gold(n INTEGER); INSERT INTO gold VALUES(9);")?;
    let held_reader = open(&rollback_database)?;
    owner.execute_batch("BEGIN IMMEDIATE")?;
    let held_row: i64 = held_reader.query_row("SELECT n FROM gold", [], |row| row.get(0))?;
    assert_eq!(held_row, 9);
    let before_reader_drop = hook.physical_closes()?;
    drop(held_reader);
    assert_eq!(hook.physical_closes()?, before_reader_drop);
    owner.execute_batch("COMMIT")?;
    assert!(hook.physical_closes()? > before_reader_drop);
    drop(owner);
    assert_eq!(hook.active_owner_files(), 0);

    let mutation_db = directory.path().join("mutation.sqlite");
    let mutation = open(&mutation_db)?;
    mutation.execute_batch("PRAGMA journal_mode=TRUNCATE; CREATE TABLE gold(n INTEGER);")?;
    let writes_before = authority.write_calls.load(Ordering::Relaxed);
    let truncates_before = authority.truncate_calls.load(Ordering::Relaxed);
    let syncs_before = authority.sync_calls.load(Ordering::Relaxed);
    mutation.execute_batch("INSERT INTO gold VALUES(11)")?;
    assert!(authority.write_calls.load(Ordering::Relaxed) > writes_before);
    assert!(authority.truncate_calls.load(Ordering::Relaxed) > truncates_before);
    assert!(authority.sync_calls.load(Ordering::Relaxed) > syncs_before);
    let committed: i64 = mutation.query_row("SELECT COUNT(*) FROM gold", [], |row| row.get(0))?;
    assert_eq!(committed, 1);

    authority.deny_next_write.store(true, Ordering::Release);
    let before_refusal = std::fs::metadata(&mutation_db)?.len();
    let refused = mutation.execute_batch("INSERT INTO gold VALUES(12)");
    let Err(rusqlite::Error::SqliteFailure(error, _)) = refused else {
        panic!("pre-write admission refusal must reach SQLite");
    };
    assert_eq!(error.extended_code, rusqlite::ffi::SQLITE_FULL);
    let after_refusal = std::fs::metadata(&mutation_db)?.len();
    assert_eq!(after_refusal, before_refusal);
    let committed: i64 = mutation.query_row("SELECT COUNT(*) FROM gold", [], |row| row.get(0))?;
    assert_eq!(committed, 1);

    authority.deny_next_sync.store(true, Ordering::Release);
    let refused = mutation.execute_batch("INSERT INTO gold VALUES(13)");
    let Err(rusqlite::Error::SqliteFailure(error, _)) = refused else {
        panic!("post-sync settlement refusal must reach SQLite");
    };
    assert_eq!(error.extended_code, rusqlite::ffi::SQLITE_IOERR_FSYNC);
    drop(mutation);
    assert_eq!(hook.active_owner_files(), 0);
    assert!(
        authority
            .file_reservations
            .lock()
            .expect("file reservations")
            .is_empty()
    );
    assert_eq!(authority.file_funded.load(Ordering::Acquire), 0);

    let partial_db = canonical_root.join("partial-write.sqlite");
    let partial = open(&partial_db)?;
    partial.execute_batch("CREATE TABLE gold(n INTEGER);")?;
    let settlements_before = authority
        .file_settlements
        .lock()
        .expect("settlements")
        .len();
    let failed = with_one_partial_pwrite(|| partial.execute_batch("INSERT INTO gold VALUES(99)"))?;
    assert!(PARTIAL_PWRITE.load(Ordering::SeqCst) > 0);
    let Err(rusqlite::Error::SqliteFailure(error, _)) = failed else {
        panic!("partial lower pwrite error must reach SQLite");
    };
    assert_eq!(error.extended_code, rusqlite::ffi::SQLITE_IOERR_WRITE);
    assert!(
        authority
            .file_settlements
            .lock()
            .expect("settlements")
            .iter()
            .skip(settlements_before)
            .any(|(previous, observed, rc, funded)| {
                *rc != rusqlite::ffi::SQLITE_OK && observed > previous && *funded > 0
            })
    );
    drop(partial);
    assert_eq!(authority.file_funded.load(Ordering::Acquire), 0);

    let settlement_db = canonical_root.join("settlement.sqlite");
    let settlement = open(&settlement_db)?;
    settlement.execute_batch("CREATE TABLE gold(n INTEGER);")?;
    authority
        .deny_next_file_settlement
        .store(true, Ordering::Release);
    let refused = settlement.execute_batch("INSERT INTO gold VALUES(100)");
    let Err(rusqlite::Error::SqliteFailure(error, _)) = refused else {
        panic!("post-write settlement refusal must reach SQLite");
    };
    assert_eq!(error.extended_code, rusqlite::ffi::SQLITE_IOERR_WRITE);
    drop(settlement);
    assert_eq!(authority.file_funded.load(Ordering::Acquire), 0);

    let delete_path = canonical_root.join("delete-owner.tmp");
    std::fs::write(&delete_path, b"delete witness")?;
    let delete_name = CString::new(delete_path.as_os_str().as_bytes())?;
    // SAFETY: The named VFS remains registered for the full test process.
    let named_vfs = unsafe { rusqlite::ffi::sqlite3_vfs_find(vfs.as_ptr()) };
    assert!(!named_vfs.is_null());
    // SAFETY: The registered xDelete accepts this live VFS and C pathname.
    let delete = unsafe { (*named_vfs).xDelete.ok_or("missing owner xDelete")? };
    authority.deny_next_delete.store(true, Ordering::Release);
    // SAFETY: The pathname and VFS stay valid through each synchronous call.
    assert_eq!(
        unsafe { delete(named_vfs, delete_name.as_ptr(), 0) },
        rusqlite::ffi::SQLITE_IOERR_DELETE
    );
    assert!(delete_path.exists());
    // SAFETY: The pathname and VFS stay valid through each synchronous call.
    assert_eq!(
        unsafe { delete(named_vfs, delete_name.as_ptr(), 0) },
        rusqlite::ffi::SQLITE_OK
    );
    assert!(!delete_path.exists());
    // SAFETY: The pathname and VFS stay valid through each synchronous call.
    assert_ne!(
        unsafe { delete(named_vfs, delete_name.as_ptr(), 0) },
        rusqlite::ffi::SQLITE_OK
    );
    let results = authority.delete_results.lock().expect("delete results");
    assert_eq!(results[results.len() - 2], rusqlite::ffi::SQLITE_OK);
    assert_ne!(results[results.len() - 1], rusqlite::ffi::SQLITE_OK);
    drop(results);
    assert!(
        authority
            .delete_pending
            .lock()
            .expect("pending deletes")
            .is_empty()
    );
    assert!(
        !authority
            .delete_quarantined
            .lock()
            .expect("quarantined deletes")
            .is_empty()
    );
    assert!(authority.delete_calls.load(Ordering::Relaxed) >= 3);

    let mut held = Vec::new();
    for index in 0..limits.max_files.get() {
        held.push(open(
            &directory.path().join(format!("capacity-{index}.sqlite")),
        )?);
    }
    assert_eq!(hook.active_owner_files(), limits.max_files.get());
    let refused_path = directory.path().join("capacity-refused.sqlite");
    assert!(open(&refused_path).is_err());
    assert!(!refused_path.exists());
    assert_eq!(hook.active_owner_files(), limits.max_files.get());
    drop(held.pop());
    let recovered = open(&refused_path)?;
    assert_eq!(hook.active_owner_files(), limits.max_files.get());
    drop(recovered);
    drop(held);
    assert_eq!(hook.active_owner_files(), 0);

    let start = Barrier::new(9);
    let release = Barrier::new(9);
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        for index in 0..8 {
            let sender = sender.clone();
            let start = &start;
            let release = &release;
            let path = directory.path().join(format!("parallel-{index}.sqlite"));
            scope.spawn(move || {
                start.wait();
                let connection = open(&path);
                sender
                    .send(connection.is_ok())
                    .expect("parallel result receiver");
                release.wait();
                drop(connection);
            });
        }
        drop(sender);
        start.wait();
        let opened: Vec<_> = (0..8)
            .map(|_| receiver.recv().expect("parallel result"))
            .collect();
        let simultaneous = hook.active_owner_files();
        release.wait();
        assert_eq!(opened, vec![true; 8]);
        assert_eq!(simultaneous, 8);
    });
    assert_eq!(hook.active_owner_files(), 0);

    let tokens = authority.opened.lock().expect("open records").clone();
    assert!(!tokens.is_empty());
    let (shm_tokens, ordinary_tokens): (Vec<_>, Vec<_>) = tokens
        .iter()
        .partition(|(path, _token)| path.ends_with(b"-shm"));
    assert!(!ordinary_tokens.is_empty());
    assert!(!shm_tokens.is_empty());
    // The named VFS released xOpen and xShmMap owners at SQLite close/unmap.
    assert_eq!(
        authority.retired.lock().expect("retirement records").len(),
        tokens.len(),
    );
    for (_path, token) in &ordinary_tokens {
        assert!(hook.release_owner(*token).is_err());
    }
    for (_path, token) in &shm_tokens {
        assert!(hook.release_owner(*token).is_err());
    }
    let receipts = authority.retired.lock().expect("retirement records");
    assert_eq!(receipts.len(), tokens.len());
    assert!(receipts.iter().all(|receipt| receipt.root.get() == 1));
    assert!(receipts.iter().all(|receipt| receipt.identity.inode != 0));
    assert_eq!(
        receipts
            .iter()
            .map(|receipt| receipt.charge.get())
            .collect::<BTreeSet<_>>()
            .len(),
        receipts.len(),
    );
    drop(receipts);
    for (_path, token) in tokens {
        assert_eq!(ledger.current(token.fd())?, None);
        assert!(hook.release_owner(token).is_err());
    }
    assert_eq!(authority.quarantined.load(Ordering::Relaxed), 0);
    assert_eq!(authority.shm_funded.load(Ordering::Acquire), 0);
    Ok(())
}
