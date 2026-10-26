//! Service-owned disposable scratch with one shared logical disk reservation.
//!
//! A managed namespace is cleaned only while its own exclusive lock is held.
//! Standalone stores take shared leases and never sweep old sessions.

use std::{
    fs::{self, File, TryLockError},
    os::unix::{
        ffi::OsStrExt as _,
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use quanta_index_core::index::{
    IndexError, ScratchAllocation, ScratchIoBinding, ScratchNamespaceLease, ScratchRequest,
    ScratchSpacePort, WorkBudget, WorkUsage,
};
use rustix::fs::{AtFlags, Dir, Mode, OFlags};
use tempfile::TempDir;

const MAX_CLEANUP_ENTRIES: u64 = 1_000_000;

#[derive(Debug)]
struct SpaceInner {
    root: PathBuf,
    root_file: File,
    max_bytes: u64,
    reserved: Mutex<u64>,
    io_binding: Option<Arc<dyn ScratchIoBinding>>,
    _lock: NamespaceLock,
}

#[derive(Debug)]
enum NamespaceLock {
    Standalone {
        _file: File,
    },
    Managed {
        _lease: Box<dyn ScratchNamespaceLease>,
    },
}

/// A private scratch namespace.
///
/// Reservations are shared by every clone and retained allocation; they are
/// capacity promises, not physical disk quotas.
/// Independently opened standalone instances have separate reservation counters.
#[derive(Clone, Debug)]
pub struct FileScratchSpace {
    inner: Arc<SpaceInner>,
}

#[derive(Debug)]
struct FileScratchAllocation {
    path: PathBuf,
    directory: Option<TempDir>,
    max_bytes: u64,
    reserved: Mutex<u64>,
    inner: Arc<SpaceInner>,
}

impl FileScratchSpace {
    /// Opens a private standalone namespace under a shared lease without
    /// removing existing files. A managed cleanup cannot run concurrently.
    ///
    /// `max_bytes` limits this opened instance and its clones; another standalone
    /// opener does not share its admission counter. Use one shared instance when
    /// callers need one logical cap, or managed assembly for an exclusive service.
    pub fn open_standalone(root: impl AsRef<Path>, max_bytes: u64) -> Result<Self, IndexError> {
        validate_cap(max_bytes)?;
        let root = root.as_ref();
        let root_file = open_private_root(root)?;
        let lock = open_namespace_lock(&root_file)?;
        match lock.try_lock_shared() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(IndexError::ResourceLimited);
            }
            Err(TryLockError::Error(error)) => {
                return Err(IndexError::Storage(format!(
                    "lock standalone scratch namespace: {error}"
                )));
            }
        }
        Self::from_open_root(
            root,
            root_file,
            max_bytes,
            NamespaceLock::Standalone { _file: lock },
            None,
        )
    }

    /// Opens a service namespace, taking its own process-lifetime exclusive
    /// lock before removing orphaned allocations. The caller must already
    /// hold the enclosing state-root lease.
    pub fn open_managed(root: impl AsRef<Path>, max_bytes: u64) -> Result<Self, IndexError> {
        Self::open_managed_inner(root.as_ref(), max_bytes, None, None)
    }

    /// The same physical owner prepares the lease and retires allocation files.
    pub fn open_managed_with_io(
        root: impl AsRef<Path>,
        max_bytes: u64,
        io_binding: Arc<dyn ScratchIoBinding>,
    ) -> Result<Self, IndexError> {
        Self::open_managed_inner(root.as_ref(), max_bytes, Some(io_binding), None)
    }

    /// Open a managed namespace under the caller's original startup budget.
    /// The same mutable budget reaches each physical orphan retirement, where
    /// its owner charges work and checks cancellation between owned steps.
    pub fn open_managed_with_io_budgeted(
        root: impl AsRef<Path>,
        max_bytes: u64,
        io_binding: Arc<dyn ScratchIoBinding>,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        Self::open_managed_inner(root.as_ref(), max_bytes, Some(io_binding), Some(budget))
    }

    fn open_managed_inner(
        root: &Path,
        max_bytes: u64,
        io_binding: Option<Arc<dyn ScratchIoBinding>>,
        mut budget: Option<&mut WorkBudget>,
    ) -> Result<Self, IndexError> {
        if let Some(budget) = budget.as_deref_mut() {
            budget.checkpoint()?;
        }
        validate_cap(max_bytes)?;
        let (root_file, lock) = if let Some(binding) = &io_binding {
            let lease = binding.acquire_namespace(root)?;
            let root_file = open_private_root(root)?;
            let metadata = root_file.metadata().map_err(|error| {
                IndexError::Storage(format!("inspect admitted scratch namespace: {error}"))
            })?;
            if lease.namespace_identity()
                != (quanta_index_core::publication_capacity::InodeKey {
                    device: metadata.dev(),
                    inode: metadata.ino(),
                })
            {
                return Err(IndexError::Storage(
                    "scratch namespace changed after lock".into(),
                ));
            }
            (root_file, NamespaceLock::Managed { _lease: lease })
        } else {
            let root_file = open_private_root(root)?;
            let file = open_namespace_lock(&root_file)?;
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Err(IndexError::ResourceLimited),
                Err(TryLockError::Error(error)) => {
                    return Err(IndexError::Storage(format!(
                        "lock scratch namespace: {error}"
                    )));
                }
            }
            (root_file, NamespaceLock::Standalone { _file: file })
        };
        cleanup_orphans(&root_file, root, io_binding.as_deref(), budget)?;
        Self::from_open_root(root, root_file, max_bytes, lock, io_binding)
    }

    fn from_open_root(
        root: &Path,
        root_file: File,
        max_bytes: u64,
        lock: NamespaceLock,
        io_binding: Option<Arc<dyn ScratchIoBinding>>,
    ) -> Result<Self, IndexError> {
        let root = canonical_open_root(root, &root_file)?;
        Ok(Self {
            inner: Arc::new(SpaceInner {
                root,
                root_file,
                max_bytes,
                reserved: Mutex::new(0),
                io_binding,
                _lock: lock,
            }),
        })
    }
}

