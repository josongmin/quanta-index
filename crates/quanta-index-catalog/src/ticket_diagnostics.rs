//! B81 synchronous ticket validation timings for the current OS thread.
//! These observations carry no Catalog authority or request admission.

use std::{
    cell::Cell,
    marker::PhantomData,
    rc::Rc,
    time::{Duration, Instant},
};

use crate::SqliteCatalog;

const FIRST_LANE: u16 = 200;
const LANE_COUNT: usize = 6;

/// Aggregate timing for ticket validation lanes 200 through 205, in lane order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TicketValidationTimings {
    pub elapsed: [Duration; LANE_COUNT],
    pub calls: [u64; LANE_COUNT],
    pub failed: [u64; LANE_COUNT],
    /// CPU time is present only for valid same-thread, monotonic sample pairs.
    /// Compare `cpu_calls` with `calls` before interpreting an aggregate.
    pub cpu_elapsed: [Option<Duration>; LANE_COUNT],
    pub cpu_calls: [u64; LANE_COUNT],
    pub connection_busy_polls: u64,
    pub connection_retry_sleep: Duration,
}

#[derive(Clone, Copy)]
struct Binding {
    owner: u64,
    catalog_ptr: usize,
    cpu_sampler: Option<fn() -> Option<u64>>,
    timings: TicketValidationTimings,
}

#[derive(Clone, Copy)]
struct State {
    next_owner: u64,
    current: Option<Binding>,
}

thread_local! {
    static STATE: Cell<State> = const { Cell::new(State { next_owner: 0, current: None }) };
}

/// Lexical, thread-local observation of synchronous ticket validation.
#[must_use = "dropping the scope restores the previous observation"]
pub struct TicketValidationObservation<'a> {
    owner: u64,
    catalog_ptr: usize,
    previous: Option<Binding>,
    _catalog: &'a SqliteCatalog,
    _not_send_sync: PhantomData<Rc<()>>,
}

impl<'a> TicketValidationObservation<'a> {
    /// Starts a zeroed observation for this Catalog, saving any enclosing one.
    /// Refuses a scope if the thread's owner sequence is exhausted.
    pub fn begin(
        catalog: &'a SqliteCatalog,
        cpu_sampler: Option<fn() -> Option<u64>>,
    ) -> Option<Self> {
        STATE.with(|slot| {
            let mut state = slot.get();
            let owner = state.next_owner.checked_add(1)?;
            let catalog_ptr = std::ptr::from_ref(catalog).addr();
            let previous = state.current;
            state.next_owner = owner;
            state.current = Some(Binding {
                owner,
                catalog_ptr,
                cpu_sampler,
                timings: TicketValidationTimings::default(),
            });
            slot.set(state);
            Some(Self {
                owner,
                catalog_ptr,
                previous,
                _catalog: catalog,
                _not_send_sync: PhantomData,
            })
        })
    }

    /// Copies the observation only while this scope is active on this thread.
    #[must_use]
    pub fn snapshot(&self) -> Option<TicketValidationTimings> {
        STATE.with(|slot| {
            slot.get().current.and_then(|binding| {
                (binding.owner == self.owner && binding.catalog_ptr == self.catalog_ptr)
                    .then_some(binding.timings)
            })
        })
    }
}

impl Drop for TicketValidationObservation<'_> {
    fn drop(&mut self) {
        STATE.with(|slot| {
            let mut state = slot.get();
            state.current = match state.current {
                Some(binding)
                    if binding.owner == self.owner && binding.catalog_ptr == self.catalog_ptr =>
                {
                    self.previous
                }
                _ => None,
            };
            slot.set(state);
        });
    }
}

