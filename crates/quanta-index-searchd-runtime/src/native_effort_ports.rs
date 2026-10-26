//! Shared Native effort policy, signal and clock ports. The service and executor depend on this lower contract.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_contract::native_protocol::{NativeOverlayRevision, NativeOverlaySession};
use quanta_index_core::index::{
    Digest, IndexError,
    effort::{EffortPolicy, EffortSignals, ExecutionPhase, PolicyState, StageCostEstimator},
};

pub trait NativeEffortSignalPort: Send + Sync {
    fn sample(&self, now_ms: u64) -> EffortSignals;

    fn sample_for_document(
        &self,
        now_ms: u64,
        _context: &NativeEffortDocumentContext<'_>,
    ) -> EffortSignals {
        self.sample(now_ms)
    }
}

/// A live overlay corpus observation. The caller obtains `unchanged_for`
/// from the same authenticated snapshot used by the query, not query age.
#[derive(Clone, Copy)]
pub struct NativeEffortDocumentContext<'a> {
    pub session: NativeOverlaySession,
    pub identity: Digest,
    pub revisions: &'a [NativeOverlayRevision],
    pub unchanged_for: Option<Duration>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ObservationScope {
    General,
    Document {
        session: NativeOverlaySession,
        identity: Digest,
    },
}

/// Milliseconds from one monotonic origin. Direct query controls may use a
/// query-local origin; shared observation history requires one service origin.
pub trait NativeEffortClockPort: Send + Sync {
    fn now_ms(&self) -> u64;
}

struct MonotonicEffortClock(Instant);

impl NativeEffortClockPort for MonotonicEffortClock {
    fn now_ms(&self) -> u64 {
        // MAX is rejected as an invalid clock sentinel at every observation.
        u64::try_from(self.0.elapsed().as_millis()).map_or(u64::MAX, std::convert::identity)
    }
}

struct ObservationState {
    scope: Option<ObservationScope>,
    /// Most recently completed sample, in service-clock milliseconds.
    last_sample: Option<u64>,
    /// First instant of the current contiguous clear window, in milliseconds.
    clear_since: Option<u64>,
    /// Exclusive end of the latest clear sample, in milliseconds.
    clear_valid_until: Option<u64>,
}

/// O(1) service-local pressure history.
///
/// It stores no request grant, phase, proposal, provider result, or user
/// preference. Its lock covers only a bounded clock read, local signal sample,
/// and policy proposal. Signal ports and policies must not perform IO or
/// reenter this history.
pub struct NativeEffortObservationHistory {
    clock: Arc<dyn NativeEffortClockPort>,
    state: Mutex<ObservationState>,
    fast_residence_since_ms: AtomicU64,
}

