use super::*;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::Instant;

const CHILD_MODE: &str = "QIDX_NATIVE_RETIREMENT_RACE_CHILD";
const MAIN_SCOPE: u64 = 1;
const OTHER_SCOPE: u64 = 2;
const WAIT: Duration = Duration::from_secs(10);
const CHILD_WAIT: Duration = Duration::from_secs(45);
const LOG_PREFIX_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mode {
    ImmediateMain,
    ImmediateShm,
    DeferredMain,
    RejectedOpenCleanup,
    FailedMainCallback,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::ImmediateMain => "immediate-main",
            Self::ImmediateShm => "immediate-shm",
            Self::DeferredMain => "deferred-main",
            Self::RejectedOpenCleanup => "rejected-open-cleanup",
            Self::FailedMainCallback => "failed-main-callback",
        }
    }

    fn event(self) -> EventKind {
        match self {
            Self::ImmediateMain
            | Self::DeferredMain
            | Self::RejectedOpenCleanup
            | Self::FailedMainCallback => EventKind::AfterClose,
            Self::ImmediateShm => EventKind::AfterShmUnmap,
        }
    }

    fn retirement_operation(self) -> i32 {
        i32::from(self == Self::ImmediateShm)
    }
}

#[derive(Default)]
struct State {
    armed: bool,
    entered: bool,
    released: bool,
    open_rejected: bool,
    target_handle: Option<u64>,
    opening_witness: Option<FileWitness>,
    shm_witness: Option<FileWitness>,
    immediate_witness: Option<FileWitness>,
    immediate_fd_retired: Option<bool>,
    retirement_count: usize,
    retirement_witness: Option<FileWitness>,
}

struct Gate {
    mode: Mode,
    state: Mutex<State>,
    changed: Condvar,
}

impl Gate {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        })
    }

    fn arm(&self) -> TestResult {
        let mut state = self
            .state
            .lock()
            .map_err(|_error| std::io::Error::other("retirement gate poisoned"))?;
        if self.mode != Mode::RejectedOpenCleanup {
            require(state.target_handle.is_some(), "main FD was not opened")?;
        }
        state.armed = true;
        Ok(())
    }

    fn wait_entered(&self) -> Result<(bool, Option<FileWitness>), Box<dyn std::error::Error>> {
        let state = self
            .state
            .lock()
            .map_err(|_error| std::io::Error::other("retirement gate poisoned"))?;
        let (state, _timeout) = self
            .changed
            .wait_timeout_while(state, WAIT, |state| !state.entered)
            .map_err(|_error| std::io::Error::other("retirement wait poisoned"))?;
        if !state.entered {
            return Err(std::io::Error::other("native close callback did not enter").into());
        }
        Ok((
            state
                .immediate_fd_retired
                .ok_or("missing close retirement snapshot")?,
            state.immediate_witness,
        ))
    }

    fn release(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.released = true;
            self.changed.notify_all();
        }
    }

    fn snapshot(
        &self,
    ) -> Result<(usize, Option<FileWitness>, Option<FileWitness>), Box<dyn std::error::Error>> {
        let state = self
            .state
            .lock()
            .map_err(|_error| std::io::Error::other("retirement gate poisoned"))?;
        Ok((
            state.retirement_count,
            if self.mode == Mode::ImmediateShm {
                state.shm_witness
            } else {
                state.opening_witness
            },
            state.retirement_witness,
        ))
    }
}

struct GateAdmission(Arc<Gate>);

