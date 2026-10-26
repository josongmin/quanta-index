//! Bounded automatic reclaim traversal for the service-owned maintenance pump.

use std::time::{Duration, Instant};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    IndexError, MAX_RECLAIM_DRAIN_PAGE, ObjectRef, ReadSessionRecoveryPage, ReadSessionRef,
    ReclaimDrainPage, WorkBudget, WorkLimits,
    ports::{CatalogReclaimPort, ManagedObjectReclaimPort},
};
use quanta_index_storage::FileObjectStore;

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
    head_sweep: HeadSweep,
    rounds_since_head: u32,
    read_recovery_after: Option<ReadSessionRef>,
    read_recovery_pending: bool,
    next_due: Instant,
}

impl NativeReclaimDriver {
    pub(crate) fn new(config: NativeReclaimConfig, now: Instant) -> Result<Self, IndexError> {
        config.validate()?;
        Ok(Self {
            config,
            forward_after: None,
            head_sweep: HeadSweep::Idle,
            rounds_since_head: 0,
            read_recovery_after: None,
            read_recovery_pending: true,
            next_due: now,
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
            let head = attempt_cursor_step(after, head)?;
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
            let page = attempt_cursor_step(after, step(after, self.config.child_limit, budget)?)?;
            result.steps = result
                .steps
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            result.examined = result
                .examined
                .checked_add(u32::from(page.examined.is_some()))
                .ok_or(IndexError::ResourceLimited)?;
            self.forward_after = if page.has_more {
                page.resume_after
            } else {
                None
            };
            if !page.has_more {
                result.exhausted_forward = true;
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
}
