//! Same-view History execution. Recency reuses the canonical selector; both
//! orders use the canonical row predicates. BM25 populations are independently
//! the complete commit and diff sets of this immutable member, before filters.

use super::*;
use crate::query_dispatcher::routes::history::{
    HistoryPageRow, HistoryPageSelection, HistoryPageSelector, ensure_cursor_continues_order,
    ensure_history_shards_ready, select_history_recency, validate_history_query,
};
use crate::query_dispatcher::routes::history_records::{
    HistoryDiffText, HistoryQueryKind, ensure_cursor_kind, history_commit_matches_with,
    history_diff_matches_with, history_diff_snippet_text, resolve_history_query_kind,
};
use quanta_index_contract::{
    CandidateCountV1, CommitCandidate, DiffCandidate, HistoryCursor, HistoryCursorOrderV1,
    HistoryOrderV1, HistoryScoreV1, LqExpr, LqLeaf, LqPatternType, LqQuery, QueryResultWindowV1,
};
use quanta_index_lq_text_normalizer::query_tokens;
use std::collections::BTreeMap;
use std::ops::ControlFlow;

mod matching;
mod phrase_automaton;
use matching::BoundedHistoryMatcher;
use phrase_automaton::PhraseAutomaton;

/// Positional boundary only. C's wire cursor additionally binds this view,
/// artifact, exact lowered query/options/order and requested page size.
pub struct NativeHistoryPageV1 {
    pub view: ViewRef,
    pub instance: quanta_index_contract::index::IndexInstanceId,
    pub artifact: ObjectRef,
    pub epoch: AuxEpochV1,
    pub commits: Vec<CommitCandidate>,
    pub diffs: Vec<DiffCandidate>,
    /// All eligible rows of this kind, including those before the boundary.
    pub matched: u64,
    /// Eligible rows strictly after the validated boundary, before page cut.
    pub remaining: u64,
    pub window: QueryResultWindowV1,
    pub examined: u64,
    pub next_cursor: Option<HistoryCursor>,
}

/// DATA lifetime extends through response write. Borrow `page()` for encoding;
/// dropping an execution slot must not drop this result's retained grants.
pub struct NativeHistoryQueryResultV1 {
    page: NativeHistoryPageV1,
    _read: NativeHistoryReadV1,
    _result_memory: Option<MemoryLease>,
}

impl NativeHistoryQueryResultV1 {
    #[must_use]
    pub const fn page(&self) -> &NativeHistoryPageV1 {
        &self.page
    }
}

