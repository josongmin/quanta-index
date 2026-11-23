//! Lexical request observation for the original synchronous Catalog APIs.
//! This state carries no lock, funding, lease or mutation permission.

use std::{cell::RefCell, marker::PhantomData, rc::Rc};

use quanta_index_core::{
    DiagnosticContextV1,
    index::ports::{CatalogDiagnosticPort, CatalogDiagnosticScope},
};

use super::SqliteCatalog;

struct Binding {
    owner: usize,
    context: Option<DiagnosticContextV1>,
}

thread_local! {
    static CURRENT: RefCell<Option<Binding>> = const { RefCell::new(None) };
}

struct Scope<'a> {
    previous: Option<Binding>,
    _catalog: &'a SqliteCatalog,
    _not_send: PhantomData<Rc<()>>,
}

impl CatalogDiagnosticScope for Scope<'_> {}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        CURRENT.with(|slot| {
            let _finished = slot.replace(self.previous.take());
        });
    }
}

impl CatalogDiagnosticPort for SqliteCatalog {
    fn enter_diagnostic_scope(
        &self,
        context: Option<&DiagnosticContextV1>,
    ) -> Option<Box<dyn CatalogDiagnosticScope + '_>> {
        CURRENT.with(|slot| {
            // Normal Off has no allocation, context clone, clock or event.
            // Nested Off must still mask and later restore an active binding.
            if context.is_none() && slot.borrow().is_none() {
                return None;
            }
            let previous = slot.replace(Some(Binding {
                owner: std::ptr::from_ref(self).addr(),
                context: context.cloned(),
            }));
            let scope: Box<dyn CatalogDiagnosticScope + '_> = Box::new(Scope {
                previous,
                _catalog: self,
                _not_send: PhantomData,
            });
            Some(scope)
        })
    }
}

