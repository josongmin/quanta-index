//! Guard result-routing and real root-lease custody; no native I/O fault claim.

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use quanta_index_core::CoreError;

use super::{RuntimeRetirementGuard, RuntimeRetirementPort, StateRootLease};

type TestResult = Result<(), Box<dyn Error>>;

struct ResultPort {
    calls: Arc<AtomicUsize>,
    result: Result<(), CoreError>,
}

impl RuntimeRetirementPort for ResultPort {
    fn retire(&self) -> Result<(), CoreError> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

struct PanicPort(Arc<AtomicUsize>);

impl RuntimeRetirementPort for PanicPort {
    fn retire(&self) -> Result<(), CoreError> {
        let _previous = self.0.fetch_add(1, Ordering::SeqCst);
        std::panic::resume_unwind(Box::new("fixed retirement panic"))
    }
}

#[test]
fn checked_success_retires_once_and_releases_the_original_lease() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("state");
    let calls = Arc::new(AtomicUsize::new(0));
    let port: Arc<dyn RuntimeRetirementPort> = Arc::new(ResultPort {
        calls: Arc::clone(&calls),
        result: Ok(()),
    });
    let weak = Arc::downgrade(&port);
    let mut guard = RuntimeRetirementGuard::new(port, StateRootLease::acquire(&root)?);
    guard.retire_checked()?;
    guard.retire_checked()?;
    if calls.load(Ordering::SeqCst) != 1 || StateRootLease::acquire_existing(&root).is_ok() {
        return Err("checked success duplicated work or released the guard lease early".into());
    }
    drop(guard);
    if weak.upgrade().is_some() {
        return Err("clean guard retained the retirement owner".into());
    }
    drop(StateRootLease::acquire_existing(&root)?);
    Ok(())
}

#[test]
fn failed_explicit_retirement_keeps_original_error_and_does_not_retry_on_drop() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("state");
    let error = CoreError::StorageIo {
        kind: std::io::ErrorKind::StorageFull,
        raw_os_error: Some(28),
        message: "fixed original close failure".into(),
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let port: Arc<dyn RuntimeRetirementPort> = Arc::new(ResultPort {
        calls: Arc::clone(&calls),
        result: Err(error.clone()),
    });
    let weak = Arc::downgrade(&port);
    let mut guard = RuntimeRetirementGuard::new(port, StateRootLease::acquire(&root)?);
    if guard.retire_checked() != Err(error) {
        return Err("guard replaced the original kind, errno or message".into());
    }
    drop(guard);
    if calls.load(Ordering::SeqCst) != 1
        || weak.upgrade().is_none()
        || StateRootLease::acquire_existing(&root).is_ok()
    {
        return Err("failed guard retried close or surrendered original owner/root custody".into());
    }
    Ok(())
}

#[test]
fn unchecked_drop_checks_retirement_before_releasing_the_lease() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("state");
    let calls = Arc::new(AtomicUsize::new(0));
    let port: Arc<dyn RuntimeRetirementPort> = Arc::new(ResultPort {
        calls: Arc::clone(&calls),
        result: Ok(()),
    });
    drop(RuntimeRetirementGuard::new(
        port,
        StateRootLease::acquire(&root)?,
    ));
    if calls.load(Ordering::SeqCst) != 1 {
        return Err("unchecked drop did not retire the physical owner exactly once".into());
    }
    drop(StateRootLease::acquire_existing(&root)?);
    Ok(())
}

#[test]
fn retirement_panic_is_nonclean_and_keeps_original_owner_and_lease() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("state");
    let calls = Arc::new(AtomicUsize::new(0));
    let port: Arc<dyn RuntimeRetirementPort> = Arc::new(PanicPort(Arc::clone(&calls)));
    let weak = Arc::downgrade(&port);
    let mut guard = RuntimeRetirementGuard::new(port, StateRootLease::acquire(&root)?);
    if !matches!(guard.retire_checked(), Err(CoreError::Storage(message))
        if message == "runtime retirement panicked")
    {
        return Err("retirement panic escaped or was accepted as a clean close".into());
    }
    drop(guard);
    if calls.load(Ordering::SeqCst) != 1
        || weak.upgrade().is_none()
        || StateRootLease::acquire_existing(&root).is_ok()
    {
        return Err("panic retirement retried or surrendered original owner/root custody".into());
    }
    Ok(())
}
