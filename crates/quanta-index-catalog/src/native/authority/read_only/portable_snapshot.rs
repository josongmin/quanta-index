//! macOS offline Catalog reader: copy held Main/WAL descriptors into one
//! private directory, then inspect that copy with the stock SQLite VFS.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::{Mutex, TryLockError},
};

use quanta_index_core::index::{IndexError, MemoryLease, WorkBudget};
use rusqlite::{Connection, OpenFlags};
use rustix::fs::{AtFlags, FlockOperation, Mode, OFlags, fcntl_lock, openat, statat};

use super::{CATALOG_FILE_NAME, CatalogReadOnlyIoAdmission, FileIdentity, engine, sql};

pub(super) enum Outcome<T, E, F> {
    Declined(F, Option<MemoryLease>),
    Complete(Result<T, E>),
}

pub(super) fn inspect_if_selected<T, E, F>(
    io: &dyn CatalogReadOnlyIoAdmission,
    path: &Path,
    main: FileIdentity,
    wal: Option<FileIdentity>,
    budget: &mut WorkBudget,
    map_close: fn(IndexError) -> E,
    pending: (F, Option<MemoryLease>),
) -> Result<Outcome<T, E, F>, E>
where
    E: From<IndexError>,
    F: FnOnce(&mut Connection, &Path, &mut WorkBudget, Option<MemoryLease>) -> Result<T, E>,
{
    let directory = path.parent().ok_or_else(|| protocol_error())?;
    let request = budget.request().clone();
    let mut pending = Some(pending);
    let mut completed = None;
    let selected =
        io.with_protected_catalog_directory_v1(directory, &request, wal.is_some(), &mut |held| {
            let Some((inspect, allocation)) = pending.take() else {
                completed = Some(Err(protocol_error().into()));
                return;
            };
            completed = Some(inspect_copy(
                held, path, main, wal, budget, map_close, inspect, allocation,
            ));
        })?;
    match (selected, pending, completed) {
        (true, None, Some(result)) => Ok(Outcome::Complete(result)),
        (false, Some((inspect, allocation)), None) => Ok(Outcome::Declined(inspect, allocation)),
        _ => Err(protocol_error().into()),
    }
}

fn protocol_error() -> IndexError {
    IndexError::BackendViolation(
        "protected Catalog directory callback violated its contract".into(),
    )
}

fn changed() -> IndexError {
    IndexError::Storage("Catalog inputs changed during verification".into())
}

fn storage(operation: &str, error: impl std::fmt::Display) -> IndexError {
    IndexError::Storage(format!("{operation}: {error}"))
}