impl Admission for GateAdmission {
    fn on_event(&self, event: Event<'_>) -> Result<Option<CString>, Refusal> {
        if event.scope_id.get() != MAIN_SCOPE {
            return Ok(None);
        }
        let mut state = self.0.state.lock().map_err(|_error| Refusal::Io)?;
        if event.kind == EventKind::AfterOpen
            && event.open_flags & rusqlite::ffi::SQLITE_OPEN_MAIN_DB != 0
            && event.sqlite_result == rusqlite::ffi::SQLITE_OK
        {
            state.target_handle = Some(event.handle_id);
            state.opening_witness = event.witness;
            if self.0.mode == Mode::RejectedOpenCleanup && state.armed && !state.open_rejected {
                state.open_rejected = true;
                return Err(Refusal::CannotOpen);
            }
        }
        if event.kind == EventKind::AfterShmMap
            && state.target_handle == Some(event.handle_id)
            && event.sqlite_result == rusqlite::ffi::SQLITE_OK
            && event.witness.is_some()
        {
            state.shm_witness = event.witness;
        }
        if event.kind == EventKind::FdRetired
            && state.target_handle == Some(event.handle_id)
            && event.operation == self.0.mode.retirement_operation()
        {
            state.retirement_count += 1;
            state.retirement_witness = event.witness;
        }
        if state.armed
            && event.kind == self.0.mode.event()
            && state.target_handle == Some(event.handle_id)
            && !state.entered
        {
            state.entered = true;
            state.immediate_fd_retired = Some(event.fd_retired);
            state.immediate_witness = event.witness;
            self.0.changed.notify_all();
            let (returned, _timeout) = self
                .0
                .changed
                .wait_timeout_while(state, WAIT, |state| !state.released)
                .map_err(|_error| Refusal::Io)?;
            if !returned.released {
                return Err(Refusal::Io);
            }
            if self.0.mode == Mode::FailedMainCallback {
                return Err(Refusal::Io);
            }
        }
        Ok(None)
    }
}

fn register_gate(
    mode: Mode,
) -> Result<(&'static RegisteredVfs, Arc<Gate>), Box<dyn std::error::Error>> {
    let gate = Gate::new(mode);
    let vfs = quanta_capacity_vfs::register(
        CString::new("qidx-native-retirement-race")?,
        32,
        Box::new(GateAdmission(Arc::clone(&gate))),
    )
    .map_err(|error| std::io::Error::other(format!("register test VFS: {error:?}")))?;
    Ok((vfs, gate))
}

fn open_in_scope(
    vfs: &'static RegisteredVfs,
    scope: u64,
    path: &Path,
) -> Result<Connection, Box<dyn std::error::Error>> {
    let _scope = vfs.enter_scope(NonZeroU64::new(scope).ok_or("zero test scope")?);
    let connection = Connection::open_with_flags_and_vfs(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        vfs.name().to_str()?,
    )?;
    let value: i64 = connection.query_row("SELECT v FROM retirement_gold", [], |row| row.get(0))?;
    require(value == 7, "test database changed")?;
    Ok(connection)
}

fn close_in_scope(
    vfs: &'static RegisteredVfs,
    scope: u64,
    connection: Connection,
) -> Result<(), std::io::Error> {
    let _scope =
        vfs.enter_scope(NonZeroU64::new(scope).ok_or_else(|| std::io::Error::other("zero scope"))?);
    connection
        .close()
        .map_err(|(_connection, error)| std::io::Error::other(error.to_string()))
}

fn same_original_inode(first: Option<FileWitness>, second: Option<FileWitness>) -> bool {
    matches!((first, second), (Some(first), Some(second))
        if first.device == second.device && first.inode == second.inode)
}

fn join_close(join: thread::JoinHandle<Result<(), std::io::Error>>) -> TestResult {
    join.join()
        .map_err(|_panic| std::io::Error::other("native close thread panicked"))??;
    Ok(())
}