fn canonical_open_root(path: &Path, root_file: &File) -> Result<PathBuf, IndexError> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| IndexError::Storage(format!("canonicalize scratch root: {error}")))?;
    let linked = fs::symlink_metadata(&canonical)
        .map_err(|error| IndexError::Storage(format!("inspect canonical scratch root: {error}")))?;
    let opened = root_file
        .metadata()
        .map_err(|error| IndexError::Storage(format!("inspect held scratch root: {error}")))?;
    validate_private_metadata(&linked, opened.dev(), true)?;
    if linked.ino() != opened.ino() {
        return Err(IndexError::Storage(
            "scratch root changed during canonicalization".into(),
        ));
    }
    Ok(canonical)
}

fn validate_cap(max_bytes: u64) -> Result<(), IndexError> {
    if max_bytes == 0 {
        return Err(IndexError::InvalidInput(
            "scratch reservation cap must be positive".into(),
        ));
    }
    Ok(())
}

impl ScratchSpacePort for FileScratchSpace {
    fn allocate(
        &self,
        request: ScratchRequest,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
        if request.initial_bytes == 0 || request.initial_bytes > request.max_bytes {
            return Err(IndexError::InvalidInput(
                "scratch allocation requires a positive initial reservation within its bound"
                    .into(),
            ));
        }
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        validate_open_root(&self.inner)?;
        let root_device = self
            .inner
            .root_file
            .metadata()
            .map_err(|error| IndexError::Storage(format!("inspect scratch root: {error}")))?
            .dev();
        {
            let mut reserved = self.inner.reserved.lock().map_err(|_error| {
                IndexError::Storage("scratch reservation lock poisoned".into())
            })?;
            let next = reserved
                .checked_add(request.initial_bytes)
                .ok_or(IndexError::ResourceLimited)?;
            if next > self.inner.max_bytes {
                return Err(IndexError::ResourceLimited);
            }
            *reserved = next;
        }
        if let Err(error) = budget.checkpoint() {
            release_reservation(&self.inner, request.initial_bytes);
            return Err(error);
        }
        let directory = match tempfile::Builder::new()
            .prefix("alloc-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&self.inner.root)
        {
            Ok(directory) => directory,
            Err(error) => {
                release_reservation(&self.inner, request.initial_bytes);
                return Err(IndexError::Storage(format!(
                    "create scratch allocation: {error}"
                )));
            }
        };
        let path = directory.path().to_path_buf();
        if let Err(error) = validate_private_directory(&path, root_device)
            .and_then(|()| validate_open_root(&self.inner))
        {
            if remove_allocation(directory, self.inner.io_binding.as_deref()).is_ok() {
                release_reservation(&self.inner, request.initial_bytes);
            }
            return Err(error);
        }
        Ok(Box::new(FileScratchAllocation {
            path,
            directory: Some(directory),
            max_bytes: request.max_bytes,
            reserved: Mutex::new(request.initial_bytes),
            inner: Arc::clone(&self.inner),
        }))
    }
}

impl ScratchAllocation for FileScratchAllocation {
    fn path(&self) -> &Path {
        &self.path
    }

    fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn reserved_bytes(&self) -> Result<u64, IndexError> {
        self.reserved
            .lock()
            .map(|reserved| *reserved)
            .map_err(|_error| {
                IndexError::Storage("scratch allocation reservation lock poisoned".into())
            })
    }

    fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if bytes > self.max_bytes {
            return Err(IndexError::ResourceLimited);
        }
        let mut current = self.reserved.lock().map_err(|_error| {
            IndexError::Storage("scratch allocation reservation lock poisoned".into())
        })?;
        if bytes <= *current {
            return Ok(());
        }
        let delta = bytes
            .checked_sub(*current)
            .ok_or(IndexError::ResourceLimited)?;
        let mut shared =
            self.inner.reserved.lock().map_err(|_error| {
                IndexError::Storage("scratch reservation lock poisoned".into())
            })?;
        let next = shared
            .checked_add(delta)
            .ok_or(IndexError::ResourceLimited)?;
        if next > self.inner.max_bytes {
            return Err(IndexError::ResourceLimited);
        }
        budget.checkpoint()?;
        *shared = next;
        *current = bytes;
        drop(current);
        drop(shared);
        Ok(())
    }

    fn io_binding(&self) -> Option<Arc<dyn ScratchIoBinding>> {
        self.inner.io_binding.clone()
    }
}

