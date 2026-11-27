//! Bounded automatic reclaim traversal for the service-owned maintenance pump.

use std::time::{Duration, Instant};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    IndexError, MAX_RECLAIM_DRAIN_PAGE, ObjectRef, ReadSessionRecoveryPage, ReadSessionRef,
    ReclaimDrainPage, WorkBudget, WorkLimits,
    ports::{CatalogReclaimPort, ManagedObjectReclaimPort},
};
use quanta_index_storage::FileObjectStore;

use crate::native_wal_reclaim::NativeWalReclaimPolicy;

const MAX_STEPS_PER_ROUND: u32 = 8;
const MAX_HEAD_PROBE_PERIOD: u32 = 1024;
const MAX_HEAD_REVISIT: Duration = Duration::from_secs(300);
const MIN_ROUND_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_ROUND_TIMEOUT: Duration = Duration::from_secs(120);
const MIN_INTERVAL: Duration = Duration::from_millis(10);
const MAX_INTERVAL: Duration = Duration::from_secs(60);

// One candidate may validate a 16 MiB wire object twice, decode a bounded
// registration and drain 32 child edges. These are conservative initial
// WorkUsage admission floors, not a proved all-phase maximum or IO/RSS limit.
// A charged step can still return ResourceLimited and its durable phase must
// be surfaced rather than silently retried in a hot loop.
const STEP_READS: u64 = 16_384;
const STEP_WRITES: u64 = 256;
const STEP_BYTES: u64 = 134_217_728;
const STEP_MUTATIONS: u64 = 256;

/// One resource-admitted, bounded physical-GC step. A partially drained
/// object keeps the caller's previous cursor so the next step can resume it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReclaimStep {
    pub examined: Option<ObjectRef>,
    pub drain: Option<ReclaimDrainPage>,
    pub resume_after: Option<ObjectRef>,
    pub has_more: bool,
}

/// A conflict with a validated queue key can defer that object while the
/// automatic sweep continues. It does not report successful reclamation.
#[derive(Clone, Copy)]
pub(super) enum ReclaimAttempt {
    Applied(NativeReclaimStep),
    Contended { examined: ObjectRef, has_more: bool },
}

impl ReclaimAttempt {
    fn into_manual_step(self) -> Result<NativeReclaimStep, IndexError> {
        match self {
            Self::Applied(step) => Ok(step),
            Self::Contended { .. } => Err(IndexError::Conflict),
        }
    }
}

/// Explicit automatic reclaim cadence and original per-round work allowance.
///
/// The caller must reserve `MaintenanceControl` before invoking the driver and
/// construct one `WorkBudget` with this allowance and `round_timeout`. Every
/// step in that round consumes the same budget. The timeout is checked at
/// explicit checkpoints; it cannot preempt an in-flight filesystem syscall.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReclaimConfig {
    pub interval: Duration,
    /// Independent optional WAL truncation cadence; never inferred from GC reserve.
    pub wal_reclaim: NativeWalReclaimPolicy,
    pub max_steps_per_round: u32,
    pub child_limit: u32,
    pub work_limits: WorkLimits,
    pub round_timeout: Duration,
    /// Every Nth round probes the queue head independently of the forward
    /// cursor. The forward cursor continues on subsequent rounds.
    pub head_probe_every: u32,
}

