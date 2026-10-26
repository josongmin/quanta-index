//! Native Auto inputs sampled from the worker and the single resource ledger.
//!
//! Worker timestamps have a worker-local monotonic origin while effort policy
//! timestamps have a query-local origin. A historical lag is usable only when
//! its age can be represented in the query clock without changing its expiry.
//! Memory headroom is logical admission headroom, not RSS or available RAM.

use std::{sync::Arc, time::Duration};

use quanta_index_core::index::{
    IndexError,
    effort::{EffortSignals, Signal, SignalSample, UnknownReason},
};

use crate::{
    native_effort_ports::{NativeEffortDocumentContext, NativeEffortSignalPort},
    native_resources::{NativeResourceObservation, NativeResourcePlan, NativeResourceSnapshot},
    native_worker::{NativeWorkerObservation, NativeWorkerSnapshot, NativeWorkerStatus},
};

/// Real foreground activity sampled synchronously.
///
/// The implementation uses recorded user or IDE activity. Query duration and
/// worker idleness do not measure foreground activity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeInteractionActivity {
    IdleFor { idle: Duration, valid_for: Duration },
    Unknown(UnknownReason),
}

pub trait NativeInteractionActivityPort: Send + Sync {
    fn observe_activity(&self) -> NativeInteractionActivity;
}

/// The two optional floors must match the service's resource ledger setup.
#[derive(Clone, Copy, Debug)]
pub struct NativeAutoSignalConfig {
    pub resource_plan: NativeResourcePlan,
    pub background_control_enabled: bool,
    pub maintenance_control_enabled: bool,
    pub worker_lag_max_age: Duration,
    pub sample_ttl: Duration,
}

impl NativeAutoSignalConfig {
    fn validate(self) -> Result<u64, IndexError> {
        if self.maintenance_control_enabled {
            self.resource_plan
                .validate_maintenance(self.background_control_enabled)?;
        } else if self.background_control_enabled {
            self.resource_plan.validate_owned_pump()?;
        } else {
            self.resource_plan.validate()?;
        }
        let _validated_lag_ms = positive_ms(self.worker_lag_max_age, "worker lag max age")?;
        positive_ms(self.sample_ttl, "Auto signal TTL")
    }
}

fn positive_ms(value: Duration, name: &str) -> Result<u64, IndexError> {
    let value = u64::try_from(value.as_millis())
        .map_err(|error| IndexError::InvalidInput(format!("{name}: {error}")))?;
    if !(1..=60_000).contains(&value) {
        return Err(IndexError::InvalidInput(format!(
            "{name} must be 1..=60000 ms"
        )));
    }
    Ok(value)
}

pub(crate) trait WorkerSource: Send + Sync {
    fn observe(&self, max_age: Duration) -> Result<NativeWorkerSnapshot, IndexError>;
}

impl WorkerSource for NativeWorkerObservation {
    fn observe(&self, max_age: Duration) -> Result<NativeWorkerSnapshot, IndexError> {
        NativeWorkerObservation::observe(self, max_age)
    }
}

pub(crate) trait ResourceSource: Send + Sync {
    fn observe(&self) -> Result<NativeResourceSnapshot, IndexError>;
}

impl ResourceSource for NativeResourceObservation {
    fn observe(&self) -> Result<NativeResourceSnapshot, IndexError> {
        NativeResourceObservation::observe(self)
    }
}

/// Replaceable signal source for the existing policy and cost-estimator ports.
/// An absent or failed source remains Unknown and cannot authorize Auto Deep.
pub struct NativeAutoSignals {
    worker: Arc<dyn WorkerSource>,
    resources: Arc<dyn ResourceSource>,
    interaction: Option<Arc<dyn NativeInteractionActivityPort>>,
    provider: Option<Arc<dyn crate::native_provider_signals::NativeProviderSignalPort>>,
    config: NativeAutoSignalConfig,
    sample_ttl_ms: u64,
}

impl NativeAutoSignals {
    pub fn new(
        worker: NativeWorkerObservation,
        resources: NativeResourceObservation,
        interaction: Option<Arc<dyn NativeInteractionActivityPort>>,
        config: NativeAutoSignalConfig,
    ) -> Result<Self, IndexError> {
        let sample_ttl_ms = config.validate()?;
        Ok(Self {
            worker: Arc::new(worker),
            resources: Arc::new(resources),
            interaction,
            provider: None,
            config,
            sample_ttl_ms,
        })
    }

