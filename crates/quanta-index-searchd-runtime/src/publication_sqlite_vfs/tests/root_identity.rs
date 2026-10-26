use super::*;
use std::os::unix::fs::MetadataExt as _;

fn pending_write(
    fixture: &Fixture,
    path: &Path,
) -> Result<(Arc<Adapter>, Event<'static>), Box<dyn std::error::Error>> {
    let adapter = {
        let scopes = fixture
            .vfs
            .router
            .scopes
            .lock()
            .map_err(|error| std::io::Error::other(format!("VFS scope lock: {error}")))?;
        Arc::clone(
            &scopes
                .get(&fixture.vfs.scope)
                .ok_or("missing VFS scope")?
                .adapter,
        )
    };
    let handle_id = {
        let state = adapter
            .state
            .lock()
            .map_err(|error| std::io::Error::other(format!("VFS handle lock: {error}")))?;
        state
            .handles
            .iter()
            .find(|(_id, handle)| handle.path == path && handle.mutation.is_some())
            .map(|(id, _handle)| *id)
            .ok_or("new SQLite file has no pending writer")?
    };
    Ok((
        adapter,
        Event {
            kind: EventKind::BeforeWrite,
            scope_id: fixture.vfs.scope,
            handle_id,
            path: None,
            open_flags: 0,
            sqlite_result: rusqlite::ffi::SQLITE_OK,
            operation: 0,
            offset: 0,
            amount: 2,
            target_size: 2,
            observed_size: None,
            witness: None,
            fd_retired: false,
            first_shm_fd: false,
            native_sync: quanta_capacity_vfs::NativeSyncObservation::default(),
        },
    ))
}

#[test]
fn pending_sqlite_write_refuses_replaced_root_then_same_connection_recovers() -> TestResult {
    let fixture = fixture(16 * 1024 * 1024)?;
    let _scope = fixture.vfs.scope();
    let path = fixture.root.join("root-identity.sqlite3");
    let connection = Connection::open_with_flags_and_vfs(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        fixture.vfs.name(),
    )?;
    let (adapter, before_write) = pending_write(&fixture, &path)?;
    // Keep a started ticket with an already reserved end. Repeated writes
    // must still authenticate the live root even when no growth/start is due.
    adapter
        .before_mutation(&before_write)
        .map_err(|error| format!("reserve pending write: {error:?}"))?;
    adapter
        .before_mutation(&before_write)
        .map_err(|error| format!("repeat pending write: {error:?}"))?;
    let original = fs::read(&path)?;
    let charged = fixture.authority.snapshot()?;
    let moved = fixture.directory.path().join("held-state-root");
    fs::rename(&fixture.root, &moved)?;
    fs::create_dir(&fixture.root)?;
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700))?;

    let direct = adapter.before_mutation(&before_write);
    let sql = connection.execute_batch("CREATE TABLE root_gold(v INTEGER);");
    let observed = (|| -> Result<_, Box<dyn std::error::Error>> {
        Ok((
            fs::read(moved.join("root-identity.sqlite3"))? == original,
            fs::read_dir(&fixture.root)?.next().is_none(),
            fixture.authority.snapshot()? == charged,
        ))
    })();
    // A violated refusal may create a file in this fixture-only replacement.
    // Preserve the observation above, then restore the held root before failing.
    fs::remove_dir_all(&fixture.root)?;
    fs::rename(&moved, &fixture.root)?;
    let (source_unchanged, replacement_empty, ledger_unchanged) = observed?;
    require(
        matches!(direct, Err(Refusal::Io)) && sql.is_err(),
        "replaced state root admitted a pending SQLite write",
    )?;
    require(
        source_unchanged && replacement_empty && ledger_unchanged,
        "replaced-root refusal changed source bytes, replacement root, or capacity",
    )?;

    connection.execute_batch("CREATE TABLE root_gold(v INTEGER);")?;
    let count: i64 =
        connection.query_row("SELECT count(*) FROM root_gold", [], |row| row.get(0))?;
    require(count == 0, "recovered connection has unexpected rows")?;
    drop(connection);
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn pending_sqlite_write_rechecks_held_lock_link_and_mode_then_recovers() -> TestResult {
    let fixture = fixture(16 * 1024 * 1024)?;
    let _scope = fixture.vfs.scope();
    let lock = fixture.root.join(".searchd-state-root.lock");
    let alias = fixture.root.join("held-lock-alias");
    for (name, hard_link) in [("lock-link.sqlite3", true), ("lock-mode.sqlite3", false)] {
        let path = fixture.root.join(name);
        let connection = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            fixture.vfs.name(),
        )?;
        let (adapter, before_write) = pending_write(&fixture, &path)?;
        let original = fs::read(&path)?;
        let charged = fixture.authority.snapshot()?;
        if hard_link {
            fs::hard_link(&lock, &alias)?;
            let held = fs::metadata(&lock)?;
            let linked = fs::metadata(&alias)?;
            require(
                held.dev() == linked.dev() && held.ino() == linked.ino() && held.nlink() == 2,
                "lock alias did not retain the held inode with two links",
            )?;
        } else {
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o640))?;
            require(
                fs::metadata(&lock)?.mode() & 0o7777 == 0o640,
                "lock mode change was not observed",
            )?;
        }

        let direct = adapter.before_mutation(&before_write);
        let sql = connection.execute_batch("CREATE TABLE lock_gold(v INTEGER);");
        let observed = (|| -> Result<_, Box<dyn std::error::Error>> {
            Ok((
                fs::read(&path)? == original,
                fixture.authority.snapshot()? == charged,
            ))
        })();
        if hard_link {
            fs::remove_file(&alias)?;
        } else {
            fs::set_permissions(&lock, fs::Permissions::from_mode(0o600))?;
        }
        let (source_unchanged, ledger_unchanged) = observed?;
        require(
            matches!(direct, Err(Refusal::Io)) && sql.is_err(),
            "post-acquire lock change admitted a pending SQLite write",
        )?;
        require(
            source_unchanged && ledger_unchanged,
            "lock refusal changed SQLite bytes or capacity",
        )?;

        connection.execute_batch("CREATE TABLE lock_gold(v INTEGER);")?;
        let count: i64 =
            connection.query_row("SELECT count(*) FROM lock_gold", [], |row| row.get(0))?;
        require(count == 0, "recovered connection has unexpected rows")?;
        drop(connection);
        fixture.vfs.drain_retirements()?;
    }
    Ok(())
}