impl NativeEffortObservationHistory {
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(Arc::new(MonotonicEffortClock(Instant::now())))
    }

    #[must_use]
    pub fn with_clock(clock: Arc<dyn NativeEffortClockPort>) -> Self {
        let fast_residence_since_ms = clock.now_ms();
        Self {
            clock,
            state: Mutex::new(ObservationState {
                scope: None,
                last_sample: None,
                clear_since: None,
                clear_valid_until: None,
            }),
            fast_residence_since_ms: AtomicU64::new(fast_residence_since_ms),
        }
    }

    pub(crate) fn observe_for_document<T>(
        &self,
        signals: &dyn NativeEffortSignalPort,
        context: Option<&NativeEffortDocumentContext<'_>>,
        propose: impl FnOnce(u64, EffortSignals, PolicyState) -> Result<(T, PolicyState), IndexError>,
    ) -> Result<T, IndexError> {
        let mut state = self.state.lock().map_err(|_poison| {
            IndexError::BackendViolation("effort observation history poisoned".into())
        })?;
        let scope = context.map_or(ObservationScope::General, |document| {
            ObservationScope::Document {
                session: document.session,
                identity: document.identity,
            }
        });
        let residence_at_start = self.fast_residence_since_ms.load(Ordering::Acquire);
        let sample_started_ms = self.clock.now_ms();
        if sample_started_ms == u64::MAX {
            return Err(IndexError::BackendViolation(
                "effort observation clock overflowed".into(),
            ));
        }
        if state
            .last_sample
            .is_some_and(|previous| sample_started_ms < previous)
            || residence_at_start == u64::MAX
            || sample_started_ms < residence_at_start
        {
            return Err(IndexError::BackendViolation(
                "effort observation clock regressed".into(),
            ));
        }
        let sample = context.map_or_else(
            || signals.sample(sample_started_ms),
            |document| signals.sample_for_document(sample_started_ms, document),
        );
        // The source may block. A prediction is only fresh at the instant
        // sampling completes; source timestamps remain at sample start.
        let fast_residence_since_ms = self.fast_residence_since_ms.load(Ordering::Acquire);
        let now_ms = self.clock.now_ms();
        if now_ms == u64::MAX {
            return Err(IndexError::BackendViolation(
                "effort observation clock overflowed".into(),
            ));
        }
        if now_ms < sample_started_ms {
            return Err(IndexError::BackendViolation(
                "effort observation clock regressed during sampling".into(),
            ));
        }
        // A completed Deep delivery publishes this marker without taking the
        // pressure-history lock. This read is the residence snapshot used for
        // the proposal; concurrent later deliveries affect later proposals.
        if fast_residence_since_ms == u64::MAX || now_ms < fast_residence_since_ms {
            return Err(IndexError::BackendViolation(
                "effort observation residence clock regressed or overflowed".into(),
            ));
        }
        let (clear_since, clear_valid_until) = if state.scope == Some(scope) {
            (state.clear_since, state.clear_valid_until)
        } else {
            (None, None)
        };
        let policy_state = PolicyState {
            phase: ExecutionPhase::Fast,
            entered_at_ms: fast_residence_since_ms,
            clear_since_ms: clear_since,
            clear_valid_until_ms: clear_valid_until,
            automatic_transitions: 0,
        };
        let (result, next) = propose(now_ms, sample, policy_state)?;
        if next.clear_since_ms.is_some() != next.clear_valid_until_ms.is_some()
            || next.clear_since_ms.is_some_and(|since| since > now_ms)
            || next
                .clear_valid_until_ms
                .is_some_and(|until| until < now_ms)
        {
            return Err(IndexError::BackendViolation(
                "effort policy returned invalid observation window".into(),
            ));
        }
        state.last_sample = Some(now_ms);
        state.scope = Some(scope);
        state.clear_since = next.clear_since_ms;
        state.clear_valid_until = next.clear_valid_until_ms;
        drop(state);
        Ok(result)
    }

    /// Reset Fast residence only after an automatic Deep stage was delivered.
    pub(crate) fn record_auto_deep_delivery(&self) {
        let now_ms = self.clock.now_ms();
        // Even a regressed clock cannot move a previous delivery backward.
        // MAX is an invalid sentinel: the next observation fails closed.
        let _previous_residence = self
            .fast_residence_since_ms
            .fetch_max(now_ms, Ordering::AcqRel);
    }
}

impl Default for NativeEffortObservationHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
pub struct NativeEffortControl<'a> {
    pub policy: &'a dyn EffortPolicy,
    pub estimator: &'a dyn StageCostEstimator,
    pub signals: &'a dyn NativeEffortSignalPort,
    /// None uses the process monotonic clock from the start of this query.
    pub clock: Option<&'a dyn NativeEffortClockPort>,
    /// Installed services share clear-window observations across requests.
    pub history: Option<&'a NativeEffortObservationHistory>,
}

/// Installed once before the service starts accepting streamed effort queries.
/// Missing signals remain Unknown through the provided signal port.
pub struct NativeEffortConfiguration {
    pub policy: Arc<dyn EffortPolicy>,
    pub estimator: Arc<dyn StageCostEstimator>,
    pub signals: Arc<dyn NativeEffortSignalPort>,
    pub history: Arc<NativeEffortObservationHistory>,
}

