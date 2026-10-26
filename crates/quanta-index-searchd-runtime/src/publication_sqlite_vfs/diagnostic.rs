//! Request-local native sync observation. No admission, clocks or events in Off.
//!
//! `SqliteWork` lane11 is WAL sync,12 main-DB sync,13 other `SQLite` files. The
//! BeforeSync/AfterSync interval includes the native call and the VFS's post-call
//! size/inode witness, but excludes subsequent capacity settlement. It is not
//! device service time or a complete checkpoint measurement.
//! Lanes15/16/17 observe the subsequent WAL/main/other `AfterSync` adapter dispatch,
//! including its lock, inode/root checks, parent sync and capacity settlement.
//! These envelopes are not parent-fsync-only timers or CPU measurements.
//! Lane21 observes only the parent sync within a successful native-sync settlement.
//! Lane22 aggregates native `AfterWrite` callbacks across one Record scope.
//! Its bytes are requested, not successful or device bytes. A preflight
//! refusal has no `AfterWrite`; an adapter refusal after lower `xWrite` still does.

use std::{cell::RefCell, marker::PhantomData, num::NonZeroU64, rc::Rc};

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticLaneV1,
    DiagnosticSpanGuardV1, DiagnosticSqliteSyncObservationV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};
use rusqlite::ffi::quanta_capacity_vfs::NativeSyncObservation;

/// Mutation attempts since the last successful capacity settlement. This is
/// diagnostic DATA, not a durable-name witness or permission to omit a sync.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum MutationClass {
    #[default]
    NoMutation,
    Create,
    Overwrite,
    Growth,
    Truncate,
    Unclassified,
}

impl MutationClass {
    pub(super) fn write(&mut self, bytes: Option<u64>, target: Option<u64>) {
        let (Some(bytes), Some(target)) = (bytes, target) else {
            *self = Self::Unclassified;
            return;
        };
        match self {
            Self::Create | Self::Truncate | Self::Unclassified | Self::Growth => {}
            Self::NoMutation | Self::Overwrite => {
                *self = if target > bytes {
                    Self::Growth
                } else {
                    Self::Overwrite
                };
            }
        }
    }

    pub(super) fn truncate(&mut self) {
        if !matches!(self, Self::Create | Self::Unclassified) {
            *self = Self::Truncate;
        }
    }

    fn counter(self) -> DiagnosticCounterV1 {
        match self {
            Self::NoMutation => DiagnosticCounterV1::SqliteSyncNoMutation,
            Self::Create => DiagnosticCounterV1::SqliteSyncCreate,
            Self::Overwrite => DiagnosticCounterV1::SqliteSyncOverwrite,
            Self::Growth => DiagnosticCounterV1::SqliteSyncGrowth,
            Self::Truncate => DiagnosticCounterV1::SqliteSyncTruncate,
            Self::Unclassified => DiagnosticCounterV1::SqliteSyncUnclassified,
        }
    }
}

struct Active {
    owner: NonZeroU64,
    context: DiagnosticContextV1,
    sync: Option<(u64, DiagnosticSpanGuardV1, MutationClass)>,
    writes: Option<WriteAggregate>,
}

struct WriteAggregate {
    span: DiagnosticSpanGuardV1,
    // calls, requested bytes, failed calls, WAL, main DB, other.
    counts: [u64; 6],
    invalid: bool,
}

