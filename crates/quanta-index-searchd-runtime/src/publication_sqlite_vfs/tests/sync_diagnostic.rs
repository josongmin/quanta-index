use super::*;
use rusqlite::ffi::quanta_capacity_vfs::NativeSyncObservation;
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1,
    DiagnosticRootKindV1, DiagnosticStageV1, DiagnosticTerminalV1,
    index::{WorkBudget, WorkLimits},
};

#[derive(Debug, Default)]
struct Clock(AtomicU64);
impl DiagnosticClockV1 for Clock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}
#[derive(Debug, Default)]
struct Events(Mutex<Vec<DiagnosticEmitV1>>);
impl DiagnosticEventSinkV1 for Events {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut events) = self.0.try_lock() else {
            return false;
        };
        if events.len() >= 256 {
            return false;
        }
        events.push(event);
        true
    }
}

#[derive(Debug)]
struct ReenteringSink {
    owner: NonZeroU64,
    calls: AtomicU64,
    mask_failures: AtomicU64,
    events: Mutex<Vec<DiagnosticEmitV1>>,
}
impl DiagnosticEventSinkV1 for ReenteringSink {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        // This must never observe a live RefCell borrow held by write scope
        // entry, sync callbacks, callback aggregation, or scope teardown.
        let _context = diagnostic::context(self.owner);
        {
            let _masked = diagnostic::Scope::enter(self.owner, None);
            if diagnostic::context(self.owner).is_some() {
                let _previous = self.mask_failures.fetch_add(1, Ordering::Relaxed);
                return false;
            }
        }
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
        let _previous = self.calls.fetch_add(1, Ordering::Relaxed);
        true
    }
}

#[derive(Debug)]
struct ReenteringClock {
    owner: NonZeroU64,
    ticks: AtomicU64,
}

impl DiagnosticClockV1 for ReenteringClock {
    fn now_ns(&self) -> u64 {
        let _context = diagnostic::context(self.owner);
        self.ticks.fetch_add(1, Ordering::Relaxed)
    }
}

#[test]
fn native_write_sink_reentry_never_borrows_active_scope() -> TestResult {
    let owner = NonZeroU64::new(7).ok_or("zero scope")?;
    let sink = Arc::new(ReenteringSink {
        owner,
        calls: AtomicU64::new(0),
        mask_failures: AtomicU64::new(0),
        events: Mutex::new(Vec::new()),
    });
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([102; 16]),
        Arc::new(Clock::default()),
        sink.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        diagnostic::write(
            owner,
            rusqlite::ffi::SQLITE_OPEN_WAL,
            4,
            rusqlite::ffi::SQLITE_OK,
        );
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    require(
        sink.calls.load(Ordering::Relaxed) >= 9,
        "reentrant sink did not observe aggregate begin/counters/end",
    )?;
    require(
        diagnostic::context(owner).is_none(),
        "reentrant sink leaked the completed scope",
    )?;
    require(
        sink.mask_failures.load(Ordering::Relaxed) == 0,
        "reentrant sink observed an unmasked nested scope",
    )
}

#[test]
fn native_sync_clock_and_sink_reentry_preserve_terminal_and_health() -> TestResult {
    let owner = NonZeroU64::new(8).ok_or("zero scope")?;
    let sink = Arc::new(ReenteringSink {
        owner,
        calls: AtomicU64::new(0),
        mask_failures: AtomicU64::new(0),
        events: Mutex::new(Vec::new()),
    });
    let clock = Arc::new(ReenteringClock {
        owner,
        ticks: AtomicU64::new(0),
    });
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([103; 16]),
        clock.clone(),
        sink.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        diagnostic::before(owner, 17, rusqlite::ffi::SQLITE_OPEN_WAL, || {
            diagnostic::MutationClass::Overwrite
        });
        diagnostic::after(
            owner,
            17,
            rusqlite::ffi::SQLITE_OK,
            NativeSyncObservation::default(),
            true,
        );
        let settled = diagnostic::finish_settlement(
            diagnostic::settlement(owner, rusqlite::ffi::SQLITE_OPEN_WAL),
            rusqlite::ffi::SQLITE_OK,
            Result::<(), Refusal>::Ok(()),
        );
        require(settled.is_ok(), "normal settlement changed result")?;

        diagnostic::before(owner, 18, rusqlite::ffi::SQLITE_OPEN_MAIN_DB, || {
            diagnostic::MutationClass::Growth
        });
        diagnostic::before(owner, 19, rusqlite::ffi::SQLITE_OPEN_MAIN_DB, || {
            diagnostic::MutationClass::Truncate
        });
        diagnostic::after(
            owner,
            20,
            rusqlite::ffi::SQLITE_IOERR_FSYNC,
            NativeSyncObservation::default(),
            true,
        );
        diagnostic::before(owner, 21, rusqlite::ffi::SQLITE_OPEN_MAIN_DB, || {
            diagnostic::MutationClass::NoMutation
        });
        diagnostic::after(
            owner,
            21,
            rusqlite::ffi::SQLITE_IOERR_FSYNC,
            NativeSyncObservation::default(),
            false,
        );
    }
    require(
        diagnostic::context(owner).is_none(),
        "scope was not restored",
    )?;
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let events = sink
        .events
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let sync_begins = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::SpanBegin {
                    stage: DiagnosticStageV1::SqliteWork,
                    lane: DiagnosticLaneV1(11 | 12),
                    ..
                }
            )
        })
        .count();
    require(sync_begins == 4, "sync begin denominator changed")?;
    require(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::SpanEnd {
                        outcome: DiagnosticTerminalV1::Aborted,
                        ..
                    }
                )
            })
            .count()
            == 2,
        "replaced or foreign-handle sync did not abort",
    )?;
    require(
        events
            .iter()
            .filter(|event| {
                matches!(event.kind, DiagnosticEventKindV1::SpanEnd {
            outcome: DiagnosticTerminalV1::Error { code }, ..
        } if code == rusqlite::ffi::SQLITE_IOERR_FSYNC.unsigned_abs())
            })
            .count()
            == 1,
        "failed native sync terminal changed",
    )?;
    require(
        events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Health {
                        code: quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan
                            | quanta_index_core::DiagnosticHealthCodeV1::ForeignCaptureSpan,
                    }
                )
            })
            .count()
            == 2,
        "replaced or foreign-handle health changed",
    )?;
    drop(events);
    require(
        clock.ticks.load(Ordering::Relaxed) > 0 && sink.calls.load(Ordering::Relaxed) > 0,
        "clock or sink did not exercise reentry",
    )?;
    require(
        sink.mask_failures.load(Ordering::Relaxed) == 0,
        "nested sink scope did not mask its parent",
    )
}

