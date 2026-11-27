//! A read ticket whose Catalog release always fails must not orphan its pin.
//!
//! Faults are real `SQLite` write-lock contention on the same
//! Catalog file, so release fails with a typed busy error until health returns.

use super::*;
use quanta_index_core::index::ports::{CatalogReadPort, ViewRetirementPage};

/// Holds the Catalog's `SQLite` write lock from a second connection. Every
/// Catalog write fails after the 100 ms busy timeout until `end`.
struct WriteOutage(rusqlite::Connection);

impl WriteOutage {
    fn begin(catalog: &SqliteCatalog) -> TestResult<Self> {
        let connection = rusqlite::Connection::open(catalog.path())?;
        connection.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Self(connection))
    }

    fn end(self) -> TestResult {
        self.0.execute_batch("ROLLBACK")?;
        Ok(())
    }
}

fn pins_at(catalog: &SqliteCatalog, view: ViewRef) -> TestResult<i64> {
    Ok(rusqlite::Connection::open(catalog.path())?.query_row(
        "SELECT COUNT(*) FROM native_pin_v1 WHERE sequence=?1",
        [i64::try_from(view.sequence)?],
        |row| row.get(0),
    )?)
}

fn retire(catalog: &SqliteCatalog) -> TestResult<ViewRetirementPage> {
    Ok(catalog.retire_exact_history_page(workspace(), 1, None, 8, &mut budget())?)
}

/// Publish A, pin it, publish B and move the Exact floor to B. Genesis and A
/// are below the floor; genesis is unpinned, so the first sweep retires it and
/// A is reclaimable exactly when no pin protects it.
fn pinned_below_floor(
    kernel: &Kernel<SqliteCatalog>,
    catalog: &SqliteCatalog,
) -> TestResult<(ViewRef, ViewRef, quanta_index_kernel::ReadTicket)> {
    let view_a = configured(kernel)?;
    let ticket = kernel.pin_view(ViewSelection::Exact(view_a), &budget())?;
    let view_b = reconfigure(kernel, view_a, 2)?;
    assert_eq!(
        catalog.advance_exact_history_floor(workspace(), 1, view_b.sequence, &mut budget())?,
        view_b.sequence
    );
    assert_eq!(pins_at(catalog, view_a)?, 1);
    Ok((view_a, view_b, ticket))
}

#[test]
fn twice_failed_release_is_settled_once_by_read_recovery_and_view_retires() -> TestResult {
    let root = tempfile::tempdir()?;
    let (kernel, catalog) = kernel(root.path())?;
    let (view_a, view_b, ticket) = pinned_below_floor(&kernel, &catalog)?;

    // Attempt 1 is the explicit release; attempt 2 is the consumed ticket's
    // drop. Both fail while the Catalog write lock is held elsewhere.
    let outage = WriteOutage::begin(&catalog)?;
    assert!(ticket.release().is_err());
    outage.end()?;

    // Health is restored, but no ticket owner remains: the pin still exists
    // and protects A until a settlement owner retries the release.
    assert_eq!(pins_at(&catalog, view_a)?, 1);
    let protected = retire(&catalog)?;
    assert_eq!((protected.retired, protected.blocked), (1, 1));

    // The existing read-session maintenance round settles the original pin
    // without a daemon restart.
    let _page = catalog.recover_read_sessions(None, 16, &mut budget())?;
    assert_eq!(pins_at(&catalog, view_a)?, 0);

    // Settlement is exact and happens once: a later round neither fails nor
    // touches an unrelated live pin issued by the same session.
    let live = kernel.pin_view(ViewSelection::Exact(view_b), &budget())?;
    let _again = catalog.recover_read_sessions(None, 16, &mut budget())?;
    assert_eq!(pins_at(&catalog, view_b)?, 1);
    assert_eq!(native_pin_count(&catalog)?, 1);

    // The ordinary retention sweep now reclaims A.
    let reclaimed = retire(&catalog)?;
    assert_eq!((reclaimed.retired, reclaimed.blocked), (1, 0));
    assert!(matches!(
        kernel.pin_view(ViewSelection::Exact(view_a), &budget()),
        Err(IndexError::SnapshotUnavailable)
    ));
    live.release()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

/// Recorded current behaviour (ticket 07.9): a live session's expired pin blocks.
///
/// Read recovery settles only dead
/// sessions and abandoned pins, because an OS-live reader may still use an
/// expired pin; the owner's release (or process death) frees it.
#[test]
fn live_session_expired_pin_keeps_blocking_until_owner_release() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock(AtomicU64::new(1_000)));
    let (kernel, catalog) = kernel_with_clock(root.path(), clock.clone())?;
    let (view_a, _view_b, ticket) = pinned_below_floor(&kernel, &catalog)?;

    clock.0.store(1_000 + 3_600_000, Ordering::SeqCst);
    assert!(matches!(
        ticket.validate(),
        Err(IndexError::SnapshotUnavailable)
    ));
    let _page = catalog.recover_read_sessions(None, 16, &mut budget())?;
    assert_eq!(pins_at(&catalog, view_a)?, 1);
    let blocked = retire(&catalog)?;
    assert_eq!((blocked.retired, blocked.blocked), (1, 1));

    ticket.release()?;
    assert_eq!(pins_at(&catalog, view_a)?, 0);
    let reclaimed = retire(&catalog)?;
    assert_eq!((reclaimed.retired, reclaimed.blocked), (1, 0));
    Ok(())
}
