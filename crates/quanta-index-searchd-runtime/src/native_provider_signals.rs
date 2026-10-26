//! Replaceable provider ingress prediction for Native Auto.
//!
//! Pool readiness is observed under the same registry lock used by physical
//! attempt admission. It is not a slot reservation, remote endpoint health,
//! provider permission, quality, or a successful execution receipt.

use std::sync::Arc;

use quanta_index_core::index::effort::{
    ProviderAvailability, ProviderMode, ProviderSignal, Signal, SignalSample, UnknownReason,
};
use quanta_index_embed::{ProviderAttemptIngressSnapshot, ProviderAttemptPool};

pub trait NativeProviderSignalPort: Send + Sync {
    /// `now_ms` is the caller's query clock. The sample starts no later than
    /// the actual pool observation, so an intervening call can only shorten
    /// its effective TTL.
    fn sample(&self, now_ms: u64, valid_for_ms: u64) -> Signal<ProviderSignal>;
}

/// A local attempt-ingress observation for a provider with a configured mode.
/// Remote mode describes the transport, not the health of the remote service.
pub struct NativeProviderPoolSignals {
    pool: Arc<ProviderAttemptPool>,
    mode: ProviderMode,
}

impl NativeProviderPoolSignals {
    #[must_use]
    pub fn new(pool: Arc<ProviderAttemptPool>, mode: ProviderMode) -> Self {
        Self { pool, mode }
    }
}