#[test]
fn native_sync_reentered_record_scope_does_not_receive_outer_span() -> TestResult {
    let owner = NonZeroU64::new(9).ok_or("zero scope")?;
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([104; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let outer = diagnostic::Scope::enter(owner, Some(&child));
    let mut nested: Option<diagnostic::Scope> = None;
    diagnostic::before(owner, 31, rusqlite::ffi::SQLITE_OPEN_WAL, || {
        nested = Some(diagnostic::Scope::enter(owner, Some(&child)));
        diagnostic::MutationClass::Overwrite
    });
    require(
        nested.is_some(),
        "mutation did not retain nested Record scope",
    )?;
    // The nested Record scope has no sync for handle31. An incorrect marker
    // check would install the outer span here and turn this into a success.
    diagnostic::after(
        owner,
        31,
        rusqlite::ffi::SQLITE_OK,
        NativeSyncObservation::default(),
        true,
    );
    drop(nested.take());
    require(
        diagnostic::context(owner).is_some(),
        "nested scope did not restore outer Record scope",
    )?;
    diagnostic::before(owner, 32, rusqlite::ffi::SQLITE_OPEN_WAL, || {
        diagnostic::MutationClass::NoMutation
    });
    diagnostic::after(
        owner,
        32,
        rusqlite::ffi::SQLITE_OK,
        NativeSyncObservation::default(),
        true,
    );
    drop(outer);
    require(
        diagnostic::context(owner).is_none(),
        "outer Record scope leaked",
    )?;
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let sync_begins = rows
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::SpanBegin {
                    stage: DiagnosticStageV1::SqliteWork,
                    lane: DiagnosticLaneV1(11),
                    ..
                }
            )
        })
        .count();
    let sync_ok = rows
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::SpanEnd {
                    outcome: DiagnosticTerminalV1::Ok,
                    ..
                }
            )
        })
        .count();
    let aborted = rows
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::SpanEnd {
                    outcome: DiagnosticTerminalV1::Aborted,
                    ..
                }
            )
        })
        .count();
    let missing = rows
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::Health {
                    code: quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan,
                }
            )
        })
        .count();
    require(sync_begins == 2, "outer/next sync begin count differs")?;
    // Two write aggregate spans and the operation root also end Ok.
    require(sync_ok == 4, "nested/outer normal terminal count differs")?;
    require(aborted == 1, "uninstalled outer sync was not aborted")?;
    require(
        missing == 2,
        "nested missing sync or uninstalled span lacked health",
    )?;
    require(
        rows.iter()
            .filter(|event| matches!(event.kind, DiagnosticEventKindV1::Health { .. }))
            .count()
            == 2,
        "nested Record scope emitted unexpected health",
    )?;
    require(
        rows.iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Counter {
                        key: DiagnosticCounterV1::SqliteSyncNoMutation,
                        value: 1,
                        ..
                    }
                )
            })
            .count()
            == 1,
        "restored outer scope lost its next sync",
    )?;
    drop(rows);
    Ok(())
}

#[test]
fn native_sync_same_public_capture_id_does_not_bridge_distinct_sessions() -> TestResult {
    let owner = NonZeroU64::new(11).ok_or("zero scope")?;
    let capture_id = DiagnosticCaptureIdV1([106; 16]);
    let outer_events = Arc::new(Events::default());
    let inner_events = Arc::new(Events::default());
    let outer_context =
        DiagnosticContextV1::new(capture_id, Arc::new(Clock::default()), outer_events.clone());
    let inner_context =
        DiagnosticContextV1::new(capture_id, Arc::new(Clock::default()), inner_events.clone());
    require(
        !outer_context.shares_capture_session(&inner_context),
        "fixture reused the same capture session",
    )?;
    let outer_root =
        outer_context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let inner_root =
        inner_context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    require(
        outer_root.span_id() == inner_root.span_id(),
        "independent root span IDs did not collide",
    )?;
    let outer_child = outer_context.child_context(&outer_root);
    let inner_child = inner_context.child_context(&inner_root);
    let outer = diagnostic::Scope::enter(owner, Some(&outer_child));
    let mut nested: Option<diagnostic::Scope> = None;
    diagnostic::before(owner, 51, rusqlite::ffi::SQLITE_OPEN_WAL, || {
        nested = Some(diagnostic::Scope::enter(owner, Some(&inner_child)));
        diagnostic::MutationClass::Overwrite
    });
    require(
        nested.is_some(),
        "foreign session did not retain nested scope",
    )?;
    diagnostic::after(
        owner,
        51,
        rusqlite::ffi::SQLITE_OK,
        NativeSyncObservation::default(),
        true,
    );
    drop(nested.take());
    require(
        diagnostic::context(owner)
            .as_ref()
            .is_some_and(|current| current.shares_capture_session(&outer_context)),
        "foreign session did not restore outer scope",
    )?;
    diagnostic::before(owner, 52, rusqlite::ffi::SQLITE_OPEN_WAL, || {
        diagnostic::MutationClass::NoMutation
    });
    diagnostic::after(
        owner,
        52,
        rusqlite::ffi::SQLITE_OK,
        NativeSyncObservation::default(),
        true,
    );
    drop(outer);
    require(
        diagnostic::context(owner).is_none(),
        "outer scope leaked after foreign nested session",
    )?;
    let _outer_finished = outer_root.finish(DiagnosticTerminalV1::Ok);
    let _inner_finished = inner_root.finish(DiagnosticTerminalV1::Ok);
    let outer_rows = outer_events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let inner_rows = inner_events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let write_span = |rows: &[DiagnosticEmitV1]| {
        rows.iter().find_map(|event| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(22),
                ..
            } = event.kind
            {
                Some(span_id)
            } else {
                None
            }
        })
    };
    require(
        write_span(&outer_rows) == write_span(&inner_rows) && write_span(&outer_rows).is_some(),
        "separate sessions did not reproduce equal write span IDs",
    )?;
    require(
        outer_rows
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::SqliteWork,
                        lane: DiagnosticLaneV1(11),
                        ..
                    }
                )
            })
            .count()
            == 2,
        "outer session sync denominator changed",
    )?;
    require(
        outer_rows
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::SpanEnd {
                        outcome: DiagnosticTerminalV1::Aborted,
                        ..
                    }
                )
            })
            .count()
            == 1
            && outer_rows
                .iter()
                .filter(|event| {
                    matches!(
                        event.kind,
                        DiagnosticEventKindV1::Counter {
                            key: DiagnosticCounterV1::SqliteSyncNoMutation,
                            value: 1,
                            ..
                        }
                    )
                })
                .count()
                == 1,
        "outer session lost aborted refusal or restored normal sync",
    )?;
    drop(outer_rows);
    require(
        !inner_rows.iter().any(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::SpanBegin {
                    stage: DiagnosticStageV1::SqliteWork,
                    lane: DiagnosticLaneV1(11),
                    ..
                }
            )
        }),
        "outer sync span was attached to foreign session",
    )?;
    require(
        inner_rows
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Health {
                        code: quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan,
                    }
                )
            })
            .count()
            == 1,
        "foreign session unexpectedly accepted outer sync",
    )?;
    drop(inner_rows);
    Ok(())
}

