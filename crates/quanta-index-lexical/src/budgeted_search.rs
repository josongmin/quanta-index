//! Cooperative cancellation inside a native collect (W5 phase 2).
//!
//! A native search used to run to completion once it started; the request
//! budget (QI-BB-002) was observed before it and after it, never during.
//! This module wraps the compiled weight so the budget is observed while
//! the postings are walked: every matched document a pruning collector
//! sees, and every [`TICK_INTERVAL`] documents any scorer advances, the
//! probe asks the budget whether the request is still alive. Once it is
//! not, the scorer reports exhaustion and the pruning callback prunes
//! everything left, so the native call unwinds within one interval instead
//! of at the end of the corpus, and the caller answers with the typed
//! interruption naming this checkpoint. Between segments the budget is
//! asked outright.
//!
//! `TermQuery` retains native `BlockWAND`. Other weights use a budgeted scorer
//! because their public pruning callback does not guarantee early termination.
//! This sacrifices Boolean `TermUnion` block skipping; performance is unqualified.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use quanta_index_core::{
    CoreError, LexicalCollectionBudget, LexicalExecutionBudgetV1, LexicalMemoryReservation,
    RequestBudgetV1,
};
use tantivy::collector::{Collector, SegmentCollector};
use tantivy::query::{EmptyScorer, EnableScoring, Explanation, Query, Scorer, TermQuery, Weight};
use tantivy::{DocId, DocSet, Score, Searcher, SegmentReader, TERMINATED};

/// Documents a scorer advances between two looks at the budget.
///
/// A look costs one `Instant::now()` and one atomic load. This is an outer
/// scorer-action interval, not a wall-clock bound on an inner engine operation.
pub(crate) const TICK_INTERVAL: u32 = 1_024;

/// One exact-set collection's admission counter, shared across segments.
///
/// Charge before reading keys or growing a group map. The first excess match
/// is only a refusal probe: it is never materialized. The sticky stop is also
/// observed by the native scorer, independently of the cancellation interval.
/// `budgeted_collection` binds one request to all existing collector clones;
/// direct Tantivy callers without a request enforce resource limits only.
#[derive(Clone, Debug)]
pub(crate) struct CollectionBudget {
    policy: LexicalExecutionBudgetV1,
    pub(crate) resources: LexicalCollectionBudget,
    admitted: Arc<AtomicUsize>,
    exceeded: Arc<AtomicBool>,
    aborted: Arc<AtomicBool>,
    // Share request interruption with the native walk without owning its probe
    // or a collection handle. This state cannot form a reference cycle.
    request_probe: Arc<OnceLock<RequestProbe>>,
}

impl CollectionBudget {
    pub(crate) fn new(
        policy: LexicalExecutionBudgetV1,
        resources: LexicalCollectionBudget,
    ) -> Self {
        Self {
            policy,
            resources,
            admitted: Arc::new(AtomicUsize::new(0)),
            exceeded: Arc::new(AtomicBool::new(false)),
            aborted: Arc::new(AtomicBool::new(false)),
            request_probe: Arc::new(OnceLock::new()),
        }
    }

    fn bind_request(&self, budget: &RequestBudgetV1) -> Result<BudgetProbe, CoreError> {
        let probe = BudgetProbe::new(budget);
        self.request_probe
            .set(probe.request.clone())
            .map_err(|_rejected_probe| {
                CoreError::InvalidContract(
                    "lexical: a collection budget belongs to exactly one search".into(),
                )
            })?;
        Ok(probe)
    }

