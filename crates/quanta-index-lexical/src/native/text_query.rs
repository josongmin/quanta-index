//! Page-bounded Content Boolean and positional phrase execution of the planner IR.

use super::text::{
    NativeTextIndex, TextPostingCursor, TextReadContext, TextStatistics, ValidatedPostingHit,
    admitted_query_tokens,
};
use super::text_memory::PreparedKey;
use super::text_overlay::TextOverlayCorpus;
use super::text_regex::{MAX_NATIVE_CONTENT_REGEX, NativeRegexProgram};
use super::text_universe::TextOwnerCursor;
use crate::{
    filters::FilterPlan,
    phrase::PhrasePolicy,
    plan::{LexicalPlan, PlanLeaf, PlanNode},
};
use quanta_index_contract::native_protocol::MAX_NATIVE_TEXT_TOP_K;
use quanta_index_contract::{LqCountBound, LqFilter};
use quanta_index_core::index::{
    IndexError, MAX_SOURCE_PATH_BYTES, MemberBinding, MemoryLease, MemoryLifetime, ObjectRef,
    OwnerKey, ViewRef, WorkBudget, WorkUsage,
};
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticSpanGuardV1,
    DiagnosticStageV1, DiagnosticTerminalV1,
};
use quanta_index_lq_text_normalizer::TextQueryError;
use std::time::Instant;
use std::{cmp::Ordering, collections::BinaryHeap};

/// One query-owned aggregate. All observations remain advisory to the work budget.
struct QueryLexicalObservation {
    diagnostic: Option<DiagnosticContextV1>,
    span: Option<DiagnosticSpanGuardV1>,
    posting_calls: u64,
    posting_hits: u64,
    posting_elapsed_ns: u64,
    phrase_checks: u64,
    phrase_elapsed_ns: u64,
    overflowed: bool,
}

impl QueryLexicalObservation {
    fn new(parent: Option<&DiagnosticContextV1>) -> Self {
        let entered = parent.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryLexical)
                .map(|span| (context.clone(), span))
        });
        let (diagnostic, span) =
            entered.map_or((None, None), |(context, span)| (Some(context), Some(span)));
        Self {
            diagnostic,
            span,
            posting_calls: 0,
            posting_hits: 0,
            posting_elapsed_ns: 0,
            phrase_checks: 0,
            phrase_elapsed_ns: 0,
            overflowed: false,
        }
    }

    fn child_context(&self) -> Option<DiagnosticContextV1> {
        self.diagnostic
            .as_ref()
            .zip(self.span.as_ref())
            .map(|(context, span)| context.child_context(span))
    }

    fn add(total: &mut u64, amount: Option<u64>, overflowed: &mut bool) {
        if let Some(next) = amount.and_then(|amount| total.checked_add(amount)) {
            *total = next;
        } else {
            *overflowed = true;
        }
    }

    fn elapsed(total: &mut u64, started: Instant, overflowed: &mut bool) {
        match u64::try_from(started.elapsed().as_nanos()) {
            Ok(elapsed) => Self::add(total, Some(elapsed), overflowed),
            Err(_overflow) => *overflowed = true,
        }
    }

    fn posting(
        &mut self,
        cursor: &mut TextPostingCursor,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<ValidatedPostingHit>, IndexError> {
        let Some(_span) = self.span.as_ref() else {
            return cursor.next_hit(context);
        };
        Self::add(&mut self.posting_calls, Some(1), &mut self.overflowed);
        let started = Instant::now();
        let result = cursor.next_hit(context);
        if matches!(&result, Ok(Some(_))) {
            Self::add(&mut self.posting_hits, Some(1), &mut self.overflowed);
        }
        Self::elapsed(&mut self.posting_elapsed_ns, started, &mut self.overflowed);
        result
    }

    fn phrase(
        &mut self,
        positions: &[&[u32]],
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        let Some(_span) = self.span.as_ref() else {
            return super::text_phrase::matches_positions(positions, budget);
        };
        let started = Instant::now();
        let result = super::text_phrase::matches_positions(positions, budget);
        Self::add(&mut self.phrase_checks, Some(1), &mut self.overflowed);
        Self::elapsed(&mut self.phrase_elapsed_ns, started, &mut self.overflowed);
        result
    }

    fn close(&mut self, outcome: DiagnosticTerminalV1) {
        if let (Some(context), Some(span)) = (self.diagnostic.as_ref(), self.span.take()) {
            if self.overflowed {
                context.health(DiagnosticHealthCodeV1::CounterOverflow);
            } else {
                context.counter(
                    &span,
                    DiagnosticCounterV1::QueryPostingCalls,
                    self.posting_calls,
                );
                context.counter(
                    &span,
                    DiagnosticCounterV1::QueryPostingHits,
                    self.posting_hits,
                );
                context.counter(
                    &span,
                    DiagnosticCounterV1::QueryPostingElapsedNs,
                    self.posting_elapsed_ns,
                );
                context.counter(
                    &span,
                    DiagnosticCounterV1::QueryPhraseChecks,
                    self.phrase_checks,
                );
                context.counter(
                    &span,
                    DiagnosticCounterV1::QueryPhraseElapsedNs,
                    self.phrase_elapsed_ns,
                );
            }
            let _finished = span.finish(outcome);
        }
    }
}

impl Drop for QueryLexicalObservation {
    fn drop(&mut self) {
        self.close(DiagnosticTerminalV1::Aborted);
    }
}

const MAX_CONTENT_LEAVES: usize = 64;
const MAX_QUERY_NODES: usize = 256;
const MAX_QUERY_DEPTH: usize = 32;

#[derive(Clone, Copy)]
enum Clause {
    Term(usize),
    Regex(usize),
    All(usize),
    Any(usize),
    Not,
    Phrase {
        first: usize,
        count: usize,
        cap: u32,
    },
}

#[derive(Clone, Copy, Default)]
struct MatchState {
    matched: bool,
    positive_terms: u64,
}

#[derive(Default)]
struct Complexity {
    nodes: usize,
    terms: usize,
    regexes: usize,
}