    #[must_use]
    pub fn with_provider_signals(
        mut self,
        provider: Arc<dyn crate::native_provider_signals::NativeProviderSignalPort>,
    ) -> Self {
        self.provider = Some(provider);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_sources(
        worker: Arc<dyn WorkerSource>,
        resources: Arc<dyn ResourceSource>,
        interaction: Option<Arc<dyn NativeInteractionActivityPort>>,
        config: NativeAutoSignalConfig,
    ) -> Result<Self, IndexError> {
        let sample_ttl_ms = config.validate()?;
        Ok(Self {
            worker,
            resources,
            interaction,
            provider: None,
            config,
            sample_ttl_ms,
        })
    }
}

impl NativeEffortSignalPort for NativeAutoSignals {
    fn sample(&self, now_ms: u64) -> EffortSignals {
        let worker = self.worker.observe(self.config.worker_lag_max_age);
        let resources = self.resources.observe();
        let (index_backlog, dispatch_lag_ms) = match worker {
            Ok(snapshot) if snapshot.status == NativeWorkerStatus::Running => {
                let backlog = u32::from(snapshot.core.queued)
                    .checked_add(u32::from(snapshot.enrichment.queued))
                    .map_or(Signal::Unknown(UnknownReason::Dropped), |value| {
                        known(value, now_ms, self.sample_ttl_ms)
                    });
                (backlog, combined_lag(snapshot, now_ms, self.sample_ttl_ms))
            }
            Ok(_) | Err(_) => (
                Signal::Unknown(UnknownReason::Dropped),
                Signal::Unknown(UnknownReason::Dropped),
            ),
        };
        let (interactive_pressure_bps, memory_headroom_bytes) = match resources {
            Ok(snapshot) if snapshot.capacity == self.config.resource_plan.capacity => (
                interactive_pressure(&snapshot, now_ms, self.sample_ttl_ms),
                logical_memory_headroom(&snapshot, self.config)
                    .map_or(Signal::Unknown(UnknownReason::Dropped), |value| {
                        known(value, now_ms, self.sample_ttl_ms)
                    }),
            ),
            Ok(_) | Err(_) => (
                Signal::Unknown(UnknownReason::Dropped),
                Signal::Unknown(UnknownReason::Dropped),
            ),
        };
        let interaction_idle_ms =
            self.interaction
                .as_ref()
                .map_or(
                    Signal::Unknown(UnknownReason::Missing),
                    |source| match source.observe_activity() {
                        NativeInteractionActivity::IdleFor { idle, valid_for } => {
                            u64::try_from(idle.as_millis())
                                .and_then(|value| {
                                    u64::try_from(valid_for.as_millis()).map(|ttl| (value, ttl))
                                })
                                .map_or(Signal::Unknown(UnknownReason::Dropped), |(value, ttl)| {
                                    if ttl == 0 {
                                        Signal::Unknown(UnknownReason::Missing)
                                    } else {
                                        known(value, now_ms, self.sample_ttl_ms.min(ttl))
                                    }
                                })
                        }
                        NativeInteractionActivity::Unknown(reason) => Signal::Unknown(reason),
                    },
                );
        EffortSignals {
            interaction_idle_ms,
            interactive_pressure_bps,
            index_backlog,
            dispatch_lag_ms,
            memory_headroom_bytes,
            provider: self
                .provider
                .as_ref()
                .map_or(Signal::Unknown(UnknownReason::Unsupported), |source| {
                    source.sample(now_ms, self.sample_ttl_ms)
                }),
            quality_confidence_bps: Signal::Unknown(UnknownReason::Unsupported),
        }
    }

