//! Bounded readiness and failure observations through the public work API.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write as _;

use super::*;
use quanta_index_contract::index::{OwnerInputBinding, WorkIntent, WorkScope, WorkStatus};

const COMPLETION_STATUS_PAGE: usize = 64;
const MAX_COMPLETION_SOURCES: usize = 4_096;
const INITIAL_READINESS_PAUSE: Duration = Duration::from_millis(500);
const MAX_READINESS_PAUSE: Duration = Duration::from_secs(1);

/// Test-harness polling cadence; completion receipts remain the only readiness authority.
pub(super) struct ReadinessPace {
    next: Duration,
}

impl ReadinessPace {
    pub(super) fn new() -> Self {
        Self {
            next: INITIAL_READINESS_PAUSE,
        }
    }

    pub(super) fn after_page(&mut self, advanced: bool, remaining: Duration) -> Duration {
        if advanced {
            self.next = INITIAL_READINESS_PAUSE;
        }
        let pause = self.next.min(remaining);
        if !advanced {
            self.next = self.next.saturating_mul(2).min(MAX_READINESS_PAUSE);
        }
        pause
    }
}

/// Test-side cost of the existing read-only readiness calls. The bounded
/// samples share the caller's progress cadence and never change work funding.
pub(super) struct ReadinessCost {
    phase: &'static str,
    started: Instant,
    reads: u64,
    rpc_wall: Duration,
    sampled_reads: u64,
    sampled_wall: Duration,
    completed: usize,
    samples: VecDeque<(u64, usize, u64, u128)>,
    succeeded: bool,
}

impl ReadinessCost {
    pub(super) fn new(phase: &'static str) -> Self {
        Self {
            phase,
            started: Instant::now(),
            reads: 0,
            rpc_wall: Duration::ZERO,
            sampled_reads: 0,
            sampled_wall: Duration::ZERO,
            completed: 0,
            samples: VecDeque::with_capacity(8),
            succeeded: false,
        }
    }

    pub(super) fn record_rpc(&mut self, elapsed: Duration) {
        self.reads = self.reads.saturating_add(1);
        self.rpc_wall = self.rpc_wall.saturating_add(elapsed);
    }

    pub(super) fn observe_completed(&mut self, completed: usize) {
        self.completed = completed;
    }

    pub(super) fn sample(&mut self, elapsed: Duration) {
        if self.samples.len() == 8 {
            let _oldest = self.samples.pop_front();
        }
        self.samples.push_back((
            elapsed.as_secs(),
            self.completed,
            self.reads.saturating_sub(self.sampled_reads),
            self.rpc_wall.saturating_sub(self.sampled_wall).as_millis(),
        ));
        self.sampled_reads = self.reads;
        self.sampled_wall = self.rpc_wall;
    }

    pub(super) fn mark_success(&mut self) {
        self.succeeded = true;
    }

    fn report(&self, elapsed: Duration) -> String {
        let tail = (
            elapsed.as_secs(),
            self.completed,
            self.reads.saturating_sub(self.sampled_reads),
            self.rpc_wall.saturating_sub(self.sampled_wall).as_millis(),
        );
        format!(
            "native-ann-work-status phase={:?} success={} reads={} rpc_wall_ms={} elapsed_ms={} samples=(elapsed_s,completed,reads,rpc_wall_ms){:?} tail={tail:?}",
            self.phase,
            self.succeeded,
            self.reads,
            self.rpc_wall.as_millis(),
            elapsed.as_millis(),
            self.samples,
        )
    }
}

impl Drop for ReadinessCost {
    fn drop(&mut self) {
        // Observation-only output also runs on early errors. The original
        // error, deadline and daemon cleanup retain authority if stderr fails.
        let _written = writeln!(
            std::io::stderr().lock(),
            "{}",
            self.report(self.started.elapsed())
        );
    }
}

