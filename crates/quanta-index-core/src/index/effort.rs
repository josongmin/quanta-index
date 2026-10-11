//! Bounded query-effort decisions. Policy proposes; the governor and the shared
//! scheduler retain admission authority. This module does not evaluate results.

use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffortPreference {
    Fast,
    Auto,
    Deep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionPhase {
    Fast,
    Deep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnknownReason {
    Missing,
    Dropped,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalSample<T> {
    pub value: T,
    pub observed_at_ms: u64,
    pub valid_for_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal<T> {
    Known(SignalSample<T>),
    Unknown(UnknownReason),
}

impl<T: Copy> Signal<T> {
    /// A future timestamp, expired sample, or zero TTL has no predictive value.
    #[must_use]
    pub fn fresh(self, now_ms: u64) -> Option<T> {
        match self {
            Self::Known(sample)
                if sample.valid_for_ms > 0
                    && now_ms
                        .checked_sub(sample.observed_at_ms)
                        .is_some_and(|age| age < sample.valid_for_ms) =>
            {
                Some(sample.value)
            }
            Self::Known(_) | Self::Unknown(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderAvailability {
    Ready,
    Busy,
    Offline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderMode {
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderSignal {
    pub availability: ProviderAvailability,
    pub mode: ProviderMode,
}

/// Prediction inputs only. None of these fields proves coverage, permission,
/// completion, or available capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffortSignals {
    pub interaction_idle_ms: Signal<u64>,
    pub interactive_pressure_bps: Signal<u16>,
    pub index_backlog: Signal<u32>,
    pub dispatch_lag_ms: Signal<u64>,
    pub memory_headroom_bytes: Signal<u64>,
    pub provider: Signal<ProviderSignal>,
    pub quality_confidence_bps: Signal<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PressureThresholds {
    pub interactive_low_bps: u16,
    pub interactive_high_bps: u16,
    pub backlog_low: u32,
    pub backlog_high: u32,
    pub lag_low_ms: u64,
    pub lag_high_ms: u64,
    pub memory_clear_bytes: u64,
    pub memory_high_pressure_bytes: u64,
}

impl PressureThresholds {
    pub fn validate(self) -> Result<(), EffortError> {
        if self.interactive_low_bps >= self.interactive_high_bps
            || self.interactive_high_bps > 10_000
            || self.backlog_low >= self.backlog_high
            || self.lag_low_ms >= self.lag_high_ms
            || self.memory_high_pressure_bytes >= self.memory_clear_bytes
        {
            return Err(EffortError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HysteresisConfig {
    pub thresholds: PressureThresholds,
    pub stable_idle_ms: u64,
    pub clear_dwell_ms: u64,
    pub minimum_residence_ms: u64,
}

impl HysteresisConfig {
    pub fn validate(self) -> Result<(), EffortError> {
        self.thresholds.validate()?;
        if self.stable_idle_ms == 0 || self.clear_dwell_ms == 0 {
            return Err(EffortError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PolicyState {
    pub phase: ExecutionPhase,
    pub entered_at_ms: u64,
    pub clear_since_ms: Option<u64>,
    pub automatic_transitions: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageObligation {
    Required,
    Optional,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceNeed {
    pub latency_ms: u64,
    pub cpu_units: u64,
    pub io_bytes: u64,
    pub memory_bytes: u64,
    pub provider_units: u64,
}

impl ResourceNeed {
    #[must_use]
    pub fn fits(self, available: Self) -> bool {
        self.latency_ms <= available.latency_ms
            && self.cpu_units <= available.cpu_units
            && self.io_bytes <= available.io_bytes
            && self.memory_bytes <= available.memory_bytes
            && self.provider_units <= available.provider_units
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StageOffer {
    pub id: u64,
    pub phase: ExecutionPhase,
    pub obligation: StageObligation,
    /// The planner binds prerequisites to the exact query/ticket before offering.
    pub ready: bool,
    pub requires_exact_coverage: bool,
    pub requires_provider: bool,
    /// Hard upper bound for reservation, independent of estimated cost.
    pub reservation: ResourceNeed,
}

/// Whether the authority's query/ticket context still matches the request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextStatus {
    Current,
    Stale,
}

/// Cancellation state issued by the request owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationStatus {
    Active,
    Cancelled,
}

/// Exact coverage is a prerequisite for offers that require it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageExactness {
    Exact,
    Inexact,
}

/// Permission to use the provider, independent of provider telemetry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderPermission {
    Allowed,
    Denied,
}

/// State of the required result proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequiredProofStatus {
    Pending,
    Satisfied,
}

/// Owner-issued facts; telemetry cannot modify this record. The executor still
/// rechecks its exact ticket, permissions, and result proof at dispatch/completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffortAuthority {
    pub context: ContextStatus,
    pub cancellation: CancellationStatus,
    pub coverage: CoverageExactness,
    pub provider: ProviderPermission,
    pub required_proof: RequiredProofStatus,
    pub remaining: ResourceNeed,
    pub max_automatic_transitions: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct DecisionFrame<'a> {
    pub now_ms: u64,
    pub preference: EffortPreference,
    pub signals: EffortSignals,
    pub authority: EffortAuthority,
    pub useful_refinement: bool,
    pub offers: &'a [StageOffer],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionReason {
    ExplicitFast,
    ExplicitDeep,
    RequiredProgress,
    StableRefinement,
    Pressure,
    UnknownTelemetry,
    Dwell,
    NoOpportunity,
    ResourceDenied,
    TransitionCapped,
    ProviderUnavailable,
    AwaitRequired,
    AwaitOptional,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PolicyProposal {
    pub target_phase: ExecutionPhase,
    pub optional_stage: Option<u64>,
    pub reason: DecisionReason,
    pub next_state: PolicyState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EffortError {
    #[error("invalid effort configuration")]
    InvalidConfiguration,
    #[error("invalid effort frame")]
    InvalidFrame,
    #[error("invalid effort proposal")]
    InvalidProposal,
    #[error("stale effort context")]
    StaleContext,
    #[error("effort cancelled")]
    Cancelled,
    #[error("effort deadline exceeded")]
    Deadline,
}

/// An estimator ranks work only. Unknown estimates stay unknown; reservation
/// uses `StageOffer::reservation` and the scheduler's live capacity instead.
pub trait StageCostEstimator: Send + Sync {
    fn estimated_latency_ms(&self, offer: &StageOffer) -> Option<u64>;
}

/// Deterministic cold-start ordering from the planner's declared reservation
/// envelope. It is not an observed latency or a completeness proof.
pub struct EnvelopeCostEstimator;

impl StageCostEstimator for EnvelopeCostEstimator {
    fn estimated_latency_ms(&self, offer: &StageOffer) -> Option<u64> {
        Some(offer.reservation.latency_ms)
    }
}

pub trait EffortPolicy: Send + Sync {
    fn propose(
        &self,
        frame: &DecisionFrame<'_>,
        state: PolicyState,
        estimator: &dyn StageCostEstimator,
    ) -> Result<PolicyProposal, EffortError>;
}

pub struct HysteresisPolicy {
    config: HysteresisConfig,
}

impl HysteresisPolicy {
    pub fn new(config: HysteresisConfig) -> Result<Self, EffortError> {
        config.validate()?;
        Ok(Self { config })
    }

    fn pressure(&self, frame: &DecisionFrame<'_>) -> PressureState {
        let signals = frame.signals;
        let now = frame.now_ms;
        let values = (
            signals.interactive_pressure_bps.fresh(now),
            signals.index_backlog.fresh(now),
            signals.dispatch_lag_ms.fresh(now),
            signals.memory_headroom_bytes.fresh(now),
        );
        let limits = self.config.thresholds;
        if values
            .0
            .is_some_and(|value| value >= limits.interactive_high_bps)
            || values.1.is_some_and(|value| value >= limits.backlog_high)
            || values.2.is_some_and(|value| value >= limits.lag_high_ms)
            || values
                .3
                .is_some_and(|value| value <= limits.memory_high_pressure_bytes)
        {
            return PressureState::High;
        }
        match values {
            (Some(interactive), Some(backlog), Some(lag), Some(memory))
                if interactive <= limits.interactive_low_bps
                    && backlog <= limits.backlog_low
                    && lag <= limits.lag_low_ms
                    && memory >= limits.memory_clear_bytes =>
            {
                PressureState::Clear
            }
            (None, _, _, _) | (_, None, _, _) | (_, _, None, _) | (_, _, _, None) => {
                PressureState::Unknown
            }
            _ => PressureState::Middle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PressureState {
    High,
    Middle,
    Clear,
    Unknown,
}

impl EffortPolicy for HysteresisPolicy {
    fn propose(
        &self,
        frame: &DecisionFrame<'_>,
        state: PolicyState,
        estimator: &dyn StageCostEstimator,
    ) -> Result<PolicyProposal, EffortError> {
        validate_frame(frame, state)?;
        let pressure = self.pressure(frame);
        let mut next = state;
        next.clear_since_ms = if pressure == PressureState::Clear {
            state.clear_since_ms.or(Some(frame.now_ms))
        } else {
            None
        };
        let (mut target, mut reason) = match frame.preference {
            EffortPreference::Fast => (ExecutionPhase::Fast, DecisionReason::ExplicitFast),
            EffortPreference::Deep => (ExecutionPhase::Deep, DecisionReason::ExplicitDeep),
            EffortPreference::Auto if pressure == PressureState::High => {
                (ExecutionPhase::Fast, DecisionReason::Pressure)
            }
            EffortPreference::Auto if pressure == PressureState::Unknown => {
                (state.phase, DecisionReason::UnknownTelemetry)
            }
            EffortPreference::Auto if !frame.useful_refinement => {
                (state.phase, DecisionReason::NoOpportunity)
            }
            EffortPreference::Auto if state.phase == ExecutionPhase::Deep => {
                (ExecutionPhase::Deep, DecisionReason::StableRefinement)
            }
            EffortPreference::Auto => {
                let stable = frame
                    .signals
                    .interaction_idle_ms
                    .fresh(frame.now_ms)
                    .is_some_and(|idle| idle >= self.config.stable_idle_ms);
                let clear = pressure == PressureState::Clear
                    && next.clear_since_ms.is_some_and(|since| {
                        frame.now_ms.saturating_sub(since) >= self.config.clear_dwell_ms
                    });
                let residence = frame.now_ms.saturating_sub(state.entered_at_ms)
                    >= self.config.minimum_residence_ms;
                if stable && clear && residence {
                    (ExecutionPhase::Deep, DecisionReason::StableRefinement)
                } else {
                    (ExecutionPhase::Fast, DecisionReason::Dwell)
                }
            }
        };
        if frame.preference == EffortPreference::Auto
            && state.phase == ExecutionPhase::Fast
            && target == ExecutionPhase::Deep
            && state.automatic_transitions >= frame.authority.max_automatic_transitions
        {
            target = ExecutionPhase::Fast;
            reason = DecisionReason::TransitionCapped;
        }
        if target != state.phase {
            next.phase = target;
            next.entered_at_ms = frame.now_ms;
            if frame.preference == EffortPreference::Auto && target == ExecutionPhase::Deep {
                next.automatic_transitions = state
                    .automatic_transitions
                    .checked_add(1)
                    .ok_or(EffortError::InvalidFrame)?;
            }
        }
        let optional_stage = if target == ExecutionPhase::Deep
            && (frame.preference == EffortPreference::Deep || pressure == PressureState::Clear)
        {
            let mut best_key = None;
            let mut best_stage = None;
            for offer in frame.offers.iter().filter(|offer| {
                offer.ready
                    && offer.phase == ExecutionPhase::Deep
                    && offer.obligation == StageObligation::Optional
            }) {
                let cost = estimator.estimated_latency_ms(offer);
                let order = (cost.is_none(), cost.unwrap_or(u64::MAX), offer.id);
                if best_key.is_none_or(|prior| order < prior) {
                    best_key = Some(order);
                    best_stage = Some(offer.id);
                }
            }
            best_stage
        } else {
            None
        };
        Ok(PolicyProposal {
            target_phase: target,
            optional_stage,
            reason,
            next_state: next,
        })
    }
}

fn validate_frame(frame: &DecisionFrame<'_>, state: PolicyState) -> Result<(), EffortError> {
    if frame.now_ms < state.entered_at_ms || frame.offers.len() > 64 {
        return Err(EffortError::InvalidFrame);
    }
    let mut ids = BTreeSet::new();
    for offer in frame.offers {
        if !ids.insert(offer.id) || offer.reservation.latency_ms == 0 {
            return Err(EffortError::InvalidFrame);
        }
    }
    Ok(())
}

/// Implemented by the existing shared scheduler, not by an effort policy.
///
/// Reservation tokens remain charged until their guard is dropped after work
/// quiesces; actual work still uses the intent's cumulative budget.
pub trait ResourceScheduler: Send + Sync + 'static {
    type Token: Send;
    fn try_reserve(&self, need: ResourceNeed) -> Option<Self::Token>;
    fn release(&self, token: Self::Token);
}

pub struct ReservationGuard<S: ResourceScheduler> {
    scheduler: Arc<S>,
    token: Option<S::Token>,
}

impl<S: ResourceScheduler> Drop for ReservationGuard<S> {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            self.scheduler.release(token);
        }
    }
}

pub struct AdmittedEffort<S: ResourceScheduler> {
    pub requested_phase: ExecutionPhase,
    pub runnable_phase: ExecutionPhase,
    pub requested_stage: Option<u64>,
    pub runnable_stage: Option<u64>,
    pub reason: DecisionReason,
    /// Commit this state only with the application receipt; denied proposals
    /// retain the previous phase and transition count.
    pub next_state: PolicyState,
    pub permit: Option<ReservationGuard<S>>,
}

pub struct EffortGovernor<S: ResourceScheduler> {
    scheduler: Arc<S>,
}

impl<S: ResourceScheduler> EffortGovernor<S> {
    #[must_use]
    pub fn new(scheduler: Arc<S>) -> Self {
        Self { scheduler }
    }

    pub fn admit(
        &self,
        frame: &DecisionFrame<'_>,
        state: PolicyState,
        proposal: PolicyProposal,
    ) -> Result<AdmittedEffort<S>, EffortError> {
        validate_frame(frame, state)?;
        if frame.authority.cancellation == CancellationStatus::Cancelled {
            return Err(EffortError::Cancelled);
        }
        if frame.authority.context == ContextStatus::Stale {
            return Err(EffortError::StaleContext);
        }
        if frame.authority.remaining.latency_ms == 0 {
            return Err(EffortError::Deadline);
        }
        if frame.preference == EffortPreference::Fast
            && proposal.target_phase == ExecutionPhase::Deep
        {
            return Err(EffortError::InvalidProposal);
        }
        let expected_transitions = if frame.preference == EffortPreference::Auto
            && state.phase == ExecutionPhase::Fast
            && proposal.target_phase == ExecutionPhase::Deep
        {
            state
                .automatic_transitions
                .checked_add(1)
                .ok_or(EffortError::InvalidProposal)?
        } else {
            state.automatic_transitions
        };
        if proposal.next_state.phase != proposal.target_phase
            || proposal.next_state.automatic_transitions != expected_transitions
            || (proposal.target_phase != state.phase
                && proposal.next_state.entered_at_ms != frame.now_ms)
            || (proposal.target_phase == state.phase
                && proposal.next_state.entered_at_ms != state.entered_at_ms)
        {
            return Err(EffortError::InvalidProposal);
        }
        let cap_reached = frame.preference == EffortPreference::Auto
            && proposal.target_phase == ExecutionPhase::Deep
            && state.phase == ExecutionPhase::Fast
            && proposal.next_state.automatic_transitions
                > frame.authority.max_automatic_transitions;
        let optional = proposal
            .optional_stage
            .map(|id| frame.offers.iter().find(|offer| offer.id == id));
        if optional.is_some_and(|found| found.is_none())
            || optional.flatten().is_some_and(|offer| {
                offer.obligation != StageObligation::Optional
                    || offer.phase != ExecutionPhase::Deep
                    || !offer.ready
            })
            || (proposal.optional_stage.is_some() && proposal.target_phase != ExecutionPhase::Deep)
        {
            return Err(EffortError::InvalidProposal);
        }
        let required_pending = frame.authority.required_proof == RequiredProofStatus::Pending
            || frame
                .offers
                .iter()
                .any(|offer| offer.ready && offer.obligation == StageObligation::Required);
        let required = frame
            .offers
            .iter()
            .filter(|offer| offer.ready && offer.obligation == StageObligation::Required)
            .filter(|offer| {
                frame.preference != EffortPreference::Fast || offer.phase == ExecutionPhase::Fast
            })
            .filter(|offer| {
                !offer.requires_exact_coverage
                    || frame.authority.coverage == CoverageExactness::Exact
            })
            .filter(|offer| {
                !offer.requires_provider || frame.authority.provider == ProviderPermission::Allowed
            })
            .min_by_key(|offer| offer.id);
        let chosen_offer = if cap_reached || required_pending {
            required
        } else {
            required.or_else(|| optional.flatten())
        };
        let Some(offer) = chosen_offer else {
            return Ok(AdmittedEffort {
                requested_phase: proposal.target_phase,
                runnable_phase: if proposal.target_phase == ExecutionPhase::Fast {
                    ExecutionPhase::Fast
                } else {
                    state.phase
                },
                requested_stage: proposal.optional_stage,
                runnable_stage: None,
                reason: if required_pending {
                    DecisionReason::AwaitRequired
                } else if cap_reached {
                    DecisionReason::TransitionCapped
                } else {
                    DecisionReason::AwaitOptional
                },
                next_state: if proposal.target_phase == ExecutionPhase::Fast {
                    proposal.next_state
                } else {
                    state
                },
                permit: None,
            });
        };
        if offer.requires_exact_coverage && frame.authority.coverage != CoverageExactness::Exact {
            return Ok(self.denied(
                frame,
                state,
                proposal,
                offer.id,
                DecisionReason::AwaitRequired,
            ));
        }
        if offer.requires_provider
            && (frame.authority.provider != ProviderPermission::Allowed
                || (offer.obligation == StageObligation::Optional
                    && frame.preference == EffortPreference::Auto
                    && !frame
                        .signals
                        .provider
                        .fresh(frame.now_ms)
                        .is_some_and(|signal| signal.availability == ProviderAvailability::Ready)))
        {
            return Ok(self.denied(
                frame,
                state,
                proposal,
                offer.id,
                DecisionReason::ProviderUnavailable,
            ));
        }
        if !offer.reservation.fits(frame.authority.remaining) {
            return Ok(self.denied(
                frame,
                state,
                proposal,
                offer.id,
                DecisionReason::ResourceDenied,
            ));
        }
        let Some(token) = self.scheduler.try_reserve(offer.reservation) else {
            return Ok(self.denied(
                frame,
                state,
                proposal,
                offer.id,
                DecisionReason::ResourceDenied,
            ));
        };
        Ok(AdmittedEffort {
            requested_phase: proposal.target_phase,
            runnable_phase: offer.phase,
            requested_stage: Some(offer.id),
            runnable_stage: Some(offer.id),
            reason: if offer.obligation == StageObligation::Required {
                DecisionReason::RequiredProgress
            } else {
                proposal.reason
            },
            next_state: if cap_reached {
                state
            } else {
                proposal.next_state
            },
            permit: Some(ReservationGuard {
                scheduler: Arc::clone(&self.scheduler),
                token: Some(token),
            }),
        })
    }

    fn denied(
        &self,
        _frame: &DecisionFrame<'_>,
        state: PolicyState,
        proposal: PolicyProposal,
        offer_id: u64,
        reason: DecisionReason,
    ) -> AdmittedEffort<S> {
        AdmittedEffort {
            requested_phase: proposal.target_phase,
            runnable_phase: if proposal.target_phase == ExecutionPhase::Fast {
                ExecutionPhase::Fast
            } else {
                state.phase
            },
            requested_stage: Some(offer_id),
            runnable_stage: None,
            reason,
            next_state: if proposal.target_phase == ExecutionPhase::Fast {
                proposal.next_state
            } else {
                state
            },
            permit: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct SingleSlot(Mutex<bool>);
    impl ResourceScheduler for SingleSlot {
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
            if let Ok(mut held) = self.0.lock() {
                *held = false;
            }
        }
    }

    fn known<T>(value: T, now_ms: u64) -> Signal<T> {
        Signal::Known(SignalSample {
            value,
            observed_at_ms: now_ms,
            valid_for_ms: 1_000,
        })
    }

    fn clear_signals(now_ms: u64) -> EffortSignals {
        EffortSignals {
            interaction_idle_ms: known(300, now_ms),
            interactive_pressure_bps: known(10, now_ms),
            index_backlog: known(0, now_ms),
            dispatch_lag_ms: known(0, now_ms),
            memory_headroom_bytes: known(10_000, now_ms),
            provider: known(
                ProviderSignal {
                    availability: ProviderAvailability::Ready,
                    mode: ProviderMode::Local,
                },
                now_ms,
            ),
            quality_confidence_bps: Signal::Unknown(UnknownReason::Unsupported),
        }
    }

    fn config() -> HysteresisConfig {
        HysteresisConfig {
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
        }
    }

    fn state() -> PolicyState {
        PolicyState {
            phase: ExecutionPhase::Fast,
            entered_at_ms: 0,
            clear_since_ms: None,
            automatic_transitions: 0,
        }
    }

    fn authority() -> EffortAuthority {
        EffortAuthority {
            context: ContextStatus::Current,
            cancellation: CancellationStatus::Active,
            coverage: CoverageExactness::Inexact,
            provider: ProviderPermission::Allowed,
            required_proof: RequiredProofStatus::Satisfied,
            remaining: ResourceNeed {
                latency_ms: 1_000,
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
            reservation: ResourceNeed {
                latency_ms: 100,
                cpu_units: 1,
                io_bytes: 0,
                memory_bytes: 100,
                provider_units: 1,
            },
        }
    }

    #[test]
    fn stale_or_missing_pressure_never_becomes_zero() -> Result<(), EffortError> {
        let policy = HysteresisPolicy::new(config())?;
        let mut signals = clear_signals(100);
        signals.index_backlog = Signal::Unknown(UnknownReason::Dropped);
        let offers = [optional_offer()];
        let frame = DecisionFrame {
            now_ms: 100,
            preference: EffortPreference::Auto,
            signals,
            authority: authority(),
            useful_refinement: true,
            offers: &offers,
        };
        let proposal = policy.propose(&frame, state(), &EnvelopeCostEstimator)?;
        assert_eq!(proposal.target_phase, ExecutionPhase::Fast);
        assert_eq!(proposal.reason, DecisionReason::UnknownTelemetry);
        assert_eq!(proposal.optional_stage, None);
        assert_eq!(known(0_u32, 200).fresh(100), None);
        assert_eq!(known(0_u32, 0).fresh(1_000), None);
        Ok(())
    }

    #[test]
    fn clear_dwell_and_pressure_demote_without_oscillation() -> Result<(), EffortError> {
        let policy = HysteresisPolicy::new(config())?;
        let offers = [optional_offer()];
        let mut first = DecisionFrame {
            now_ms: 200,
            preference: EffortPreference::Auto,
            signals: clear_signals(200),
            authority: authority(),
            useful_refinement: true,
            offers: &offers,
        };
        let waiting = policy.propose(&first, state(), &EnvelopeCostEstimator)?;
        assert_eq!(waiting.target_phase, ExecutionPhase::Fast);
        first.now_ms = 301;
        first.signals = clear_signals(301);
        let promoted = policy.propose(&first, waiting.next_state, &EnvelopeCostEstimator)?;
        assert_eq!(promoted.target_phase, ExecutionPhase::Deep);
        assert_eq!(promoted.optional_stage, Some(7));
        let mut high = first;
        high.now_ms = 302;
        high.signals = clear_signals(302);
        high.signals.interactive_pressure_bps = known(900, 302);
        let demoted = policy.propose(&high, promoted.next_state, &EnvelopeCostEstimator)?;
        assert_eq!(demoted.target_phase, ExecutionPhase::Fast);
        assert_eq!(demoted.next_state.automatic_transitions, 1);
        assert_eq!(demoted.optional_stage, None);
        Ok(())
    }

    #[test]
    fn governor_keeps_required_progress_and_reservations_independent() -> Result<(), EffortError> {
        let scheduler = Arc::new(SingleSlot::default());
        let governor = EffortGovernor::new(Arc::clone(&scheduler));
        let required = StageOffer {
            id: 2,
            obligation: StageObligation::Required,
            requires_provider: false,
            ..optional_offer()
        };
        let offers = [required];
        let mut frame = DecisionFrame {
            now_ms: 301,
            preference: EffortPreference::Auto,
            signals: clear_signals(301),
            authority: authority(),
            useful_refinement: true,
            offers: &offers,
        };
        frame.authority.max_automatic_transitions = 0;
        let proposal = PolicyProposal {
            target_phase: ExecutionPhase::Deep,
            optional_stage: None,
            reason: DecisionReason::StableRefinement,
            next_state: PolicyState {
                phase: ExecutionPhase::Deep,
                entered_at_ms: 301,
                automatic_transitions: 1,
                ..state()
            },
        };
        let admitted = governor.admit(&frame, state(), proposal)?;
        assert_eq!(admitted.reason, DecisionReason::RequiredProgress);
        assert_eq!(admitted.runnable_stage, Some(2));
        assert_eq!(admitted.next_state.phase, ExecutionPhase::Fast);
        assert!(scheduler.0.lock().is_ok_and(|held| *held));
        drop(admitted);
        assert!(scheduler.0.lock().is_ok_and(|held| !*held));
        Ok(())
    }

    #[test]
    fn governor_rejects_foreign_stage_and_provider_metadata_cannot_grant_access()
    -> Result<(), EffortError> {
        let scheduler = Arc::new(SingleSlot::default());
        let governor = EffortGovernor::new(scheduler);
        let offers = [optional_offer()];
        let mut frame = DecisionFrame {
            now_ms: 301,
            preference: EffortPreference::Deep,
            signals: clear_signals(301),
            authority: authority(),
            useful_refinement: true,
            offers: &offers,
        };
        let proposal = PolicyProposal {
            target_phase: ExecutionPhase::Deep,
            optional_stage: Some(99),
            reason: DecisionReason::ExplicitDeep,
            next_state: PolicyState {
                phase: ExecutionPhase::Deep,
                entered_at_ms: 301,
                ..state()
            },
        };
        assert!(matches!(
            governor.admit(&frame, state(), proposal),
            Err(EffortError::InvalidProposal)
        ));
        frame.authority.provider = ProviderPermission::Denied;
        let denied = governor.admit(
            &frame,
            state(),
            PolicyProposal {
                optional_stage: Some(7),
                ..proposal
            },
        )?;
        assert_eq!(denied.runnable_stage, None);
        assert_eq!(denied.reason, DecisionReason::ProviderUnavailable);
        assert!(denied.permit.is_none());
        frame.authority.provider = ProviderPermission::Allowed;
        frame.authority.remaining.latency_ms = 50;
        let budget_denied = governor.admit(
            &frame,
            state(),
            PolicyProposal {
                optional_stage: Some(7),
                ..proposal
            },
        )?;
        assert_eq!(budget_denied.reason, DecisionReason::ResourceDenied);
        Ok(())
    }

    #[test]
    fn policy_cannot_forge_transition_counter_or_phase() {
        let governor = EffortGovernor::new(Arc::new(SingleSlot::default()));
        let offers = [optional_offer()];
        let frame = DecisionFrame {
            now_ms: 301,
            preference: EffortPreference::Auto,
            signals: clear_signals(301),
            authority: authority(),
            useful_refinement: true,
            offers: &offers,
        };
        let skipped_count = PolicyProposal {
            target_phase: ExecutionPhase::Deep,
            optional_stage: Some(7),
            reason: DecisionReason::StableRefinement,
            next_state: PolicyState {
                phase: ExecutionPhase::Deep,
                entered_at_ms: 301,
                ..state()
            },
        };
        assert!(matches!(
            governor.admit(&frame, state(), skipped_count),
            Err(EffortError::InvalidProposal)
        ));
        let forged_phase = PolicyProposal {
            next_state: PolicyState {
                phase: ExecutionPhase::Fast,
                automatic_transitions: 1,
                ..skipped_count.next_state
            },
            ..skipped_count
        };
        assert!(matches!(
            governor.admit(&frame, state(), forged_phase),
            Err(EffortError::InvalidProposal)
        ));
        let valid = PolicyProposal {
            next_state: PolicyState {
                automatic_transitions: 1,
                ..skipped_count.next_state
            },
            ..skipped_count
        };
        let admitted = governor.admit(&frame, state(), valid);
        assert!(admitted.is_ok());
        let mut capped = frame;
        capped.authority.max_automatic_transitions = 0;
        let capped_decision = governor.admit(&capped, state(), valid);
        assert!(capped_decision.is_ok_and(|decision| {
            decision.reason == DecisionReason::TransitionCapped
                && decision.runnable_stage.is_none()
                && decision.next_state == state()
        }));
    }
}
