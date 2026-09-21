//! The request budget inside the dense lane (W5 phase 3).
//!
//! Before this module the dense lane saw the request budget (QI-BB-002)
//! only at the dispatcher's checkpoints around it: once the vector query
//! was issued it ran to the end — an approximate probe or a flat scan of
//! every row — for a peer that had left or a deadline that had passed.
//! The lane now observes the budget at three points of its own:
//!
//! 1. **before the query is issued** — the watcher looks first, so an
//!    already interrupted request never reaches the library;
//! 2. **while the query is in flight** — [`race_with_budget`] races the
//!    query against a watcher that sleeps until the deadline and looks at
//!    the cancellation flag every [`SEMANTIC_BUDGET_POLL_INTERVAL`];
//! 3. **as rows come back** — [`RowBudgetProbe`] looks on the first row
//!    and every [`SEMANTIC_BUDGET_TICK_ROWS`] rows the lane reads.
//!
//! Every observation answers the typed interruption
//! (`REQUEST_CANCELLED` / `REQUEST_DEADLINE_EXCEEDED`) naming the lane
//! that looked: `semantic:ann` for an approximate lane, `semantic:exact`
//! for an exact one, and counts it in [`DenseLaneTalliesV1`] so the
//! metrics scrape can say how often each lane was interrupted.
//!
//! **What dropping the in-flight query does.** The watcher winning the
//! race drops the query future, and with it the library's plan stream: no
//! further batch is requested and the stream's buffers are released. The
//! library does not interrupt work it has already handed to its CPU pool
//! (one blocking task per flat-scan batch or partition probe, bounded by
//! its CPU count): those run to completion, their results are discarded,
//! and their threads and memory stay charged until they finish — the
//! plan's rule that a backend call without cooperative interruption keeps
//! its slot until it returns (plan §7.3). Nothing here waits for them,
//! and nothing here reports the interruption as "the work has stopped";
//! it reports that the request stopped being served.
//!
//! There is no refill or rerank stage in this adapter: a filtered query is
//! prefiltered inside one plan, and an approximate lane's refinement is
//! part of that plan, so the only Rust-side loop is the row read-back. The
//! one second pass is completion: an approximate pass that returns fewer
//! rows than asked while its scope holds more is answered again by the
//! exact lane, and that count and that pass observe the budget like the
//! first.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use quanta_index_core::{CoreError, MetricPointV1, RequestBudgetV1};

/// Rows the lane reads back between two looks at the budget.
///
/// A look costs one `Instant::now()` and one atomic load; a row costs
/// several string copies, so at this interval the looks are noise. The
/// rows a lane reads are bounded by the fetch ceiling, which keeps the
/// number of looks per query small either way.
pub(crate) const SEMANTIC_BUDGET_TICK_ROWS: usize = 256;

/// How long the in-flight watcher sleeps between looks at the cancellation
/// flag.
///
/// The deadline needs no polling — the watcher sleeps straight to it — so
/// this bounds only how long a departed peer's query keeps running past
/// the moment the transport noticed.
pub(crate) const SEMANTIC_BUDGET_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Which index the dense lane runs through, as the checkpoint names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DenseLaneKindV1 {
    /// The sealed approximate index (QI-BB-027).
    Approximate,
    /// Every row scored; the index bypassed or absent.
    Exact,
}

impl DenseLaneKindV1 {
    const ALL: [Self; 2] = [Self::Approximate, Self::Exact];

    /// The checkpoint name an interruption observed in this lane carries.
    pub(crate) const fn checkpoint(self) -> &'static str {
        match self {
            Self::Approximate => "semantic:ann",
            Self::Exact => "semantic:exact",
        }
    }

    const fn metric_infix(self) -> &'static str {
        match self {
            Self::Approximate => "ann",
            Self::Exact => "exact",
        }
    }
}

/// One lane's counts since process start.
#[derive(Debug)]
struct LaneTallyV1 {
    /// Vector queries handed to the library.
    queries: AtomicU64,
    /// Budget interruptions observed inside the lane.
    interruptions: AtomicU64,
}

impl LaneTallyV1 {
    const fn new() -> Self {
        Self {
            queries: AtomicU64::new(0),
            interruptions: AtomicU64::new(0),
        }
    }
}