impl NativeReclaimConfig {
    pub fn validate(&self) -> Result<(), IndexError> {
        // Validate shape here; service assembly checks the actual Control IO bound.
        self.wal_reclaim.validate(u64::MAX)?;
        if self.interval < MIN_INTERVAL || self.interval > MAX_INTERVAL {
            return Err(IndexError::InvalidInput(
                "native reclaim interval must be 10ms..=60s".into(),
            ));
        }
        if !(1..=MAX_STEPS_PER_ROUND).contains(&self.max_steps_per_round) {
            return Err(IndexError::InvalidInput(
                "native reclaim round must contain 1..=8 steps".into(),
            ));
        }
        if !(1..=MAX_RECLAIM_DRAIN_PAGE).contains(&self.child_limit) {
            return Err(IndexError::InvalidInput(
                "native reclaim child page must contain 1..=32 edges".into(),
            ));
        }
        if self.round_timeout < MIN_ROUND_TIMEOUT || self.round_timeout > MAX_ROUND_TIMEOUT {
            return Err(IndexError::InvalidInput(
                "native reclaim round timeout must be 100ms..=120s".into(),
            ));
        }
        if !(2..=MAX_HEAD_PROBE_PERIOD).contains(&self.head_probe_every) {
            return Err(IndexError::InvalidInput(
                "native reclaim head probe period must be 2..=1024 rounds".into(),
            ));
        }
        if self
            .interval
            .checked_mul(self.head_probe_every)
            .is_none_or(|revisit| revisit > MAX_HEAD_REVISIT)
        {
            return Err(IndexError::InvalidInput(
                "native reclaim configured head cadence exceeds five minutes".into(),
            ));
        }
        let steps = u64::from(self.max_steps_per_round);
        let required = WorkLimits {
            reads: STEP_READS
                .checked_mul(steps)
                .ok_or(IndexError::ResourceLimited)?,
            writes: STEP_WRITES
                .checked_mul(steps)
                .ok_or(IndexError::ResourceLimited)?,
            bytes: STEP_BYTES
                .checked_mul(steps)
                .ok_or(IndexError::ResourceLimited)?,
            mutations: STEP_MUTATIONS
                .checked_mul(steps)
                .ok_or(IndexError::ResourceLimited)?,
        };
        if self.work_limits.reads < required.reads
            || self.work_limits.writes < required.writes
            || self.work_limits.bytes < required.bytes
            || self.work_limits.mutations < required.mutations
        {
            return Err(IndexError::InvalidInput(
                "native reclaim WorkLimits are below the bounded round allowance".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct NativeReclaimRound {
    pub steps: u32,
    pub examined: u32,
    pub head_probed: bool,
    pub exhausted_forward: bool,
    /// A step from the queue head found no work: the queue was empty then.
    pub queue_empty: bool,
    /// Steps that applied work to an examined key (reclaimed it, drained a
    /// child page, or dequeued it as still referenced).
    pub applied: u32,
    /// A forward pass that began at the queue head reached its end and every
    /// key it examined was contended.
    pub stalled_pass: bool,
}

/// Execute one bounded page under a reservation owned by the caller.
pub(super) fn reclaim_one_step(
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    after: Option<ObjectRef>,
    child_limit: u32,
    budget: &mut WorkBudget,
) -> Result<NativeReclaimStep, IndexError> {
    reclaim_one_attempt(catalog, objects, after, child_limit, budget)?.into_manual_step()
}

pub(super) fn reclaim_one_attempt(
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    after: Option<ObjectRef>,
    child_limit: u32,
    budget: &mut WorkBudget,
) -> Result<ReclaimAttempt, IndexError> {
    budget.checkpoint()?;
    if !(1..=MAX_RECLAIM_DRAIN_PAGE).contains(&child_limit) {
        return Err(IndexError::InvalidInput(
            "reclaim child page limit is out of range".into(),
        ));
    }
    let page = catalog.reclaim_work_page(after, 1, budget)?;
    let mut items = page.items.into_iter();
    let item = items.next();
    if items.next().is_some() || (item.is_none() && page.has_more) {
        return Err(IndexError::BackendViolation(
            "reclaim work page violated one-item bound".into(),
        ));
    }
    let Some(work) = item else {
        return Ok(ReclaimAttempt::Applied(NativeReclaimStep {
            examined: None,
            drain: None,
            resume_after: after,
            has_more: false,
        }));
    };
    let object = work.object();
    if after.is_some_and(|previous| object <= previous) {
        return Err(IndexError::BackendViolation(
            "reclaim work page did not advance in key order".into(),
        ));
    }
    let drain = match objects.reclaim_one(work, child_limit, budget) {
        Ok(drain) => drain,
        Err(IndexError::Conflict) => {
            return Ok(ReclaimAttempt::Contended {
                examined: object,
                has_more: page.has_more,
            });
        }
        Err(error) => return Err(error),
    };
    let partial = drain.is_some_and(|page| page.has_more);
    Ok(ReclaimAttempt::Applied(NativeReclaimStep {
        examined: Some(object),
        drain,
        resume_after: if partial { after } else { Some(object) },
        has_more: partial || page.has_more,
    }))
}

#[derive(Clone, Copy)]
enum HeadSweep {
    Idle,
    Prefix {
        after: Option<ObjectRef>,
        until: Option<ObjectRef>,
    },
}

/// State is service-local; Catalog remains the durable phase and queue owner.
pub(crate) struct NativeReclaimDriver {
    config: NativeReclaimConfig,
    forward_after: Option<ObjectRef>,
    forward_saw_contention: bool,
    head_sweep: HeadSweep,
    rounds_since_head: u32,
    read_recovery_after: Option<ReadSessionRef>,
    read_recovery_pending: bool,
    next_due: Instant,
    /// The current forward pass began at the queue head, and what it has met
    /// so far. A pass may span rounds; it ends when the sweep exhausts.
    pass: ForwardPass,
}

#[derive(Clone, Copy, Debug, Default)]
struct ForwardPass {
    from_head: bool,
    applied: bool,
    contended: bool,
}

impl NativeReclaimDriver {
    pub(crate) fn new(config: NativeReclaimConfig, now: Instant) -> Result<Self, IndexError> {
        config.validate()?;
        Ok(Self {
            config,
            forward_after: None,
            forward_saw_contention: false,
            head_sweep: HeadSweep::Idle,
            rounds_since_head: 0,
            read_recovery_after: None,
            read_recovery_pending: true,
            next_due: now,
            pass: ForwardPass::default(),
        })
    }

    pub(crate) const fn next_due(&self) -> Instant {
        self.next_due
    }

    pub(crate) const fn read_recovery_due(&self) -> bool {
        self.read_recovery_pending
            || self.rounds_since_head >= self.config.head_probe_every.saturating_sub(1)
    }

    pub(crate) const fn read_recovery_after(&self) -> Option<ReadSessionRef> {
        self.read_recovery_after
    }

    pub(crate) fn record_read_recovery(&mut self, page: ReadSessionRecoveryPage) {
        self.read_recovery_pending = page.has_more;
        self.read_recovery_after = if page.has_more { page.after } else { None };
    }

    /// Steer the next round by committed Catalog demand.
    ///
    /// Abandoned pins make read recovery due. Newly queued work (a view
    /// retirement's roots, a drain's freed children) can sort below the
    /// forward cursor, so the forward sweep restarts at the queue head and the
    /// whole step budget of the next round reaches it. A head probe alone
    /// would take one step per round. Once the forward pass skips a contended
    /// key, finish that pass before allowing another demand to restart it.
    /// Otherwise a busy lower prefix could consume every round's steps.
    pub(crate) fn schedule(&mut self, demand: &quanta_index_catalog::MaintenanceDemand) {
        if demand.read_recovery {
            self.read_recovery_pending = true;
        }
        if demand.reclaim && !self.forward_saw_contention {
            self.forward_after = None;
        }
    }

    /// Scheduler denial consumes a cadence slot without changing traversal.
    pub(crate) fn defer(&mut self, now: Instant) -> Result<(), IndexError> {
        self.next_due = now
            .max(self.next_due)
            .checked_add(self.config.interval)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }

    /// Invoke at most one head probe and the configured total number of steps.
    ///
    /// The caller owns one `MaintenanceControl` reservation and one original
    /// budget. Unlocated errors preserve the cursor; an explicitly contended
    /// queue key advances only the forward sweep and remains due for retries.
    pub(crate) fn run_attempt_round<F>(
        &mut self,
        now: Instant,
        budget: &mut WorkBudget,
        mut step: F,
    ) -> Result<NativeReclaimRound, IndexError>
    where
        F: FnMut(Option<ObjectRef>, u32, &mut WorkBudget) -> Result<ReclaimAttempt, IndexError>,
    {
        if now < self.next_due {
            return Ok(NativeReclaimRound::default());
        }
        // Schedule before any fallible work. A transient denial or durable
        // partial failure cannot cause a tight retry loop in the pump.
        self.defer(now)?;
        let result = self.run_scheduled_round(budget, &mut step);
        let finished = Instant::now();
        if finished >= self.next_due {
            self.next_due = finished
                .checked_add(self.config.interval)
                .ok_or(IndexError::ResourceLimited)?;
        }
        result
    }

    // Keep existing applied-step goldens on the same production traversal.
    #[cfg(test)]
    fn run_round<F>(
        &mut self,
        now: Instant,
        budget: &mut WorkBudget,
        mut step: F,
    ) -> Result<NativeReclaimRound, IndexError>
    where
        F: FnMut(Option<ObjectRef>, u32, &mut WorkBudget) -> Result<NativeReclaimStep, IndexError>,
    {
        self.run_attempt_round(now, budget, |after, limit, budget| {
            step(after, limit, budget).map(ReclaimAttempt::Applied)
        })
    }

    fn run_scheduled_round<F>(
        &mut self,
        budget: &mut WorkBudget,
        step: &mut F,
    ) -> Result<NativeReclaimRound, IndexError>
    where
        F: FnMut(Option<ObjectRef>, u32, &mut WorkBudget) -> Result<ReclaimAttempt, IndexError>,
    {
        budget.checkpoint()?;
        let mut result = NativeReclaimRound::default();
        // In-round restart state: children queued since the current forward
        // pass began, and whether any key this round stayed contended.
        let mut queued_since_pass = false;
        let mut contended = false;
        let head_period_before_probe = self
            .config
            .head_probe_every
            .checked_sub(1)
            .ok_or(IndexError::ResourceLimited)?;
        let head_due = self.rounds_since_head >= head_period_before_probe;
        if head_due {
            // Cadence counts attempts, including a transient custody Conflict.
            // A repeatedly contended head must not suppress forward rounds.
            self.rounds_since_head = 0;
            // Sweep the prefix independently. Freeze the forward frontier at
            // cycle start so continuous upper-key arrivals cannot prolong a
            // head cycle, and a contended first key cannot hide lower arrivals.
            let (after, until) = match self.head_sweep {
                HeadSweep::Idle => (None, self.forward_after),
                HeadSweep::Prefix { after, until } => (after, until),
            };
            self.head_sweep = HeadSweep::Prefix { after, until };
            let head = step(after, self.config.child_limit, budget)?;
            let head_contended = matches!(head, ReclaimAttempt::Contended { .. });
            contended |= head_contended;
            let head = attempt_cursor_step(after, head)?;
            queued_since_pass |= head.drain.is_some_and(|drain| drain.enqueued_children > 0);
            result.queue_empty |= after.is_none() && head.examined.is_none();
            if head.examined.is_some() && !head_contended {
                result.applied = result
                    .applied
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            let partial = head.drain.is_some_and(|drain| drain.has_more);
            let reached_frontier =
                until.is_none_or(|until| head.examined.is_some_and(|examined| examined >= until));
            self.head_sweep = if !partial && (!head.has_more || reached_frontier) {
                HeadSweep::Idle
            } else {
                HeadSweep::Prefix {
                    after: head.resume_after,
                    until,
                }
            };
            result.steps = result
                .steps
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            result.examined = result
                .examined
                .checked_add(u32::from(head.examined.is_some()))
                .ok_or(IndexError::ResourceLimited)?;
            result.head_probed = true;
        } else {
            self.rounds_since_head = self
                .rounds_since_head
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        while result.steps < self.config.max_steps_per_round {
            let after = self.forward_after;
            if after.is_none() {
                self.pass = ForwardPass {
                    from_head: true,
                    ..ForwardPass::default()
                };
            }
            let attempt = step(after, self.config.child_limit, budget)?;
            let step_contended = matches!(attempt, ReclaimAttempt::Contended { .. });
            contended |= step_contended;
            self.pass.contended |= step_contended;
            let page = attempt_cursor_step(after, attempt)?;
            self.forward_saw_contention |= step_contended;
            result.steps = result
                .steps
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            result.examined = result
                .examined
                .checked_add(u32::from(page.examined.is_some()))
                .ok_or(IndexError::ResourceLimited)?;
            result.queue_empty |= after.is_none() && page.examined.is_none();
            if page.examined.is_some() && !step_contended {
                self.pass.applied = true;
                result.applied = result
                    .applied
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            queued_since_pass |= page.drain.is_some_and(|drain| drain.enqueued_children > 0);
            self.forward_after = if page.has_more {
                page.resume_after
            } else {
                None
            };
            if !page.has_more {
                self.forward_saw_contention = false;
                result.exhausted_forward = true;
                // A pass from the head that met only contended keys.
                result.stalled_pass |=
                    self.pass.from_head && self.pass.contended && !self.pass.applied;
                self.pass = ForwardPass::default();
                // Children queued during this pass may sort below keys it
                // already passed. Restart from the head so the round's step
                // budget reaches them. Every key examined this round has left
                // the queue unless it was contended, so the restart examines
                // only newly queued keys and keys this round never reached;
                // a contended key ends the round instead.
                if queued_since_pass && !contended {
                    queued_since_pass = false;
                    continue;
                }
                break;
            }
        }
        Ok(result)
    }
}

fn attempt_cursor_step(
    after: Option<ObjectRef>,
    attempt: ReclaimAttempt,
) -> Result<NativeReclaimStep, IndexError> {
    let step = match attempt {
        ReclaimAttempt::Applied(step) => step,
        ReclaimAttempt::Contended { examined, has_more } => NativeReclaimStep {
            examined: Some(examined),
            drain: None,
            resume_after: Some(examined),
            has_more,
        },
    };
    validate_step(after, &step)?;
    Ok(step)
}

fn validate_step(after: Option<ObjectRef>, page: &NativeReclaimStep) -> Result<(), IndexError> {
    if after
        .zip(page.examined)
        .is_some_and(|(previous, object)| object <= previous)
    {
        return Err(IndexError::BackendViolation(
            "reclaim work page did not advance in key order".into(),
        ));
    }
    match page.examined {
        None if page.has_more || page.drain.is_some() || page.resume_after != after => Err(
            IndexError::BackendViolation("empty reclaim step has an invalid cursor".into()),
        ),
        Some(_) if page.drain.is_some_and(|drain| drain.has_more) => {
            if page.has_more && page.resume_after == after {
                Ok(())
            } else {
                Err(IndexError::BackendViolation(
                    "partial reclaim drain changed its cursor".into(),
                ))
            }
        }
        Some(object) if page.resume_after != Some(object) => Err(IndexError::BackendViolation(
            "reclaim step did not advance past examined object".into(),
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use quanta_index_contract::index::Digest;
    use quanta_index_core::{RequestBudgetV1, index::WorkUsage};

    use super::*;

    macro_rules! check {
        ($condition:expr) => {{
            if $condition {
                Ok::<(), IndexError>(())
            } else {
                Err(IndexError::BackendViolation(format!(
                    "native reclaim check failed: {}",
                    stringify!($condition)
                )))
            }
        }};
    }

    macro_rules! check_eq {
        ($actual:expr, $expected:expr) => {{
            let actual = &$actual;
            let expected = &$expected;
            if actual == expected {
                Ok::<(), IndexError>(())
            } else {
                Err(IndexError::BackendViolation(format!(
                    "native reclaim check {} == {}: actual={actual:?}, expected={expected:?}",
                    stringify!($actual),
                    stringify!($expected)
                )))
            }
        }};
    }

    fn object(n: u8) -> ObjectRef {
        ObjectRef(Digest([n; 32]))
    }

    fn config() -> NativeReclaimConfig {
        NativeReclaimConfig {
            interval: Duration::from_millis(20),
            wal_reclaim: NativeWalReclaimPolicy {
                trigger_bytes: 8 * 1024 * 1024,
                min_interval: Duration::from_secs(1),
            },
            max_steps_per_round: 1,
            child_limit: 1,
            work_limits: WorkLimits {
                reads: STEP_READS,
                writes: STEP_WRITES,
                bytes: STEP_BYTES,
                mutations: STEP_MUTATIONS,
            },
            round_timeout: Duration::from_secs(5),
            head_probe_every: 3,
        }
    }

    fn budget() -> WorkBudget {
        WorkBudget::with_request(
            config().work_limits,
            RequestBudgetV1::for_duration(Duration::from_secs(5)),
        )
    }

    #[test]
    fn invalid_config_refuses_before_driver_construction() -> Result<(), IndexError> {
        let now = Instant::now();
        let mut invalid = config();
        invalid.max_steps_per_round = 0;
        check!(NativeReclaimDriver::new(invalid, now).is_err())?;
        invalid = config();
        invalid.work_limits.bytes = invalid.work_limits.bytes.saturating_sub(1);
        check!(NativeReclaimDriver::new(invalid, now).is_err())?;
        invalid = config();
        invalid.head_probe_every = 1;
        check!(NativeReclaimDriver::new(invalid, now).is_err())?;
        invalid = config();
        invalid.child_limit = MAX_RECLAIM_DRAIN_PAGE
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        check!(NativeReclaimDriver::new(invalid, now).is_err())?;
        Ok(())
    }

    #[test]
    fn forward_cursor_survives_head_probes_and_discovers_both_key_ranges() -> Result<(), IndexError>
    {
        let now = Instant::now();
        let mut driver = NativeReclaimDriver::new(config(), now)?;
        let mut queue = BTreeSet::from([2_u8, 3, 4, 5]);
        let mut seen = Vec::new();
        for tick in 0..5 {
            if tick == 1 {
                check!(queue.insert(1))?;
            }
            let when = driver.next_due();
            let round = driver.run_round(when, &mut budget(), |after, _, _| {
                seen.push(after);
                let lower = after
                    .and_then(|cursor| (cursor.0).0.first().copied())
                    .unwrap_or_default();
                let candidate = queue.iter().copied().find(|key| *key > lower);
                let Some(candidate) = candidate else {
                    return Ok(NativeReclaimStep {
                        examined: None,
                        drain: None,
                        resume_after: after,
                        has_more: false,
                    });
                };
                check!(queue.remove(&candidate))?;
                let has_more = queue.iter().any(|key| *key > candidate);
                Ok(NativeReclaimStep {
                    examined: Some(object(candidate)),
                    drain: None,
                    resume_after: Some(object(candidate)),
                    has_more,
                })
            })?;
            check_eq!(round.steps, 1)?;
        }
        check_eq!(
            seen,
            [
                None,
                Some(object(2)),
                None,
                Some(object(3)),
                Some(object(4))
            ]
        )?;
        check!(queue.is_empty())?;
        Ok(())
    }

    #[test]
    fn contended_first_key_cannot_starve_start_restart_or_exhausted_sweeps()
    -> Result<(), IndexError> {
        let mut options = config();
        options.head_probe_every = 2;
        let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
        let mut queue = std::collections::BTreeSet::from([object(1)]);
        let mut conflicts = 0_u32;
        for restart in [false, true] {
            if restart {
                driver = NativeReclaimDriver::new(options, Instant::now())?;
            }
            // A fresh/restarted traversal needs at most three rounds; an
            // exhausted traversal may first owe one additional head probe.
            for (next, rounds) in [(2, 3), (3, 4)] {
                let inserted = queue.insert(object(next));
                check!(inserted)?;
                for _round in 0..rounds {
                    let result = driver.run_attempt_round(
                        driver.next_due(),
                        &mut budget(),
                        |after, _, _| {
                            let examined = queue
                                .iter()
                                .copied()
                                .find(|candidate| {
                                    after.is_none_or(|previous| *candidate > previous)
                                })
                                .ok_or_else(|| {
                                    IndexError::BackendViolation("oracle queue is empty".into())
                                })?;
                            let has_more = queue.iter().any(|candidate| *candidate > examined);
                            if examined == object(1) {
                                conflicts = conflicts
                                    .checked_add(1)
                                    .ok_or(IndexError::ResourceLimited)?;
                                return Ok(ReclaimAttempt::Contended { examined, has_more });
                            }
                            check_eq!(examined, object(next))?;
                            check!(queue.remove(&examined))?;
                            Ok(ReclaimAttempt::Applied(NativeReclaimStep {
                                examined: Some(examined),
                                drain: None,
                                resume_after: Some(examined),
                                has_more,
                            }))
                        },
                    )?;
                    check_eq!(result.steps, 1)?;
                    check_eq!(result.examined, 1)?;
                }
                check_eq!(queue, std::collections::BTreeSet::from([object(1)]))?;
                check_eq!(driver.forward_after, None)?;
            }
        }
        check!(conflicts >= 4)?;
        Ok(())
    }

    #[test]
    fn contended_head_cannot_hide_lower_arrivals_while_forward_stays_busy() -> Result<(), IndexError>
    {
        let mut options = config();
        options.head_probe_every = 2;
        let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
        let mut queue = BTreeSet::from([object(1), object(50)]);
        queue.extend((51..=60).map(object));
        for round in 0..6 {
            if round == 3 {
                // Forward has reached 50, and the queue still has upper work.
                // A new lower key must be served without forward exhaustion.
                check!(queue.insert(object(2)))?;
            }
            let result =
                driver.run_attempt_round(driver.next_due(), &mut budget(), |after, _, _| {
                    let examined = queue
                        .iter()
                        .copied()
                        .find(|candidate| after.is_none_or(|previous| *candidate > previous))
                        .ok_or_else(|| {
                            IndexError::BackendViolation("oracle queue is empty".into())
                        })?;
                    let has_more = queue.iter().any(|candidate| *candidate > examined);
                    if examined == object(1) {
                        return Ok(ReclaimAttempt::Contended { examined, has_more });
                    }
                    check!(queue.remove(&examined))?;
                    Ok(ReclaimAttempt::Applied(NativeReclaimStep {
                        examined: Some(examined),
                        drain: None,
                        resume_after: Some(examined),
                        has_more,
                    }))
                })?;
            check_eq!(result.steps, 1)?;
            check!(!result.exhausted_forward)?;
        }
        check!(!queue.contains(&object(2)))?;
        check!(queue.contains(&object(1)))?;
        check!(queue.iter().any(|candidate| *candidate > object(50)))?;
        Ok(())
    }

    #[test]
    fn partial_head_drain_keeps_frozen_frontier_and_restarts_its_prefix() -> Result<(), IndexError>
    {
        let mut options = config();
        options.head_probe_every = 2;
        let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
        let mut queue =
            BTreeSet::from([object(10), object(20), object(30), object(40), object(41)]);
        let mut children = 2_u32;
        for round in 0..8 {
            match round {
                1 => check!(queue.insert(object(1)))?,
                4 => check!(queue.insert(object(11)))?,
                7 => check!(queue.insert(object(2)))?,
                _ => {}
            }
            let result =
                driver.run_attempt_round(driver.next_due(), &mut budget(), |after, _, _| {
                    let examined = queue
                        .iter()
                        .copied()
                        .find(|candidate| after.is_none_or(|previous| *candidate > previous))
                        .ok_or_else(|| {
                            IndexError::BackendViolation("oracle queue is empty".into())
                        })?;
                    let drain = if examined == object(1) {
                        children = children.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
                        Some(ReclaimDrainPage {
                            released_children: 1,
                            enqueued_children: 0,
                            has_more: children > 0,
                        })
                    } else {
                        None
                    };
                    let partial = drain.is_some_and(|page| page.has_more);
                    if !partial {
                        check!(queue.remove(&examined))?;
                    }
                    let has_more = partial || queue.iter().any(|candidate| *candidate > examined);
                    Ok(ReclaimAttempt::Applied(NativeReclaimStep {
                        examined: Some(examined),
                        drain,
                        resume_after: if partial { after } else { Some(examined) },
                        has_more,
                    }))
                })?;
            check_eq!(result.steps, 1)?;
            check!(!result.exhausted_forward)?;
        }
        // The first head cycle drains 1 and crosses its frozen frontier 10
        // at 11. It must then restart to serve the new lower key 2, even
        // though forward has advanced and upper key 41 remains queued.
        check_eq!(children, 0)?;
        check!(!queue.contains(&object(2)))?;
        check_eq!(queue, BTreeSet::from([object(41)]))?;
        Ok(())
    }

    #[test]
    fn contended_key_preserves_manual_conflict_and_rejects_nonadvancing_cursor()
    -> Result<(), IndexError> {
        let manual = ReclaimAttempt::Contended {
            examined: object(1),
            has_more: true,
        }
        .into_manual_step();
        check_eq!(manual, Err(IndexError::Conflict))?;
        let unordered = attempt_cursor_step(
            Some(object(2)),
            ReclaimAttempt::Contended {
                examined: object(2),
                has_more: true,
            },
        );
        check!(matches!(unordered, Err(IndexError::BackendViolation(_))))?;
        Ok(())
    }

    #[test]
    fn conflicting_head_probe_cannot_starve_forward_reclaim() -> Result<(), IndexError> {
        let mut options = config();
        options.head_probe_every = 2;
        let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
        let _first = driver.run_round(driver.next_due(), &mut budget(), |after, _, _| {
            check_eq!(after, None)?;
            Ok(NativeReclaimStep {
                examined: Some(object(10)),
                drain: None,
                resume_after: Some(object(10)),
                has_more: true,
            })
        })?;
        for (previous, next) in [(10, 11), (11, 12)] {
            let failed_head = driver.run_round(driver.next_due(), &mut budget(), |after, _, _| {
                check_eq!(after, None)?;
                Err(IndexError::Conflict)
            });
            check_eq!(failed_head, Err(IndexError::Conflict))?;
            // Even if every head attempt conflicts, the next cadence slot
            // must retain the independent successful forward cursor.
            let forward = driver.run_round(driver.next_due(), &mut budget(), |after, _, _| {
                check_eq!(after, Some(object(previous)))?;
                Ok(NativeReclaimStep {
                    examined: Some(object(next)),
                    drain: None,
                    resume_after: Some(object(next)),
                    has_more: true,
                })
            })?;
            check_eq!(forward.steps, 1)?;
            check!(!forward.head_probed)?;
        }
        Ok(())
    }

    #[test]
    fn failure_and_scheduler_denial_preserve_cursor_and_respect_cadence() -> Result<(), IndexError>
    {
        let now = Instant::now();
        let mut options = config();
        options.head_probe_every = 4;
        let mut driver = NativeReclaimDriver::new(options, now)?;
        let first = driver.run_round(now, &mut budget(), |after, _, _| {
            check_eq!(after, None)?;
            Ok(NativeReclaimStep {
                examined: Some(object(1)),
                drain: None,
                resume_after: Some(object(1)),
                has_more: true,
            })
        })?;
        check_eq!(first.steps, 1)?;
        let due_after_first = driver.next_due();
        let mut early_calls: u32 = 0;
        let early = driver.run_round(now, &mut budget(), |_, _, _| {
            early_calls = early_calls
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            Err(IndexError::BackendViolation(
                "step called before due".into(),
            ))
        })?;
        check_eq!(early.steps, 0)?;
        check_eq!(early_calls, 0)?;
        driver.defer(due_after_first)?;
        let due_after_denial = driver.next_due();
        check!(due_after_denial > due_after_first)?;
        let error = driver.run_round(due_after_denial, &mut budget(), |after, _, _| {
            check_eq!(after, Some(object(1)))?;
            Err(IndexError::ResourceLimited)
        });
        check!(matches!(error, Err(IndexError::ResourceLimited)))?;
        let due_after_error = driver.next_due();
        check!(due_after_error > due_after_denial)?;
        let _resumed = driver.run_round(due_after_error, &mut budget(), |after, _, _| {
            check_eq!(after, Some(object(1)))?;
            Ok(NativeReclaimStep {
                examined: Some(object(2)),
                drain: None,
                resume_after: Some(object(2)),
                has_more: false,
            })
        })?;
        Ok(())
    }

    #[test]
    fn equal_candidate_refuses_without_advancing_forward_cursor() -> Result<(), IndexError> {
        let now = Instant::now();
        let mut options = config();
        options.head_probe_every = 4;
        let mut driver = NativeReclaimDriver::new(options, now)?;
        let first = driver.run_round(now, &mut budget(), |after, _, _| {
            check_eq!(after, None)?;
            Ok(NativeReclaimStep {
                examined: Some(object(2)),
                drain: None,
                resume_after: Some(object(2)),
                has_more: true,
            })
        });
        check!(first.is_ok())?;
        let equal_due = driver.next_due();
        let equal = driver.run_round(equal_due, &mut budget(), |after, _, _| {
            check_eq!(after, Some(object(2)))?;
            Ok(NativeReclaimStep {
                examined: Some(object(2)),
                drain: None,
                resume_after: Some(object(2)),
                has_more: true,
            })
        });
        check!(matches!(equal, Err(IndexError::BackendViolation(_))))?;
        let later_due = driver.next_due();
        let later = driver.run_round(later_due, &mut budget(), |after, _, _| {
            check_eq!(after, Some(object(2)))?;
            Ok(NativeReclaimStep {
                examined: Some(object(3)),
                drain: None,
                resume_after: Some(object(3)),
                has_more: false,
            })
        });
        check!(later.is_ok())?;
        Ok(())
    }

    #[test]
    fn two_steps_share_the_original_round_budget() -> Result<(), IndexError> {
        let now = Instant::now();
        let mut options = config();
        options.max_steps_per_round = 2;
        options.work_limits = WorkLimits {
            reads: STEP_READS
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?,
            writes: STEP_WRITES
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?,
            bytes: STEP_BYTES
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?,
            mutations: STEP_MUTATIONS
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?,
        };
        let mut driver = NativeReclaimDriver::new(options, now)?;
        let mut budget = WorkBudget::with_request(
            options.work_limits,
            RequestBudgetV1::for_duration(options.round_timeout),
        );
        let mut before = Vec::new();
        let round = driver.run_round(now, &mut budget, |after, _, work| {
            before.push(work.usage().reads);
            work.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let next = match after {
                None => object(1),
                Some(previous) if previous == object(1) => object(2),
                _ => return Err(IndexError::BackendViolation("unexpected cursor".into())),
            };
            Ok(NativeReclaimStep {
                examined: Some(next),
                drain: None,
                resume_after: Some(next),
                has_more: next == object(1),
            })
        })?;
        check_eq!(round.steps, 2)?;
        check_eq!(before, [0, 1])?;
        check_eq!(budget.usage().reads, 2)?;
        Ok(())
    }

    /// Demand from committed abandonment and retirement steers the next round.
    ///
    /// Read recovery becomes due, and the forward sweep restarts at the queue
    /// head instead of continuing from its cursor; no head probe is spent.
    #[test]
    fn maintenance_demand_forces_read_recovery_and_head_restart_next_round()
    -> Result<(), IndexError> {
        let options = NativeReclaimConfig {
            head_probe_every: 64,
            ..config()
        };
        let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
        driver.record_read_recovery(ReadSessionRecoveryPage {
            after: None,
            has_more: false,
            recovered: 0,
            pins_released: 0,
            open_intents_reaped: 0,
            abandoned_settled: 0,
            abandoned_deferred: 0,
        });
        let mut afters = Vec::new();
        for _ in 0..2 {
            let mut budget = WorkBudget::with_request(
                options.work_limits,
                RequestBudgetV1::for_duration(options.round_timeout),
            );
            let _round = driver.run_round(driver.next_due(), &mut budget, |after, _, _| {
                afters.push(after);
                let next = object(after.map_or(1, |previous| previous.0.0[0].saturating_add(1)));
                Ok(NativeReclaimStep {
                    examined: Some(next),
                    drain: None,
                    resume_after: Some(next),
                    has_more: true,
                })
            })?;
        }
        check_eq!(afters, [None, Some(object(1))])?;
        check!(!driver.read_recovery_due())?;

        driver.schedule(&quanta_index_catalog::MaintenanceDemand {
            read_recovery: true,
            reclaim: true,
            ..quanta_index_catalog::MaintenanceDemand::default()
        });
        check!(driver.read_recovery_due())?;
        let mut budget = WorkBudget::with_request(
            options.work_limits,
            RequestBudgetV1::for_duration(options.round_timeout),
        );
        let round = driver.run_round(driver.next_due(), &mut budget, |after, _, _| {
            afters.push(after);
            Ok(NativeReclaimStep {
                examined: Some(object(9)),
                drain: None,
                resume_after: Some(object(9)),
                has_more: false,
            })
        })?;
        check!(!round.head_probed)?;
        check_eq!(afters.last(), Some(&None))?;
        Ok(())
    }

    /// Key-ordered model of the Catalog reclaim queue.
    ///
    /// A step reclaims the first queued key after the cursor and queues the
    /// children `graph` gives it; `demand` stands for the reclaim demand the
    /// Catalog records after that commit. Keys in `contended` report custody
    /// contention and stay queued. `examined` logs every key a step examined.
    #[derive(Default)]
    struct ModelQueue {
        queue: BTreeSet<ObjectRef>,
        graph: std::collections::BTreeMap<ObjectRef, Vec<ObjectRef>>,
        contended: BTreeSet<ObjectRef>,
        /// Keys contended on their next attempt only.
        contend_once: BTreeSet<ObjectRef>,
        examined: Vec<ObjectRef>,
        demand: bool,
    }

    impl ModelQueue {
        fn attempt(&mut self, after: Option<ObjectRef>) -> Result<ReclaimAttempt, IndexError> {
            let next = match after {
                None => self.queue.first().copied(),
                Some(after) => self
                    .queue
                    .range((std::ops::Bound::Excluded(after), std::ops::Bound::Unbounded))
                    .next()
                    .copied(),
            };
            let Some(key) = next else {
                return Ok(ReclaimAttempt::Applied(NativeReclaimStep {
                    examined: None,
                    drain: None,
                    resume_after: after,
                    has_more: false,
                }));
            };
            self.examined.push(key);
            // The work page reports later keys before this drain commits.
            let has_more = self
                .queue
                .range((std::ops::Bound::Excluded(key), std::ops::Bound::Unbounded))
                .next()
                .is_some();
            if self.contended.contains(&key) || self.contend_once.remove(&key) {
                return Ok(ReclaimAttempt::Contended {
                    examined: key,
                    has_more,
                });
            }
            let _removed = self.queue.remove(&key);
            let children: Vec<ObjectRef> = self.graph.remove(&key).into_iter().flatten().collect();
            let mut enqueued = 0_u32;
            for child in &children {
                if self.queue.insert(*child) {
                    enqueued = enqueued.saturating_add(1);
                }
            }
            if enqueued > 0 {
                self.demand = true;
            }
            Ok(ReclaimAttempt::Applied(NativeReclaimStep {
                examined: Some(key),
                drain: Some(ReclaimDrainPage {
                    released_children: u32::try_from(children.len())
                        .map_err(|_overflow| IndexError::ResourceLimited)?,
                    enqueued_children: enqueued,
                    has_more: false,
                }),
                resume_after: Some(key),
                has_more,
            }))
        }

        /// One pump iteration: hand committed demand to the driver, then run
        /// one reclaim round due now.
        fn round(
            &mut self,
            driver: &mut NativeReclaimDriver,
            options: NativeReclaimConfig,
        ) -> Result<NativeReclaimRound, IndexError> {
            if std::mem::take(&mut self.demand) {
                driver.schedule(&quanta_index_catalog::MaintenanceDemand {
                    reclaim: true,
                    ..quanta_index_catalog::MaintenanceDemand::default()
                });
            }
            let mut budget = WorkBudget::with_request(
                options.work_limits,
                RequestBudgetV1::for_duration(options.round_timeout),
            );
            driver.run_attempt_round(driver.next_due(), &mut budget, |after, _, _| {
                self.attempt(after)
            })
        }
    }

    fn four_step_options() -> NativeReclaimConfig {
        NativeReclaimConfig {
            max_steps_per_round: 4,
            child_limit: 32,
            head_probe_every: 64,
            work_limits: WorkLimits {
                reads: STEP_READS.saturating_mul(4),
                writes: STEP_WRITES.saturating_mul(4),
                bytes: STEP_BYTES.saturating_mul(4),
                mutations: STEP_MUTATIONS.saturating_mul(4),
            },
            ..config()
        }
    }

    #[test]
    fn reclaim_demand_cannot_rewind_past_a_contended_prefix_every_round() -> Result<(), IndexError>
    {
        let options = four_step_options();
        let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
        let blocked: BTreeSet<_> = (1..=4).map(object).collect();
        let mut model = ModelQueue {
            queue: blocked.clone(),
            contended: blocked.clone(),
            ..ModelQueue::default()
        };
        let _queued = model.queue.insert(object(9));
        model.demand = true;
        let first = model.round(&mut driver, options)?;
        check_eq!(first.steps, options.max_steps_per_round)?;
        check_eq!(model.examined, [object(1), object(2), object(3), object(4)])?;
        check!(model.queue.contains(&object(9)))?;

        // Another retirement queues a key behind the cursor. Its hint must
        // leave the remaining forward work reachable, then allow a restart.
        let _queued = model.queue.insert(object(0));
        model.demand = true;
        let second = model.round(&mut driver, options)?;
        check_eq!(second.steps, 1)?;
        check!(!model.queue.contains(&object(9)))?;
        check!(model.queue.contains(&object(0)))?;
        model.demand = true;
        let _round = model.round(&mut driver, options)?;
        check_eq!(model.queue, blocked)?;
        Ok(())
    }

    /// Freed children below the forward cursor drain in step-bounded rounds.
    ///
    /// A retired view's root frees ten children whose keys sort below the
    /// forward cursor while new work keeps arriving above it (five keys per
    /// round, four steps per round, so the forward sweep never reaches the
    /// queue end). The reclaim demand from that drain restarts the sweep at
    /// the head, so the children drain in ceil(10 / 4) = 3 consecutive rounds
    /// after the retirement round; the periodic head probe (every 64 rounds)
    /// never comes due.
    #[test]
    fn freed_children_below_the_cursor_drain_in_step_bounded_rounds() -> Result<(), IndexError> {
        let options = four_step_options();
        let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
        let children: Vec<ObjectRef> = (10..20).map(object).collect();
        let mut model = ModelQueue::default();
        let _root = model.graph.insert(object(100), children.clone());
        let mut arrivals = 101_u8..=255;
        let mut round =
            |driver: &mut NativeReclaimDriver, model: &mut ModelQueue| -> Result<(), IndexError> {
                // Other work keeps arriving above the cursor, one round's worth
                // at a time, so the forward sweep never reaches the queue end.
                for key in arrivals.by_ref().take(5) {
                    let _queued = model.queue.insert(object(key));
                }
                let _round = model.round(driver, options)?;
                Ok(())
            };
        let remaining = |model: &ModelQueue| {
            children
                .iter()
                .filter(|child| model.queue.contains(child))
                .count()
        };

        // Two rounds of unrelated work move the forward cursor to key 108.
        round(&mut driver, &mut model)?;
        round(&mut driver, &mut model)?;
        check_eq!(driver.forward_after, Some(object(108)))?;

        // Retirement queues the view root below the cursor and records demand.
        // The sweep restarts at the head, reclaims the root, and moves on
        // above it (109, 110, 111), leaving every freed child behind it.
        let _root = model.queue.insert(object(100));
        model.demand = true;
        round(&mut driver, &mut model)?;
        check!(!model.queue.contains(&object(100)))?;
        check_eq!(remaining(&model), 10)?;
        check_eq!(driver.forward_after, Some(object(111)))?;

        let mut drained = Vec::new();
        for _ in 0..3 {
            round(&mut driver, &mut model)?;
            drained.push(remaining(&model));
        }
        check_eq!(drained, [6, 2, 0])?;
        Ok(())
    }

    /// A multi-level cascade drains in exactly ceil(S / steps) rounds.
    ///
    /// Root 100 frees children 10..=14; child 10 frees 1, 2, 3 and child 11
    /// frees 4, 5. That is S = 11 steps, so four steps per round need exactly
    /// ceil(11 / 4) = 3 rounds. Each level's children sort below the cursor,
    /// and the root's round reaches the queue end after one step; the sweep
    /// restarts from the head within the round instead of ending it early.
    #[test]
    fn multi_level_cascade_drains_in_exactly_ceil_steps_rounds() -> Result<(), IndexError> {
        let options = four_step_options();
        let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
        let mut model = ModelQueue::default();
        let _root = model
            .graph
            .insert(object(100), (10..=14).map(object).collect());
        let _first = model
            .graph
            .insert(object(10), (1..=3).map(object).collect());
        let _second = model
            .graph
            .insert(object(11), (4..=5).map(object).collect());
        let _queued = model.queue.insert(object(100));
        model.demand = true;
        let mut queued_after_round = Vec::new();
        for _ in 0..3 {
            let _round = model.round(&mut driver, options)?;
            queued_after_round.push(model.queue.len());
        }
        // Round 1: 100, restart, 10, 11, 12. Round 2: 1, 2, 3, 4.
        // Round 3: 5, 13, 14.
        check_eq!(queued_after_round, [7, 3, 0])?;
        check_eq!(model.examined.len(), 11)?;
        Ok(())
    }

    /// The in-round restart never re-examines a key this round examined.
    ///
    /// Key 5 is contended and stays queued; root 100 then frees 1 and 2 below
    /// the cursor and the sweep reaches the queue end. Restarting would
    /// examine key 5 a second time this round, so the round ends instead and
    /// the next round (driven by the drain's demand) starts at the head.
    #[test]
    fn in_round_restart_does_not_reexamine_contended_keys() -> Result<(), IndexError> {
        let options = four_step_options();
        let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
        let mut model = ModelQueue::default();
        let _root = model.graph.insert(object(100), vec![object(1), object(2)]);
        let _queued = model.queue.insert(object(5));
        let _queued = model.queue.insert(object(100));
        let _contended = model.contended.insert(object(5));
        model.demand = true;
        let round = model.round(&mut driver, options)?;
        check_eq!(model.examined, [object(5), object(100)])?;
        check_eq!(round.steps, 2)?;
        let _round = model.round(&mut driver, options)?;
        check_eq!(
            model.examined,
            [object(5), object(100), object(1), object(2), object(5)]
        )?;
        Ok(())
    }

    /// Run model rounds and record each one, as the pump does.
    fn observed_rounds(
        model: &mut ModelQueue,
        driver: &mut NativeReclaimDriver,
        observation: &crate::native_reclaim_observation::NativeReclaimObservation,
        rounds: u32,
    ) -> Result<(), IndexError> {
        for _ in 0..rounds {
            let round = model.round(driver, four_step_options())?;
            observation.record_round(&round);
        }
        Ok(())
    }

    /// A key held contended indefinitely ends the wait with `Stalled`.
    ///
    /// Each round sweeps the queue from its head to its end and meets only the
    /// contended key, so each round completes one stalled pass. One stalled
    /// pass may be a single contention episode; the second is a stall.
    #[test]
    fn indefinitely_contended_key_reports_stalled_instead_of_waiting() -> Result<(), IndexError> {
        use crate::native_progress_observation::NativeProgressWaitError;
        use crate::native_reclaim_observation::NativeReclaimObservation;
        let mut driver = NativeReclaimDriver::new(four_step_options(), Instant::now())?;
        let mut model = ModelQueue::default();
        let _queued = model.queue.insert(object(5));
        let _contended = model.contended.insert(object(5));
        let observation = NativeReclaimObservation::default();
        let mark = observation.mark();
        observed_rounds(&mut model, &mut driver, &observation, 1)?;
        check_eq!(observation.outcome_after(mark), None)?;
        observed_rounds(&mut model, &mut driver, &observation, 1)?;
        let stalled = Err(NativeProgressWaitError::Stalled { rounds: 2 });
        check_eq!(observation.outcome_after(mark), Some(stalled))?;
        // The wait returns the stall instead of blocking for a drained round.
        check_eq!(observation.wait_drained_after(mark), stalled)?;
        Ok(())
    }

    /// A single contention episode and a normal drain still end drained.
    ///
    /// Key 5 is contended on its first attempt only, so round 1 is one
    /// stalled pass. Round 2 reclaims 5, which frees 1 and 2; the in-round
    /// restart reclaims them too. Rounds 3 and 4 find the queue empty.
    #[test]
    fn single_contention_episode_and_normal_drain_report_drained() -> Result<(), IndexError> {
        use crate::native_reclaim_observation::NativeReclaimObservation;
        let mut driver = NativeReclaimDriver::new(four_step_options(), Instant::now())?;
        let mut model = ModelQueue::default();
        let _queued = model.queue.insert(object(5));
        let _once = model.contend_once.insert(object(5));
        let _children = model.graph.insert(object(5), vec![object(1), object(2)]);
        let observation = NativeReclaimObservation::default();
        let mark = observation.mark();
        observed_rounds(&mut model, &mut driver, &observation, 2)?;
        check!(model.queue.is_empty())?;
        check_eq!(observation.outcome_after(mark), None)?;
        observed_rounds(&mut model, &mut driver, &observation, 2)?;
        check_eq!(observation.outcome_after(mark), Some(Ok(())))?;
        check_eq!(observation.wait_drained_after(mark), Ok(()))?;
        Ok(())
    }

    /// `SplitMix64`: a fixed-seed generator for the property tests below.
    struct SplitMix(u64);

    impl SplitMix {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut mixed = self.0;
            mixed = (mixed ^ mixed.wrapping_shr(30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            mixed = (mixed ^ mixed.wrapping_shr(27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            mixed ^ mixed.wrapping_shr(31)
        }

        /// A value in `0..bound`; a zero `bound` yields zero.
        fn below(&mut self, bound: u64) -> u64 {
            let Some(value) = self.next().checked_rem(bound) else {
                return 0;
            };
            value
        }
    }

    fn steps_options(steps: u32) -> NativeReclaimConfig {
        let steps_u64 = u64::from(steps);
        NativeReclaimConfig {
            max_steps_per_round: steps,
            child_limit: 32,
            head_probe_every: 1024,
            work_limits: WorkLimits {
                reads: STEP_READS.saturating_mul(steps_u64),
                writes: STEP_WRITES.saturating_mul(steps_u64),
                bytes: STEP_BYTES.saturating_mul(steps_u64),
                mutations: STEP_MUTATIONS.saturating_mul(steps_u64),
            },
            ..config()
        }
    }

    /// `count` distinct keys drawn from `low..=high` in random order.
    fn distinct_keys(random: &mut SplitMix, low: u8, high: u8, count: u64) -> Vec<ObjectRef> {
        let mut pool: Vec<u8> = (low..=high).collect();
        let mut keys = Vec::new();
        for _ in 0..count {
            let Ok(len) = u64::try_from(pool.len()) else {
                break;
            };
            if len == 0 {
                break;
            }
            let Ok(index) = usize::try_from(random.below(len)) else {
                break;
            };
            keys.push(object(pool.swap_remove(index)));
        }
        keys
    }

    /// Random cascades drain in exactly ceil(S / steps) rounds.
    ///
    /// Each seed builds a forest of S objects at random distinct keys: some
    /// queued roots, every other object freed by an earlier one, so children
    /// sort anywhere relative to the cursor. Nothing is contended, so each
    /// step reclaims one entry and every round but the last uses all steps.
    #[test]
    fn random_cascades_drain_in_exactly_ceil_steps_rounds() -> Result<(), IndexError> {
        for seed in 1..=64_u64 {
            let mut random = SplitMix(seed);
            let steps = u32::try_from(random.below(8))
                .map_err(|_overflow| IndexError::ResourceLimited)?
                .saturating_add(1);
            let options = steps_options(steps);
            let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
            let mut model = ModelQueue::default();
            let objects = random.below(60).saturating_add(1);
            let keys = distinct_keys(&mut random, 1, 250, objects);
            for (ordinal, key) in keys.iter().enumerate() {
                let parent = match u64::try_from(ordinal) {
                    Ok(earlier) if earlier > 0 && random.below(3) != 0 => {
                        match usize::try_from(random.below(earlier)) {
                            Ok(index) => keys.get(index),
                            Err(_overflow) => None,
                        }
                    }
                    Ok(_) | Err(_) => None,
                };
                match parent {
                    Some(parent) => model.graph.entry(*parent).or_default().push(*key),
                    None => {
                        let _queued = model.queue.insert(*key);
                    }
                }
            }
            model.demand = true;
            let expected = objects.div_ceil(u64::from(steps));
            let mut rounds = 0_u64;
            while !model.queue.is_empty() && rounds <= expected {
                let _round = model.round(&mut driver, options)?;
                rounds = rounds.saturating_add(1);
            }
            check_eq!(
                (seed, steps, objects, rounds),
                (seed, steps, objects, expected)
            )?;
            let examined = u64::try_from(model.examined.len())
                .map_err(|_overflow| IndexError::ResourceLimited)?;
            check_eq!((seed, examined), (seed, objects))?;
        }
        Ok(())
    }

    /// Random contention schedules against the stall rule.
    ///
    /// Keys are permanently contended, contended once, or plain; some free
    /// children when reclaimed, and new keys arrive for a few rounds. `Stalled` may only appear while a permanently
    /// contended key is queued, a round never examines a key twice, and the
    /// wait ends: drained exactly when no stuck key was ever queued.
    #[test]
    fn random_contention_reports_stalled_only_for_stuck_keys() -> Result<(), IndexError> {
        use crate::native_progress_observation::NativeProgressWaitError;
        use crate::native_reclaim_observation::NativeReclaimObservation;
        for seed in 1..=64_u64 {
            let mut random = SplitMix(seed.wrapping_mul(7919));
            let steps = u32::try_from(random.below(8))
                .map_err(|_overflow| IndexError::ResourceLimited)?
                .saturating_add(1);
            let options = steps_options(steps);
            let mut driver = NativeReclaimDriver::new(options, Instant::now())?;
            let mut model = ModelQueue::default();
            let place = |model: &mut ModelQueue, random: &mut SplitMix, key: ObjectRef| {
                match random.below(10) {
                    0 => {
                        let _stuck = model.contended.insert(key);
                    }
                    1..=3 => {
                        let _once = model.contend_once.insert(key);
                    }
                    _ => {}
                }
                let _queued = model.queue.insert(key);
            };
            let initial = random.below(12).saturating_add(1);
            let mut children = distinct_keys(&mut random, 101, 180, 80).into_iter();
            for key in distinct_keys(&mut random, 1, 100, initial) {
                place(&mut model, &mut random, key);
                // Freed children (some below the cursor) force in-round
                // restarts, so the fairness check meets contended keys.
                if random.below(3) == 0 {
                    for _ in 0..random.below(3).saturating_add(1) {
                        if let Some(child) = children.next() {
                            if random.below(4) == 0 {
                                let _once = model.contend_once.insert(child);
                            }
                            model.graph.entry(key).or_default().push(child);
                        }
                    }
                }
            }
            let mut arrivals = distinct_keys(&mut random, 181, 250, 24).into_iter();
            let arrival_rounds = random.below(7);
            let observation = NativeReclaimObservation::default();
            let mark = observation.mark();
            let mut verdict = None;
            for round in 0..400_u64 {
                if round < arrival_rounds {
                    for _ in 0..random.below(3) {
                        if let Some(key) = arrivals.next() {
                            place(&mut model, &mut random, key);
                        }
                    }
                }
                let start = model.examined.len();
                let recorded = model.round(&mut driver, options)?;
                observation.record_round(&recorded);
                let Some(this_round) = model.examined.get(start..) else {
                    return Err(IndexError::BackendViolation("examined log shrank".into()));
                };
                let unique: BTreeSet<&ObjectRef> = this_round.iter().collect();
                check_eq!((seed, round, unique.len()), (seed, round, this_round.len()))?;
                let stuck_queued = model.queue.iter().any(|key| model.contended.contains(key));
                match observation.outcome_after(mark) {
                    None => {}
                    Some(Err(NativeProgressWaitError::Stalled { .. })) => {
                        check!(stuck_queued)?;
                        verdict = Some(false);
                        break;
                    }
                    Some(Ok(())) => {
                        check!(!stuck_queued)?;
                        verdict = Some(true);
                        break;
                    }
                    Some(Err(NativeProgressWaitError::Stopped)) => {
                        return Err(IndexError::BackendViolation("model pump stopped".into()));
                    }
                }
            }
            let stuck_ever = !model.contended.is_empty();
            check_eq!((seed, verdict), (seed, Some(!stuck_ever)))?;
        }
        Ok(())
    }
}