    pub(crate) fn admit(&self) -> bool {
        if self.stopped() {
            return false;
        }
        let Ok(_) = self
            .admitted
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                (count < self.policy.max_examined_candidates()).then(|| count.saturating_add(1))
            })
        else {
            self.exceeded.store(true, Ordering::Release);
            return false;
        };
        true
    }

    pub(crate) fn stopped(&self) -> bool {
        self.exceeded.load(Ordering::Acquire)
            || self.aborted.load(Ordering::Acquire)
            || self.resources.failure().is_some()
            || self.request_probe.get().is_some_and(RequestProbe::observe)
    }

    /// Stop walking after a collector integrity error; preserve that error in
    /// its fruit instead of reclassifying it as a resource refusal.
    pub(crate) fn abort(&self) {
        self.aborted.store(true, Ordering::Release);
    }

    pub(crate) fn error(&self, surface: &'static str) -> Option<CoreError> {
        self.exceeded
            .load(Ordering::Acquire)
            .then(|| self.policy.exceeded(surface))
            .or_else(|| self.resources.failure())
            .or_else(|| {
                self.request_probe
                    .get()
                    .and_then(|probe| probe.error(surface))
            })
    }

    /// Observe the shared request before harvest/merge work or allocation.
    /// An earlier integrity/resource stop remains authoritative.
    pub(crate) fn checkpoint(&self) -> tantivy::Result<()> {
        if !self.stopped() {
            return Ok(());
        }
        Err(tantivy::TantivyError::InvalidArgument(
            self.error("lexical:collection").map_or_else(
                || "lexical collection was aborted".to_string(),
                |error| error.to_string(),
            ),
        ))
    }

    pub(crate) fn charge_work(&self, units: u64) -> tantivy::Result<()> {
        self.checkpoint()?;
        self.resources
            .charge_work(units)
            .map_err(|error| tantivy::TantivyError::InvalidArgument(error.to_string()))
    }

    pub(crate) fn reserve_bytes(&self, bytes: usize) -> tantivy::Result<LexicalMemoryReservation> {
        self.checkpoint()?;
        let bytes = u64::try_from(bytes).map_err(|error| {
            tantivy::TantivyError::InvalidArgument(format!(
                "collection byte size overflow: {error}"
            ))
        })?;
        self.resources
            .reserve_bytes(bytes)
            .map_err(|error| tantivy::TantivyError::InvalidArgument(error.to_string()))
    }

    /// Reserve the search wrapper's fruit carrier before it enters a Tantivy
    /// collector. Preserve the typed request/resource error at this boundary.
    fn reserve_search_bytes(
        &self,
        bytes: u64,
        stage: &'static str,
    ) -> Result<LexicalMemoryReservation, CoreError> {
        if self.stopped() {
            return Err(self.error(stage).unwrap_or_else(|| {
                CoreError::Storage("lexical collection was aborted".to_string())
            }));
        }
        self.resources.reserve_bytes(bytes)
    }

    /// Conservative node-layout admission for the pinned Rust 1.92 `BTreeMap`:
    /// at most 11 key/value slots, 12 edges, parent/index/length and padding.
    /// Insert-only maps allocate no more nodes than admitted entries. Keep
    /// leases outside the map until its nodes, including a drain, are dropped.
    pub(crate) fn reserve_map_entry<K, V>(&self) -> tantivy::Result<LexicalMemoryReservation> {
        let pointer = std::mem::size_of::<usize>();
        let alignment = std::mem::align_of::<K>()
            .max(std::mem::align_of::<V>())
            .max(std::mem::align_of::<usize>());
        let bytes = std::mem::size_of::<K>()
            .checked_add(std::mem::size_of::<V>())
            .and_then(|size| size.checked_mul(12))
            .and_then(|size| {
                pointer
                    .checked_mul(16)
                    .and_then(|overhead| size.checked_add(overhead))
            })
            .and_then(|size| {
                alignment
                    .checked_mul(8)
                    .and_then(|padding| size.checked_add(padding))
            })
            .ok_or_else(|| {
                tantivy::TantivyError::InvalidArgument("collection map layout overflow".to_string())
            })?;
        self.reserve_bytes(bytes)
    }
}

/// Shared interruption authority for native traversal and collection phases.
/// It owns no collection or traversal handle.
#[derive(Clone, Debug)]
struct RequestProbe {
    budget: RequestBudgetV1,
    interrupted: Arc<AtomicBool>,
}

impl RequestProbe {
    fn observe(&self) -> bool {
        if self.interrupted.load(Ordering::Relaxed) {
            return true;
        }
        if self.budget.interruption().is_some() {
            self.interrupted.store(true, Ordering::Release);
            return true;
        }
        false
    }

    fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire)
    }

    fn error(&self, stage: &'static str) -> Option<CoreError> {
        if !self.interrupted() {
            return None;
        }
        Some(self.budget.interrupted_at(stage).unwrap_or_else(|| {
            CoreError::Storage(format!(
                "lexical: `{stage}` observed an interruption the request budget no longer reports"
            ))
        }))
    }
}

/// One search's view of the request budget, shared by every scorer and
/// callback the search creates.
#[derive(Clone, Debug)]
pub(crate) struct BudgetProbe {
    request: RequestProbe,
    ticks: Arc<AtomicU32>,
    collection: Option<CollectionBudget>,
}

