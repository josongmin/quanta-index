//! Same-view History execution. Recency reuses the canonical selector; both
//! orders use the canonical row predicates. BM25 populations are independently
//! the complete commit and diff sets of this immutable member, before filters.

use super::*;
use crate::query_dispatcher::routes::history::{
    ensure_cursor_continues_order, ensure_history_shards_ready, execute_history_query_with_work,
    validate_history_query,
};
use crate::query_dispatcher::routes::history_records::{
    HistoryPage, HistoryQueryKind, commit_candidate_from_record, diff_candidate_from_record,
    ensure_cursor_kind, history_commit_matches, history_diff_matches, resolve_history_query_kind,
};
use crate::readiness::history_diff_search_text;
use quanta_index_contract::{
    CandidateCountV1, CommitCandidate, DiffCandidate, HistoryCursor, HistoryCursorOrderV1,
    HistoryOrderV1, HistoryScoreV1, LqExpr, LqLeaf, LqPatternType, LqQuery, QueryResultWindowV1,
};
use quanta_index_core::{HistoryTextDocKeyV1, HistoryTextHitV1};
use quanta_index_lq_text_normalizer::{Token, query_tokens, tokenize};
use std::collections::BinaryHeap;

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
        // The existing predicates/tokenizer may allocate per-row normalized
        // text/tokens. Fund the largest such row before invoking either path.
        let row_bound = self
            .manifest
            .maximum_row_bytes
            .checked_mul(128)
            .and_then(|bytes| bytes.checked_add(4096))
            .ok_or_else(|| invalid("History row execution bound overflow"))?;
        let _row_memory = budget
            .reserve_memory(row_bound, MemoryLifetime::Temporary)
            .map_err(|error| history_work_error(budget, error))?;
        let result_bound = self
            .manifest
            .maximum_row_bytes
            .checked_mul(32)
            .and_then(|bytes| bytes.checked_add(4096))
            .and_then(|bytes| bytes.checked_mul(u64::from(top_k).checked_add(1)?))
            .ok_or_else(|| invalid("History response bound overflow"))?;
        let result_memory = budget
            .reserve_memory(result_bound, MemoryLifetime::Retained)
            .map_err(|error| history_work_error(budget, error))?;
        let mut scoring = match order {
            HistoryOrderV1::Recency => None,
            HistoryOrderV1::Relevance => Some(Scoring::new(query)?),
        };
        let state = &self.source.state;
        let mut matched = 0_u64;
        // Statistics never depend on a filter or page. This same traversal
        // also counts the whole eligible universe for either order.
        visit_documents(state, kind, budget, &mut |key, time, text| {
            if admits(state, query, &key)? {
                matched = matched
                    .checked_add(1)
                    .ok_or_else(|| invalid("History matched count overflow"))?;
            }
            if let Some(scoring) = &mut scoring {
                scoring.observe(&text, query)?;
            }
            let _time_belongs_to_this_member = time;
            Ok(())
        })?;
        if let Some(boundary) = boundary {
            validate_anchor(state, query, kind, boundary, scoring.as_ref())?;
        }
        let page = if let Some(scoring) = scoring {
            execute_relevance(
                state,
                query,
                kind,
                self.source.cut.durable_epoch,
                top_k,
                boundary,
                &scoring,
                budget,
            )?
        } else {
            execute_history_query_with_work(
                query,
                state,
                self.source.cut.durable_epoch,
                top_k,
                boundary,
                &mut || charge_row(budget),
            )?
        };
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

fn charge_row(budget: &mut WorkBudget) -> Result<(), CoreError> {
    budget
        .charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })
        .map_err(|error| history_work_error(budget, error))
}