fn compile_clauses<'a>(
    node: &'a PlanNode,
    depth: usize,
    complexity: &mut Complexity,
    program: &mut Vec<Clause>,
    term: &mut impl FnMut(&'a str) -> Result<usize, IndexError>,
    phrase: &mut impl FnMut(&str, &crate::phrase::PhrasePlan) -> Result<(), IndexError>,
    regex: &mut impl FnMut(&'a str) -> Result<usize, IndexError>,
) -> Result<bool, IndexError> {
    complexity.nodes = complexity
        .nodes
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    if depth > MAX_QUERY_DEPTH || complexity.nodes > MAX_QUERY_NODES {
        return Err(IndexError::InvalidInput(
            "native BM25 boolean complexity limit".into(),
        ));
    }
    match node {
        PlanNode::Leaf {
            leaf: PlanLeaf::Content { term: text },
            cap: None,
        } => {
            complexity.terms = complexity
                .terms
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if complexity.terms > MAX_CONTENT_LEAVES {
                return Err(IndexError::InvalidInput(
                    "native BM25 exceeds 64 Content leaves".into(),
                ));
            }
            program.push(Clause::Term(term(text)?));
            Ok(true)
        }
        PlanNode::Leaf {
            leaf: PlanLeaf::Regex { source, plan },
            cap: None,
        } => {
            if plan.source() != source.as_str() {
                return Err(IndexError::InvalidInput(
                    "native regex plan source mismatch".into(),
                ));
            }
            complexity.regexes = complexity
                .regexes
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if complexity.regexes > MAX_NATIVE_CONTENT_REGEX {
                return Err(IndexError::ResourceLimited);
            }
            program.push(Clause::Regex(regex(source)?));
            Ok(false)
        }
        PlanNode::Leaf {
            leaf: PlanLeaf::Phrase { phrase: raw, plan },
            cap,
        } => {
            if plan.field != crate::phrase::PhraseField::Content
                || plan.slop != 0
                || plan.candidate_cap == 0
                || cap.is_some_and(|cap| cap.0 == 0)
                || plan.tokens.is_empty()
            {
                return Err(IndexError::InvalidInput(
                    "invalid native Content phrase policy".into(),
                ));
            }
            let first = complexity.terms;
            complexity.terms = first
                .checked_add(plan.tokens.len())
                .ok_or(IndexError::ResourceLimited)?;
            if complexity.terms > MAX_CONTENT_LEAVES {
                return Err(IndexError::InvalidInput(
                    "native query exceeds 64 term occurrences".into(),
                ));
            }
            phrase(raw, plan)?;
            for token in &plan.tokens {
                let _ordinal = term(token)?;
            }
            program.push(Clause::Phrase {
                first,
                count: plan.tokens.len(),
                cap: cap.map_or(plan.candidate_cap, |cap| cap.0.min(plan.candidate_cap)),
            });
            Ok(true)
        }
        PlanNode::All(children) | PlanNode::Any(children) if !children.is_empty() => {
            if children.len() > MAX_QUERY_NODES.saturating_sub(complexity.nodes) {
                return Err(IndexError::InvalidInput(
                    "native BM25 boolean complexity limit".into(),
                ));
            }
            let all = matches!(node, PlanNode::All(_));
            let mut anchored = !all;
            for child in children {
                let child_anchored = compile_clauses(
                    child,
                    depth.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                    complexity,
                    program,
                    term,
                    phrase,
                    regex,
                )?;
                anchored = if all {
                    anchored || child_anchored
                } else {
                    anchored && child_anchored
                };
            }
            program.push(if all {
                Clause::All(children.len())
            } else {
                Clause::Any(children.len())
            });
            Ok(anchored)
        }
        PlanNode::Not(child) => {
            let _child_anchored = compile_clauses(
                child,
                depth.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                complexity,
                program,
                term,
                phrase,
                regex,
            )?;
            program.push(Clause::Not);
            // A complement does not supply a candidate universe. Even double
            // negation remains a filter, never a scoring or universe source.
            Ok(false)
        }
        PlanNode::Leaf { .. } | PlanNode::Empty | PlanNode::All(_) | PlanNode::Any(_) => {
            Err(IndexError::InvalidInput(
                "native BM25 requires uncapped Content or bounded Content phrase clauses".into(),
            ))
        }
    }
}

// A NOT branch changes truth but contributes no ranking terms. Failed AND
// branches contribute no terms through an enclosing OR. Truth and score mask
// must remain distinct: an unanchored match may correctly have zero score.
fn evaluate_boolean(
    program: &[Clause],
    present: u64,
    regex_present: u8,
    mut phrase: impl FnMut(usize, usize, usize, u32) -> Result<MatchState, IndexError>,
) -> Result<MatchState, IndexError> {
    let mut stack = [MatchState::default(); MAX_QUERY_NODES];
    let mut end = 0usize;
    for (ordinal, clause) in program.iter().enumerate() {
        let state = match *clause {
            Clause::Term(index) => {
                let term = 1u64
                    .checked_shl(
                        u32::try_from(index).map_err(|_error| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?
                    & present;
                MatchState {
                    matched: term != 0,
                    positive_terms: term,
                }
            }
            Clause::Regex(index) => {
                let bit = 1u8
                    .checked_shl(
                        u32::try_from(index).map_err(|_error| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
                MatchState {
                    matched: regex_present & bit != 0,
                    positive_terms: 0,
                }
            }
            Clause::Phrase { first, count, cap } => phrase(ordinal, first, count, cap)?,
            Clause::Not => {
                let child = end.checked_sub(1).ok_or_else(|| {
                    IndexError::BackendViolation("boolean NOT stack underflow".into())
                })?;
                let state = *stack.get(child).ok_or_else(|| {
                    IndexError::BackendViolation("boolean NOT stack range exceeds bound".into())
                })?;
                end = child;
                MatchState {
                    matched: !state.matched,
                    positive_terms: 0,
                }
            }
            Clause::All(count) | Clause::Any(count) => {
                let start = end.checked_sub(count).ok_or_else(|| {
                    IndexError::BackendViolation("boolean evaluation stack underflow".into())
                })?;
                let children = stack.get(start..end).ok_or_else(|| {
                    IndexError::BackendViolation("boolean stack range exceeds bound".into())
                })?;
                let all = matches!(clause, Clause::All(_));
                let matched = if all {
                    children.iter().all(|child| child.matched)
                } else {
                    children.iter().any(|child| child.matched)
                };
                let positive_terms = if matched {
                    children
                        .iter()
                        .filter(|child| child.matched)
                        .fold(0u64, |mask, child| mask | child.positive_terms)
                } else {
                    0
                };
                end = start;
                MatchState {
                    matched,
                    positive_terms,
                }
            }
        };
        *stack.get_mut(end).ok_or_else(|| {
            IndexError::BackendViolation("boolean evaluation stack overflow".into())
        })? = state;
        end = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    if end != 1 {
        return Err(IndexError::BackendViolation(
            "boolean evaluation has no unique result".into(),
        ));
    }
    let result = stack
        .first()
        .copied()
        .ok_or_else(|| IndexError::BackendViolation("boolean result stack is empty".into()))?;
    Ok(result)
}

/// Explicit ranking parameters; invalid or nonfinite parameters are refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bm25Policy {
    pub k1: f64,
    pub b: f64,
}

impl Bm25Policy {
    pub fn validate(self) -> Result<(), IndexError> {
        if !self.k1.is_finite()
            || self.k1 <= 0.0
            || !self.b.is_finite()
            || !(0.0..=1.0).contains(&self.b)
        {
            return Err(IndexError::InvalidInput(
                "invalid native BM25 policy".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextContentExecution {
    pub top_k: u32,
    pub policy: Bm25Policy,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeTextRankedHit {
    pub owner: OwnerKey,
    pub source: NativeTextHitSource,
    pub score: f64,
}

pub use quanta_index_core::index::NativeTextHitSource;

#[derive(Clone, Copy)]
pub struct TextOverlaySource<'a> {
    pub owner: OwnerKey,
    pub revision: u64,
    pub path: &'a str,
    pub body: &'a str,
}

/// Only returned after exhaustive, provenance-validated posting traversal.
/// Repository/revision/snippet metadata and continuation cursors are not inferred.
#[derive(Debug)]
pub struct NativeTextRanking {
    pub view: ViewRef,
    pub artifact: ObjectRef,
    pub matched: u64,
    pub hits: Vec<NativeTextRankedHit>,
    // Returned ranking data may outlive its producing execution reservation.
    _hit_memory: Option<MemoryLease>,
}

impl PartialEq for NativeTextRanking {
    fn eq(&self, other: &Self) -> bool {
        self.view == other.view
            && self.artifact == other.artifact
            && self.matched == other.matched
            && self.hits == other.hits
    }
}

fn capacity_bytes<T>(count: usize) -> Result<u64, IndexError> {
    u64::try_from(
        count
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(IndexError::ResourceLimited)?,
    )
    .map_err(|_error| IndexError::ResourceLimited)
}

// `count:N` limits returned rows, never the posting/match universe. The
// caller's top_k remains an independent, stricter result ceiling.
fn result_capacity(top_k: u32, count: Option<LqCountBound>) -> Result<usize, IndexError> {
    let rows = match count {
        None => top_k,
        Some(LqCountBound::Bounded(0) | LqCountBound::All) => {
            return Err(IndexError::InvalidInput(
                "native BM25 requires a bounded positive count".into(),
            ));
        }
        Some(LqCountBound::Bounded(rows)) => top_k.min(rows),
    };
    usize::try_from(rows).map_err(|_error| IndexError::ResourceLimited)
}

// One used query workspace owns bounded backing on partial construction,
// traversal failure and unwind. Cursor keys/pages and occurrences own their
// separate grants; this grant covers only the containers and heap storage.
struct QueryWorkspace {
    cursors: Vec<TextPostingCursor>,
    program: Vec<Clause>,
    score_order: Vec<usize>,
    heads: Vec<Option<ValidatedPostingHit>>,
    heap: BinaryHeap<HeapHit>,
    _memory: Option<MemoryLease>,
}

impl QueryWorkspace {
    fn new(top_k: usize, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let bytes = [
            capacity_bytes::<TextPostingCursor>(MAX_CONTENT_LEAVES)?,
            capacity_bytes::<Clause>(MAX_QUERY_NODES)?,
            capacity_bytes::<usize>(MAX_CONTENT_LEAVES)?,
            capacity_bytes::<Option<ValidatedPostingHit>>(MAX_CONTENT_LEAVES)?,
            capacity_bytes::<HeapHit>(top_k)?,
        ]
        .into_iter()
        .try_fold(0u64, |sum, bytes| {
            sum.checked_add(bytes).ok_or(IndexError::ResourceLimited)
        })?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        let mut work = Self {
            cursors: Vec::new(),
            program: Vec::new(),
            score_order: Vec::new(),
            heads: Vec::new(),
            heap: BinaryHeap::new(),
            _memory: memory,
        };
        work.cursors
            .try_reserve_exact(MAX_CONTENT_LEAVES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        work.program
            .try_reserve_exact(MAX_QUERY_NODES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        work.score_order
            .try_reserve_exact(MAX_CONTENT_LEAVES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        work.heads
            .try_reserve_exact(MAX_CONTENT_LEAVES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        work.heap
            .try_reserve_exact(top_k)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(work)
    }
}

struct HeapHit(NativeTextRankedHit);

impl PartialEq for HeapHit {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for HeapHit {}
impl PartialOrd for HeapHit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapHit {
    fn cmp(&self, other: &Self) -> Ordering {
        // The worst retained hit is the heap maximum.
        other
            .0
            .score
            .total_cmp(&self.0.score)
            .then(self.0.owner.cmp(&other.0.owner))
    }
}

fn count_float(value: u64) -> Result<f64, IndexError> {
    let upper = u32::try_from(value >> 32)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let lower = u32::try_from(value & u64::from(u32::MAX))
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    Ok(f64::from(upper).mul_add(4_294_967_296.0, f64::from(lower)))
}

#[derive(Clone, Copy)]
struct ScoreBasis {
    avg: f64,
    idf: f64,
}

fn score_with_basis(
    policy: Bm25Policy,
    stats: TextStatistics,
    df: u64,
    tf: u64,
    dl: u64,
    basis: &mut Option<ScoreBasis>,
) -> Result<f64, IndexError> {
    if stats.docs == 0
        || stats.indexed_tokens == 0
        || df == 0
        || df > stats.docs
        || tf == 0
        || tf > dl
    {
        return Err(IndexError::CorruptObject(
            "invalid native BM25 statistics".into(),
        ));
    }
    let frequency = count_float(tf)?;
    let ScoreBasis { avg, idf } = if let Some(derived) = *basis {
        derived
    } else {
        // This value depends only on the query's immutable corpus statistics
        // and this cursor's admitted document frequency, not on a posting.
        let n = count_float(stats.docs)?;
        let d = count_float(df)?;
        let avg = count_float(stats.indexed_tokens)? / n;
        let idf = ((count_float(
            stats
                .docs
                .checked_sub(df)
                .ok_or(IndexError::ResourceLimited)?,
        )? + 0.5)
            / (d + 0.5))
            .ln_1p();
        let derived = ScoreBasis { avg, idf };
        *basis = Some(derived);
        derived
    };
    let length = 1.0 - policy.b + policy.b * count_float(dl)? / avg;
    // Algebraic scaling avoids overflowing k1*tf or k1*length for an otherwise
    // valid finite policy, including extreme positive k1 values.
    let saturation = if policy.k1 <= 1.0 {
        (policy.k1 + 1.0) / policy.k1.mul_add(length / frequency, 1.0)
    } else {
        (1.0 + policy.k1.recip()) / (policy.k1.recip() + length / frequency)
    };
    let value = idf * saturation;
    if !value.is_finite() || value <= 0.0 {
        return Err(IndexError::CorruptObject(
            "nonfinite native BM25 score".into(),
        ));
    }
    Ok(value)
}

#[cfg(test)]
fn score(
    policy: Bm25Policy,
    stats: TextStatistics,
    df: u64,
    tf: u64,
    dl: u64,
) -> Result<f64, IndexError> {
    score_with_basis(policy, stats, df, tf, dl, &mut None)
}

impl NativeTextIndex {
    /// Execute Content terms, phrases, bounded regex and All/Any/Not.
    /// Positive-anchored queries use the posting-owner union; unanchored
    /// queries use only the member's admitted-owner universe. Any content
    /// regex additionally requires at most 4,096 admitted owners before
    /// executor construction, including when a Content term anchors it.
    /// Storage is bounded by query width, page size and `top_k`; visiting
    /// negative postings and the finite universe consumes the original budget.
    /// Equal canonical terms contribute once; boolean union never duplicates
    /// an owner. A match reached only through NOT carries score zero.
    /// Phrase tokens must occupy consecutive positions, including repetitions.
    /// Each phrase cap counts owners containing all its terms before positional
    /// verification; exceeding it refuses the query rather than returning a prefix.
    /// Regex truth and path filters never invent BM25 score. Other query
    /// domains/options return a typed refusal. Budget exhaustion
    /// discards all provisional hits; it cannot return a successful prefix.
    pub fn search_content(
        &self,
        member: &MemberBinding,
        plan: &LexicalPlan,
        constraints: &quanta_index_contract::QueryConstraintSetV1,
        top_k: u32,
        policy: Bm25Policy,
        context: TextReadContext<'_>,
    ) -> Result<NativeTextRanking, IndexError> {
        self.search_content_with_overlay(
            member,
            plan,
            constraints,
            TextContentExecution { top_k, policy },
            None,
            context,
        )
    }

    pub fn search_content_with_overlay(
        &self,
        member: &MemberBinding,
        plan: &LexicalPlan,
        constraints: &quanta_index_contract::QueryConstraintSetV1,
        execution: TextContentExecution,
        overlay: Option<(&TextOverlayCorpus, &[TextOverlaySource<'_>])>,
        context: TextReadContext<'_>,
    ) -> Result<NativeTextRanking, IndexError> {
        let TextReadContext {
            ticket,
            objects,
            maps,
            budget,
        } = context;
        let mut observation = QueryLexicalObservation::new(budget.request().diagnostic_context());
        let mut scoped_budget = budget.scoped_diagnostic_context(observation.child_context());
        let result = self.search_content_with_overlay_inner(
            member,
            plan,
            constraints,
            execution,
            overlay,
            TextReadContext {
                ticket,
                objects,
                maps,
                budget: &mut scoped_budget,
            },
            &mut observation,
        );
        drop(scoped_budget);
        observation.close(match &result {
            Ok(_) => DiagnosticTerminalV1::Ok,
            Err(IndexError::Cancelled) => DiagnosticTerminalV1::Cancelled { code: 1 },
            Err(_) => DiagnosticTerminalV1::Error { code: 1 },
        });
        result
    }

    fn search_content_with_overlay_inner(
        &self,
        member: &MemberBinding,
        plan: &LexicalPlan,
        constraints: &quanta_index_contract::QueryConstraintSetV1,
        execution: TextContentExecution,
        overlay: Option<(&TextOverlayCorpus, &[TextOverlaySource<'_>])>,
        mut context: TextReadContext<'_>,
        observation: &mut QueryLexicalObservation,
    ) -> Result<NativeTextRanking, IndexError> {
        let TextContentExecution { top_k, policy } = execution;
        if let Some((corpus, _)) = overlay {
            corpus.validate_binding(context.ticket.view.identity, member)?;
        }
        policy.validate()?;
        if top_k == 0 || top_k > MAX_NATIVE_TEXT_TOP_K {
            return Err(IndexError::InvalidInput(
                "native BM25 top_k must be 1..=1024".into(),
            ));
        }
        let empty_filters = FilterPlan::empty(&plan.options);
        if !matches!(
            plan.options.pattern_type,
            quanta_index_contract::LqPatternType::Standard
                | quanta_index_contract::LqPatternType::Keyword
        ) || plan.filters.repo != empty_filters.repo
            || plan.filters.langs != empty_filters.langs
            || plan.filters.case != empty_filters.case
            || plan.filters.count != empty_filters.count
            || plan.filters.surface != empty_filters.surface
            || plan.filters.typed_unavailable != empty_filters.typed_unavailable
            || plan.raw_filters.len() != plan.filters.paths.len()
            || plan
                .raw_filters
                .iter()
                .zip(&plan.filters.paths)
                .any(|(raw, planned)| {
                    !matches!(raw, LqFilter::File { pattern, scope }
                    if pattern == &planned.pattern
                        && crate::filters::FilePathScope::from_lq(*scope) == planned.scope)
                })
            || !plan.directives.is_empty()
            || plan.plan_cap.is_some()
            || plan.options.timeout_ms.is_some()
            || plan.options.index_mode.is_some()
            || plan.options.boost_millis.is_some()
        {
            return Err(IndexError::InvalidInput(
                "native BM25 query options are unsupported".into(),
            ));
        }
        let capacity = result_capacity(top_k, plan.options.count)?;
        if let Some((corpus, sources)) = overlay {
            let mut documents = corpus.documents();
            for source in sources {
                let document = documents.next().ok_or(IndexError::Conflict)?;
                if document.owner != source.owner
                    || document.revision != source.revision
                    || source.path.is_empty()
                    || source.path.len() > MAX_SOURCE_PATH_BYTES
                    || source.path.starts_with('/')
                    || source.path.contains('\0')
                    || source
                        .path
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == "..")
                    || !corpus.validates_body(
                        source.owner,
                        source.revision,
                        source.body,
                        context.budget,
                    )?
                {
                    return Err(IndexError::Conflict);
                }
            }
            if documents.next().is_some() {
                return Err(IndexError::Conflict);
            }
        }
        let mut workspace = QueryWorkspace::new(capacity, context.budget)?;
        let QueryWorkspace {
            cursors,
            program,
            score_order,
            heads,
            heap,
            ..
        } = &mut workspace;
        // Compile before opening any postings. If a satisfying branch lacks
        // a positive anchor, the selected member must prove a finite admitted
        // owner universe before any posting or owner page can be traversed.
        let mut source_terms = [None; MAX_CONTENT_LEAVES];
        let mut source_term_count = 0usize;
        let mut source_regexes = [None; MAX_NATIVE_CONTENT_REGEX];
        let mut source_regex_count = 0usize;
        let anchored = compile_clauses(
            &plan.root,
            0,
            &mut Complexity::default(),
            program,
            &mut |term| {
                let ordinal = source_term_count;
                *source_terms
                    .get_mut(ordinal)
                    .ok_or(IndexError::ResourceLimited)? = Some(term);
                source_term_count = source_term_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Ok(ordinal)
            },
            &mut |raw, phrase| {
                if phrase.case_sensitive
                    != (plan.options.case_mode() == super::text::CaseMode::Sensitive)
                {
                    return Err(IndexError::InvalidInput(
                        "native phrase case differs from query policy".into(),
                    ));
                }
                super::text_phrase::validate_tokens(
                    raw,
                    &phrase.tokens,
                    plan.options.case_mode(),
                    context.budget,
                )
            },
            &mut |source| {
                let ordinal = source_regex_count;
                *source_regexes
                    .get_mut(ordinal)
                    .ok_or(IndexError::ResourceLimited)? = Some(source);
                source_regex_count = source_regex_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Ok(ordinal)
            },
        )?;
        let selection = super::text_filter::resolve(self, constraints, &mut context)?;
        let root = self.pinned_root(member, &mut context)?;
        if (source_regex_count != 0 || !anchored)
            && overlay.is_some_and(|(corpus, _)| {
                corpus.statistics().docs > super::text_universe::MAX_UNANCHORED_DOCS
            })
        {
            return Err(IndexError::ResourceLimited);
        }
        if source_regex_count != 0 {
            // The finite member census is checked before regex engine birth,
            // posting pages, or source bytes are visited.
            let _finite = TextOwnerCursor::new(root, self.recipe())?;
        }
        let mut universe = (!anchored)
            .then(|| TextOwnerCursor::new(root, self.recipe()))
            .transpose()?;
        let mut regex_sources = [""; MAX_NATIVE_CONTENT_REGEX];
        for (destination, source) in regex_sources
            .iter_mut()
            .zip(source_regexes.iter())
            .take(source_regex_count)
        {
            *destination = source.ok_or_else(|| {
                IndexError::BackendViolation("native regex source missing".into())
            })?;
        }
        let regex_program = NativeRegexProgram::compile(
            regex_sources
                .get(..source_regex_count)
                .ok_or(IndexError::ResourceLimited)?,
            &plan.filters.paths,
            plan.options.case_mode(),
            context.budget,
        )?;
        // A Content keyword can contain several canonical token runs. Keep
        // the original plan's source ordinals until every term has been
        // admitted, then lower multi-run keywords to the existing positional
        // phrase clause. All prepared keys own their original memory grants.
        let mut prepared_keys: [Option<PreparedKey>; MAX_CONTENT_LEAVES] =
            std::array::from_fn(|_slot| None);
        let mut source_ranges = [(0usize, 0usize); MAX_CONTENT_LEAVES];
        let mut term_remap = [0usize; MAX_CONTENT_LEAVES];
        let mut prepared_count = 0usize;
        let mut unique_count = 0usize;
        for (ordinal, term) in source_terms
            .get(..source_term_count)
            .ok_or(IndexError::ResourceLimited)?
            .iter()
            .enumerate()
        {
            let term = term.ok_or_else(|| {
                IndexError::BackendViolation("boolean term reference missing".into())
            })?;
            let first = prepared_count;
            let mut tokens = admitted_query_tokens(term, context.budget)?;
            while let Some(run) = tokens.next_run() {
                context.budget.checkpoint()?;
                if !run.is_indexable() {
                    return Err(IndexError::InvalidInput(
                        TextQueryError::TokenTooLong {
                            bytes: run.end.saturating_sub(run.start),
                            max: quanta_index_lq_text_normalizer::MAX_TOKEN_BYTES,
                        }
                        .to_string(),
                    ));
                }
                let next = prepared_count
                    .checked_add(1)
                    .filter(|count| *count <= MAX_CONTENT_LEAVES)
                    .ok_or_else(|| {
                        IndexError::InvalidInput("native query exceeds 64 term occurrences".into())
                    })?;
                let key = PreparedKey::from_case_term(
                    run.text,
                    plan.options.case_mode(),
                    context.budget,
                )?;
                let index = if let Some(existing) = prepared_keys
                    .get(..unique_count)
                    .ok_or(IndexError::ResourceLimited)?
                    .iter()
                    .position(|prior| {
                        prior
                            .as_ref()
                            .is_some_and(|prior| prior.as_slice() == key.as_slice())
                    }) {
                    existing
                } else {
                    *prepared_keys
                        .get_mut(unique_count)
                        .ok_or(IndexError::ResourceLimited)? = Some(key);
                    let index = unique_count;
                    unique_count = unique_count
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    index
                };
                *term_remap
                    .get_mut(prepared_count)
                    .ok_or(IndexError::ResourceLimited)? = index;
                prepared_count = next;
            }
            if prepared_count == first {
                return Err(IndexError::InvalidInput(
                    TextQueryError::NoTokens.to_string(),
                ));
            }
            *source_ranges
                .get_mut(ordinal)
                .ok_or(IndexError::ResourceLimited)? = (
                first,
                prepared_count
                    .checked_sub(first)
                    .ok_or(IndexError::ResourceLimited)?,
            );
        }
        for clause in program.iter_mut() {
            match clause {
                Clause::Term(source) => {
                    let (first, count) = *source_ranges
                        .get(*source)
                        .ok_or(IndexError::ResourceLimited)?;
                    if count == 1 {
                        *source = first;
                    } else {
                        *clause = Clause::Phrase {
                            first,
                            count,
                            cap: PhrasePolicy::defaults().default_candidate_cap,
                        };
                    }
                }
                Clause::Phrase { first, count, .. } => {
                    let end = first
                        .checked_add(*count)
                        .ok_or(IndexError::ResourceLimited)?;
                    let ranges = source_ranges.get(*first..end).ok_or_else(|| {
                        IndexError::BackendViolation("phrase source range exceeds query".into())
                    })?;
                    if ranges.iter().any(|(_start, length)| *length != 1) {
                        return Err(IndexError::BackendViolation(
                            "planned phrase term is not one token".into(),
                        ));
                    }
                    *first = ranges.first().ok_or(IndexError::ResourceLimited)?.0;
                }
                Clause::Regex(_) | Clause::All(_) | Clause::Any(_) | Clause::Not => {}
            }
        }
        for key in prepared_keys
            .get_mut(..unique_count)
            .ok_or(IndexError::ResourceLimited)?
            .iter_mut()
        {
            let key = key.take().ok_or_else(|| {
                IndexError::BackendViolation("prepared query term missing".into())
            })?;
            let cursor = self.posting_cursor_from_key(root, key, &mut context)?;
            cursors.push(cursor);
        }
        for clause in program.iter_mut() {
            if let Clause::Term(index) = clause {
                *index = *term_remap.get(*index).ok_or_else(|| {
                    IndexError::BackendViolation("boolean term remap out of range".into())
                })?;
            }
        }
        // Scores use canonical term order, independent of AST permutations.
        let key_bytes = cursors.iter().try_fold(0u64, |total, cursor| {
            let length =
                u64::try_from(cursor.key().len()).map_err(|_error| IndexError::ResourceLimited)?;
            total.checked_add(length).ok_or(IndexError::ResourceLimited)
        })?;
        context.budget.charge(WorkUsage {
            bytes: key_bytes,
            ..WorkUsage::default()
        })?;
        // Borrow the existing keys only while sorting bounded stack metadata;
        // persist indices before any cursor is advanced or mutably borrowed.
        let mut terms = [(b"".as_slice(), 0usize); MAX_CONTENT_LEAVES];
        let terms = terms.get_mut(..cursors.len()).ok_or_else(|| {
            IndexError::BackendViolation("canonical term order exceeds query width".into())
        })?;
        for (entry, (index, cursor)) in terms.iter_mut().zip(cursors.iter().enumerate()) {
            *entry = (cursor.key(), index);
        }
        terms.sort_unstable_by_key(|(key, _index)| *key);
        score_order.extend(terms.iter().map(|(_key, index)| *index));
        let (_owner_root, docs, indexed_tokens) = root.owner_universe();
        let stats = cursors.first().map(TextPostingCursor::statistics);
        if stats.is_some_and(|stats| {
            cursors.iter().any(|cursor| {
                let other = cursor.statistics();
                other.docs != stats.docs || other.indexed_tokens != stats.indexed_tokens
            })
        }) {
            return Err(IndexError::CorruptObject(
                "native BM25 cursor statistics mismatch".into(),
            ));
        }
        let durable_stats = stats.map_or(
            TextStatistics {
                docs,
                indexed_tokens,
            },
            |stats| TextStatistics {
                docs: stats.docs,
                indexed_tokens: stats.indexed_tokens,
            },
        );
        let stats = overlay.map_or(durable_stats, |(corpus, _)| corpus.statistics());
        let mut term_dfs = [0u64; MAX_CONTENT_LEAVES];
        // Fixed query-width scalars; no heap allocation or new retained cache.
        let mut score_bases = [None; MAX_CONTENT_LEAVES];
        for (index, cursor) in cursors.iter().enumerate() {
            let durable_df = cursor.statistics().df;
            *term_dfs.get_mut(index).ok_or(IndexError::ResourceLimited)? =
                if let Some((corpus, _)) = overlay {
                    corpus.adjusted_df(member, cursor.key(), durable_df, &mut context)?
                } else {
                    durable_df
                };
        }
        let view = context.ticket.view.identity;
        for cursor in cursors.iter_mut() {
            heads.push(observation.posting(cursor, &mut context)?);
        }
        let mut matched = 0u64;
        let mut phrase_candidates = [0u32; MAX_QUERY_NODES];
        let mut admitted = None;
        let mut overlay_documents = overlay.map(|(corpus, _)| corpus.documents().peekable());
        loop {
            context.budget.checkpoint()?;
            if universe.is_some() && admitted.is_none() {
                admitted = universe
                    .as_mut()
                    .map(|cursor| cursor.next(&mut context))
                    .transpose()?
                    .flatten();
            }
            if let Some(row) = admitted.as_ref()
                && heads.iter().flatten().any(|hit| hit.owner < row.owner)
            {
                return Err(IndexError::CorruptObject(
                    "native text posting owner is absent from admitted universe".into(),
                ));
            }
            let durable_candidate = if universe.is_some() {
                admitted.as_ref().map(|row| row.owner)
            } else {
                heads.iter().flatten().map(|hit| hit.owner).min()
            };
            let overlay_candidate = overlay_documents
                .as_mut()
                .and_then(|documents| documents.peek().map(|document| document.owner));
            let candidate = durable_candidate.into_iter().chain(overlay_candidate).min();
            let Some(owner) = candidate else {
                break;
            };
            let overlay_document = overlay.and_then(|(corpus, _)| corpus.document(owner));
            let masked = overlay.is_some_and(|(corpus, _)| corpus.masks(owner));
            if !masked || overlay_document.is_some() {
                let overlay_source = if let Some(document) = overlay_document {
                    let sources = overlay
                        .ok_or_else(|| {
                            IndexError::BackendViolation("overlay corpus lost source rows".into())
                        })?
                        .1;
                    let index = sources
                        .binary_search_by_key(&owner, |source| source.owner)
                        .map_err(|_error| IndexError::Conflict)?;
                    let source = sources.get(index).ok_or(IndexError::Conflict)?;
                    if source.revision != document.revision {
                        return Err(IndexError::Conflict);
                    }
                    Some(source)
                } else {
                    None
                };
                let mut source = overlay_document
                    .map(|document| NativeTextHitSource::Overlay {
                        revision: document.revision,
                    })
                    .or_else(|| {
                        admitted
                            .as_ref()
                            .filter(|row| row.owner == owner)
                            .map(|row| NativeTextHitSource::Durable(row.source_record))
                    });
                let mut document_tokens = overlay_document
                    .map(|document| document.indexed_tokens)
                    .or_else(|| {
                        admitted
                            .as_ref()
                            .filter(|row| row.owner == owner)
                            .map(|row| row.doc_indexed_tokens)
                    });
                let mut present = 0u64;
                let mut scores = [0.0; MAX_CONTENT_LEAVES];
                for (index, ((cursor, head), term_score)) in cursors
                    .iter()
                    .zip(heads.iter())
                    .zip(&mut scores)
                    .enumerate()
                {
                    if let Some(document) = overlay_document {
                        if let Some((corpus, _)) = overlay
                            && let Some(hit) = corpus.term_hit(owner, cursor.key())
                        {
                            if hit.document.revision != document.revision
                                || hit.document.indexed_tokens != document.indexed_tokens
                            {
                                return Err(IndexError::Conflict);
                            }
                            present |= 1u64 << index;
                            *term_score = score_with_basis(
                                policy,
                                stats,
                                *term_dfs.get(index).ok_or(IndexError::ResourceLimited)?,
                                u64::try_from(hit.tf())
                                    .map_err(|_error| IndexError::ResourceLimited)?,
                                document.indexed_tokens,
                                score_bases
                                    .get_mut(index)
                                    .ok_or(IndexError::ResourceLimited)?,
                            )?;
                        }
                    } else if let Some(hit) = head.as_ref().filter(|hit| hit.owner == owner) {
                        let hit_source = NativeTextHitSource::Durable(hit.source_record);
                        if source.is_some_and(|source| source != hit_source)
                            || document_tokens
                                .is_some_and(|tokens| tokens != hit.doc_indexed_tokens)
                        {
                            return Err(IndexError::CorruptObject(
                                "native BM25 boolean provenance mismatch".into(),
                            ));
                        }
                        source = Some(hit_source);
                        document_tokens = Some(hit.doc_indexed_tokens);
                        present |= 1u64 << index;
                        let tf = u64::try_from(hit.occurrence.positions.len())
                            .map_err(|_error| IndexError::ResourceLimited)?;
                        *term_score = score_with_basis(
                            policy,
                            stats,
                            *term_dfs.get(index).ok_or(IndexError::ResourceLimited)?,
                            tf,
                            hit.doc_indexed_tokens,
                            score_bases
                                .get_mut(index)
                                .ok_or(IndexError::ResourceLimited)?,
                        )?;
                    }
                }
                let source = source.ok_or_else(|| {
                    IndexError::BackendViolation("text candidate lost source".into())
                })?;
                let (regex_present, path_match) =
                    if regex_program.has_content() || !plan.filters.paths.is_empty() {
                        match source {
                            NativeTextHitSource::Durable(source_ref) => {
                                regex_program.evaluate(self, owner, source_ref, &mut context)?
                            }
                            NativeTextHitSource::Overlay { .. } => {
                                let overlay_source = overlay_source.ok_or(IndexError::Conflict)?;
                                regex_program.evaluate_overlay(
                                    overlay_source.path,
                                    overlay_source.body,
                                    context.budget,
                                )?
                            }
                        }
                    } else {
                        (0, true)
                    };
                let matching = evaluate_boolean(
                    program,
                    present,
                    regex_present,
                    |ordinal, first, count, cap| {
                        let end = first
                            .checked_add(count)
                            .ok_or(IndexError::ResourceLimited)?;
                        let order = term_remap.get(first..end).ok_or_else(|| {
                            IndexError::BackendViolation("phrase term range exceeds query".into())
                        })?;
                        let mut positional = [&[][..]; MAX_CONTENT_LEAVES];
                        let mut mask = 0u64;
                        for (slot, index) in order.iter().enumerate() {
                            let bit = 1u64
                                .checked_shl(
                                    u32::try_from(*index)
                                        .map_err(|_error| IndexError::ResourceLimited)?,
                                )
                                .ok_or(IndexError::ResourceLimited)?;
                            if present & bit == 0 {
                                return Ok(MatchState::default());
                            }
                            let positions = if let Some((corpus, _)) =
                                overlay.filter(|_| overlay_document.is_some())
                            {
                                corpus
                                    .term_hit(
                                        owner,
                                        cursors
                                            .get(*index)
                                            .ok_or(IndexError::ResourceLimited)?
                                            .key(),
                                    )
                                    .ok_or_else(|| {
                                        IndexError::BackendViolation(
                                            "overlay phrase candidate lacks posting".into(),
                                        )
                                    })?
                                    .positions
                            } else {
                                &heads
                                    .get(*index)
                                    .and_then(Option::as_ref)
                                    .filter(|hit| hit.owner == owner)
                                    .ok_or_else(|| {
                                        IndexError::BackendViolation(
                                            "phrase candidate lacks validated posting".into(),
                                        )
                                    })?
                                    .occurrence
                                    .positions
                            };
                            *positional
                                .get_mut(slot)
                                .ok_or(IndexError::ResourceLimited)? = positions;
                            mask |= bit;
                        }
                        let candidates = phrase_candidates
                            .get_mut(ordinal)
                            .ok_or(IndexError::ResourceLimited)?;
                        *candidates = candidates
                            .checked_add(1)
                            .filter(|next| *next <= cap)
                            .ok_or(IndexError::ResourceLimited)?;
                        let positions =
                            positional.get(..count).ok_or(IndexError::ResourceLimited)?;
                        let matched = observation.phrase(positions, context.budget)?;
                        Ok(MatchState {
                            matched,
                            positive_terms: if matched { mask } else { 0 },
                        })
                    },
                )?;
                let mut value = 0.0;
                for index in score_order.iter() {
                    if matching.positive_terms & (1u64 << index) != 0 {
                        value += scores.get(*index).copied().ok_or_else(|| {
                            IndexError::BackendViolation("boolean score index exceeds width".into())
                        })?;
                    }
                }
                if matching.positive_terms != 0 && (!value.is_finite() || value <= 0.0) {
                    return Err(IndexError::CorruptObject(
                        "nonfinite native BM25 boolean score".into(),
                    ));
                }
                let selected = match source {
                    NativeTextHitSource::Durable(reference) => {
                        selection.matches(owner, reference)?
                    }
                    NativeTextHitSource::Overlay { .. } => {
                        let overlay_source = overlay_source.ok_or(IndexError::Conflict)?;
                        constraints
                            .repo_relative_path_exact
                            .as_ref()
                            .is_none_or(|path| path.as_str() == overlay_source.path)
                    }
                };
                if matching.matched && path_match && selected {
                    matched = matched.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    let ranked = HeapHit(NativeTextRankedHit {
                        owner,
                        source,
                        score: value,
                    });
                    if heap.len() < capacity {
                        heap.push(ranked);
                    } else if heap.peek().is_some_and(|worst| ranked < *worst) {
                        let _discarded = heap.pop();
                        heap.push(ranked);
                    }
                }
            }
            // Masked durable hits still traverse to the end. This preserves
            // validation of malformed pages and declared DF on every branch.
            for (cursor, head) in cursors.iter_mut().zip(heads.iter_mut()) {
                if head.as_ref().is_some_and(|hit| hit.owner == owner) {
                    *head = observation.posting(cursor, &mut context)?;
                }
            }
            if admitted.as_ref().is_some_and(|row| row.owner == owner) {
                admitted = None;
            }
            if let Some(documents) = overlay_documents.as_mut()
                && documents
                    .peek()
                    .is_some_and(|document| document.owner == owner)
            {
                let _processed = documents.next();
            }
        }
        if universe.is_some() && heads.iter().any(Option::is_some) {
            return Err(IndexError::CorruptObject(
                "native text posting extends beyond admitted universe".into(),
            ));
        }
        let hit_bytes = capacity_bytes::<NativeTextRankedHit>(heap.len())?;
        context.budget.charge(WorkUsage {
            bytes: hit_bytes,
            ..WorkUsage::default()
        })?;
        let hit_memory = if hit_bytes == 0 {
            None
        } else {
            context
                .budget
                .reserve_memory(hit_bytes, MemoryLifetime::Retained)?
        };
        let mut ranked = NativeTextRanking {
            view,
            artifact: member.artifact,
            matched,
            hits: Vec::new(),
            _hit_memory: hit_memory,
        };
        ranked
            .hits
            .try_reserve_exact(heap.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        ranked.hits.extend(heap.drain().map(|hit| hit.0));
        ranked
            .hits
            .sort_unstable_by(|a, b| b.score.total_cmp(&a.score).then(a.owner.cmp(&b.owner)));
        context.budget.checkpoint()?;
        Ok(ranked)
    }
}

#[cfg(test)]
#[path = "text_query/count_tests.rs"]
mod count_tests;

#[cfg(test)]
mod tests {
    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        };
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(IndexError::BackendViolation(format!(
                        "assertion failed: left == right; left: {left:?}, right: {right:?}"
                    )));
                }
                _ => {}
            }
        }};
    }

    use super::*;

    fn matched_terms(program: &[Clause], present: u64) -> Result<u64, IndexError> {
        evaluate_boolean(program, present, 0, |_ordinal, _first, _count, _cap| {
            Err(IndexError::BackendViolation(
                "unexpected phrase in term-only gold".into(),
            ))
        })
        .map(|state| state.positive_terms)
    }

    #[test]
    fn nested_boolean_masks_match_independent_truth_table() -> Result<(), IndexError> {
        // (A AND B) OR C. Failed AB branches cannot add A's score to C.
        let program = [
            Clause::Term(0),
            Clause::Term(1),
            Clause::All(2),
            Clause::Term(2),
            Clause::Any(2),
        ];
        for (present, expected) in (0..8).zip([0, 0, 0, 3, 4, 4, 4, 7]) {
            check_eq!(matched_terms(&program, present)?, expected);
        }
        check_eq!(
            matched_terms(&[Clause::Term(0), Clause::Term(0), Clause::All(2)], 1)?,
            1
        );
        Ok(())
    }

    #[test]
    fn anchored_negation_filters_without_scoring_negative_terms() -> Result<(), IndexError> {
        // A AND NOT B. Only A supplies the candidate and its BM25 term mask.
        let conjunction = [
            Clause::Term(0),
            Clause::Term(1),
            Clause::Not,
            Clause::All(2),
        ];
        for (present, expected) in [0, 1, 2, 3].into_iter().zip([0, 1, 0, 0]) {
            check_eq!(matched_terms(&conjunction, present)?, expected);
        }

        // (A AND NOT B) OR C. A failed conjunction cannot add A's score to C.
        let disjunction = [
            Clause::Term(0),
            Clause::Term(1),
            Clause::Not,
            Clause::All(2),
            Clause::Term(2),
            Clause::Any(2),
        ];
        for (present, expected) in (0..8).zip([0, 1, 0, 0, 4, 5, 4, 4]) {
            check_eq!(matched_terms(&disjunction, present)?, expected);
        }

        // A AND NOT (B OR C), including both negated postings on one owner.
        let nested = [
            Clause::Term(0),
            Clause::Term(1),
            Clause::Term(2),
            Clause::Any(2),
            Clause::Not,
            Clause::All(2),
        ];
        for (present, expected) in (0..8).zip([0, 1, 0, 0, 0, 0, 0, 0]) {
            check_eq!(matched_terms(&nested, present)?, expected);
        }
        check_eq!(
            matched_terms(
                &[
                    Clause::Term(0),
                    Clause::Term(1),
                    Clause::Not,
                    Clause::Not,
                    Clause::All(2),
                ],
                3,
            )?,
            1
        );
        Ok(())
    }

    #[test]
    fn regex_branch_is_boolean_only_and_content_keeps_its_score_mask() -> Result<(), IndexError> {
        let program = [Clause::Regex(0), Clause::Term(0), Clause::Any(2)];
        let no_phrase = |_ordinal, _first, _count, _cap| {
            Err(IndexError::BackendViolation(
                "unexpected phrase in regex gold".into(),
            ))
        };
        let regex_only = evaluate_boolean(&program, 0, 1, no_phrase)?;
        check!(regex_only.matched);
        check_eq!(regex_only.positive_terms, 0);
        let both = evaluate_boolean(&program, 1, 1, no_phrase)?;
        check!(both.matched);
        check_eq!(both.positive_terms, 1);
        let neither = evaluate_boolean(&program, 0, 0, no_phrase)?;
        check!(!neither.matched);
        check_eq!(neither.positive_terms, 0);
        Ok(())
    }

    #[test]
    fn compilation_requires_a_positive_anchor_on_every_satisfying_branch() -> Result<(), IndexError>
    {
        let leaf = |term: &str| PlanNode::Leaf {
            leaf: PlanLeaf::Content { term: term.into() },
            cap: None,
        };
        let negative = || PlanNode::Not(Box::new(leaf("b")));
        let compile = |node: &PlanNode| {
            let mut program = Vec::new();
            compile_clauses(
                node,
                0,
                &mut Complexity::default(),
                &mut program,
                &mut |_term| Ok(0),
                &mut |_raw, _phrase| Ok(()),
                &mut |_source| Ok(0),
            )
        };
        check!(!compile(&negative())?);
        check!(!compile(&PlanNode::Any(vec![leaf("a"), negative()]))?);
        check!(compile(&PlanNode::All(vec![negative(), leaf("a")]))?);
        check!(compile(&PlanNode::Any(vec![
            PlanNode::All(vec![leaf("a"), negative()]),
            leaf("c"),
        ]))?);
        // A double complement may be satisfiable by B, but this packet
        // conservatively refuses to treat it as a positive scoring anchor.
        check!(!compile(&PlanNode::Not(Box::new(negative())))?);
        Ok(())
    }

    #[test]
    fn boolean_compilation_enforces_depth_node_and_leaf_bounds() -> Result<(), IndexError> {
        let leaf = || PlanNode::Leaf {
            leaf: PlanLeaf::Content { term: "one".into() },
            cap: None,
        };
        let compile = |node: &PlanNode| {
            let mut program = Vec::new();
            compile_clauses(
                node,
                0,
                &mut Complexity::default(),
                &mut program,
                &mut |_term| Ok(0),
                &mut |_raw, _phrase| Ok(()),
                &mut |_source| Ok(0),
            )
        };
        let mut nested = leaf();
        for _ in 0..MAX_QUERY_DEPTH {
            nested = PlanNode::All(vec![nested]);
        }
        check!(compile(&nested)?);
        check!(matches!(
            compile(&PlanNode::All(vec![nested])),
            Err(IndexError::InvalidInput(_))
        ));
        check!(compile(&PlanNode::Any((0..64).map(|_| leaf()).collect()))?);
        check!(matches!(
            compile(&PlanNode::Any((0..65).map(|_| leaf()).collect())),
            Err(IndexError::InvalidInput(_))
        ));
        let wide = PlanNode::Any(
            (0..64)
                .map(|_| PlanNode::All(vec![PlanNode::All(vec![PlanNode::All(vec![leaf()])])]))
                .collect(),
        );
        check!(matches!(compile(&wide), Err(IndexError::InvalidInput(_))));
        check!(matches!(
            compile(&PlanNode::Any(vec![])),
            Err(IndexError::InvalidInput(_))
        ));
        check!(!compile(&PlanNode::Not(Box::new(leaf())))?);
        Ok(())
    }

    #[test]
    fn ranking_policy_rejects_nonfinite_and_out_of_domain_parameters() {
        for policy in [
            Bm25Policy { k1: 0.0, b: 0.75 },
            Bm25Policy { k1: -1.0, b: 0.75 },
            Bm25Policy {
                k1: f64::INFINITY,
                b: 0.75,
            },
            Bm25Policy {
                k1: f64::NAN,
                b: 0.75,
            },
            Bm25Policy { k1: 1.2, b: -0.1 },
            Bm25Policy { k1: 1.2, b: 1.1 },
            Bm25Policy {
                k1: 1.2,
                b: f64::NAN,
            },
        ] {
            assert!(matches!(
                policy.validate(),
                Err(IndexError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn finite_extreme_policies_do_not_overflow_scoring() -> Result<(), IndexError> {
        let stats = TextStatistics {
            docs: 3,
            indexed_tokens: 6,
        };
        for k1 in [f64::from_bits(1), 1.2, f64::MAX] {
            for b in [0.0, 0.75, 1.0] {
                let policy = Bm25Policy { k1, b };
                policy.validate()?;
                check!(score(policy, stats, 2, 2, 4)?.is_finite());
            }
        }
        check!(matches!(
            score(Bm25Policy { k1: 1.2, b: 0.75 }, stats, 4, 1, 1),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }
}