impl NativeHistoryReadV1 {
    /// The service validates the original ticket before/after this call and
    /// retains the same original request/attempt until response settlement.
    /// A Legacy generation token is neither accepted nor manufactured here.
    pub fn query(
        self,
        expected_view: ViewRef,
        query: &LqQuery,
        order: HistoryOrderV1,
        top_k: u32,
        boundary: Option<&HistoryCursor>,
        budget: &mut WorkBudget,
    ) -> Result<NativeHistoryQueryResultV1, CoreError> {
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        if expected_view != self.view {
            return Err(invalid("History query view differs from retained ticket"));
        }
        let top_k = quanta_index_core::validate_query_top_k(top_k)?;
        validate_history_query(query)?;
        ensure_history_shards_ready(&self.source.state, query)?;
        ensure_cursor_continues_order(order, boundary)?;
        let kind = resolve_history_query_kind(query)?;
        ensure_cursor_kind(kind, boundary)?;
        if boundary.is_some_and(|boundary| boundary.aux_epoch != self.source.cut.durable_epoch) {
            return Err(invalid(
                "History boundary epoch differs from retained member",
            ));
        }
        let plan_bytes = quanta_index_ipc::cbor_payload_len_with_progress_v1(query, &mut |bytes| {
            budget.charge(WorkUsage {
                bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })
        })
        .map_err(|error| match error {
            CborPayloadLengthProgressErrorV1::Progress(error) => history_work_error(budget, error),
            CborPayloadLengthProgressErrorV1::Codec(error) => invalid(&error.to_string()),
        })?;
        let plan_bound = plan_bytes
            .checked_mul(128)
            .and_then(|bytes| bytes.checked_add(65_536))
            .ok_or_else(|| invalid("History query plan bound overflow"))?;
        let _plan_memory = budget
            .reserve_memory(plan_bound, MemoryLifetime::Temporary)
            .map_err(|error| history_work_error(budget, error))?;
        let state = &self.source.state;
        let rows = match kind {
            HistoryQueryKind::Commit => state.commits().len(),
            HistoryQueryKind::Diff => state.diff_hunks().len(),
        };
        let capacity = rows.min(top_k as usize);
        let scoring = match order {
            HistoryOrderV1::Recency => None,
            HistoryOrderV1::Relevance => {
                let mut scoring = Scoring::new(query)?;
                // BM25 statistics use the complete population before filters.
                visit_documents(state, kind, budget, |row, budget| {
                    with_document_text(row, budget, |text, budget| {
                        scoring.observe(text, query, budget)
                    })
                })?;
                Some(scoring)
            }
        };
        if let Some(boundary) = boundary {
            validate_anchor(state, query, kind, boundary, scoring.as_ref(), budget)?;
        }
        let rank_bound = HistoryPageSelector::heap_backing_bytes(capacity)
            .ok_or_else(|| invalid("History rank bound overflow"))?;
        let _rank_memory = if rank_bound == 0 {
            None
        } else {
            budget
                .reserve_memory(rank_bound, MemoryLifetime::Temporary)
                .map_err(|error| history_work_error(budget, error))?
        };
        let selection = if let Some(scoring) = scoring {
            select_relevance(
                state,
                query,
                kind,
                self.source.cut.durable_epoch,
                capacity,
                boundary,
                &scoring,
                budget,
            )?
        } else {
            select_history_recency(
                query,
                state,
                self.source.cut.durable_epoch,
                top_k,
                boundary,
                &mut BoundedHistoryMatcher::new(budget),
            )?
        };
        let result_bound = selection.rows().try_fold(
            u64::try_from(selection.cursor_path_len())
                .map_err(|_| invalid("History cursor backing overflow"))?,
            |bytes, row| {
                bytes
                    .checked_add(candidate_backing(row, budget)?)
                    .ok_or_else(|| invalid("History response backing overflow"))
            },
        )?;
        let result_memory = if result_bound == 0 {
            None
        } else {
            budget
                .reserve_memory(result_bound, MemoryLifetime::Retained)
                .map_err(|error| history_work_error(budget, error))?
        };
        let matched = selection.matched();
        let page = selection.project();
        let remaining = match page.window.candidate_count() {
            CandidateCountV1::Exact(count) => count,
            _ => return Err(invalid("History producer did not return an exact window")),
        };
        let result = NativeHistoryPageV1 {
            view: self.view,
            instance: self.instance,
            artifact: self.artifact,
            epoch: self.source.cut.durable_epoch,
            commits: page.commits,
            diffs: page.diffs,
            matched,
            remaining,
            window: page.window,
            examined: page.examined,
            next_cursor: page.next_cursor,
        };
        budget
            .checkpoint()
            .map_err(|error| history_work_error(budget, error))?;
        Ok(NativeHistoryQueryResultV1 {
            page: result,
            _read: self,
            _result_memory: result_memory,
        })
    }
}

fn candidate_backing(row: HistoryPageRow<'_>, budget: &mut WorkBudget) -> Result<u64, CoreError> {
    let overflow = || invalid("History response backing overflow");
    let bytes = match row {
        HistoryPageRow::Commit(record) => {
            let tag_slots = record
                .tags
                .len()
                .checked_mul(std::mem::size_of::<String>())
                .ok_or_else(overflow)?;
            let parent_slots = record
                .parents
                .len()
                .checked_mul(std::mem::size_of::<quanta_index_contract::lex::CommitSha>())
                .ok_or_else(overflow)?;
            let tag_text = record.tags.iter().try_fold(0_usize, |bytes, tag| {
                charge_row(budget)?;
                bytes.checked_add(tag.len()).ok_or_else(overflow)
            })?;
            [
                std::mem::size_of::<CommitCandidate>(),
                parent_slots,
                tag_slots,
                tag_text,
                record.author.len(),
                record.committer.len(),
                record.message.len(),
            ]
            .into_iter()
            .try_fold(0_usize, usize::checked_add)
        }
        HistoryPageRow::Diff(key, record, _) => [
            std::mem::size_of::<DiffCandidate>(),
            key.file_path().len(),
            record.hunk_header.len(),
            history_diff_snippet_text(record).len(),
        ]
        .into_iter()
        .try_fold(0_usize, usize::checked_add),
    }
    .ok_or_else(overflow)?;
    u64::try_from(bytes).map_err(|_| overflow())
}

fn charge_row(budget: &mut WorkBudget) -> Result<(), CoreError> {
    budget
        .charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))
}