fn visit_documents(
    state: &HistoryAuthorityState,
    kind: HistoryQueryKind,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(
        HistoryTextDocKeyV1,
        u64,
        std::borrow::Cow<'_, str>,
    ) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    match kind {
        HistoryQueryKind::Commit => {
            for record in state.commits().values() {
                charge_row(budget)?;
                visit(
                    HistoryTextDocKeyV1::Commit { sha: record.sha },
                    record.committer_time_ms,
                    std::borrow::Cow::Borrowed(record.message.as_ref()),
                )?;
            }
        }
        HistoryQueryKind::Diff => {
            for (key, record) in state.diff_hunks() {
                charge_row(budget)?;
                let commit = state
                    .commits()
                    .get(&key.commit_sha())
                    .ok_or_else(|| invalid("History diff commit missing"))?;
                visit(
                    HistoryTextDocKeyV1::Diff {
                        sha: key.commit_sha(),
                        file_path: key.file_path().into(),
                    },
                    commit.committer_time_ms,
                    std::borrow::Cow::Owned(history_diff_search_text(key, record)),
                )?;
            }
        }
    }
    Ok(())
}

fn admits(
    state: &HistoryAuthorityState,
    query: &LqQuery,
    key: &HistoryTextDocKeyV1,
) -> Result<bool, CoreError> {
    match key {
        HistoryTextDocKeyV1::Commit { sha } => history_commit_matches(
            query,
            state,
            state
                .commits()
                .get(sha)
                .ok_or_else(|| invalid("History commit missing"))?,
        ),
        HistoryTextDocKeyV1::Diff { sha, file_path } => {
            let key = HistoryDiffKey::new(*sha, file_path);
            history_diff_matches(
                query,
                state,
                &key,
                state
                    .diff_hunks()
                    .get(&key)
                    .ok_or_else(|| invalid("History diff missing"))?,
                state
                    .commits()
                    .get(sha)
                    .ok_or_else(|| invalid("History diff commit missing"))?,
            )
        }
    }
}

