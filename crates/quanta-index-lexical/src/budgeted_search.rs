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
//! Block-WAND stays in force: the wrapper delegates `for_each_pruning` to
//! the inner weight and intercepts only its callback, so a top-k search
//! keeps skipping blocks exactly as before.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use quanta_index_core::{CoreError, RequestBudgetV1};
use tantivy::collector::Collector;
use tantivy::query::{EnableScoring, Explanation, Query, Scorer, Weight};
use tantivy::{DocId, DocSet, Score, Searcher, SegmentReader, TERMINATED};

/// Documents a scorer advances between two looks at the budget.
///
/// A look costs one `Instant::now()` and one atomic load; at this interval
/// it is noise against the postings work in between, and an interruption
/// is observed within a few microseconds of the interval's worth of docs.
pub(crate) const TICK_INTERVAL: u32 = 1_024;

/// One search's view of the request budget, shared by every scorer and
/// callback the search creates.
#[derive(Clone, Debug)]
pub(crate) struct BudgetProbe {
    budget: RequestBudgetV1,
    ticks: Arc<AtomicU32>,
    interrupted: Arc<AtomicBool>,
}

impl BudgetProbe {
    pub(crate) fn new(budget: &RequestBudgetV1) -> Self {
        Self {
            budget: budget.clone(),
            ticks: Arc::new(AtomicU32::new(0)),
            interrupted: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Count one unit of work and ask the budget on the first unit and
    /// then every [`TICK_INTERVAL`] units. `true` once the request is
    /// interrupted; sticky.
    pub(crate) fn tick(&self) -> bool {
        if self.interrupted.load(Ordering::Relaxed) {
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
        if self.interrupted.load(Ordering::Relaxed) {
            return true;
        }
        if self.budget.interruption().is_some() {
            self.interrupted.store(true, Ordering::Release);
            return true;
        }
        false
    }

    /// Whether any tick or look observed an interruption.
    pub(crate) fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Acquire)
    }

    /// The typed interruption for `stage`, once one was observed.
    ///
    /// An observed interruption is sticky in the budget too (cancellation
    /// never clears, a deadline never returns), so a probe that observed
    /// one and a budget that reports none is a defect this names rather
    /// than hides.
    pub(crate) fn interruption_error(&self, stage: &'static str) -> Option<CoreError> {
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
    let probe = BudgetProbe::new(budget);
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
    };
    let mut fruits = Vec::with_capacity(searcher.segment_readers().len());
    for (segment_ord, reader) in searcher.segment_readers().iter().enumerate() {
        if probe.observe() {
            break;
        }
        let segment_ord = u32::try_from(segment_ord).map_err(|err| {
            CoreError::Storage(format!("lexical: {stage}: segment ordinal overflow: {err}"))
        })?;
        fruits.push(
            collector
                .collect_segment(&weight, segment_ord, reader)
                .map_err(|err| CoreError::Storage(format!("lexical: {stage}: collect: {err}")))?,
        );
    }
    if let Some(interruption) = probe.interruption_error(stage) {
        return Err(interruption);
    }
    collector
        .merge_fruits(fruits)
        .map_err(|err| CoreError::Storage(format!("lexical: {stage}: merge: {err}")))
}

/// The inner weight with the probe on every path that walks postings.
struct BudgetedWeight {
    inner: Box<dyn Weight>,
    probe: BudgetProbe,
}

impl Weight for BudgetedWeight {
    fn scorer(&self, reader: &SegmentReader, boost: Score) -> tantivy::Result<Box<dyn Scorer>> {
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

    /// The pruning walk stays the inner weight's (block-WAND for term
    /// unions); only its callback is intercepted. Once the probe observes
    /// an interruption the callback answers with the highest threshold,
    /// under which no further block or document can score, so the inner
    /// walk skips the rest of the segment.
    fn for_each_pruning(
        &self,
        threshold: Score,
        reader: &SegmentReader,
        callback: &mut dyn FnMut(DocId, Score) -> Score,
    ) -> tantivy::Result<()> {
        let probe = &self.probe;
        self.inner
            .for_each_pruning(threshold, reader, &mut |doc, score| {
                if probe.tick() {
                    Score::MAX
                } else {
                    callback(doc, score)
                }
            })
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
        if self.probe.tick() {
            self.terminated = true;
            return TERMINATED;
        }
        self.inner.advance()
    }

    fn seek(&mut self, target: DocId) -> DocId {
        if self.terminated {
            return TERMINATED;
        }
        if self.probe.tick() {
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

    fn typed_code(err: &CoreError) -> Option<&str> {
        match err {
            CoreError::Typed { code, .. } => Some(code.as_str()),
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