impl Drop for FileScratchAllocation {
    fn drop(&mut self) {
        if let Some(directory) = self.directory.take() {
            // A failed close leaves the reservation held for this service
            // lifetime. The next exclusive managed open can reclaim it.
            let removed = remove_allocation(directory, self.inner.io_binding.as_deref());
            if removed.is_ok() {
                // Poison means the reservation cannot be proved; retain it conservatively.
                if let Ok(reserved) = self.reserved.lock() {
                    release_reservation(&self.inner, *reserved);
                }
            }
        }
    }
}

fn remove_allocation(
    directory: TempDir,
    binding: Option<&dyn ScratchIoBinding>,
) -> Result<(), IndexError> {
    match binding {
        Some(binding) => binding.retire_namespace(&directory.keep(), None),
        None => directory
            .close()
            .map_err(|error| IndexError::Storage(format!("remove scratch allocation: {error}"))),
    }
}

fn release_reservation(inner: &SpaceInner, bytes: u64) {
    if let Ok(mut reserved) = inner.reserved.lock()
        && let Some(next) = reserved.checked_sub(bytes)
    {
        *reserved = next;
    }
}

fn open_private_root(path: &Path) -> Result<File, IndexError> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(IndexError::Storage(format!("create scratch root: {error}"))),
    }
    let linked = fs::symlink_metadata(path)
        .map_err(|error| IndexError::Storage(format!("inspect scratch root: {error}")))?;
    validate_private_metadata(&linked, linked.dev(), true)?;
    let file = File::from(
        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| IndexError::Storage(format!("open scratch root: {error}")))?,
    );
    let opened = file
        .metadata()
        .map_err(|error| IndexError::Storage(format!("inspect open scratch root: {error}")))?;
    validate_private_metadata(&opened, linked.dev(), true)?;
    if opened.ino() != linked.ino() {
        return Err(IndexError::Storage(
            "scratch root changed during open".into(),
        ));
    }
    Ok(file)
}

fn validate_open_root(inner: &SpaceInner) -> Result<(), IndexError> {
    let linked = fs::symlink_metadata(&inner.root)
        .map_err(|error| IndexError::Storage(format!("inspect scratch root: {error}")))?;
    let opened = inner
        .root_file
        .metadata()
        .map_err(|error| IndexError::Storage(format!("inspect held scratch root: {error}")))?;
    validate_private_metadata(&linked, opened.dev(), true)?;
    if linked.ino() != opened.ino() {
        return Err(IndexError::Storage("scratch root identity changed".into()));
    }
    Ok(())
}

fn validate_private_directory(path: &Path, device: u64) -> Result<(), IndexError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| IndexError::Storage(format!("inspect scratch allocation: {error}")))?;
    validate_private_metadata(&metadata, device, true)
}

fn validate_private_metadata(
    metadata: &fs::Metadata,
    device: u64,
    directory: bool,
) -> Result<(), IndexError> {
    let expected_mode = if directory { 0o700 } else { 0o600 };
    let kind_matches = if directory {
        metadata.file_type().is_dir()
    } else {
        metadata.file_type().is_file() && metadata.nlink() == 1
    };
    if !kind_matches
        || metadata.dev() != device
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != expected_mode
    {
        return Err(IndexError::Storage(
            "scratch namespace has an unsafe owner, mode, type or device".into(),
        ));
    }
    Ok(())
}

fn open_namespace_lock(root: &File) -> Result<File, IndexError> {
    let lock = File::from(
        rustix::fs::openat(
            root,
            ".lease",
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|error| IndexError::Storage(format!("open scratch lease: {error}")))?,
    );
    let opened = lock
        .metadata()
        .map_err(|error| IndexError::Storage(format!("inspect scratch lease: {error}")))?;
    validate_private_metadata(
        &opened,
        root.metadata()
            .map_err(|error| IndexError::Storage(format!("inspect scratch root: {error}")))?
            .dev(),
        false,
    )?;
    let linked = rustix::fs::statat(root, ".lease", AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| IndexError::Storage(format!("inspect scratch lease name: {error}")))?;
    let held = rustix::fs::fstat(&lock)
        .map_err(|error| IndexError::Storage(format!("inspect held scratch lease: {error}")))?;
    if linked.st_dev != held.st_dev || linked.st_ino != held.st_ino {
        return Err(IndexError::Storage(
            "scratch lease changed during open".into(),
        ));
    }
    Ok(lock)
}