fn validate_anchor(
    state: &HistoryAuthorityState,
    query: &LqQuery,
    kind: HistoryQueryKind,
    boundary: &HistoryCursor,
    scoring: Option<&Scoring>,
) -> Result<(), CoreError> {
    let key = match kind {
        HistoryQueryKind::Commit => HistoryTextDocKeyV1::Commit { sha: boundary.sha },
        HistoryQueryKind::Diff => HistoryTextDocKeyV1::Diff {
            sha: boundary.sha,
            file_path: boundary
                .file_path
                .clone()
                .ok_or_else(|| invalid("History diff boundary path missing"))?,
        },
    };
    let commit = state
        .commits()
        .get(&boundary.sha)
        .ok_or_else(|| invalid("History boundary does not name a retained commit"))?;
    if commit.committer_time_ms != boundary.committer_time_ms || !admits(state, query, &key)? {
        return Err(invalid(
            "History boundary does not name an eligible retained row",
        ));
    }
    if let Some(scoring) = scoring {
        let text = match &key {
            HistoryTextDocKeyV1::Commit { .. } => {
                std::borrow::Cow::Borrowed(commit.message.as_ref())
            }
            HistoryTextDocKeyV1::Diff { sha, file_path } => {
                let key = HistoryDiffKey::new(*sha, file_path);
                std::borrow::Cow::Owned(history_diff_search_text(
                    &key,
                    state
                        .diff_hunks()
                        .get(&key)
                        .ok_or_else(|| invalid("History boundary diff missing"))?,
                ))
            }
        };
        if boundary.order
            != (HistoryCursorOrderV1::Relevance {
                score: scoring.score(&text, query)?,
            })
        {
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
struct Scoring {
    root: Option<ScoreNode>,
    leaves: Vec<Vec<Token>>,
    terms: Vec<(String, u64)>,
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
        let mut result = Self {
            root: None,
            leaves: Vec::new(),
            terms: Vec::new(),
            documents: 0,
            tokens: 0,
        };
        let mut nodes = 0_usize;
        // Same over/under approximation contract as history_text_index/query:
        // raw strings filter rows but never invent a BM25 contribution.
        match result.approximate(&query.expr, true, 0, &mut nodes, query)? {
            Approximation::Query(node) => result.root = Some(node),
            _ => {
                return Err(unscorable(
                    "every History relevance alternative must be bounded by a positive keyword or phrase",
                ));
            }
        }
        Ok(result)
    }
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
                for token in &tokens {
                    if !self.terms.iter().any(|(term, _)| term == &token.text) {
                        self.terms.push((token.text.clone(), 0));
                    }
                }
                let index = self.leaves.len();
                self.leaves.push(tokens);
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
    fn observe(&mut self, text: &str, query: &LqQuery) -> Result<(), CoreError> {
        let tokenized = tokenize(text, query.options.case_mode());
        self.documents = self
            .documents
            .checked_add(1)
            .ok_or_else(|| invalid("History population overflow"))?;
        self.tokens = self
            .tokens
            .checked_add(
                u64::try_from(tokenized.indexable().count())
                    .map_err(|_| invalid("History token count overflow"))?,
            )
            .ok_or_else(|| invalid("History population token overflow"))?;
        for (term, df) in &mut self.terms {
            if tokenized.indexable().any(|token| &token.text == term) {
                *df = df
                    .checked_add(1)
                    .ok_or_else(|| invalid("History term frequency overflow"))?;
            }
        }
        Ok(())
    }
    fn score(&self, text: &str, query: &LqQuery) -> Result<HistoryScoreV1, CoreError> {
        let tokenized = tokenize(text, query.options.case_mode());
        let dl = tokenized.indexable().count() as f32;
        let average = if self.documents == 0 {
            0.0
        } else {
            self.tokens as f32 / self.documents as f32
        };
        let score = self
            .evaluate(
                self.root
                    .as_ref()
                    .ok_or_else(|| invalid("History scoring plan missing"))?,
                &tokenized.tokens,
                dl,
                average,
            )?
            .ok_or_else(|| {
                invalid("History row predicate escaped the relevance expression superset")
            })?;
        HistoryScoreV1::try_new(score).map_err(|error| invalid(&error.to_string()))
    }
    fn evaluate(
        &self,
        node: &ScoreNode,
        tokens: &[Token],
        dl: f32,
        average: f32,
    ) -> Result<Option<f32>, CoreError> {
        match node {
            ScoreNode::Leaf(index) => self.leaf_score(*index, tokens, dl, average),
            ScoreNode::All { required, excluded } => {
                let mut score = 0.0;
                for node in required {
                    let Some(part) = self.evaluate(node, tokens, dl, average)? else {
                        return Ok(None);
                    };
                    score += part;
                }
                for node in excluded {
                    if self.evaluate(node, tokens, dl, average)?.is_some() {
                        return Ok(None);
                    }
                }
                Ok(Some(score))
            }
            ScoreNode::Any(clauses) => {
                let mut score = None;
                for node in clauses {
                    if let Some(part) = self.evaluate(node, tokens, dl, average)? {
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
        tokens: &[Token],
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
        let mut tf = 0_u32;
        for window in tokens.windows(leaf.len()) {
            let start = window
                .first()
                .ok_or_else(|| invalid("History phrase window missing"))?
                .position;
            if window
                .iter()
                .zip(leaf)
                .enumerate()
                .all(|(offset, (token, expected))| {
                    token.is_indexable()
                        && token.text == expected.text
                        && token.position == start + offset
                })
            {
                tf = tf
                    .checked_add(1)
                    .ok_or_else(|| invalid("History term frequency overflow"))?;
            }
        }
        if tf == 0 {
            return Ok(None);
        }
        let mut idf = 0_f32;
        for token in leaf {
            let df = self
                .terms
                .iter()
                .find(|(term, _)| term == &token.text)
                .map(|(_, df)| *df)
                .ok_or_else(|| invalid("History BM25 term missing"))?;
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

fn execute_relevance(
    state: &HistoryAuthorityState,
    query: &LqQuery,
    kind: HistoryQueryKind,
    epoch: AuxEpochV1,
    top_k: u32,
    boundary: Option<&HistoryCursor>,
    scoring: &Scoring,
    budget: &mut WorkBudget,
) -> Result<HistoryPage, CoreError> {
    let mut hits = BinaryHeap::new();
    let mut matched = 0_u64;
    let mut examined = 0_u64;
    let after = boundary
        .map(|cursor| -> Result<HistoryTextHitV1, CoreError> {
            let HistoryCursorOrderV1::Relevance { score } = cursor.order else {
                return Err(invalid("History relevance boundary order differs"));
            };
            Ok(HistoryTextHitV1 {
                key: cursor.file_path.as_ref().map_or(
                    HistoryTextDocKeyV1::Commit { sha: cursor.sha },
                    |path| HistoryTextDocKeyV1::Diff {
                        sha: cursor.sha,
                        file_path: path.clone(),
                    },
                ),
                committer_time_ms: cursor.committer_time_ms,
                score,
            })
        })
        .transpose()?;
    visit_documents(state, kind, budget, &mut |key, time, text| {
        examined = examined
            .checked_add(1)
            .ok_or_else(|| invalid("History examined count overflow"))?;
        if !admits(state, query, &key)? {
            return Ok(());
        }
        let hit = HistoryTextHitV1 {
            key,
            committer_time_ms: time,
            score: scoring.score(&text, query)?,
        };
        if after.as_ref().is_some_and(|after| hit <= *after) {
            return Ok(());
        }
        matched = matched
            .checked_add(1)
            .ok_or_else(|| invalid("History remaining count overflow"))?;
        hits.push(hit);
        if hits.len() > top_k as usize {
            drop(hits.pop());
        }
        Ok(())
    })?;
    let hits = hits.into_sorted_vec();
    let returned =
        u32::try_from(hits.len()).map_err(|_| invalid("History result count overflow"))?;
    let more = matched > u64::from(returned);
    let next_cursor = if more {
        hits.last().map(|hit| HistoryCursor {
            order: HistoryCursorOrderV1::Relevance { score: hit.score },
            committer_time_ms: hit.committer_time_ms,
            sha: hit.key.sha(),
            file_path: hit.key.file_path().map(str::to_owned),
            aux_epoch: epoch,
        })
    } else {
        None
    };
    let mut commits = Vec::new();
    let mut diffs = Vec::new();
    for hit in hits {
        match &hit.key {
            HistoryTextDocKeyV1::Commit { sha } => commits.push(commit_candidate_from_record(
                state
                    .commits()
                    .get(sha)
                    .ok_or_else(|| invalid("History selected commit missing"))?,
                Some(hit.score),
            )),
            HistoryTextDocKeyV1::Diff { sha, file_path } => {
                let key = HistoryDiffKey::new(*sha, file_path);
                diffs.push(diff_candidate_from_record(
                    &key,
                    state
                        .diff_hunks()
                        .get(&key)
                        .ok_or_else(|| invalid("History selected diff missing"))?,
                    Some(hit.score),
                ));
            }
        }
    }
    let window = QueryResultWindowV1::new(returned, CandidateCountV1::Exact(matched), more)
        .map_err(|error| invalid(&error.to_string()))?;
    Ok(HistoryPage {
        commits,
        diffs,
        window,
        examined,
        next_cursor,
    })
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
    use quanta_index_contract::{LQ_VERSION_TAG, LqOptions, LqSpan, LqType};

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

    #[test]
    fn native_history_boolean_bm25_uses_only_matching_branches_and_complete_population()
    -> Result<(), CoreError> {
        let query = query(LqExpr::Any(vec![
            LqExpr::All(vec![keyword("a"), keyword("b")]),
            keyword("c"),
        ]));
        let mut plan = Scoring::new(&query)?;
        for text in ["a c", "a b", "c"] {
            plan.observe(text, &query)?;
        }
        // In `(a AND b) OR c`, the first document reaches only c. Its a
        // must not contribute. Gold fixes N3/df2/dl2/avgdl5/3/tf1 independently.
        let idf = (1_f32 + (3_f32 - 2_f32 + 0.5) / (2_f32 + 0.5)).ln();
        let expected = idf * 2.2 / (1.0 + 1.2 * (0.25 + 0.75 * 2.0 / (5.0 / 3.0)));
        assert!((plan.score("a c", &query)?.get() - expected).abs() < 0.00001);
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
        plan.observe("cafe\u{301} needle raw", &query)?;
        assert!(plan.score("CAFÉ needle raw", &query)?.get() > 0.0);
        let separated = format!("café {} needle raw", "a".repeat(257));
        assert!(plan.score(&separated, &query).is_err());
        Ok(())
    }
}