#[test]
fn native_sync_reentered_pending_handle_survives_outer_before() -> TestResult {
    let owner = NonZeroU64::new(10).ok_or("zero scope")?;
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([105; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        diagnostic::before(owner, 41, rusqlite::ffi::SQLITE_OPEN_WAL, || {
            diagnostic::before(owner, 42, rusqlite::ffi::SQLITE_OPEN_WAL, || {
                diagnostic::MutationClass::Growth
            });
            diagnostic::MutationClass::Overwrite
        });
        diagnostic::after(
            owner,
            42,
            rusqlite::ffi::SQLITE_OK,
            NativeSyncObservation::default(),
            true,
        );
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    require(
        rows.iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::SqliteWork,
                        lane: DiagnosticLaneV1(11),
                        ..
                    }
                )
            })
            .count()
            == 2,
        "reentered pending span begin count differs",
    )?;
    require(
        rows.iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::SpanEnd {
                        outcome: DiagnosticTerminalV1::Aborted,
                        ..
                    }
                )
            })
            .count()
            == 1,
        "outer span overwrote pending inner span",
    )?;
    require(
        rows.iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::SpanEnd {
                        outcome: DiagnosticTerminalV1::Ok,
                        ..
                    }
                )
            })
            .count()
            == 3,
        "inner sync, write aggregate or root did not end normally",
    )?;
    require(
        rows.iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Counter {
                        key: DiagnosticCounterV1::SqliteSyncGrowth,
                        value: 1,
                        ..
                    }
                )
            })
            .count()
            == 1
            && !rows.iter().any(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Counter {
                        key: DiagnosticCounterV1::SqliteSyncOverwrite,
                        ..
                    }
                )
            }),
        "outer mutation replaced authenticated inner class",
    )?;
    require(
        rows.iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Health {
                        code: quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan,
                    }
                )
            })
            .count()
            == 1,
        "uninstalled outer span lacked one health event",
    )?;
    require(
        rows.iter()
            .filter(|event| matches!(event.kind, DiagnosticEventKindV1::Health { .. }))
            .count()
            == 1,
        "reentered pending sync emitted unexpected health",
    )?;
    drop(rows);
    Ok(())
}

#[test]
fn native_sync_mutation_attempt_classes_keep_conservative_history() -> TestResult {
    use crate::publication_sqlite_vfs::diagnostic::MutationClass as Class;
    // Fixed byte/operation gold: a later short write cannot erase an earlier
    // growth, truncate or unknown observation in the same unsettled mutation.
    let mut mutation = Class::NoMutation;
    mutation.write(Some(64), Some(64));
    require(
        mutation == Class::Overwrite,
        "in-range write was not overwrite",
    )?;
    mutation.write(Some(64), Some(65));
    mutation.write(Some(65), Some(1));
    require(mutation == Class::Growth, "later write erased prior growth")?;
    mutation.truncate();
    mutation.write(Some(0), Some(65));
    require(
        mutation == Class::Truncate,
        "regrow erased truncate history",
    )?;
    mutation.write(None, Some(1));
    mutation.write(Some(65), Some(1));
    mutation.truncate();
    require(
        mutation == Class::Unclassified,
        "missing witness became known",
    )?;
    let mut created = Class::Create;
    created.write(Some(0), Some(1));
    created.truncate();
    require(
        created == Class::Create,
        "new file was counted as stable overwrite",
    )?;
    created.write(Some(1), None);
    require(
        created == Class::Unclassified,
        "invalid extent became known",
    )
}

#[test]
fn native_sync_class_probe_is_off_for_foreign_scopes_and_unlinked_is_unknown() -> TestResult {
    let owner = NonZeroU64::new(1).ok_or("zero scope")?;
    let foreign = NonZeroU64::new(2).ok_or("zero scope")?;
    let reads = AtomicU64::new(0);
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([96; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let scope = diagnostic::Scope::enter(owner, Some(&child));
    let class = || {
        let _previous = reads.fetch_add(1, Ordering::Relaxed);
        diagnostic::MutationClass::Overwrite
    };
    {
        let _off = diagnostic::Scope::enter(owner, None);
        diagnostic::before(owner, 1, rusqlite::ffi::SQLITE_OPEN_WAL, class);
    }
    diagnostic::before(foreign, 1, rusqlite::ffi::SQLITE_OPEN_WAL, class);
    require(
        reads.load(Ordering::Relaxed) == 0,
        "Off/foreign acquired class state",
    )?;
    diagnostic::before(owner, 1, rusqlite::ffi::SQLITE_OPEN_WAL, class);
    diagnostic::after(
        owner,
        1,
        rusqlite::ffi::SQLITE_IOERR_FSYNC,
        NativeSyncObservation::default(),
        true,
    );
    diagnostic::before(owner, 1, rusqlite::ffi::SQLITE_OPEN_WAL, class);
    diagnostic::after(
        owner,
        1,
        rusqlite::ffi::SQLITE_OK,
        NativeSyncObservation::default(),
        false,
    );
    drop(scope);
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    require(
        reads.load(Ordering::Relaxed) == 2,
        "native class probe duplicated",
    )?;
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let classes: Vec<_> = rows
        .iter()
        .filter_map(|event| {
            if let DiagnosticEventKindV1::Counter { key, value, .. } = event.kind {
                matches!(
                    key,
                    DiagnosticCounterV1::SqliteSyncOverwrite
                        | DiagnosticCounterV1::SqliteSyncGrowth
                        | DiagnosticCounterV1::SqliteSyncTruncate
                        | DiagnosticCounterV1::SqliteSyncCreate
                        | DiagnosticCounterV1::SqliteSyncNoMutation
                        | DiagnosticCounterV1::SqliteSyncUnclassified
                )
                .then_some((key, value))
            } else {
                None
            }
        })
        .collect();
    let write_spans: Vec<_> = rows
        .iter()
        .filter_map(|event| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(22),
                ..
            } = event.kind
            {
                Some(span_id)
            } else {
                None
            }
        })
        .collect();
    require(
        write_spans.len() == 1,
        "zero-write scope span missing or duplicated",
    )?;
    let write_span = write_spans
        .first()
        .copied()
        .ok_or("zero-write scope span missing")?;
    let write_counters: Vec<_> = rows
        .iter()
        .filter_map(|event| {
            if let DiagnosticEventKindV1::Counter {
                span_id,
                key,
                value,
            } = event.kind
                && span_id == write_span
            {
                Some((key, value))
            } else {
                None
            }
        })
        .collect();
    require(
        write_counters
            == [
                (DiagnosticCounterV1::SqliteWriteCalls, 0),
                (DiagnosticCounterV1::SqliteWriteRequestedBytes, 0),
                (DiagnosticCounterV1::SqliteWriteFailures, 0),
                (DiagnosticCounterV1::SqliteWalWriteCalls, 0),
                (DiagnosticCounterV1::SqliteMainWriteCalls, 0),
                (DiagnosticCounterV1::SqliteOtherWriteCalls, 0),
            ],
        "zero-write scope counters differ",
    )?;
    require(
        rows.iter()
            .filter(|event| matches!(event.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Ok } if span_id == write_span))
            .count()
            == 1,
        "zero-write scope did not end exactly once",
    )?;
    drop(rows);
    require(
        classes
            == [
                (DiagnosticCounterV1::SqliteSyncOverwrite, 1),
                (DiagnosticCounterV1::SqliteSyncUnclassified, 1),
            ],
        "native class duplicated or unlinked file treated as overwrite",
    )?;
    Ok(())
}