impl BudgetProbe {
    pub(crate) fn new(budget: &RequestBudgetV1) -> Self {
        Self {
            request: RequestProbe {
                budget: budget.clone(),
                interrupted: Arc::new(AtomicBool::new(false)),
            },
            ticks: Arc::new(AtomicU32::new(0)),
            collection: None,
        }
    }

    /// Count one unit of work and ask the budget on the first unit and
    /// then every [`TICK_INTERVAL`] units. `true` once the request is
    /// interrupted; sticky.
    pub(crate) fn tick(&self) -> bool {
        if self
            .collection
            .as_ref()
            .is_some_and(CollectionBudget::stopped)
        {
            return true;
        }
        if self.interrupted() {
            return true;
        }
        let prior = self.ticks.fetch_add(1, Ordering::Relaxed);
        if !prior.is_multiple_of(TICK_INTERVAL) {
            return false;
        }
        self.observe()
    }

    /// Ask the budget now. `true` once the request is interrupted; sticky.
    pub(crate) fn observe(&self) -> bool {
        if self
            .collection
            .as_ref()
            .is_some_and(CollectionBudget::stopped)
        {
            return true;
        }
        self.request.observe()
    }

    /// Whether any tick or look observed an interruption.
    pub(crate) fn interrupted(&self) -> bool {
        self.request.interrupted()
    }

    /// Charge an attempted native visit before asking the inner scorer to
    /// initialize or advance. Terminal probes are conservatively charged too.
    fn admit_visit(&self) -> bool {
        self.collection
            .as_ref()
            .is_none_or(|collection| collection.charge_work(1).is_ok())
    }

    fn error(&self, stage: &'static str) -> Option<CoreError> {
        self.interruption_error(stage).or_else(|| {
            self.collection
                .as_ref()
                .and_then(|collection| collection.error(stage))
        })
    }

    /// The typed interruption for `stage`, once one was observed.
    ///
    /// An observed interruption is sticky in the budget too (cancellation
    /// never clears, a deadline never returns), so a probe that observed
    /// one and a budget that reports none is a defect this names rather
    /// than hides.
    pub(crate) fn interruption_error(&self, stage: &'static str) -> Option<CoreError> {
        self.request.error(stage)
    }
}

/// Run `query` under `collector` on `searcher`, observing `budget` inside
/// the collect and between segments.
///
/// This is [`Searcher::search`] with the budget probe threaded through
/// the weight; segments are visited in order on the calling thread, as the
/// index's default executor does. An interruption answers typed, naming
/// `stage`, and the partial fruit is discarded.
pub(crate) fn budgeted_search<C: Collector>(
    searcher: &Searcher,
    query: &dyn Query,
    collector: &C,
    budget: &RequestBudgetV1,
    stage: &'static str,
) -> Result<C::Fruit, CoreError> {
    search_with_probe(
        searcher,
        query,
        collector,
        &BudgetProbe::new(budget),
        Ok,
        stage,
    )
}

/// Exact-set collection refuses partial success on resource exhaustion.
///
/// A collector's refusal stops scoring and discards every fruit before merge.
/// Fallible child fruits are checked at the segment boundary, so an integrity
/// error cannot reach later collection or merge work that might mask its cause.
pub(crate) fn budgeted_collection<C: Collector>(
    searcher: &Searcher,
    query: &dyn Query,
    collector: &C,
    budget: &RequestBudgetV1,
    collection: CollectionBudget,
    stage: &'static str,
) -> Result<C::Fruit, CoreError>
where
    C::Child: SegmentCollector<Fruit = tantivy::Result<C::Fruit>>,
{
    let mut probe = collection.bind_request(budget)?;
    probe.collection = Some(collection);
    search_with_probe(
        searcher,
        query,
        collector,
        &probe,
        |fruit| fruit.map(Ok),
        stage,
    )
}

type SegmentFruit<C> = <<C as Collector>::Child as SegmentCollector>::Fruit;