pub(super) fn current(catalog: &SqliteCatalog) -> Option<DiagnosticContextV1> {
    let owner = std::ptr::from_ref(catalog).addr();
    CURRENT.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|binding| binding.owner == owner)
            .and_then(|binding| binding.context.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticEmitV1, DiagnosticEventKindV1,
        DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticSpanIdV1,
        DiagnosticStageV1, DiagnosticTerminalV1, RequestBudgetV1,
        index::{IndexError, WorkBudget, WorkLimits},
    };
    use std::sync::{
        Arc, Mutex, OnceLock, TryLockError, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};

    #[derive(Debug, Default)]
    struct Clock(AtomicU64);
    impl DiagnosticClockV1 for Clock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }
    #[derive(Debug)]
    struct Sink;
    impl DiagnosticEventSinkV1 for Sink {
        fn try_emit(&self, _event: DiagnosticEmitV1) -> bool {
            true
        }
    }
    #[derive(Debug, Default)]
    struct LockEvents {
        rows: Mutex<Vec<DiagnosticEmitV1>>,
        catalog: OnceLock<Weak<SqliteCatalog>>,
        hold_ends: AtomicUsize,
        unlocked_hold_ends: AtomicUsize,
    }

    impl LockEvents {
        fn rows(&self) -> Vec<DiagnosticEmitV1> {
            let rows = match self.rows.lock() {
                Ok(rows) => rows,
                Err(poison) => poison.into_inner(),
            };
            rows.clone()
        }
    }

    impl DiagnosticEventSinkV1 for LockEvents {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            let mut rows = match self.rows.lock() {
                Ok(rows) => rows,
                Err(poison) => poison.into_inner(),
            };
            if let DiagnosticEventKindV1::SpanEnd { span_id, .. } = event.kind
                && rows.iter().any(|row| {
                    matches!(
                        row.kind,
                        DiagnosticEventKindV1::SpanBegin {
                            span_id: begin,
                            stage: DiagnosticStageV1::CatalogLockHold,
                            ..
                        } if begin == span_id
                    )
                })
            {
                let _prior = self.hold_ends.fetch_add(1, Ordering::Relaxed);
                let unlocked = self
                    .catalog
                    .get()
                    .and_then(Weak::upgrade)
                    .is_some_and(|catalog| match catalog.connection.mutex() {
                        Ok(mutex) => !matches!(mutex.try_lock(), Err(TryLockError::WouldBlock)),
                        Err(_closed) => false,
                    });
                if unlocked {
                    let _prior = self.unlocked_hold_ends.fetch_add(1, Ordering::Relaxed);
                }
            }
            rows.push(event);
            true
        }
    }

    fn terminal(
        rows: &[DiagnosticEmitV1],
        stage: DiagnosticStageV1,
    ) -> Vec<(DiagnosticSpanIdV1, u64, u64, DiagnosticTerminalV1)> {
        rows.iter()
            .filter_map(|begin| {
                let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    stage: observed,
                    ..
                } = begin.kind
                else {
                    return None;
                };
                if observed != stage {
                    return None;
                }
                rows.iter().find_map(|end| {
                    if let DiagnosticEventKindV1::SpanEnd {
                        span_id: ended,
                        outcome,
                    } = end.kind
                        && ended == span_id
                    {
                        Some((span_id, begin.ts_ns, end.ts_ns, outcome))
                    } else {
                        None
                    }
                })
            })
            .collect()
    }

    fn lock_trace(
        catalog: &Arc<SqliteCatalog>,
        capture: u8,
    ) -> Result<
        (
            Arc<LockEvents>,
            DiagnosticContextV1,
            quanta_index_core::DiagnosticSpanGuardV1,
        ),
        Box<dyn std::error::Error>,
    > {
        let events = Arc::new(LockEvents::default());
        let _bound = events.catalog.set(Arc::downgrade(catalog));
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([capture; 16]),
            Arc::new(Clock::default()),
            events.clone(),
        );
        let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
        let child = context.child_context(&root);
        Ok((events, child, root))
    }
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    fn require(value: bool) -> TestResult {
        if value {
            Ok(())
        } else {
            Err("diagnostic scope boundary differs".into())
        }
    }

    fn require_lock(value: bool, message: &'static str) -> TestResult {
        if value { Ok(()) } else { Err(message.into()) }
    }

    #[test]
    fn exact_catalog_nested_off_foreign_and_thread_restore_without_clocks() -> TestResult {
        let a_root = tempfile::tempdir()?;
        let b_root = tempfile::tempdir()?;
        let a = SqliteCatalog::open(a_root.path(), std::time::Duration::from_secs(1))?;
        let b = SqliteCatalog::open(b_root.path(), std::time::Duration::from_secs(1))?;
        let clock = Arc::new(Clock::default());
        let first = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([1; 16]),
            clock.clone(),
            Arc::new(Sink),
        );
        let second = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([2; 16]),
            clock.clone(),
            Arc::new(Sink),
        );
        require(a.enter_diagnostic_scope(None).is_none())?;
        drop(a.lock()?);
        let outer = a.enter_diagnostic_scope(Some(&first));
        require(current(&a).map(|c| c.capture_id()) == Some(first.capture_id()))?;
        require(current(&b).is_none())?;
        let foreign_thread = std::thread::scope(|threads| {
            threads.spawn(|| current(&a).map(|c| c.capture_id())).join()
        })
        .map_err(|_panic| "diagnostic scope thread panicked")?;
        require(foreign_thread.is_none())?;
        {
            let _off = a.enter_diagnostic_scope(None);
            require(current(&a).is_none())?;
            drop(a.lock()?);
        }
        require(current(&a).map(|c| c.capture_id()) == Some(first.capture_id()))?;
        {
            let _foreign = b.enter_diagnostic_scope(Some(&second));
            require(current(&a).is_none())?;
            require(current(&b).map(|c| c.capture_id()) == Some(second.capture_id()))?;
            drop(a.lock()?);
        }
        require(current(&a).map(|c| c.capture_id()) == Some(first.capture_id()))?;
        drop(outer);
        require(current(&a).is_none() && a.enter_diagnostic_scope(None).is_none())?;
        require(clock.0.load(Ordering::Relaxed) == 0)
    }

    #[test]
    fn native_wait_and_hold_spans_follow_real_mutex_release() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(1))?);
        let held = catalog.lock()?;
        let (events, child, _root_span) = lock_trace(&catalog, 4)?;
        let worker_catalog = Arc::clone(&catalog);
        let worker = std::thread::spawn(move || {
            let request = RequestBudgetV1::for_duration(Duration::from_secs(3))
                .with_diagnostic_context(child);
            let budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 1,
                    writes: 1,
                    bytes: 1,
                    mutations: 1,
                },
                request,
            );
            let guard = worker_catalog.lock_native_budget(&budget)?;
            drop(guard);
            Ok::<(), IndexError>(())
        });
        let started = Instant::now();
        let wait_started = loop {
            if events.rows().iter().any(|row| {
                matches!(
                    row.kind,
                    DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::CatalogLockWait,
                        ..
                    }
                )
            }) {
                break true;
            }
            if started.elapsed() >= Duration::from_secs(1) {
                break false;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        drop(held);
        worker
            .join()
            .map_err(|_panic| "native lock worker panicked")??;
        require_lock(
            wait_started,
            "native wait span did not begin under contention",
        )?;
        let rows = events.rows();
        let wait = terminal(&rows, DiagnosticStageV1::CatalogLockWait);
        let tenure_events = terminal(&rows, DiagnosticStageV1::CatalogLockHold);
        require_lock(
            matches!(wait.as_slice(), [(_, begin, end, DiagnosticTerminalV1::Ok)] if begin < end),
            "native wait did not finish after held mutex release",
        )?;
        require_lock(
            matches!(tenure_events.as_slice(), [(_, begin, end, DiagnosticTerminalV1::Ok)] if begin < end),
            "native hold span did not end normally",
        )?;
        require_lock(
            events.hold_ends.load(Ordering::Relaxed) == 1
                && events.unlocked_hold_ends.load(Ordering::Relaxed) == 1,
            "hold end was emitted before mutex release",
        )
    }

    #[test]
    fn native_cancel_and_deadline_keep_original_errors_and_wait_terminals() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(1))?);
        let held = catalog.lock()?;
        let (events, child, _root_span) = lock_trace(&catalog, 5)?;
        let cancelled = RequestBudgetV1::unbounded().with_diagnostic_context(child.clone());
        cancelled.cancel_handle().cancel();
        let expired = RequestBudgetV1::until(Instant::now()).with_diagnostic_context(child);
        let limits = WorkLimits {
            reads: 1,
            writes: 1,
            bytes: 1,
            mutations: 1,
        };
        require_lock(
            matches!(
                catalog.lock_native_budget(&WorkBudget::with_request(limits, cancelled)),
                Err(IndexError::Cancelled)
            ),
            "cancel error changed",
        )?;
        require_lock(
            matches!(
                catalog.lock_native_budget(&WorkBudget::with_request(limits, expired)),
                Err(IndexError::ResourceLimited)
            ),
            "deadline error changed",
        )?;
        let rows = events.rows();
        let wait = terminal(&rows, DiagnosticStageV1::CatalogLockWait);
        require_lock(
            matches!(
                wait.as_slice(),
                [
                    (_, _, _, DiagnosticTerminalV1::Cancelled { code: 1 }),
                    (_, _, _, DiagnosticTerminalV1::Error { code: 1 })
                ]
            ),
            "native interruption wait terminals differ",
        )?;
        require_lock(
            terminal(&rows, DiagnosticStageV1::CatalogLockHold).is_empty(),
            "interrupted acquisition emitted a hold span",
        )?;
        drop(held);
        Ok(())
    }

    #[test]
    fn legacy_guard_unwind_releases_mutex_before_aborted_hold_end() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(1))?);
        let (events, child, _root_span) = lock_trace(&catalog, 6)?;
        let _scope = catalog.enter_diagnostic_scope(Some(&child));
        let guard = catalog.lock()?;
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = guard;
            std::panic::resume_unwind(Box::new("expected guard unwind"));
        }));
        require_lock(unwound.is_err(), "legacy guard did not unwind")?;
        let rows = events.rows();
        require_lock(
            matches!(
                terminal(&rows, DiagnosticStageV1::CatalogLockWait).as_slice(),
                [(_, _, _, DiagnosticTerminalV1::Ok)]
            ),
            "legacy wait did not finish before guarded work",
        )?;
        require_lock(
            matches!(
                terminal(&rows, DiagnosticStageV1::CatalogLockHold).as_slice(),
                [(_, _, _, DiagnosticTerminalV1::Aborted)]
            ),
            "unwound hold was reported as successful",
        )?;
        require_lock(
            events.hold_ends.load(Ordering::Relaxed) == 1
                && events.unlocked_hold_ends.load(Ordering::Relaxed) == 1,
            "unwound hold ended before poisoned mutex release",
        )
    }

    #[test]
    fn unwind_restores_the_exact_enclosing_catalog_context() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), std::time::Duration::from_secs(1))?;
        let clock = Arc::new(Clock::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([3; 16]),
            clock.clone(),
            Arc::new(Sink),
        );
        let outer = catalog.enter_diagnostic_scope(Some(&context));
        let result: std::thread::Result<()> =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _off = catalog.enter_diagnostic_scope(None);
                std::panic::resume_unwind(Box::new("expected diagnostic-scope unwind"));
            }));
        require(result.is_err())?;
        require(current(&catalog).map(|c| c.capture_id()) == Some(context.capture_id()))?;
        drop(outer);
        require(current(&catalog).is_none() && clock.0.load(Ordering::Relaxed) == 0)
    }
}
