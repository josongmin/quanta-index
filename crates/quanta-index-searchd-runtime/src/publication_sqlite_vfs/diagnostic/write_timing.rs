//! Record-only, scope-local envelopes around the existing native write callbacks.
//!
//! The interval between callbacks includes C glue, lower xWrite and native
//! witnesses; it is neither lower-VFS service time nor process CPU time.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::{cell::Cell, marker::PhantomData, num::NonZeroU64, rc::Rc};

use quanta_index_core::{DiagnosticContextV1, DiagnosticHealthCodeV1, DiagnosticSpanIdV1};
use rusqlite::ffi::quanta_capacity_vfs::{EventKind, Refusal};

use super::{CURRENT, WriteStart, ensure_writes};

#[derive(Clone, Copy, Eq, PartialEq)]
enum WriteKind {
    Before,
    After,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Callback {
    sequence: u64,
    handle: u64,
    kind: WriteKind,
}

#[derive(Clone, Copy)]
struct Pending {
    handle: u64,
    accepted_at_ns: u64,
}

#[derive(Default)]
pub(super) struct Totals {
    // admission calls/refusals/ns, lower envelope ns, post ns/refusals.
    counts: [u64; 6],
    sequence: u64,
    active: Option<Callback>,
    pending: Option<Pending>,
    invalid: Option<DiagnosticHealthCodeV1>,
}

impl Totals {
    pub(super) fn counts(&self) -> [u64; 6] {
        self.counts
    }

    pub(super) fn invalid_code(&self) -> Option<DiagnosticHealthCodeV1> {
        self.invalid
    }

    fn invalidate(&mut self, code: DiagnosticHealthCodeV1) {
        let _original = self.invalid.get_or_insert(code);
        self.pending = None;
    }

    pub(super) fn mask_for_nested_scope(&mut self) {
        if self.active.is_some() || self.pending.is_some() {
            self.invalidate(DiagnosticHealthCodeV1::ForeignCaptureSpan);
        }
    }

    pub(super) fn finish_segment(&mut self) {
        if self.active.is_some() || self.pending.is_some() {
            self.invalidate(DiagnosticHealthCodeV1::MissingParentSpan);
        }
    }

    fn charge(&mut self, index: usize, value: u64) {
        let Some(next) = self
            .counts
            .get(index)
            .and_then(|current| current.checked_add(value))
        else {
            self.invalidate(DiagnosticHealthCodeV1::CounterOverflow);
            return;
        };
        if let Some(slot) = self.counts.get_mut(index) {
            *slot = next;
        } else {
            self.invalidate(DiagnosticHealthCodeV1::CounterOverflow);
        }
    }

    fn duration(&mut self, index: usize, start: u64, end: u64) {
        let Some(elapsed) = end.checked_sub(start) else {
            self.invalidate(DiagnosticHealthCodeV1::ClockRegressed);
            return;
        };
        self.charge(index, elapsed);
    }
}

/// A token exists only for an eligible Record owner and live lane22 segment.
/// It never grants VFS admission or caches path/capacity authority.
pub(in crate::publication_sqlite_vfs) struct CallbackTiming {
    owner: NonZeroU64,
    context: DiagnosticContextV1,
    segment: DiagnosticSpanIdV1,
    callback: Callback,
    started_ns: u64,
    finished: bool,
    // Completion updates the originating thread's diagnostic TLS only.
    _not_send: PhantomData<Rc<()>>,
}

impl CallbackTiming {
    pub(in crate::publication_sqlite_vfs) fn begin(
        owner: NonZeroU64,
        handle: u64,
        kind: EventKind,
    ) -> Option<Self> {
        let kind = if kind == EventKind::BeforeWrite {
            WriteKind::Before
        } else if kind == EventKind::AfterWrite {
            WriteKind::After
        } else {
            return None;
        };
        let mut marker_fault = None;
        let needs_writes = Cell::new(false);
        let mut claim = || {
            CURRENT.with(|slot| {
                let mut current = slot.borrow_mut();
                let active = current.as_mut().filter(|active| active.owner == owner)?;
                let context = active.current_context().clone();
                let Some(writes) = active.writes.as_mut() else {
                    if active.transition.is_some() {
                        marker_fault = Some(context);
                    } else if kind == WriteKind::Before
                        && matches!(
                            active.write_start,
                            WriteStart::Pending | WriteStart::Starting
                        )
                    {
                        needs_writes.set(true);
                    }
                    return None;
                };
                let timing = &mut writes.timing;
                if active.transition.is_some() {
                    timing.invalidate(DiagnosticHealthCodeV1::MissingParentSpan);
                    return None;
                }
                if timing.active.is_some() {
                    timing.invalidate(DiagnosticHealthCodeV1::ForeignCaptureSpan);
                    return None;
                }
                match kind {
                    WriteKind::Before if timing.pending.is_some() => {
                        timing.invalidate(DiagnosticHealthCodeV1::MissingParentSpan);
                        return None;
                    }
                    WriteKind::After
                        if timing
                            .pending
                            .is_none_or(|pending| pending.handle != handle) =>
                    {
                        timing.invalidate(DiagnosticHealthCodeV1::ForeignCaptureSpan);
                        return None;
                    }
                    WriteKind::Before | WriteKind::After => {}
                }
                if timing.invalid.is_some() {
                    return None;
                }
                let Some(sequence) = timing.sequence.checked_add(1) else {
                    timing.invalidate(DiagnosticHealthCodeV1::CounterOverflow);
                    return None;
                };
                timing.sequence = sequence;
                let callback = Callback {
                    sequence,
                    handle,
                    kind,
                };
                timing.active = Some(callback);
                Some((context, writes.span.span_id(), callback))
            })
        };
        let initial = claim();
        let claimed = if needs_writes.get() {
            // Only lazy scratch pays a second TLS claim. Eager Catalog and
            // Off keep the original single-claim callback path.
            needs_writes.set(false);
            ensure_writes(owner);
            claim()
        } else {
            initial
        };
        if let Some(context) = marker_fault {
            context.health(DiagnosticHealthCodeV1::MissingParentSpan);
        }
        let claimed = claimed?;
        // The configured diagnostic clock may reenter the observer. Never
        // call it while holding CURRENT's RefCell borrow.
        let sample = catch_unwind(AssertUnwindSafe(|| claimed.0.clock_now_ns()));
        let mut token = Self {
            owner,
            context: claimed.0,
            segment: claimed.1,
            callback: claimed.2,
            started_ns: 0,
            finished: false,
            _not_send: PhantomData,
        };
        let Ok(started_ns) = sample else {
            // A diagnostic clock is an observer. Its panic cannot turn an
            // otherwise admitted native write into an I/O refusal.
            drop(token);
            return None;
        };
        token.started_ns = started_ns;
        if kind == WriteKind::After {
            token.with_totals(|totals| {
                if let Some(pending) = totals.pending.take() {
                    totals.duration(3, pending.accepted_at_ns, started_ns);
                }
            });
        }
        Some(token)
    }