#[test]
fn native_write_scope_uses_exact_additive_gold_and_restores_nested_owner() -> TestResult {
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([98; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let owner = NonZeroU64::new(1).ok_or("zero scope")?;
    let foreign = NonZeroU64::new(2).ok_or("zero scope")?;
    let outer = diagnostic::Scope::enter(owner, Some(&child));
    diagnostic::write(
        owner,
        rusqlite::ffi::SQLITE_OPEN_WAL,
        12,
        rusqlite::ffi::SQLITE_OK,
    );
    let before_mask = clock.0.load(Ordering::Relaxed);
    {
        let _off = diagnostic::Scope::enter(owner, None);
        diagnostic::write(
            owner,
            rusqlite::ffi::SQLITE_OPEN_WAL,
            99,
            rusqlite::ffi::SQLITE_OK,
        );
        diagnostic::write(foreign, 0, 99, rusqlite::ffi::SQLITE_OK);
    }
    diagnostic::write(foreign, 0, 99, rusqlite::ffi::SQLITE_OK);
    require(
        clock.0.load(Ordering::Relaxed) == before_mask,
        "Off/foreign write touched the clock",
    )?;
    {
        let _nested = diagnostic::Scope::enter(foreign, Some(&child));
        diagnostic::write(foreign, 0, 5, rusqlite::ffi::SQLITE_OK);
        diagnostic::write(owner, 0, 99, rusqlite::ffi::SQLITE_OK);
    }
    diagnostic::write(
        owner,
        rusqlite::ffi::SQLITE_OPEN_MAIN_DB,
        8,
        rusqlite::ffi::SQLITE_IOERR_WRITE,
    );
    diagnostic::write(owner, 0, 3, rusqlite::ffi::SQLITE_OK);
    drop(outer);
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let spans: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(22),
                ..
            } = row.kind
            {
                Some(span_id)
            } else {
                None
            }
        })
        .collect();
    require(
        spans.len() == 2,
        "write scope duplicated or Off scope emitted",
    )?;
    let keys = [
        DiagnosticCounterV1::SqliteWriteCalls,
        DiagnosticCounterV1::SqliteWriteRequestedBytes,
        DiagnosticCounterV1::SqliteWriteFailures,
        DiagnosticCounterV1::SqliteWalWriteCalls,
        DiagnosticCounterV1::SqliteMainWriteCalls,
        DiagnosticCounterV1::SqliteOtherWriteCalls,
    ];
    for (span, gold) in spans
        .into_iter()
        .zip([[3, 23, 1, 1, 1, 1], [1, 5, 0, 0, 0, 1]])
    {
        let actual: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                if let DiagnosticEventKindV1::Counter {
                    span_id,
                    key,
                    value,
                } = row.kind
                    && span_id == span
                {
                    Some((key, value))
                } else {
                    None
                }
            })
            .collect();
        require(
            actual == keys.into_iter().zip(gold).collect::<Vec<_>>(),
            "native write six-counter gold differs",
        )?;
        require(
            rows.iter().filter(|row| matches!(row.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Ok } if span_id == span)).count() == 1,
            "write aggregate did not end exactly once",
        )?;
    }
    require(
        !rows
            .iter()
            .any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
        "normal write aggregate invalidated trace",
    )
}

#[test]
fn native_write_invalid_amount_or_overflow_reports_health_without_partial_counters() -> TestResult {
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([99; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let owner = NonZeroU64::new(1).ok_or("zero scope")?;
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        diagnostic::write(owner, 0, -1, rusqlite::ffi::SQLITE_OK);
    }
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        for amount in [i64::MAX, i64::MAX, 2] {
            diagnostic::write(owner, 0, amount, rusqlite::ffi::SQLITE_OK);
        }
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    require(
        rows.iter()
            .filter(|row| {
                matches!(
                    row.kind,
                    DiagnosticEventKindV1::Health {
                        code: quanta_index_core::DiagnosticHealthCodeV1::CounterOverflow
                    }
                )
            })
            .count()
            == 2,
        "invalid/overflow requested bytes lacked health",
    )?;
    require(
        !rows.iter().any(|row| {
            matches!(
                row.kind,
                DiagnosticEventKindV1::Counter {
                    key: DiagnosticCounterV1::SqliteWriteCalls
                        | DiagnosticCounterV1::SqliteWriteRequestedBytes
                        | DiagnosticCounterV1::SqliteWriteFailures
                        | DiagnosticCounterV1::SqliteWalWriteCalls
                        | DiagnosticCounterV1::SqliteMainWriteCalls
                        | DiagnosticCounterV1::SqliteOtherWriteCalls,
                    ..
                }
            )
        }),
        "invalid aggregate emitted partial write counters",
    )
}

#[test]
fn native_write_unwinding_restores_scope_and_aborts_pending_sync() -> TestResult {
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([100; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let owner = NonZeroU64::new(1).ok_or("zero scope")?;
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        diagnostic::write(owner, 0, 4, rusqlite::ffi::SQLITE_IOERR_WRITE);
        diagnostic::before(owner, 7, rusqlite::ffi::SQLITE_OPEN_WAL, || {
            diagnostic::MutationClass::NoMutation
        });
        std::panic::resume_unwind(Box::new("injected unwind after native write"));
    }));
    require(caught.is_err(), "fixture did not unwind")?;
    require(diagnostic::context(owner).is_none(), "unwind leaked scope")?;
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    require(
        rows.iter().any(|row| {
            matches!(
                row.kind,
                DiagnosticEventKindV1::SpanEnd {
                    outcome: DiagnosticTerminalV1::Aborted,
                    ..
                }
            )
        }),
        "unwound sync was not aborted",
    )?;
    require(
        rows.iter().any(|row| {
            matches!(
                row.kind,
                DiagnosticEventKindV1::Counter {
                    key: DiagnosticCounterV1::SqliteWriteFailures,
                    value: 1,
                    ..
                }
            )
        }),
        "unwind hid reached native write",
    )
}

