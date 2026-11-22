//! Page-bounded Content Boolean and positional phrase execution of the planner IR.

use super::text::{
    BoundTextMember, NativeTextIndex, SelectedTextRoot, TextPostingCursor, TextReadContext,
    TextStatistics, ValidatedPostingHit, admitted_query_tokens,
};
use super::text_memory::{
    MAX_TERM_KEY_BYTES, PreparedKey, append_canonical_case_key, prepare_indexed_token_cursor,
};
use super::text_overlay::MAX_TEXT_OVERLAY_DOCUMENT_BYTES;
use super::text_overlay::TextOverlayCorpus;
use super::text_path::PathTermCursor;
use super::text_path_group::PathGroupAccumulator;
use super::text_preflight::MAX_FILE_CONTAINS_LEAVES;
use super::text_regex::{MAX_NATIVE_CONTENT_REGEX, NativeRegexProgram};
use super::text_repo_predicate::RepoPredicateProgram;
use super::text_universe::TextOwnerCursor;
use crate::{
    filters::FilterPlan,
    phrase::PhrasePolicy,
    plan::{
        LexicalPlan, PlanLeaf, PlanNode, RepoFileConstraint, file_contains_raw_literal_supported,
    },
    symbol::ResultSurface,
};
pub(super) use preview_projection::{PreviewSelection, PreviewSource, PreviewVisitor};
use preview_truth::ClauseTruth;
use quanta_index_contract::native_protocol::MAX_NATIVE_TEXT_TOP_K;
use quanta_index_contract::{LqCountBound, LqFilter, LqSelect, RepoId};
use quanta_index_core::index::{
    FiniteScore, IndexError, MAX_SOURCE_PATH_BYTES, MemberBinding, MemoryLease, MemoryLifetime,
    ObjectRef, OwnerKey, SourceRecordOwner, ViewRef, WorkBudget, WorkUsage, owner_map_key,
};
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticSpanGuardV1,
    DiagnosticStageV1, DiagnosticTerminalV1,
};
use quanta_index_lq_text_normalizer::TextQueryError;
use std::time::Instant;
use std::{cmp::Ordering, collections::BinaryHeap};

enum MemberScope<'a> {
    Direct(&'a MemberBinding),
    Bound(&'a BoundTextMember),
}

#[derive(Clone, Copy)]
struct ContentQuery<'a> {
    plan: &'a LexicalPlan,
    constraints: &'a quanta_index_contract::QueryConstraintSetV1,
    path_rank_after: PathRankAfter<'a>,
}

type PathRankAfter<'a> = Option<(FiniteScore, &'a RepoId, &'a str)>;
type PathGroupPayload = (NativeTextRankedHit, Option<ClauseTruth>, bool);

struct PathHit {
    owner: OwnerKey,
    source: ObjectRef,
    record: SourceRecordOwner,
}

fn next_path_hit(
    cursor: &mut PathTermCursor,
    index: &NativeTextIndex,
    context: &mut TextReadContext<'_>,
) -> Result<Option<PathHit>, IndexError> {
    let Some((owner, source)) = cursor.next(context.maps, context.budget)? else {
        return Ok(None);
    };
    let record =
        super::text::authenticated_text_source(index, owner, Some(source), true, context)?;
    Ok(Some(PathHit {
        owner,
        source,
        record,
    }))
}