impl NativeEffortConfiguration {
    pub(super) fn control(&self) -> NativeEffortControl<'_> {
        NativeEffortControl {
            policy: self.policy.as_ref(),
            estimator: self.estimator.as_ref(),
            signals: self.signals.as_ref(),
            clock: None,
            history: Some(self.history.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use quanta_index_contract::native_protocol::{
        NativeInteractionPublisherId, NativeOverlayRevision, NativeOverlaySession,
    };
    use quanta_index_core::index::{
        CollectionId, Digest, IndexError, OwnerKey, ProducerId, SlotId, StableOwnerId, TenantId,
        WorkspaceKey, WorktreeId,
        effort::{EffortSignals, PolicyState, Signal, SignalSample, UnknownReason},
    };

    use super::{
        NativeEffortClockPort, NativeEffortDocumentContext, NativeEffortObservationHistory,
        NativeEffortSignalPort,
    };

    struct Clock(AtomicU64);

    impl NativeEffortClockPort for Clock {
        fn now_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct BlockingFirstSample {
        first: AtomicBool,
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl NativeEffortSignalPort for BlockingFirstSample {
        fn sample(&self, now_ms: u64) -> EffortSignals {
            if self.first.swap(false, Ordering::SeqCst) {
                assert!(self.entered.send(()).is_ok());
                let release = self.release.lock();
                assert!(release.is_ok());
                if let Ok(release) = release {
                    assert!(release.recv_timeout(Duration::from_secs(3)).is_ok());
                }
            }
            EffortSignals {
                interaction_idle_ms: Signal::Unknown(UnknownReason::Missing),
                interactive_pressure_bps: Signal::Known(SignalSample {
                    value: 0,
                    observed_at_ms: now_ms,
                    valid_for_ms: 20,
                }),
                index_backlog: Signal::Unknown(UnknownReason::Missing),
                dispatch_lag_ms: Signal::Unknown(UnknownReason::Missing),
                memory_headroom_bytes: Signal::Unknown(UnknownReason::Missing),
                provider: Signal::Unknown(UnknownReason::Missing),
                quality_confidence_bps: Signal::Unknown(UnknownReason::Missing),
            }
        }
    }

    fn checked(condition: bool, message: &str) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(message.into()))
        }
    }

    fn mark_clear(
        now: u64,
        _sample: EffortSignals,
        mut state: PolicyState,
    ) -> Result<(Option<u64>, PolicyState), IndexError> {
        let previous = state.clear_since_ms;
        state.clear_since_ms = Some(now);
        state.clear_valid_until_ms = Some(now.checked_add(20).ok_or_else(|| {
            IndexError::BackendViolation("test clear window exceeds clock range".into())
        })?);
        Ok((previous, state))
    }

    #[test]
    fn document_scope_does_not_inherit_another_corpus_clear_window() -> Result<(), IndexError> {
        let clock = Arc::new(Clock(AtomicU64::new(0)));
        let history = NativeEffortObservationHistory::with_clock(clock.clone());
        let (entered, _entered_rx) = mpsc::channel();
        let (_release_tx, release) = mpsc::channel();
        let source = BlockingFirstSample {
            first: AtomicBool::new(false),
            entered,
            release: Mutex::new(release),
        };
        let rows = [NativeOverlayRevision {
            owner: OwnerKey {
                producer: ProducerId([7; 16]),
                stable_id: StableOwnerId([8; 16]),
            },
            revision: 1,
            deleted: false,
            retired: false,
        }];
        let context = NativeEffortDocumentContext {
            session: NativeOverlaySession {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                publisher: NativeInteractionPublisherId([5; 32]),
                epoch: 1,
            },
            identity: Digest([6; 32]),
            revisions: &rows,
            unchanged_for: Some(Duration::from_millis(100)),
        };
        let first = history.observe_for_document(&source, Some(&context), mark_clear)?;
        checked(first.is_none(), "first corpus inherited a clear window")?;
        clock.0.store(1, Ordering::SeqCst);
        let same = history.observe_for_document(&source, Some(&context), mark_clear)?;
        checked(same == Some(0), "same corpus lost its clear window")?;
        let changed = NativeEffortDocumentContext {
            identity: Digest([9; 32]),
            ..context
        };
        clock.0.store(2, Ordering::SeqCst);
        let next = history.observe_for_document(&source, Some(&changed), mark_clear)?;
        checked(
            next.is_none(),
            "changed corpus inherited the old clear window",
        )?;
        clock.0.store(3, Ordering::SeqCst);
        let general = history.observe_for_document(&source, None, mark_clear)?;
        checked(
            general.is_none(),
            "general query inherited document clear window",
        )?;
        Ok(())
    }

    #[test]
    fn concurrent_observations_serialize_and_slow_sample_cannot_create_dwell()
    -> Result<(), IndexError> {
        let clock = Arc::new(Clock(AtomicU64::new(0)));
        let observation_clock: Arc<dyn NativeEffortClockPort> = clock.clone();
        let history = Arc::new(NativeEffortObservationHistory::with_clock(
            observation_clock,
        ));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let source = Arc::new(BlockingFirstSample {
            first: AtomicBool::new(true),
            entered: entered_tx,
            release: Mutex::new(release_rx),
        });
        let first_history = Arc::clone(&history);
        let first_source = Arc::clone(&source);
        let first = thread::spawn(move || {
            first_history.observe_for_document(
                first_source.as_ref(),
                None,
                |now, sample, mut state| {
                    let fresh = sample.interactive_pressure_bps.fresh(now).is_some();
                    if fresh {
                        state.clear_since_ms = Some(now);
                        state.clear_valid_until_ms = now.checked_add(20);
                    }
                    Ok(((now, fresh), state))
                },
            )
        });
        if let Err(error) = entered_rx.recv_timeout(Duration::from_secs(3)) {
            let _release = release_tx.send(());
            let join = first.join();
            if join.is_err() {
                return Err(IndexError::BackendViolation(
                    "first observation panicked before sample entry".into(),
                ));
            }
            return Err(IndexError::BackendViolation(format!(
                "first sample did not enter: {error}"
            )));
        }
        clock.0.store(25, Ordering::SeqCst);
        let second_history = Arc::clone(&history);
        let second_source = Arc::clone(&source);
        let second = thread::spawn(move || {
            second_history.observe_for_document(
                second_source.as_ref(),
                None,
                |now, sample, mut state| {
                    let prior_clear = state.clear_since_ms;
                    let fresh = sample.interactive_pressure_bps.fresh(now).is_some();
                    if fresh {
                        state.clear_since_ms = Some(now);
                        state.clear_valid_until_ms = now.checked_add(20);
                    }
                    Ok(((now, fresh, prior_clear), state))
                },
            )
        });
        let released = release_tx.send(()).map_err(|error| {
            IndexError::BackendViolation(format!("release first sample: {error}"))
        });
        let first_result = first
            .join()
            .map_err(|_panic| IndexError::BackendViolation("first observation panicked".into()));
        let second_result = second
            .join()
            .map_err(|_panic| IndexError::BackendViolation("second observation panicked".into()));
        released?;
        let first_result = first_result??;
        let second_result = second_result??;
        checked(
            first_result == (25, false),
            "slow sample counted as fresh dwell",
        )?;
        checked(
            second_result == (25, true, None),
            "concurrent sample overwrote the prior observation",
        )?;
        let state = history.state.lock().map_err(|_poison| {
            IndexError::BackendViolation("history lock poisoned after test".into())
        })?;
        checked(
            state.clear_since == Some(25),
            "fresh sample was not retained",
        )?;
        drop(state);
        Ok(())
    }
}
