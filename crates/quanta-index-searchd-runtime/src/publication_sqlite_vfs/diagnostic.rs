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
//! Scratch starts that envelope at its first attempted write or sync, so a
//! missing scratch lane22 only means no observed write/sync in that scope;
//! read-only `SQLite` work can still have occurred. Catalog remains eager.
//! Its bytes are requested, not successful or device bytes. A preflight
//! refusal has no `AfterWrite`; an adapter refusal after lower `xWrite` still does.
//! Lane25 observes only the `CKPT_START` to `CKPT_DONE` backfill window. `SQLite`
//! emits START after WAL sync and DONE before main-DB sync; its Ok terminal
//! confirms a marker pair, not checkpoint success. Lane22 is segmented at
//! both markers so writes within lane25 have that span as their parent.

use std::{
    cell::RefCell,
    marker::PhantomData,
    num::NonZeroU64,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticLaneV1,
    DiagnosticSpanGuardV1, DiagnosticSpanIdV1, DiagnosticSqliteSyncObservationV1,
    DiagnosticStageV1, DiagnosticTerminalV1,
};
use rusqlite::ffi::quanta_capacity_vfs::NativeSyncObservation;

pub(super) mod write_timing;

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
    write_start: WriteStart,
    backfill: Option<Backfill>,
    transition: Option<DiagnosticSpanIdV1>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum WriteStart {
    // Scratch read-only scopes have no lane22 span or diagnostic clock sample.
    Pending,
    // Prevent observer reentry from opening a duplicate segment.
    Starting,
    Ready,
    // Reentrant observer I/O invalidates only this diagnostic aggregate.
    Faulted,
}

struct Backfill {
    handle: u64,
    original_segment: DiagnosticSpanIdV1,
    child_segment: DiagnosticSpanIdV1,
    context: DiagnosticContextV1,
    span: DiagnosticSpanGuardV1,
}

struct WriteAggregate {
    span: DiagnosticSpanGuardV1,
    // calls, requested bytes, failed calls, WAL, main DB, other.
    counts: [u64; 6],
    invalid: bool,
    timing: write_timing::Totals,
}

fn new_writes(context: &DiagnosticContextV1) -> Option<WriteAggregate> {
    crate::runtime_diagnostic_span(
        context,
        context.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(22)),
    )
    .map(|span| WriteAggregate {
        span,
        counts: [0; 6],
        invalid: false,
        timing: write_timing::Totals::default(),
    })
}

/// Start a scratch aggregate at its first attempted write or sync.
///
/// The state claim and installation touch TLS separately; clock and sink
/// callbacks run without a `RefCell` borrow and cannot reject physical `SQLite` I/O.
pub(super) fn ensure_writes(owner: NonZeroU64) {
    let pending = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let active = current.as_mut().filter(|active| active.owner == owner)?;
        match active.write_start {
            WriteStart::Pending => {
                active.write_start = WriteStart::Starting;
                Some(Ok(active.current_context().clone()))
            }
            WriteStart::Starting => {
                active.write_start = WriteStart::Faulted;
                Some(Err(active.current_context().clone()))
            }
            WriteStart::Ready | WriteStart::Faulted => None,
        }
    });
    let context = match pending {
        Some(Ok(context)) => context,
        Some(Err(context)) => {
            let _observer = catch_unwind(AssertUnwindSafe(|| {
                context.health(DiagnosticHealthCodeV1::MissingParentSpan);
            }));
            return;
        }
        None => return,
    };
    let writes = match catch_unwind(AssertUnwindSafe(|| new_writes(&context))) {
        Ok(writes) => writes,
        Err(_observer_panic) => {
            let _observer = catch_unwind(AssertUnwindSafe(|| {
                context.health(DiagnosticHealthCodeV1::MissingParentSpan);
            }));
            None
        }
    };
    let uninstalled = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let Some(active) = current.as_mut().filter(|active| {
            active.owner == owner
                && active.context.shares_capture_session(&context)
                && active.write_start == WriteStart::Starting
                && active.writes.is_none()
                && active.transition.is_none()
        }) else {
            return writes;
        };
        active.write_start = WriteStart::Ready;
        active.writes = writes;
        None
    });
    if let Some(writes) = uninstalled {
        let _observer = catch_unwind(AssertUnwindSafe(|| {
            context.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
        }));
        let _observer = catch_unwind(AssertUnwindSafe(|| drop(writes)));
    }
}

fn finish_writes(
    context: &DiagnosticContextV1,
    mut writes: WriteAggregate,
    outcome: DiagnosticTerminalV1,
) {
    writes.timing.finish_segment();
    if let Some(code) = writes.timing.invalid_code() {
        context.health(code);
    } else {
        for (key, count) in [
            DiagnosticCounterV1::SqliteWriteAdmissionCalls,
            DiagnosticCounterV1::SqliteWriteAdmissionRefusals,
            DiagnosticCounterV1::SqliteWriteAdmissionElapsedNs,
            DiagnosticCounterV1::SqliteWriteLowerEnvelopeElapsedNs,
            DiagnosticCounterV1::SqliteWritePostCallbackElapsedNs,
            DiagnosticCounterV1::SqliteWritePostCallbackRefusals,
        ]
        .into_iter()
        .zip(writes.timing.counts())
        {
            context.counter(&writes.span, key, count);
        }
    }
    if writes.invalid {
        context.health(DiagnosticHealthCodeV1::CounterOverflow);
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
            context.counter(&writes.span, key, count);
        }
    }
    let _finished = writes.span.finish(outcome);
}

