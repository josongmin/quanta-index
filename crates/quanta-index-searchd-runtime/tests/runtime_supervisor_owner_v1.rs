//! SEP-21 P08 / S21-09 owner suite: the supervised runtime lifecycle.
//!
//! Everything here drives the supervisor's own primitives (registry,
//! two-deadline drain, rollback, RAII reconciliation, lease exclusion)
//! with disposable local resources only: synthetic child threads,
//! disposable temp state roots, and helper processes spawned from this
//! very test binary. No release daemon, no external corpus, no network.

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    reason = "integration-test helpers outside `#[test]` fns assert fixture setup with `expect`; the workspace already permits this inside test fns and a helper that cannot set up its fixture has no caller to propagate to"
)]

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use quanta_index_searchd::{
    CancelRoot, ChildContext, ChildExitKind, SearchdSupervisor, SupervisionOutcome,
};

/// The cadence synthetic children poll their cancellation flag at.
const CHILD_POLL: Duration = Duration::from_millis(5);

/// What the drop-order recorder observed, in order.
type DropLog = Arc<Mutex<Vec<&'static str>>>;

/// Guard payload for the synthetic tests: records its own drop so every
/// test can prove the guards did not drop before the children.
struct RecordedGuards {
    log: DropLog,
}

impl Drop for RecordedGuards {
    fn drop(&mut self) {
        let mut log = self.log.lock().expect("drop log is not poisoned");
        log.push("guards-dropped");
    }
}

/// Spawn a child that exits cleanly once the cancellation flag fires.
fn well_behaved_child(
    name: &'static str,
    log: &DropLog,
) -> impl FnOnce(ChildContext) -> anyhow::Result<std::thread::JoinHandle<()>> {
    let log = Arc::clone(log);
    move |context: ChildContext| {
        let log = Arc::clone(&log);
        let handle = std::thread::Builder::new()
            .name(format!("child-{name}"))
            .spawn(move || {
                while !context.shutdown().load(Ordering::Acquire) {
                    std::thread::sleep(CHILD_POLL);
                }
                log.lock().expect("drop log is not poisoned").push(name);
                context.report_exit(ChildExitKind::Completed);
            })?;
        Ok(handle)
    }
}

/// Spawn a child that reports a failed outcome once cancelled.
fn failing_child() -> impl FnOnce(ChildContext) -> anyhow::Result<std::thread::JoinHandle<()>> {
    move |context: ChildContext| {
        let handle = std::thread::Builder::new()
            .name("child-failing".to_string())
            .spawn(move || {
                while !context.shutdown().load(Ordering::Acquire) {
                    std::thread::sleep(CHILD_POLL);
                }
                context.report_exit(ChildExitKind::Failed);
            })?;
        Ok(handle)
    }
}

/// A spawn the thread table refuses; typed for the rollback path. The
/// errno is a stable "resource unavailable" stand-in.
fn refused_spawn(_context: ChildContext) -> anyhow::Result<std::thread::JoinHandle<()>> {
    Err(anyhow::Error::new(std::io::Error::from_raw_os_error(11)))
}

fn no_stop() -> Box<dyn FnOnce() + Send> {
    Box::new(|| {})
}

/// A clean operator shutdown drains both children, joins them, and
/// drops the guards only after both children are done.
#[test]
fn clean_drain_returns_zero_and_drops_guards_last() {
    let log: DropLog = Arc::new(Mutex::new(Vec::new()));
    let root = CancelRoot::new();
    let fire_after = {
        let root = CancelRoot::clone(&root);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            root.request_shutdown();
        })
    };
    let supervisor = SearchdSupervisor::new(
        Duration::from_secs(2),
        Duration::from_secs(5),
        RecordedGuards {
            log: Arc::clone(&log),
        },
        CancelRoot::clone(&root),
    );
    let outcome = run_two_children(
        supervisor,
        &root,
        well_behaved_child("child-a", &log),
        well_behaved_child("child-b", &log),
    );
    let _joined = fire_after.join();

    assert_eq!(
        outcome.exit_code(),
        0,
        "clean drain exits zero: {outcome:?}"
    );
    let SupervisionOutcome::StoppedClean {
        drained,
        cooperative_overdue,
    } = outcome
    else {
        panic!("expected a clean stop: {outcome:?}");
    };
    assert_eq!(drained, vec!["child-a", "child-b"]);
    assert!(
        cooperative_overdue.is_empty(),
        "no child missed the cooperative deadline"
    );
    let mut observed = log.lock().expect("drop log is not poisoned").clone();
    assert_eq!(
        observed.pop(),
        Some("guards-dropped"),
        "the guards drop after every child joins: {observed:?}"
    );
    observed.sort_unstable();
    assert_eq!(observed, vec!["child-a", "child-b"]);
}