fn inspect_copy<T, E, F>(
    held: &File,
    path: &Path,
    main: FileIdentity,
    wal: Option<FileIdentity>,
    budget: &mut WorkBudget,
    map_close: fn(IndexError) -> E,
    inspect: F,
    allocation: Option<MemoryLease>,
) -> Result<T, E>
where
    E: From<IndexError>,
    F: FnOnce(&mut Connection, &Path, &mut WorkBudget, Option<MemoryLease>) -> Result<T, E>,
{
    // Darwin POSIX record locks are process-owned. A second selected reader
    // could close the same inode and release this reader's lock otherwise.
    static SOURCE_SERIAL: Mutex<()> = Mutex::new(());
    let _serial = SOURCE_SERIAL.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => IndexError::Conflict,
        TryLockError::Poisoned(_) => protocol_error(),
    })?;
    budget.checkpoint()?;
    let directory = held
        .metadata()
        .map_err(|error| storage("stat held Catalog directory", error))?;
    if !directory.is_dir()
        || directory.uid() != rustix::process::geteuid().as_raw()
        || directory.mode() & 0o022 != 0
        || directory.dev() != main.device
        || wal.is_some_and(|wal| wal.device != main.device)
    {
        return Err(storage(
            "inspect held Catalog directory",
            "unsafe directory or cross-device input",
        )
        .into());
    }
    let source = open_source(held, CATALOG_FILE_NAME, main, OFlags::RDWR)?;
    // A whole-file POSIX lock covers SQLite's reserved byte range, including
    // bytes beyond EOF. Offline startup guarantees that this process has no
    // other connection to the old Catalog: POSIX locks are process-associated.
    lock_main(&source)?;
    if entry_exists(held, &format!("{CATALOG_FILE_NAME}-journal"))? {
        return Err(storage("inspect Catalog input", "rollback journal is present").into());
    }
    let wal_name = format!("{CATALOG_FILE_NAME}-wal");
    let wal_source = wal
        .map(|identity| open_source(held, &wal_name, identity, OFlags::RDONLY))
        .transpose()?;
    if wal_source.is_none() && entry_exists(held, &wal_name)? {
        return Err(changed().into());
    }
    let temp_parent = std::env::temp_dir()
        .canonicalize()
        .map_err(|error| storage("resolve private Catalog snapshot parent", error))?;
    let source_root = path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(protocol_error)?
        .canonicalize()
        .map_err(|error| storage("resolve old Catalog root", error))?;
    if temp_parent.starts_with(&source_root) {
        return Err(IndexError::BackendViolation(
            "private Catalog snapshot parent belongs to the old root".into(),
        )
        .into());
    }
    let scratch = tempfile::Builder::new()
        .prefix("quanta-catalog-offline-")
        .tempdir_in(temp_parent)
        .map_err(|error| storage("create private Catalog snapshot", error))?;
    let copy = scratch.path().join(CATALOG_FILE_NAME);
    copy_file(&source, &copy, main.bytes, budget)?;
    if let (Some(source), Some(identity)) = (&wal_source, wal) {
        copy_file(
            source,
            &scratch.path().join(&wal_name),
            identity.bytes,
            budget,
        )?;
    }
    verify_source(held, CATALOG_FILE_NAME, &source, main)?;
    if let (Some(source), Some(identity)) = (&wal_source, wal) {
        verify_source(held, &wal_name, source, identity)?;
    } else if entry_exists(held, &wal_name)? {
        return Err(changed().into());
    }
    if entry_exists(held, &format!("{CATALOG_FILE_NAME}-journal"))? {
        return Err(changed().into());
    }
    budget.checkpoint()?;
    let mut connection = Connection::open_with_flags(
        &copy,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
    )
    .map_err(|error| engine(&error))?;
    let result = inspect(&mut connection, path, budget, allocation);
    let closed = connection
        .close()
        .map_err(|(_connection, error)| map_close(sql(&error)));
    budget.checkpoint()?;
    verify_source(held, CATALOG_FILE_NAME, &source, main)?;
    if let (Some(source), Some(identity)) = (&wal_source, wal) {
        verify_source(held, &wal_name, source, identity)?;
    } else if entry_exists(held, &wal_name)? {
        return Err(changed().into());
    }
    if entry_exists(held, &format!("{CATALOG_FILE_NAME}-journal"))? {
        return Err(changed().into());
    }
    let value = result?;
    closed?;
    Ok(value)
}

fn open_source(
    held: &File,
    name: &str,
    expected: FileIdentity,
    access: OFlags,
) -> Result<File, IndexError> {
    let file = File::from(
        openat(
            held,
            name,
            access | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|error| storage("open held Catalog input", error))?,
    );
    verify_source(held, name, &file, expected)?;
    Ok(file)
}

fn identity(file: &File) -> Result<FileIdentity, IndexError> {
    let metadata = file
        .metadata()
        .map_err(|error| storage("stat held Catalog input", error))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(storage(
            "inspect Catalog input",
            "input is not a private, owned, single-link regular file",
        ));
    }
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        bytes: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanos: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanos: metadata.ctime_nsec(),
    })
}

fn verify_source(
    held: &File,
    name: &str,
    file: &File,
    expected: FileIdentity,
) -> Result<(), IndexError> {
    if identity(file)? != expected {
        return Err(changed());
    }
    let linked = statat(held, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| storage("stat linked Catalog input", error))?;
    if u64::try_from(linked.st_dev).ok() != Some(expected.device) || linked.st_ino != expected.inode
    {
        return Err(changed());
    }
    Ok(())
}

fn entry_exists(held: &File, name: &str) -> Result<bool, IndexError> {
    match statat(held, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(error) => Err(storage("stat Catalog sidecar", error)),
    }
}

