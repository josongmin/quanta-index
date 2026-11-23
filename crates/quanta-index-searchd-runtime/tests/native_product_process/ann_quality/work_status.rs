//! Bounded readiness and failure observations through the public work API.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write as _;

use super::*;
use quanta_index_contract::index::{OwnerInputBinding, WorkIntent, WorkScope, WorkStatus};

const COMPLETION_STATUS_PAGE: usize = 64;
const MAX_UNREADY_STATUS_PROBES: usize = 2;
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

/// Test-side readiness costs. Poll pauses overlap daemon work. The receipt
/// timestamp is the first complete public observation, not a worker timestamp.
pub(super) struct ReadinessCost {
    phase: &'static str,
    started: Instant,
    reads: u64,
    rpc_wall: Duration,
    poll_pauses: u64,
    poll_pause_wall: Duration,
    receipts_elapsed: Option<Duration>,
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
            poll_pauses: 0,
            poll_pause_wall: Duration::ZERO,
            receipts_elapsed: None,
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

    pub(super) fn pause_before_receipts(&mut self, delay: Duration) {
        let started = Instant::now();
        std::thread::sleep(delay);
        self.record_poll_pause(started.elapsed());
    }

    fn record_poll_pause(&mut self, elapsed: Duration) {
        self.poll_pauses = self.poll_pauses.saturating_add(1);
        self.poll_pause_wall = self.poll_pause_wall.saturating_add(elapsed);
    }

    pub(super) fn observe_receipts_complete(&mut self, observed_at: Instant) {
        let _first = self
            .receipts_elapsed
            .get_or_insert(observed_at.saturating_duration_since(self.started));
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
        let receipts_elapsed_ms = self.receipts_elapsed.map(|receipt| receipt.as_millis());
        let post_receipt_ms = self
            .receipts_elapsed
            .map(|receipt| elapsed.saturating_sub(receipt).as_millis());
        format!(
            "native-ann-work-status phase={:?} success={} reads={} rpc_wall_ms={} poll_pauses={} poll_pause_wall_ms={} receipts_elapsed_ms={receipts_elapsed_ms:?} post_receipt_ms={post_receipt_ms:?} elapsed_ms={} samples=(elapsed_s,completed,reads,rpc_wall_ms){:?} tail={tail:?}",
            self.phase,
            self.succeeded,
            self.reads,
            self.rpc_wall.as_millis(),
            self.poll_pauses,
            self.poll_pause_wall.as_millis(),
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

pub(super) fn owner_intent(
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
    completed_sequences: BTreeMap<u64, OwnerKey>,
    highest_completed_view: Option<ViewRef>,
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
            completed_sequences: BTreeMap::new(),
            highest_completed_view: None,
        })
    }

    pub(crate) fn completed(&self) -> usize {
        self.completed_count
    }

    fn highest_completed_view(&self) -> Option<ViewRef> {
        self.highest_completed_view
    }

    pub(crate) fn verify_latest_publication(&self, latest: ViewRef) -> TestResult {
        let completed = self.highest_completed_view().unwrap_or(self.source_view);
        require_equal(
            &latest.workspace,
            &completed.workspace,
            "ANN latest workspace",
        )?;
        require_equal(&latest.epoch, &completed.epoch, "ANN latest epoch")?;
        require(
            latest.sequence >= completed.sequence,
            format!("ANN latest view {latest:?} precedes completed work view {completed:?}"),
        )?;
        if latest.sequence == completed.sequence {
            require_equal(
                &latest,
                &completed,
                "ANN latest identity at completed sequence",
            )?;
        }
        Ok(())
    }

    pub(crate) fn total(&self) -> usize {
        self.sources.len()
    }

    pub(super) fn spec(&self) -> &InstanceSpec {
        self.spec
    }