fn owner_intent(
    workspace: WorkspaceKey,
    epoch: u64,
    spec: &InstanceSpec,
    owner: OwnerKey,
    source: ObjectRef,
) -> WorkIntent {
    WorkIntent {
        workspace,
        epoch,
        instance: spec.instance,
        scope: WorkScope::Owner {
            owner,
            expected: OwnerInputBinding {
                source: Some(source),
                projection: None,
                producer_context: Digest([0; 32]),
                derivation_policy: spec.policy,
            },
        },
    }
}

/// Read-only readiness cursor over newly staged owner/input bindings.
///
/// A round-robin bitmap keeps completed receipts while the pump finishes work
/// in a different order from source ownership. The caller separately checks
/// the final selected view and full coverage.
pub(crate) struct CompletionProgress<'a> {
    sources: &'a [(OwnerKey, ObjectRef)],
    source_view: ViewRef,
    spec: &'a InstanceSpec,
    next: usize,
    completed: Vec<bool>,
    completed_count: usize,
}

impl<'a> CompletionProgress<'a> {
    pub(crate) fn new(
        sources: &'a [(OwnerKey, ObjectRef)],
        source_view: ViewRef,
        spec: &'a InstanceSpec,
    ) -> TestResult<Self> {
        require(
            sources.len() <= MAX_COMPLETION_SOURCES,
            "ANN status tracker exceeds fixed corpus bound",
        )?;
        let mut completed = Vec::new();
        completed.try_reserve_exact(sources.len())?;
        completed.resize(sources.len(), false);
        Ok(Self {
            sources,
            source_view,
            spec,
            next: 0,
            completed,
            completed_count: 0,
        })
    }

    pub(crate) fn completed(&self) -> usize {
        self.completed_count
    }

    pub(crate) fn total(&self) -> usize {
        self.sources.len()
    }

    pub(crate) fn instance(&self) -> IndexInstanceId {
        self.spec.instance
    }

    pub(crate) fn poll_next(
        &mut self,
        mut read: impl FnMut(WorkIntent) -> TestResult<WorkStatus>,
    ) -> TestResult<bool> {
        let mut visited = 0_usize;
        let mut reads = 0_usize;
        while visited < self.sources.len() && reads < COMPLETION_STATUS_PAGE {
            if self
                .completed
                .get(self.next)
                .copied()
                .ok_or("ANN status cursor outside bitmap")?
            {
                self.advance_cursor()?;
                visited = visited.checked_add(1).ok_or("ANN status visit overflow")?;
                continue;
            }
            let (owner, source) = self
                .sources
                .get(self.next)
                .copied()
                .ok_or("ANN status cursor outside sources")?;
            let intent = owner_intent(
                self.source_view.workspace,
                self.source_view.epoch,
                self.spec,
                owner,
                source,
            );
            reads = reads.checked_add(1).ok_or("ANN status read overflow")?;
            match read(intent)? {
                WorkStatus::Completed(view)
                    if view.workspace == self.source_view.workspace
                        && view.epoch == self.source_view.epoch
                        && view.sequence > self.source_view.sequence =>
                {
                    *self
                        .completed
                        .get_mut(self.next)
                        .ok_or("ANN status cursor outside bitmap")? = true;
                    self.completed_count = self
                        .completed_count
                        .checked_add(1)
                        .ok_or("ANN completed count overflow")?;
                }
                WorkStatus::Completed(view) => {
                    return Err(format!(
                        "ANN work completion for owner {owner:?} is outside the pending source scope: source={:?}, completed={view:?}",
                        self.source_view,
                    )
                    .into());
                }
                WorkStatus::Unknown | WorkStatus::Pending | WorkStatus::Claimed(_) => {}
                WorkStatus::Superseded => {
                    return Err(format!("ANN work for owner {owner:?} was superseded").into());
                }
                WorkStatus::Blocked(reason) => {
                    return Err(
                        format!("ANN work for owner {owner:?} was blocked: {reason:?}").into(),
                    );
                }
            }
            self.advance_cursor()?;
            visited = visited.checked_add(1).ok_or("ANN status visit overflow")?;
        }
        Ok(self.completed_count == self.sources.len())
    }

