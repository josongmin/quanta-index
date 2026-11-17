//! Registry finalization against the real Catalog. Each fault is injected only
//! after the query has produced a private payload, independently of call counts.

use super::*;
use quanta_index_catalog::CatalogClockPort;
use std::sync::atomic::AtomicU64;

struct Clock(AtomicU64);

impl CatalogClockPort for Clock {
    fn now_unix_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Payload(Arc<AtomicUsize>);

impl Drop for Payload {
    fn drop(&mut self) {
        let _previous = self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy)]
enum Fault {
    Expiry,
    ClockRegression,
    Commit,
}

#[test]
fn final_observation_refuses_private_payload_preserves_reader_error_and_drains_pin() -> TestResult {
    for fault in [Fault::Expiry, Fault::ClockRegression, Fault::Commit] {
        for reader_failed in [false, true] {
            let root = tempfile::tempdir()?;
            let clock = Arc::new(Clock(AtomicU64::new(1_000)));
            let catalog = Arc::new(SqliteCatalog::open_with_clock(
                root.path(),
                Duration::from_secs(2),
                clock.clone(),
            )?);
            let objects = Arc::new(FileObjectStore::open(root.path().join("objects"))?);
            let kernel = Kernel::new(
                KernelServices::new(
                    objects.clone(),
                    objects,
                    catalog.clone(),
                    Arc::new(FileScratchSpace::open_standalone(
                        root.path().join("scratch"),
                        16 * 1024 * 1024,
                    )?),
                    Duration::from_secs(30),
                )?,
                AdapterRegistry::new(),
            );
            let ViewSelection::Latest { workspace, epoch } = selection(1) else {
                return Err("fixture requires Latest selection".into());
            };
            let limits = WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 16 * 1024 * 1024,
                mutations: 100_000,
            };
            let mut work = WorkBudget::with_request(
                limits,
                RequestBudgetV1::for_duration(Duration::from_secs(30)),
            );
            let _genesis = kernel.create_workspace(workspace, epoch, &mut work)?;
            let registry =
                NativeReadSessions::new_unfunded_for_test(NativeReadSessionConfig::default())?;
            let opened = registry.open(42, [51; 32], selection(1), &work, |selected| {
                kernel.pin_view(selected, &work)
            })?;
            let drops = Arc::new(AtomicUsize::new(0));
            let primary = IndexError::CorruptObject("private reader failure".into());
            let result = registry.with_ticket(
                42,
                opened.session,
                &work,
                || Ok(()),
                |_ticket, _attempt, ()| {
                    match fault {
                        Fault::Expiry => clock.0.store(31_000, Ordering::SeqCst),
                        Fault::ClockRegression => clock.0.store(999, Ordering::SeqCst),
                        Fault::Commit => {
                            // A deferred FK fails COMMIT, after the clock UPDATE
                            // itself succeeds. This is not an UPDATE refusal.
                            rusqlite::Connection::open(catalog.path())
                                .map_err(|error| IndexError::Storage(error.to_string()))?
                                .execute_batch(
                                    "CREATE TABLE b83_commit_fault (
                                        parent INTEGER REFERENCES native_work_clock_v1(singleton)
                                        DEFERRABLE INITIALLY DEFERRED
                                     );
                                     CREATE TRIGGER b83_clock_commit_fault
                                     AFTER UPDATE ON native_work_clock_v1
                                     BEGIN INSERT INTO b83_commit_fault VALUES(2); END;",
                                )
                                .map_err(|error| IndexError::Storage(error.to_string()))?;
                            clock.0.store(1_001, Ordering::SeqCst);
                        }
                    }
                    if reader_failed {
                        Err(primary.clone())
                    } else {
                        Ok(Payload(Arc::clone(&drops)))
                    }
                },
            );
            if reader_failed {
                check!(matches!(result, Err(ref error) if *error == primary));
            } else {
                match fault {
                    Fault::Expiry => check!(matches!(result, Err(IndexError::SnapshotUnavailable))),
                    Fault::ClockRegression | Fault::Commit => {
                        check!(matches!(result, Err(IndexError::Storage(_))));
                    }
                }
                check_eq!(drops.load(Ordering::SeqCst), 1);
            }
            check_eq!(
                registry.status(42, opened.session, &work)?.state,
                NativeReadSessionState::Unknown
            );
            let query_calls = AtomicUsize::new(0);
            let refused: Result<(), IndexError> = registry.with_ticket(
                42,
                opened.session,
                &work,
                || Ok(()),
                |_ticket, _attempt, ()| {
                    let _previous = query_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            );
            check_eq!(refused, Err(IndexError::SnapshotUnavailable));
            check_eq!(query_calls.load(Ordering::SeqCst), 0);
            if matches!(fault, Fault::Commit) {
                rusqlite::Connection::open(catalog.path())?.execute_batch(
                    "DROP TRIGGER b83_clock_commit_fault; DROP TABLE b83_commit_fault;",
                )?;
            }
            clock.0.store(31_000, Ordering::SeqCst);
            let report = registry.drain(&work, |ticket| {
                ticket.try_release()?;
                Ok(false)
            })?;
            check_eq!(report.released, 1);
            check_eq!(report.failures, 0);
            let pins: i64 = rusqlite::Connection::open(catalog.path())?.query_row(
                "SELECT COUNT(*) FROM native_pin_v1",
                [],
                |row| row.get(0),
            )?;
            check_eq!(pins, 0);
        }
    }
    Ok(())
}