/// What the dense lanes did since process start.
///
/// Per lane, the queries issued to the library and the budget
/// interruptions observed inside it; and how many short approximate passes
/// the exact lane completed.
///
/// One instance per adapter, shared by every searcher it opens; the
/// adapter reports it to the metrics scrape (QI-BB-015).
#[derive(Debug)]
pub(crate) struct DenseLaneTalliesV1 {
    approximate: LaneTallyV1,
    exact: LaneTallyV1,
    /// Approximate passes that returned fewer rows than asked while the
    /// scope held more, and were answered by the exact lane instead.
    exact_completions: AtomicU64,
}

impl Default for DenseLaneTalliesV1 {
    fn default() -> Self {
        Self::new()
    }
}

impl DenseLaneTalliesV1 {
    /// Scrape points one lane contributes.
    const POINTS_PER_LANE: usize = 2;

    /// Tallies at zero.
    pub(crate) const fn new() -> Self {
        Self {
            approximate: LaneTallyV1::new(),
            exact: LaneTallyV1::new(),
            exact_completions: AtomicU64::new(0),
        }
    }

    const fn lane(&self, lane: DenseLaneKindV1) -> &LaneTallyV1 {
        match lane {
            DenseLaneKindV1::Approximate => &self.approximate,
            DenseLaneKindV1::Exact => &self.exact,
        }
    }

    /// One short approximate pass was answered by the exact lane.
    pub(crate) fn count_exact_completion(&self) {
        let _prior = self.exact_completions.fetch_add(1, Ordering::Relaxed);
    }

    /// One vector query was handed to the library.
    pub(crate) fn count_query(&self, lane: DenseLaneKindV1) {
        let _prior = self.lane(lane).queries.fetch_add(1, Ordering::Relaxed);
    }

    /// One interruption was observed inside `lane`.
    pub(crate) fn count_interruption(&self, lane: DenseLaneKindV1) {
        let _prior = self
            .lane(lane)
            .interruptions
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Queries handed to the library by `lane`.
    pub(crate) fn queries(&self, lane: DenseLaneKindV1) -> u64 {
        self.lane(lane).queries.load(Ordering::Relaxed)
    }

    /// Interruptions observed inside `lane`.
    pub(crate) fn interruptions(&self, lane: DenseLaneKindV1) -> u64 {
        self.lane(lane).interruptions.load(Ordering::Relaxed)
    }

    /// The tallies as scrape points: `semantic_dense_queries_<lane>_total`
    /// and `semantic_budget_interruptions_<lane>_total`, one pair per
    /// lane, the lane in the name as the repo's route counters carry theirs,
    /// and `semantic_dense_exact_completions_total`.
    pub(crate) fn scrape(&self) -> Vec<MetricPointV1> {
        let mut points = Vec::with_capacity(
            DenseLaneKindV1::ALL
                .len()
                .saturating_mul(Self::POINTS_PER_LANE)
                .saturating_add(1),
        );
        points.push(MetricPointV1::counter(
            "semantic_dense_exact_completions_total",
            self.exact_completions.load(Ordering::Relaxed),
        ));
        for lane in DenseLaneKindV1::ALL {
            points.push(MetricPointV1::counter(
                format!("semantic_dense_queries_{}_total", lane.metric_infix()),
                self.queries(lane),
            ));
            points.push(MetricPointV1::counter(
                format!(
                    "semantic_budget_interruptions_{}_total",
                    lane.metric_infix()
                ),
                self.interruptions(lane),
            ));
        }
        points
    }
}

/// The request's budget and the tallies the lane reports to; handed down
/// every query path explicitly, so a new query path cannot forget either.
#[derive(Clone, Copy)]
pub(crate) struct DenseLaneBudgetV1<'a> {
    pub(crate) budget: &'a RequestBudgetV1,
    pub(crate) tallies: &'a DenseLaneTalliesV1,
}

/// Counts the rows a lane reads back and looks at the budget on the first
/// row and then every [`SEMANTIC_BUDGET_TICK_ROWS`] rows.
///
/// Rows are read on one task, so this needs no sharing; an observed
/// interruption is counted in the tallies and returned typed, and the
/// read-back ends there.
pub(crate) struct RowBudgetProbe<'a> {
    watch: DenseLaneBudgetV1<'a>,
    lane: DenseLaneKindV1,
    rows: usize,
}