impl NativeProviderSignalPort for NativeProviderPoolSignals {
    fn sample(&self, now_ms: u64, valid_for_ms: u64) -> Signal<ProviderSignal> {
        if valid_for_ms == 0 {
            return Signal::Unknown(UnknownReason::Missing);
        }
        let availability = match self.pool.ingress_snapshot() {
            ProviderAttemptIngressSnapshot::Ready { .. } => ProviderAvailability::Ready,
            ProviderAttemptIngressSnapshot::Busy { .. } => ProviderAvailability::Busy,
            ProviderAttemptIngressSnapshot::Offline(_) => ProviderAvailability::Offline,
        };
        Signal::Known(SignalSample {
            value: ProviderSignal {
                availability,
                mode: self.mode,
            },
            observed_at_ms: now_ms,
            valid_for_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Mutex, mpsc},
        time::{Duration, Instant},
    };

    use quanta_index_contract::native_protocol::NativeInteractionPublisherId;
    use quanta_index_core::index::{
        IndexError,
        effort::{
            CancellationStatus, ContextStatus, CoverageExactness, DecisionFrame, DecisionReason,
            DeclaredCostEstimator, EffortAuthority, EffortGovernor, EffortPolicy, EffortPreference,
            ExecutionPhase, HysteresisConfig, HysteresisPolicy, PolicyState, PressureThresholds,
            ProviderPermission, RequiredProofStatus, ResourceNeed, ResourceScheduler,
            StageObligation, StageOffer,
        },
    };

    use super::*;
    use crate::{
        native_auto_signals::{
            NativeAutoSignalConfig, NativeAutoSignals, NativeInteractionActivityPort,
            ResourceSource, WorkerSource,
        },
        native_effort_ports::NativeEffortSignalPort,
        native_interaction::{InteractionClock, NativeInteractionRecorder},
        native_resources::{NativeClassSnapshot, NativeResourcePlan, NativeResourceSnapshot},
        native_worker::{NativeLaneSnapshot, NativeWorkerSnapshot, NativeWorkerStatus},
    };

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) => {
                    if left != right {
                        return Err(format!(
                            "assertion failed: {} == {}; left: {left:?}, right: {right:?}",
                            stringify!($left),
                            stringify!($right)
                        )
                        .into());
                    }
                }
            }
        }};
    }

    struct DrainPoolOnDrop(Arc<ProviderAttemptPool>);

    impl Drop for DrainPoolOnDrop {
        fn drop(&mut self) {
            let _report = self.0.drain(Duration::from_secs(2));
        }
    }

    struct TestClock(Mutex<Instant>);

    impl TestClock {
        fn advance(&self, duration: Duration) {
            let mut now = self.0.lock().expect("test clock lock");
            *now = now.checked_add(duration).expect("test clock advance");
        }
    }

    impl InteractionClock for TestClock {
        fn now(&self) -> Instant {
            *self.0.lock().expect("test clock lock")
        }
    }

    struct EmptyWorker;

    impl WorkerSource for EmptyWorker {
        fn observe(&self, _max_age: Duration) -> Result<NativeWorkerSnapshot, IndexError> {
            let empty = NativeLaneSnapshot {
                queued: 0,
                active: 0,
                dispatch_lag_ms: Signal::Unknown(UnknownReason::Missing),
            };
            Ok(NativeWorkerSnapshot {
                observed_at_ms: 0,
                status: NativeWorkerStatus::Running,
                core: empty,
                enrichment: empty,
            })
        }
    }

    struct EmptyResources(NativeResourcePlan);

    impl ResourceSource for EmptyResources {
        fn observe(&self) -> Result<NativeResourceSnapshot, IndexError> {
            Ok(NativeResourceSnapshot {
                capacity: self.0.capacity,
                reserved: ResourceNeed::default(),
                live_memory_bytes: 0,
                retained_memory_bytes: 0,
                pending_escape_bytes: 0,
                control: NativeClassSnapshot::default(),
                background_control: NativeClassSnapshot::default(),
                maintenance_control: NativeClassSnapshot::default(),
                interactive: NativeClassSnapshot::default(),
                core: NativeClassSnapshot::default(),
                enrichment: NativeClassSnapshot::default(),
            })
        }
    }

    #[derive(Default)]
    struct OnePermit(Mutex<bool>);

    impl ResourceScheduler for OnePermit {
        type Token = ();

        fn try_reserve(&self, _need: ResourceNeed) -> Option<Self::Token> {
            let Ok(mut held) = self.0.lock() else {
                return None;
            };
            if *held {
                return None;
            }
            *held = true;
            Some(())
        }

        fn release(&self, _token: Self::Token) {
            *self.0.lock().expect("test reservation lock") = false;
        }
    }

    fn floor() -> ResourceNeed {
        ResourceNeed {
            cpu_units: 1,
            io_bytes: 1,
            memory_bytes: 100,
            provider_units: 0,
        }
    }

    fn clear_resource_plan() -> NativeResourcePlan {
        NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 10,
                io_bytes: 10_000,
                memory_bytes: 10_000,
                provider_units: 1,
            },
            control: floor(),
            interactive: floor(),
            core: floor(),
            enrichment: floor(),
        }
    }

    fn policy() -> Result<HysteresisPolicy, Box<dyn std::error::Error>> {
        Ok(HysteresisPolicy::new(HysteresisConfig {
            thresholds: PressureThresholds {
                interactive_low_bps: 100,
                interactive_high_bps: 800,
                backlog_low: 1,
                backlog_high: 8,
                lag_low_ms: 10,
                lag_high_ms: 80,
                memory_clear_bytes: 1_000,
                memory_high_pressure_bytes: 100,
            },
            stable_idle_ms: 200,
            clear_dwell_ms: 100,
            minimum_residence_ms: 50,
        })?)
    }

    fn authority() -> EffortAuthority {
        EffortAuthority {
            context: ContextStatus::Current,
            cancellation: CancellationStatus::Active,
            coverage: CoverageExactness::Inexact,
            provider: ProviderPermission::Allowed,
            required_proof: RequiredProofStatus::Satisfied,
            remaining_time_ms: 1_000,
            remaining: ResourceNeed {
                cpu_units: 10,
                io_bytes: 1_000,
                memory_bytes: 1_000,
                provider_units: 1,
            },
            max_automatic_transitions: 1,
        }
    }

    fn optional_offer() -> StageOffer {
        StageOffer {
            id: 7,
            phase: ExecutionPhase::Deep,
            obligation: StageObligation::Optional,
            ready: true,
            requires_exact_coverage: false,
            requires_provider: true,
            estimated_latency_ms: Some(100),
            reservation: ResourceNeed {
                cpu_units: 1,
                io_bytes: 0,
                memory_bytes: 100,
                provider_units: 1,
            },
        }
    }

    #[test]
    fn actual_pool_ingress_controls_remote_prediction() {
        let pool = Arc::new(ProviderAttemptPool::new(1).expect("valid pool"));
        let port = NativeProviderPoolSignals::new(Arc::clone(&pool), ProviderMode::Remote);
        assert_eq!(
            port.sample(7, 10).fresh(7),
            Some(ProviderSignal {
                availability: ProviderAvailability::Ready,
                mode: ProviderMode::Remote,
            })
        );
        let (entered, entered_rx) = mpsc::channel::<()>();
        let (release, parked) = mpsc::channel::<()>();
        pool.spawn_tracked("held", move || {
            let _sent = entered.send(());
            let _done = parked.recv();
        })
        .expect("attempt admitted");
        entered_rx.recv().expect("attempt entered");
        assert_eq!(
            port.sample(8, 10)
                .fresh(8)
                .map(|sample| sample.availability),
            Some(ProviderAvailability::Busy)
        );
        drop(release);
        let _drained = pool.drain(Duration::from_secs(2));
        assert_eq!(
            port.sample(9, 10)
                .fresh(9)
                .map(|sample| sample.availability),
            Some(ProviderAvailability::Offline)
        );
        assert_eq!(port.sample(10, 0), Signal::Unknown(UnknownReason::Missing));
    }

    #[test]
    fn recorded_activity_and_actual_pool_ingress_control_auto_deep()
    -> Result<(), Box<dyn std::error::Error>> {
        let clock = Arc::new(TestClock(Mutex::new(Instant::now())));
        let recorder = Arc::new(NativeInteractionRecorder::with_clock(clock.clone()));
        let publisher = NativeInteractionPublisherId([7; 32]);
        recorder.begin(42, publisher, Duration::from_secs(1))?;

        let pool = Arc::new(ProviderAttemptPool::new(1)?);
        let _cleanup = DrainPoolOnDrop(Arc::clone(&pool));
        let provider: Arc<dyn NativeProviderSignalPort> = Arc::new(NativeProviderPoolSignals::new(
            Arc::clone(&pool),
            ProviderMode::Remote,
        ));
        // Only worker and ledger pressure are fixed fixtures. Foreground idle
        // and provider availability come from their production observers.
        let interaction: Arc<dyn NativeInteractionActivityPort> = recorder.clone();
        let signals = NativeAutoSignals::with_sources(
            Arc::new(EmptyWorker),
            Arc::new(EmptyResources(clear_resource_plan())),
            Some(interaction),
            NativeAutoSignalConfig {
                resource_plan: clear_resource_plan(),
                background_control_enabled: false,
                maintenance_control_enabled: false,
                worker_lag_max_age: Duration::from_millis(200),
                sample_ttl: Duration::from_millis(1_000),
            },
        )?
        .with_provider_signals(provider);
        check_eq!(
            signals.sample(0).interaction_idle_ms,
            Signal::Unknown(UnknownReason::Missing)
        );

        recorder.activity(42, publisher, 1)?;
        let policy = policy()?;
        let governor = EffortGovernor::new(Arc::new(OnePermit::default()));
        let offers = [optional_offer()];
        let initial_state = PolicyState {
            phase: ExecutionPhase::Fast,
            entered_at_ms: 0,
            clear_since_ms: None,
            clear_valid_until_ms: None,
            automatic_transitions: 0,
        };
        let first = DecisionFrame {
            now_ms: 0,
            preference: EffortPreference::Auto,
            signals: signals.sample(0),
            authority: authority(),
            useful_refinement: true,
            offers: &offers,
        };
        let waiting = policy.propose(&first, initial_state, &DeclaredCostEstimator)?;
        check_eq!(waiting.target_phase, ExecutionPhase::Fast);

        clock.advance(Duration::from_millis(300));
        recorder.renew(42, publisher, 2)?;
        let ready = DecisionFrame {
            now_ms: 300,
            signals: signals.sample(300),
            ..first
        };
        check_eq!(ready.signals.interaction_idle_ms.fresh(300), Some(300));
        check_eq!(
            ready
                .signals
                .provider
                .fresh(300)
                .map(|sample| sample.availability),
            Some(ProviderAvailability::Ready)
        );
        let proposal = policy.propose(&ready, waiting.next_state, &DeclaredCostEstimator)?;
        check_eq!(proposal.target_phase, ExecutionPhase::Deep);
        check_eq!(proposal.optional_stage, Some(7));
        let admitted = governor.admit(&ready, waiting.next_state, proposal)?;
        check_eq!(admitted.runnable_stage, Some(7));
        check!(admitted.permit.is_some());
        drop(admitted);

        let (entered, entered_rx) = mpsc::channel::<()>();
        let (release, parked) = mpsc::channel::<()>();
        pool.spawn_tracked("held", move || {
            let _sent = entered.send(());
            let _done = parked.recv();
        })?;
        entered_rx.recv()?;
        clock.advance(Duration::from_millis(1));
        let busy = DecisionFrame {
            now_ms: 301,
            signals: signals.sample(301),
            ..first
        };
        let proposal = policy.propose(&busy, waiting.next_state, &DeclaredCostEstimator)?;
        let denied = governor.admit(&busy, waiting.next_state, proposal)?;
        check_eq!(denied.reason, DecisionReason::ProviderUnavailable);
        check!(denied.permit.is_none());

        drop(release);
        let drained = pool.drain(Duration::from_secs(2));
        check!(drained.unfinished.is_empty());
        check!(drained.panicked.is_empty());
        clock.advance(Duration::from_millis(1));
        let offline = DecisionFrame {
            now_ms: 302,
            signals: signals.sample(302),
            ..first
        };
        let proposal = policy.propose(&offline, waiting.next_state, &DeclaredCostEstimator)?;
        let denied = governor.admit(&offline, waiting.next_state, proposal)?;
        check_eq!(denied.reason, DecisionReason::ProviderUnavailable);
        check!(denied.permit.is_none());

        recorder.end(42, publisher, 3)?;
        let ended = DecisionFrame {
            now_ms: 303,
            signals: signals.sample(303),
            ..first
        };
        check_eq!(
            ended.signals.interaction_idle_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        let proposal = policy.propose(&ended, waiting.next_state, &DeclaredCostEstimator)?;
        check_eq!(proposal.target_phase, ExecutionPhase::Fast);
        check_eq!(proposal.optional_stage, None);
        Ok(())
    }
}