/// Measures one completed synchronous phase without changing its result.
/// Invalid lanes and unmatched scopes execute `work` without reading a clock.
pub(crate) fn observe<T, E>(
    catalog: &SqliteCatalog,
    lane: u16,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let index = usize::from(lane.saturating_sub(FIRST_LANE));
    if lane < FIRST_LANE || index >= LANE_COUNT {
        return work();
    }
    let catalog_ptr = std::ptr::from_ref(catalog).addr();
    let binding = STATE.with(|slot| {
        slot.get()
            .current
            .filter(|binding| binding.catalog_ptr == catalog_ptr)
    });
    let Some(binding) = binding else {
        return work();
    };

    let owner = binding.owner;
    let cpu_started = binding.cpu_sampler.and_then(|sample| sample());
    let started = Instant::now();
    let result = work();
    let elapsed = started.elapsed();
    // A nested observation may still be active after `work`. Never charge its
    // CPU sample to the enclosing owner or call its sampler in that case.
    let cpu_finished = STATE.with(|slot| {
        slot.get().current.and_then(|current| {
            (current.owner == owner && current.catalog_ptr == catalog_ptr)
                .then(|| binding.cpu_sampler.and_then(|sample| sample()))
                .flatten()
        })
    });
    STATE.with(|slot| {
        let mut state = slot.get();
        if let Some(mut binding) = state.current {
            if binding.owner != owner || binding.catalog_ptr != catalog_ptr {
                return;
            }
            if let (Some(total), Some(calls), Some(failed)) = (
                binding.timings.elapsed.get_mut(index),
                binding.timings.calls.get_mut(index),
                binding.timings.failed.get_mut(index),
            ) {
                *total = total.saturating_add(elapsed);
                *calls = calls.saturating_add(1);
                if result.is_err() {
                    *failed = failed.saturating_add(1);
                }
                if let (Some(before), Some(after)) = (cpu_started, cpu_finished)
                    && let Some(delta) = after.checked_sub(before)
                    && let (Some(cpu_elapsed), Some(cpu_calls)) = (
                        binding.timings.cpu_elapsed.get_mut(index),
                        binding.timings.cpu_calls.get_mut(index),
                    )
                {
                    *cpu_elapsed = Some(
                        (*cpu_elapsed)
                            .unwrap_or(Duration::ZERO)
                            .saturating_add(Duration::from_nanos(delta)),
                    );
                    *cpu_calls = cpu_calls.saturating_add(1);
                }
                state.current = Some(binding);
                slot.set(state);
            }
        }
    });
    result
}