    fn sample_for_document(
        &self,
        now_ms: u64,
        context: &NativeEffortDocumentContext<'_>,
    ) -> EffortSignals {
        let mut signals = self.sample(now_ms);
        if !valid_document_context(context) {
            signals.interaction_idle_ms = Signal::Unknown(UnknownReason::Dropped);
            return signals;
        }
        if !context.revisions.iter().any(|row| !row.retired) {
            signals.interaction_idle_ms = Signal::Unknown(UnknownReason::Missing);
            return signals;
        }
        let Some(unchanged_for) = context.unchanged_for else {
            signals.interaction_idle_ms = Signal::Unknown(UnknownReason::Missing);
            return signals;
        };
        let Ok(unchanged_ms) = u64::try_from(unchanged_for.as_millis()) else {
            signals.interaction_idle_ms = Signal::Unknown(UnknownReason::Dropped);
            return signals;
        };
        if let Signal::Known(sample) = &mut signals.interaction_idle_ms {
            sample.value = sample.value.min(unchanged_ms);
        }
        signals
    }
}

fn valid_document_context(context: &NativeEffortDocumentContext<'_>) -> bool {
    if context.session.epoch == 0
        || context.session.publisher.0 == [0; 32]
        || context.revisions.len() > crate::native_overlay::HARD_MAX_DOCUMENTS
    {
        return false;
    }
    let mut previous = None;
    for row in context.revisions {
        if row.revision == 0
            || (row.retired && !row.deleted)
            || previous.is_some_and(|owner| owner >= row.owner)
        {
            return false;
        }
        previous = Some(row.owner);
    }
    true
}

fn known<T>(value: T, observed_at_ms: u64, valid_for_ms: u64) -> Signal<T> {
    Signal::Known(SignalSample {
        value,
        observed_at_ms,
        valid_for_ms,
    })
}

fn project_lag(lag: Signal<u64>, worker_now_ms: u64, query_now_ms: u64) -> Signal<u64> {
    match lag {
        Signal::Known(sample) => {
            let Some(age) = worker_now_ms.checked_sub(sample.observed_at_ms) else {
                return Signal::Unknown(UnknownReason::Dropped);
            };
            if age >= sample.valid_for_ms {
                return Signal::Unknown(UnknownReason::Dropped);
            }
            let Some(observed_at_ms) = query_now_ms.checked_sub(age) else {
                // The observation predates the query origin. This clock cannot
                // represent it without extending the source's original TTL.
                return Signal::Unknown(UnknownReason::Dropped);
            };
            known(sample.value, observed_at_ms, sample.valid_for_ms)
        }
        Signal::Unknown(reason) => Signal::Unknown(reason),
    }
}

fn combined_lag(snapshot: NativeWorkerSnapshot, now_ms: u64, sample_ttl_ms: u64) -> Signal<u64> {
    if snapshot.core.queued > 0 || snapshot.enrichment.queued > 0 {
        // A previous dequeue says nothing about how long the currently
        // accepted commands have waited. The worker does not expose their
        // oldest enqueue timestamp, so this cannot be a clear-pressure input.
        return Signal::Unknown(UnknownReason::Missing);
    }
    if snapshot.core.active == 0 && snapshot.enrichment.active == 0 {
        // No accepted work is waiting or executing on either lane in this
        // coherent snapshot. Current pending dispatch wait is observed zero;
        // this does not reinterpret a missing historical dequeue as zero.
        return known(0, now_ms, sample_ttl_ms);
    }
    let core = project_lag(
        snapshot.core.dispatch_lag_ms,
        snapshot.observed_at_ms,
        now_ms,
    );
    let enrichment = project_lag(
        snapshot.enrichment.dispatch_lag_ms,
        snapshot.observed_at_ms,
        now_ms,
    );
    match (core, enrichment) {
        (Signal::Known(core), Signal::Known(enrichment)) => {
            let observed_at_ms = core.observed_at_ms.min(enrichment.observed_at_ms);
            let expiry_ms = core
                .observed_at_ms
                .checked_add(core.valid_for_ms)
                .zip(
                    enrichment
                        .observed_at_ms
                        .checked_add(enrichment.valid_for_ms),
                )
                .map(|(core, enrichment)| core.min(enrichment));
            expiry_ms
                .and_then(|expiry| expiry.checked_sub(observed_at_ms))
                .map_or(Signal::Unknown(UnknownReason::Dropped), |valid_for_ms| {
                    known(
                        core.value.max(enrichment.value),
                        observed_at_ms,
                        valid_for_ms,
                    )
                })
        }
        (Signal::Unknown(UnknownReason::Dropped), _)
        | (_, Signal::Unknown(UnknownReason::Dropped)) => Signal::Unknown(UnknownReason::Dropped),
        (Signal::Unknown(reason), _) | (_, Signal::Unknown(reason)) => Signal::Unknown(reason),
    }
}

fn ratio_bps(used: u64, capacity: u64) -> Result<Option<u16>, IndexError> {
    if capacity == 0 {
        return Ok(None);
    }
    let ratio = u128::from(used)
        .checked_mul(10_000)
        .and_then(|value| value.checked_div(u128::from(capacity)))
        .ok_or(IndexError::ResourceLimited)?
        .min(10_000);
    u16::try_from(ratio)
        .map(Some)
        .map_err(|_error| IndexError::ResourceLimited)
}

fn interactive_pressure(
    snapshot: &NativeResourceSnapshot,
    now_ms: u64,
    ttl_ms: u64,
) -> Signal<u16> {
    let used = snapshot.interactive.reserved;
    let cap = snapshot.capacity;
    if snapshot.interactive.active > 0 && used.cpu_units == 0 {
        return Signal::Unknown(UnknownReason::Dropped);
    }
    let ratios = [
        ratio_bps(used.cpu_units, cap.cpu_units),
        ratio_bps(used.io_bytes, cap.io_bytes),
        ratio_bps(used.memory_bytes, cap.memory_bytes),
        ratio_bps(used.provider_units, cap.provider_units),
    ];
    let pressure = ratios
        .into_iter()
        .try_fold(None, |maximum, ratio| ratio.map(|value| maximum.max(value)));
    pressure.map_or(Signal::Unknown(UnknownReason::Dropped), |value| {
        value.map_or(Signal::Unknown(UnknownReason::Dropped), |value| {
            known(value, now_ms, ttl_ms)
        })
    })
}

fn logical_memory_headroom(
    snapshot: &NativeResourceSnapshot,
    config: NativeAutoSignalConfig,
) -> Option<u64> {
    let mut promised = snapshot
        .reserved
        .memory_bytes
        .checked_add(snapshot.retained_memory_bytes)?
        .checked_add(snapshot.pending_escape_bytes)?;
    let plan = config.resource_plan;
    for (floor, reserved) in [
        (plan.control, snapshot.control.reserved),
        (plan.interactive, snapshot.interactive.reserved),
        (plan.core, snapshot.core.reserved),
        (plan.enrichment, snapshot.enrichment.reserved),
    ] {
        promised =
            promised.checked_add(floor.memory_bytes.saturating_sub(reserved.memory_bytes))?;
    }
    if config.background_control_enabled {
        promised = promised.checked_add(
            plan.control
                .memory_bytes
                .saturating_sub(snapshot.background_control.reserved.memory_bytes),
        )?;
    }
    if config.maintenance_control_enabled {
        promised = promised.checked_add(
            plan.control
                .memory_bytes
                .saturating_sub(snapshot.maintenance_control.reserved.memory_bytes),
        )?;
    }
    snapshot.capacity.memory_bytes.checked_sub(promised)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{native_resources::NativeClassSnapshot, native_worker::NativeLaneSnapshot};
    use quanta_index_contract::native_protocol::{
        NativeInteractionPublisherId, NativeOverlayRevision, NativeOverlaySession,
    };
    use quanta_index_core::index::{
        CollectionId, Digest, OwnerKey, ProducerId, SlotId, StableOwnerId, TenantId, WorkspaceKey,
        WorktreeId, effort::ResourceNeed,
    };

    macro_rules! check_eq {
        ($actual:expr, $expected:expr $(,)?) => {
            if $actual != $expected {
                return Err(IndexError::BackendViolation(format!(
                    "assertion failed: {} == {}",
                    stringify!($actual),
                    stringify!($expected)
                )));
            }
        };
    }

    struct StaticWorker(Option<NativeWorkerSnapshot>);

    impl WorkerSource for StaticWorker {
        fn observe(&self, _max_age: Duration) -> Result<NativeWorkerSnapshot, IndexError> {
            self.0.ok_or(IndexError::ResourceLimited)
        }
    }

    struct StaticResources(Option<NativeResourceSnapshot>);

    impl ResourceSource for StaticResources {
        fn observe(&self) -> Result<NativeResourceSnapshot, IndexError> {
            self.0.ok_or(IndexError::ResourceLimited)
        }
    }

    struct Activity(NativeInteractionActivity);

    impl NativeInteractionActivityPort for Activity {
        fn observe_activity(&self) -> NativeInteractionActivity {
            self.0
        }
    }

    fn need(memory_bytes: u64) -> ResourceNeed {
        ResourceNeed {
            cpu_units: 1,
            io_bytes: 100,
            memory_bytes,
            provider_units: 0,
        }
    }

    fn config() -> NativeAutoSignalConfig {
        NativeAutoSignalConfig {
            resource_plan: NativeResourcePlan {
                capacity: ResourceNeed {
                    cpu_units: 100,
                    io_bytes: 10_000,
                    memory_bytes: 10_000,
                    provider_units: 10,
                },
                control: need(100),
                interactive: need(100),
                core: need(100),
                enrichment: need(100),
            },
            background_control_enabled: true,
            maintenance_control_enabled: false,
            worker_lag_max_age: Duration::from_millis(200),
            sample_ttl: Duration::from_millis(20),
        }
    }

    fn worker() -> NativeWorkerSnapshot {
        NativeWorkerSnapshot {
            observed_at_ms: 1_000,
            status: NativeWorkerStatus::Running,
            core: NativeLaneSnapshot {
                queued: 2,
                active: 1,
                dispatch_lag_ms: known(10, 990, 200),
            },
            enrichment: NativeLaneSnapshot {
                queued: 3,
                active: 0,
                dispatch_lag_ms: known(15, 980, 200),
            },
        }
    }

    fn worker_without_backlog() -> NativeWorkerSnapshot {
        let mut snapshot = worker();
        snapshot.core.queued = 0;
        snapshot.enrichment.queued = 0;
        snapshot.enrichment.active = 1;
        snapshot
    }

    fn resources() -> NativeResourceSnapshot {
        let floor = NativeClassSnapshot {
            active: 1,
            reserved: need(100),
        };
        NativeResourceSnapshot {
            capacity: config().resource_plan.capacity,
            reserved: ResourceNeed {
                cpu_units: 5,
                io_bytes: 1_000,
                memory_bytes: 2_000,
                provider_units: 0,
            },
            live_memory_bytes: 1_000,
            retained_memory_bytes: 500,
            pending_escape_bytes: 500,
            control: floor,
            background_control: NativeClassSnapshot::default(),
            maintenance_control: NativeClassSnapshot::default(),
            interactive: floor,
            core: floor,
            enrichment: floor,
        }
    }

    fn adapter(
        worker: Option<NativeWorkerSnapshot>,
        resources: Option<&NativeResourceSnapshot>,
        interaction: Option<NativeInteractionActivity>,
    ) -> Result<NativeAutoSignals, IndexError> {
        NativeAutoSignals::with_sources(
            Arc::new(StaticWorker(worker)),
            Arc::new(StaticResources(resources.copied())),
            interaction.map(|activity| {
                let source: Arc<dyn NativeInteractionActivityPort> = Arc::new(Activity(activity));
                source
            }),
            config(),
        )
    }

    fn document_context(
        revisions: &[NativeOverlayRevision],
        unchanged_for: Option<Duration>,
    ) -> NativeEffortDocumentContext<'_> {
        NativeEffortDocumentContext {
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
            revisions,
            unchanged_for,
        }
    }