fn with_document_text<T>(
    row: HistoryPageRow<'_>,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&str, &mut WorkBudget) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    match row {
        HistoryPageRow::Commit(record) => visit(&record.message, budget),
        HistoryPageRow::Diff(key, record, _) => {
            let mut diff = HistoryDiffText::new(key, record);
            let text = diff.get(&mut BoundedHistoryMatcher::new(budget))?;
            visit(text, budget)
        }
    }
}

fn visit_documents<'a>(
    state: &'a HistoryAuthorityState,
    kind: HistoryQueryKind,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(HistoryPageRow<'a>, &mut WorkBudget) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    match kind {
        HistoryQueryKind::Commit => {
            for record in state.commits().values() {
                charge_row(budget)?;
                visit(HistoryPageRow::Commit(record), budget)?;
            }
        }
        HistoryQueryKind::Diff => {
            for (key, record) in state.diff_hunks() {
                charge_row(budget)?;
                let commit = state
                    .commits()
                    .get(&key.commit_sha())
                    .ok_or_else(|| invalid("History diff commit missing"))?;
                visit(HistoryPageRow::Diff(key, record, commit), budget)?;
            }
        }
    }
    Ok(())
}

fn admits(
    state: &HistoryAuthorityState,
    query: &LqQuery,
    row: HistoryPageRow<'_>,
    budget: &mut WorkBudget,
) -> Result<bool, CoreError> {
    let mut matcher = BoundedHistoryMatcher::new(budget);
    match row {
        HistoryPageRow::Commit(record) => {
            history_commit_matches_with(query, state, record, &mut matcher)
        }
        HistoryPageRow::Diff(key, record, commit) => {
            history_diff_matches_with(query, state, key, record, commit, &mut matcher)
        }
    }
}

fn validate_anchor(
    state: &HistoryAuthorityState,
    query: &LqQuery,
    kind: HistoryQueryKind,
    boundary: &HistoryCursor,
    scoring: Option<&Scoring>,
    budget: &mut WorkBudget,
) -> Result<(), CoreError> {
    let commit = state
        .commits()
        .get(&boundary.sha)
        .ok_or_else(|| invalid("History boundary does not name a retained commit"))?;
    let ineligible = || invalid("History boundary does not name an eligible retained row");
    if commit.committer_time_ms != boundary.committer_time_ms {
        return Err(ineligible());
    }
    let row = match kind {
        HistoryQueryKind::Commit => HistoryPageRow::Commit(commit),
        HistoryQueryKind::Diff => {
            let path = boundary
                .file_path
                .as_deref()
                .ok_or_else(|| invalid("History diff boundary path missing"))?;
            let path_bytes = u64::try_from(path.len())
                .map_err(|_| invalid("History boundary backing overflow"))?;
            let _lookup_memory = budget
                .reserve_memory(path_bytes, MemoryLifetime::Temporary)
                .map_err(|error| history_work_error(budget, error))?;
            let lookup = HistoryDiffKey::new(boundary.sha, path);
            let (key, record) = state
                .diff_hunks()
                .get_key_value(&lookup)
                .ok_or_else(|| invalid("History diff missing"))?;
            HistoryPageRow::Diff(key, record, commit)
        }
    };
    if !admits(state, query, row, budget)? {
        return Err(ineligible());
    }
    if let Some(scoring) = scoring {
        let score = with_document_text(row, budget, |text, budget| {
            scoring.score(text, query, budget)
        })?;
        if boundary.order != (HistoryCursorOrderV1::Relevance { score }) {
            return Err(invalid(
                "History boundary score differs from selected view statistics",
            ));
        }
    }
    Ok(())
}

enum ScoreNode {
    Leaf(usize),
    All {
        required: Vec<Self>,
        excluded: Vec<Self>,
    },
    Any(Vec<Self>),
}
enum Approximation {
    Query(ScoreNode),
    Everything,
    Nothing,
    Excluding(ScoreNode),
}
impl Approximation {
    fn complement(self) -> Self {
        match self {
            Self::Query(node) => Self::Excluding(node),
            Self::Excluding(node) => Self::Query(node),
            Self::Everything => Self::Nothing,
            Self::Nothing => Self::Everything,
        }
    }
}
#[derive(Default)]
struct ScorePlanBuilder {
    leaves: Vec<Vec<usize>>,
    terms: BTreeMap<String, usize>,
}