/// A spawn failure during startup rolls back exactly what was started,
/// in reverse order, before the guards drop; the exit code is 70.
#[test]
fn startup_spawn_failure_rolls_back_in_reverse_order() {
    let log: DropLog = Arc::new(Mutex::new(Vec::new()));
    let root = CancelRoot::new();
    let mut supervisor = SearchdSupervisor::new(
        Duration::from_secs(2),
        Duration::from_secs(5),
        RecordedGuards {
            log: Arc::clone(&log),
        },
        CancelRoot::clone(&root),
    );
    let first = supervisor.spawn_child("child-a", no_stop(), well_behaved_child("child-a", &log));
    assert!(first.is_ok(), "the first child spawns");
    let second = supervisor.spawn_child("child-b", no_stop(), refused_spawn);
    assert!(second.is_err(), "the second spawn is refused");
    // Rollback stops children through their registered stop closures and
    // the supervisor's own cancellation flag; latch it before rolling
    // back.
    supervisor.cancel_root().request_shutdown();
    let outcome = supervisor.rollback("child-b");

    assert_eq!(outcome.exit_code(), 70, "rollback exits 70: {outcome:?}");
    let SupervisionOutcome::StartupRollback {
        failed,
        torn_down,
        escalated,
    } = outcome
    else {
        panic!("expected a rollback: {outcome:?}");
    };
    assert_eq!(failed, "child-b");
    assert_eq!(torn_down, vec!["child-a"], "the started child is torn down");
    assert!(escalated.is_empty(), "rollback is bounded and complete");
    assert_eq!(
        log.lock().expect("drop log is not poisoned").clone(),
        vec!["child-a", "guards-dropped"]
    );
}

/// A required child that exits while serving takes readiness down,
/// drains its peers, and exits non-zero — never a partial-ready
/// process.
#[test]
fn required_child_exit_propagates_and_drains_peers() {
    let log: DropLog = Arc::new(Mutex::new(Vec::new()));
    let root = CancelRoot::new();
    let mut supervisor = SearchdSupervisor::new(
        Duration::from_secs(2),
        Duration::from_secs(5),
        RecordedGuards {
            log: Arc::clone(&log),
        },
        CancelRoot::clone(&root),
    );
    // child-a exits on its own, immediately.
    let early = move |context: ChildContext| {
        let handle = std::thread::Builder::new()
            .name("child-early".to_string())
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                context.report_exit(ChildExitKind::Completed);
            })?;
        Ok(handle)
    };
    let spawned_a = supervisor.spawn_child("child-a", no_stop(), early);
    let spawned_b =
        supervisor.spawn_child("child-b", no_stop(), well_behaved_child("child-b", &log));
    assert!(spawned_a.is_ok() && spawned_b.is_ok());
    let outcome = supervisor.run(&root);

    assert_eq!(
        outcome.exit_code(),
        70,
        "a lost required child exits 70: {outcome:?}"
    );
    let SupervisionOutcome::RequiredChildLost {
        name,
        kind,
        drained,
        escalated,
    } = outcome
    else {
        panic!("expected a lost required child: {outcome:?}");
    };
    assert_eq!((name, kind), ("child-a", ChildExitKind::Completed));
    assert_eq!(drained, vec!["child-b"]);
    assert!(escalated.is_empty());
    assert_eq!(
        log.lock().expect("drop log is not poisoned").clone(),
        vec!["child-b", "guards-dropped"]
    );
}