impl Active {
    fn current_context(&self) -> &DiagnosticContextV1 {
        self.backfill
            .as_ref()
            .map_or(&self.context, |backfill| &backfill.context)
    }

    fn finish(mut self) {
        // A missing AfterSync still drops as Aborted, before this scope's
        // aggregate ends. The TLS owner has already been restored.
        drop(self.sync.take());
        if let Some(writes) = self.writes.take() {
            let outcome = if std::thread::panicking() {
                DiagnosticTerminalV1::Aborted
            } else {
                DiagnosticTerminalV1::Ok
            };
            let context = self
                .backfill
                .as_ref()
                .map_or(&self.context, |backfill| &backfill.context);
            finish_writes(context, writes, outcome);
        }
        // An unpaired START must never look like a completed checkpoint.
        drop(self.backfill.take());
    }
}

fn resume_original_writes(
    owner: NonZeroU64,
    context: &DiagnosticContextV1,
    marker: DiagnosticSpanIdV1,
) {
    // Clock and sink callbacks may enter nested scopes. Never hold a TLS
    // RefCell borrow while entering the replacement segment.
    let writes = new_writes(context);
    let uninstalled = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let Some(active) = current.as_mut().filter(|active| {
            active.owner == owner
                && active.context.shares_capture_session(context)
                && active.transition == Some(marker)
                && active.writes.is_none()
                && active.backfill.is_none()
        }) else {
            return writes;
        };
        active.transition = None;
        active.writes = writes;
        None
    });
    if let Some(writes) = uninstalled {
        context.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
        drop(writes);
    }
}

/// Observe the bundled `SQLite` `CKPT_START` marker after WAL sync.
///
/// This marker precedes main-DB backfill. Its advisory return grants no authority.
pub(super) fn checkpoint_start(owner: NonZeroU64, handle: u64) {
    // A sync-only scratch scope can reach a checkpoint without an xWrite.
    ensure_writes(owner);
    let pending = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let active = current.as_mut().filter(|active| active.owner == owner)?;
        if active.backfill.is_some() || active.transition.is_some() {
            return Some(Err(active.context.clone()));
        }
        let writes = active.writes.take()?;
        let marker = writes.span.span_id();
        active.transition = Some(marker);
        Some(Ok((active.context.clone(), marker, writes)))
    });
    let Some(pending) = pending else {
        return;
    };
    let (context, marker, writes) = match pending {
        Ok(pending) => pending,
        Err(context) => {
            context.health(DiagnosticHealthCodeV1::MissingParentSpan);
            return;
        }
    };
    finish_writes(&context, writes, DiagnosticTerminalV1::Ok);
    let span = crate::runtime_diagnostic_span(
        &context,
        context.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(25)),
    );
    let Some(span) = span else {
        resume_original_writes(owner, &context, marker);
        return;
    };
    let child_context = context.child_context(&span);
    let child_writes = new_writes(&child_context);
    let Some(child_writes) = child_writes else {
        drop(span);
        resume_original_writes(owner, &context, marker);
        return;
    };
    let child_segment = child_writes.span.span_id();
    let uninstalled = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let Some(active) = current.as_mut().filter(|active| {
            active.owner == owner
                && active.context.shares_capture_session(&context)
                && active.transition == Some(marker)
                && active.writes.is_none()
                && active.backfill.is_none()
        }) else {
            return Some((child_writes, span));
        };
        active.transition = None;
        active.writes = Some(child_writes);
        active.backfill = Some(Backfill {
            handle,
            original_segment: marker,
            child_segment,
            context: child_context.clone(),
            span,
        });
        None
    });
    if let Some((child_writes, span)) = uninstalled {
        context.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
        finish_writes(&child_context, child_writes, DiagnosticTerminalV1::Aborted);
        drop(span);
        resume_original_writes(owner, &context, marker);
    }
}