impl<'a> RowBudgetProbe<'a> {
    pub(crate) const fn new(watch: DenseLaneBudgetV1<'a>, lane: DenseLaneKindV1) -> Self {
        Self {
            watch,
            lane,
            rows: 0,
        }
    }

    /// Count one row about to be read. `Err` is the typed interruption
    /// naming this lane's checkpoint, once one is observed.
    pub(crate) fn tick(&mut self) -> Result<(), CoreError> {
        let looked = self.rows.is_multiple_of(SEMANTIC_BUDGET_TICK_ROWS);
        self.rows = self.rows.saturating_add(1);
        if !looked {
            return Ok(());
        }
        match self.watch.budget.interrupted_at(self.lane.checkpoint()) {
            Some(interruption) => {
                self.watch.tallies.count_interruption(self.lane);
                Err(interruption)
            }
            None => Ok(()),
        }
    }
}

/// The typed interruption for `lane`, once the budget reports one.
async fn watch_budget(budget: &RequestBudgetV1, lane: DenseLaneKindV1) -> CoreError {
    loop {
        if let Some(interruption) = budget.interrupted_at(lane.checkpoint()) {
            return interruption;
        }
        tokio::time::sleep(budget.remaining().min(SEMANTIC_BUDGET_POLL_INTERVAL)).await;
    }
}

/// Run `work` — the lane's vector query, issue to last row — under the
/// request budget.
///
/// The watcher is polled first, so an interrupted budget is answered
/// before `work` is ever started, and an interruption observed while
/// `work` is pending drops it where it stands (see the module doc for
/// what that does inside the library). An interruption `work` observed
/// itself, through its row probe, comes back as its own `Err` and is
/// already counted.
pub(crate) async fn race_with_budget<T, F>(
    watch: DenseLaneBudgetV1<'_>,
    lane: DenseLaneKindV1,
    work: F,
) -> Result<T, CoreError>
where
    F: Future<Output = Result<T, CoreError>>,
{
    tokio::select! {
        biased;
        interruption = watch_budget(watch.budget, lane) => {
            watch.tallies.count_interruption(lane);
            Err(interruption)
        }
        result = work => result,
    }
}

/// Debug-only delay injection for the cancellation end-to-end proof.
///
/// An armed hold parks every dense lane before its query is issued, so
/// the only way the request ends is its budget — deterministic where a
/// naturally slow query would be a timing guess.
#[cfg(any(test, debug_assertions))]
pub(crate) mod failpoint {
    use std::sync::atomic::{AtomicBool, Ordering};

    static HOLD_DENSE_LANE: AtomicBool = AtomicBool::new(false);

    pub(crate) fn set_hold_dense_lane_until_interrupted(enabled: bool) {
        HOLD_DENSE_LANE.store(enabled, Ordering::Release);
    }

    // Referenced so a `cfg(test)` build without debug assertions, where the
    // public hook is absent, does not report the setter unused.
    const _: fn(bool) = set_hold_dense_lane_until_interrupted;