/// A child that ignores cooperative cancellation is escalated at the
/// hard deadline: the join is abandoned, the outcome is named, and the
/// exit is 70 — the drain is not marked graceful.
#[test]
fn hard_deadline_escalation_is_not_graceful() {
    let log: DropLog = Arc::new(Mutex::new(Vec::new()));
    let root = CancelRoot::new();
    let mut supervisor = SearchdSupervisor::new(
        Duration::from_millis(80),
        Duration::from_millis(400),
        RecordedGuards {
            log: Arc::clone(&log),
        },
        CancelRoot::clone(&root),
    );
    let ignoring = move |context: ChildContext| {
        let handle = std::thread::Builder::new()
            .name("child-ignoring".to_string())
            .spawn(move || {
                // Ignores the cancellation flag outright.
                std::thread::sleep(Duration::from_secs(30));
                context.report_exit(ChildExitKind::Completed);
            })?;
        Ok(handle)
    };
    let spawned = supervisor.spawn_child("child-ignoring", no_stop(), ignoring);
    assert!(spawned.is_ok());
    let started = std::time::Instant::now();
    let trigger = {
        let root = CancelRoot::clone(&root);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            root.request_shutdown();
        })
    };
    let outcome = supervisor.run(&root);
    let _joined = trigger.join();

    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "the hard deadline bounds the drain: {elapsed:?}"
    );
    assert_eq!(outcome.exit_code(), 70, "escalation exits 70: {outcome:?}");
    let SupervisionOutcome::HardDeadlineEscalated { unfinished } = outcome else {
        panic!("expected a hard-deadline escalation: {outcome:?}");
    };
    assert_eq!(unfinished, vec!["child-ignoring"]);
    assert_eq!(
        log.lock().expect("drop log is not poisoned").clone(),
        vec!["guards-dropped"]
    );
}

/// A child that needs longer than the cooperative deadline but finishes
/// before the hard one joins cleanly; the receipt names it overdue and
/// the exit stays zero.
#[test]
fn cooperative_overdue_child_still_drains_cleanly() {
    let log: DropLog = Arc::new(Mutex::new(Vec::new()));
    let root = CancelRoot::new();
    let slow = move |context: ChildContext| {
        let handle = std::thread::Builder::new()
            .name("child-slow".to_string())
            .spawn(move || {
                while !context.shutdown().load(Ordering::Acquire) {
                    std::thread::sleep(CHILD_POLL);
                }
                // Takes longer than the cooperative deadline, far less
                // than the hard one.
                std::thread::sleep(Duration::from_millis(150));
                context.report_exit(ChildExitKind::Completed);
            })?;
        Ok(handle)
    };
    let mut supervisor = SearchdSupervisor::new(
        Duration::from_millis(60),
        Duration::from_secs(5),
        RecordedGuards {
            log: Arc::clone(&log),
        },
        CancelRoot::clone(&root),
    );
    let spawned = supervisor.spawn_child("child-slow", no_stop(), slow);
    assert!(spawned.is_ok());
    let trigger = {
        let root = CancelRoot::clone(&root);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            root.request_shutdown();
        })
    };
    let outcome = supervisor.run(&root);
    let _joined = trigger.join();

    assert_eq!(outcome.exit_code(), 0);
    let SupervisionOutcome::StoppedClean {
        drained,
        cooperative_overdue,
    } = outcome
    else {
        panic!("expected a clean stop: {outcome:?}");
    };
    assert_eq!(drained, vec!["child-slow"]);
    assert_eq!(cooperative_overdue, vec!["child-slow"]);
}

/// A child that ends uncleanly during an operator drain makes the stop
/// non-graceful: exit 70, the failure named.
#[test]
fn failed_child_during_drain_is_not_graceful() {
    let log: DropLog = Arc::new(Mutex::new(Vec::new()));
    let root = CancelRoot::new();
    let mut supervisor = SearchdSupervisor::new(
        Duration::from_secs(2),
        Duration::from_secs(5),
        RecordedGuards {
            log: Arc::clone(&log),
        },
        CancelRoot::clone(&root),
    );
    let spawned = supervisor.spawn_child("child-failing", no_stop(), failing_child());
    assert!(spawned.is_ok());
    let trigger = {
        let root = CancelRoot::clone(&root);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            root.request_shutdown();
        })
    };
    let outcome = supervisor.run(&root);
    let _joined = trigger.join();

    assert_eq!(outcome.exit_code(), 70);
    let SupervisionOutcome::DrainFailed { failed } = outcome else {
        panic!("expected a failed drain: {outcome:?}");
    };
    assert_eq!(failed, vec![("child-failing", ChildExitKind::Failed)]);
}