#[test]
fn real_sqlite_wal_and_checkpoint_emit_requested_write_aggregates() -> TestResult {
    let fixture = fixture(64 * 1024 * 1024)?;
    let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
    let catalog =
        SqliteCatalog::open_with_io_admission(&fixture.root, Duration::from_secs(1), admission)?;
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([101; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = fixture.vfs.enter_scope(Some(&child));
        let writer = Connection::open_with_flags_and_vfs(
            catalog.path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        writer.execute_batch(
            "PRAGMA wal_autocheckpoint=0;
             CREATE TABLE write_aggregate_gold(value INTEGER);
             INSERT INTO write_aggregate_gold VALUES(7);
             PRAGMA wal_checkpoint(PASSIVE);",
        )?;
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let actual: i64 = {
        let _off = fixture.vfs.enter_scope(None);
        let reader = Connection::open_with_flags_and_vfs(
            catalog.path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        reader.query_row("SELECT value FROM write_aggregate_gold", [], |row| {
            row.get(0)
        })?
    };
    require(
        actual == 7,
        "native write observation changed committed rows",
    )?;
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut counts = [0_u64; 6];
    for row in rows.iter() {
        if let DiagnosticEventKindV1::Counter { key, value, .. } = row.kind {
            let slot = [
                DiagnosticCounterV1::SqliteWriteCalls,
                DiagnosticCounterV1::SqliteWriteRequestedBytes,
                DiagnosticCounterV1::SqliteWriteFailures,
                DiagnosticCounterV1::SqliteWalWriteCalls,
                DiagnosticCounterV1::SqliteMainWriteCalls,
                DiagnosticCounterV1::SqliteOtherWriteCalls,
            ]
            .iter()
            .position(|candidate| *candidate == key);
            if let Some(slot) = slot {
                let count = counts
                    .get_mut(slot)
                    .ok_or("write counter slot out of range")?;
                *count = count.checked_add(value).ok_or("write counter overflow")?;
            }
        }
    }
    let [calls, requested, failures, wal, main, other] = counts;
    let classified = wal
        .checked_add(main)
        .and_then(|sum| sum.checked_add(other))
        .ok_or("classified write count overflow")?;
    require(
        calls > 0 && requested > 0 && failures == 0 && wal > 0 && main > 0 && calls == classified,
        "real WAL/main requested writes lacked additive classification",
    )?;
    require(
        rows.iter().any(|row| {
            matches!(
                row.kind,
                DiagnosticEventKindV1::SpanBegin {
                    stage: DiagnosticStageV1::SqliteWork,
                    lane: DiagnosticLaneV1(22),
                    ..
                }
            )
        }),
        "reached native write lacked aggregate span",
    )?;
    drop(rows);
    drop(catalog);
    fixture.vfs.drain_retirements()?;
    require(
        fixture.common.is_quiescent()?,
        "write aggregate fixture retained a native IO handle",
    )
}

// Test-only schema equivalent to native_attempt_object_v1 for the property
// under test: a WITHOUT ROWID composite key and two immediate FKs.
// It intentionally omits Catalog ownership/reclaim rows, so this is a VFS
// statement-journal discriminator, not a managed-confirm product benchmark.
fn protection_write_arm(packet: bool) -> Result<[u64; 6], Box<dyn std::error::Error>> {
    const TARGETS: usize = 64;
    let fixture = fixture(64 * 1024 * 1024)?;
    let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
    let catalog =
        SqliteCatalog::open_with_io_admission(&fixture.root, Duration::from_secs(1), admission)?;
    let path = catalog.path().to_path_buf();
    let _off = fixture.vfs.enter_scope(None);
    let writer = Connection::open_with_flags_and_vfs(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        fixture.vfs.name(),
    )?;
    writer.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA fullfsync=ON;
         PRAGMA foreign_keys=ON;
         PRAGMA wal_autocheckpoint=0;
         CREATE TABLE diag_operation(operation BLOB PRIMARY KEY NOT NULL) WITHOUT ROWID;
         CREATE TABLE diag_object(object BLOB PRIMARY KEY NOT NULL) WITHOUT ROWID;
         CREATE TABLE diag_attempt_object(
           operation BLOB NOT NULL,
           fence INTEGER NOT NULL,
           object BLOB NOT NULL,
           row_digest BLOB NOT NULL,
           PRIMARY KEY(operation, fence, object),
           FOREIGN KEY(operation) REFERENCES diag_operation(operation),
           FOREIGN KEY(object) REFERENCES diag_object(object)
         ) WITHOUT ROWID;
         INSERT INTO diag_operation VALUES(zeroblob(32));
         WITH RECURSIVE seq(n) AS
           (VALUES(0) UNION ALL SELECT n+1 FROM seq WHERE n<16383)
         INSERT INTO diag_object
           SELECT CAST(printf('%032x',n) AS BLOB) FROM seq;
         WITH RECURSIVE seq(n) AS
           (VALUES(0) UNION ALL SELECT n+1 FROM seq WHERE n<8191)
         INSERT INTO diag_attempt_object
           SELECT zeroblob(32), 1, CAST(printf('%032x',2*n) AS BLOB), zeroblob(32)
           FROM seq;",
    )?;
    let journal_mode: String = writer.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let synchronous: i64 = writer.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
    let fullfsync: i64 = writer.query_row("PRAGMA fullfsync", [], |row| row.get(0))?;
    let foreign_keys: i64 = writer.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    require(
        journal_mode.eq_ignore_ascii_case("wal")
            && synchronous == 2
            && fullfsync == 1
            && foreign_keys == 1,
        "protection discriminator lacks Catalog WAL/FULL/fullfsync/FK settings",
    )?;
    let seeded: i64 = writer.query_row("SELECT count(*) FROM diag_attempt_object", [], |row| {
        row.get(0)
    })?;
    require(
        seeded == 8192,
        "protection discriminator seed count differs",
    )?;
    let seed_width: i64 = writer.query_row(
        "SELECT count(*) FROM diag_attempt_object
         WHERE length(operation)=32 AND length(object)=32 AND length(row_digest)=32",
        [],
        |row| row.get(0),
    )?;
    require(seed_width == seeded, "protection seed row width differs")?;

    // Dirty the existing leaf pages within the same transaction before the
    // target inserts. Dispersed target keys give the statement savepoint many
    // previously modified pages to preserve; the default SQLite cache and
    // statement-journal spill settings remain in force.
    writer.execute_batch("BEGIN IMMEDIATE")?;
    let predirty_digest = vec![0x5a_u8; 32];
    let operation = vec![0_u8; 32];
    let mut target_rows = Vec::with_capacity(TARGETS);
    for ordinal in 0..TARGETS {
        let base = ordinal.checked_mul(256).ok_or("seed key overflow")?;
        let seed_key = format!("{base:032x}").into_bytes();
        let changed = writer.execute(
            "UPDATE diag_attempt_object SET row_digest=?1
             WHERE operation=?2 AND fence=1 AND object=?3",
            rusqlite::params![&predirty_digest, &operation, &seed_key],
        )?;
        require(
            changed == 1,
            "protection discriminator predirty missed a row",
        )?;
        let target = base.checked_add(1).ok_or("target key overflow")?;
        let digest_value = target.checked_add(1).ok_or("target digest overflow")?;
        target_rows.push((
            format!("{target:032x}").into_bytes(),
            format!("{digest_value:032x}").into_bytes(),
        ));
    }

    // Prepare the fixed 64-row reference packet outside the measured scope.
    // The alternate arm executes the same ordered rows via one cached scalar
    // INSERT OR IGNORE, matching the proposed protection-write shape.
    let mut packet_sql = String::from(
        "INSERT OR IGNORE INTO diag_attempt_object(operation,fence,object,row_digest) VALUES ",
    );
    for ordinal in 0..TARGETS {
        if ordinal != 0 {
            packet_sql.push(',');
        }
        let key = ordinal
            .checked_mul(2)
            .and_then(|value| value.checked_add(3))
            .ok_or("packet parameter overflow")?;
        let digest = key.checked_add(1).ok_or("digest parameter overflow")?;
        write!(packet_sql, "(?1,?2,?{key},?{digest})")?;
    }
    let mut parameters: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(2 + 2 * TARGETS);
    let fence = 1_i64;
    parameters.push(&operation);
    parameters.push(&fence);
    for (key, digest) in &target_rows {
        parameters.push(key);
        parameters.push(digest);
    }

    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([103; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = fixture.vfs.enter_scope(Some(&child));
        if packet {
            let changed = writer.execute(&packet_sql, rusqlite::params_from_iter(&parameters))?;
            require(
                changed == TARGETS,
                "reference packet inserted wrong row count",
            )?;
        } else {
            for (key, digest) in &target_rows {
                let changed = writer.prepare_cached(
                    "INSERT OR IGNORE INTO diag_attempt_object(operation,fence,object,row_digest)
                     VALUES (?1,?2,?3,?4)",
                )?.execute(rusqlite::params![&operation, fence, key, digest])?;
                require(changed == 1, "cached scalar inserted wrong row count")?;
            }
        }
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    writer.execute_batch("COMMIT")?;
    writer.close().map_err(|(_writer, error)| error)?;
    let reader = Connection::open_with_flags_and_vfs(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE,
        fixture.vfs.name(),
    )?;
    let total: i64 = reader.query_row("SELECT count(*) FROM diag_attempt_object", [], |row| {
        row.get(0)
    })?;
    let expected_total = 8192_i64
        .checked_add(i64::try_from(TARGETS)?)
        .ok_or("protection total overflow")?;
    require(total == expected_total, "protection row total differs")?;
    let final_width: i64 = reader.query_row(
        "SELECT count(*) FROM diag_attempt_object
         WHERE length(operation)=32 AND length(object)=32 AND length(row_digest)=32",
        [],
        |row| row.get(0),
    )?;
    require(
        final_width == total,
        "protection committed row width differs",
    )?;
    for (key, digest) in &target_rows {
        let stored: Vec<u8> = reader.query_row(
            "SELECT row_digest FROM diag_attempt_object
             WHERE operation=?1 AND fence=1 AND object=?2",
            rusqlite::params![&operation, key],
            |row| row.get(0),
        )?;
        require(stored == *digest, "protection digest differs")?;
    }
    let fk_violations: i64 =
        reader.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    require(fk_violations == 0, "protection rows violated a foreign key")?;
    reader.close().map_err(|(_reader, error)| error)?;
    drop(catalog);
    fixture.vfs.drain_retirements()?;

    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    require(
        !rows
            .iter()
            .any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
        "protection write observation reported unhealthy capture",
    )?;
    let spans: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(22),
                ..
            } = row.kind
            {
                Some(span_id)
            } else {
                None
            }
        })
        .collect();
    require(spans.len() == 1, "protection arm lacks one write aggregate")?;
    let span = *spans.first().ok_or("protection write span missing")?;
    require(
        rows.iter().filter(|row| matches!(row.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Ok } if span_id == span)).count() == 1,
        "protection aggregate did not finish exactly once",
    )?;
    let keys = [
        DiagnosticCounterV1::SqliteWriteCalls,
        DiagnosticCounterV1::SqliteWriteRequestedBytes,
        DiagnosticCounterV1::SqliteWriteFailures,
        DiagnosticCounterV1::SqliteWalWriteCalls,
        DiagnosticCounterV1::SqliteMainWriteCalls,
        DiagnosticCounterV1::SqliteOtherWriteCalls,
    ];
    let mut counts = [None; 6];
    for row in rows.iter() {
        if let DiagnosticEventKindV1::Counter {
            span_id,
            key,
            value,
        } = row.kind
            && span_id == span
            && let Some(index) = keys.iter().position(|candidate| *candidate == key)
        {
            let count = counts.get_mut(index).ok_or("write counter slot missing")?;
            require(count.replace(value).is_none(), "duplicate write counter")?;
        }
    }
    let [
        Some(calls),
        Some(bytes),
        Some(failures),
        Some(wal),
        Some(main),
        Some(other),
    ] = counts
    else {
        return Err(std::io::Error::other("missing protection write counter").into());
    };
    let classified = wal
        .checked_add(main)
        .and_then(|value| value.checked_add(other))
        .ok_or("classified write count overflow")?;
    require(
        calls == classified && failures == 0,
        "protection write counters lack additive classification",
    )?;
    drop(rows);
    let counts = [calls, bytes, failures, wal, main, other];
    writeln!(
        std::io::stderr().lock(),
        "native-protection-write packet={packet} counts={counts:?}"
    )?;
    fixture
        .common
        .require_quiescent("protection discriminator after checked close")?;
    Ok(counts)
}

#[test]
fn real_protection_packet_vs_cached_scalar_exposes_statement_journal_writes() -> TestResult {
    let packet = protection_write_arm(true)?;
    let scalar = protection_write_arm(false)?;
    writeln!(
        std::io::stderr().lock(),
        "native-protection-write counts=[calls,requested_bytes,failures,wal,main,other] packet={packet:?} scalar={scalar:?}"
    )?;
    // "Other" includes all native writes outside WAL and main DB. A decrease
    // is evidence for this fixed fixture, not proof every such write was a
    // statement subjournal write or that end-to-end latency improves.
    if packet[5] > scalar[5] && packet[0] > scalar[0] && packet[1] > scalar[1] {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "native write reduction absent: packet={packet:?}, scalar={scalar:?}"
        ))
        .into())
    }
}