fn search_with_probe<C: Collector>(
    searcher: &Searcher,
    query: &dyn Query,
    collector: &C,
    probe: &BudgetProbe,
    validate_segment: impl Fn(SegmentFruit<C>) -> tantivy::Result<SegmentFruit<C>>,
    stage: &'static str,
) -> Result<C::Fruit, CoreError> {
    // Refuse an already interrupted request before weight construction or any
    // collection allocation. Otherwise a byte limit can mask its typed reason.
    let _stopped = probe.observe();
    if let Some(error) = probe.error(stage) {
        return Err(error);
    }
    // Reserve the concurrently retained segment-fruit vector before allocating
    // it. Payloads retained by each fruit require their own collector guards.
    let _fruit_memory = match &probe.collection {
        Some(collection) => {
            let bytes = searcher
                .segment_readers()
                .len()
                .checked_mul(std::mem::size_of::<
                    <C::Child as tantivy::collector::SegmentCollector>::Fruit,
                >())
                .ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: segment-fruit buffer size overflow".to_string(),
                    )
                })?;
            let bytes = u64::try_from(bytes).map_err(|error| {
                CoreError::InvalidContract(format!(
                    "lexical: segment-fruit byte size overflow: {error}"
                ))
            })?;
            Some(collection.reserve_search_bytes(bytes, stage)?)
        }
        None => None,
    };
    let enable_scoring = if collector.requires_scoring() {
        EnableScoring::enabled_from_searcher(searcher)
    } else {
        EnableScoring::disabled_from_searcher(searcher)
    };
    let weight = BudgetedWeight {
        inner: query
            .weight(enable_scoring)
            .map_err(|err| CoreError::Storage(format!("lexical: {stage}: weight: {err}")))?,
        probe: probe.clone(),
        native_term: query
            .as_any()
            .downcast_ref::<TermQuery>()
            .map(|query| query.term().clone()),
    };
    let mut fruits = Vec::new();
    fruits
        .try_reserve_exact(searcher.segment_readers().len())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: {stage}: segment-fruit allocation failed: {error}"
            ))
        })?;
    for (segment_ord, reader) in searcher.segment_readers().iter().enumerate() {
        if probe.observe() {
            break;
        }
        let segment_ord = u32::try_from(segment_ord).map_err(|err| {
            CoreError::Storage(format!("lexical: {stage}: segment ordinal overflow: {err}"))
        })?;
        let fruit = collector
            .collect_segment(&weight, segment_ord, reader)
            .and_then(&validate_segment);
        if let Some(error) = probe.error(stage) {
            return Err(error);
        }
        fruits.push(
            fruit.map_err(|err| CoreError::Storage(format!("lexical: {stage}: collect: {err}")))?,
        );
    }
    if let Some(error) = probe.error(stage) {
        return Err(error);
    }
    let result = collector.merge_fruits(fruits);
    let _stopped = probe.observe();
    if let Some(error) = probe.error(stage) {
        return Err(error);
    }
    result.map_err(|err| CoreError::Storage(format!("lexical: {stage}: merge: {err}")))
}

/// The inner weight with the probe on every path that walks postings.
struct BudgetedWeight {
    native_term: Option<tantivy::Term>,
    inner: Box<dyn Weight>,
    probe: BudgetProbe,
}

impl Weight for BudgetedWeight {
    fn scorer(&self, reader: &SegmentReader, boost: Score) -> tantivy::Result<Box<dyn Scorer>> {
        if !self.probe.admit_visit() {
            return Ok(Box::new(EmptyScorer));
        }
        Ok(Box::new(BudgetedScorer {
            inner: self.inner.scorer(reader, boost)?,
            probe: self.probe.clone(),
            terminated: false,
        }))
    }

    fn explain(&self, reader: &SegmentReader, doc: DocId) -> tantivy::Result<Explanation> {
        self.inner.explain(reader, doc)
    }

    // `count`, `for_each` and `for_each_no_score` keep their default
    // bodies, which drive `self.scorer()` and so the probe.

    /// `TermQuery` retains block-WAND after admission of its full postings upper
    /// bound. Other weights may ignore MAX while walking their scorer, so drive
    /// their scorer directly and charge every attempted native visit.
    fn for_each_pruning(
        &self,
        threshold: Score,
        reader: &SegmentReader,
        callback: &mut dyn FnMut(DocId, Score) -> Score,
    ) -> tantivy::Result<()> {
        if let Some(term) = &self.native_term {
            let probe = &self.probe;
            if probe.observe() {
                return Ok(());
            }
            if let Some(collection) = &probe.collection {
                // Native block-WAND may score below-threshold rows without a
                // callback, and finishes its current block after MAX. Admit a
                // conservative complete postings walk before entering it.
                // doc_freq includes deleted postings; one extra terminal probe
                // is charged. Skipped blocks do not refund admitted work.
                let doc_freq = reader
                    .inverted_index(term.field())?
                    .get_term_info(term)?
                    .map_or(0, |info| info.doc_freq);
                collection.charge_work(u64::from(doc_freq).saturating_add(1))?;
            }
            return self
                .inner
                .for_each_pruning(threshold, reader, &mut |doc, score| {
                    if probe.tick() {
                        return Score::MAX;
                    }
                    let next = callback(doc, score);
                    if probe.observe() { Score::MAX } else { next }
                });
        }
        let mut scorer = self.scorer(reader, 1.0)?;
        let mut threshold = threshold;
        while scorer.doc() != TERMINATED {
            let score = scorer.score();
            if score > threshold {
                threshold = callback(scorer.doc(), score);
            }
            let _next = scorer.advance();
        }
        Ok(())
    }
}