    /// Park forever while the hold is armed; a no-op otherwise.
    pub(crate) async fn hold_dense_lane_if_armed() {
        if HOLD_DENSE_LANE.load(Ordering::Acquire) {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(not(any(test, debug_assertions)))]
pub(crate) mod failpoint {
    /// Release builds carry no hold: the lane issues its query at once.
    pub(crate) async fn hold_dense_lane_if_armed() {}
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning tests assert with `assert!` on fixture invariants; a violated invariant is not a propagatable error"
)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::{Duration, Instant};

    use quanta_index_core::{
        CancelHandleV1, CoreError, MetricValueV1, REQUEST_CANCELLED_CODE,
        REQUEST_DEADLINE_EXCEEDED_CODE, RequestBudgetV1,
    };

    use super::{
        DenseLaneBudgetV1, DenseLaneKindV1, DenseLaneTalliesV1, RowBudgetProbe,
        SEMANTIC_BUDGET_TICK_ROWS, race_with_budget,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Rows a live-budget probe is asked to pass: three full intervals and
    /// one row into the fourth, so every look in between is exercised.
    const THREE_INTERVALS_AND_ONE: usize = 3 * SEMANTIC_BUDGET_TICK_ROWS + 1;

    fn runtime() -> Result<tokio::runtime::Runtime, Box<dyn std::error::Error>> {
        Ok(tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?)
    }

    fn typed(
        result: &Result<(), CoreError>,
    ) -> Result<(quanta_index_contract::SearchPlaneErrorCodeV2, String), Box<dyn std::error::Error>>
    {
        match result {
            Err(CoreError::Typed { code, message }) => Ok((*code, message.clone())),
            other => Err(format!("expected a typed interruption, got {other:?}").into()),
        }
    }

    /// A query that cancels its own request the first time it is polled and
    /// then never completes: the shape of a peer leaving while the library
    /// is busy.
    struct CancelOnFirstPoll {
        handle: CancelHandleV1,
    }

    impl Future for CancelOnFirstPoll {
        type Output = Result<(), CoreError>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.handle.cancel();
            Poll::Pending
        }
    }

    /// The watcher answers a cancelled budget before the work is polled at
    /// all, and a pending query is dropped once the cancellation lands.
    #[test]
    fn a_cancelled_budget_is_answered_before_the_query_and_a_pending_query_is_dropped() -> TestResult
    {
        let runtime = runtime()?;
        let tallies = DenseLaneTalliesV1::default();

        let cancelled = RequestBudgetV1::unbounded();
        cancelled.cancel_handle().cancel();
        let watch = DenseLaneBudgetV1 {
            budget: &cancelled,
            tallies: &tallies,
        };
        let mut work_started = false;
        let result = crate::run_blocking(
            &runtime,
            race_with_budget(watch, DenseLaneKindV1::Approximate, async {
                work_started = true;
                Ok(())
            }),
        );
        let (code, message) = typed(&result)?;
        assert_eq!(code, REQUEST_CANCELLED_CODE);
        assert!(message.contains("checkpoint `semantic:ann`"), "{message}");
        assert!(
            !work_started,
            "the query must not start under a cancelled budget"
        );
        assert_eq!(tallies.interruptions(DenseLaneKindV1::Approximate), 1);
        assert_eq!(tallies.interruptions(DenseLaneKindV1::Exact), 0);

        let live = RequestBudgetV1::unbounded();
        let watch = DenseLaneBudgetV1 {
            budget: &live,
            tallies: &tallies,
        };
        let pending = CancelOnFirstPoll {
            handle: live.cancel_handle(),
        };
        let result = crate::run_blocking(
            &runtime,
            race_with_budget(watch, DenseLaneKindV1::Exact, pending),
        );
        let (code, message) = typed(&result)?;
        assert_eq!(code, REQUEST_CANCELLED_CODE);
        assert!(message.contains("checkpoint `semantic:exact`"), "{message}");
        assert!(live.is_cancelled());
        assert_eq!(tallies.interruptions(DenseLaneKindV1::Exact), 1);
        Ok(())
    }

    /// A budget whose deadline already passed is answered as a deadline,
    /// and a live budget hands the work's own answer back untouched.
    #[test]
    fn a_passed_deadline_is_a_deadline_and_a_live_budget_serves() -> TestResult {
        let runtime = runtime()?;
        let tallies = DenseLaneTalliesV1::default();
        let expired = RequestBudgetV1::until(
            Instant::now()
                .checked_sub(Duration::from_millis(5))
                .ok_or("the clock is more than five milliseconds old")?,
        );
        let watch = DenseLaneBudgetV1 {
            budget: &expired,
            tallies: &tallies,
        };
        let result = crate::run_blocking(
            &runtime,
            race_with_budget(watch, DenseLaneKindV1::Exact, async { Ok(()) }),
        );
        let (code, message) = typed(&result)?;
        assert_eq!(code, REQUEST_DEADLINE_EXCEEDED_CODE);
        assert!(message.contains("checkpoint `semantic:exact`"), "{message}");

        let live = RequestBudgetV1::unbounded();
        let watch = DenseLaneBudgetV1 {
            budget: &live,
            tallies: &tallies,
        };
        let served = crate::run_blocking(
            &runtime,
            race_with_budget(watch, DenseLaneKindV1::Exact, async { Ok(7_u32) }),
        )?;
        assert_eq!(served, 7);
        let refused = crate::run_blocking(
            &runtime,
            race_with_budget(watch, DenseLaneKindV1::Exact, async {
                Err::<(), _>(CoreError::Storage("the library said no".to_string()))
            }),
        );
        assert!(
            matches!(&refused, Err(CoreError::Storage(message)) if message == "the library said no"),
            "the work's own error passes through: {refused:?}"
        );
        assert_eq!(tallies.interruptions(DenseLaneKindV1::Exact), 1);
        assert_eq!(tallies.interruptions(DenseLaneKindV1::Approximate), 0);
        Ok(())
    }

    /// The row probe looks on the first row and then once per interval.
    ///
    /// Under a live budget every row passes; under a cancelled one the
    /// first row is refused, so fewer rows than one interval are read; a
    /// cancellation after the first look is seen at the next one.
    #[test]
    fn the_row_probe_looks_on_the_first_row_and_every_interval() -> TestResult {
        let tallies = DenseLaneTalliesV1::default();
        let live = RequestBudgetV1::unbounded();
        let watch = DenseLaneBudgetV1 {
            budget: &live,
            tallies: &tallies,
        };
        let mut probe = RowBudgetProbe::new(watch, DenseLaneKindV1::Exact);
        let mut passed = 0_usize;
        for _ in 0..THREE_INTERVALS_AND_ONE {
            probe.tick()?;
            passed = passed.saturating_add(1);
        }
        assert_eq!(passed, THREE_INTERVALS_AND_ONE);

        let cancelled = RequestBudgetV1::unbounded();
        cancelled.cancel_handle().cancel();
        let watch = DenseLaneBudgetV1 {
            budget: &cancelled,
            tallies: &tallies,
        };
        let mut probe = RowBudgetProbe::new(watch, DenseLaneKindV1::Approximate);
        let (code, message) = typed(&probe.tick())?;
        assert_eq!(code, REQUEST_CANCELLED_CODE);
        assert!(message.contains("checkpoint `semantic:ann`"), "{message}");
        assert_eq!(tallies.interruptions(DenseLaneKindV1::Approximate), 1);

        // Cancelled after the first look: exactly the rest of one interval
        // passes before the next look refuses.
        let late = RequestBudgetV1::unbounded();
        let watch = DenseLaneBudgetV1 {
            budget: &late,
            tallies: &tallies,
        };
        let mut probe = RowBudgetProbe::new(watch, DenseLaneKindV1::Exact);
        probe.tick()?;
        late.cancel_handle().cancel();
        let mut passed = 1_usize;
        let refused = loop {
            match probe.tick() {
                Ok(()) => passed = passed.saturating_add(1),
                Err(err) => break err,
            }
        };
        assert!(
            matches!(&refused, CoreError::Typed { code, .. } if *code == REQUEST_CANCELLED_CODE)
        );
        assert_eq!(passed, SEMANTIC_BUDGET_TICK_ROWS);
        assert_eq!(tallies.interruptions(DenseLaneKindV1::Exact), 1);
        Ok(())
    }

    /// The tallies scrape as one counter pair per lane, named by lane.
    #[test]
    fn the_tallies_scrape_one_counter_pair_per_lane() {
        let tallies = DenseLaneTalliesV1::default();
        tallies.count_query(DenseLaneKindV1::Approximate);
        tallies.count_query(DenseLaneKindV1::Approximate);
        tallies.count_query(DenseLaneKindV1::Exact);
        tallies.count_interruption(DenseLaneKindV1::Exact);
        tallies.count_exact_completion();
        let points: Vec<(String, MetricValueV1)> = tallies
            .scrape()
            .into_iter()
            .map(|point| (point.name, point.value))
            .collect();
        assert_eq!(
            points,
            vec![
                (
                    "semantic_dense_exact_completions_total".to_string(),
                    MetricValueV1::Counter(1)
                ),
                (
                    "semantic_dense_queries_ann_total".to_string(),
                    MetricValueV1::Counter(2)
                ),
                (
                    "semantic_budget_interruptions_ann_total".to_string(),
                    MetricValueV1::Counter(0)
                ),
                (
                    "semantic_dense_queries_exact_total".to_string(),
                    MetricValueV1::Counter(1)
                ),
                (
                    "semantic_budget_interruptions_exact_total".to_string(),
                    MetricValueV1::Counter(1)
                ),
            ]
        );
    }
}