    pub(crate) fn poll_next(
        &mut self,
        mut read: impl FnMut(WorkIntent) -> TestResult<WorkStatus>,
    ) -> TestResult<bool> {
        if self.completed_count == self.sources.len() {
            return Ok(true);
        }
        let mut visited = 0_usize;
        let mut reads = 0_usize;
        let mut unready = 0_usize;
        while visited < self.sources.len()
            && reads < COMPLETION_STATUS_PAGE
            && unready < MAX_UNREADY_STATUS_PROBES
        {
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
                    if let Some(prior_owner) = self.completed_sequences.get(&view.sequence) {
                        return Err(format!(
                            "ANN work for owners {prior_owner:?} and {owner:?} completed at the same view sequence {}",
                            view.sequence,
                        )
                        .into());
                    }
                    let next_count = self
                        .completed_count
                        .checked_add(1)
                        .ok_or("ANN completed count overflow")?;
                    *self
                        .completed
                        .get_mut(self.next)
                        .ok_or("ANN status cursor outside bitmap")? = true;
                    let _previous = self.completed_sequences.insert(view.sequence, owner);
                    self.completed_count = next_count;
                    if self
                        .highest_completed_view
                        .is_none_or(|highest| view.sequence > highest.sequence)
                    {
                        self.highest_completed_view = Some(view);
                    }
                }
                WorkStatus::Completed(view) => {
                    return Err(format!(
                        "ANN work completion for owner {owner:?} is outside the pending source scope: source={:?}, completed={view:?}",
                        self.source_view,
                    )
                    .into());
                }
                WorkStatus::Claimed(claim) => {
                    if claim.intent != intent || claim.fence == 0 || claim.lease_until_ms == 0 {
                        return Err(format!(
                            "ANN work claim for owner {owner:?} has invalid intent or lease"
                        )
                        .into());
                    }
                    unready = unready.checked_add(1).ok_or("ANN pending probe overflow")?;
                }
                WorkStatus::Unknown | WorkStatus::Pending => {
                    unready = unready.checked_add(1).ok_or("ANN pending probe overflow")?;
                }
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
        require(
            cost.report(Duration::ZERO)
                .contains("receipts_elapsed_ms=None post_receipt_ms=None"),
            "unfinished receipt census acquired an observation time",
        )?;
        cost.record_rpc(Duration::from_millis(2));
        cost.record_rpc(Duration::from_millis(3));
        cost.record_poll_pause(Duration::from_millis(500));
        cost.record_poll_pause(Duration::from_millis(1_050));
        cost.observe_completed(1);
        cost.sample(Duration::from_secs(60));
        cost.record_rpc(Duration::from_millis(7));
        cost.observe_completed(2);
        for second in [65, 68] {
            let observed_at = cost
                .started
                .checked_add(Duration::from_secs(second))
                .ok_or("test receipt observation overflow")?;
            cost.observe_receipts_complete(observed_at);
        }
        let report = cost.report(Duration::from_secs(70));
        require(
            report.contains("reads=3 rpc_wall_ms=12")
                && report.contains("poll_pauses=2 poll_pause_wall_ms=1550")
                && report.contains("receipts_elapsed_ms=Some(65000) post_receipt_ms=Some(5000)")
                && report.contains("(60, 1, 2, 5)")
                && report.contains("tail=(70, 2, 1, 7)"),
            "ANN readiness report lost a poll cost, receipt time or RPC window",
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

    fn completed_for(intent: WorkIntent) -> TestResult<WorkStatus> {
        let WorkScope::Owner { owner, .. } = intent.scope else {
            return Err("completion observed a non-owner scope".into());
        };
        Ok(WorkStatus::Completed(view(
            6 + u64::from(owner.stable_id.0[0]),
        )))
    }

    #[test]
    fn product_size_readiness_separates_available_receipts_from_observation_deadline() -> TestResult
    {
        let sources: Vec<_> = (0..4_096_u16)
            .map(|ordinal| {
                let ordinal = ordinal.to_be_bytes();
                let mut owner = [0; 16];
                owner
                    .first_chunk_mut::<2>()
                    .ok_or("owner key has no ordinal")?
                    .copy_from_slice(&ordinal);
                let mut source = [0; 32];
                source
                    .first_chunk_mut::<2>()
                    .ok_or("source digest has no ordinal")?
                    .copy_from_slice(&ordinal);
                Ok((
                    OwnerKey {
                        producer: operation(2).producer,
                        stable_id: StableOwnerId(owner),
                    },
                    ObjectRef(Digest(source)),
                ))
            })
            .collect::<TestResult<_>>()?;
        let spec = text_spec();
        let source_view = view(5);
        // The 63 pauses between 64 completed pages require 31.5 s even
        // when every receipt is available before the observer starts.
        for (ready_at_start, deadline_ms, expected_ms, expected_reads, expected_complete) in [
            (true, 31_000, 31_000, 3_968, 3_968),
            (true, 32_000, 31_500, 4_096, 4_096),
            (false, 1_200_000, 1_200_000, 2_402, 0),
        ] {
            let mut progress = CompletionProgress::new(&sources, source_view, &spec)?;
            let mut pace = ReadinessPace::new();
            let deadline = Duration::from_millis(deadline_ms);
            let mut elapsed = Duration::ZERO;
            let mut observed = Vec::new();
            let mut ready = false;
            for _ in 0..1_202 {
                if elapsed >= deadline {
                    break;
                }
                let prior = progress.completed();
                ready = progress.poll_next(|intent| {
                    let WorkScope::Owner { owner, .. } = intent.scope else {
                        return Err("ANN readiness requested a non-owner status".into());
                    };
                    let [high, low, ..] = owner.stable_id.0;
                    let ordinal = u16::from_be_bytes([high, low]);
                    let (expected_owner, source) = sources
                        .get(usize::from(ordinal))
                        .copied()
                        .ok_or("ANN readiness requested an unknown owner")?;
                    require_equal(
                        &intent,
                        &owner_intent(workspace(), 1, &spec, expected_owner, source),
                        "ANN readiness work identity",
                    )?;
                    observed.push(ordinal);
                    if ready_at_start {
                        // Publish in the reverse of the observer's owner order.
                        Ok(WorkStatus::Completed(view(
                            4_101_u64.saturating_sub(u64::from(ordinal)),
                        )))
                    } else {
                        Ok(WorkStatus::Pending)
                    }
                })?;
                if ready {
                    break;
                }
                let remaining = deadline.saturating_sub(elapsed);
                let pause = pace.after_page(progress.completed() > prior, remaining);
                require(
                    !pause.is_zero() && pause <= remaining,
                    "invalid ANN poll pause",
                )?;
                elapsed = elapsed.saturating_add(pause);
            }
            require_equal(
                &elapsed,
                &Duration::from_millis(expected_ms),
                "ANN readiness virtual elapsed time",
            )?;
            require_equal(
                &observed,
                &(0..expected_reads).collect::<Vec<_>>(),
                "ANN readiness exact probe order and bound",
            )?;
            require_equal(
                &progress.completed(),
                &expected_complete,
                "ANN receipt count",
            )?;
            require_equal(
                &ready,
                &(expected_complete == 4_096),
                "ANN readiness result",
            )?;
            if ready {
                progress.verify_latest_publication(view(4_101))?;
            }
        }
        Ok(())
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
        require_equal(
            &progress.highest_completed_view(),
            &Some(view(6)),
            "later completion view",
        )?;

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
        require_equal(
            &progress.highest_completed_view(),
            &Some(view(7)),
            "highest out-of-order completion view",
        )?;
        Ok(())
    }

    #[test]
    fn duplicate_completion_sequence_cannot_complete_distinct_intents() -> TestResult {
        let sources = sources(2);
        let spec = text_spec();
        let mut progress = CompletionProgress::new(&sources, view(5), &spec)?;
        let rejected = progress
            .poll_next(|_| Ok(WorkStatus::Completed(view(6))))
            .expect_err("one published view cannot complete two owner intents");
        require(
            rejected.to_string().contains("same view sequence"),
            "duplicate completion must fail for the sequence collision",
        )?;
        require_equal(&progress.completed(), &1, "unique completed receipt count")?;
        require_equal(
            &progress.highest_completed_view(),
            &Some(view(6)),
            "highest accepted completion view",
        )?;
        let (second_owner, second_source) = sources.get(1).copied().ok_or("second source")?;
        require(
            progress.poll_next(|intent| {
                require_equal(
                    &intent,
                    &owner_intent(workspace(), 1, &spec, second_owner, second_source),
                    "duplicate completion preserves pending cursor",
                )?;
                Ok(WorkStatus::Completed(view(7)))
            })?,
            "distinct completion publications must finish both owners",
        )?;
        require_equal(
            &progress.completed(),
            &2,
            "distinct completed receipt count",
        )?;
        require_equal(
            &progress.highest_completed_view(),
            &Some(view(7)),
            "highest completion after recovery",
        )
    }

    #[test]
    fn claimed_status_rejects_foreign_intent_and_invalid_lease() -> TestResult {
        let foreign_sources = sources(2);
        let sources = sources(1);
        let spec = text_spec();
        let mut progress = CompletionProgress::new(&sources, view(5), &spec)?;
        let (first_owner, first_source) = sources.first().copied().ok_or("first source")?;
        let (second_owner, second_source) =
            foreign_sources.get(1).copied().ok_or("foreign source")?;
        let first = owner_intent(workspace(), 1, &spec, first_owner, first_source);
        let second = owner_intent(workspace(), 1, &spec, second_owner, second_source);
        let valid = quanta_index_contract::index::WorkClaim {
            intent: first,
            fence: 1,
            lease_until_ms: 1,
        };
        for invalid in [
            quanta_index_contract::index::WorkClaim {
                intent: second,
                ..valid
            },
            quanta_index_contract::index::WorkClaim { fence: 0, ..valid },
            quanta_index_contract::index::WorkClaim {
                lease_until_ms: 0,
                ..valid
            },
        ] {
            let rejected = progress
                .poll_next(|_| Ok(WorkStatus::Claimed(invalid)))
                .expect_err("malformed claim must fail instead of appearing pending");
            require(
                rejected.to_string().contains("invalid intent or lease"),
                "malformed claim must fail for its identity or lease",
            )?;
            require_equal(&progress.completed(), &0, "rejected claim completion count")?;
        }
        require(
            !progress.poll_next(|intent| {
                require_equal(&intent, &first, "rejected claim preserves cursor")?;
                Ok(WorkStatus::Claimed(valid))
            })?,
            "valid claim remains pending",
        )?;
        require_equal(&progress.completed(), &0, "valid claim completion count")
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
        require_equal(
            &progress.spec().instance,
            &spec.instance,
            "completion instance",
        )?;
        Ok(())
    }

    #[test]
    fn latest_view_must_include_the_observed_completion_publication() -> TestResult {
        let sources = sources(1);
        let spec = text_spec();
        let mut progress = CompletionProgress::new(&sources, view(5), &spec)?;
        require(
            progress.poll_next(|_| Ok(WorkStatus::Completed(view(8))))?,
            "one receipt must complete",
        )?;
        progress.verify_latest_publication(view(8))?;
        progress.verify_latest_publication(view(9))?;
        for invalid in [
            view(7),
            ViewRef {
                digest: Digest([10; 32]),
                ..view(8)
            },
            ViewRef {
                epoch: 2,
                ..view(9)
            },
        ] {
            require(
                progress.verify_latest_publication(invalid).is_err(),
                "latest view failed to bind observed publication",
            )?;
        }
        Ok(())
    }

    #[test]
    fn pending_status_probe_budget_yields_fairly_and_preserves_all_completion_receipts()
    -> TestResult {
        let sources = sources(66);
        let spec = text_spec();
        let mut progress = CompletionProgress::new(&sources, view(5), &spec)?;
        let mut pending = Vec::new();
        for _ in 0..2 {
            let before = pending.len();
            require(
                !progress.poll_next(|intent| {
                    pending.push(intent);
                    Ok(WorkStatus::Pending)
                })?,
                "pending work must never become completion",
            )?;
            require_equal(&(pending.len() - before), &2, "pending RPC probe bound")?;
        }
        for (intent, (owner, source)) in pending.iter().zip(&sources) {
            require_equal(
                intent,
                &owner_intent(workspace(), 1, &spec, *owner, *source),
                "pending cursor must advance fairly",
            )?;
        }
        require_equal(&progress.completed(), &0, "pending receipt count")?;
        let mut completed = std::collections::BTreeSet::new();
        for expected_ready in [false, true] {
            let ready = progress.poll_next(|intent| {
                let owner = match intent.scope {
                    WorkScope::Owner { owner, .. } => owner,
                    _ => return Err("completion observed a non-owner scope".into()),
                };
                require(completed.insert(owner), "completed receipt reread")?;
                completed_for(intent)
            })?;
            require_equal(&ready, &expected_ready, "all original receipts required")?;
        }
        require_equal(
            &completed.len(),
            &sources.len(),
            "exact completed source count",
        )?;
        require_equal(
            &progress.completed(),
            &sources.len(),
            "complete bitmap count",
        )?;
        require_equal(
            &progress.highest_completed_view(),
            &Some(view(71)),
            "highest completed status view",
        )
    }

    #[test]
    fn status_reads_are_bounded_and_errors_preserve_the_cursor() -> TestResult {
        let sources = sources(66);
        let spec = text_spec();
        let mut progress = CompletionProgress::new(&sources, view(5), &spec)?;
        let mut reads = 0_usize;
        require(
            !progress.poll_next(|intent| {
                reads = reads.checked_add(1).ok_or("status read count overflow")?;
                completed_for(intent)
            })?,
            "64 reads must yield before the last two inputs",
        )?;
        require_equal(&reads, &64_usize, "bounded status reads")?;
        require_equal(&progress.completed(), &64_usize, "bounded cursor")?;
        let mut second_reads = 0_usize;
        require(
            progress
                .poll_next(|intent| {
                    second_reads = second_reads
                        .checked_add(1)
                        .ok_or("second status read count overflow")?;
                    if second_reads == 1 {
                        completed_for(intent)
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
                completed_for(intent)
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