impl ScorePlanBuilder {
    fn approximate(
        &mut self,
        expr: &LqExpr,
        over: bool,
        depth: usize,
        nodes: &mut usize,
        query: &LqQuery,
    ) -> Result<Approximation, CoreError> {
        *nodes += 1;
        if depth > 32 || *nodes > 256 {
            return Err(unscorable(
                "History relevance expression exceeds bounded plan",
            ));
        }
        match expr {
            LqExpr::Empty => Ok(Approximation::Everything),
            LqExpr::Not(expr) => Ok(self
                .approximate(expr, !over, depth + 1, nodes, query)?
                .complement()),
            LqExpr::All(children) => {
                let mut required = Vec::new();
                let mut excluded = Vec::new();
                for child in children {
                    match self.approximate(child, over, depth + 1, nodes, query)? {
                        Approximation::Nothing => return Ok(Approximation::Nothing),
                        Approximation::Everything => {}
                        Approximation::Query(node) => required.push(node),
                        Approximation::Excluding(node) => excluded.push(node),
                    }
                }
                if required.is_empty() {
                    Ok(if excluded.is_empty() || over {
                        Approximation::Everything
                    } else {
                        Approximation::Nothing
                    })
                } else {
                    Ok(Approximation::Query(ScoreNode::All { required, excluded }))
                }
            }
            LqExpr::Any(children) => {
                let mut clauses = Vec::new();
                for child in children {
                    match self.approximate(child, over, depth + 1, nodes, query)? {
                        Approximation::Everything | Approximation::Excluding(_) if over => {
                            return Ok(Approximation::Everything);
                        }
                        Approximation::Everything
                        | Approximation::Excluding(_)
                        | Approximation::Nothing => {}
                        Approximation::Query(node) => clauses.push(node),
                    }
                }
                Ok(if clauses.is_empty() {
                    Approximation::Nothing
                } else {
                    Approximation::Query(ScoreNode::Any(clauses))
                })
            }
            LqExpr::Leaf(LqLeaf::Keyword(text) | LqLeaf::Phrase(text)) => {
                let tokens = query_tokens(text, query.options.case_mode())
                    .map_err(|error| CoreError::Typed {
                        code: match error {
                            quanta_index_lq_text_normalizer::TextQueryError::NoTokens => quanta_index_contract::SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                            quanta_index_lq_text_normalizer::TextQueryError::TokenTooLong { .. } => quanta_index_contract::SearchPlaneErrorCodeV2::LexTextQueryTokenTooLong,
                        },
                        message: error.to_string(),
                    })?;
                if self.leaves.len() >= 64
                    || self
                        .terms
                        .len()
                        .checked_add(tokens.len())
                        .is_none_or(|n| n > 256)
                {
                    return Err(unscorable("History relevance term bound exceeded"));
                }
                let phrase = tokens
                    .into_iter()
                    .map(|token| {
                        let next_term = self.terms.len();
                        *self.terms.entry(token.text).or_insert(next_term)
                    })
                    .collect();
                let index = self.leaves.len();
                self.leaves.push(phrase);
                Ok(Approximation::Query(ScoreNode::Leaf(index)))
            }
            LqExpr::Leaf(LqLeaf::RawString(_)) => Ok(if over {
                Approximation::Everything
            } else {
                Approximation::Nothing
            }),
            LqExpr::Leaf(_) => Err(unscorable("History relevance leaf has no BM25 score")),
        }
    }
}

struct Scoring {
    root: ScoreNode,
    leaves: Vec<Vec<usize>>,
    terms: BTreeMap<String, usize>,
    phrases: PhraseAutomaton,
    document_frequencies: Vec<u64>,
    documents: u64,
    tokens: u64,
}

impl Scoring {
    fn new(query: &LqQuery) -> Result<Self, CoreError> {
        if !matches!(
            query.options.pattern_type,
            LqPatternType::Standard | LqPatternType::Keyword
        ) {
            return Err(unscorable(
                "History relevance requires standard/keyword pattern mode",
            ));
        }
        let mut builder = ScorePlanBuilder::default();
        let mut nodes = 0_usize;
        // Same over/under approximation contract as history_text_index/query:
        // raw strings filter rows but never invent a BM25 contribution.
        let Approximation::Query(root) =
            builder.approximate(&query.expr, true, 0, &mut nodes, query)?
        else {
            return Err(unscorable(
                "every History relevance alternative must be bounded by a positive keyword or phrase",
            ));
        };
        let ScorePlanBuilder { leaves, terms } = builder;
        let phrases = PhraseAutomaton::new(&leaves);
        let document_frequencies = vec![0; terms.len()];
        Ok(Self {
            root,
            leaves,
            terms,
            phrases,
            document_frequencies,
            documents: 0,
            tokens: 0,
        })
    }