#[test]
fn native_sync_syscall_observation_preserves_borrowed_fd() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("native-sync-oracle");
    fs::write(&path, b"native sync oracle")?;
    let file = File::options().read(true).write(true).open(&path)?;
    quanta_capacity_vfs::test_native_sync_syscall_outcomes(&file)
        .map_err(|error| std::io::Error::other(format!("native sync oracle: {error:?}")))?;
    require(
        file.metadata()?.len() == 18,
        "native observation closed or changed its borrowed FD",
    )?;
    require(
        fs::read(path)? == b"native sync oracle",
        "native observation changed file contents",
    )
}

#[test]
fn real_wal_commit_and_budgeted_checkpoint_keep_off_result_and_cost() -> TestResult {
    let mut usages = Vec::new();
    for record in [false, true] {
        let fixture = fixture(64 * 1024 * 1024)?;
        let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
        let catalog = SqliteCatalog::open_with_io_admission(
            &fixture.root,
            Duration::from_secs(1),
            admission,
        )?;
        let clock = Arc::new(Clock::default());
        let events = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([91; 16]),
            clock.clone(),
            events.clone(),
        );
        let root = if record {
            Some(context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?)
        } else {
            None
        };
        let selected = root.as_ref().map(|root| context.child_context(root));
        {
            let _scope = fixture.vfs.enter_scope(selected.as_ref());
            let writer = Connection::open_with_flags_and_vfs(
                catalog.path(),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
                fixture.vfs.name(),
            )?;
            writer.execute_batch("PRAGMA wal_autocheckpoint=0; PRAGMA synchronous=FULL; CREATE TABLE sync_diagnostic_gold(n INTEGER); INSERT INTO sync_diagnostic_gold VALUES(6); PRAGMA wal_checkpoint(PASSIVE); UPDATE sync_diagnostic_gold SET n=7;")?;
        }
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let request = match selected {
            Some(context) => request.with_diagnostic_context(context),
            None => request,
        };
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 536_870_912,
                mutations: 1_000_000,
            },
            request,
        );
        let before_checkpoint = events
            .0
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .len();
        require(
            catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut budget)?,
            "budgeted checkpoint skipped the real WAL",
        )?;
        usages.push(budget.usage());
        if let Some(root) = root {
            let _finished = root.finish(DiagnosticTerminalV1::Ok);
        }
        {
            let _scope = fixture.vfs.enter_scope(None);
            let reader = Connection::open_with_flags_and_vfs(
                catalog.path(),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
                fixture.vfs.name(),
            )?;
            let gold: i64 =
                reader.query_row("SELECT n FROM sync_diagnostic_gold", [], |row| row.get(0))?;
            require(gold == 7, "sync observation changed committed contents")?;
        }
        let events = events
            .0
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        if record {
            let checkpoint = events
                .get(before_checkpoint..)
                .ok_or("diagnostic events regressed")?;
            require(
                checkpoint.iter().any(|event| {
                    matches!(
                        event.kind,
                        DiagnosticEventKindV1::SpanBegin {
                            stage: DiagnosticStageV1::SqliteWork,
                            lane: DiagnosticLaneV1(12),
                            ..
                        }
                    )
                }),
                "budgeted Catalog scope did not forward the original request to checkpoint sync",
            )?;
            for lane in [11, 12, 15, 16, 21] {
                let ids: Vec<_> = events
                    .iter()
                    .filter_map(|event| {
                        if let DiagnosticEventKindV1::SpanBegin {
                            span_id,
                            stage: DiagnosticStageV1::SqliteWork,
                            lane: DiagnosticLaneV1(observed),
                            ..
                        } = event.kind
                        {
                            return (observed == lane).then_some(span_id);
                        }
                        None
                    })
                    .collect();
                require(!ids.is_empty(), "native WAL/main sync boundary missing")?;
                for id in ids {
                    require(events.iter().filter(|event| matches!(event.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Ok } if span_id == id)).count() == 1, "native sync did not end exactly once")?;
                    if matches!(lane, 11 | 12) {
                        let classes: Vec<_> = events
                            .iter()
                            .filter_map(|event| {
                                if let DiagnosticEventKindV1::Counter {
                                    span_id,
                                    key,
                                    value,
                                } = event.kind
                                    && span_id == id
                                    && matches!(
                                        key,
                                        DiagnosticCounterV1::SqliteSyncOverwrite
                                            | DiagnosticCounterV1::SqliteSyncGrowth
                                            | DiagnosticCounterV1::SqliteSyncTruncate
                                            | DiagnosticCounterV1::SqliteSyncCreate
                                            | DiagnosticCounterV1::SqliteSyncNoMutation
                                            | DiagnosticCounterV1::SqliteSyncUnclassified
                                    )
                                {
                                    Some(value)
                                } else {
                                    None
                                }
                            })
                            .collect();
                        require(
                            classes == [1],
                            "native xSync has missing or duplicate mutation class",
                        )?;
                        let observations: Vec<_> = events
                            .iter()
                            .filter_map(|event| match event.kind {
                                DiagnosticEventKindV1::SqliteSyncObservation {
                                    span_id,
                                    observation,
                                } if span_id == id => Some(observation),
                                DiagnosticEventKindV1::SpanBegin { .. }
                                | DiagnosticEventKindV1::SpanEnd { .. }
                                | DiagnosticEventKindV1::RequestIdentity { .. }
                                | DiagnosticEventKindV1::PhaseIdentity { .. }
                                | DiagnosticEventKindV1::Counter { .. }
                                | DiagnosticEventKindV1::AnnRegionObservation { .. }
                                | DiagnosticEventKindV1::SqliteSyncObservation { .. }
                                | DiagnosticEventKindV1::SyncAggregate { .. }
                                | DiagnosticEventKindV1::ObjectKindAggregate { .. }
                                | DiagnosticEventKindV1::AdoptionGroupAggregate { .. }
                                | DiagnosticEventKindV1::Bm25CensusAggregate { .. }
                                | DiagnosticEventKindV1::Bm25BoundaryAggregate { .. }
                                | DiagnosticEventKindV1::Bm25LiveBuildAggregate { .. }
                                | DiagnosticEventKindV1::Link { .. }
                                | DiagnosticEventKindV1::Health { .. } => None,
                            })
                            .collect();
                        require(
                            observations.len() == 1,
                            "native sync observation missing or duplicated",
                        )?;
                        let observation =
                            observations.first().ok_or("missing native observation")?;
                        require(
                            observation.observed
                                && observation.same_native_fd
                                && !observation.ambiguous,
                            "real native sync observation lost its original FD",
                        )?;
                        require(
                            observation.full_result.is_some() || observation.posix_result.is_some(),
                            "real sync was mislabeled as an unattempted syscall",
                        )?;
                    }
                }
            }
            require_parent_sync_enclosed(&events)?;
            require(
                events.iter().any(|event| {
                    matches!(
                        event.kind,
                        DiagnosticEventKindV1::Counter {
                            key: DiagnosticCounterV1::SqliteSyncOverwrite,
                            value: 1,
                            ..
                        }
                    )
                }),
                "real checkpoint/reuse sequence did not observe a WAL overwrite",
            )?;
            require(
                !events
                    .iter()
                    .any(|event| matches!(event.kind, DiagnosticEventKindV1::Health { .. })),
                "normal native sync produced invalid diagnostics",
            )?;
        } else {
            require(
                events.is_empty() && clock.0.load(Ordering::Relaxed) == 0,
                "Off started a sync diagnostic clock",
            )?;
        }
        drop(events);
        drop(catalog);
        fixture.vfs.drain_retirements()?;
        require(
            fixture.common.is_quiescent()?,
            "sync diagnostic retained an IO handle",
        )?;
    }
    require(
        usages.first() == usages.get(1),
        "Record changed checkpoint work usage",
    )
}

