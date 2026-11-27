use std::{
    fs::{self, File},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use quanta_index_sqlite_unix_syscall::{
    ProtectedFileIdentity, ProtectedReadError, ProtectedReadExpectation, ProtectedReadLimits,
    with_protected_read,
};
use rusqlite::{Connection, ErrorCode};

type TestResult = Result<(), Box<dyn std::error::Error>>;
const LIMITS: ProtectedReadLimits = ProtectedReadLimits {
    main_bytes: 4 * 1024 * 1024,
    wal_bytes: 4 * 1024 * 1024,
    expected: None,
};

fn require(ok: bool, message: &str) -> TestResult {
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

fn private_dir() -> std::io::Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
}

fn witness(path: &Path) -> std::io::Result<ProtectedFileIdentity> {
    let metadata = fs::metadata(path)?;
    Ok(ProtectedFileIdentity {
        dev: metadata.dev(),
        ino: metadata.ino(),
        bytes: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

// Independently capture a durable main+WAL image with committed data present
// only in the WAL. A separate directory avoids inheriting the writer's SHM.
fn leave_wal_image(source: &Path, target: &Path) -> TestResult {
    let writer = Connection::open(source.join("catalog.sqlite"))?;
    writer.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; \
        CREATE TABLE gold(id INTEGER PRIMARY KEY, bytes BLOB); \
        INSERT INTO gold VALUES(19, zeroblob(524288));",
    )?;
    for name in ["catalog.sqlite", "catalog.sqlite-wal"] {
        let path = target.join(name);
        fs::copy(source.join(name), &path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[test]
fn pinned_vfs_reads_committed_wal_without_shm_or_mutation() -> TestResult {
    let source = private_dir()?;
    let target = private_dir()?;
    leave_wal_image(source.path(), target.path())?;
    let database = target.path().join("catalog.sqlite");
    let wal = target.path().join("catalog.sqlite-wal");
    let main_before = fs::read(&database)?;
    let wal_before = fs::read(&wal)?;
    require(wal_before.len() > 524288, "fixture WAL is too small")?;
    let directory = File::open(target.path())?;

    let result = with_protected_read::<_, ProtectedReadError>(
        &directory,
        c"catalog.sqlite",
        LIMITS,
        Arc::new(|| true),
        |read| {
            let row = read.query_row("SELECT id, length(bytes) FROM gold", [], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })?;
            read.execute_batch("PRAGMA query_only=OFF;")?;
            assert_eq!(
                read.execute_batch("DELETE FROM gold")
                    .unwrap_err()
                    .sqlite_error_code(),
                Some(ErrorCode::ReadOnly),
            );
            let competing = Connection::open(&database)?;
            competing.busy_timeout(std::time::Duration::ZERO)?;
            let error = competing
                .query_row("SELECT id FROM gold", [], |row| row.get::<_, i64>(0))
                .unwrap_err();
            assert_eq!(error.sqlite_error_code(), Some(ErrorCode::DatabaseBusy));
            Ok(row)
        },
    )
    .map_err(|error| std::io::Error::other(format!("protected read: {error:?}")))?;
    require(result == (19, 524288), "committed WAL row differs")?;
    require(fs::read(&database)? == main_before, "main DB changed")?;
    require(fs::read(&wal)? == wal_before, "WAL changed")?;
    require(
        !target.path().join("catalog.sqlite-shm").exists(),
        "reader created SHM",
    )?;
    let reopened = Connection::open(database)?;
    reopened.busy_timeout(std::time::Duration::ZERO)?;
    let row: i64 = reopened.query_row("SELECT id FROM gold", [], |row| row.get(0))?;
    require(row == 19, "source remained locked after checked close")?;
    Ok(())
}

#[test]
fn pinned_vfs_refuses_contention_limits_and_replacement() -> TestResult {
    let source = private_dir()?;
    let target = private_dir()?;
    leave_wal_image(source.path(), target.path())?;
    let directory = File::open(target.path())?;
    let database = target.path().join("catalog.sqlite");
    let reader = Connection::open(database)?;
    reader.execute_batch("BEGIN; SELECT id FROM gold;")?;
    let read_empty = |_: &mut Connection| Ok::<_, ProtectedReadError>(());
    assert!(matches!(
        with_protected_read(
            &directory,
            c"catalog.sqlite",
            LIMITS,
            Arc::new(|| true),
            read_empty
        ),
        Err(ProtectedReadError::Contended)
    ));
    reader.close().map_err(|(_reader, error)| error)?;

    // Generate fresh images because a stock connection can checkpoint on close.
    let replacement_source = private_dir()?;
    let replacement = private_dir()?;
    leave_wal_image(replacement_source.path(), replacement.path())?;
    let directory = File::open(replacement.path())?;
    let main = witness(&replacement.path().join("catalog.sqlite"))?;
    let wal = witness(&replacement.path().join("catalog.sqlite-wal"))?;
    for expected in [
        ProtectedReadExpectation { main, wal: None },
        ProtectedReadExpectation {
            main: ProtectedFileIdentity {
                ino: main.ino.wrapping_add(1),
                ..main
            },
            wal: Some(wal),
        },
    ] {
        let limits = ProtectedReadLimits {
            expected: Some(expected),
            ..LIMITS
        };
        let result = with_protected_read::<(), ProtectedReadError>(
            &directory,
            c"catalog.sqlite",
            limits,
            Arc::new(|| true),
            |_| panic!("unexpected input reached SQLite inspection"),
        );
        assert!(matches!(result, Err(ProtectedReadError::Changed)));
    }
    let limited = ProtectedReadLimits {
        wal_bytes: 1,
        ..LIMITS
    };
    assert!(matches!(
        with_protected_read(
            &directory,
            c"catalog.sqlite",
            limited,
            Arc::new(|| true),
            read_empty
        ),
        Err(ProtectedReadError::ByteLimit)
    ));
    let cancel = Arc::new(AtomicBool::new(false));
    let check = Arc::clone(&cancel);
    let stopped = with_protected_read::<_, ProtectedReadError>(
        &directory,
        c"catalog.sqlite",
        LIMITS,
        Arc::new(move || !check.load(Ordering::Relaxed)),
        |read| {
            cancel.store(true, Ordering::Relaxed);
            read.query_row("SELECT id FROM gold", [], |row| row.get::<_, i64>(0))
                .map_err(Into::into)
        },
    );
    assert!(matches!(stopped, Err(ProtectedReadError::Cancelled)));
    let wal = replacement.path().join("catalog.sqlite-wal");
    let moved = replacement.path().join("displaced-wal");
    let result = with_protected_read::<_, ProtectedReadError>(
        &directory,
        c"catalog.sqlite",
        LIMITS,
        Arc::new(|| true),
        |read| {
            fs::rename(&wal, &moved)?;
            fs::write(&wal, b"unrelated bytes")?;
            let row = read.query_row("SELECT id FROM gold", [], |row| row.get::<_, i64>(0))?;
            assert_eq!(row, 19, "SQLite re-opened the replacement WAL path");
            Ok(())
        },
    );
    assert!(matches!(result, Err(ProtectedReadError::Changed)));
    Ok(())
}

#[test]
fn pinned_vfs_supports_main_and_refuses_rollback_journal() -> TestResult {
    let query = |read: &mut Connection| -> Result<i64, ProtectedReadError> {
        Ok(read.query_row("SELECT id FROM gold", [], |row| row.get(0))?)
    };
    for mode in ["DELETE", "WAL"] {
        let target = private_dir()?;
        let database = target.path().join("catalog.sqlite");
        let writer = Connection::open(&database)?;
        writer.execute_batch(&format!(
            "PRAGMA journal_mode={mode}; CREATE TABLE gold(id); INSERT INTO gold VALUES(71);"
        ))?;
        writer.close().map_err(|(_writer, error)| error)?;
        fs::set_permissions(database, fs::Permissions::from_mode(0o600))?;
        require(
            !target.path().join("catalog.sqlite-wal").exists(),
            "fixture left a WAL",
        )?;
        let directory = File::open(target.path())?;
        let result = with_protected_read(
            &directory,
            c"catalog.sqlite",
            LIMITS,
            Arc::new(|| true),
            query,
        );
        require(
            matches!(result, Ok(71)),
            &format!("{mode} main-only snapshot differs: {result:?}"),
        )?;
        let incomplete = target.path().join("incomplete.sqlite");
        fs::write(&incomplete, [0_u8; 99])?;
        fs::set_permissions(incomplete, fs::Permissions::from_mode(0o600))?;
        assert!(matches!(
            with_protected_read(
                &directory,
                c"incomplete.sqlite",
                LIMITS,
                Arc::new(|| true),
                query
            ),
            Err(ProtectedReadError::TruncatedMain),
        ));
        fs::write(target.path().join("catalog.sqlite-journal"), b"journal")?;
        assert!(matches!(
            with_protected_read(
                &directory,
                c"catalog.sqlite",
                LIMITS,
                Arc::new(|| true),
                query
            ),
            Err(ProtectedReadError::RollbackJournal)
        ));
    }
    Ok(())
}