    fn advance_cursor(&mut self) -> TestResult {
        let next = self
            .next
            .checked_add(1)
            .ok_or("ANN status cursor overflow")?;
        self.next = if next == self.sources.len() { 0 } else { next };
        Ok(())
    }
}

pub(super) fn with_snapshot(
    primary: &dyn std::error::Error,
    client: &NativeUdsClient,
    spec: &InstanceSpec,
    sources: &[(OwnerKey, ObjectRef)],
) -> Box<dyn std::error::Error> {
    // This is a separate failure diagnostic, after the original gate failed.
    // One shared SDK deadline bounds every read; it never resumes, claims or
    // funds product work, and cannot change the original gate result.
    let diagnostic_budget = budget();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::from([
        ("unknown", 0),
        ("pending", 0),
        ("claimed", 0),
        ("completed", 0),
        ("superseded", 0),
        ("blocked", 0),
    ]);
    let mut representatives = BTreeMap::new();
    let mut observed = 0usize;
    let mut stopped = None;
    for (owner, source) in sources {
        let intent = owner_intent(workspace(), 1, spec, *owner, *source);
        let status =
            match client.request(NativeIndexRequest::WorkStatus(intent), &diagnostic_budget) {
                Ok(NativeIndexResponse::WorkStatus(status)) => status,
                Ok(_response) => {
                    stopped = Some("unexpected work-status reply variant".into());
                    break;
                }
                Err(error) => {
                    let prefix: String = error.to_string().chars().take(512).collect();
                    stopped = Some(format!(
                        "work-status read stopped (bounded error): {prefix}"
                    ));
                    break;
                }
            };
        let label = match status {
            WorkStatus::Unknown => "unknown",
            WorkStatus::Pending => "pending",
            WorkStatus::Claimed(_) => "claimed",
            WorkStatus::Completed(_) => "completed",
            WorkStatus::Superseded => "superseded",
            WorkStatus::Blocked(_) => "blocked",
        };
        // At most the fixed admitted source count is visited, and the six
        // status classes each keep one exact intent/status example.
        let Some(count) = counts.get_mut(label) else {
            stopped = Some("unknown work-status classifier".into());
            break;
        };
        *count = count.saturating_add(1);
        observed = observed.saturating_add(1);
        let _example = representatives
            .entry(label)
            .or_insert_with(|| format!("{intent:?}: {status:?}"));
    }
    let primary = quality_error_chain(primary);
    format!(
        "{primary}; non-atomic public work snapshot observed={observed}/{} counts={counts:?} examples={representatives:?} stopped={stopped:?}; this is final-state observation, not temporal first-failure proof",
        sources.len(),
    ).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::Refusal;

    #[test]
    fn readiness_pace_backs_off_without_progress_and_resets_with_deadline_cap() -> TestResult {
        let mut pace = ReadinessPace::new();
        let one_minute = Duration::from_secs(60);
        for expected_ms in [500, 1_000, 1_000] {
            require_equal(
                &pace.after_page(false, one_minute),
                &Duration::from_millis(expected_ms),
                "ANN no-progress poll pause",
            )?;
        }
        require_equal(
            &pace.after_page(true, Duration::from_millis(30)),
            &Duration::from_millis(30),
            "ANN progress reset capped by original deadline",
        )?;
        require_equal(
            &pace.after_page(false, one_minute),
            &Duration::from_millis(500),
            "ANN progress reset restarts initial cadence",
        )?;
        require_equal(
            &pace.after_page(false, Duration::ZERO),
            &Duration::ZERO,
            "ANN expired deadline cannot add poll delay",
        )?;
        require_equal(
            &pace.after_page(false, one_minute),
            &Duration::from_millis(1_000),
            "ANN deadline cap does not reset no-progress cadence",
        )
    }

    #[test]
    fn readiness_cost_keeps_bounded_windows_and_unsampled_tail() -> TestResult {
        let mut cost = ReadinessCost::new("unit");
        cost.record_rpc(Duration::from_millis(2));
        cost.record_rpc(Duration::from_millis(3));
        cost.observe_completed(1);
        cost.sample(Duration::from_secs(60));
        cost.record_rpc(Duration::from_millis(7));
        cost.observe_completed(2);
        let report = cost.report(Duration::from_secs(70));
        require(
            report.contains("reads=3 rpc_wall_ms=12")
                && report.contains("(60, 1, 2, 5)")
                && report.contains("tail=(70, 2, 1, 7)"),
            "ANN readiness cost lost a sampled or final RPC",
        )?;
        for minute in 2..=10 {
            cost.sample(Duration::from_secs(minute * 60));
        }
        require_equal(&cost.samples.len(), &8, "ANN readiness sample bound")?;
        require_equal(
            &cost.samples.front().map(|sample| sample.0),
            &Some(180),
            "ANN readiness retained oldest sample after ring saturation",
        )
    }

    fn view(sequence: u64) -> ViewRef {
        ViewRef {
            workspace: workspace(),
            epoch: 1,
            sequence,
            digest: Digest([9; 32]),
        }
    }

    fn sources(count: u8) -> Vec<(OwnerKey, ObjectRef)> {
        (0..count)
            .map(|seed| {
                (
                    OwnerKey {
                        producer: operation(2).producer,
                        stable_id: StableOwnerId([seed; 16]),
                    },
                    ObjectRef(Digest([seed; 32])),
                )
            })
            .collect()
    }

    #[test]
    fn out_of_order_completion_never_hides_pending_input() -> TestResult {
        let sources = sources(2);
        let spec = text_spec();
        let mut progress = CompletionProgress::new(&sources, view(5), &spec)?;
        let (first_owner, first_source) = sources.first().copied().ok_or("missing first source")?;
        let (second_owner, second_source) =
            sources.get(1).copied().ok_or("missing second source")?;
        let expected_first = WorkIntent {
            workspace: workspace(),
            epoch: 1,
            instance: spec.instance,
            scope: WorkScope::Owner {
                owner: first_owner,
                expected: OwnerInputBinding {
                    source: Some(first_source),
                    projection: None,
                    producer_context: Digest([0; 32]),
                    derivation_policy: spec.policy,
                },
            },
        };
        let expected_second = WorkIntent {
            workspace: workspace(),
            epoch: 1,
            instance: spec.instance,
            scope: WorkScope::Owner {
                owner: second_owner,
                expected: OwnerInputBinding {
                    source: Some(second_source),
                    projection: None,
                    producer_context: Digest([0; 32]),
                    derivation_policy: spec.policy,
                },
            },
        };
        let mut observed = Vec::new();
        require(
            !progress.poll_next(|intent| {
                observed.push(intent);
                if intent == expected_first {
                    Ok(WorkStatus::Pending)
                } else if intent == expected_second {
                    Ok(WorkStatus::Completed(view(6)))
                } else {
                    Err("tracker queried a foreign input".into())
                }
            })?,
            "later completion must not report all inputs ready",
        )?;
        require_equal(&observed.len(), &2_usize, "out-of-order round read count")?;
        require_equal(
            observed.first().ok_or("missing first read")?,
            &expected_first,
            "first exact intent",
        )?;
        require_equal(
            observed.get(1).ok_or("missing second read")?,
            &expected_second,
            "second exact intent",
        )?;
        require_equal(&progress.completed(), &1_usize, "later completed count")?;

        let mut retried = Vec::new();
        require(
            !progress.poll_next(|intent| {
                retried.push(intent);
                Ok(WorkStatus::Claimed(
                    quanta_index_contract::index::WorkClaim {
                        intent,
                        fence: 1,
                        lease_until_ms: 1,
                    },
                ))
            })?,
            "claimed first input must remain incomplete",
        )?;
        require_equal(
            &retried.len(),
            &1_usize,
            "verified second receipt must not be reread",
        )?;
        require_equal(
            retried.first().ok_or("missing retried intent")?,
            &expected_first,
            "retry exact first intent",
        )?;
        require_equal(
            &progress.completed(),
            &1_usize,
            "claimed first completed count",
        )?;

        require(
            progress.poll_next(|intent| {
                require_equal(&intent, &expected_first, "only first intent remains")?;
                Ok(WorkStatus::Completed(view(7)))
            })?,
            "both exact inputs must become ready",
        )?;
        require_equal(
            &progress.completed(),
            &sources.len(),
            "completed input count",
        )?;
        Ok(())
    }

    #[test]
    fn completion_rejects_wrong_scope_and_old_view() -> TestResult {
        let sources = sources(1);
        let spec = text_spec();
        let mut progress = CompletionProgress::new(&sources, view(5), &spec)?;
        for invalid in [
            ViewRef {
                workspace: WorkspaceKey {
                    tenant: TenantId([8; 16]),
                    ..workspace()
                },
                ..view(6)
            },
            ViewRef {
                epoch: 2,
                ..view(6)
            },
            view(5),
            view(4),
        ] {
            require(
                progress
                    .poll_next(|_| Ok(WorkStatus::Completed(invalid)))
                    .is_err(),
                "foreign or stale completion must fail",
            )?;
            require_equal(
                &progress.completed(),
                &0_usize,
                "rejected completion cursor",
            )?;
        }
        require_equal(&progress.instance(), &spec.instance, "completion instance")?;
        Ok(())
    }

    #[test]
    fn status_reads_are_bounded_and_errors_preserve_the_cursor() -> TestResult {
        let sources = sources(66);
        let spec = text_spec();
        let mut progress = CompletionProgress::new(&sources, view(5), &spec)?;
        let mut reads = 0_usize;
        require(
            !progress.poll_next(|_| {
                reads = reads.checked_add(1).ok_or("status read count overflow")?;
                Ok(WorkStatus::Completed(view(6)))
            })?,
            "64 reads must yield before the last two inputs",
        )?;
        require_equal(&reads, &64_usize, "bounded status reads")?;
        require_equal(&progress.completed(), &64_usize, "bounded cursor")?;
        let mut second_reads = 0_usize;
        require(
            progress
                .poll_next(|_| {
                    second_reads = second_reads
                        .checked_add(1)
                        .ok_or("second status read count overflow")?;
                    if second_reads == 1 {
                        Ok(WorkStatus::Completed(view(6)))
                    } else {
                        Err("transport refused".into())
                    }
                })
                .is_err(),
            "transport error must propagate",
        )?;
        require_equal(
            &second_reads,
            &2_usize,
            "transport error after one completed status",
        )?;
        require_equal(&progress.completed(), &65_usize, "transport error cursor")?;
        for terminal in [
            WorkStatus::Superseded,
            WorkStatus::Blocked(Refusal::ResourceLimited),
        ] {
            require(
                progress.poll_next(|_| Ok(terminal)).is_err(),
                "terminal status must fail",
            )?;
            require_equal(&progress.completed(), &65_usize, "terminal status cursor")?;
        }
        let (last_owner, last_source) = sources.get(65).copied().ok_or("missing last source")?;
        require(
            progress.poll_next(|intent| {
                require_equal(
                    &intent.scope,
                    &WorkScope::Owner {
                        owner: last_owner,
                        expected: OwnerInputBinding {
                            source: Some(last_source),
                            projection: None,
                            producer_context: Digest([0; 32]),
                            derivation_policy: spec.policy,
                        },
                    },
                    "last status read after errors",
                )?;
                Ok(WorkStatus::Completed(view(6)))
            })?,
            "last completed input must finish the cursor",
        )?;
        require_equal(&progress.completed(), &sources.len(), "final cursor")?;
        let oversized = vec![*sources.first().ok_or("missing source for bound")?; 4_097];
        require(
            CompletionProgress::new(&oversized, view(5), &spec).is_err(),
            "tracker must reject more than the fixed 4096 input rows",
        )?;
        Ok(())
    }
}