/// The signal latch: the first request is a cooperative shutdown, the
/// second latches an abort whose exit code is `128 + signum`.
#[test]
fn second_signal_latches_an_immediate_abort() {
    let log: DropLog = Arc::new(Mutex::new(Vec::new()));
    let root = CancelRoot::new();
    let mut supervisor = SearchdSupervisor::new(
        Duration::from_secs(60),
        Duration::from_secs(125),
        RecordedGuards {
            log: Arc::clone(&log),
        },
        CancelRoot::clone(&root),
    );
    // The child ignores cooperative cancellation, so the drain is still
    // waiting when the second signal latches the abort.
    let blocking = move |context: ChildContext| {
        let handle = std::thread::Builder::new()
            .name("child-blocking".to_string())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(30));
                context.report_exit(ChildExitKind::Completed);
            })?;
        Ok(handle)
    };
    let spawned = supervisor.spawn_child("child-blocking", no_stop(), blocking);
    assert!(spawned.is_ok());
    let trigger = {
        let root = CancelRoot::clone(&root);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            root.request_shutdown();
            std::thread::sleep(Duration::from_millis(200));
            root.request_abort(15);
        })
    };
    let outcome = supervisor.run(&root);
    let _joined = trigger.join();

    let SupervisionOutcome::SignalAbort { signum } = outcome else {
        panic!("expected a signal abort: {outcome:?}");
    };
    assert_eq!(signum, 15);
    assert_eq!(outcome.exit_code(), 143);
}

/// Two-process state-root exclusion (S21-09).
///
/// While the first process is serving (holding the lease), a second
/// process gets the typed `STATE_ROOT_IN_USE` refusal; after the first
/// process exits, the lease is acquirable again. The lock file is
/// exactly the expected owner, mode `0600`, a regular file, one link.
#[test]
fn state_root_lease_two_process_exclusion() {
    // Parent side: two disposable child processes of this very test
    // binary (see `p08_lease_child_entry`) over a disposable temp root.
    run_two_process_lease_parent();
}

/// The child-process entry the parent spawns: `holder` takes the lease
/// and holds until released, `second` must be refused, `orphan_parent`
/// orphans a holder without releasing it.
#[test]
fn p08_lease_child_entry() {
    let Some(role) = std::env::var_os("QUANTA_INDEX_P08_LEASE_ROLE") else {
        // Parent-run: nothing to do here.
        return;
    };
    let Some(root) = std::env::var_os("QUANTA_INDEX_P08_LEASE_ROOT").map(std::path::PathBuf::from)
    else {
        panic!("the lease root is provided");
    };
    let Some(report) =
        std::env::var_os("QUANTA_INDEX_P08_LEASE_REPORT").map(std::path::PathBuf::from)
    else {
        panic!("the report path is provided");
    };
    match role.to_str() {
        Some("holder") => hold_the_lease(&root, &report),
        Some("second") => attempt_the_lease(&root, &report),
        Some("orphan_parent") => {
            let Some(holder_report) = std::env::var_os("QUANTA_INDEX_P08_LEASE_HOLDER_REPORT")
                .map(std::path::PathBuf::from)
            else {
                panic!("the holder report path is provided");
            };
            orphan_a_holder(&root, &report, &holder_report);
        }
        _ => panic!("unknown lease role"),
    }
}

/// Parent death must release the holder (TOPT-03 proof): an
///
/// intermediate parent exits without sending the release byte. Closing
/// its pipe makes the holder observe EOF and release the lease, with no
/// child-side timer or polling loop.
#[test]
fn lease_holder_exits_when_parent_dies_without_release() {
    run_orphaned_lease_parent();
}

/// Holder side: acquire, prove the lock file's fstat invariants
///
/// (expected owner, exact mode, regular, one link), report, then hold
/// until the parent's acknowledged release — no fixed hold duration.
///
/// The parent owns the pipe writer. It sends `R` only after the second
/// process is refused; if the parent exits instead, EOF releases the
/// holder without waiting for a deadline.
#[expect(
    clippy::panic,
    reason = "child-process fixture must fail on an invalid release byte"
)]
fn hold_the_lease(root: &std::path::Path, report: &std::path::Path) {
    use std::os::unix::fs::MetadataExt;
    let lease = quanta_index_searchd::app::runtime::StateRootLease::acquire(root)
        .expect("the first process acquires the lease");
    let lock = root.join(".searchd-state-root.lock");
    let metadata = std::fs::metadata(&lock).expect("the lock file exists");
    assert!(metadata.is_file(), "the lock is a regular file");
    assert_eq!(
        metadata.mode() & 0o7777,
        0o600,
        "the lock mode is exactly 0600"
    );
    assert_eq!(metadata.nlink(), 1, "the lock has exactly one link");
    assert_eq!(
        metadata.uid(),
        rustix::process::geteuid().as_raw(),
        "the lock is owned by this uid"
    );
    std::fs::write(report, b"held\n").expect("the report is written");
    let mut signal = [0_u8; 1];
    let stdin = std::io::stdin();
    let mut stdin_lock = stdin.lock();
    let release_reason = match stdin_lock.read(&mut signal) {
        Ok(0) => "parent-eof",
        Ok(1) if signal[0] == b'R' => "explicit",
        Ok(_) => panic!("the parent sent an invalid lease-release byte"),
        Err(error) => panic!("reading the lease-release pipe failed: {error}"),
    };
    drop(stdin_lock);
    drop(lease);
    std::fs::write(report, format!("held\nreleased-{release_reason}\n"))
        .expect("the terminal report is written");
}

