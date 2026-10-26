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
    /// Exclusive end of the last fully observed clear-pressure window.
    /// A new clear sample at this exact instant continues without a gap.
    pub clear_valid_until_ms: Option<u64>,
    pub automatic_transitions: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageObligation {
    Required,
    Optional,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceNeed {
    pub cpu_units: u64,
    pub io_bytes: u64,
    pub memory_bytes: u64,
    pub provider_units: u64,
}

impl ResourceNeed {
    #[must_use]
    pub fn fits(self, available: Self) -> bool {
        self.cpu_units <= available.cpu_units
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
    /// Advisory planner estimate for stage ordering; None means unknown.
    pub estimated_latency_ms: Option<u64>,
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
    /// Owner-issued time remaining until the request deadline.
    pub remaining_time_ms: u64,
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

/// Deterministic ordering from the planner's declared cost. It is not an
/// observed latency or a completeness proof.
pub struct DeclaredCostEstimator;

impl StageCostEstimator for DeclaredCostEstimator {
    fn estimated_latency_ms(&self, offer: &StageOffer) -> Option<u64> {
        offer.estimated_latency_ms
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

    fn clear_valid_until(frame: &DecisionFrame<'_>) -> Option<u64> {
        [
            fresh_until(frame.signals.interactive_pressure_bps, frame.now_ms),
            fresh_until(frame.signals.index_backlog, frame.now_ms),
            fresh_until(frame.signals.dispatch_lag_ms, frame.now_ms),
            fresh_until(frame.signals.memory_headroom_bytes, frame.now_ms),
        ]
        .into_iter()
        .try_fold(u64::MAX, |earliest, expiry| {
            expiry.map(|until| earliest.min(until))
        })
    }
}

fn fresh_until<T: Copy>(signal: Signal<T>, now_ms: u64) -> Option<u64> {
    match signal {
        Signal::Known(sample) if signal.fresh(now_ms).is_some() => {
            sample.observed_at_ms.checked_add(sample.valid_for_ms)
        }
        Signal::Known(_) | Signal::Unknown(_) => None,
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
        let clear_window = (pressure == PressureState::Clear)
            .then(|| Self::clear_valid_until(frame))
            .flatten();
        if let Some(valid_until) = clear_window {
            // `valid_until` is exclusive for the prior sample. A replacement
            // observed exactly there leaves no unobserved millisecond.
            next.clear_since_ms = Some(
                state
                    .clear_since_ms
                    .filter(|since| *since <= frame.now_ms)
                    .filter(|_| {
                        state
                            .clear_valid_until_ms
                            .is_some_and(|prior| prior >= frame.now_ms)
                    })
                    .unwrap_or(frame.now_ms),
            );
            next.clear_valid_until_ms = Some(valid_until);
        } else {
            next.clear_since_ms = None;
            next.clear_valid_until_ms = None;
        }
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
        if !ids.insert(offer.id)
            || offer.estimated_latency_ms == Some(0)
            || offer.reservation == ResourceNeed::default()
        {
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
    /// Allocation custody is an optional capability for explicit standalone
    /// schedulers. The native service provides it through its existing ledger.
    fn memory_scope(&self, _token: &Self::Token) -> Option<super::MemoryScope> {
        None
    }
}

pub struct ReservationGuard<S: ResourceScheduler> {
    scheduler: Arc<S>,
    need: ResourceNeed,
    token: Option<S::Token>,
}

impl<S: ResourceScheduler> ReservationGuard<S> {
    /// Reserve execution capacity and keep it charged until work quiesces.
    /// A refusal never transfers ownership of a scheduler token.
    pub fn try_acquire(scheduler: Arc<S>, need: ResourceNeed) -> Option<Self> {
        let token = scheduler.try_reserve(need)?;
        Some(Self {
            scheduler,
            need,
            token: Some(token),
        })
    }
    /// The exact logical need charged when this non-cloneable guard was admitted.
    #[must_use]
    pub const fn need(&self) -> ResourceNeed {
        self.need
    }

    #[must_use]
    pub fn memory_scope(&self) -> Option<super::MemoryScope> {
        self.token
            .as_ref()
            .and_then(|token| self.scheduler.memory_scope(token))
    }
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
        if frame.authority.remaining_time_ms == 0 {
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
        let Some(permit) =
            ReservationGuard::try_acquire(Arc::clone(&self.scheduler), offer.reservation)
        else {
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
            permit: Some(permit),
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    // Preserve single evaluation and borrowed operands while reporting test failures as errors.
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

    #[test]
    fn execution_guard_retains_capacity_through_move_and_unwind() {
        struct CountingSlot {
            occupied: AtomicBool,
            releases: AtomicUsize,
        }
        impl ResourceScheduler for CountingSlot {
            type Token = ();

            fn try_reserve(&self, _need: ResourceNeed) -> Option<Self::Token> {
                match self.occupied.compare_exchange(
                    false,
                    true,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_previous) => Some(()),
                    Err(_occupied) => None,
                }
            }

            fn release(&self, _token: Self::Token) {
                assert!(self.occupied.swap(false, Ordering::AcqRel));
                let _prior = self.releases.fetch_add(1, Ordering::AcqRel);
            }
        }
        let scheduler = Arc::new(CountingSlot {
            occupied: AtomicBool::new(false),
            releases: AtomicUsize::new(0),
        });
        let Some(guard) =
            ReservationGuard::try_acquire(Arc::clone(&scheduler), ResourceNeed::default())
        else {
            panic!("empty slot must admit execution");
        };
        let moved_guard = guard;
        assert!(
            ReservationGuard::try_acquire(Arc::clone(&scheduler), ResourceNeed::default())
                .is_none()
        );
        assert_eq!(scheduler.releases.load(Ordering::Acquire), 0);
        drop(moved_guard);
        assert_eq!(scheduler.releases.load(Ordering::Acquire), 1);
        let unwound = std::panic::catch_unwind(|| {
            let Some(_held) =
                ReservationGuard::try_acquire(Arc::clone(&scheduler), ResourceNeed::default())
            else {
                panic!("released slot must admit execution");
            };
            panic!("simulate execution unwind");
        });
        assert!(unwound.is_err());
        assert!(!scheduler.occupied.load(Ordering::Acquire));
        assert_eq!(scheduler.releases.load(Ordering::Acquire), 2);
    }

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

    fn short_clear_signals(now_ms: u64) -> EffortSignals {
        let mut signals = clear_signals(now_ms);
        signals.interactive_pressure_bps = Signal::Known(SignalSample {
            value: 10,
            observed_at_ms: now_ms,
            valid_for_ms: 20,
        });
        signals.index_backlog = Signal::Known(SignalSample {
            value: 0,
            observed_at_ms: now_ms,
            valid_for_ms: 20,
        });
        signals.dispatch_lag_ms = Signal::Known(SignalSample {
            value: 0,
            observed_at_ms: now_ms,
            valid_for_ms: 20,
        });
        signals.memory_headroom_bytes = Signal::Known(SignalSample {
            value: 10_000,
            observed_at_ms: now_ms,
            valid_for_ms: 20,
        });
        signals
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
            clear_valid_until_ms: None,
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
    fn stale_or_missing_pressure_never_becomes_zero() -> Result<(), Box<dyn std::error::Error>> {
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
        let proposal = policy.propose(&frame, state(), &DeclaredCostEstimator)?;
        check_eq!(proposal.target_phase, ExecutionPhase::Fast);
        check_eq!(proposal.reason, DecisionReason::UnknownTelemetry);
        check_eq!(proposal.optional_stage, None);
        check_eq!(known(0_u32, 200).fresh(100), None);
        check_eq!(known(0_u32, 0).fresh(1_000), None);
        Ok(())
    }

    #[test]
    fn clear_dwell_and_pressure_demote_without_oscillation()
    -> Result<(), Box<dyn std::error::Error>> {
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
        let waiting = policy.propose(&first, state(), &DeclaredCostEstimator)?;
        check_eq!(waiting.target_phase, ExecutionPhase::Fast);
        first.now_ms = 301;
        first.signals = clear_signals(301);
        let promoted = policy.propose(&first, waiting.next_state, &DeclaredCostEstimator)?;
        check_eq!(promoted.target_phase, ExecutionPhase::Deep);
        check_eq!(promoted.optional_stage, Some(7));
        let mut high = first;
        high.now_ms = 302;
        high.signals = clear_signals(302);
        high.signals.interactive_pressure_bps = known(900, 302);
        let demoted = policy.propose(&high, promoted.next_state, &DeclaredCostEstimator)?;
        check_eq!(demoted.target_phase, ExecutionPhase::Fast);
        check_eq!(demoted.next_state.automatic_transitions, 1);
        check_eq!(demoted.optional_stage, None);
        Ok(())
    }

    #[test]
    fn clear_dwell_requires_contiguous_fresh_pressure_windows()
    -> Result<(), Box<dyn std::error::Error>> {
        let policy = HysteresisPolicy::new(config())?;
        let offers = [optional_offer()];
        let mut frame = DecisionFrame {
            now_ms: 0,
            preference: EffortPreference::Auto,
            signals: short_clear_signals(0),
            authority: authority(),
            useful_refinement: true,
            offers: &offers,
        };
        let initial = state();
        let first = policy.propose(&frame, initial, &DeclaredCostEstimator)?;
        check_eq!(first.next_state.clear_since_ms, Some(0));
        check_eq!(first.next_state.clear_valid_until_ms, Some(20));

        // A sample is expired at its exclusive TTL boundary. Missing its
        // replacement clears the whole dwell window.
        frame.now_ms = 20;
        let expired = policy.propose(&frame, first.next_state, &DeclaredCostEstimator)?;
        check_eq!(expired.reason, DecisionReason::UnknownTelemetry);
        check_eq!(expired.next_state.clear_since_ms, None);
        check_eq!(expired.next_state.clear_valid_until_ms, None);

        // A fresh clear sample after an unobserved gap starts a new dwell.
        frame.now_ms = 300;
        frame.signals = short_clear_signals(300);
        let gap = policy.propose(&frame, first.next_state, &DeclaredCostEstimator)?;
        check_eq!(gap.target_phase, ExecutionPhase::Fast);
        check_eq!(gap.next_state.clear_since_ms, Some(300));
        check_eq!(gap.next_state.clear_valid_until_ms, Some(320));

        // Replacements exactly at each exclusive boundary are contiguous.
        let mut carry = gap.next_state;
        for now_ms in [320, 340, 360, 380, 400] {
            frame.now_ms = now_ms;
            frame.signals = short_clear_signals(now_ms);
            let next = policy.propose(&frame, carry, &DeclaredCostEstimator)?;
            if now_ms < 400 {
                check_eq!(next.target_phase, ExecutionPhase::Fast);
            } else {
                check_eq!(next.target_phase, ExecutionPhase::Deep);
                check_eq!(next.optional_stage, Some(7));
            }
            carry = next.next_state;
        }
        check_eq!(carry.clear_since_ms, Some(300));
        check_eq!(carry.clear_valid_until_ms, Some(420));

        frame.now_ms = 0;
        check!(matches!(
            policy.propose(
                &frame,
                PolicyState {
                    entered_at_ms: 1,
                    ..initial
                },
                &DeclaredCostEstimator
            ),
            Err(EffortError::InvalidFrame)
        ));
        Ok(())
    }

    #[test]
    fn governor_keeps_required_progress_and_reservations_independent()
    -> Result<(), Box<dyn std::error::Error>> {
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
        check_eq!(admitted.reason, DecisionReason::RequiredProgress);
        check_eq!(admitted.runnable_stage, Some(2));
        check_eq!(admitted.next_state.phase, ExecutionPhase::Fast);
        check!(scheduler.0.lock().is_ok_and(|held| *held));
        drop(admitted);
        check!(scheduler.0.lock().is_ok_and(|held| !*held));
        Ok(())
    }

    #[test]
    fn governor_rejects_foreign_stage_and_provider_metadata_cannot_grant_access()
    -> Result<(), Box<dyn std::error::Error>> {
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
        check!(matches!(
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
        check_eq!(denied.runnable_stage, None);
        check_eq!(denied.reason, DecisionReason::ProviderUnavailable);
        check!(denied.permit.is_none());
        frame.authority.provider = ProviderPermission::Allowed;
        frame.authority.remaining.memory_bytes = 50;
        let budget_denied = governor.admit(
            &frame,
            state(),
            PolicyProposal {
                optional_stage: Some(7),
                ..proposal
            },
        )?;
        check_eq!(budget_denied.reason, DecisionReason::ResourceDenied);
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

    #[test]
    fn estimated_stage_time_does_not_consume_reservation_capacity()
    -> Result<(), Box<dyn std::error::Error>> {
        let scheduler = Arc::new(SingleSlot::default());
        let governor = EffortGovernor::new(Arc::clone(&scheduler));
        let policy = HysteresisPolicy::new(config())?;
        let mut owner = authority();
        // The deadline is still open and all four actual capacity dimensions fit.
        // This offer's declared time is only a ranking estimate.
        owner.remaining_time_ms = 1;
        let offers = [StageOffer {
            estimated_latency_ms: Some(10_000),
            reservation: ResourceNeed {
                cpu_units: 1,
                io_bytes: 1,
                memory_bytes: 100,
                provider_units: 1,
            },
            ..optional_offer()
        }];
        let frame = DecisionFrame {
            now_ms: 301,
            preference: EffortPreference::Deep,
            signals: clear_signals(301),
            authority: owner,
            useful_refinement: true,
            offers: &offers,
        };
        let proposal = policy.propose(&frame, state(), &DeclaredCostEstimator)?;
        check_eq!(proposal.optional_stage, Some(7));
        let admitted = governor.admit(&frame, state(), proposal)?;
        check_eq!(admitted.runnable_stage, Some(7));
        check_eq!(
            admitted.permit.as_ref().map(ReservationGuard::need),
            Some(offers[0].reservation)
        );
        Ok(())
    }

    #[test]
    fn native_capacity_offer_accepts_unknown_time_estimate()
    -> Result<(), Box<dyn std::error::Error>> {
        struct UnknownCost;
        impl StageCostEstimator for UnknownCost {
            fn estimated_latency_ms(&self, _offer: &StageOffer) -> Option<u64> {
                None
            }
        }

        let scheduler = Arc::new(SingleSlot::default());
        let governor = EffortGovernor::new(scheduler);
        let policy = HysteresisPolicy::new(config())?;
        let offers = [StageOffer {
            estimated_latency_ms: None,
            reservation: ResourceNeed {
                cpu_units: 1,
                io_bytes: 1,
                memory_bytes: 100,
                provider_units: 1,
            },
            ..optional_offer()
        }];
        let frame = DecisionFrame {
            now_ms: 301,
            preference: EffortPreference::Deep,
            signals: clear_signals(301),
            authority: authority(),
            useful_refinement: true,
            offers: &offers,
        };
        let proposal = policy.propose(&frame, state(), &UnknownCost)?;
        check_eq!(proposal.optional_stage, Some(7));
        let admitted = governor.admit(&frame, state(), proposal)?;
        check_eq!(admitted.runnable_stage, Some(7));
        check!(admitted.permit.is_some());
        Ok(())
    }

    #[test]
    fn expired_deadline_denies_before_scheduler_reservation()
    -> Result<(), Box<dyn std::error::Error>> {
        struct CountingScheduler(AtomicUsize);
        impl ResourceScheduler for CountingScheduler {
            type Token = ();

            fn try_reserve(&self, _need: ResourceNeed) -> Option<Self::Token> {
                let _prior = self.0.fetch_add(1, Ordering::AcqRel);
                Some(())
            }

            fn release(&self, _token: Self::Token) {}
        }

        let scheduler = Arc::new(CountingScheduler(AtomicUsize::new(0)));
        let governor = EffortGovernor::new(Arc::clone(&scheduler));
        let policy = HysteresisPolicy::new(config())?;
        let offers = [optional_offer()];
        let mut owner = authority();
        owner.remaining_time_ms = 0;
        let frame = DecisionFrame {
            now_ms: 301,
            preference: EffortPreference::Deep,
            signals: clear_signals(301),
            authority: owner,
            useful_refinement: true,
            offers: &offers,
        };
        let proposal = policy.propose(&frame, state(), &DeclaredCostEstimator)?;
        check_eq!(
            governor.admit(&frame, state(), proposal).err(),
            Some(EffortError::Deadline)
        );
        check_eq!(scheduler.0.load(Ordering::Acquire), 0);
        Ok(())
    }

    #[test]
    fn stage_cost_changes_ordering_not_capacity_admission() -> Result<(), Box<dyn std::error::Error>>
    {
        let policy = HysteresisPolicy::new(config())?;
        for estimate in [None, Some(1), Some(10_000)] {
            let scheduler = Arc::new(SingleSlot::default());
            let governor = EffortGovernor::new(Arc::clone(&scheduler));
            let offers = [StageOffer {
                estimated_latency_ms: estimate,
                ..optional_offer()
            }];
            let mut owner = authority();
            owner.remaining_time_ms = 1;
            let frame = DecisionFrame {
                now_ms: 301,
                preference: EffortPreference::Deep,
                signals: clear_signals(301),
                authority: owner,
                useful_refinement: true,
                offers: &offers,
            };
            let proposal = policy.propose(&frame, state(), &DeclaredCostEstimator)?;
            check_eq!(proposal.optional_stage, Some(7));
            let admitted = governor.admit(&frame, state(), proposal)?;
            check_eq!(admitted.runnable_stage, Some(7));
            check_eq!(
                admitted.permit.as_ref().map(ReservationGuard::need),
                Some(offers[0].reservation)
            );
        }
        Ok(())
    }

    #[test]
    fn cheap_estimate_cannot_override_capacity_or_invalid_offer()
    -> Result<(), Box<dyn std::error::Error>> {
        let scheduler = Arc::new(SingleSlot::default());
        let governor = EffortGovernor::new(Arc::clone(&scheduler));
        let policy = HysteresisPolicy::new(config())?;
        let offers = [StageOffer {
            estimated_latency_ms: Some(1),
            ..optional_offer()
        }];
        let mut owner = authority();
        owner.remaining.memory_bytes = 50;
        let frame = DecisionFrame {
            now_ms: 301,
            preference: EffortPreference::Deep,
            signals: clear_signals(301),
            authority: owner,
            useful_refinement: true,
            offers: &offers,
        };
        let proposal = policy.propose(&frame, state(), &DeclaredCostEstimator)?;
        let denied = governor.admit(&frame, state(), proposal)?;
        check_eq!(denied.reason, DecisionReason::ResourceDenied);
        check!(denied.permit.is_none());

        let zero_estimate = [StageOffer {
            estimated_latency_ms: Some(0),
            ..offers[0]
        }];
        let invalid_estimate = DecisionFrame {
            offers: &zero_estimate,
            ..frame
        };
        check_eq!(
            policy
                .propose(&invalid_estimate, state(), &DeclaredCostEstimator)
                .err(),
            Some(EffortError::InvalidFrame)
        );
        let no_capacity = [StageOffer {
            reservation: ResourceNeed::default(),
            ..offers[0]
        }];
        let invalid_capacity = DecisionFrame {
            offers: &no_capacity,
            ..frame
        };
        check_eq!(
            policy
                .propose(&invalid_capacity, state(), &DeclaredCostEstimator)
                .err(),
            Some(EffortError::InvalidFrame)
        );
        Ok(())
    }
}