/// Counts a bounded Catalog mutex retry and measures its actual sleep wall time.
/// Off and foreign Catalog work performs the same sleep without reading a clock.
pub(crate) fn retry_sleep(catalog: &SqliteCatalog, duration: Duration) {
    let catalog_ptr = std::ptr::from_ref(catalog).addr();
    let owner = STATE.with(|slot| {
        slot.get()
            .current
            .and_then(|binding| (binding.catalog_ptr == catalog_ptr).then_some(binding.owner))
    });
    let Some(owner) = owner else {
        std::thread::sleep(duration);
        return;
    };

    let started = Instant::now();
    std::thread::sleep(duration);
    let elapsed = started.elapsed();
    STATE.with(|slot| {
        let mut state = slot.get();
        if let Some(mut binding) = state.current
            && binding.owner == owner
            && binding.catalog_ptr == catalog_ptr
        {
            binding.timings.connection_busy_polls =
                binding.timings.connection_busy_polls.saturating_add(1);
            binding.timings.connection_retry_sleep = binding
                .timings
                .connection_retry_sleep
                .saturating_add(elapsed);
            state.current = Some(binding);
            slot.set(state);
        }
    });
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions independently verify diagnostic scope isolation"
)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque, error::Error};

    thread_local! {
        static CPU_SAMPLES: RefCell<VecDeque<Option<u64>>> = const { RefCell::new(VecDeque::new()) };
        static CPU_SAMPLE_CALLS: Cell<u64> = const { Cell::new(0) };
    }

    fn fake_cpu_now() -> Option<u64> {
        CPU_SAMPLE_CALLS.with(|calls| calls.set(calls.get().saturating_add(1)));
        CPU_SAMPLES.with(|samples| samples.borrow_mut().pop_front().flatten())
    }

    fn reset_cpu_samples(samples: impl IntoIterator<Item = Option<u64>>) {
        CPU_SAMPLES.with(|queue| *queue.borrow_mut() = samples.into_iter().collect());
        CPU_SAMPLE_CALLS.with(|calls| calls.set(0));
    }

    type TestResult = Result<(), Box<dyn Error>>;

    fn catalog() -> Result<(tempfile::TempDir, SqliteCatalog), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        Ok((root, catalog))
    }

    fn begin(catalog: &SqliteCatalog) -> Result<TicketValidationObservation<'_>, Box<dyn Error>> {
        TicketValidationObservation::begin(catalog, None)
            .ok_or_else(|| std::io::Error::other("ticket observation owner exhausted").into())
    }

    #[test]
    fn nested_scope_restores_outer_and_excludes_other_catalog() -> TestResult {
        let (_root_a, catalog_a) = catalog()?;
        let (_root_b, catalog_b) = catalog()?;
        let outer = begin(&catalog_a)?;
        assert_eq!(observe(&catalog_a, 200, || Ok::<_, ()>(7)), Ok(7));
        assert_eq!(observe(&catalog_b, 200, || Ok::<_, ()>(9)), Ok(9));
        let before_inner = outer.snapshot();
        assert_eq!(
            before_inner.and_then(|timings| timings.calls.first().copied()),
            Some(1)
        );

        {
            let inner = begin(&catalog_b)?;
            assert_eq!(outer.snapshot(), None);
            assert_eq!(observe(&catalog_a, 201, || Ok::<_, ()>(())), Ok(()));
            assert_eq!(
                observe(&catalog_b, 201, || Err::<(), _>("inner")),
                Err("inner")
            );
            assert_eq!(
                inner
                    .snapshot()
                    .and_then(|timings| timings.calls.get(1).copied()),
                Some(1)
            );
            assert_eq!(
                inner
                    .snapshot()
                    .and_then(|timings| timings.failed.get(1).copied()),
                Some(1)
            );
        }

        assert_eq!(outer.snapshot(), before_inner);
        assert_eq!(observe(&catalog_a, 200, || Ok::<_, ()>(8)), Ok(8));
        assert_eq!(
            outer
                .snapshot()
                .and_then(|timings| timings.calls.first().copied()),
            Some(2)
        );
        Ok(())
    }

    #[test]
    fn out_of_order_drop_clears_stale_scope() -> TestResult {
        let (_root, catalog) = catalog()?;
        let outer = begin(&catalog)?;
        let inner = begin(&catalog)?;
        drop(outer);
        assert_eq!(inner.snapshot(), None);
        assert_eq!(observe(&catalog, 202, || Ok::<_, ()>(1)), Ok(1));
        drop(inner);
        let fresh = begin(&catalog)?;
        assert_eq!(fresh.snapshot(), Some(TicketValidationTimings::default()));
        Ok(())
    }

    #[test]
    fn retry_sleep_counts_only_matching_active_catalog() -> TestResult {
        let (_root_a, catalog_a) = catalog()?;
        let (_root_b, catalog_b) = catalog()?;
        retry_sleep(&catalog_a, Duration::ZERO);

        let outer = begin(&catalog_a)?;
        retry_sleep(&catalog_b, Duration::ZERO);
        assert_eq!(
            outer
                .snapshot()
                .map(|timings| timings.connection_busy_polls),
            Some(0)
        );
        retry_sleep(&catalog_a, Duration::ZERO);
        assert_eq!(
            outer
                .snapshot()
                .map(|timings| timings.connection_busy_polls),
            Some(1)
        );

        {
            let inner = begin(&catalog_b)?;
            retry_sleep(&catalog_a, Duration::ZERO);
            retry_sleep(&catalog_b, Duration::ZERO);
            assert_eq!(
                inner
                    .snapshot()
                    .map(|timings| timings.connection_busy_polls),
                Some(1)
            );
        }
        assert_eq!(
            outer
                .snapshot()
                .map(|timings| timings.connection_busy_polls),
            Some(1)
        );
        Ok(())
    }

    #[test]
    fn observations_do_not_leak_across_threads() -> TestResult {
        let (_root, catalog) = catalog()?;
        let outer = begin(&catalog)?;
        let other = std::thread::scope(|threads| {
            let other_thread =
                threads.spawn(|| -> Result<TicketValidationTimings, std::io::Error> {
                    assert_eq!(observe(&catalog, 202, || Ok::<_, ()>(1)), Ok(1));
                    let local =
                        TicketValidationObservation::begin(&catalog, None).ok_or_else(|| {
                            std::io::Error::other("ticket observation owner exhausted")
                        })?;
                    assert_eq!(observe(&catalog, 202, || Ok::<_, ()>(2)), Ok(2));
                    assert_eq!(
                        local
                            .snapshot()
                            .and_then(|timings| timings.calls.get(2).copied()),
                        Some(1)
                    );
                    local
                        .snapshot()
                        .ok_or_else(|| std::io::Error::other("scope became inactive"))
                });
            other_thread
                .join()
                .map_err(|panic| std::io::Error::other(format!("thread panicked: {panic:?}")))?
        })?;
        assert_eq!(other.calls.get(2), Some(&1));
        assert_eq!(outer.snapshot(), Some(TicketValidationTimings::default()));
        Ok(())
    }

    #[test]
    fn result_and_error_are_preserved() -> TestResult {
        let (_root, catalog) = catalog()?;
        let scope = begin(&catalog)?;
        assert_eq!(observe(&catalog, 203, || Ok::<_, &str>("ok")), Ok("ok"));
        assert_eq!(
            observe(&catalog, 203, || Err::<(), _>("failed")),
            Err("failed")
        );
        assert_eq!(
            scope
                .snapshot()
                .and_then(|timings| timings.calls.get(3).copied()),
            Some(2)
        );
        assert_eq!(
            scope
                .snapshot()
                .and_then(|timings| timings.failed.get(3).copied()),
            Some(1)
        );
        assert_eq!(observe(&catalog, 206, || Ok::<_, ()>(())), Ok(()));
        assert_eq!(
            scope
                .snapshot()
                .and_then(|timings| timings.calls.get(5).copied()),
            Some(0)
        );
        Ok(())
    }

    #[test]
    fn cpu_pairs_cover_success_and_refusal_without_changing_results() -> TestResult {
        let (_root, catalog) = catalog()?;
        reset_cpu_samples([Some(1_000), Some(5_000), Some(7_000), Some(9_000)]);
        let scope = TicketValidationObservation::begin(&catalog, Some(fake_cpu_now))
            .ok_or("ticket observation owner exhausted")?;
        assert_eq!(observe(&catalog, 203, || Ok::<_, &str>(7)), Ok(7));
        assert_eq!(
            observe(&catalog, 203, || Err::<(), _>("refused")),
            Err("refused")
        );
        let timings = scope
            .snapshot()
            .ok_or("observation scope became inactive")?;
        assert_eq!(timings.calls.get(3), Some(&2));
        assert_eq!(timings.failed.get(3), Some(&1));
        assert_eq!(timings.cpu_calls.get(3), Some(&2));
        assert_eq!(
            timings.cpu_elapsed.get(3),
            Some(&Some(Duration::from_nanos(6_000)))
        );
        assert_eq!(CPU_SAMPLE_CALLS.with(Cell::get), 4);
        Ok(())
    }

    #[test]
    fn unavailable_or_regressing_cpu_samples_are_not_zero_durations() -> TestResult {
        let (_root, catalog) = catalog()?;
        reset_cpu_samples([
            None,
            Some(8),
            Some(10),
            None,
            Some(15),
            Some(14),
            Some(20),
            Some(30),
        ]);
        let scope = TicketValidationObservation::begin(&catalog, Some(fake_cpu_now))
            .ok_or("ticket observation owner exhausted")?;
        for _ in 0..4 {
            assert_eq!(observe(&catalog, 204, || Ok::<_, ()>(())), Ok(()));
        }
        let timings = scope
            .snapshot()
            .ok_or("observation scope became inactive")?;
        assert_eq!(timings.calls.get(4), Some(&4));
        assert_eq!(timings.cpu_calls.get(4), Some(&1));
        assert_eq!(
            timings.cpu_elapsed.get(4),
            Some(&Some(Duration::from_nanos(10)))
        );
        assert_eq!(CPU_SAMPLE_CALLS.with(Cell::get), 8);
        Ok(())
    }

    #[test]
    fn cpu_sampling_respects_off_foreign_invalid_and_nested_scopes() -> TestResult {
        let (_root_a, catalog_a) = catalog()?;
        let (_root_b, catalog_b) = catalog()?;
        reset_cpu_samples([Some(100), Some(120), Some(200), Some(230)]);
        assert_eq!(observe(&catalog_a, 200, || Ok::<_, ()>(())), Ok(()));
        let outer = TicketValidationObservation::begin(&catalog_a, Some(fake_cpu_now))
            .ok_or("ticket observation owner exhausted")?;
        assert_eq!(observe(&catalog_b, 200, || Ok::<_, ()>(())), Ok(()));
        assert_eq!(observe(&catalog_a, 206, || Ok::<_, ()>(())), Ok(()));
        assert_eq!(CPU_SAMPLE_CALLS.with(Cell::get), 0);
        assert_eq!(observe(&catalog_a, 200, || Ok::<_, ()>(())), Ok(()));
        {
            let inner = TicketValidationObservation::begin(&catalog_b, Some(fake_cpu_now))
                .ok_or("ticket observation owner exhausted")?;
            assert_eq!(observe(&catalog_a, 201, || Ok::<_, ()>(())), Ok(()));
            assert_eq!(observe(&catalog_b, 201, || Ok::<_, ()>(())), Ok(()));
            assert_eq!(outer.snapshot(), None);
            let timings = inner.snapshot().ok_or("inner scope became inactive")?;
            assert_eq!(timings.cpu_calls.get(1), Some(&1));
            assert_eq!(
                timings.cpu_elapsed.get(1),
                Some(&Some(Duration::from_nanos(30)))
            );
        }
        let timings = outer.snapshot().ok_or("outer scope was not restored")?;
        assert_eq!(timings.cpu_calls.first(), Some(&1));
        assert_eq!(
            timings.cpu_elapsed.first(),
            Some(&Some(Duration::from_nanos(20)))
        );
        assert_eq!(CPU_SAMPLE_CALLS.with(Cell::get), 4);
        Ok(())
    }
}