/// Recheck a path posting against its exact pinned field, including adjacency
/// across skipped long runs. The first indexed term is the posting anchor.
fn path_phrase_stats(
    path: &str,
    phrase: &[usize],
    cursors: &[TextPostingCursor],
    case: quanta_index_lq_text_normalizer::CaseMode,
    budget: &mut WorkBudget,
) -> Result<(u64, u64), IndexError> {
    let ord = phrase
        .len()
        .checked_sub(1)
        .and_then(|ordinal| u32::try_from(ordinal).ok())
        .ok_or(IndexError::ResourceLimited)?;
    let last = 1u64.checked_shl(ord).ok_or(IndexError::ResourceLimited)?;
    let _memory = budget.reserve_memory(MAX_TERM_KEY_BYTES, MemoryLifetime::Temporary)?;
    let capacity =
        usize::try_from(MAX_TERM_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?;
    let mut key = Vec::new();
    key.try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut cursor = prepare_indexed_token_cursor(path, budget)?;
    let mut state = 0u64;
    let mut tokens = 0u64;
    let mut hits = 0u64;
    let mut anchored = false;
    while let Some(run) = cursor.next_run() {
        budget.checkpoint()?;
        if !run.is_indexable() {
            state = 0;
            continue;
        }
        key.clear();
        append_canonical_case_key(&mut key, run.text, case, capacity)?;
        let mut same = 0u64;
        for (ordinal, slot) in phrase.iter().enumerate() {
            let term = cursors.get(*slot).ok_or(IndexError::ResourceLimited)?.key();
            budget.charge(WorkUsage {
                bytes: u64::try_from(term.len().min(key.len()))
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            if key.as_slice() == term {
                let shift = u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?;
                same |= 1u64.checked_shl(shift).ok_or(IndexError::ResourceLimited)?;
            }
        }
        anchored |= same & 1 != 0;
        state = ((state << 1) | 1) & same;
        tokens = tokens.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        if state & last != 0 {
            hits = hits.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
    }
    if !anchored {
        return Err(IndexError::CorruptObject(
            "native path posting term is absent from its selected source path".into(),
        ));
    }
    Ok((tokens, hits))
}

impl MemberScope<'_> {
    fn member(&self) -> &MemberBinding {
        match self {
            Self::Direct(member) => member,
            Self::Bound(bound) => bound.member(),
        }
    }

    fn root(
        &self,
        index: &NativeTextIndex,
        context: &mut TextReadContext<'_>,
    ) -> Result<SelectedTextRoot, IndexError> {
        match self {
            Self::Direct(member) => index.pinned_root(member, context),
            Self::Bound(bound) => bound.root(index, context),
        }
    }
}

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
        occurrences: &mut super::text_occurrence::OccurrenceReader,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<ValidatedPostingHit>, IndexError> {
        let Some(_span) = self.span.as_ref() else {
            return cursor.next_hit(context, occurrences);
        };
        Self::add(&mut self.posting_calls, Some(1), &mut self.overflowed);
        let started = Instant::now();
        let result = cursor.next_hit(context, occurrences);
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
    Contains(usize),
    Repo(usize),
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
    contains: usize,
    repositories: usize,
}

#[expect(
    clippy::too_many_arguments,
    reason = "The private compiler passes independent term, phrase, regex, contains and repository resolvers"
)]
fn compile_clauses_with_repo<'a>(
    node: &'a PlanNode,
    depth: usize,
    complexity: &mut Complexity,
    program: &mut Vec<Clause>,
    term: &mut impl FnMut(&'a str) -> Result<usize, IndexError>,
    phrase: &mut impl FnMut(&str, &crate::phrase::PhrasePlan) -> Result<(), IndexError>,
    regex: &mut impl FnMut(&'a str) -> Result<usize, IndexError>,
    contains: &mut impl FnMut(&'a str) -> Result<usize, IndexError>,
    repo: &mut impl FnMut(&'a RepoFileConstraint) -> Result<usize, IndexError>,
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
            leaf: PlanLeaf::FileContains { needle },
            cap: None,
        } => {
            if !file_contains_raw_literal_supported(needle) {
                return Err(IndexError::InvalidInput(
                    "native file.contains requires a bounded literal raw substring".into(),
                ));
            }
            complexity.contains = complexity
                .contains
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if complexity.contains > MAX_FILE_CONTAINS_LEAVES {
                return Err(IndexError::ResourceLimited);
            }
            program.push(Clause::Contains(contains(needle)?));
            Ok(false)
        }
        PlanNode::Leaf {
            leaf: PlanLeaf::RepoHasFile { constraint },
            cap: None,
        } => {
            complexity.repositories = complexity
                .repositories
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if complexity.repositories > 4 {
                return Err(IndexError::ResourceLimited);
            }
            program.push(Clause::Repo(repo(constraint)?));
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
                let child_anchored = compile_clauses_with_repo(
                    child,
                    depth.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                    complexity,
                    program,
                    term,
                    phrase,
                    regex,
                    contains,
                    repo,
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
            let _child_anchored = compile_clauses_with_repo(
                child,
                depth.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                complexity,
                program,
                term,
                phrase,
                regex,
                contains,
                repo,
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

#[cfg(test)]
fn compile_clauses<'a>(
    node: &'a PlanNode,
    depth: usize,
    complexity: &mut Complexity,
    program: &mut Vec<Clause>,
    term: &mut impl FnMut(&'a str) -> Result<usize, IndexError>,
    phrase: &mut impl FnMut(&str, &crate::phrase::PhrasePlan) -> Result<(), IndexError>,
    regex: &mut impl FnMut(&'a str) -> Result<usize, IndexError>,
    contains: &mut impl FnMut(&'a str) -> Result<usize, IndexError>,
) -> Result<bool, IndexError> {
    compile_clauses_with_repo(
        node,
        depth,
        complexity,
        program,
        term,
        phrase,
        regex,
        contains,
        &mut |_constraint| {
            Err(IndexError::InvalidInput(
                "repo predicate requires native witness preparation".into(),
            ))
        },
    )
}

// A NOT branch changes truth but contributes no ranking terms. Failed AND
// branches contribute no terms through an enclosing OR. Truth and score mask
// must remain distinct: an unanchored match may correctly have zero score.
fn evaluate_boolean_with_repo(
    program: &[Clause],
    present: u64,
    regex_present: u8,
    contains_present: u8,
    repo_present: u8,
    mut observe_truth: impl FnMut(usize, bool) -> Result<(), IndexError>,
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
            Clause::Contains(index) => {
                let bit = 1u8
                    .checked_shl(
                        u32::try_from(index).map_err(|_error| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
                MatchState {
                    matched: contains_present & bit != 0,
                    positive_terms: 0,
                }
            }
            Clause::Repo(index) => {
                let bit = 1u8
                    .checked_shl(
                        u32::try_from(index).map_err(|_width| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
                MatchState {
                    matched: repo_present & bit != 0,
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
        observe_truth(ordinal, state.matched)?;
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

#[cfg(test)]
fn evaluate_boolean(
    program: &[Clause],
    present: u64,
    regex_present: u8,
    contains_present: u8,
    phrase: impl FnMut(usize, usize, usize, u32) -> Result<MatchState, IndexError>,
) -> Result<MatchState, IndexError> {
    evaluate_boolean_with_repo(
        program,
        present,
        regex_present,
        contains_present,
        0,
        |_ordinal, _matched| Ok(()),
        phrase,
    )
}

pub(super) fn file_contains_mask(
    source_text: &str,
    needles: &[&str],
    case: super::text::CaseMode,
    budget: &mut WorkBudget,
) -> Result<u8, IndexError> {
    if needles.len() > MAX_FILE_CONTAINS_LEAVES
        || needles
            .iter()
            .any(|needle| !file_contains_raw_literal_supported(needle))
    {
        return Err(IndexError::InvalidInput(
            "native file.contains requires bounded literal raw substrings".into(),
        ));
    }
    let text_bytes =
        u64::try_from(source_text.len()).map_err(|_width| IndexError::ResourceLimited)?;
    let needle_bytes = needles.iter().try_fold(0u64, |sum, needle| {
        let len = u64::try_from(needle.len()).map_err(|_width| IndexError::ResourceLimited)?;
        sum.checked_add(len).ok_or(IndexError::ResourceLimited)
    })?;
    let backing = text_bytes
        .checked_add(needle_bytes)
        .and_then(|bytes| bytes.checked_mul(16))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget.reserve_memory(backing, MemoryLifetime::Temporary)?;
    budget.charge(WorkUsage {
        bytes: text_bytes,
        ..WorkUsage::default()
    })?;
    budget.checkpoint()?;
    let normalized = crate::normalize::nfc(source_text);
    let folded = crate::normalize::apply_case(normalized.as_ref(), case);
    let normalized_bytes =
        u64::try_from(normalized.len()).map_err(|_width| IndexError::ResourceLimited)?;
    let folded_bytes = u64::try_from(folded.len()).map_err(|_width| IndexError::ResourceLimited)?;
    let source_prepare_bytes = normalized_bytes
        .checked_add(folded_bytes)
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: source_prepare_bytes,
        ..WorkUsage::default()
    })?;
    let mut mask = 0u8;
    for (ordinal, needle) in needles.iter().enumerate() {
        budget.checkpoint()?;
        let needle_len =
            u64::try_from(needle.len()).map_err(|_width| IndexError::ResourceLimited)?;
        let scan_bytes = folded_bytes
            .checked_add(needle_len)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: scan_bytes,
            ..WorkUsage::default()
        })?;
        let normalized_needle = crate::normalize::nfc(needle);
        let folded_needle = crate::normalize::apply_case(normalized_needle.as_ref(), case);
        let normalized_needle_bytes =
            u64::try_from(normalized_needle.len()).map_err(|_width| IndexError::ResourceLimited)?;
        let folded_needle_bytes =
            u64::try_from(folded_needle.len()).map_err(|_width| IndexError::ResourceLimited)?;
        let literal_prepare_bytes = normalized_needle_bytes
            .checked_add(folded_needle_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: literal_prepare_bytes,
            ..WorkUsage::default()
        })?;
        if folded.contains(folded_needle.as_ref()) {
            mask |= 1u8
                .checked_shl(u32::try_from(ordinal).map_err(|_width| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
        }
    }
    budget.checkpoint()?;
    Ok(mask)
}

fn file_contains_durable(
    index: &NativeTextIndex,
    owner: OwnerKey,
    source: ObjectRef,
    needles: &[&str],
    case: super::text::CaseMode,
    allow_excluded_path: bool,
    context: &mut TextReadContext<'_>,
) -> Result<u8, IndexError> {
    context.budget.checkpoint()?;
    if context.maps.lookup_object(
        context.ticket.view.source.owners,
        &owner_map_key(owner),
        context.budget,
    )? != Some(source)
    {
        return Err(IndexError::CorruptObject(
            "native file.contains source owner differs from selected view".into(),
        ));
    }
    let stored = context.objects.get(source, context.budget)?;
    let record =
        SourceRecordOwner::decode_json(&stored.payload, context.budget, MemoryLifetime::Temporary)?;
    let admitted = index.validate_selected_source(&record)?;
    if stored.registration.reference != source
        || stored.registration.children.as_slice() != [record.bytes]
        || (!admitted && !allow_excluded_path)
        || record.path.is_empty()
        || record.path.len() > MAX_SOURCE_PATH_BYTES
        || record.path.starts_with('/')
        || record.path.contains('\0')
        || record
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(IndexError::CorruptObject(
            "native file.contains source record differs from admitted selection".into(),
        ));
    }
    if !admitted {
        return Ok(0);
    }
    let body = context.objects.get(record.bytes, context.budget)?;
    if body.registration.reference != record.bytes
        || !body.registration.children.is_empty()
        || u64::try_from(body.payload.len()).map_err(|_width| IndexError::ResourceLimited)?
            != record.byte_len
    {
        return Err(IndexError::CorruptObject(
            "native file.contains source bytes differ from admitted leaf".into(),
        ));
    }
    let text = std::str::from_utf8(&body.payload).map_err(|_utf8| {
        IndexError::CorruptObject("native file.contains admitted source is not UTF-8".into())
    })?;
    file_contains_mask(text, needles, case, context.budget)
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
    /// Continue strictly after this score/owner in the selected immutable view.
    /// The caller authenticates the anchor's view, query and artifact binding.
    pub rank_after: Option<(FiniteScore, OwnerKey)>,
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
    /// Eligible matches strictly after the anchor, before the bounded heap.
    pub remaining: u64,
    pub hits: Vec<NativeTextRankedHit>,
    // Returned ranking data may outlive its producing execution reservation.
    _hit_memory: Option<MemoryLease>,
}

impl PartialEq for NativeTextRanking {
    fn eq(&self, other: &Self) -> bool {
        self.view == other.view
            && self.artifact == other.artifact
            && self.matched == other.matched
            && self.remaining == other.remaining
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
    heap: TextHeap,
    _memory: Option<MemoryLease>,
}

impl QueryWorkspace {
    fn new(top_k: usize, with_preview: bool, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let bytes = [
            capacity_bytes::<TextPostingCursor>(MAX_CONTENT_LEAVES)?,
            capacity_bytes::<Clause>(MAX_QUERY_NODES)?,
            capacity_bytes::<usize>(MAX_CONTENT_LEAVES)?,
            capacity_bytes::<Option<ValidatedPostingHit>>(MAX_CONTENT_LEAVES)?,
            if with_preview {
                capacity_bytes::<HeapHit<ClauseTruth>>(top_k)?
            } else {
                capacity_bytes::<HeapHit<()>>(top_k)?
            },
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
            heap: TextHeap::new(top_k, with_preview)?,
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
        Ok(work)
    }
}

enum TextHeap {
    Ranked(BinaryHeap<HeapHit<()>>),
    Preview(BinaryHeap<HeapHit<ClauseTruth>>),
}

impl TextHeap {
    fn new(capacity: usize, with_preview: bool) -> Result<Self, IndexError> {
        fn heap<T>(capacity: usize) -> Result<BinaryHeap<HeapHit<T>>, IndexError> {
            let mut heap = BinaryHeap::new();
            heap.try_reserve_exact(capacity)
                .map_err(|_error| IndexError::ResourceLimited)?;
            Ok(heap)
        }
        if with_preview {
            Ok(Self::Preview(heap(capacity)?))
        } else {
            Ok(Self::Ranked(heap(capacity)?))
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Ranked(heap) => heap.len(),
            Self::Preview(heap) => heap.len(),
        }
    }

    fn consider(
        &mut self,
        capacity: usize,
        hit: NativeTextRankedHit,
        truth: Option<ClauseTruth>,
    ) -> Result<(), IndexError> {
        match (self, truth) {
            (Self::Ranked(heap), None) => retain_hit(heap, capacity, HeapHit(hit, (), false)),
            (Self::Preview(heap), Some(truth)) => {
                retain_hit(heap, capacity, HeapHit(hit, truth, false));
            }
            _ => return Err(preview_mode_error()),
        }
        Ok(())
    }
}

fn preview_mode_error() -> IndexError {
    IndexError::BackendViolation("native preview heap mode differs from query".into())
}

struct HeapHit<T>(NativeTextRankedHit, T, bool);

fn retain_hit<T>(heap: &mut BinaryHeap<HeapHit<T>>, capacity: usize, candidate: HeapHit<T>) {
    if heap.len() < capacity {
        heap.push(candidate);
    } else if heap.peek().is_some_and(|worst| candidate < *worst) {
        let _discarded = heap.pop();
        heap.push(candidate);
    }
}

// Ordering in the result: best first, including the complete source identity.
// A maximum heap with this order retains its worst hit at the root.
fn compare_rank(score: f64, owner: OwnerKey, other_score: f64, other_owner: OwnerKey) -> Ordering {
    other_score.total_cmp(&score).then(owner.cmp(&other_owner))
}

impl<T> PartialEq for HeapHit<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl<T> Eq for HeapHit<T> {}
impl<T> PartialOrd for HeapHit<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for HeapHit<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // The worst retained hit is the heap maximum.
        compare_rank(self.0.score, self.0.owner, other.0.score, other.0.owner)
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
            TextContentExecution {
                top_k,
                policy,
                rank_after: None,
            },
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
        self.search_content_with_scope(
            &MemberScope::Direct(member),
            ContentQuery {
                plan,
                constraints,
                path_rank_after: None,
            },
            execution,
            overlay,
            None,
            context,
        )
    }

    pub(super) fn search_content_with_bound_member(
        &self,
        bound: &BoundTextMember,
        plan: &LexicalPlan,
        constraints: &quanta_index_contract::QueryConstraintSetV1,
        execution: TextContentExecution,
        path_rank_after: PathRankAfter<'_>,
        overlay: Option<(&TextOverlayCorpus, &[TextOverlaySource<'_>])>,
        preview: Option<&mut PreviewVisitor<'_>>,
        context: TextReadContext<'_>,
    ) -> Result<NativeTextRanking, IndexError> {
        self.search_content_with_scope(
            &MemberScope::Bound(bound),
            ContentQuery {
                plan,
                constraints,
                path_rank_after,
            },
            execution,
            overlay,
            preview,
            context,
        )
    }

    fn search_content_with_scope(
        &self,
        member_scope: &MemberScope<'_>,
        query: ContentQuery<'_>,
        execution: TextContentExecution,
        overlay: Option<(&TextOverlayCorpus, &[TextOverlaySource<'_>])>,
        preview: Option<&mut PreviewVisitor<'_>>,
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
            member_scope,
            query,
            execution,
            overlay,
            preview,
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
        member_scope: &MemberScope<'_>,
        query: ContentQuery<'_>,
        execution: TextContentExecution,
        overlay: Option<(&TextOverlayCorpus, &[TextOverlaySource<'_>])>,
        preview: Option<&mut PreviewVisitor<'_>>,
        mut context: TextReadContext<'_>,
        observation: &mut QueryLexicalObservation,
    ) -> Result<NativeTextRanking, IndexError> {
        let member = member_scope.member();
        let ContentQuery {
            plan,
            constraints,
            path_rank_after,
        } = query;
        let TextContentExecution {
            top_k,
            policy,
            rank_after,
        } = execution;
        let path_mode = plan.filters.surface == ResultSurface::Path;
        if (path_mode && (overlay.is_some() || rank_after.is_some()))
            || (path_rank_after.is_some() && (!path_mode || plan.options.count.is_some()))
        {
            return Err(IndexError::InvalidInput(
                "native path groups require a durable path projection and a path anchor".into(),
            ));
        }
        if rank_after.is_some_and(|(score, _owner)| score.get() < 0.0)
            || (rank_after.is_some() && plan.options.count.is_some())
        {
            return Err(IndexError::InvalidInput(
                "native BM25 rank-after requires a nonnegative score and no count option".into(),
            ));
        }
        let mut occurrences = super::text_occurrence::OccurrenceReader::new(64)?;
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
        let path_selects = plan
            .raw_filters
            .iter()
            .filter(|raw| {
                matches!(
                    raw,
                    LqFilter::Select {
                        dim: LqSelect::Path
                    }
                )
            })
            .count();
        let file_filters = plan
            .raw_filters
            .iter()
            .filter(|raw| matches!(raw, LqFilter::File { .. }));
        if !matches!(
            plan.options.pattern_type,
            quanta_index_contract::LqPatternType::Standard
                | quanta_index_contract::LqPatternType::Keyword
        ) || plan.filters.repo != empty_filters.repo
            || plan.filters.langs != empty_filters.langs
            || plan.filters.case != empty_filters.case
            || plan.filters.count != empty_filters.count
            || (!path_mode && plan.filters.surface != empty_filters.surface)
            || path_selects != usize::from(path_mode)
            || plan.filters.typed_unavailable != empty_filters.typed_unavailable
            || plan.raw_filters.len() != plan.filters.paths.len().saturating_add(path_selects)
            || file_filters.clone().count() != plan.filters.paths.len()
            || file_filters.zip(&plan.filters.paths).any(|(raw, planned)| {
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
        let with_preview = preview.is_some();
        let owner_capacity = if path_mode { 0 } else { capacity };
        let mut workspace = QueryWorkspace::new(owner_capacity, with_preview, context.budget)?;
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
        let mut source_contains = [None; MAX_FILE_CONTAINS_LEAVES];
        let mut source_contains_count = 0usize;
        let mut source_repositories: [Option<&RepoFileConstraint>; 4] = [None; 4];
        let mut source_repository_count = 0usize;
        let anchored = compile_clauses_with_repo(
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
            &mut |needle| {
                let ordinal = source_contains_count;
                *source_contains
                    .get_mut(ordinal)
                    .ok_or(IndexError::ResourceLimited)? = Some(needle);
                source_contains_count = source_contains_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Ok(ordinal)
            },
            &mut |constraint| {
                let ordinal = source_repository_count;
                *source_repositories
                    .get_mut(ordinal)
                    .ok_or(IndexError::ResourceLimited)? = Some(constraint);
                source_repository_count = source_repository_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                Ok(ordinal)
            },
        )?;
        if source_repository_count != 0 && overlay.is_some() {
            return Err(IndexError::InvalidInput(
                "native repo.has.file requires authenticated overlay repository identity".into(),
            ));
        }
        let repository_constraints = source_repositories
            .get(..source_repository_count)
            .ok_or(IndexError::ResourceLimited)?;
        let selection =
            super::text_filter::resolve_for_surface(self, constraints, path_mode, &mut context)?;
        let root = member_scope.root(self, &mut context)?;
        let path_field = path_mode.then(|| root.path_field()).transpose()?;
        let mut path_groups = path_field
            .map(|(_, stats, _)| {
                PathGroupAccumulator::new(stats.docs, |(hit, _, _): &PathGroupPayload| {
                    (hit.owner, hit.score)
                })
            })
            .transpose()?;
        if (source_regex_count != 0 || source_contains_count != 0 || !anchored)
            && overlay.is_some_and(|(corpus, _)| {
                corpus.statistics().docs > super::text_universe::MAX_UNANCHORED_DOCS
            })
        {
            return Err(IndexError::ResourceLimited);
        }
        if source_regex_count != 0 || source_contains_count != 0 {
            // The finite member census is checked before regex engine birth,
            // posting pages, or source bytes are visited.
            TextOwnerCursor::check_statistics(root.statistics())?;
        }
        let mut universe = (!anchored)
            .then(|| TextOwnerCursor::new(root, self.recipe()))
            .transpose()?;
        let repository_program = RepoPredicateProgram::prepare(
            repository_constraints,
            plan.options.case_mode(),
            self,
            &mut context,
        )?;
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
        let mut contains_needles = [""; MAX_FILE_CONTAINS_LEAVES];
        for (destination, source) in contains_needles
            .iter_mut()
            .zip(source_contains.iter())
            .take(source_contains_count)
        {
            *destination = source.ok_or_else(|| {
                IndexError::BackendViolation("native file.contains source missing".into())
            })?;
        }
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
                Clause::Regex(_)
                | Clause::Contains(_)
                | Clause::Repo(_)
                | Clause::All(_)
                | Clause::Any(_)
                | Clause::Not => {}
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
        let TextStatistics {
            docs,
            indexed_tokens,
        } = root.statistics();
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
        let keyword_path = path_field.is_some()
            && matches!(
                &plan.root,
                PlanNode::Leaf {
                    leaf: PlanLeaf::Content { .. },
                    ..
                }
            );
        let path_phrase = if keyword_path {
            term_remap
                .get(..prepared_count)
                .ok_or(IndexError::ResourceLimited)?
        } else {
            &[]
        };
        let mut path_cursor = if keyword_path {
            let first = path_phrase.first().ok_or(IndexError::ResourceLimited)?;
            let key = cursors
                .get(*first)
                .ok_or(IndexError::ResourceLimited)?
                .key();
            let (postings, _, value) = path_field.ok_or(IndexError::ResourceLimited)?;
            Some(PathTermCursor::new(postings, key, value, context.budget)?)
        } else {
            None
        };
        let path_df = path_cursor
            .as_ref()
            .map(|cursor| cursor.df(context.maps, context.budget))
            .transpose()?
            .unwrap_or_default();
        if path_field.is_some_and(|(_, stats, _)| path_df > stats.docs) {
            return Err(IndexError::CorruptObject(
                "native path DF exceeds documents".into(),
            ));
        }
        let mut path_basis = None;
        let mut path_visited = 0u64;
        let mut path_head = path_cursor
            .as_mut()
            .map(|cursor| next_path_hit(cursor, self, &mut context))
            .transpose()?
            .flatten();
        for cursor in cursors.iter_mut() {
            heads.push(observation.posting(cursor, &mut occurrences, &mut context)?);
        }
        let mut matched = 0u64;
        let mut remaining = 0u64;
        let mut anchor_found = rank_after.is_none();
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
            let path_owner = path_head.as_ref().map(|hit| hit.owner);
            let candidate = durable_candidate
                .into_iter()
                .chain(path_owner)
                .chain(overlay_candidate)
                .min();
            let Some(owner) = candidate else {
                break;
            };
            let overlay_document = overlay.and_then(|(corpus, _)| corpus.document(owner));
            let path_hit = if path_owner == Some(owner) {
                path_head.take()
            } else {
                None
            };
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
                    })
                    .or_else(|| {
                        path_hit
                            .as_ref()
                            .map(|hit| NativeTextHitSource::Durable(hit.source))
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
                                regex_program.evaluate(
                                    self,
                                    owner,
                                    source_ref,
                                    path_mode,
                                    &mut context,
                                )?
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
                let contains_present = if source_contains_count == 0 || !selected || !path_match {
                    0
                } else {
                    let needles = contains_needles
                        .get(..source_contains_count)
                        .ok_or(IndexError::ResourceLimited)?;
                    match source {
                        NativeTextHitSource::Durable(source_ref) => file_contains_durable(
                            self,
                            owner,
                            source_ref,
                            needles,
                            plan.options.case_mode(),
                            path_mode,
                            &mut context,
                        )?,
                        NativeTextHitSource::Overlay { .. } => {
                            let overlay_source = overlay_source.ok_or(IndexError::Conflict)?;
                            if overlay_source.path.is_empty()
                                || overlay_source.path.len() > MAX_SOURCE_PATH_BYTES
                                || overlay_source.path.starts_with('/')
                                || overlay_source.path.contains('\0')
                                || overlay_source
                                    .path
                                    .split('/')
                                    .any(|part| part.is_empty() || part == "." || part == "..")
                                || overlay_source.body.len() > MAX_TEXT_OVERLAY_DOCUMENT_BYTES
                            {
                                return Err(IndexError::InvalidInput(
                                    "native file.contains overlay source exceeds admitted shape"
                                        .into(),
                                ));
                            }
                            file_contains_mask(
                                overlay_source.body,
                                needles,
                                plan.options.case_mode(),
                                context.budget,
                            )?
                        }
                    }
                };
                let repo_present = if source_repository_count == 0 || !selected || !path_match {
                    0
                } else {
                    match source {
                        NativeTextHitSource::Durable(reference) => repository_program.mask(self, owner, reference, &mut context)?,
                        NativeTextHitSource::Overlay { .. } => return Err(IndexError::InvalidInput(
                            "native repo.has.file requires authenticated overlay repository identity".into(),
                        )),
                    }
                };
                let evaluate_phrase = |ordinal: usize, first: usize, count: usize, cap: u32| {
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
                };
                let mut truth = with_preview.then(ClauseTruth::default);
                let matching = if let Some(trace) = truth.as_mut() {
                    evaluate_boolean_with_repo(
                        program,
                        present,
                        regex_present,
                        contains_present,
                        repo_present,
                        |ordinal, matched| trace.record(ordinal, matched),
                        evaluate_phrase,
                    )?
                } else {
                    evaluate_boolean_with_repo(
                        program,
                        present,
                        regex_present,
                        contains_present,
                        repo_present,
                        |_ordinal, _matched| Ok(()),
                        evaluate_phrase,
                    )?
                };
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
                let mut matched_path = false;
                if let Some(hit) = path_hit.as_ref() {
                    if source != NativeTextHitSource::Durable(hit.source) {
                        return Err(IndexError::CorruptObject(
                            "native path and body candidates differ from the selected source"
                                .into(),
                        ));
                    }
                    let (_, path_stats, _) = path_field.ok_or(IndexError::ResourceLimited)?;
                    let (length, tf) = path_phrase_stats(
                        &hit.record.path,
                        path_phrase,
                        cursors,
                        plan.options.case_mode(),
                        context.budget,
                    )?;
                    if tf != 0 {
                        value += score_with_basis(
                            policy,
                            path_stats,
                            path_df,
                            tf,
                            length,
                            &mut path_basis,
                        )?;
                        matched_path = true;
                    }
                }
                if (matching.matched || matched_path) && path_match && selected {
                    if !value.is_finite() {
                        return Err(IndexError::CorruptObject(
                            "nonfinite native grouped score".into(),
                        ));
                    }
                    let hit = NativeTextRankedHit {
                        owner,
                        source,
                        score: value,
                    };
                    if let Some(groups) = path_groups.as_mut() {
                        let NativeTextHitSource::Durable(source) = source else {
                            return Err(IndexError::Conflict);
                        };
                        let record = if let Some(path_hit) = path_hit {
                            path_hit.record
                        } else {
                            super::text::authenticated_text_source(
                                self,
                                owner,
                                Some(source),
                                false,
                                &mut context,
                            )?
                        };
                        let (mut record, memory) = record.into_retained_parts(context.budget)?;
                        let repository = record
                            .source_repo_id
                            .take()
                            .ok_or(IndexError::SnapshotUnavailable)?;
                        groups.consider(
                            (hit, truth, matched_path && !matching.matched),
                            repository,
                            record.path,
                            memory,
                            context.budget,
                        )?;
                    } else {
                        matched = matched.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                        let after_order = rank_after.map(|(score, anchor_owner)| {
                            compare_rank(value, owner, score.get(), anchor_owner)
                        });
                        anchor_found |= after_order == Some(Ordering::Equal);
                        if after_order.is_none_or(|order| order == Ordering::Greater) {
                            remaining = remaining
                                .checked_add(1)
                                .ok_or(IndexError::ResourceLimited)?;
                            heap.consider(capacity, hit, truth)?;
                        }
                    }
                }
            }
            // Masked durable hits still traverse to the end. This preserves
            // validation of malformed pages and declared DF on every branch.
            for (cursor, head) in cursors.iter_mut().zip(heads.iter_mut()) {
                if head.as_ref().is_some_and(|hit| hit.owner == owner) {
                    *head = observation.posting(cursor, &mut occurrences, &mut context)?;
                }
            }
            if path_owner == Some(owner) {
                path_visited = path_visited
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                path_head = next_path_hit(
                    path_cursor.as_mut().ok_or(IndexError::ResourceLimited)?,
                    self,
                    &mut context,
                )?;
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
        if let Some(groups) = path_groups {
            if path_visited != path_df {
                return Err(IndexError::CorruptObject(
                    "native path posting cardinality changed during traversal".into(),
                ));
            }
            let groups = groups.finish(capacity, path_rank_after, context.budget)?;
            let hit_bytes = capacity_bytes::<NativeTextRankedHit>(groups.rows.len())?;
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
            let mut hits = Vec::new();
            hits.try_reserve_exact(groups.rows.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            hits.extend(groups.rows.iter().map(|row| row.payload.0));
            if let Some(visitor) = preview {
                let preview_bytes = capacity_bytes::<HeapHit<ClauseTruth>>(groups.rows.len())?;
                let _memory = if preview_bytes == 0 {
                    None
                } else {
                    context
                        .budget
                        .reserve_memory(preview_bytes, MemoryLifetime::Temporary)?
                };
                context.budget.charge(WorkUsage {
                    bytes: preview_bytes,
                    ..WorkUsage::default()
                })?;
                let mut preview_hits = Vec::new();
                preview_hits
                    .try_reserve_exact(groups.rows.len())
                    .map_err(|_error| IndexError::ResourceLimited)?;
                for row in &groups.rows {
                    let (hit, truth, path_only) = row.payload;
                    preview_hits.push(HeapHit(
                        hit,
                        truth.ok_or_else(preview_mode_error)?,
                        path_only,
                    ));
                }
                visitor(
                    &PreviewSelection {
                        hits: &preview_hits,
                        program,
                        term_remap: &term_remap,
                        cursors,
                        regex: &regex_program,
                        case: plan.options.case_mode(),
                    },
                    &mut context,
                )?;
            }
            context.budget.checkpoint()?;
            return Ok(NativeTextRanking {
                view,
                artifact: member.artifact,
                matched: groups.matched,
                remaining: groups.remaining,
                hits,
                _hit_memory: hit_memory,
            });
        }
        // A positional cursor must name an actual eligible row, including its
        // exact score. Finish validation first so an unknown cursor cannot hide
        // corruption or turn provisional heap contents into a successful suffix.
        if !anchor_found {
            return Err(IndexError::InvalidInput(
                "native BM25 rank-after anchor is absent from eligible matches".into(),
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
            remaining,
            hits: Vec::new(),
            _hit_memory: hit_memory,
        };
        ranked
            .hits
            .try_reserve_exact(heap.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        match (heap, preview) {
            (TextHeap::Ranked(heap), None) => {
                ranked.hits.extend(heap.drain().map(|hit| hit.0));
                ranked.hits.sort_unstable_by(|a, b| compare_rank(a.score, a.owner, b.score, b.owner));
            }
            (TextHeap::Preview(heap), Some(visitor)) => {
                let sorted = std::mem::take(heap).into_sorted_vec();
                visitor(
                    &PreviewSelection {
                        hits: &sorted,
                        program,
                        term_remap: &term_remap,
                        cursors,
                        regex: &regex_program,
                        case: plan.options.case_mode(),
                    },
                    &mut context,
                )?;
                ranked.hits.extend(sorted.into_iter().map(|row| row.0));
            }
            _ => return Err(preview_mode_error()),
        }
        context.budget.checkpoint()?;
        Ok(ranked)
    }
}

mod preview_truth;
mod preview_projection;

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
        evaluate_boolean(program, present, 0, 0, |_ordinal, _first, _count, _cap| {
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
        let regex_only = evaluate_boolean(&program, 0, 1, 0, no_phrase)?;
        check!(regex_only.matched);
        check_eq!(regex_only.positive_terms, 0);
        let both = evaluate_boolean(&program, 1, 1, 0, no_phrase)?;
        check!(both.matched);
        check_eq!(both.positive_terms, 1);
        let neither = evaluate_boolean(&program, 0, 0, 0, no_phrase)?;
        check!(!neither.matched);
        check_eq!(neither.positive_terms, 0);
        Ok(())
    }

    #[test]
    fn direct_file_contains_plan_cannot_reinterpret_slash_regex_as_literal()
    -> Result<(), IndexError> {
        let mut program = Vec::new();
        let slash_regex = PlanNode::Leaf {
            leaf: PlanLeaf::FileContains {
                needle: "/foo.*/".into(),
            },
            cap: None,
        };
        check!(matches!(
            compile_clauses(
                &slash_regex,
                0,
                &mut Complexity::default(),
                &mut program,
                &mut |_term| Ok(0),
                &mut |_raw, _phrase| Ok(()),
                &mut |_source| Ok(0),
                &mut |_needle| Ok(0),
            ),
            Err(IndexError::InvalidInput(_))
        ));
        check!(program.is_empty());
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
                &mut |_needle| Ok(0),
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
                &mut |_needle| Ok(0),
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