/// Observe `CKPT_DONE` after page copy, before main-DB sync and backfill settlement.
///
/// A paired Ok confirms marker observation; `SQLite` may still fail the checkpoint.
pub(super) fn checkpoint_done(owner: NonZeroU64, handle: u64) {
    let pending = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let active = current.as_mut().filter(|active| active.owner == owner)?;
        // SQLite emits CKPT_DONE even if the preceding WAL xSync failed,
        // while CKPT_START is skipped on that supported error path. There is
        // no backfill window to close and the WAL sync span already owns the
        // failure. Do not invalidate this capture for a missing START.
        let backfill = active.backfill.as_ref()?;
        if backfill.handle != handle
            || !active.context.shares_capture_session(&backfill.context)
            || active
                .writes
                .as_ref()
                .is_none_or(|writes| writes.span.span_id() != backfill.child_segment)
        {
            return Some(Err((
                active.context.clone(),
                DiagnosticHealthCodeV1::ForeignCaptureSpan,
            )));
        }
        let backfill = active.backfill.take()?;
        let writes = active.writes.take()?;
        active.transition = Some(backfill.original_segment);
        Some(Ok((active.context.clone(), backfill, writes)))
    });
    let Some(pending) = pending else {
        return;
    };
    let (context, backfill, writes) = match pending {
        Ok(pending) => pending,
        Err((context, code)) => {
            context.health(code);
            return;
        }
    };
    let marker = backfill.original_segment;
    finish_writes(&backfill.context, writes, DiagnosticTerminalV1::Ok);
    let _finished = backfill.span.finish(DiagnosticTerminalV1::Ok);
    resume_original_writes(owner, &context, marker);
}

// Keep diagnostic aggregates out of the per-operation guard. An unobserved
// scope still masks/restores nested contexts without allocating Active state.
thread_local! {
    static CURRENT: RefCell<Option<Box<Active>>> = const { RefCell::new(None) };
}

pub(super) struct Scope {
    previous: Option<Box<Active>>,
    _not_send: PhantomData<Rc<()>>,
}

impl Scope {
    pub(super) fn enter(owner: NonZeroU64, context: Option<&DiagnosticContextV1>) -> Self {
        Self::enter_with_writes(owner, context, true)
    }

    /// Defer scratch lane22 until an attempted write or sync; keep the same
    /// nested Off/foreign masking and restoration as eager Catalog scopes.
    pub(super) fn enter_scratch(owner: NonZeroU64, context: Option<&DiagnosticContextV1>) -> Self {
        Self::enter_with_writes(owner, context, false)
    }

    fn enter_with_writes(
        owner: NonZeroU64,
        context: Option<&DiagnosticContextV1>,
        eager: bool,
    ) -> Self {
        // A nested diagnostic scope masks the in-flight callback. Neither an
        // Off nor a foreign capture may complete the outer write envelope.
        CURRENT.with(|slot| {
            if let Some(active) = slot.borrow_mut().as_mut()
                && let Some(writes) = active.writes.as_mut()
            {
                writes.timing.mask_for_nested_scope();
            }
        });
        let current = context.map(|context| {
            // Span entry may invoke the clock or sink. Do it before touching
            // CURRENT so observer reentry cannot borrow an active RefCell.
            Box::new(Active {
                owner,
                context: context.clone(),
                sync: None,
                writes: eager.then(|| new_writes(context)).flatten(),
                write_start: if eager {
                    WriteStart::Ready
                } else {
                    WriteStart::Pending
                },
                backfill: None,
                transition: None,
            })
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
            (*active).finish();
        }
    }
}

/// Count only a reached native `xWrite` result. Called from `AfterWrite`, never
/// from preflight or from a higher-level SQL statement.
pub(super) fn write(owner: NonZeroU64, open_flags: i32, amount: i64, sqlite_result: i32) {
    let missing = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let active = current.as_mut().filter(|active| active.owner == owner)?;
        if active.transition.is_some() {
            // Recursive SQLite I/O from an Admission/diagnostic callback is
            // unsupported. If it occurs anyway, invalidate the capture
            // instead of publishing a silently incomplete write total.
            return Some(active.context.clone());
        }
        let writes = active.writes.as_mut()?;
        if writes.invalid {
            return None;
        }
        let Ok(requested) = u64::try_from(amount) else {
            writes.invalid = true;
            return None;
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
                return None;
            };
            let Some(sum) = slot.checked_add(value) else {
                writes.invalid = true;
                return None;
            };
            *slot = sum;
        }
        writes.counts = next;
        None
    });
    if let Some(context) = missing {
        context.health(DiagnosticHealthCodeV1::MissingParentSpan);
    }
}

pub(super) fn before(
    owner: NonZeroU64,
    handle: u64,
    open_flags: i32,
    mutation: impl FnOnce() -> MutationClass,
) {
    // BeforeSync may be the first physical I/O in a scratch scope. Preserve
    // its original lane11/12/13 parent and AfterSync pairing.
    ensure_writes(owner);
    let pending = CURRENT.with(|slot| {
        let mut current = slot.borrow_mut();
        let active = current.as_mut().filter(|active| active.owner == owner)?;
        // The write span is an existing identity within this Record session.
        // A failed initial span has already invalidated the capture and cannot
        // produce valid sync spans.
        let marker = active.writes.as_ref()?.span.span_id();
        Some((active.current_context().clone(), marker, active.sync.take()))
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
        Some((active.current_context().clone(), active.sync.take()))
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
        Some(active.current_context().clone())
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
            .map(|active| active.current_context().clone())
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