    fn observe(
        &mut self,
        text: &str,
        query: &LqQuery,
        budget: &mut WorkBudget,
    ) -> Result<(), CoreError> {
        let mut present = vec![false; self.terms.len()];
        let mut count = 0_u64;
        let _ = matching::visit_history_tokens(text, query.options.case_mode(), budget, |token| {
            if let Some(text) = token {
                count = count
                    .checked_add(1)
                    .ok_or_else(|| invalid("History token count overflow"))?;
                if let Some(&term) = self.terms.get(text) {
                    present[term] = true;
                }
            }
            Ok(ControlFlow::Continue(()))
        })?;
        self.documents = self
            .documents
            .checked_add(1)
            .ok_or_else(|| invalid("History population overflow"))?;
        self.tokens = self
            .tokens
            .checked_add(count)
            .ok_or_else(|| invalid("History population token overflow"))?;
        for (present, df) in present.into_iter().zip(&mut self.document_frequencies) {
            if present {
                *df = df
                    .checked_add(1)
                    .ok_or_else(|| invalid("History term frequency overflow"))?;
            }
        }
        Ok(())
    }
    fn score(
        &self,
        text: &str,
        query: &LqQuery,
        budget: &mut WorkBudget,
    ) -> Result<HistoryScoreV1, CoreError> {
        let mut counter = self.phrases.counter();
        let mut doc_length = 0_u64;
        let _ = matching::visit_history_tokens(text, query.options.case_mode(), budget, |token| {
            if token.is_some() {
                doc_length = doc_length
                    .checked_add(1)
                    .ok_or_else(|| invalid("History token count overflow"))?;
            }
            // Unmatched terms and oversized runs both break every phrase.
            counter.push(token.and_then(|term| self.terms.get(term).copied()))?;
            Ok(ControlFlow::Continue(()))
        })?;
        let frequencies = counter.finish()?;
        let average = if self.documents == 0 {
            0.0
        } else {
            self.tokens as f32 / self.documents as f32
        };
        let score = self
            .evaluate(&self.root, &frequencies, doc_length as f32, average)?
            .ok_or_else(|| {
                invalid("History row predicate escaped the relevance expression superset")
            })?;
        HistoryScoreV1::try_new(score).map_err(|error| invalid(&error.to_string()))
    }
    fn evaluate(
        &self,
        node: &ScoreNode,
        frequencies: &[u32],
        dl: f32,
        average: f32,
    ) -> Result<Option<f32>, CoreError> {
        match node {
            ScoreNode::Leaf(index) => self.leaf_score(*index, frequencies, dl, average),
            ScoreNode::All { required, excluded } => {
                let mut score = 0.0;
                for node in required {
                    let Some(part) = self.evaluate(node, frequencies, dl, average)? else {
                        return Ok(None);
                    };
                    score += part;
                }
                for node in excluded {
                    if self.evaluate(node, frequencies, dl, average)?.is_some() {
                        return Ok(None);
                    }
                }
                Ok(Some(score))
            }
            ScoreNode::Any(clauses) => {
                let mut score = None;
                for node in clauses {
                    if let Some(part) = self.evaluate(node, frequencies, dl, average)? {
                        score = Some(score.unwrap_or(0.0) + part);
                    }
                }
                Ok(score)
            }
        }
    }
    fn leaf_score(
        &self,
        index: usize,
        frequencies: &[u32],
        dl: f32,
        average: f32,
    ) -> Result<Option<f32>, CoreError> {
        let leaf = self
            .leaves
            .get(index)
            .ok_or_else(|| invalid("History scoring leaf missing"))?;
        if leaf.is_empty() {
            return Err(unscorable("History relevance query has no tokens"));
        }
        let tf = *frequencies
            .get(index)
            .ok_or_else(|| invalid("History term frequency missing"))?;
        if tf == 0 {
            return Ok(None);
        }
        let mut idf = 0_f32;
        for &term in leaf {
            let df = self.document_frequencies[term];
            idf += (1.0 + (self.documents as f32 - df as f32 + 0.5) / (df as f32 + 0.5)).ln();
        }
        if average <= 0.0 {
            return Err(invalid("nonempty History term has an empty population"));
        }
        let tf = tf as f32;
        Ok(Some(
            idf * 2.2 * tf / (tf + 1.2 * (0.25 + 0.75 * dl / average)),
        ))
    }
}

fn select_relevance<'a>(
    state: &'a HistoryAuthorityState,
    query: &LqQuery,
    kind: HistoryQueryKind,
    epoch: AuxEpochV1,
    capacity: usize,
    boundary: Option<&'a HistoryCursor>,
    scoring: &Scoring,
    budget: &mut WorkBudget,
) -> Result<HistoryPageSelection<'a>, CoreError> {
    let mut selector = HistoryPageSelector::new(capacity, boundary, epoch);
    visit_documents(state, kind, budget, |row, budget| {
        selector.examined_one();
        if admits(state, query, row, budget)? {
            let score = with_document_text(row, budget, |text, budget| {
                scoring.score(text, query, budget)
            })?;
            selector.offer(row, Some(score));
        }
        Ok(())
    })?;
    selector.finish()
}

