//! Settlement custody for pins transferred by `abandon_ticket`.
//!
//! Each queued pin is settled independently inside the read-maintenance round.
//! The persistent fault is a real `SQLite` trigger that aborts deletes of one
//! pin row on this Catalog's own connection until it is dropped.

use super::*;

fn pins_at(catalog: &SqliteCatalog, sequence: u64) -> Result<i64, Box<dyn Error>> {
    Ok(catalog.lock()?.query_row(
        "SELECT COUNT(*) FROM native_pin_v1 WHERE sequence=?1",
        params![int(sequence)?],
        |row| row.get(0),
    )?)
}

fn set_pin_delete_fault(
    catalog: &SqliteCatalog,
    sequence: Option<u64>,
) -> Result<(), Box<dyn Error>> {
    let guard = catalog.lock()?;
    match sequence {
        Some(sequence) => guard.execute_batch(&format!(
            "CREATE TRIGGER injected_pin_fault BEFORE DELETE ON native_pin_v1 \
             WHEN OLD.sequence = {sequence} BEGIN SELECT RAISE(ABORT, 'injected pin fault'); END;"
        ))?,
        None => guard.execute_batch("DROP TRIGGER injected_pin_fault")?,
    }
    Ok(())
}

#[test]
fn faulted_abandoned_pin_defers_alone_while_other_pins_and_dead_sessions_settle() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, genesis) = setup(root.path())?;
    let next = publish_plain_next(&catalog, &genesis, 1, 231)?;
    let live = catalog.open_read_session()?;
    let lease = Duration::from_secs(30);
    let faulted = acquire_ticket_with_test_budget(
        &catalog,
        live.as_ref(),
        ViewSelection::Exact(genesis.identity),
        lease,
    )?;
    let healthy = acquire_ticket_with_test_budget(
        &catalog,
        live.as_ref(),
        ViewSelection::Exact(next.identity),
        lease,
    )?;
    catalog.abandon_ticket(Arc::clone(&live), faulted.lease)?;
    catalog.abandon_ticket(Arc::clone(&live), healthy.lease)?;
    let dead = catalog.open_read_session()?;
    let _dead_pin = acquire_ticket_with_test_budget(
        &catalog,
        dead.as_ref(),
        ViewSelection::Exact(next.identity),
        lease,
    )?;
    drop(dead);
    check_eq!(pins_at(&catalog, 0)?, 1);
    check_eq!(pins_at(&catalog, 1)?, 2);

    // The first queued pin fails persistently. The same round still settles
    // the second, recovers the dead session and reports the deferral.
    set_pin_delete_fault(&catalog, Some(0))?;
    let round = catalog.recover_read_sessions(None, 16, &mut admission_budget())?;
    check_eq!((round.abandoned_settled, round.abandoned_deferred), (1, 1));
    check_eq!((round.recovered, round.pins_released), (1, 1));
    check_eq!(pins_at(&catalog, 0)?, 1);
    check_eq!(pins_at(&catalog, 1)?, 0);
    let queued = catalog.abandoned_read_pins()?;
    check_eq!(queued.len(), 1);
    let (queued_lease, queued_error) = queued.first().ok_or("deferred pin left custody")?;
    check_eq!(*queued_lease, faulted.lease);
    check!(
        matches!(queued_error, Some(IndexError::Storage(message)) if message.contains("injected pin fault")),
        "deferred pin did not record its fault: {queued_error:?}"
    );

    // A still-faulted pin keeps deferring without failing the round.
    let repeated = catalog.recover_read_sessions(None, 16, &mut admission_budget())?;
    check_eq!(
        (repeated.abandoned_settled, repeated.abandoned_deferred),
        (0, 1)
    );

    // Once healthy, a later round settles it exactly once.
    set_pin_delete_fault(&catalog, None)?;
    let healed = catalog.recover_read_sessions(None, 16, &mut admission_budget())?;
    check_eq!(
        (healed.abandoned_settled, healed.abandoned_deferred),
        (1, 0)
    );
    check_eq!(pins_at(&catalog, 0)?, 0);
    check!(catalog.abandoned_read_pins()?.is_empty());
    let quiet = catalog.recover_read_sessions(None, 16, &mut admission_budget())?;
    check_eq!((quiet.abandoned_settled, quiet.abandoned_deferred), (0, 0));
    check_eq!(quiet.recovered, 0);
    drop(live);
    Ok(())
}