fn require_parent_sync_enclosed(events: &[DiagnosticEmitV1]) -> TestResult {
    for parent in events.iter().filter(|event| {
        matches!(
            event.kind,
            DiagnosticEventKindV1::SpanBegin {
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(21),
                ..
            }
        )
    }) {
        let DiagnosticEventKindV1::SpanBegin { span_id, .. } = parent.kind else {
            return Err("parent sync begin missing".into());
        };
        let end = events.iter().find(|event| matches!(event.kind,
            DiagnosticEventKindV1::SpanEnd { span_id: id, outcome: DiagnosticTerminalV1::Ok } if id == span_id
        )).ok_or("parent sync did not finish successfully")?;
        let enclosing = events.iter().filter(|event| {
            let DiagnosticEventKindV1::SpanBegin {
                span_id: post_id,
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(15..=17),
                ..
            } = event.kind else {
                return false;
            };
            event.ts_ns <= parent.ts_ns && events.iter().any(|post| {
                matches!(post.kind, DiagnosticEventKindV1::SpanEnd { span_id, .. } if span_id == post_id)
                    && end.ts_ns <= post.ts_ns
            })
        }).count();
        require(
            enclosing == 1,
            "parent sync escaped its original AfterSync dispatch",
        )?;
    }
    Ok(())
}