fn unscorable(message: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextQueryUnscorable,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::{
        DiffHunkSide, LQ_VERSION_TAG, LqFilter, LqOptions, LqSpan, LqType, ManifestGeneration,
        RepoId, RevisionId,
        index::{
            CollectionId, Digest, IndexInstanceId, SlotId, TenantId, WorkspaceKey, WorktreeId,
        },
        lex::{CommitRecord, CommitSha, DiffHunkRecord},
    };
    use quanta_index_core::{
        AuxiliaryGenerationKeyV1,
        index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    struct MemoryLedger {
        live: AtomicU64,
        maximum: u64,
    }
    struct MemoryGate(Arc<MemoryLedger>);
    impl MemoryLeaseOwner for MemoryLedger {
        fn release(&self, bytes: u64) {
            let previous = self.live.fetch_sub(bytes, Ordering::AcqRel);
            assert!(previous >= bytes);
        }
    }
    impl MemoryScopePort for MemoryGate {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _previous = self
                .0
                .live
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                    live.checked_add(bytes)
                        .filter(|next| *next <= self.0.maximum)
                })
                .map_err(|_| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn keyword(text: &str) -> LqExpr {
        LqExpr::Leaf(LqLeaf::Keyword(text.into()))
    }
    fn query(expr: LqExpr) -> LqQuery {
        LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr,
            filters: vec![quanta_index_contract::LqFilter::Type {
                kind: LqType::Commit,
            }],
            options: LqOptions::defaults(),
            directives: Vec::new(),
            source_span: LqSpan::eof(0),
        }
    }

    fn test_budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                mutations: 1_000_000,
                bytes: 1_000_000_000,
            },
            Instant::now() + Duration::from_secs(60),
        )
    }

    #[test]
    fn native_history_queries_read_large_commit_and_diff_under_64_mib() -> Result<(), CoreError> {
        let ledger = Arc::new(MemoryLedger {
            live: AtomicU64::new(0),
            maximum: 64 * 1024 * 1024,
        });
        let mut budget = test_budget();
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(MemoryGate(ledger.clone())))));
        let resident_bytes = 5 * 1024 * 1024;
        let resident_loan = budget
            .reserve_memory(resident_bytes, MemoryLifetime::Retained)
            .map_err(|error| history_work_error(&budget, error))?;
        let generation = AuxiliaryGenerationKeyV1 {
            repo_id: RepoId::new("query-shape").map_err(|error| invalid(&error.to_string()))?,
            revision_id: RevisionId::new("main").map_err(|error| invalid(&error.to_string()))?,
            generation: ManifestGeneration::new(1),
        };
        let sha = CommitSha::from_bytes([7; 20]);
        let mut state = HistoryAuthorityState::default();
        state.restore_commit(
            sha,
            CommitRecord {
                wire_version: 1,
                sha,
                parents: Vec::new(),
                author_time_ms: 1,
                committer_time_ms: 1,
                applied_at_ms: 1,
                author: "a".into(),
                author_name: None,
                author_email: None,
                committer: "c".into(),
                committer_name: None,
                committer_email: None,
                message: "needle".into(),
                is_merge: false,
                tags: Vec::new(),
            },
        );
        let huge = "x ".repeat(5 * 1024 * 1024 / 2);
        state.restore_diff_hunk(
            HistoryDiffKey::new(sha, "large.rs"),
            DiffHunkRecord {
                wire_version: 1,
                hunk_header: "@@".into(),
                side: DiffHunkSide::After,
                added_text: huge.into_boxed_str(),
                removed_text: "".into(),
                touched_text: "short snippet".into(),
                byte_start: 0,
                byte_end: 5 * 1024 * 1024,
            },
        );
        state.restore_meta(HistoryStateMeta {
            commits_materialized: true,
            refs_materialized: false,
            tags_materialized: false,
            diff_hunks_materialized: true,
        });
        let cut = HistorySourceCutV1 {
            generation: generation.clone(),
            durable_epoch: AuxEpochV1::new(1),
            snapshot_digest: Digest([2; 32]),
            replay_start: Digest([3; 32]),
        };
        let view = ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 1,
            digest: Digest([5; 32]),
        };
        let source = ReplayedHistorySourceV1 {
            state,
            cut: cut.clone(),
            sequence: 1,
            manifest_digest: None,
            _custody: ReplayCustody::default(),
        };
        let mut diff_query = query(keyword("needle"));
        diff_query.filters = vec![LqFilter::Type { kind: LqType::Diff }];
        let read = |state: &HistoryAuthorityState| NativeHistoryReadV1 {
            source: ReplayedHistorySourceV1 {
                state: state.clone(),
                cut: source.cut.clone(),
                sequence: source.sequence,
                manifest_digest: None,
                _custody: ReplayCustody::default(),
            },
            manifest: Manifest {
                format: REPLAY_HISTORY_MANIFEST,
                repo: generation.repo_id.clone(),
                revision: generation.revision_id.clone(),
                generation: generation.generation,
                epoch: cut.durable_epoch,
                snapshot: cut.snapshot_digest,
                replay: cut.replay_start,
                sequence: 1,
                source_manifest: None,
                predecessor: None,
                source_input: ObjectRef(Digest([6; 32])),
                rows: MapRoot(Some(ObjectRef(Digest([7; 32])))),
                row_count: 4,
                maximum_row_bytes: 5 * 1024 * 1024 + 1024,
            },
            view,
            instance: IndexInstanceId([8; 16]),
            artifact: ObjectRef(Digest([9; 32])),
            retained_initial: None,
            _manifest_memory: None,
        };
        for order in [HistoryOrderV1::Recency, HistoryOrderV1::Relevance] {
            let result = read(&source.state).query(
                view,
                &query(keyword("needle")),
                order,
                1,
                None,
                &mut budget,
            )?;
            assert_eq!(result.page().commits.len(), 1);
            assert_eq!(result.page().commits[0].sha, sha);
            assert_eq!(result.page().commits[0].message, "needle");
            assert_eq!(result.page().matched, 1);
            assert_eq!(
                ledger.live.load(Ordering::Acquire),
                resident_bytes
                    + (std::mem::size_of::<CommitCandidate>()
                        + "a".len()
                        + "c".len()
                        + "needle".len()) as u64
            );
            drop(result);
            assert_eq!(ledger.live.load(Ordering::Acquire), resident_bytes);
            diff_query.expr = keyword("snippet");
            let result =
                read(&source.state).query(view, &diff_query, order, 1, None, &mut budget)?;
            assert_eq!(result.page().matched, 1);
            assert_eq!(result.page().diffs[0].snippet, "short snippet");
            assert_eq!(
                ledger.live.load(Ordering::Acquire),
                resident_bytes
                    + (std::mem::size_of::<DiffCandidate>()
                        + "large.rs".len()
                        + "@@".len()
                        + "short snippet".len()) as u64
            );
            drop(result);
            assert_eq!(ledger.live.load(Ordering::Acquire), resident_bytes);
        }
        let large_commit_loan = budget
            .reserve_memory(resident_bytes, MemoryLifetime::Retained)
            .map_err(|error| history_work_error(&budget, error))?;
        let mut with_large_commit = source.state.clone();
        let mut record = source
            .state
            .commits()
            .get(&sha)
            .cloned()
            .ok_or_else(|| invalid("fixture Commit missing"))?;
        let mut message = "x".repeat(5 * 1024 * 1024);
        message.replace_range(message.len() - 7.., " needle");
        record.message = message.into_boxed_str();
        with_large_commit.restore_commit(sha, record);
        for order in [HistoryOrderV1::Recency, HistoryOrderV1::Relevance] {
            let result = read(&with_large_commit).query(
                view,
                &query(keyword("needle")),
                order,
                1,
                None,
                &mut budget,
            )?;
            assert_eq!(result.page().matched, 1);
            assert_eq!(result.page().commits[0].message.len(), 5 * 1024 * 1024);
        }
        drop(with_large_commit);
        drop(large_commit_loan);
        assert_eq!(ledger.live.load(Ordering::Acquire), resident_bytes);

        // An unselected large Commit must not reserve result backing.
        // Leave room for the selector and the small page, but not for it.
        let large_bytes = 1024 * 1024;
        let free_bytes = 512 * 1024;
        let competing_loan = budget
            .reserve_memory(
                ledger.maximum - resident_bytes - free_bytes,
                MemoryLifetime::Retained,
            )
            .map_err(|error| history_work_error(&budget, error))?;
        let fixed_bytes = ledger.maximum - free_bytes;
        let mut selected_state = source.state.clone();
        let mut bulk = source
            .state
            .commits()
            .get(&sha)
            .cloned()
            .ok_or_else(|| invalid("fixture Commit missing"))?;
        bulk.sha = CommitSha::from_bytes([9; 20]);
        bulk.message = format!("bulk {}", "x".repeat(large_bytes)).into_boxed_str();
        selected_state.restore_commit(bulk.sha, bulk);
        for order in [HistoryOrderV1::Recency, HistoryOrderV1::Relevance] {
            let mut work = test_budget();
            work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MemoryGate(ledger.clone())))));
            let result = read(&selected_state).query(
                view,
                &query(keyword("needle")),
                order,
                1,
                None,
                &mut work,
            )?;
            assert_eq!(result.page().matched, 1);
            assert_eq!(result.page().commits[0].sha, sha);
            assert!(
                (fixed_bytes + 1..fixed_bytes + 8192)
                    .contains(&ledger.live.load(Ordering::Acquire))
            );
            drop(result);
            assert_eq!(ledger.live.load(Ordering::Acquire), fixed_bytes);

            let rejected = read(&selected_state).query(
                view,
                &query(keyword("bulk")),
                order,
                1,
                None,
                &mut work,
            );
            assert!(matches!(
                rejected,
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                    ..
                })
            ));
            assert_eq!(ledger.live.load(Ordering::Acquire), fixed_bytes);
        }
        drop(selected_state);
        drop(competing_loan);
        drop(source);
        drop(resident_loan);
        assert_eq!(ledger.live.load(Ordering::Acquire), 0);
        Ok(())
    }

    #[test]
    fn native_history_boolean_bm25_uses_only_matching_branches_and_complete_population()
    -> Result<(), CoreError> {
        let query = query(LqExpr::Any(vec![
            LqExpr::All(vec![keyword("a"), keyword("b")]),
            keyword("c"),
        ]));
        let mut plan = Scoring::new(&query)?;
        let mut budget = test_budget();
        for text in ["a c", "a b", "c"] {
            plan.observe(text, &query, &mut budget)?;
        }
        // In `(a AND b) OR c`, the first document reaches only c. Its a
        // must not contribute. Gold fixes N3/df2/dl2/avgdl5/3/tf1 independently.
        let idf = (1_f32 + (3_f32 - 2_f32 + 0.5) / (2_f32 + 0.5)).ln();
        let expected = idf * 2.2 / (1.0 + 1.2 * (0.25 + 0.75 * 2.0 / (5.0 / 3.0)));
        assert!((plan.score("a c", &query, &mut budget)?.get() - expected).abs() < 0.00001);
        Ok(())
    }

    #[test]
    fn native_history_relevance_preserves_overlapping_and_repeated_phrase_scores()
    -> Result<(), CoreError> {
        let query = query(LqExpr::Any(vec![
            LqExpr::Leaf(LqLeaf::Phrase(
                "a a a a".into()
            ));
            64
        ]));
        let mut scoring = Scoring::new(&query)?;
        let mut budget = test_budget();
        let text = "a a a a a";
        scoring.observe(text, &query, &mut budget)?;
        // Two overlapping matches in a one-document population. Each of the
        // 64 leaves contributes its score even when it shares a terminal.
        let idf = 4.0 * (1.0_f32 + 0.5 / 1.5).ln();
        let expected = 64.0 * idf * 2.2 * 2.0 / (2.0 + 1.2);
        assert!((scoring.score(text, &query, &mut budget)?.get() - expected).abs() < 0.0002);
        Ok(())
    }

    #[test]
    fn native_history_relevance_refuses_unscored_alternatives_and_preserves_phrase_positions()
    -> Result<(), CoreError> {
        for expr in [
            LqExpr::Empty,
            LqExpr::Leaf(LqLeaf::RawString("x".into())),
            LqExpr::Not(Box::new(keyword("x"))),
            LqExpr::Any(vec![
                keyword("x"),
                LqExpr::Leaf(LqLeaf::RawString("raw".into())),
            ]),
        ] {
            assert!(matches!(
                Scoring::new(&query(expr)),
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextQueryUnscorable,
                    ..
                })
            ));
        }
        let query = query(LqExpr::All(vec![
            LqExpr::Leaf(LqLeaf::Phrase("café needle".into())),
            LqExpr::Leaf(LqLeaf::RawString("raw".into())),
        ]));
        let mut plan = Scoring::new(&query)?;
        let mut budget = test_budget();
        plan.observe("cafe\u{301} needle raw", &query, &mut budget)?;
        assert!(plan.score("CAFÉ needle raw", &query, &mut budget)?.get() > 0.0);
        let separated = format!("café {} needle raw", "a".repeat(257));
        assert!(plan.score(&separated, &query, &mut budget).is_err());
        Ok(())
    }
}