fn rejected_open_cleanup(vfs: &'static RegisteredVfs, gate: &Arc<Gate>, path: &Path) -> TestResult {
    gate.arm()?;
    let path = path.to_path_buf();
    let name = vfs.name().to_str()?.to_owned();
    let attempted = thread::spawn(move || {
        let _scope = vfs.enter_scope(
            NonZeroU64::new(MAIN_SCOPE).ok_or_else(|| std::io::Error::other("zero main scope"))?,
        );
        match Connection::open_with_flags_and_vfs(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            name.as_str(),
        ) {
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == rusqlite::ErrorCode::CannotOpen =>
            {
                Ok(())
            }
            Err(error) => Err(std::io::Error::other(format!(
                "open did not preserve AfterOpen CannotOpen: {error}"
            ))),
            Ok(connection) => {
                drop(connection);
                Err(std::io::Error::other(
                    "rejected AfterOpen unexpectedly succeeded",
                ))
            }
        }
    });
    let entered = gate.wait_entered();
    let concurrent_drain = if entered.is_ok() {
        Some(vfs.drain_retirements())
    } else {
        None
    };
    // A fixture failure must still release the blocked callback and join its thread.
    gate.release();
    let attempted = join_close(attempted);
    let (retired, close_witness) = entered?;
    if let Some(drain) = concurrent_drain {
        drain.map_err(|error| std::io::Error::other(format!("concurrent drain: {error:?}")))?;
    }
    attempted?;
    require(
        retired,
        "rejected-open cleanup did not retire its native main FD",
    )?;
    vfs.drain_retirements()
        .map_err(|error| std::io::Error::other(format!("final drain: {error:?}")))?;
    let (count, opening_witness, retired_witness) = gate.snapshot()?;
    require(
        count == 0,
        "concurrent drain duplicated rejected-open retirement",
    )?;
    require(
        same_original_inode(close_witness, opening_witness),
        "rejected-open cleanup did not carry the original FD witness",
    )?;
    require(
        retired_witness.is_none(),
        "rejected-open cleanup unexpectedly emitted deferred retirement",
    )
}

fn child(mode: Mode) -> TestResult {
    let other_scope = NonZeroU64::new(OTHER_SCOPE).ok_or("zero other scope")?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("retirement.sqlite");
    let seed = Connection::open(&path)?;
    seed.execute_batch(
        "CREATE TABLE retirement_gold(v INTEGER); INSERT INTO retirement_gold VALUES(7)",
    )?;
    drop(seed);
    let (vfs, gate) = register_gate(mode)?;
    if mode == Mode::RejectedOpenCleanup {
        return rejected_open_cleanup(vfs, &gate, &path);
    }
    let other = if mode == Mode::DeferredMain {
        let _scope = vfs.enter_scope(other_scope);
        let connection = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            vfs.name().to_str()?,
        )?;
        connection.execute_batch("BEGIN")?;
        let _: i64 = connection.query_row("SELECT v FROM retirement_gold", [], |row| row.get(0))?;
        Some(connection)
    } else {
        None
    };
    let connection = open_in_scope(vfs, MAIN_SCOPE, &path)?;
    if mode == Mode::ImmediateShm {
        let _scope = vfs.enter_scope(NonZeroU64::new(MAIN_SCOPE).ok_or("zero main scope")?);
        connection
            .execute_batch("PRAGMA journal_mode=WAL; INSERT INTO retirement_gold VALUES(8)")?;
    }
    gate.arm()?;
    let close = thread::spawn(move || {
        if mode == Mode::FailedMainCallback {
            let _scope = vfs.enter_scope(
                NonZeroU64::new(MAIN_SCOPE)
                    .ok_or_else(|| std::io::Error::other("zero main scope"))?,
            );
            // SQLite's destructor does not expose xClose's result. The admission
            // callback itself returns Io, and a later drain must retry custody.
            drop(connection);
            Ok(())
        } else {
            close_in_scope(vfs, MAIN_SCOPE, connection)
        }
    });
    let entered = gate.wait_entered();
    // Always release and join the close thread, including a broken fixture.
    let result = if let Ok((snapshot, _witness)) = &entered {
        if mode == Mode::DeferredMain {
            if let Some(other) = other {
                let _scope = vfs.enter_scope(other_scope);
                let released = other
                    .execute_batch("ROLLBACK")
                    .map_err(|error| std::io::Error::other(error.to_string()));
                let closed = other
                    .close()
                    .map_err(|(_connection, error)| std::io::Error::other(error.to_string()));
                (released.and(closed), None)
            } else {
                (
                    Err(std::io::Error::other("missing read-lock connection")),
                    None,
                )
            }
        } else if mode == Mode::FailedMainCallback {
            (Ok(()), None)
        } else if *snapshot {
            (Ok(()), Some(vfs.drain_retirements()))
        } else {
            (Ok(()), None)
        }
    } else {
        (Ok(()), None)
    };
    gate.release();
    let closed = join_close(close);
    let (snapshot, immediate_witness) = entered?;
    result.0?;
    if let Some(drain) = result.1 {
        drain.map_err(|error| std::io::Error::other(format!("concurrent drain: {error:?}")))?;
    }
    closed?;
    require(
        snapshot == (mode != Mode::DeferredMain),
        "test did not create required immediate/deferred FD retirement",
    )?;
    vfs.drain_retirements()
        .map_err(|error| std::io::Error::other(format!("final drain: {error:?}")))?;
    let (count, opening_witness, retired_witness) = gate.snapshot()?;
    if mode == Mode::DeferredMain || mode == Mode::FailedMainCallback {
        require(
            count == 1,
            "deferred original FD retirement was lost or duplicated",
        )?;
        require(
            same_original_inode(opening_witness, retired_witness),
            "deferred retirement did not carry original FD witness",
        )?;
    } else {
        require(
            count == 0,
            "concurrent drain duplicated immediate retirement",
        )?;
        require(
            same_original_inode(immediate_witness, opening_witness),
            "immediate retirement did not carry original FD witness",
        )?;
    }
    Ok(())
}