impl Active {
    fn finish(mut self) {
        // A missing AfterSync still drops as Aborted, before this scope's
        // aggregate ends. The TLS owner has already been restored.
        drop(self.sync.take());
        if let Some(writes) = self.writes.take() {
            if writes.invalid {
                self.context.health(DiagnosticHealthCodeV1::CounterOverflow);
            } else {
                for (key, count) in [
                    DiagnosticCounterV1::SqliteWriteCalls,
                    DiagnosticCounterV1::SqliteWriteRequestedBytes,
                    DiagnosticCounterV1::SqliteWriteFailures,
                    DiagnosticCounterV1::SqliteWalWriteCalls,
                    DiagnosticCounterV1::SqliteMainWriteCalls,
                    DiagnosticCounterV1::SqliteOtherWriteCalls,
                ]
                .into_iter()
                .zip(writes.counts)
                {
                    self.context.counter(&writes.span, key, count);
                }
            }
            let outcome = if std::thread::panicking() {
                DiagnosticTerminalV1::Aborted
            } else {
                DiagnosticTerminalV1::Ok
            };
            let _finished = writes.span.finish(outcome);
        }
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Active>> = const { RefCell::new(None) };
}

pub(super) struct Scope {
    previous: Option<Active>,
    _not_send: PhantomData<Rc<()>>,
}

impl Scope {
    pub(super) fn enter(owner: NonZeroU64, context: Option<&DiagnosticContextV1>) -> Self {
        let current = context.map(|context| {
            // Span entry may invoke the clock or sink. Do it before touching
            // CURRENT so observer reentry cannot borrow an active RefCell.
            let span = crate::runtime_diagnostic_span(
                context,
                context.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(22)),
            );
            Active {
                owner,
                context: context.clone(),
                sync: None,
                writes: span.map(|span| WriteAggregate {
                    span,
                    counts: [0; 6],
                    invalid: false,
                }),
            }
        });
        Self {
            previous: CURRENT.with(|slot| slot.replace(current)),
            _not_send: PhantomData,
        }
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        // Release the RefCell borrow and restore the original owner before
        // emitting or dropping an unfinished sync span. Nested Off masks it.
        let completed = CURRENT.with(|slot| slot.replace(self.previous.take()));
        if let Some(active) = completed {
            active.finish();
        }
    }
}

/// Count only a reached native `xWrite` result. Called from `AfterWrite`, never
/// from preflight or from a higher-level SQL statement.
pub(super) fn write(owner: NonZeroU64, open_flags: i32, amount: i64, sqlite_result: i32) {
    CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let Some(active) = current.as_mut().filter(|active| active.owner == owner) else {
            return;
        };
        let Some(writes) = active.writes.as_mut() else {
            return;
        };
        if writes.invalid {
            return;
        }
        let Ok(requested) = u64::try_from(amount) else {
            writes.invalid = true;
            return;
        };
        let class = if open_flags & rusqlite::ffi::SQLITE_OPEN_WAL != 0 {
            3
        } else if open_flags & rusqlite::ffi::SQLITE_OPEN_MAIN_DB != 0 {
            4
        } else {
            5
        };
        let mut next = writes.counts;
        let changes = [
            (0, 1),
            (1, requested),
            (2, u64::from(sqlite_result != rusqlite::ffi::SQLITE_OK)),
            (class, 1),
        ];
        for (index, value) in changes {
            let Some(slot) = next.get_mut(index) else {
                writes.invalid = true;
                return;
            };
            let Some(sum) = slot.checked_add(value) else {
                writes.invalid = true;
                return;
            };
            *slot = sum;
        }
        writes.counts = next;
    });
}

pub(super) fn before(
    owner: NonZeroU64,
    handle: u64,
    open_flags: i32,
    mutation: impl FnOnce() -> MutationClass,
) {
    let pending = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let active = current.as_mut().filter(|active| active.owner == owner)?;
        // The write span is an existing identity within this Record session.
        // A failed initial span has already invalidated the capture and cannot
        // produce valid sync spans.
        let marker = active.writes.as_ref()?.span.span_id();
        Some((active.context.clone(), marker, active.sync.take()))
    });
    let Some((context, marker, previous)) = pending else {
        return;
    };
    if previous.is_some() {
        context.health(DiagnosticHealthCodeV1::MissingParentSpan);
    }
    // Aborted SpanEnd and any observer reentry happen outside the TLS borrow.
    drop(previous);
    // Normal Off and foreign scopes never invoke this closure. Keep its state
    // lookup outside the timed native-call envelope and TLS borrow.
    let mutation = mutation();
    let lane = if open_flags & rusqlite::ffi::SQLITE_OPEN_WAL != 0 {
        11
    } else if open_flags & rusqlite::ffi::SQLITE_OPEN_MAIN_DB != 0 {
        12
    } else {
        13
    };
    let span = crate::runtime_diagnostic_span(
        &context,
        context.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(lane)),
    );
    let Some(span) = span else {
        return;
    };
    let uninstalled = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let Some(active) = current.as_mut().filter(|active| {
            active.owner == owner
                && active.context.shares_capture_session(&context)
                && active
                    .writes
                    .as_ref()
                    .is_some_and(|writes| writes.span.span_id() == marker)
        }) else {
            return Some(span);
        };
        if active.sync.is_some() {
            return Some(span);
        }
        active.sync = Some((handle, span, mutation));
        None
    });
    if let Some(span) = uninstalled {
        context.health(DiagnosticHealthCodeV1::MissingParentSpan);
        drop(span);
    }
}