/// Second-process side: the typed `STATE_ROOT_IN_USE` refusal is the
/// only acceptable outcome while the first process serves.
fn attempt_the_lease(root: &std::path::Path, report: &std::path::Path) {
    let attempt = quanta_index_searchd::app::runtime::StateRootLease::acquire(root);
    let outcome = match attempt {
        Err(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInUse,
            ..
        }) => "refused",
        Ok(_unexpectedly_held) => "held",
        Err(_other) => "error",
    };
    std::fs::write(report, format!("{outcome}\n").as_bytes()).expect("the report is written");
}

/// Intermediate-parent side of the abort proof: spawn a holder, wait
///
/// until it reports holding, then exit without sending a release byte.
/// The OS closes the pipe writer on process exit, which is the holder's
/// release event. Reports `orphaned` to prove the abort happened while held.
#[expect(
    clippy::exit,
    reason = "the abort fixture must exit without running destructors so the holder observes pipe EOF"
)]
fn orphan_a_holder(
    root: &std::path::Path,
    report: &std::path::Path,
    holder_report: &std::path::Path,
) {
    let exe = std::env::current_exe().expect("this test binary exists");
    let mut holder = Command::new(&exe);
    let _configured = holder
        .arg("--exact")
        .arg("p08_lease_child_entry")
        .arg("--nocapture")
        .env("QUANTA_INDEX_P08_LEASE_ROLE", "holder")
        .env("QUANTA_INDEX_P08_LEASE_ROOT", root)
        .env("QUANTA_INDEX_P08_LEASE_REPORT", holder_report)
        .stdin(Stdio::piped());
    // The child remains alive while this process holds its stdin writer.
    let _child = holder.spawn().expect("the holder child spawns");
    let deadline = std::time::Instant::now()
        .checked_add(Duration::from_secs(10))
        .expect("the deadline is representable");
    while !holder_report.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        holder_report.exists(),
        "the holder reported holding before the abort"
    );
    std::fs::write(report, b"orphaned\n").expect("the report is written");
    // Do not drop the Child handle first: process exit itself must close
    // the inherited writer and cause the holder to observe EOF.
    std::process::exit(0);
}