fn read_log_prefix(file: &mut File) -> Result<String, std::io::Error> {
    let _position = file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    let _bytes_read = file.take(LOG_PREFIX_BYTES).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn run_isolated(name: &str, mode: Mode) -> TestResult {
    if std::env::var(CHILD_MODE).ok().as_deref() == Some(mode.label()) {
        child(mode)?;
        writeln!(
            std::io::stdout().lock(),
            "NATIVE_RETIREMENT_CHILD_OK:{}",
            mode.label()
        )?;
        return Ok(());
    }
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--exact", name, "--nocapture"])
        .env(CHILD_MODE, mode.label())
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?))
        .spawn()?;
    // Polling controls test-process liveness only; the Condvar event is the race oracle.
    let deadline = Instant::now() + CHILD_WAIT;
    let (status, wait_error) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), None),
            Ok(None) if Instant::now() >= deadline => {
                break (None, Some(format!("child timed out after {CHILD_WAIT:?}")));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => break (None, Some(format!("child wait failed: {error}"))),
        }
    };
    if status.is_none() {
        drop(child.kill());
        drop(child.wait());
    }
    let stdout = read_log_prefix(&mut stdout)?;
    let stderr = read_log_prefix(&mut stderr)?;
    require(
        status.as_ref().is_some_and(|status| status.success())
            && stdout.contains(&format!("NATIVE_RETIREMENT_CHILD_OK:{}", mode.label())),
        &format!(
            "isolated retirement child failed: status={status:?} wait_error={wait_error:?} stdout={stdout} stderr={stderr}",
        ),
    )
}

#[test]
fn immediate_main_close_and_global_drain_notify_once() -> TestResult {
    run_isolated(
        "publication_sqlite_vfs::tests::native_retirement::immediate_main_close_and_global_drain_notify_once",
        Mode::ImmediateMain,
    )
}

#[test]
fn immediate_shm_unmap_and_global_drain_notify_once() -> TestResult {
    run_isolated(
        "publication_sqlite_vfs::tests::native_retirement::immediate_shm_unmap_and_global_drain_notify_once",
        Mode::ImmediateShm,
    )
}

#[test]
fn late_main_close_requires_deferred_original_fd_notification() -> TestResult {
    run_isolated(
        "publication_sqlite_vfs::tests::native_retirement::late_main_close_requires_deferred_original_fd_notification",
        Mode::DeferredMain,
    )
}

#[test]
fn rejected_open_cleanup_and_global_drain_notify_once() -> TestResult {
    run_isolated(
        "publication_sqlite_vfs::tests::native_retirement::rejected_open_cleanup_and_global_drain_notify_once",
        Mode::RejectedOpenCleanup,
    )
}

#[test]
fn failed_immediate_callback_retries_original_fd_retirement() -> TestResult {
    run_isolated(
        "publication_sqlite_vfs::tests::native_retirement::failed_immediate_callback_retries_original_fd_retirement",
        Mode::FailedMainCallback,
    )
}
