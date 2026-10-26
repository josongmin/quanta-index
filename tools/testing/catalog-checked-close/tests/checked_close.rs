//! The actual catalog close implementation with a real outstanding SQLite VM.
//! This test crate can call SQLite FFI without weakening the catalog library's
//! `forbid(unsafe_code)` boundary.

#[path = "../../../../crates/quanta-index-catalog/src/io_admission/close_connection.rs"]
mod close_connection;

use std::io;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use rusqlite::{Connection, Error, ErrorCode, ffi};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct CloseScope(Arc<AtomicUsize>);

impl Drop for CloseScope {
    fn drop(&mut self) {
        let _previous = self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[test]
fn busy_native_close_restores_original_connection_and_retries_under_scope() -> TestResult {
    let connection = Connection::open_in_memory()?;
    let statement = connection.prepare("SELECT 1")?;
    std::mem::forget(statement);
    // SAFETY: the connection remains in `slot` through the failed close and
    // the native pointer is used only before a successful retry.
    let original_handle = unsafe { connection.handle() };
    let mut slot = Some(Mutex::new(connection));
    let entries = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let observed = close_connection::close_connection_with_scope(
        &mut slot,
        || {
            let _previous = entries.fetch_add(1, Ordering::SeqCst);
            let _previous = active.fetch_add(1, Ordering::SeqCst);
            CloseScope(Arc::clone(&active))
        },
        |error| (error, active.load(Ordering::SeqCst)),
    );
    let (error, active_during_mapping) = match observed {
        Ok(()) => return Err(io::Error::other("SQLite closed with an outstanding VM").into()),
        Err(failure) => failure,
    };
    if !matches!(
        &error,
        Error::SqliteFailure(failure, _) if failure.code == ErrorCode::DatabaseBusy
    ) || active_during_mapping != 1
        || entries.load(Ordering::SeqCst) != 1
        || active.load(Ordering::SeqCst) != 0
    {
        return Err(io::Error::other(format!(
            "BUSY close lost its native cause or mapping scope: {error:?}"
        ))
        .into());
    }
    {
        let restored = slot
            .as_ref()
            .ok_or_else(|| io::Error::other("BUSY close discarded its connection"))?
            .lock()
            .map_err(|_| io::Error::other("restored connection mutex was poisoned"))?;
        // SAFETY: a failed checked close must return the same live connection.
        if unsafe { restored.handle() } != original_handle {
            return Err(io::Error::other("BUSY close replaced the native connection").into());
        }
    }
    let mapping_panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), ()> = close_connection::close_connection_with_scope(
            &mut slot,
            || {
                let _previous = entries.fetch_add(1, Ordering::SeqCst);
                let _previous = active.fetch_add(1, Ordering::SeqCst);
                CloseScope(Arc::clone(&active))
            },
            |_error| std::panic::panic_any("close classification failed"),
        );
    }));
    if mapping_panic.is_ok()
        || slot.is_none()
        || entries.load(Ordering::SeqCst) != 2
        || active.load(Ordering::SeqCst) != 0
    {
        return Err(io::Error::other("classifier panic lost the original connection").into());
    }
    // SAFETY: this is the one deliberately forgotten statement on the still
    // live original connection. We finalize it exactly once before retry.
    let statement = unsafe { ffi::sqlite3_next_stmt(original_handle, std::ptr::null_mut()) };
    if statement.is_null() {
        return Err(io::Error::other("original outstanding VM disappeared").into());
    }
    // SAFETY: `statement` has not been finalized by any other owner.
    let finalized = unsafe { ffi::sqlite3_finalize(statement) };
    if finalized != ffi::SQLITE_OK {
        return Err(io::Error::other(format!("finalize original VM: {finalized}")).into());
    }
    close_connection::close_connection_with_scope(
        &mut slot,
        || {
            let _previous = entries.fetch_add(1, Ordering::SeqCst);
            let _previous = active.fetch_add(1, Ordering::SeqCst);
            CloseScope(Arc::clone(&active))
        },
        std::convert::identity,
    )?;
    if slot.is_some() || entries.load(Ordering::SeqCst) != 3 || active.load(Ordering::SeqCst) != 0 {
        return Err(io::Error::other("retry did not settle the same connection").into());
    }
    Ok(())
}

#[test]
fn close_scope_entry_panic_keeps_the_original_connection_in_place() -> TestResult {
    let mut slot = Some(Mutex::new(Connection::open_in_memory()?));
    let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), Error> = close_connection::close_connection_with_scope(
            &mut slot,
            || std::panic::panic_any("close scope entry failed"),
            std::convert::identity,
        );
    }));
    if observed.is_ok() || slot.is_none() {
        return Err(io::Error::other("scope entry lost the original connection").into());
    }
    close_connection::close_connection_with_scope(&mut slot, || (), std::convert::identity)?;
    if slot.is_some() {
        return Err(io::Error::other("retained connection failed to close").into());
    }
    Ok(())
}