/// Parent side of the two-process exclusion proof: disposable temp
/// root, two child processes of this binary, exact file evidence.
#[expect(
    clippy::panic,
    reason = "the helper-process timeout is a terminal test failure"
)]
fn run_two_process_lease_parent() {
    let temp = tempfile::tempdir().expect("a disposable temp root");
    let root = temp.path().join("state");
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .expect("the state root is created exactly private");
    }
    let holder_report = temp.path().join("holder-report");
    let second_report = temp.path().join("second-report");
    let exe = std::env::current_exe().expect("this test binary exists");
    let base_env = |role: &str, report: &std::path::Path| {
        let mut command = Command::new(&exe);
        let _configured = command
            .arg("--exact")
            .arg("p08_lease_child_entry")
            .arg("--nocapture")
            .env("QUANTA_INDEX_P08_LEASE_ROLE", role)
            .env("QUANTA_INDEX_P08_LEASE_ROOT", &root)
            .env("QUANTA_INDEX_P08_LEASE_REPORT", report);
        command
    };

    let mut holder = base_env("holder", &holder_report)
        .stdin(Stdio::piped())
        .spawn()
        .expect("the holder child spawns");
    // Wait for the holder to actually hold the lease.
    let deadline = std::time::Instant::now()
        .checked_add(Duration::from_secs(10))
        .expect("the deadline is representable");
    while !holder_report.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        holder_report.exists(),
        "the holder reported holding the lease"
    );

    // While the first process is serving, the second is refused.
    let mut second = base_env("second", &second_report)
        .spawn()
        .expect("the second child spawns");
    let second_status = second.wait().expect("the second child exits");
    assert!(second_status.success(), "the second child ran its proof");
    let second_outcome = std::fs::read_to_string(&second_report).expect("the report is readable");
    assert_eq!(
        second_outcome.trim(),
        "refused",
        "the second process is refused while the first serves: {second_outcome}"
    );

    // The parent owns the only writer and sends the release byte only
    // after the typed refusal above is proven.
    let mut release = holder.stdin.take().expect("the holder has a release pipe");
    release
        .write_all(b"R")
        .expect("the release byte is written");
    drop(release);
    let exit_deadline = std::time::Instant::now()
        .checked_add(Duration::from_secs(10))
        .expect("the exit deadline is representable");
    let holder_status = loop {
        match holder.try_wait().expect("the holder is waitable") {
            Some(status) => break status,
            None if std::time::Instant::now() >= exit_deadline => {
                let _killed = holder.kill();
                let _reaped = holder.wait();
                panic!("the holder exits promptly after release");
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    assert!(holder_status.success(), "the holder ran its proof");
    assert_eq!(
        std::fs::read_to_string(&holder_report).expect("the terminal report is readable"),
        "held\nreleased-explicit\n",
        "the holder observed the explicit release"
    );
    let reacquired = quanta_index_searchd::app::runtime::StateRootLease::acquire(&root);
    assert!(
        reacquired.is_ok(),
        "the lease is acquirable after the first process exits"
    );
}

/// Grandparent side of the abort proof: the intermediate parent exits
/// while the holder holds. The holder must report pipe EOF and release
/// the lease; a containment deadline only bounds this parent's wait.
fn run_orphaned_lease_parent() {
    let temp = tempfile::tempdir().expect("a disposable temp root");
    let root = temp.path().join("state");
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .expect("the state root is created exactly private");
    }
    let holder_report = temp.path().join("holder-report");
    let orphan_report = temp.path().join("orphan-report");
    let exe = std::env::current_exe().expect("this test binary exists");
    let mut command = Command::new(&exe);
    let _configured = command
        .arg("--exact")
        .arg("p08_lease_child_entry")
        .arg("--nocapture")
        .env("QUANTA_INDEX_P08_LEASE_ROLE", "orphan_parent")
        .env("QUANTA_INDEX_P08_LEASE_ROOT", &root)
        .env("QUANTA_INDEX_P08_LEASE_REPORT", &orphan_report)
        .env("QUANTA_INDEX_P08_LEASE_HOLDER_REPORT", &holder_report)
        .stdin(Stdio::null());
    let mut orphan = command.spawn().expect("the orphan-parent spawns");
    let orphan_status = orphan.wait().expect("the orphan-parent exits");
    assert!(orphan_status.success(), "the orphan-parent ran its proof");
    let orphan_outcome =
        std::fs::read_to_string(&orphan_report).expect("the orphan report is readable");
    assert_eq!(
        orphan_outcome.trim(),
        "orphaned",
        "the abort happened while the holder held"
    );
    let held = std::fs::read_to_string(&holder_report).expect("the holder report is readable");
    assert!(
        held.starts_with("held\n"),
        "the holder held before the abort"
    );

    // A terminal report is written only after the holder read EOF and
    // dropped the lease. Wait for that event, not for a lease timeout.
    let deadline = std::time::Instant::now()
        .checked_add(Duration::from_secs(10))
        .expect("the deadline is representable");
    loop {
        let report =
            std::fs::read_to_string(&holder_report).expect("the holder report is readable");
        if report == "held\nreleased-parent-eof\n" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the orphaned holder must observe parent EOF, got {report:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let reacquired = quanta_index_searchd::app::runtime::StateRootLease::acquire(&root)
        .expect("the holder released its lease after parent EOF");
    drop(reacquired);
    let again = quanta_index_searchd::app::runtime::StateRootLease::acquire(&root);
    assert!(again.is_ok(), "no lease residue survives the abort");
}

/// Helper: run one supervisor with two children and derive its outcome.
fn run_two_children<A, B>(
    supervisor: SearchdSupervisor<RecordedGuards>,
    root: &CancelRoot,
    spawn_a: A,
    spawn_b: B,
) -> SupervisionOutcome
where
    A: FnOnce(ChildContext) -> anyhow::Result<std::thread::JoinHandle<()>>,
    B: FnOnce(ChildContext) -> anyhow::Result<std::thread::JoinHandle<()>>,
{
    let mut supervisor = supervisor;
    let spawned_a = supervisor.spawn_child("child-a", no_stop(), spawn_a);
    let spawned_b = supervisor.spawn_child("child-b", no_stop(), spawn_b);
    assert!(spawned_a.is_ok() && spawned_b.is_ok());
    supervisor.run(root)
}