fn copy_file(
    source: &File,
    destination: &Path,
    bytes: u64,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .map_err(|error| storage("create private Catalog input", error))?;
    let mut input = source;
    let mut remaining = bytes;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining != 0 {
        budget.checkpoint()?;
        let count = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = input
            .read(&mut buffer[..count])
            .map_err(|error| storage("read held Catalog input", error))?;
        if read == 0 {
            return Err(changed());
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| storage("write private Catalog input", error))?;
        remaining -= u64::try_from(read).map_err(|_error| IndexError::ResourceLimited)?;
    }
    Ok(())
}

fn lock_main(file: &File) -> Result<(), IndexError> {
    if let Err(error) = fcntl_lock(file, FlockOperation::NonBlockingLockExclusive) {
        return if matches!(error, rustix::io::Errno::ACCESS | rustix::io::Errno::AGAIN) {
            Err(IndexError::Conflict)
        } else {
            Err(storage("lock held Catalog input", error))
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::{RequestBudgetV1, index::WorkLimits};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process::{Child, Command},
        thread,
        time::{Duration, Instant},
    };

    struct SqlitePeer(Child, PathBuf);

    impl Drop for SqlitePeer {
        fn drop(&mut self) {
            drop(fs::write(&self.1, b"release"));
            drop(self.0.kill());
            drop(self.0.wait());
        }
    }

    struct HeldDirectory(PathBuf);

    impl CatalogReadOnlyIoAdmission for HeldDirectory {
        fn with_protected_catalog_directory_v1(
            &self,
            directory: &Path,
            _request: &RequestBudgetV1,
            _needs_wal: bool,
            visit: &mut dyn FnMut(&File),
        ) -> Result<bool, IndexError> {
            assert_eq!(directory, self.0);
            let held =
                File::open(directory).map_err(|error| storage("open test directory", error))?;
            visit(&held);
            Ok(true)
        }
    }

    #[test]
    fn stock_reader_replays_copied_wal_without_writing_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let live = tempfile::tempdir()?;
        let original = live.path().join(CATALOG_FILE_NAME);
        let writer = Connection::open(&original)?;
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE offline_value (n INTEGER NOT NULL); INSERT INTO offline_value VALUES (42);")?;
        let offline = tempfile::tempdir()?;
        let catalog_directory = offline.path().join("catalog");
        fs::create_dir(&catalog_directory)?;
        let copy = catalog_directory.join(CATALOG_FILE_NAME);
        let wal_name = format!("{CATALOG_FILE_NAME}-wal");
        let wal_path = catalog_directory.join(&wal_name);
        let copied_main = fs::copy(&original, &copy)?;
        let copied_wal = fs::copy(original.with_file_name(&wal_name), &wal_path)?;
        assert_eq!(copied_main, fs::metadata(&copy)?.len());
        assert_eq!(copied_wal, fs::metadata(&wal_path)?.len());
        writer.close().map_err(|(_, error)| error)?;
        for input in [&copy, &wal_path] {
            fs::set_permissions(input, fs::Permissions::from_mode(0o600))?;
        }
        let main = super::super::input(&copy, false)?.ok_or("missing main")?;
        let wal = super::super::input(&wal_path, false)?.ok_or("missing WAL")?;
        assert!(wal.bytes > 0);
        let original_main = fs::read(&copy)?;
        let original_wal = fs::read(&wal_path)?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100_000,
                writes: 0,
                bytes: 32 * 1024 * 1024,
                mutations: 0,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        let result = inspect_if_selected(
            &HeldDirectory(catalog_directory.clone()),
            &copy,
            main,
            Some(wal),
            &mut budget,
            |error| error,
            (
                |connection: &mut Connection, _, _, _| {
                    connection
                        .query_row("SELECT n FROM offline_value", [], |row| {
                            row.get::<_, i64>(0)
                        })
                        .map_err(|error| sql(&error))
                },
                None,
            ),
        )?;
        match result {
            Outcome::Complete(value) => assert_eq!(value?, 42),
            Outcome::Declined(_, _) => return Err("protected route declined".into()),
        }
        assert_eq!(fs::read(&copy)?, original_main);
        assert_eq!(fs::read(&wal_path)?, original_wal);
        assert!(
            !catalog_directory
                .join(format!("{CATALOG_FILE_NAME}-shm"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn held_input_rejects_replaced_name() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join(CATALOG_FILE_NAME);
        fs::write(&path, b"old")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let held = File::open(root.path())?;
        let expected = super::super::input(&path, false)?.ok_or("missing input")?;
        let source = open_source(&held, CATALOG_FILE_NAME, expected, OFlags::RDWR)?;
        fs::rename(&path, root.path().join("retired"))?;
        fs::write(&path, b"new")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        assert!(verify_source(&held, CATALOG_FILE_NAME, &source, expected).is_err());
        Ok(())
    }

    #[test]
    fn sqlite_peer_lock_child() -> Result<(), Box<dyn std::error::Error>> {
        let Ok(directory) = std::env::var("QIDX_SQLITE_PEER_DIRECTORY") else {
            return Ok(());
        };
        let mode = std::env::var("QIDX_SQLITE_PEER_MODE")?;
        let directory = PathBuf::from(directory);
        let connection = Connection::open(directory.join(CATALOG_FILE_NAME))?;
        match mode.as_str() {
            "reader" => {
                connection.execute_batch("BEGIN; SELECT n FROM offline_value;")?;
            }
            "writer" => {
                connection.execute_batch("BEGIN IMMEDIATE;")?;
            }
            "wal-writer" => {
                connection.execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE;")?;
            }
            _ => return Err("unexpected SQLite peer mode".into()),
        }
        fs::write(directory.join("ready"), b"ready")?;
        let release = directory.join("release");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !release.exists() {
            if Instant::now() >= deadline {
                return Err("SQLite peer release timed out".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        connection.execute_batch("ROLLBACK;")?;
        Ok(())
    }

    #[test]
    fn live_sqlite_peer_blocks_protected_copy() -> Result<(), Box<dyn std::error::Error>> {
        for mode in ["reader", "writer", "wal-writer"] {
            let root = tempfile::tempdir()?;
            let catalog = root.path().join(CATALOG_FILE_NAME);
            let connection = Connection::open(&catalog)?;
            connection.execute_batch(
                "CREATE TABLE offline_value (n INTEGER); INSERT INTO offline_value VALUES (42);",
            )?;
            connection.close().map_err(|(_, error)| error)?;
            fs::set_permissions(&catalog, fs::Permissions::from_mode(0o600))?;
            let child = Command::new(std::env::current_exe()?)
                .arg("sqlite_peer_lock_child")
                .env("QIDX_SQLITE_PEER_DIRECTORY", root.path())
                .env("QIDX_SQLITE_PEER_MODE", mode)
                .spawn()?;
            let mut peer = SqlitePeer(child, root.path().join("release"));
            let deadline = Instant::now() + Duration::from_secs(15);
            while !root.path().join("ready").exists() {
                if let Some(status) = peer.0.try_wait()? {
                    return Err(format!("SQLite {mode} peer exited before lock: {status}").into());
                }
                if Instant::now() >= deadline {
                    return Err(format!("SQLite {mode} peer did not take a lock").into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            let main = super::super::input(&catalog, false)?.ok_or("missing main")?;
            let wal_path = root.path().join(format!("{CATALOG_FILE_NAME}-wal"));
            let wal = super::super::input(&wal_path, true)?;
            let mut budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 100_000,
                    writes: 0,
                    bytes: 32 * 1024 * 1024,
                    mutations: 0,
                },
                RequestBudgetV1::for_duration(Duration::from_secs(15)),
            );
            let result = inspect_if_selected(
                &HeldDirectory(root.path().to_path_buf()),
                &catalog,
                main,
                wal,
                &mut budget,
                |error| error,
                (
                    (|_connection: &mut Connection, _, _, _| Ok::<_, IndexError>(())),
                    None,
                ),
            )?;
            assert!(
                matches!(result, Outcome::Complete(Err(IndexError::Conflict))),
                "{mode} peer was not refused"
            );
            fs::write(&peer.1, b"release")?;
            assert!(peer.0.wait()?.success(), "{mode} peer failed");
        }
        Ok(())
    }
}