/// The inner scorer, exhausted early once the probe observes an
/// interruption.
struct BudgetedScorer {
    inner: Box<dyn Scorer>,
    probe: BudgetProbe,
    /// Set once this scorer reported `TERMINATED` for the budget's sake, so
    /// `doc()` agrees with what `advance()`/`seek()` said.
    terminated: bool,
}

impl DocSet for BudgetedScorer {
    fn advance(&mut self) -> DocId {
        if self.terminated {
            return TERMINATED;
        }
        if self.probe.tick() || !self.probe.admit_visit() {
            self.terminated = true;
            return TERMINATED;
        }
        self.inner.advance()
    }

    fn seek(&mut self, target: DocId) -> DocId {
        if self.terminated {
            return TERMINATED;
        }
        if self.probe.tick() || !self.probe.admit_visit() {
            self.terminated = true;
            return TERMINATED;
        }
        self.inner.seek(target)
    }

    fn doc(&self) -> DocId {
        if self.terminated {
            TERMINATED
        } else {
            self.inner.doc()
        }
    }

    fn size_hint(&self) -> u32 {
        self.inner.size_hint()
    }
}

impl Scorer for BudgetedScorer {
    fn score(&mut self) -> Score {
        self.inner.score()
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "Result-returning tests use assertions as test-failure reporting; the Result carries fixture errors"
    )]
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use quanta_index_core::{
        CoreError, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE, RequestBudgetV1,
    };
    use tantivy::collector::{Count, TopDocs};
    use tantivy::query::{
        AllQuery, BooleanQuery, EnableScoring, Explanation, Occur, Query, Scorer, TermQuery, Weight,
    };
    use tantivy::schema::{IndexRecordOption, STORED, Schema, TEXT};
    use tantivy::{
        DocId, DocSet, Index, IndexWriter, Score, SegmentReader, TERMINATED, TantivyDocument, Term,
        doc,
    };

    use super::{BudgetProbe, TICK_INTERVAL, budgeted_search};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn collection_request_checkpoints_precede_work_and_byte_refusal() -> TestResult {
        use quanta_index_core::{LexicalCollectionBudget, LexicalExecutionBudgetV1};
        use std::time::{Duration, Instant};

        let cancelled = RequestBudgetV1::unbounded();
        cancelled.cancel_handle().cancel();
        let expired = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .ok_or("clock underflow")?;
        for (request, code) in [
            (cancelled, REQUEST_CANCELLED_CODE),
            (
                RequestBudgetV1::until(expired),
                REQUEST_DEADLINE_EXCEEDED_CODE,
            ),
        ] {
            let resources = LexicalCollectionBudget::new(1, 1)?;
            let collection =
                super::CollectionBudget::new(LexicalExecutionBudgetV1::new(1)?, resources.clone());
            let binding = Arc::downgrade(&collection.request_probe);
            let mut probe = collection.bind_request(&request)?;
            probe.collection = Some(collection.clone());
            assert!(collection.reserve_bytes(2).is_err());
            assert!(collection.charge_work(2).is_err());
            assert!(!collection.admit());
            assert!(
                matches!(probe.interruption_error("test:bound-collection"), Some(CoreError::Typed { code: actual, .. }) if actual == code)
            );
            assert!(
                matches!(collection.error("test:bound-collection"), Some(CoreError::Typed { code: actual, .. }) if actual == code)
            );
            assert_eq!(resources.used_work(), 0);
            assert_eq!(resources.peak_bytes(), 0);
            assert!(resources.failure().is_none());
            drop(probe);
            drop(collection);
            assert!(
                binding.upgrade().is_none(),
                "request binding must not form a cycle"
            );
        }
        Ok(())
    }

    #[test]
    fn fruit_carrier_reservation_observes_bound_cancellation() -> TestResult {
        use quanta_index_core::{LexicalCollectionBudget, LexicalExecutionBudgetV1};

        let request = RequestBudgetV1::unbounded();
        let resources = LexicalCollectionBudget::new(1, 1)?;
        let collection =
            super::CollectionBudget::new(LexicalExecutionBudgetV1::new(1)?, resources.clone());
        let _probe = collection.bind_request(&request)?;
        request.cancel_handle().cancel();
        let result = collection.reserve_search_bytes(2, "test:fruit-carrier");
        assert!(
            matches!(&result, Err(CoreError::Typed { code, .. }) if *code == REQUEST_CANCELLED_CODE),
            "cancelled carrier must keep its typed reason: {result:?}"
        );
        assert_eq!(resources.peak_bytes(), 0);
        assert!(resources.failure().is_none());
        Ok(())
    }

    /// A small index of `docs` documents in one segment.
    fn index_with(docs: u32) -> Result<Index, Box<dyn std::error::Error>> {
        let mut schema = Schema::builder();
        let body = schema.add_text_field("body", TEXT | STORED);
        let index = Index::create_in_ram(schema.build());
        let mut writer: IndexWriter<TantivyDocument> = index.writer(15_000_000)?;
        for n in 0..docs {
            let _op = writer.add_document(doc!(body => format!("needle doc {n}")))?;
        }
        let _commit = writer.commit()?;
        Ok(index)
    }

    fn typed_code(err: &CoreError) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
        match err {
            CoreError::Typed { code, .. } => Some(*code),
            CoreError::InvalidContract(_)
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_) => None,
        }
    }

    /// A query that matches every document of the segment and counts how
    /// many times its scorer was advanced, so a test can see where the
    /// walk stopped.
    #[derive(Clone, Debug)]
    struct CountingQuery {
        docs: u32,
        advanced: Arc<AtomicU32>,
    }

    impl Query for CountingQuery {
        fn weight(&self, _enable_scoring: EnableScoring<'_>) -> tantivy::Result<Box<dyn Weight>> {
            Ok(Box::new(CountingWeight {
                docs: self.docs,
                advanced: Arc::clone(&self.advanced),
            }))
        }
    }

    struct CountingWeight {
        docs: u32,
        advanced: Arc<AtomicU32>,
    }

    impl Weight for CountingWeight {
        fn scorer(
            &self,
            _reader: &SegmentReader,
            _boost: Score,
        ) -> tantivy::Result<Box<dyn Scorer>> {
            Ok(Box::new(CountingScorer {
                doc: 0,
                docs: self.docs,
                advanced: Arc::clone(&self.advanced),
            }))
        }

        fn explain(&self, _reader: &SegmentReader, _doc: DocId) -> tantivy::Result<Explanation> {
            Ok(Explanation::new("counting", 1.0))
        }
    }

    struct CountingScorer {
        doc: DocId,
        docs: u32,
        advanced: Arc<AtomicU32>,
    }

    impl DocSet for CountingScorer {
        fn advance(&mut self) -> DocId {
            let _prior = self.advanced.fetch_add(1, Ordering::Relaxed);
            self.doc = self.doc.saturating_add(1);
            self.doc()
        }

        fn doc(&self) -> DocId {
            if self.doc < self.docs {
                self.doc
            } else {
                TERMINATED
            }
        }

        fn size_hint(&self) -> u32 {
            self.docs
        }
    }

    impl Scorer for CountingScorer {
        fn score(&mut self) -> Score {
            1.0
        }
    }

    /// A budget that is not interrupted runs the search to its full
    /// result through every collector shape.
    #[test]
    fn an_unbounded_budget_collects_everything() -> TestResult {
        let index = index_with(5_000)?;
        let searcher = index.reader()?.searcher();
        let budget = RequestBudgetV1::unbounded();
        let count = budgeted_search(&searcher, &AllQuery, &Count, &budget, "test:count")?;
        assert_eq!(count, 5_000);
        let (count, hits) = budgeted_search(
            &searcher,
            &AllQuery,
            &(Count, TopDocs::with_limit(10)),
            &budget,
            "test:top",
        )?;
        assert_eq!((count, hits.len()), (5_000, 10));
        Ok(())
    }

    /// The probe looks at the budget on its first tick and then every
    /// interval, and an observation is sticky.
    #[test]
    fn the_probe_looks_on_the_first_tick_and_every_interval() {
        let budget = RequestBudgetV1::unbounded();
        let probe = BudgetProbe::new(&budget);
        assert!(!probe.tick(), "a live budget is not an interruption");
        budget.cancel_handle().cancel();
        // Ticks 2..INTERVAL do not look; the next interval boundary does.
        let mut observed_at = None;
        for tick in 2..=TICK_INTERVAL.saturating_add(1) {
            if probe.tick() {
                observed_at = Some(tick);
                break;
            }
        }
        assert_eq!(observed_at, Some(TICK_INTERVAL.saturating_add(1)));
        assert!(probe.interrupted() && probe.tick(), "sticky once observed");
        assert_eq!(
            probe.interruption_error("lexical:collect").map(|err| err.to_string()),
            Some(
                "typed failure REQUEST_CANCELLED: request cancelled by its peer; observed at checkpoint `lexical:collect`"
                    .to_string()
            )
        );
    }

    /// A budget cancelled before the search starts is observed inside the
    /// collect.
    ///
    /// The answer is the typed cancellation naming the stage, and the
    /// scorer was advanced at most one interval's worth of documents, not
    /// the whole corpus.
    #[test]
    fn a_cancelled_budget_stops_a_count_walk_within_one_interval() -> TestResult {
        let index = index_with(1)?;
        let searcher = index.reader()?.searcher();
        let budget = RequestBudgetV1::unbounded();
        budget.cancel_handle().cancel();
        let advanced = Arc::new(AtomicU32::new(0));
        let query = CountingQuery {
            docs: 50_000,
            advanced: Arc::clone(&advanced),
        };
        let err = budgeted_search(&searcher, &query, &Count, &budget, "lexical:collect")
            .err()
            .ok_or("a cancelled budget refuses")?;
        assert_eq!(typed_code(&err), Some(REQUEST_CANCELLED_CODE));
        assert!(
            err.to_string().contains("checkpoint `lexical:collect`"),
            "{err}"
        );
        let walked = advanced.load(Ordering::Relaxed);
        assert!(
            walked <= TICK_INTERVAL,
            "the walk stopped within one interval: advanced {walked} of 50000"
        );
        // The control: the same query under a live budget walks everything.
        let advanced = Arc::new(AtomicU32::new(0));
        let query = CountingQuery {
            docs: 50_000,
            advanced: Arc::clone(&advanced),
        };
        let count = budgeted_search(
            &searcher,
            &query,
            &Count,
            &RequestBudgetV1::unbounded(),
            "lexical:collect",
        )?;
        assert_eq!((count, advanced.load(Ordering::Relaxed)), (50_000, 50_000));
        Ok(())
    }

    /// A passed deadline is observed through the pruning path a top-k
    /// collector uses over a real term union (block-WAND stays in force
    /// and is pruned to nothing once the interruption is observed).
    #[test]
    fn a_passed_deadline_interrupts_a_top_k_term_union() -> TestResult {
        let index = index_with(20_000)?;
        let searcher = index.reader()?.searcher();
        let body = index.schema().get_field("body")?;
        let needle: Box<dyn Query> = Box::new(TermQuery::new(
            Term::from_field_text(body, "needle"),
            IndexRecordOption::WithFreqs,
        ));
        let doc_term: Box<dyn Query> = Box::new(TermQuery::new(
            Term::from_field_text(body, "doc"),
            IndexRecordOption::WithFreqs,
        ));
        let union = BooleanQuery::new(vec![(Occur::Should, needle), (Occur::Should, doc_term)]);
        let live = budgeted_search(
            &searcher,
            &union,
            &TopDocs::with_limit(10),
            &RequestBudgetV1::unbounded(),
            "lexical:collect",
        )?;
        assert_eq!(live.len(), 10);
        let deadline = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .ok_or("clock underflow")?;
        let err = budgeted_search(
            &searcher,
            &union,
            &TopDocs::with_limit(10),
            &RequestBudgetV1::until(deadline),
            "lexical:collect",
        )
        .err()
        .ok_or("a passed deadline refuses")?;
        assert_eq!(typed_code(&err), Some(REQUEST_DEADLINE_EXCEEDED_CODE));
        assert!(
            err.to_string().contains("checkpoint `lexical:collect`"),
            "{err}"
        );
        Ok(())
    }
}