#[test]
fn nested_off_and_foreign_scope_do_not_steal_sync_or_hide_failure() -> TestResult {
    use crate::publication_sqlite_vfs::diagnostic;
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([92; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let owner = NonZeroU64::new(1).ok_or("zero scope")?;
    let foreign = NonZeroU64::new(2).ok_or("zero foreign scope")?;
    let scope = diagnostic::Scope::enter(owner, Some(&child));
    diagnostic::before(owner, 8, rusqlite::ffi::SQLITE_OPEN_WAL, || {
        diagnostic::MutationClass::NoMutation
    });
    let before = clock.0.load(Ordering::Relaxed);
    {
        let _off = diagnostic::Scope::enter(owner, None);
        require(
            diagnostic::context(owner).is_none(),
            "Off leaked parent-sync context",
        )?;
        diagnostic::before(owner, 7, rusqlite::ffi::SQLITE_OPEN_WAL, || {
            diagnostic::MutationClass::NoMutation
        });
        diagnostic::after(
            owner,
            7,
            rusqlite::ffi::SQLITE_OK,
            NativeSyncObservation::default(),
            true,
        );
        let settlement = diagnostic::settlement(owner, rusqlite::ffi::SQLITE_OPEN_WAL);
        require(
            diagnostic::finish_settlement(settlement, rusqlite::ffi::SQLITE_OK, Ok(())) == Ok(()),
            "Off/foreign settlement changed success",
        )?;
    }
    diagnostic::before(foreign, 7, rusqlite::ffi::SQLITE_OPEN_WAL, || {
        diagnostic::MutationClass::NoMutation
    });
    require(
        diagnostic::context(foreign).is_none(),
        "foreign parent-sync context leaked",
    )?;
    require(
        diagnostic::context(owner).is_some(),
        "original parent-sync context was not restored",
    )?;
    diagnostic::after(
        foreign,
        7,
        rusqlite::ffi::SQLITE_OK,
        NativeSyncObservation::default(),
        true,
    );
    let settlement = diagnostic::settlement(foreign, rusqlite::ffi::SQLITE_OPEN_WAL);
    require(
        diagnostic::finish_settlement(settlement, rusqlite::ffi::SQLITE_OK, Ok(())) == Ok(()),
        "Off/foreign settlement changed success",
    )?;
    require(
        clock.0.load(Ordering::Relaxed) == before,
        "Off/foreign callback consumed original diagnostic clock",
    )?;
    diagnostic::after(
        owner,
        8,
        rusqlite::ffi::SQLITE_IOERR_FSYNC,
        NativeSyncObservation::default(),
        true,
    );
    drop(scope);
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let events = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    require(events.iter().filter(|event| matches!(event.kind, DiagnosticEventKindV1::SpanEnd { outcome: DiagnosticTerminalV1::Error { code }, .. } if code == rusqlite::ffi::SQLITE_IOERR_FSYNC.unsigned_abs())).count() == 1, "injected sync refusal was hidden")?;
    require(
        !events
            .iter()
            .any(|event| matches!(event.kind, DiagnosticEventKindV1::Health { .. })),
        "nested original scope was not restored",
    )
}

#[test]
fn post_sync_refusal_does_not_relabel_successful_native_sync() -> TestResult {
    use crate::publication_sqlite_vfs::diagnostic;
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([93; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let owner = NonZeroU64::new(1).ok_or("zero scope")?;
    let scope = diagnostic::Scope::enter(owner, Some(&child));
    diagnostic::before(owner, 8, rusqlite::ffi::SQLITE_OPEN_WAL, || {
        diagnostic::MutationClass::NoMutation
    });
    diagnostic::after(
        owner,
        8,
        rusqlite::ffi::SQLITE_OK,
        NativeSyncObservation::default(),
        true,
    );
    let settlement = diagnostic::settlement(owner, rusqlite::ffi::SQLITE_OPEN_WAL);
    require(
        diagnostic::finish_settlement(
            settlement,
            rusqlite::ffi::SQLITE_OK,
            Err::<(), _>(Refusal::Capacity),
        ) == Err(Refusal::Capacity),
        "post-sync diagnostics replaced the original capacity refusal",
    )?;
    drop(scope);
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let events = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    for (lane, expected) in [
        (11, DiagnosticTerminalV1::Ok),
        (15, DiagnosticTerminalV1::Error { code: 1 }),
    ] {
        let ids: Vec<_> = events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    stage: DiagnosticStageV1::SqliteWork,
                    lane: DiagnosticLaneV1(observed),
                    ..
                } = event.kind
                {
                    return (observed == lane).then_some(span_id);
                }
                None
            })
            .collect();
        require(
            ids.len() == 1,
            "native/settlement boundary missing or duplicated",
        )?;
        for id in ids {
            require(events.iter().filter(|event| matches!(event.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome } if span_id == id && outcome == expected)).count() == 1, "native and settlement outcomes were merged")?;
        }
    }
    require(
        !events
            .iter()
            .any(|event| matches!(event.kind, DiagnosticEventKindV1::Health { .. })),
        "post-sync observation invalidated capture",
    )
}

#[test]
fn legacy_catalog_context_records_native_sync_and_preserves_explicit_off() -> TestResult {
    use quanta_index_core::index::ports::{CatalogDiagnosticPort, CatalogReadPort};
    let mut bindings = Vec::new();
    for record in [false, true] {
        let fixture = fixture(64 * 1024 * 1024)?;
        let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
        let catalog = SqliteCatalog::open_with_io_admission(
            &fixture.root,
            Duration::from_secs(1),
            admission,
        )?;
        let clock = Arc::new(Clock::default());
        let events = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([94; 16]),
            clock.clone(),
            events.clone(),
        );
        let root = if record {
            Some(context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?)
        } else {
            None
        };
        let selected = root.as_ref().map(|root| context.child_context(root));
        {
            let _request = catalog.enter_diagnostic_scope(selected.as_ref());
            let session = catalog.open_read_session()?;
            bindings.push(session.binding());
            drop(session);
            let before_off = clock.0.load(Ordering::Relaxed);
            {
                let _off = catalog.enter_diagnostic_scope(None);
                let session = catalog.open_read_session()?;
                drop(session);
            }
            require(
                clock.0.load(Ordering::Relaxed) == before_off,
                "nested Off inherited legacy Catalog observation",
            )?;
            // A budget-aware call supplies its own explicit Off even inside a
            // legacy Record scope; diagnostics never replace its original budget.
            let mut budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 1_000_000,
                    writes: 1_000_000,
                    bytes: 536_870_912,
                    mutations: 1_000_000,
                },
                RequestBudgetV1::for_duration(Duration::from_secs(30)),
            );
            require(
                catalog.checkpoint_wal_for_reclaim(1, 64 * 1024 * 1024, &mut budget)?,
                "legacy fixture produced no real checkpoint",
            )?;
            require(
                clock.0.load(Ordering::Relaxed) == before_off,
                "budget-aware Off borrowed the ambient legacy context",
            )?;
            let session = catalog.open_read_session()?;
            drop(session);
        }
        if let Some(root) = root {
            let _finished = root.finish(DiagnosticTerminalV1::Ok);
        }
        let events = events
            .0
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        if record {
            for lane in [11, 15] {
                require(events.iter().any(|event| matches!(event.kind, DiagnosticEventKindV1::SpanBegin { stage: DiagnosticStageV1::SqliteWork, lane: DiagnosticLaneV1(observed), .. } if observed == lane)), "legacy Catalog call omitted native/post-sync observation")?;
            }
            require(
                !events
                    .iter()
                    .any(|event| matches!(event.kind, DiagnosticEventKindV1::Health { .. })),
                "legacy Catalog observation invalidated capture",
            )?;
        } else {
            require(
                events.is_empty() && clock.0.load(Ordering::Relaxed) == 0,
                "legacy Off consumed a diagnostic clock/event",
            )?;
        }
        drop(events);
        drop(catalog);
        fixture.vfs.drain_retirements()?;
        require(
            fixture.common.is_quiescent()?,
            "legacy observation retained native IO custody",
        )?;
    }
    require(
        bindings.first() == bindings.get(1),
        "Record changed the original Catalog session identity",
    )
}
