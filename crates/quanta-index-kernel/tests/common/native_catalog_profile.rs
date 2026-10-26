//! Opt-in fixture-local aggregate SQL profiling; no statement text is retained.

use std::{cell::Cell, marker::PhantomData, rc::Rc, time::Duration};

use quanta_index_catalog::SqliteCatalog;
use rusqlite::trace::TraceEvent;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Bucket {
    pub calls: u64,
    pub elapsed: Duration,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Snapshot {
    pub read: Bucket,
    pub write: Bucket,
    pub commit: Bucket,
    pub other: Bucket,
    epoch: u64,
    failed: bool,
}

thread_local! {
    static PROFILE: Cell<Option<Snapshot>> = const { Cell::new(None) };
    static PROFILE_EPOCH: Cell<u64> = const { Cell::new(0) };
}

pub(super) struct Guard {
    _thread: PhantomData<Rc<()>>,
}

impl Guard {
    pub(super) fn start() -> Result<Self, &'static str> {
        PROFILE.with(|profile| {
            if profile.get().is_some() {
                return Err("catalog profile already active on this fixture thread");
            }
            let epoch = PROFILE_EPOCH.with(|epoch| {
                let next = epoch
                    .get()
                    .checked_add(1)
                    .ok_or("SQL profile epoch overflow")?;
                epoch.set(next);
                Ok::<_, &'static str>(next)
            })?;
            profile.set(Some(Snapshot {
                epoch,
                ..Snapshot::default()
            }));
            Ok(Self {
                _thread: PhantomData,
            })
        })
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        PROFILE.with(|profile| profile.set(None));
    }
}

pub(super) fn attach(catalog: &SqliteCatalog) -> Result<(), quanta_index_core::CoreError> {
    if PROFILE.with(|profile| profile.get().is_some()) {
        catalog.set_statement_profile(Some(record))?;
    }
    Ok(())
}

pub(super) fn snapshot() -> Result<Option<Snapshot>, &'static str> {
    let current = PROFILE.with(Cell::get);
    if current.is_some_and(|current| current.failed) {
        return Err("catalog SQL profile overflow or invalid event");
    }
    Ok(current)
}

fn record(event: TraceEvent<'_>) {
    PROFILE.with(|profile| {
        let Some(mut current) = profile.get() else {
            return;
        };
        let TraceEvent::Profile(statement, elapsed) = event else {
            current.failed = true;
            profile.set(Some(current));
            return;
        };
        let sql = statement.sql();
        let Some(command) = sql.split_ascii_whitespace().next() else {
            current.failed = true;
            profile.set(Some(current));
            return;
        };
        let bucket = match command {
            "SELECT" | "WITH" => &mut current.read,
            "INSERT" | "UPDATE" | "DELETE" | "REPLACE" => &mut current.write,
            "COMMIT" => &mut current.commit,
            _ => &mut current.other,
        };
        match (
            bucket.calls.checked_add(1),
            bucket.elapsed.checked_add(elapsed),
        ) {
            (Some(calls), Some(total)) => {
                bucket.calls = calls;
                bucket.elapsed = total;
            }
            _ => current.failed = true,
        }
        profile.set(Some(current));
    });
}

impl Snapshot {
    fn checked_since(self, before: Self) -> Result<Self, &'static str> {
        fn subtract(current: Bucket, before: Bucket) -> Result<Bucket, &'static str> {
            Ok(Bucket {
                calls: current
                    .calls
                    .checked_sub(before.calls)
                    .ok_or("SQL count regression")?,
                elapsed: current
                    .elapsed
                    .checked_sub(before.elapsed)
                    .ok_or("SQL clock regression")?,
            })
        }
        if self.failed || before.failed {
            return Err("failed SQL profile cannot yield a measurement");
        }
        if self.epoch != before.epoch {
            return Err("SQL observation session changed during a measured phase");
        }
        Ok(Self {
            read: subtract(self.read, before.read)?,
            write: subtract(self.write, before.write)?,
            commit: subtract(self.commit, before.commit)?,
            other: subtract(self.other, before.other)?,
            epoch: self.epoch,
            failed: false,
        })
    }
}

pub(super) fn since(before: Option<Snapshot>) -> Result<Option<Snapshot>, &'static str> {
    match (before, snapshot()?) {
        (None, None) => Ok(None),
        (Some(before), Some(current)) => current.checked_since(before).map(Some),
        _ => Err("SQL observation mode changed during a measured phase"),
    }
}

#[test]
fn overflow_and_changed_observation_mode_cannot_yield_a_profile()
-> Result<(), Box<dyn std::error::Error>> {
    let guard = Guard::start()?;
    let broken = Snapshot {
        read: Bucket {
            calls: u64::MAX,
            elapsed: Duration::ZERO,
        },
        ..snapshot()?.ok_or("missing active profile")?
    };
    PROFILE.with(|profile| profile.set(Some(broken)));
    let connection = rusqlite::Connection::open_in_memory()?;
    connection.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,
        Some(record),
    );
    connection.execute_batch("SELECT 1;")?;
    assert!(snapshot().is_err());
    drop(guard);
    assert!(snapshot()?.is_none());
    assert!(since(Some(Snapshot::default())).is_err());
    Ok(())
}

#[test]
fn restarting_observation_cannot_accept_a_previous_session_baseline()
-> Result<(), Box<dyn std::error::Error>> {
    let first = Guard::start()?;
    let baseline = snapshot()?;
    drop(first);
    let second = Guard::start()?;
    let connection = rusqlite::Connection::open_in_memory()?;
    connection.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,
        Some(record),
    );
    connection.execute_batch("SELECT 1;")?;
    // Counts increased, so counter subtraction alone would accept this reset.
    assert!(since(baseline).is_err());
    drop(second);
    assert!(snapshot()?.is_none());
    Ok(())
}