    fn with_totals(&self, apply: impl FnOnce(&mut Totals)) {
        CURRENT.with(|slot| {
            let mut current = slot.borrow_mut();
            let Some(active) = current.as_mut().filter(|active| {
                active.owner == self.owner
                    && active
                        .current_context()
                        .shares_capture_session(&self.context)
            }) else {
                return;
            };
            let Some(writes) = active.writes.as_mut() else {
                return;
            };
            if active.transition.is_some() || writes.span.span_id() != self.segment {
                writes
                    .timing
                    .invalidate(DiagnosticHealthCodeV1::ForeignCaptureSpan);
                return;
            }
            if writes.timing.active != Some(self.callback) {
                writes
                    .timing
                    .invalidate(DiagnosticHealthCodeV1::ForeignCaptureSpan);
                return;
            }
            apply(&mut writes.timing);
        });
    }

    pub(in crate::publication_sqlite_vfs) fn finish(
        mut self,
        result: &Result<Option<std::ffi::CString>, Refusal>,
    ) {
        let Ok(ended_ns) = catch_unwind(AssertUnwindSafe(|| self.context.clock_now_ns())) else {
            self.with_totals(|totals| {
                totals.invalidate(DiagnosticHealthCodeV1::MissingParentSpan);
                totals.active = None;
            });
            self.finished = true;
            return;
        };
        self.with_totals(|totals| {
            if totals.invalid.is_none() {
                match self.callback.kind {
                    WriteKind::Before => {
                        totals.charge(0, 1);
                        totals.charge(1, u64::from(result.is_err()));
                        totals.duration(2, self.started_ns, ended_ns);
                        if result.is_ok() && totals.invalid.is_none() {
                            totals.pending = Some(Pending {
                                handle: self.callback.handle,
                                accepted_at_ns: ended_ns,
                            });
                        }
                    }
                    WriteKind::After => {
                        totals.duration(4, self.started_ns, ended_ns);
                        totals.charge(5, u64::from(result.is_err()));
                    }
                }
            }
            totals.active = None;
        });
        self.finished = true;
    }
}

impl Drop for CallbackTiming {
    fn drop(&mut self) {
        if !self.finished {
            // Panics are caught at the native facade. Keep product unwind and
            // original refusal behavior; invalidate only the timing series.
            self.with_totals(|totals| {
                totals.invalidate(DiagnosticHealthCodeV1::MissingParentSpan);
                totals.active = None;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_duration_and_counter_overflow_invalidate_timing() {
        let mut totals = Totals {
            counts: [0, 0, u64::MAX, 0, 0, 0],
            ..Totals::default()
        };
        totals.duration(2, 1, 2);
        assert_eq!(
            totals.invalid_code(),
            Some(DiagnosticHealthCodeV1::CounterOverflow)
        );

        let mut totals = Totals::default();
        totals.duration(3, 3, 2);
        assert_eq!(
            totals.invalid_code(),
            Some(DiagnosticHealthCodeV1::ClockRegressed)
        );
    }
}