fn cleanup_orphans(
    root: &File,
    path: &Path,
    binding: Option<&dyn ScratchIoBinding>,
    mut budget: Option<&mut WorkBudget>,
) -> Result<(), IndexError> {
    let device = root
        .metadata()
        .map_err(|error| IndexError::Storage(format!("inspect scratch root: {error}")))?
        .dev();
    let mut entries = 0u64;
    loop {
        if let Some(budget) = budget.as_deref_mut() {
            budget.checkpoint()?;
        }
        let mut removed = false;
        let mut directory = Dir::read_from(root)
            .map_err(|error| IndexError::Storage(format!("read scratch namespace: {error}")))?;
        while let Some(entry) = directory.read() {
            let entry = entry
                .map_err(|error| IndexError::Storage(format!("read scratch entry: {error}")))?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." || name.to_bytes() == b".lease" {
                continue;
            }
            if let Some(budget) = budget.as_deref_mut() {
                budget.charge(WorkUsage {
                    reads: 1,
                    ..WorkUsage::default()
                })?;
            }
            if !name.to_bytes().starts_with(b"alloc-") {
                return Err(IndexError::Storage(
                    "unexpected entry in managed scratch namespace".into(),
                ));
            }
            charge_cleanup_entry(&mut entries)?;
            let child = File::from(
                rustix::fs::openat(
                    root,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|error| {
                    IndexError::Storage(format!("open stale scratch allocation: {error}"))
                })?,
            );
            validate_private_metadata(
                &child.metadata().map_err(|error| {
                    IndexError::Storage(format!("inspect stale scratch allocation: {error}"))
                })?,
                device,
                true,
            )?;
            if let Some(binding) = binding {
                let child_path = path.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
                // Physical deletion charges its own actual work. Pass the
                // original budget through; no duplicate namespace charge here.
                binding.retire_namespace(&child_path, budget.as_deref_mut())?;
                if let Some(budget) = budget.as_deref_mut() {
                    budget.checkpoint()?;
                }
            } else {
                cleanup_allocation(&child, device, &mut entries)?;
                rustix::fs::unlinkat(root, name, AtFlags::REMOVEDIR).map_err(|error| {
                    IndexError::Storage(format!("remove stale scratch allocation: {error}"))
                })?;
            }
            removed = true;
        }
        if !removed {
            return Ok(());
        }
    }
}

fn cleanup_allocation(allocation: &File, device: u64, entries: &mut u64) -> Result<(), IndexError> {
    loop {
        let mut removed = false;
        let mut directory = Dir::read_from(allocation).map_err(|error| {
            IndexError::Storage(format!("read stale scratch allocation: {error}"))
        })?;
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(|error| {
                IndexError::Storage(format!("read stale scratch file: {error}"))
            })?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            charge_cleanup_entry(entries)?;
            let file = File::from(
                rustix::fs::openat(
                    allocation,
                    name,
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|error| {
                    IndexError::Storage(format!("open stale scratch file: {error}"))
                })?,
            );
            let metadata = file.metadata().map_err(|error| {
                IndexError::Storage(format!("inspect stale scratch file: {error}"))
            })?;
            if !metadata.file_type().is_file()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.dev() != device
                || metadata.nlink() != 1
                || metadata.mode() & 0o022 != 0
            {
                return Err(IndexError::Storage("unsafe stale scratch file".into()));
            }
            rustix::fs::unlinkat(allocation, name, AtFlags::empty()).map_err(|error| {
                IndexError::Storage(format!("remove stale scratch file: {error}"))
            })?;
            removed = true;
        }
        if !removed {
            return Ok(());
        }
    }
}