pub(super) fn after(
    owner: NonZeroU64,
    handle: u64,
    sqlite_result: i32,
    native: NativeSyncObservation,
    linked_witness: bool,
) {
    let pending = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let active = current.as_mut().filter(|active| active.owner == owner)?;
        Some((active.context.clone(), active.sync.take()))
    });
    let Some((context, pending)) = pending else {
        return;
    };
    let Some((started_handle, span, mutation)) = pending else {
        context.health(DiagnosticHealthCodeV1::MissingParentSpan);
        return;
    };
    if started_handle != handle {
        context.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
        return;
    }
    let mutation = if linked_witness {
        mutation
    } else {
        MutationClass::Unclassified
    };
    context.counter(&span, mutation.counter(), 1);
    context.sqlite_sync_observation(
        &span,
        DiagnosticSqliteSyncObservationV1 {
            observed: native.observed,
            same_native_fd: native.same_native_fd,
            ambiguous: native.ambiguous,
            full_requested: native.full_requested,
            full_default_syscall: native.full_default_syscall,
            full_result: native.full_attempted.then_some(native.full_result),
            full_errno: native.full_attempted.then_some(native.full_errno),
            posix_result: native.posix_attempted.then_some(native.posix_result),
            posix_errno: native.posix_attempted.then_some(native.posix_errno),
        },
    );
    let outcome = if sqlite_result == rusqlite::ffi::SQLITE_OK {
        DiagnosticTerminalV1::Ok
    } else {
        DiagnosticTerminalV1::Error {
            code: sqlite_result.unsigned_abs(),
        }
    };
    let _finished = span.finish(outcome);
}

/// Observe the existing post-native-sync dispatch without owning its outcome.
pub(super) fn settlement(owner: NonZeroU64, open_flags: i32) -> Option<DiagnosticSpanGuardV1> {
    let context = CURRENT.with(|slot| {
        let current = slot.borrow();
        let active = current.as_ref().filter(|active| active.owner == owner)?;
        Some(active.context.clone())
    })?;
    let lane = if open_flags & rusqlite::ffi::SQLITE_OPEN_WAL != 0 {
        15
    } else if open_flags & rusqlite::ffi::SQLITE_OPEN_MAIN_DB != 0 {
        16
    } else {
        17
    };
    crate::runtime_diagnostic_span(
        &context,
        context.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(lane)),
    )
}

/// Original-owner metadata only; does not confer file or capacity authority.
pub(super) fn context(owner: NonZeroU64) -> Option<DiagnosticContextV1> {
    CURRENT.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|active| active.owner == owner)
            .map(|active| active.context.clone())
    })
}

pub(super) fn finish_settlement<T>(
    span: Option<DiagnosticSpanGuardV1>,
    native_result: i32,
    result: Result<T, super::Refusal>,
) -> Result<T, super::Refusal> {
    if let Some(span) = span {
        let outcome = if native_result == rusqlite::ffi::SQLITE_OK {
            DiagnosticTerminalV1::from_result(&result)
        } else {
            DiagnosticTerminalV1::Error {
                code: native_result.unsigned_abs(),
            }
        };
        let _finished = span.finish(outcome);
    }
    result
}