    fn revision(id: u8, revision: u64, deleted: bool, retired: bool) -> NativeOverlayRevision {
        NativeOverlayRevision {
            owner: OwnerKey {
                producer: ProducerId([7; 16]),
                stable_id: StableOwnerId([id; 16]),
            },
            revision,
            deleted,
            retired,
        }
    }

    #[test]
    fn document_idle_is_bounded_by_live_revision_age_without_extending_ttl()
    -> Result<(), IndexError> {
        let source = adapter(
            Some(worker()),
            Some(&resources()),
            Some(NativeInteractionActivity::IdleFor {
                idle: Duration::from_millis(500),
                valid_for: Duration::from_millis(3),
            }),
        )?;
        let rows = [revision(1, 1, false, false)];
        let first = source.sample_for_document(
            100,
            &document_context(&rows, Some(Duration::from_millis(120))),
        );
        check_eq!(first.interaction_idle_ms, known(120, 100, 3));
        check_eq!(first.interaction_idle_ms.fresh(102), Some(120));
        check_eq!(first.interaction_idle_ms.fresh(103), None);
        check_eq!(
            first.quality_confidence_bps,
            Signal::Unknown(UnknownReason::Unsupported)
        );
        let changed = [revision(1, 2, false, false)];
        let next = source.sample_for_document(
            100,
            &document_context(&changed, Some(Duration::from_millis(0))),
        );
        check_eq!(next.interaction_idle_ms.fresh(100), Some(0));
        let unknown_source = adapter(Some(worker()), Some(&resources()), None)?;
        let unknown = unknown_source.sample_for_document(
            100,
            &document_context(&rows, Some(Duration::from_millis(120))),
        );
        check_eq!(
            unknown.interaction_idle_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        Ok(())
    }

    #[test]
    fn missing_or_malformed_document_context_cannot_claim_idle() -> Result<(), IndexError> {
        let source = adapter(
            Some(worker()),
            Some(&resources()),
            Some(NativeInteractionActivity::IdleFor {
                idle: Duration::from_millis(500),
                valid_for: Duration::from_millis(20),
            }),
        )?;
        let empty = source.sample_for_document(100, &document_context(&[], None));
        check_eq!(
            empty.interaction_idle_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        let retired = [revision(1, 2, true, true)];
        let missing = source.sample_for_document(
            100,
            &document_context(&retired, Some(Duration::from_secs(10))),
        );
        check_eq!(
            missing.interaction_idle_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        for malformed in [
            [revision(1, 1, false, false), revision(1, 2, false, false)],
            [revision(2, 1, false, false), revision(1, 2, false, false)],
            [revision(1, 0, false, false), revision(2, 2, false, false)],
            [revision(1, 1, false, true), revision(2, 2, false, false)],
        ] {
            let sampled = source.sample_for_document(
                100,
                &document_context(&malformed, Some(Duration::from_secs(10))),
            );
            check_eq!(
                sampled.interaction_idle_ms,
                Signal::Unknown(UnknownReason::Dropped)
            );
        }
        let active = [revision(1, 1, false, false)];
        let mut foreign = document_context(&active, Some(Duration::from_millis(120)));
        foreign.session.publisher = NativeInteractionPublisherId([0; 32]);
        check_eq!(
            source
                .sample_for_document(100, &foreign)
                .interaction_idle_ms,
            Signal::Unknown(UnknownReason::Dropped)
        );
        Ok(())
    }

    #[test]
    fn samples_actual_load_and_conservative_logical_headroom() -> Result<(), IndexError> {
        let sampled = adapter(
            Some(worker()),
            Some(&resources()),
            Some(NativeInteractionActivity::IdleFor {
                idle: Duration::from_millis(500),
                valid_for: Duration::from_millis(20),
            }),
        )?
        .sample(100);
        check_eq!(sampled.index_backlog.fresh(100), Some(5));
        check_eq!(
            sampled.dispatch_lag_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        check_eq!(sampled.interactive_pressure_bps.fresh(100), Some(100));
        // 2,000 active + 500 retained + 500 future escape + 100 optional
        // background-control floor. live_memory overlaps these commitments.
        check_eq!(sampled.memory_headroom_bytes.fresh(100), Some(6_900));
        check_eq!(sampled.interaction_idle_ms.fresh(100), Some(500));
        check_eq!(
            sampled.provider,
            Signal::Unknown(UnknownReason::Unsupported)
        );
        check_eq!(
            sampled.quality_confidence_bps,
            Signal::Unknown(UnknownReason::Unsupported)
        );
        Ok(())
    }

    #[test]
    fn interaction_sample_never_outlives_its_publisher_lease() -> Result<(), IndexError> {
        let sampled = adapter(
            Some(worker()),
            Some(&resources()),
            Some(NativeInteractionActivity::IdleFor {
                idle: Duration::from_millis(500),
                valid_for: Duration::from_millis(3),
            }),
        )?
        .sample(100);
        check_eq!(sampled.interaction_idle_ms.fresh(102), Some(500));
        check_eq!(sampled.interaction_idle_ms.fresh(103), None);
        let expired = adapter(
            Some(worker()),
            Some(&resources()),
            Some(NativeInteractionActivity::IdleFor {
                idle: Duration::from_millis(500),
                valid_for: Duration::from_micros(999),
            }),
        )?
        .sample(100);
        check_eq!(
            expired.interaction_idle_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        Ok(())
    }

    #[test]
    fn worker_lag_keeps_original_expiry_across_clock_origins() -> Result<(), IndexError> {
        let sampled =
            adapter(Some(worker_without_backlog()), Some(&resources()), None)?.sample(100);
        check_eq!(sampled.dispatch_lag_ms, known(15, 80, 200));
        check_eq!(sampled.dispatch_lag_ms.fresh(279), Some(15));
        check_eq!(sampled.dispatch_lag_ms.fresh(280), None);
        let mut shorter_enrichment = worker_without_backlog();
        shorter_enrichment.enrichment.dispatch_lag_ms = known(15, 980, 100);
        let sampled = adapter(Some(shorter_enrichment), Some(&resources()), None)?.sample(100);
        check_eq!(sampled.dispatch_lag_ms, known(15, 80, 100));
        check_eq!(sampled.dispatch_lag_ms.fresh(179), Some(15));
        check_eq!(sampled.dispatch_lag_ms.fresh(180), None);
        check_eq!(
            sampled.interaction_idle_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        Ok(())
    }

    #[test]
    fn missing_or_unrepresentable_lag_never_becomes_zero_or_fresh() -> Result<(), IndexError> {
        let sampled = adapter(Some(worker_without_backlog()), Some(&resources()), None)?.sample(5);
        check_eq!(
            sampled.dispatch_lag_ms,
            Signal::Unknown(UnknownReason::Dropped)
        );
        let mut no_dequeue = worker_without_backlog();
        no_dequeue.enrichment.dispatch_lag_ms = Signal::Unknown(UnknownReason::Missing);
        let sampled = adapter(Some(no_dequeue), Some(&resources()), None)?.sample(100);
        check_eq!(
            sampled.dispatch_lag_ms,
            Signal::Unknown(UnknownReason::Missing)
        );
        let sampled = adapter(None, Some(&resources()), None)?.sample(100);
        check_eq!(
            sampled.index_backlog,
            Signal::Unknown(UnknownReason::Dropped)
        );
        Ok(())
    }

    #[test]
    fn observed_empty_lanes_report_current_zero_pending_wait() -> Result<(), IndexError> {
        let mut empty = worker();
        empty.core.queued = 0;
        empty.core.active = 0;
        empty.enrichment.queued = 0;
        empty.enrichment.active = 0;
        empty.core.dispatch_lag_ms = Signal::Unknown(UnknownReason::Missing);
        empty.enrichment.dispatch_lag_ms = Signal::Unknown(UnknownReason::Missing);
        let sampled = adapter(Some(empty), Some(&resources()), None)?.sample(0);
        check_eq!(sampled.dispatch_lag_ms.fresh(0), Some(0));
        check_eq!(sampled.index_backlog.fresh(0), Some(0));
        Ok(())
    }

    #[test]
    fn stale_or_mismatched_ledger_cannot_claim_memory_headroom() -> Result<(), IndexError> {
        let mut bad = resources();
        bad.capacity.memory_bytes += 1;
        let sampled = adapter(Some(worker()), Some(&bad), None)?.sample(100);
        check_eq!(
            sampled.memory_headroom_bytes,
            Signal::Unknown(UnknownReason::Dropped)
        );
        let mut overcommitted = resources();
        overcommitted.pending_escape_bytes = u64::MAX;
        let sampled = adapter(Some(worker()), Some(&overcommitted), None)?.sample(100);
        check_eq!(
            sampled.memory_headroom_bytes,
            Signal::Unknown(UnknownReason::Dropped)
        );
        Ok(())
    }
}