fn charge_cleanup_entry(entries: &mut u64) -> Result<(), IndexError> {
    *entries = entries
        .checked_add(1)
        .ok_or(IndexError::MaintenanceRequired)?;
    if *entries > MAX_CLEANUP_ENTRIES {
        return Err(IndexError::MaintenanceRequired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{self, BufRead as _, Write as _},
        os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
        path::PathBuf,
        process::{Child, Command, Stdio},
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use quanta_index_core::index::WorkLimits;

    use super::*;

    fn assertion_error(detail: String) -> Box<dyn std::error::Error> {
        std::io::Error::other(detail).into()
    }

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(assertion_error(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(assertion_error(format!($($message)+)));
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(assertion_error(format!(
                    "assertion failed: left {left:?} != right {right:?}"
                )));
            }
        }};
        ($left:expr, $right:expr, $($message:tt)+) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(assertion_error(format!(
                    "assertion failed: left {left:?} != right {right:?}: {}",
                    format_args!($($message)+)
                )));
            }
        }};
    }

    const CHILD_ROOT_ENV: &str = "QIDX_SCRATCH_PROCESS_TEST_ROOT";
    const CHILD_MODE_ENV: &str = "QIDX_SCRATCH_PROCESS_TEST_MODE";
    const CHILD_READY: &str = "QIDX_SCRATCH_PROCESS_READY";

    struct ChildGuard {
        child: Option<Child>,
    }

    impl ChildGuard {
        fn kill_and_wait(&mut self) -> io::Result<()> {
            let Some(child) = self.child.as_mut() else {
                return Ok(());
            };
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            let _status = child.wait()?;
            let _finished = self.child.take();
            Ok(())
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _killed = child.kill();
                let _waited = child.wait();
            }
        }
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 10,
                writes: 10,
                bytes: 100_000,
                mutations: 10,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    fn run_killed_holder_case(mode: &str) -> Result<(), Box<dyn std::error::Error>> {
        let test_root = tempfile::tempdir()?;
        let namespace = test_root.path().join("scratch");
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--ignored")
            .arg("--exact")
            .arg("scratch::tests::scratch_process_holder")
            .arg("--nocapture")
            .env(CHILD_ROOT_ENV, &namespace)
            .env(CHILD_MODE_ENV, mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("scratch child stdout is unavailable"))?;
        let mut guard = ChildGuard { child: Some(child) };
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let mut lines = io::BufReader::new(stdout);
            let mut line = String::new();
            let mut result = Ok(false);
            for _line_number in 0..32 {
                line.clear();
                match lines.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_count) if line.contains(CHILD_READY) => {
                        result = Ok(true);
                        break;
                    }
                    Ok(_count) => {}
                    Err(error) => {
                        result = Err(error);
                        break;
                    }
                }
            }
            let _sent = ready_tx.send(result);
        });
        let ready = ready_rx.recv_timeout(Duration::from_secs(20));
        if !matches!(ready, Ok(Ok(true))) {
            guard.kill_and_wait()?;
            reader
                .join()
                .map_err(|_panic| io::Error::other("scratch child reader panicked"))?;
            return Err(io::Error::other("scratch child did not signal readiness").into());
        }
        reader
            .join()
            .map_err(|_panic| io::Error::other("scratch child reader panicked"))?;

        let mut orphan: Option<PathBuf> = None;
        for entry in fs::read_dir(&namespace)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with("alloc-")
                && orphan.replace(entry.path()).is_some()
            {
                return Err(io::Error::other("scratch child created multiple allocations").into());
            }
        }
        let orphan =
            orphan.ok_or_else(|| io::Error::other("scratch child allocation is missing"))?;
        check!(orphan.join("scratch.sqlite").is_file());
        check!(matches!(
            FileScratchSpace::open_managed(&namespace, 100),
            Err(IndexError::ResourceLimited)
        ));
        check!(orphan.is_dir());
        guard.kill_and_wait()?;
        let reopened = FileScratchSpace::open_managed(&namespace, 100)?;
        check!(!orphan.exists());
        drop(reopened);
        Ok(())
    }

    #[test]
    fn managed_recovery_uses_process_lease_not_pid_or_age() -> Result<(), Box<dyn std::error::Error>>
    {
        run_killed_holder_case("managed")?;
        run_killed_holder_case("standalone")?;
        Ok(())
    }

    #[test]
    #[ignore = "subprocess fixture; invoked by exact name from managed_recovery_uses_process_lease_not_pid_or_age"]
    fn scratch_process_holder() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::var_os(CHILD_ROOT_ENV)
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("scratch subprocess root was not supplied"))?;
        let mode = std::env::var(CHILD_MODE_ENV)?;
        let space = match mode.as_str() {
            "managed" => FileScratchSpace::open_managed(&root, 100)?,
            "standalone" => FileScratchSpace::open_standalone(&root, 100)?,
            _ => return Err(io::Error::other("unknown scratch subprocess mode").into()),
        };
        let allocation = space.allocate(
            ScratchRequest {
                initial_bytes: 50,
                max_bytes: 50,
            },
            &mut budget()?,
        )?;
        let file = allocation.path().join("scratch.sqlite");
        fs::write(&file, b"unpublished scratch")?;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
        writeln!(io::stdout(), "{CHILD_READY}")?;
        io::stdout().flush()?;
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    fn reservations_are_shared_and_released_after_directory_cleanup()
    -> Result<(), Box<dyn std::error::Error>> {
        let test_root = tempfile::tempdir()?;
        let space = FileScratchSpace::open_standalone(test_root.path().join("scratch"), 100)?;
        let clone = space.clone();
        let first = space.allocate(
            ScratchRequest {
                initial_bytes: 60,
                max_bytes: 60,
            },
            &mut budget()?,
        )?;
        let first_path = first.path().to_path_buf();
        check!(matches!(
            clone.allocate(
                ScratchRequest {
                    initial_bytes: 41,
                    max_bytes: 41
                },
                &mut budget()?
            ),
            Err(IndexError::ResourceLimited)
        ));
        let second = clone.allocate(
            ScratchRequest {
                initial_bytes: 40,
                max_bytes: 40,
            },
            &mut budget()?,
        )?;
        check!(first_path.is_dir());
        drop(first);
        check!(!first_path.exists());
        let third = clone.allocate(
            ScratchRequest {
                initial_bytes: 60,
                max_bytes: 60,
            },
            &mut budget()?,
        )?;
        drop(second);
        drop(third);
        Ok(())
    }

    #[test]
    fn growth_is_shared_atomic_and_released_with_namespace()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let space = FileScratchSpace::open_standalone(root.path().join("scratch"), 100)?;
        let first = space.allocate(
            ScratchRequest {
                initial_bytes: 20,
                max_bytes: 100,
            },
            &mut budget()?,
        )?;
        let second = space.allocate(
            ScratchRequest {
                initial_bytes: 30,
                max_bytes: 100,
            },
            &mut budget()?,
        )?;
        first.reserve_at_least(70, &mut budget()?)?;
        check_eq!(first.reserved_bytes()?, 70);
        check!(matches!(
            second.reserve_at_least(31, &mut budget()?),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(second.reserved_bytes()?, 30);
        check!(matches!(
            first.reserve_at_least(101, &mut budget()?),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(first.reserved_bytes()?, 70);
        let path = first.path().to_path_buf();
        drop(first);
        check!(!path.exists());
        second.reserve_at_least(100, &mut budget()?)?;
        check_eq!(second.reserved_bytes()?, 100);
        Ok(())
    }

    #[test]
    fn cancelled_growth_preserves_shared_reservation() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let space = FileScratchSpace::open_standalone(root.path().join("scratch"), 100)?;
        let first = space.allocate(
            ScratchRequest {
                initial_bytes: 20,
                max_bytes: 100,
            },
            &mut budget()?,
        )?;
        let mut cancelled = budget()?;
        cancelled.request().cancel_handle().cancel();
        check!(matches!(
            first.reserve_at_least(100, &mut cancelled),
            Err(IndexError::Cancelled)
        ));
        check_eq!(first.reserved_bytes()?, 20);
        let second = space.allocate(
            ScratchRequest {
                initial_bytes: 80,
                max_bytes: 80,
            },
            &mut budget()?,
        )?;
        check_eq!(second.reserved_bytes()?, 80);
        Ok(())
    }

    #[test]
    fn main_database_grows_before_writes_and_remains_inside_reservation()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let space =
            FileScratchSpace::open_standalone(root.path().join("scratch"), 2 * 1024 * 1024)?;
        let allocation = space.allocate(
            ScratchRequest {
                initial_bytes: 256 * 1024,
                max_bytes: 2 * 1024 * 1024,
            },
            &mut budget()?,
        )?;
        let db = rusqlite::Connection::open(allocation.path().join("growth.sqlite"))?;
        quanta_index_scratch_sqlite::configure(&db, allocation.as_ref())?;
        db.execute_batch(
            "PRAGMA journal_mode=OFF; CREATE TABLE rows(k INTEGER PRIMARY KEY, v BLOB NOT NULL);",
        )?;
        let payload = vec![7u8; 8192];
        for key in 0..100 {
            quanta_index_scratch_sqlite::reserve_for_write(
                &db,
                allocation.as_ref(),
                8192,
                &mut budget()?,
            )?;
            let inserted = db.execute(
                "INSERT INTO rows(k,v) VALUES(?1,?2)",
                rusqlite::params![key, &payload],
            )?;
            check_eq!(inserted, 1);
            let pages: u32 = db.query_row("PRAGMA main.max_page_count", [], |row| row.get(0))?;
            check!(
                u64::from(pages)
                    .checked_mul(4096)
                    .ok_or_else(|| io::Error::other("page cap overflow"))?
                    <= allocation.reserved_bytes()?
            );
        }
        check!(allocation.reserved_bytes()? > 256 * 1024);
        let rows: u32 = db.query_row("SELECT count(*) FROM rows", [], |row| row.get(0))?;
        check_eq!(rows, 100);
        Ok(())
    }

    #[test]
    fn allocation_directory_is_owner_only_in_both_open_modes()
    -> Result<(), Box<dyn std::error::Error>> {
        let test_root = tempfile::tempdir()?;
        let standalone =
            FileScratchSpace::open_standalone(test_root.path().join("standalone"), 100)?;
        let standalone_allocation = standalone.allocate(
            ScratchRequest {
                initial_bytes: 50,
                max_bytes: 50,
            },
            &mut budget()?,
        )?;
        let standalone_metadata = fs::symlink_metadata(standalone_allocation.path())?;
        check!(standalone_metadata.file_type().is_dir());
        check_eq!(standalone_metadata.permissions().mode() & 0o7777, 0o700);
        drop(standalone_allocation);
        drop(standalone);

        let managed = FileScratchSpace::open_managed(test_root.path().join("managed"), 100)?;
        let managed_allocation = managed.allocate(
            ScratchRequest {
                initial_bytes: 50,
                max_bytes: 50,
            },
            &mut budget()?,
        )?;
        let managed_metadata = fs::symlink_metadata(managed_allocation.path())?;
        check!(managed_metadata.file_type().is_dir());
        check_eq!(managed_metadata.permissions().mode() & 0o7777, 0o700);
        Ok(())
    }

    #[test]
    fn managed_open_requires_exclusive_lease_and_removes_orphans()
    -> Result<(), Box<dyn std::error::Error>> {
        let test_root = tempfile::tempdir()?;
        let namespace = test_root.path().join("scratch");
        let managed = FileScratchSpace::open_managed(&namespace, 100)?;
        let allocation = managed.allocate(
            ScratchRequest {
                initial_bytes: 50,
                max_bytes: 50,
            },
            &mut budget()?,
        )?;
        let live_path = allocation.path().to_path_buf();
        check!(matches!(
            FileScratchSpace::open_managed(&namespace, 100),
            Err(IndexError::ResourceLimited)
        ));
        check!(live_path.is_dir());
        drop(allocation);
        drop(managed);

        let stale = namespace.join("alloc-orphan");
        fs::DirBuilder::new().mode(0o700).create(&stale)?;
        let stale_file = stale.join("scratch.sqlite");
        fs::write(&stale_file, b"aborted transaction")?;
        fs::set_permissions(&stale_file, fs::Permissions::from_mode(0o600))?;
        let reopened = FileScratchSpace::open_managed(&namespace, 100)?;
        check!(!stale.exists());
        let next = reopened.allocate(
            ScratchRequest {
                initial_bytes: 100,
                max_bytes: 100,
            },
            &mut budget()?,
        )?;
        check!(next.path().is_dir());
        drop(next);
        Ok(())
    }

    #[test]
    fn standalone_readers_prevent_managed_sweep_of_live_allocation()
    -> Result<(), Box<dyn std::error::Error>> {
        let test_root = tempfile::tempdir()?;
        let namespace = test_root.path().join("scratch");
        let first = FileScratchSpace::open_standalone(&namespace, 100)?;
        let second = FileScratchSpace::open_standalone(&namespace, 100)?;
        let allocation = first.allocate(
            ScratchRequest {
                initial_bytes: 60,
                max_bytes: 60,
            },
            &mut budget()?,
        )?;
        let live_path = allocation.path().to_path_buf();
        check!(matches!(
            FileScratchSpace::open_managed(&namespace, 100),
            Err(IndexError::ResourceLimited)
        ));
        check!(live_path.is_dir());
        drop(first);
        drop(second);
        check!(matches!(
            FileScratchSpace::open_managed(&namespace, 100),
            Err(IndexError::ResourceLimited)
        ));
        drop(allocation);
        let managed = FileScratchSpace::open_managed(&namespace, 100)?;
        check!(!live_path.exists());
        drop(managed);
        Ok(())
    }

    #[test]
    fn managed_cleanup_refuses_symlinks_without_touching_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let test_root = tempfile::tempdir()?;
        let namespace = test_root.path().join("scratch");
        let managed = FileScratchSpace::open_managed(&namespace, 100)?;
        drop(managed);
        let sentinel = test_root.path().join("sentinel");
        fs::write(&sentinel, b"retain")?;
        symlink(&sentinel, namespace.join("alloc-link"))?;
        check!(matches!(
            FileScratchSpace::open_managed(&namespace, 100),
            Err(IndexError::Storage(_))
        ));
        check_eq!(fs::read(sentinel)?, b"retain");
        Ok(())
    }

    #[test]
    fn budgeted_managed_open_cancels_before_acquire_or_next_orphan()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Debug)]
        struct Lease(quanta_index_core::publication_capacity::InodeKey);
        impl ScratchNamespaceLease for Lease {
            fn namespace_identity(&self) -> quanta_index_core::publication_capacity::InodeKey {
                self.0
            }
        }

        #[derive(Debug)]
        struct CancelIo {
            cancel: quanta_index_core::CancelHandleV1,
            acquired: Arc<AtomicUsize>,
            retired: Arc<AtomicUsize>,
        }
        impl ScratchIoBinding for CancelIo {
            fn driver_name(&self) -> &'static str {
                "budgeted-cleanup-fixture"
            }

            fn enter_scope(&self) -> Box<dyn quanta_index_core::index::ScratchIoScope> {
                struct Scope;
                impl quanta_index_core::index::ScratchIoScope for Scope {
                    fn is_capacity_refusal(&self, _: i32) -> bool {
                        false
                    }
                }
                Box::new(Scope)
            }

            fn acquire_namespace(
                &self,
                path: &Path,
            ) -> Result<Box<dyn ScratchNamespaceLease>, IndexError> {
                let _previous = self.acquired.fetch_add(1, Ordering::SeqCst);
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(path)
                    .or_else(|error| {
                        if error.kind() == io::ErrorKind::AlreadyExists {
                            Ok(())
                        } else {
                            Err(error)
                        }
                    })
                    .map_err(|error| IndexError::Storage(format!("fixture namespace: {error}")))?;
                let metadata = fs::metadata(path)
                    .map_err(|error| IndexError::Storage(format!("fixture metadata: {error}")))?;
                Ok(Box::new(Lease(
                    quanta_index_core::publication_capacity::InodeKey {
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    },
                )))
            }

            fn retire_namespace(
                &self,
                path: &Path,
                budget: Option<&mut WorkBudget>,
            ) -> Result<(), IndexError> {
                let budget = budget.ok_or_else(|| {
                    IndexError::BackendViolation("budgeted cleanup lost its original budget".into())
                })?;
                budget.charge(WorkUsage {
                    writes: 1,
                    mutations: 1,
                    ..WorkUsage::default()
                })?;
                fs::remove_dir(path)
                    .map_err(|error| IndexError::Storage(format!("fixture retirement: {error}")))?;
                if self.retired.fetch_add(1, Ordering::SeqCst) == 0 {
                    self.cancel.cancel();
                    // This must observe cancellation inside the binding,
                    // before the outer namespace scan resumes.
                    budget.checkpoint()?;
                }
                Ok(())
            }
        }

        let work = || {
            let request = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
            let cancel = request.cancel_handle();
            (
                WorkBudget::with_request(
                    WorkLimits {
                        reads: 100,
                        writes: 100,
                        bytes: 100_000,
                        mutations: 100,
                    },
                    request,
                ),
                cancel,
            )
        };
        let directory = tempfile::tempdir()?;
        let absent = directory.path().join("absent");
        let (mut cancelled, handle) = work();
        let acquired = Arc::new(AtomicUsize::new(0));
        let retired = Arc::new(AtomicUsize::new(0));
        let io = Arc::new(CancelIo {
            cancel: handle.clone(),
            acquired: Arc::clone(&acquired),
            retired: Arc::clone(&retired),
        });
        handle.cancel();
        check!(matches!(
            FileScratchSpace::open_managed_with_io_budgeted(&absent, 100, io, &mut cancelled,),
            Err(IndexError::Cancelled)
        ));
        check!(!absent.exists());
        check_eq!(acquired.load(Ordering::SeqCst), 0);

        let root = directory.path().join("scratch");
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        for name in ["alloc-one", "alloc-two"] {
            fs::DirBuilder::new().mode(0o700).create(root.join(name))?;
        }
        let (mut first, handle) = work();
        let io = Arc::new(CancelIo {
            cancel: handle,
            acquired: Arc::clone(&acquired),
            retired: Arc::clone(&retired),
        });
        check!(matches!(
            FileScratchSpace::open_managed_with_io_budgeted(&root, 100, io.clone(), &mut first,),
            Err(IndexError::Cancelled)
        ));
        check_eq!(retired.load(Ordering::SeqCst), 1);
        check_eq!(first.usage().writes, 1);
        check_eq!(first.usage().mutations, 1);
        check_eq!(
            ["alloc-one", "alloc-two"]
                .iter()
                .filter(|name| root.join(name).exists())
                .count(),
            1
        );
        let (mut restart, _handle) = work();
        let reopened =
            FileScratchSpace::open_managed_with_io_budgeted(&root, 100, io, &mut restart)?;
        check_eq!(retired.load(Ordering::SeqCst), 2);
        check_eq!(restart.usage().writes, 1);
        check_eq!(restart.usage().mutations, 1);
        check!(!root.join("alloc-one").exists());
        check!(!root.join("alloc-two").exists());
        drop(reopened);
        Ok(())
    }

    #[test]
    fn wrong_namespace_lease_refuses_before_orphan_cleanup()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        #[derive(Debug)]
        struct WrongIo(Arc<AtomicUsize>);
        #[derive(Debug)]
        struct WrongLease(Arc<AtomicUsize>);
        impl ScratchNamespaceLease for WrongLease {
            fn namespace_identity(&self) -> quanta_index_core::publication_capacity::InodeKey {
                quanta_index_core::publication_capacity::InodeKey {
                    device: 0,
                    inode: 0,
                }
            }
        }
        impl Drop for WrongLease {
            fn drop(&mut self) {
                let _previous = self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        impl ScratchIoBinding for WrongIo {
            fn driver_name(&self) -> &'static str {
                "unused-wrong-namespace-fixture"
            }
            fn enter_scope(&self) -> Box<dyn quanta_index_core::index::ScratchIoScope> {
                struct Scope;
                impl quanta_index_core::index::ScratchIoScope for Scope {
                    fn is_capacity_refusal(&self, _: i32) -> bool {
                        false
                    }
                }
                Box::new(Scope)
            }
            fn acquire_namespace(
                &self,
                _: &Path,
            ) -> Result<Box<dyn ScratchNamespaceLease>, IndexError> {
                Ok(Box::new(WrongLease(Arc::clone(&self.0))))
            }
            fn retire_namespace(
                &self,
                _: &Path,
                _budget: Option<&mut WorkBudget>,
            ) -> Result<(), IndexError> {
                Err(IndexError::BackendViolation(
                    "invalid lease reached cleanup".into(),
                ))
            }
        }
        let directory = tempfile::tempdir()?;
        let root = directory.path().join("scratch");
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        let orphan = root.join("alloc-retained");
        fs::DirBuilder::new().mode(0o700).create(&orphan)?;
        fs::write(orphan.join("payload"), b"independent retained golden")?;
        let retired = Arc::new(AtomicUsize::new(0));
        check!(matches!(
            FileScratchSpace::open_managed_with_io(
                &root,
                100,
                Arc::new(WrongIo(Arc::clone(&retired)))
            ),
            Err(IndexError::Storage(_))
        ));
        check_eq!(
            fs::read(orphan.join("payload"))?,
            b"independent retained golden"
        );
        check_eq!(retired.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn physical_cleanup_refusal_never_uses_tempdir_fallback_or_refunds_reservation()
    -> Result<(), Box<dyn std::error::Error>> {
        #[derive(Debug)]
        struct RefusingIo;
        struct Scope;
        impl quanta_index_core::index::ScratchIoScope for Scope {
            fn is_capacity_refusal(&self, _: i32) -> bool {
                false
            }
        }
        impl ScratchIoBinding for RefusingIo {
            fn driver_name(&self) -> &'static str {
                "unused-fixture-driver"
            }
            fn enter_scope(&self) -> Box<dyn quanta_index_core::index::ScratchIoScope> {
                Box::new(Scope)
            }
            fn acquire_namespace(
                &self,
                path: &Path,
            ) -> Result<Box<dyn ScratchNamespaceLease>, IndexError> {
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(path)
                    .map_err(|error| IndexError::Storage(format!("fixture namespace: {error}")))?;
                let metadata = fs::metadata(path)
                    .map_err(|error| IndexError::Storage(format!("fixture identity: {error}")))?;
                Ok(Box::new(FixtureLease(
                    quanta_index_core::publication_capacity::InodeKey {
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    },
                )))
            }
            fn retire_namespace(
                &self,
                _: &Path,
                _budget: Option<&mut WorkBudget>,
            ) -> Result<(), IndexError> {
                Err(IndexError::ResourceLimited)
            }
        }
        #[derive(Debug)]
        struct FixtureLease(quanta_index_core::publication_capacity::InodeKey);
        impl ScratchNamespaceLease for FixtureLease {
            fn namespace_identity(&self) -> quanta_index_core::publication_capacity::InodeKey {
                self.0
            }
        }
        let directory = tempfile::tempdir()?;
        let space = FileScratchSpace::open_managed_with_io(
            directory.path().join("scratch"),
            100,
            Arc::new(RefusingIo),
        )?;
        let allocation = space.allocate(
            ScratchRequest {
                initial_bytes: 40,
                max_bytes: 80,
            },
            &mut budget()?,
        )?;
        let path = allocation.path().to_path_buf();
        fs::write(path.join("retained"), b"fixed retained bytes")?;
        check!(allocation.io_binding().is_some());
        drop(allocation);
        check_eq!(fs::read(path.join("retained"))?, b"fixed retained bytes");
        check_eq!(
            *space
                .inner
                .reserved
                .lock()
                .map_err(|_error| assertion_error("reservation lock".into()))?,
            40
        );
        Ok(())
    }
}
