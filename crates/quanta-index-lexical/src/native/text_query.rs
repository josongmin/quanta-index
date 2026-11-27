//! Page-bounded Content Boolean and positional phrase execution of the planner IR.

use super::text::{
    BoundTextMember, NativeTextIndex, PostingStageSample, SelectedTextRoot, TextPostingCursor,
    TextReadContext, TextStatistics, ValidatedPostingHit, admitted_query_tokens,
};
use super::text_code_search_rank::{
    CodeSearchMatch, CodeSearchRank, RankMatcher, RankedFile, TermScope,
};
pub(super) use super::text_contains::file_contains_mask;
use super::text_literal_anchor::LiteralAnchor;
use super::text_memory::{
    MAX_TERM_KEY_BYTES, PreparedKey, PreparedRows, append_canonical_case_key,
    prepare_indexed_token_cursor,
};
use super::text_overlay::MAX_TEXT_OVERLAY_DOCUMENT_BYTES;
use super::text_overlay::TextOverlayCorpus;
pub use super::text_overlay::TextOverlaySource;
use super::text_path::PathTermCursor;
use super::text_path_census::{PathCensus, PathLeaf, path_census, path_leaf_mask};
use super::text_path_group::PathGroupAccumulator;
use super::text_preflight::{
    MAX_FILE_CONTAINS_LEAVES, MAX_PATH_CONTAINS_LEAVES, MAX_PATH_REGEX_LEAVES,
};
use super::text_rank_order::{RankOrderCapture, TextRankEntry, TextRankMemoryPort, TextRankOrder};
use super::text_regex::{NativeRegexProgram, PathLeafRegex, RegexMemory, compile_one};
use super::text_repo_predicate::RepoPredicateProgram;
use super::text_universe::{CandidateUniverse, TextOwnerCursor};
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
use quanta_index_contract::{
    LqCountBound, LqFilter, LqSelect, MAX_NATIVE_CONTENT_REGEX_LEAVES, RepoId,
};
use quanta_index_core::index::{
    FiniteScore, IndexError, MAX_SOURCE_PATH_BYTES, MemberBinding, MemoryLease, MemoryLifetime,
    ObjectRef, OwnerKey, SourceRecordOwner, ViewRef, WorkBudget, WorkUsage, owner_map_key,
};
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticSpanGuardV1,
    DiagnosticStageV1, DiagnosticTerminalV1,
};
use quanta_index_lq_text_normalizer::TextQueryError;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::Instant;
use std::{cmp::Ordering, collections::BinaryHeap};

static QUERY_PHASE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
const QUERY_PHASE_SAMPLE_LIMIT: usize = 8;

struct QueryPhaseProbe {
    phase: usize,
    entered: Instant,
    elapsed_ns: [u64; 4],
    candidates: u64,
    source_checks: u64,
}

impl QueryPhaseProbe {
    fn new() -> Option<Self> {
        if std::env::var_os("QUANTA_INDEX_NATIVE_WORK_DIAGNOSTICS_V1").as_deref()
            != Some(std::ffi::OsStr::new("1"))
        {
            return None;
        }
        let _prior_sample_count = QUERY_PHASE_SAMPLES
            .fetch_update(AtomicOrdering::Relaxed, AtomicOrdering::Relaxed, |count| {
                (count < QUERY_PHASE_SAMPLE_LIMIT).then_some(count + 1)
            })
            .ok()?;
        Some(Self {
            phase: 0,
            entered: Instant::now(),
            elapsed_ns: [0; 4],
            candidates: 0,
            source_checks: 0,
        })
    }

    fn mark(&mut self, next: usize) {
        self.elapsed_ns[self.phase] =
            u64::try_from(self.entered.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.phase = next;
        self.entered = Instant::now();
    }
}

enum MemberScope<'a> {
    Direct(&'a MemberBinding),
    Bound(&'a BoundTextMember),
}

struct ContentQuery<'a> {
    plan: &'a LexicalPlan,
    constraints: &'a quanta_index_contract::QueryConstraintSetV1,
    path_rank_after: PathRankAfter<'a>,
    /// Capture the full ranked order of a first page.
    capture_order: bool,
    capture_memory: Option<Arc<dyn TextRankMemoryPort>>,
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
    let record = super::text::authenticated_text_source(index, owner, Some(source), true, context)?;
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
    posting_stages: PostingStageSample,
    posting_stage_layout: Option<&'static str>,
    phrase_checks: u64,
    phrase_elapsed_ns: u64,
    overflowed: bool,
    phase_probe: Option<QueryPhaseProbe>,
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
            posting_stages: PostingStageSample::default(),
            posting_stage_layout: None,
            phrase_checks: 0,
            phrase_elapsed_ns: 0,
            overflowed: false,
            phase_probe: QueryPhaseProbe::new(),
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
        positions_required: bool,
    ) -> Result<Option<ValidatedPostingHit>, IndexError> {
        if self.span.is_none() && self.phase_probe.is_none() {
            return cursor.next_hit(context, occurrences, positions_required);
        }
        self.posting_stage_layout = Some(cursor.stage_probe_layout());
        Self::add(&mut self.posting_calls, Some(1), &mut self.overflowed);
        let started = Instant::now();
        let result = cursor.next_hit(context, occurrences, positions_required);
        if let Some(sample) = cursor.take_stage_probe() {
            for (total, amount) in self.posting_stages.calls.iter_mut().zip(sample.calls) {
                Self::add(total, Some(amount), &mut self.overflowed);
            }
            for (total, amount) in self
                .posting_stages
                .elapsed_ns
                .iter_mut()
                .zip(sample.elapsed_ns)
            {
                Self::add(total, Some(amount), &mut self.overflowed);
            }
            for (total, amount) in self
                .posting_stages
                .object_read_elapsed_ns
                .iter_mut()
                .zip(sample.object_read_elapsed_ns)
            {
                Self::add(total, Some(amount), &mut self.overflowed);
            }
            Self::add(
                &mut self.posting_stages.object_contribution_attempts,
                Some(sample.object_contribution_attempts),
                &mut self.overflowed,
            );
            Self::add(
                &mut self.posting_stages.scalar_retained_replay_attempts,
                Some(sample.scalar_retained_replay_attempts),
                &mut self.overflowed,
            );
        }
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
        if let Some(mut probe) = self.phase_probe.take() {
            let phase = probe.phase;
            probe.mark(phase);
            let layered = self.posting_stage_layout == Some("layered_v2");
            let stage_names = if layered {
                [
                    "partition",
                    "source_owner_map",
                    "source_record_decode",
                    "source_record_validation",
                    "occurrence",
                ]
            } else {
                [
                    "page",
                    "contribution",
                    "source_record",
                    "inventory",
                    "occurrence",
                ]
            };
            let tree_object_reads = if layered {
                [0, 0]
            } else {
                self.posting_stages.object_read_elapsed_ns
            };
            let event = serde_json::json!({
                "event": "native_lexical_query_phase_probe",
                "terminal": format!("{outcome:?}"),
                "active_phase": phase,
                "phase_names": ["pre_anchor", "literal_anchor", "posting_setup", "candidate_scan"],
                "elapsed_ns": probe.elapsed_ns,
                "posting_calls": self.posting_calls,
                "posting_hits": self.posting_hits,
                "posting_elapsed_ns": self.posting_elapsed_ns,
                "posting_stage_layout": self.posting_stage_layout,
                "posting_stage_names": stage_names,
                "posting_stage_calls": self.posting_stages.calls,
                "posting_stage_elapsed_ns": self.posting_stages.elapsed_ns,
                "tree_posting_object_read_names": ["contribution", "source_record"],
                "tree_posting_object_read_elapsed_ns": tree_object_reads,
                "layered_source_record_object_read_elapsed_ns": if layered { self.posting_stages.object_read_elapsed_ns[1] } else { 0 },
                "tree_object_contribution_attempts": self.posting_stages.object_contribution_attempts,
                "tree_scalar_retained_replay_attempts": self.posting_stages.scalar_retained_replay_attempts,
                "candidates": probe.candidates,
                "source_checks": probe.source_checks,
            });
            if let Ok(mut bytes) = serde_json::to_vec(&event) {
                bytes.push(b'\n');
                let _written = std::io::stderr().lock().write_all(&bytes);
            }
        }
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
/// Path leaves share one census and one `u8` outcome mask.
const MAX_PATH_LEAVES: usize = 8;
const _: () = assert!(MAX_PATH_CONTAINS_LEAVES + MAX_PATH_REGEX_LEAVES <= MAX_PATH_LEAVES);

/// One path leaf as compiled: a `path.contains` needle or a `path.regex`
/// source. Both are decided from Source path metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathLeafSource<'a> {
    Contains(&'a str),
    Regex(&'a str),
}

#[derive(Clone, Copy)]
pub(super) enum Clause {
    Term(usize),
    Regex(usize),
    Contains(usize),
    /// `path.contains`: decided and bounded by the Source path census.
    PathContains(usize),
    /// The root of a path-only query: true for every candidate, whose
    /// supplier and bound is the census of paths passing every `file:` filter.
    PathFilters,
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
    paths: usize,
    path_regexes: usize,
    repositories: usize,
}

#[expect(
    clippy::too_many_arguments,
    reason = "The private compiler passes independent term, phrase, regex, contains, path and repository resolvers"
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
    path: &mut impl FnMut(PathLeafSource<'a>) -> Result<usize, IndexError>,
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
            if complexity.regexes > MAX_NATIVE_CONTENT_REGEX_LEAVES {
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
            leaf: PlanLeaf::PathContains { needle },
            cap: None,
        } => {
            if !file_contains_raw_literal_supported(needle) {
                return Err(IndexError::InvalidInput(
                    "native path.contains requires a bounded literal raw substring".into(),
                ));
            }
            complexity.paths = complexity
                .paths
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if complexity.paths > MAX_PATH_CONTAINS_LEAVES {
                return Err(IndexError::ResourceLimited);
            }
            program.push(Clause::PathContains(path(PathLeafSource::Contains(
                needle,
            ))?));
            // The exact path census bounds its candidates like a term.
            Ok(true)
        }
        PlanNode::Leaf {
            leaf: PlanLeaf::PathRegex { source },
            cap: None,
        } => {
            complexity.path_regexes = complexity
                .path_regexes
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if complexity.path_regexes > MAX_PATH_REGEX_LEAVES {
                return Err(IndexError::ResourceLimited);
            }
            // A path regex shares the path census and its exact bound.
            program.push(Clause::PathContains(path(PathLeafSource::Regex(source))?));
            Ok(true)
        }
        PlanNode::Empty if depth == 0 => {
            program.push(Clause::PathFilters);
            Ok(true)
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
                    path,
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
                path,
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
        &mut |_leaf| {
            Err(IndexError::InvalidInput(
                "path predicate requires a native path census".into(),
            ))
        },
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
    path_present: u8,
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
            Clause::PathContains(index) => {
                let bit = 1u8
                    .checked_shl(
                        u32::try_from(index).map_err(|_width| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
                MatchState {
                    matched: path_present & bit != 0,
                    positive_terms: 0,
                }
            }
            // Every path filter already gates the candidate.
            Clause::PathFilters => MatchState {
                matched: true,
                positive_terms: 0,
            },
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
        0,
        |_ordinal, _matched| Ok(()),
        phrase,
    )
}

/// Structural value of a compiled clause.
///
/// `Finite` is constrained by positive posting anchors; `Unbounded` is not
/// constrained by any posting. Regex, repository and complement clauses, and
/// a `file.contains` without a Literal anchor, decide truth only after source
/// verification; they never supply candidates. A `file.contains` anchored on
/// the pinned Literal sibling (`text_literal_anchor`) is finite like a term.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Structural<T> {
    Unbounded,
    Finite(T),
}

/// Folds the postfix clause program with the candidate-set algebra.
///
/// A conjunction is constrained by its finite children (the others only
/// filter), and a disjunction is finite only when every child is finite.
fn fold_structure<T: Copy>(
    program: &[Clause],
    mut leaf: impl FnMut(Clause) -> Result<Structural<T>, IndexError>,
    mut all: impl FnMut(T, T) -> Result<T, IndexError>,
    mut any: impl FnMut(T, T) -> Result<T, IndexError>,
) -> Result<Structural<T>, IndexError> {
    let mut stack = [Structural::Unbounded; MAX_QUERY_NODES];
    let mut end = 0usize;
    for clause in program {
        let value = match *clause {
            Clause::Term(_)
            | Clause::Phrase { .. }
            | Clause::Contains(_)
            | Clause::PathContains(_)
            | Clause::PathFilters => leaf(*clause)?,
            Clause::Regex(_) | Clause::Repo(_) => Structural::Unbounded,
            Clause::Not => {
                end = end.checked_sub(1).ok_or_else(|| {
                    IndexError::BackendViolation("structural NOT stack underflow".into())
                })?;
                Structural::Unbounded
            }
            Clause::All(count) | Clause::Any(count) => {
                let start = end.checked_sub(count).ok_or_else(|| {
                    IndexError::BackendViolation("structural stack underflow".into())
                })?;
                let children = stack.get(start..end).ok_or_else(|| {
                    IndexError::BackendViolation("structural stack range exceeds bound".into())
                })?;
                let conjunction = matches!(clause, Clause::All(_));
                let mut folded = None;
                let mut unbounded = false;
                for child in children {
                    match *child {
                        Structural::Unbounded => unbounded = true,
                        Structural::Finite(value) => {
                            folded = Some(match folded {
                                None => value,
                                Some(prior) if conjunction => all(prior, value)?,
                                Some(prior) => any(prior, value)?,
                            });
                        }
                    }
                }
                end = start;
                match folded {
                    Some(value) if conjunction || !unbounded => Structural::Finite(value),
                    Some(_) | None => Structural::Unbounded,
                }
            }
        };
        *stack
            .get_mut(end)
            .ok_or_else(|| IndexError::BackendViolation("structural stack overflow".into()))? =
            value;
        end = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    if end != 1 {
        return Err(IndexError::BackendViolation(
            "structural evaluation has no unique result".into(),
        ));
    }
    stack
        .first()
        .copied()
        .ok_or_else(|| IndexError::BackendViolation("structural result stack is empty".into()))
}

fn phrase_terms(term_remap: &[usize], first: usize, count: usize) -> Result<&[usize], IndexError> {
    let end = first
        .checked_add(count)
        .ok_or(IndexError::ResourceLimited)?;
    let terms = term_remap
        .get(first..end)
        .ok_or_else(|| IndexError::BackendViolation("phrase term range exceeds query".into()))?;
    if terms.is_empty() {
        return Err(IndexError::BackendViolation("phrase has no terms".into()));
    }
    Ok(terms)
}

/// Upper bound on the owners that can satisfy a remapped program.
///
/// It uses the declared DF of each posting cursor: a term is bounded by its
/// DF, a phrase by its rarest term, a conjunction by its smallest finite
/// child and a disjunction by the sum of its children. `None` means no
/// posting anchor bounds the program. Posting cursors refuse more than their
/// declared DF, so a corrupt DF cannot enlarge the verified set past this
/// bound. An anchored `file.contains` is bounded by its exact Literal
/// candidate count (`contains`); an unanchored one returns `None` there.
pub(super) fn candidate_bound(
    program: &[Clause],
    term_remap: &[usize],
    df: impl Fn(usize) -> Result<u64, IndexError>,
    contains: impl Fn(usize) -> Result<Option<u64>, IndexError>,
    paths: impl Fn(Option<usize>) -> Result<Option<u64>, IndexError>,
) -> Result<Option<u64>, IndexError> {
    let bound = fold_structure(
        program,
        |clause| match clause {
            Clause::Term(index) => df(index).map(Structural::Finite),
            Clause::Phrase { first, count, .. } => phrase_terms(term_remap, first, count)?
                .iter()
                .try_fold(u64::MAX, |rarest, index| Ok(rarest.min(df(*index)?)))
                .map(Structural::Finite),
            Clause::Contains(index) => {
                Ok(contains(index)?.map_or(Structural::Unbounded, Structural::Finite))
            }
            Clause::PathContains(index) => {
                Ok(paths(Some(index))?.map_or(Structural::Unbounded, Structural::Finite))
            }
            Clause::PathFilters => {
                Ok(paths(None)?.map_or(Structural::Unbounded, Structural::Finite))
            }
            Clause::Regex(_) | Clause::Repo(_) | Clause::All(_) | Clause::Any(_) | Clause::Not => {
                Err(IndexError::BackendViolation(
                    "structural leaf is not a posting anchor".into(),
                ))
            }
        },
        |left, right| Ok(left.min(right)),
        |left, right| left.checked_add(right).ok_or(IndexError::ResourceLimited),
    )?;
    Ok(match bound {
        Structural::Finite(bound) => Some(bound),
        Structural::Unbounded => None,
    })
}

/// Whether an owner lies in the structural candidate set.
///
/// `present` is the owner's posting presence mask; `candidate_bound` bounds
/// the cardinality of this set. An owner outside it cannot satisfy the
/// program for any source-predicate outcome. `None` means the program is
/// unanchored and every admitted owner is a candidate. `contains` is the
/// owner's membership in an anchored `file.contains` candidate set, `None`
/// for an unanchored one.
fn structural_candidate(
    program: &[Clause],
    term_remap: &[usize],
    present: u64,
    contains: impl Fn(usize) -> Option<bool>,
    paths: impl Fn(Option<usize>) -> Option<bool>,
) -> Result<Option<bool>, IndexError> {
    let has = |index: usize| -> Result<bool, IndexError> {
        let bit = 1u64
            .checked_shl(u32::try_from(index).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(present & bit != 0)
    };
    let member = fold_structure(
        program,
        |clause| match clause {
            Clause::Term(index) => has(index).map(Structural::Finite),
            Clause::Phrase { first, count, .. } => phrase_terms(term_remap, first, count)?
                .iter()
                .try_fold(true, |all, index| Ok(all && has(*index)?))
                .map(Structural::Finite),
            Clause::Contains(index) => {
                Ok(contains(index).map_or(Structural::Unbounded, Structural::Finite))
            }
            Clause::PathContains(index) => {
                Ok(paths(Some(index)).map_or(Structural::Unbounded, Structural::Finite))
            }
            Clause::PathFilters => {
                Ok(paths(None).map_or(Structural::Unbounded, Structural::Finite))
            }
            Clause::Regex(_) | Clause::Repo(_) | Clause::All(_) | Clause::Any(_) | Clause::Not => {
                Err(IndexError::BackendViolation(
                    "structural leaf is not a posting anchor".into(),
                ))
            }
        },
        |left, right| Ok(left && right),
        |left, right| Ok(left || right),
    )?;
    Ok(match member {
        Structural::Finite(member) => Some(member),
        Structural::Unbounded => None,
    })
}

/// The path leaves under a negation, one bit per leaf ordinal: they can only
/// filter, never supply candidates.
fn negated_path_leaves(program: &[Clause]) -> Result<u8, IndexError> {
    // Each stack entry is the set of path leaves under one subtree.
    let mut stack = [0u8; MAX_QUERY_NODES];
    let mut end = 0usize;
    let mut negated = 0u8;
    for clause in program {
        let value = match *clause {
            Clause::PathContains(index) => 1u8
                .checked_shl(u32::try_from(index).map_err(|_width| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?,
            Clause::Not => {
                end = end.checked_sub(1).ok_or_else(|| {
                    IndexError::BackendViolation("negated path stack underflow".into())
                })?;
                let child = *stack.get(end).ok_or(IndexError::ResourceLimited)?;
                negated |= child;
                child
            }
            Clause::All(count) | Clause::Any(count) => {
                let start = end.checked_sub(count).ok_or_else(|| {
                    IndexError::BackendViolation("negated path stack underflow".into())
                })?;
                let children = stack.get(start..end).ok_or(IndexError::ResourceLimited)?;
                let union = children.iter().fold(0u8, |union, child| union | child);
                end = start;
                union
            }
            Clause::Term(_)
            | Clause::Phrase { .. }
            | Clause::Regex(_)
            | Clause::Contains(_)
            | Clause::PathFilters
            | Clause::Repo(_) => 0,
        };
        *stack.get_mut(end).ok_or(IndexError::ResourceLimited)? = value;
        end = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    Ok(negated)
}

/// The bound the Literal anchors alone put on the program: terms, phrases
/// and path leaves count as unbounded here. `None` when no anchor bounds it.
fn anchor_bound(
    program: &[Clause],
    contains: impl Fn(usize) -> Result<Option<u64>, IndexError>,
) -> Result<Option<u64>, IndexError> {
    let bound = fold_structure(
        program,
        |clause| match clause {
            Clause::Contains(index) => {
                Ok(contains(index)?.map_or(Structural::Unbounded, Structural::Finite))
            }
            Clause::Term(_)
            | Clause::Phrase { .. }
            | Clause::PathContains(_)
            | Clause::PathFilters => Ok(Structural::Unbounded),
            Clause::Regex(_) | Clause::Repo(_) | Clause::All(_) | Clause::Any(_) | Clause::Not => {
                Err(IndexError::BackendViolation(
                    "structural leaf is not a posting anchor".into(),
                ))
            }
        },
        |left, right| Ok(left.min(right)),
        |left, right| Ok(left.saturating_add(right)),
    )?;
    Ok(match bound {
        Structural::Finite(bound) => Some(bound),
        Structural::Unbounded => None,
    })
}

/// Whether some positive posting anchor bounds the program.
///
/// This is decided before any cursor is opened. `contains` says which
/// `file.contains` leaves are anchored on the Literal sibling; `paths` whether
/// `path.contains` and `path.regex` leaves may anchor (their census supplies
/// candidates).
fn structurally_anchored(
    program: &[Clause],
    contains: impl Fn(usize) -> bool,
    paths: bool,
) -> Result<bool, IndexError> {
    let anchored = fold_structure(
        program,
        |clause| match clause {
            Clause::Term(_) | Clause::Phrase { .. } | Clause::PathFilters => {
                Ok(Structural::Finite(()))
            }
            Clause::PathContains(_) if paths => Ok(Structural::Finite(())),
            Clause::Contains(index) if contains(index) => Ok(Structural::Finite(())),
            Clause::PathContains(_) | Clause::Contains(_) => Ok(Structural::Unbounded),
            Clause::Regex(_) | Clause::Repo(_) | Clause::All(_) | Clause::Any(_) | Clause::Not => {
                Err(IndexError::BackendViolation(
                    "structural leaf is not a posting anchor".into(),
                ))
            }
        },
        |(), ()| Ok(()),
        |(), ()| Ok(()),
    )?;
    Ok(anchored != Structural::Unbounded)
}

/// One finite body anchor supplies every possible match of a conjunction of
/// body substrings. Final Source verification still evaluates every leaf.
fn is_body_conjunction(program: &[Clause]) -> bool {
    program
        .iter()
        .all(|clause| matches!(clause, Clause::Contains(_) | Clause::All(_)))
}

/// One term of a CodeSearch-shaped program.
///
/// `body` is the term's `file.contains` or Content regex ordinal and `path`
/// its path leaf ordinal; the Legacy scope follows from which it has.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ShapeTerm<'a> {
    text: &'a str,
    regex: bool,
    body: Option<usize>,
    path: Option<usize>,
}

impl ShapeTerm<'_> {
    const fn scope(&self) -> TermScope {
        match (self.body.is_some(), self.path.is_some()) {
            (true, true) => TermScope::Both,
            (true, false) => TermScope::Content,
            (false, _) => TermScope::Path,
        }
    }
}

/// The literal and regex terms of a CodeSearch-shaped program, in query
/// order.
///
/// The shape is one term, or a conjunction of terms, where a term is a body
/// leaf (`file.contains(x)` or a Content regex `/x/`), a path leaf
/// (`path.contains(x)` or `path.regex('x')`), or a body leaf OR a path leaf
/// of the same kind and text (both surfaces). Any other clause, including a
/// Content term, phrase, repository predicate or negation, is not
/// CodeSearch-shaped and keeps the BM25 ranking.
fn code_search_terms<'a>(
    program: &[Clause],
    contains: &[Option<&'a str>],
    regexes: &[Option<&'a str>],
    paths: &[PathLeafSource<'a>],
) -> Result<Option<Vec<ShapeTerm<'a>>>, IndexError> {
    let (units, conjunction) = match program.split_last() {
        Some((Clause::All(count), units)) => (units, Some(*count)),
        Some(_) | None => (program, None),
    };
    let mut terms: Vec<ShapeTerm<'a>> = Vec::new();
    terms
        .try_reserve_exact(units.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for clause in units {
        let term = match *clause {
            Clause::Contains(index) => ShapeTerm {
                text: contains
                    .get(index)
                    .copied()
                    .flatten()
                    .ok_or(IndexError::ResourceLimited)?,
                regex: false,
                body: Some(index),
                path: None,
            },
            Clause::Regex(index) => ShapeTerm {
                text: regexes
                    .get(index)
                    .copied()
                    .flatten()
                    .ok_or(IndexError::ResourceLimited)?,
                regex: true,
                body: Some(index),
                path: None,
            },
            Clause::PathContains(index) => {
                let (text, regex) = match paths.get(index).ok_or(IndexError::ResourceLimited)? {
                    PathLeafSource::Contains(text) => (*text, false),
                    PathLeafSource::Regex(text) => (*text, true),
                };
                ShapeTerm {
                    text,
                    regex,
                    body: None,
                    path: Some(index),
                }
            }
            Clause::Any(2) => {
                let (Some(second), Some(first)) = (terms.pop(), terms.pop()) else {
                    return Ok(None);
                };
                let surfaces = matches!(
                    (first.scope(), second.scope()),
                    (TermScope::Content, TermScope::Path) | (TermScope::Path, TermScope::Content)
                );
                if !surfaces || first.regex != second.regex || first.text != second.text {
                    return Ok(None);
                }
                ShapeTerm {
                    text: first.text,
                    regex: first.regex,
                    body: first.body.or(second.body),
                    path: first.path.or(second.path),
                }
            }
            Clause::Term(_)
            | Clause::PathFilters
            | Clause::Repo(_)
            | Clause::All(_)
            | Clause::Any(_)
            | Clause::Not
            | Clause::Phrase { .. } => return Ok(None),
        };
        terms.push(term);
    }
    Ok((terms.len() == conjunction.unwrap_or(1)).then_some(terms))
}

/// One overlay row's exact path-leaf and anchored-contains outcomes.
struct OverlayRowMask {
    owner: OwnerKey,
    paths: u8,
    filtered: bool,
    contains: u8,
}

impl OverlayRowMask {
    fn path_leaf(&self, leaf: Option<usize>) -> bool {
        leaf.map_or(self.filtered, |index| mask_bit(self.paths, index))
    }

    fn contains_bit(&self, index: usize) -> bool {
        mask_bit(self.contains, index)
    }
}

fn mask_bit(mask: u8, index: usize) -> bool {
    u32::try_from(index)
        .is_ok_and(|shift| mask.checked_shr(shift).is_some_and(|rest| rest & 1 != 0))
}

/// Decide every overlay row's path and anchored-contains leaves.
///
/// Path leaves, path-only filters and anchored `file.contains` needles use the
/// same mask functions the durable rows use. Rows keep the sources' owner
/// order. An empty anchored needle is an unanchored leaf and is never
/// evaluated here.
fn overlay_row_masks(
    sources: &[TextOverlaySource<'_>],
    path_leaves: &[PathLeaf<'_>],
    path_filters: Option<&NativeRegexProgram>,
    anchored_needles: &[&str],
    case: super::text::CaseMode,
    budget: &mut WorkBudget,
) -> Result<Option<PreparedRows<OverlayRowMask>>, IndexError> {
    if sources.is_empty()
        || (path_leaves.is_empty()
            && path_filters.is_none()
            && anchored_needles.iter().all(|needle| needle.is_empty()))
    {
        return Ok(None);
    }
    let mut rows = PreparedRows::with_capacity(sources.len(), budget)?;
    for source in sources {
        budget.checkpoint()?;
        let paths = if path_leaves.is_empty() {
            0
        } else {
            path_leaf_mask(source.path, path_leaves, case, budget)?
        };
        let filtered = match path_filters {
            Some(program) => program.matches_path_filters(source.path, budget)?,
            None => false,
        };
        let mut contains = 0u8;
        for (ordinal, needle) in anchored_needles.iter().enumerate() {
            if !needle.is_empty() && file_contains_mask(source.body, &[needle], case, budget)? != 0
            {
                contains |= 1u8
                    .checked_shl(
                        u32::try_from(ordinal).map_err(|_width| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        rows.push_preallocated(OverlayRowMask {
            owner: source.owner,
            paths,
            filtered,
            contains,
        })?;
    }
    Ok(Some(rows))
}

fn file_contains_durable(
    index: &NativeTextIndex,
    owner: OwnerKey,
    source: ObjectRef,
    needles: &[&str],
    case: super::text::CaseMode,
    allow_excluded_path: bool,
    rank: Option<&CodeSearchRank<'_>>,
    context: &mut TextReadContext<'_>,
) -> Result<(u8, Option<RankedFile>), IndexError> {
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
        return Ok((0, None));
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
    let mask = file_contains_mask(text, needles, case, context.budget)?;
    let ranked = rank
        .map(|rank| rank.score(Some(text), &record.path, context.budget))
        .transpose()?
        .flatten();
    Ok((mask, ranked))
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
    /// The representative verified match of a CodeSearch-shaped query.
    pub code_search: Option<CodeSearchMatch>,
}

pub use quanta_index_core::index::NativeTextHitSource;

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
    /// Every eligible row in page order, when a first page captured it.
    pub order: Option<TextRankOrder>,
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

    /// [`Self::search_content`], also capturing the full ranked order.
    #[cfg(test)]
    pub(super) fn search_content_capturing(
        &self,
        member: &MemberBinding,
        plan: &LexicalPlan,
        constraints: &quanta_index_contract::QueryConstraintSetV1,
        top_k: u32,
        policy: Bm25Policy,
        context: TextReadContext<'_>,
    ) -> Result<NativeTextRanking, IndexError> {
        self.search_content_with_scope(
            &MemberScope::Direct(member),
            ContentQuery {
                plan,
                constraints,
                path_rank_after: None,
                capture_order: true,
                capture_memory: None,
            },
            TextContentExecution {
                top_k,
                policy,
                rank_after: None,
            },
            None,
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
                capture_order: false,
                capture_memory: None,
            },
            execution,
            overlay,
            None,
            context,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the reader passes one query's plan, cursors, overlay, preview and capture"
    )]
    pub(super) fn search_content_with_bound_member(
        &self,
        bound: &BoundTextMember,
        plan: &LexicalPlan,
        constraints: &quanta_index_contract::QueryConstraintSetV1,
        execution: TextContentExecution,
        path_rank_after: PathRankAfter<'_>,
        capture_order: bool,
        capture_memory: Option<Arc<dyn TextRankMemoryPort>>,
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
                capture_order,
                capture_memory,
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
            capture_order,
            capture_memory,
        } = query;
        let TextContentExecution {
            top_k,
            policy,
            rank_after,
        } = execution;
        let path_mode = plan.filters.surface == ResultSurface::Path;
        // Only a first page of a durable query captures its full order.
        let mut order_capture = (capture_order
            && rank_after.is_none()
            && path_rank_after.is_none()
            && overlay.is_none()
            && preview.is_none())
        .then(|| RankOrderCapture::new(capture_memory));
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
        let repo_filters = plan
            .raw_filters
            .iter()
            .filter(|raw| matches!(raw, LqFilter::Repo { .. }));
        let lang_filters = plan
            .raw_filters
            .iter()
            .filter(|raw| matches!(raw, LqFilter::Lang { .. }));
        if plan
            .filters
            .repo
            .iter()
            .any(|repository| !repository.revs.is_empty())
        {
            return Err(IndexError::InvalidInput(
                "native repo filter revisions require a history producer".into(),
            ));
        }
        if !matches!(
            plan.options.pattern_type,
            quanta_index_contract::LqPatternType::Standard
                | quanta_index_contract::LqPatternType::Keyword
        ) || plan.filters.case != empty_filters.case
            || plan.filters.count != empty_filters.count
            || (!path_mode && plan.filters.surface != empty_filters.surface)
            || path_selects != usize::from(path_mode)
            || plan.filters.typed_unavailable != empty_filters.typed_unavailable
            || plan.raw_filters.len()
                != plan
                    .filters
                    .paths
                    .len()
                    .saturating_add(plan.filters.repo.len())
                    .saturating_add(plan.filters.langs.len())
                    .saturating_add(path_selects)
            || file_filters.clone().count() != plan.filters.paths.len()
            || file_filters.zip(&plan.filters.paths).any(|(raw, planned)| {
                !matches!(raw, LqFilter::File { pattern, scope }
                    if pattern == &planned.pattern
                        && crate::filters::FilePathScope::from_lq(*scope) == planned.scope)
            })
            || repo_filters.clone().count() != plan.filters.repo.len()
            || repo_filters.zip(&plan.filters.repo).any(|(raw, planned)| {
                !matches!(raw, LqFilter::Repo { pattern, revs }
                    if pattern == &planned.pattern && revs == &planned.revs)
            })
            // The planner drops blank language ids; a dropped id must not
            // silently widen the language set.
            || lang_filters.clone().count() != plan.filters.langs.len()
            || lang_filters.zip(&plan.filters.langs).any(|(raw, planned)| {
                !matches!(raw, LqFilter::Lang { id }
                    if crate::metadata_normalize::normalize_language(id).as_ref() == Some(planned))
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
        let mut source_regexes = [None; MAX_NATIVE_CONTENT_REGEX_LEAVES];
        let mut source_regex_count = 0usize;
        let mut source_contains = [None; MAX_FILE_CONTAINS_LEAVES];
        let mut source_contains_count = 0usize;
        let mut source_paths = [PathLeafSource::Contains(""); MAX_PATH_LEAVES];
        let mut source_path_count = 0usize;
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
            &mut |leaf| {
                let ordinal = source_path_count;
                *source_paths
                    .get_mut(ordinal)
                    .ok_or(IndexError::ResourceLimited)? = leaf;
                source_path_count = source_path_count
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
        // Path leaves are decided from the pinned Source path map for durable
        // rows and on the authenticated row path for overlay rows.
        let path_only = program
            .iter()
            .any(|clause| matches!(clause, Clause::PathFilters));
        if path_only && plan.filters.paths.is_empty() {
            return Err(IndexError::InvalidInput(
                "native path-only query requires a path filter".into(),
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
        let source_predicates = source_regex_count != 0 || source_contains_count != 0;
        let selected_count = selection.selected_len();
        // A `file.contains` needle may be anchored on the pinned Literal
        // sibling over the identical Source cut. The sibling anchors durable
        // rows only; overlay rows are decided on their own text below.
        if let Some(probe) = observation.phase_probe.as_mut() {
            probe.mark(1);
        }
        let literal_sibling = match self.literal_sibling() {
            Some(format) if source_contains_count != 0 => {
                super::text_literal_anchor::pinned_literal_sibling(format, member, &mut context)?
            }
            Some(_) | None => None,
        };
        let mut contains_anchors: [Option<LiteralAnchor>; MAX_FILE_CONTAINS_LEAVES] =
            std::array::from_fn(|_slot| None);
        let body_conjunction = source_contains_count > 1 && is_body_conjunction(program);
        if let Some(grams) = literal_sibling {
            if body_conjunction {
                let needles = source_contains
                    .get(..source_contains_count)
                    .ok_or(IndexError::ResourceLimited)?;
                if let Some((index, anchor)) = super::text_literal_anchor::anchor_body_conjunction(
                    grams,
                    needles,
                    plan.options.case_mode(),
                    &mut context,
                )? {
                    *contains_anchors
                        .get_mut(index)
                        .ok_or(IndexError::ResourceLimited)? = Some(anchor);
                }
            } else {
                for (anchor, needle) in contains_anchors
                    .iter_mut()
                    .zip(source_contains.iter())
                    .take(source_contains_count)
                {
                    let needle = needle.ok_or_else(|| {
                        IndexError::BackendViolation("native file.contains source missing".into())
                    })?;
                    *anchor = super::text_literal_anchor::anchor_needle(
                        grams,
                        needle,
                        plan.options.case_mode(),
                        &mut context,
                    )?;
                }
            }
        }
        if let Some(probe) = observation.phase_probe.as_mut() {
            probe.mark(2);
        }
        let path_sources = source_paths
            .get(..source_path_count)
            .ok_or(IndexError::ResourceLimited)?;
        let mut path_regex_engines: [Option<PathLeafRegex>; MAX_PATH_LEAVES] =
            std::array::from_fn(|_slot| None);
        for (engine, leaf) in path_regex_engines.iter_mut().zip(path_sources) {
            if let PathLeafSource::Regex(source) = leaf {
                *engine = Some(PathLeafRegex::compile(
                    source,
                    plan.options.case_mode(),
                    context.budget,
                )?);
            }
        }
        let mut path_leaf_slots = [PathLeaf::Contains(""); MAX_PATH_LEAVES];
        for ((slot, leaf), engine) in path_leaf_slots
            .iter_mut()
            .zip(path_sources)
            .zip(path_regex_engines.iter())
        {
            *slot = match leaf {
                PathLeafSource::Contains(needle) => PathLeaf::Contains(needle),
                PathLeafSource::Regex(_) => PathLeaf::Regex(engine.as_ref().ok_or_else(|| {
                    IndexError::BackendViolation("native path.regex engine missing".into())
                })?),
            };
        }
        let path_leaves = path_leaf_slots
            .get(..source_path_count)
            .ok_or(IndexError::ResourceLimited)?;
        let path_filters = path_only
            .then(|| {
                NativeRegexProgram::compile(
                    &[],
                    &plan.filters.paths,
                    plan.options.case_mode(),
                    context.budget,
                )
            })
            .transpose()?;
        // A negated path leaf never supplies candidates, and when the
        // program is anchored without its path leaves no path leaf has to:
        // such a leaf filters the candidates another clause supplies, decided
        // on each candidate's authenticated path, without enumerating the
        // path map. Only when those anchors are not already admitted (no
        // Literal bound, or one above the 4,096-owner structural bound) can a
        // selective path leaf lower the bound; its census then collects at
        // most that bound and drives, or stops and filters.
        let max_unanchored = usize::try_from(super::text_universe::MAX_UNANCHORED_DOCS)
            .map_err(|_width| IndexError::ResourceLimited)?;
        let negated_paths = negated_path_leaves(program)?;
        let (census_skip, census_cap) = if source_path_count != 0
            && !path_only
            && structurally_anchored(
                program,
                |index| contains_anchors.get(index).is_some_and(Option::is_some),
                false,
            )? {
            let anchors = anchor_bound(program, |index| {
                contains_anchors
                    .get(index)
                    .and_then(Option::as_ref)
                    .map(LiteralAnchor::len)
                    .transpose()
            })?;
            if anchors.is_some_and(|bound| bound <= super::text_universe::MAX_UNANCHORED_DOCS) {
                (u8::MAX, None)
            } else {
                (negated_paths, Some(max_unanchored))
            }
        } else {
            (negated_paths, None)
        };
        let census: Option<PathCensus> = if source_path_count != 0 || path_only {
            Some(path_census(
                path_leaves,
                path_filters.as_ref(),
                plan.options.case_mode(),
                census_cap,
                census_skip,
                &mut context,
            )?)
        } else {
            None
        };
        // Path leaves whose census exceeded its cap filter per candidate.
        let mut filtered_leaves = 0u8;
        if let Some(census) = census.as_ref() {
            for (ordinal, set) in census.needles.iter().enumerate() {
                if set.is_none() {
                    filtered_leaves |= 1u8
                        .checked_shl(
                            u32::try_from(ordinal).map_err(|_width| IndexError::ResourceLimited)?,
                        )
                        .ok_or(IndexError::ResourceLimited)?;
                }
            }
        }
        let path_filtered =
            |leaf: Option<usize>| leaf.is_some_and(|index| mask_bit(filtered_leaves, index));
        // Overlay rows replace their durable owners: each row is decided
        // exactly on its authenticated path and in-memory body, and a durable
        // owner the overlay masks never counts towards a census or anchor.
        let overlay_conjunction_mask = if overlay.is_some()
            && body_conjunction
            && contains_anchors.iter().any(Option::is_some)
        {
            let width = u32::try_from(source_contains_count)
                .map_err(|_width| IndexError::ResourceLimited)?;
            let mask = 1_u16
                .checked_shl(width)
                .ok_or(IndexError::ResourceLimited)?
                .saturating_sub(1);
            Some(u8::try_from(mask).map_err(|_width| IndexError::ResourceLimited)?)
        } else {
            None
        };
        let mut anchored_needles = [""; MAX_FILE_CONTAINS_LEAVES];
        for ((destination, needle), anchor) in anchored_needles
            .iter_mut()
            .zip(source_contains.iter())
            .zip(contains_anchors.iter())
            .take(source_contains_count)
        {
            if anchor.is_some() || overlay_conjunction_mask.is_some() {
                *destination = needle.ok_or_else(|| {
                    IndexError::BackendViolation("native file.contains source missing".into())
                })?;
            }
        }
        let overlay_masks = overlay_row_masks(
            overlay.map_or(&[][..], |(_, sources)| sources),
            path_leaves,
            path_filters.as_ref(),
            anchored_needles
                .get(..source_contains_count)
                .ok_or(IndexError::ResourceLimited)?,
            plan.options.case_mode(),
            context.budget,
        )?;
        let overlay_rows = overlay_masks
            .as_ref()
            .map_or(&[][..], PreparedRows::as_slice);
        let overlay_row = |owner: OwnerKey| {
            overlay_rows
                .binary_search_by_key(&owner, |row| row.owner)
                .map_or(None, |position| overlay_rows.get(position))
        };
        let masked = |owner: &OwnerKey| overlay.is_some_and(|(corpus, _)| corpus.masks(*owner));
        let unmasked_count = |owners: &[OwnerKey]| {
            u64::try_from(owners.iter().filter(|owner| !masked(owner)).count())
                .map_err(|_width| IndexError::ResourceLimited)
        };
        let overlay_count = |matches: &dyn Fn(&OverlayRowMask) -> bool| {
            u64::try_from(overlay_rows.iter().filter(|row| matches(row)).count())
                .map_err(|_width| IndexError::ResourceLimited)
        };
        let overlay_contains = |row: &OverlayRowMask, index: usize| {
            overlay_conjunction_mask.map_or_else(
                || row.contains_bit(index),
                |mask| row.contains & mask == mask,
            )
        };
        let census_set = |leaf: Option<usize>| {
            census.as_ref().and_then(|census| {
                leaf.map_or(census.filtered.as_ref(), |index| {
                    census.needles.get(index).and_then(Option::as_ref)
                })
            })
        };
        let anchored = anchored
            || structurally_anchored(
                program,
                |index| contains_anchors.get(index).is_some_and(Option::is_some),
                true,
            )?;
        // An anchored bound already counts overlay rows exactly; only the
        // unanchored census must fit the whole virtual corpus.
        if !anchored
            && overlay.is_some_and(|(corpus, _)| {
                corpus.statistics().docs > super::text_universe::MAX_UNANCHORED_DOCS
            })
        {
            return Err(IndexError::ResourceLimited);
        }
        if source_predicates {
            // A selected path marker certifies its exhaustive owner bound.
            // Without one, an unanchored predicate needs the full finite
            // admitted census; an anchored predicate is bounded by the
            // structural posting-DF candidate bound once the term cursors
            // have been opened below.
            match selected_count {
                Some(count)
                    if u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?
                        <= super::text_universe::MAX_UNANCHORED_DOCS => {}
                Some(_) => return Err(IndexError::ResourceLimited),
                None if !anchored => TextOwnerCursor::check_statistics(root.statistics())?,
                None => {}
            }
        }
        let mut universe = if !anchored {
            Some(CandidateUniverse::new(
                root,
                self.recipe(),
                &selection,
                source_predicates,
            )?)
        } else if contains_anchors.iter().any(Option::is_some) || census.is_some() {
            // A pure body conjunction contributed at most one anchor. Mixed
            // expressions retain all independent candidate suppliers.
            Some(CandidateUniverse::anchored(
                root,
                self.recipe(),
                contains_anchors
                    .iter()
                    .flatten()
                    .map(LiteralAnchor::owners)
                    .chain(
                        census
                            .iter()
                            .flat_map(|census| {
                                census
                                    .needles
                                    .iter()
                                    .flatten()
                                    .chain(census.filtered.as_ref())
                            })
                            .map(super::text_path_census::PathOwners::owners),
                    ),
                context.budget,
            )?)
        } else {
            None
        };
        let repository_program = RepoPredicateProgram::prepare(
            repository_constraints,
            plan.options.case_mode(),
            self,
            &mut context,
        )?;
        let mut regex_sources = [""; MAX_NATIVE_CONTENT_REGEX_LEAVES];
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
        )?
        .with_record_metadata(&plan.filters.repo, &plan.filters.langs, context.budget)?;
        // CodeSearch-shaped file results take the Legacy CodeSearch score. A
        // path regex term is scored by a document engine over the NFC path; a
        // body regex term reuses its Content engine.
        let shape = if path_mode && !path_only && plan.filters.paths.is_empty() {
            code_search_terms(
                program,
                source_contains
                    .get(..source_contains_count)
                    .ok_or(IndexError::ResourceLimited)?,
                source_regexes
                    .get(..source_regex_count)
                    .ok_or(IndexError::ResourceLimited)?,
                path_sources,
            )?
        } else {
            None
        };
        let mut path_scorers: [Option<(quanta_index_lq_regex::RegexExecutor, RegexMemory)>;
            MAX_PATH_LEAVES] = std::array::from_fn(|_slot| None);
        for term in shape.iter().flatten() {
            if term.regex
                && let Some(leaf) = term.path
            {
                let (executor, _states, memory) = compile_one(
                    term.text,
                    Some(plan.options.case_mode()),
                    false,
                    context.budget,
                )?;
                *path_scorers
                    .get_mut(leaf)
                    .ok_or(IndexError::ResourceLimited)? = Some((executor, memory));
            }
        }
        let code_search = match shape.as_ref() {
            Some(terms) => {
                let mut matchers = Vec::new();
                matchers
                    .try_reserve_exact(terms.len())
                    .map_err(|_error| IndexError::ResourceLimited)?;
                for term in terms {
                    let matcher = if term.regex {
                        RankMatcher::Regex {
                            content: term
                                .body
                                .map(|index| {
                                    regex_program
                                        .content_executor(index)
                                        .map(|(executor, _states)| executor)
                                })
                                .transpose()?,
                            path: term
                                .path
                                .map(|leaf| {
                                    path_scorers
                                        .get(leaf)
                                        .and_then(Option::as_ref)
                                        .map(|(executor, _memory)| executor)
                                        .ok_or_else(|| {
                                            IndexError::BackendViolation(
                                                "code search path regex engine missing".into(),
                                            )
                                        })
                                })
                                .transpose()?,
                        }
                    } else {
                        RankMatcher::Literal(term.text)
                    };
                    matchers.push((matcher, term.scope()));
                }
                Some(CodeSearchRank::new(matchers, plan.options.case_mode()))
            }
            None => None,
        };
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
                | Clause::PathContains(_)
                | Clause::PathFilters
                | Clause::Repo(_)
                | Clause::All(_)
                | Clause::Any(_)
                | Clause::Not => {}
            }
        }
        let prefer_frequencies =
            !path_mode && overlay.is_none() && matches!(program.as_slice(), [Clause::Term(_)]);
        for key in prepared_keys
            .get_mut(..unique_count)
            .ok_or(IndexError::ResourceLimited)?
            .iter_mut()
        {
            let key = key.take().ok_or_else(|| {
                IndexError::BackendViolation("prepared query term missing".into())
            })?;
            let cursor =
                self.posting_cursor_from_key(root, key, prefer_frequencies, &mut context)?;
            cursors.push(cursor);
        }
        for clause in program.iter_mut() {
            if let Clause::Term(index) = clause {
                *index = *term_remap.get(*index).ok_or_else(|| {
                    IndexError::BackendViolation("boolean term remap out of range".into())
                })?;
            }
        }
        // A term shared with any phrase must retain positions for that phrase.
        // All other terms still validate each occurrence, but need only its TF.
        let mut positions_required = [false; MAX_CONTENT_LEAVES];
        for clause in program.iter() {
            if let Clause::Phrase { first, count, .. } = *clause {
                for index in phrase_terms(&term_remap, first, count)? {
                    context.budget.checkpoint()?;
                    *positions_required.get_mut(*index).ok_or_else(|| {
                        IndexError::BackendViolation("phrase term remap exceeds query width".into())
                    })? = true;
                }
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
        let bound = candidate_bound(
            program,
            &term_remap,
            // Exact virtual DF: the durable posting adjusted for the overlay.
            |index| {
                term_dfs
                    .get(index)
                    .copied()
                    .ok_or(IndexError::ResourceLimited)
            },
            |index| {
                contains_anchors
                    .get(index)
                    .and_then(Option::as_ref)
                    .map(|anchor| {
                        if overlay.is_none() {
                            return anchor.len();
                        }
                        unmasked_count(anchor.owners())?
                            .checked_add(overlay_count(&|row| overlay_contains(row, index))?)
                            .ok_or(IndexError::ResourceLimited)
                    })
                    .transpose()
            },
            |leaf| {
                if path_filtered(leaf) {
                    return Ok(None);
                }
                let set = census_set(leaf).ok_or_else(|| {
                    IndexError::BackendViolation("path leaf lost its census".into())
                })?;
                if overlay.is_none() {
                    return set.len().map(Some);
                }
                unmasked_count(set.owners())?
                    .checked_add(overlay_count(&|row| row.path_leaf(leaf))?)
                    .ok_or(IndexError::ResourceLimited)
                    .map(Some)
            },
        )?;
        if bound.is_some() != anchored {
            return Err(IndexError::BackendViolation(
                "structural candidate bound disagrees with compiled anchor".into(),
            ));
        }
        if source_predicates
            && selected_count.is_none()
            && let Some(bound) = bound
        {
            // Only structural candidates are verified against source bytes
            // below, and they cannot outnumber this bound. Refuse before any
            // source read so no late page can be silently omitted.
            TextOwnerCursor::check_candidate_bound(bound)?;
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
        for (index, cursor) in cursors.iter_mut().enumerate() {
            heads.push(
                observation.posting(
                    cursor,
                    &mut occurrences,
                    &mut context,
                    *positions_required
                        .get(index)
                        .ok_or(IndexError::ResourceLimited)?,
                )?,
            );
        }
        let mut matched = 0u64;
        let mut remaining = 0u64;
        let mut anchor_found = rank_after.is_none();
        let mut phrase_candidates = [0u32; MAX_QUERY_NODES];
        let mut admitted = None;
        let mut overlay_documents = overlay.map(|(corpus, _)| corpus.documents().peekable());
        if let Some(probe) = observation.phase_probe.as_mut() {
            probe.mark(3);
        }
        loop {
            context.budget.checkpoint()?;
            if universe.is_some() && admitted.is_none() {
                admitted = universe
                    .as_mut()
                    .map(|cursor| cursor.next(&mut context))
                    .transpose()?
                    .flatten();
            }
            if universe.as_ref().is_some_and(CandidateUniverse::is_full)
                && let Some(row) = admitted.as_ref()
                && heads.iter().flatten().any(|hit| hit.owner < row.owner)
            {
                return Err(IndexError::CorruptObject(
                    "native text posting owner is absent from admitted universe".into(),
                ));
            }
            let posting_candidate = heads.iter().flatten().map(|hit| hit.owner).min();
            let durable_candidate = if universe.as_ref().is_some_and(CandidateUniverse::is_full) {
                admitted.as_ref().map(|row| row.owner)
            } else {
                admitted
                    .as_ref()
                    .map(|row| row.owner)
                    .into_iter()
                    .chain(posting_candidate)
                    .min()
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
            if let Some(probe) = observation.phase_probe.as_mut() {
                probe.candidates = probe.candidates.saturating_add(1);
            }
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
                        let tf = hit.frequency()?;
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
                let in_scope = match source {
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
                // The posting union is drained for corruption detection, but
                // only a structural candidate can satisfy the program, so only
                // such owners (or a path-field hit) are verified against source
                // bytes and ranked. This keeps verification within the bound.
                let selected = in_scope
                    && (path_hit.is_some()
                        || structural_candidate(
                            program,
                            &term_remap,
                            present,
                            |index| {
                                contains_anchors
                                    .get(index)
                                    .and_then(Option::as_ref)
                                    .map(|anchor| {
                                        overlay_row(owner).map_or_else(
                                            || anchor.contains(owner),
                                            |row| overlay_contains(row, index),
                                        )
                                    })
                            },
                            |leaf| {
                                if path_filtered(leaf) {
                                    return None;
                                }
                                Some(overlay_row(owner).map_or_else(
                                    || census_set(leaf).is_some_and(|set| set.contains(owner)),
                                    |row| row.path_leaf(leaf),
                                ))
                            },
                        )?
                        .is_none_or(|member| member));
                // A query without `file.contains` ranks its body regexes on
                // the bytes the regex verification reads.
                let regex_rank = code_search
                    .as_ref()
                    .filter(|rank| rank.reads_content() && source_contains_count == 0);
                let (regex_present, path_match, regex_ranked) = if selected
                    && (regex_program.has_content() || regex_program.has_record_filters())
                {
                    match source {
                        NativeTextHitSource::Durable(source_ref) => regex_program.evaluate(
                            self,
                            owner,
                            source_ref,
                            path_mode,
                            regex_rank,
                            &mut context,
                        )?,
                        NativeTextHitSource::Overlay { .. } => {
                            let overlay_source = overlay_source.ok_or(IndexError::Conflict)?;
                            regex_program.evaluate_overlay(
                                overlay_source,
                                regex_rank,
                                context.budget,
                            )?
                        }
                    }
                } else {
                    (0, true, None)
                };
                let (contains_present, body_ranked) =
                    if source_contains_count == 0 || !selected || !path_match {
                        (0, None)
                    } else {
                        let needles = contains_needles
                            .get(..source_contains_count)
                            .ok_or(IndexError::ResourceLimited)?;
                        match source {
                            NativeTextHitSource::Durable(source_ref) => {
                                if let Some(probe) = observation.phase_probe.as_mut() {
                                    probe.source_checks = probe.source_checks.saturating_add(1);
                                }
                                file_contains_durable(
                                    self,
                                    owner,
                                    source_ref,
                                    needles,
                                    plan.options.case_mode(),
                                    path_mode,
                                    code_search.as_ref(),
                                    &mut context,
                                )?
                            }
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
                                let mask = file_contains_mask(
                                    overlay_source.body,
                                    needles,
                                    plan.options.case_mode(),
                                    context.budget,
                                )?;
                                let ranked = code_search
                                    .as_ref()
                                    .map(|rank| {
                                        rank.score(
                                            Some(overlay_source.body),
                                            overlay_source.path,
                                            context.budget,
                                        )
                                    })
                                    .transpose()?
                                    .flatten();
                                (mask, ranked)
                            }
                        }
                    };
                // The census is exact for a durable owner's pinned path; an
                // overlay row is decided on its own path.
                // A filtering path leaf reads the candidate's authenticated
                // path once; only verified candidates pay for it.
                let filtered_paths = match (overlay_row(owner), source) {
                    (None, NativeTextHitSource::Durable(source_ref))
                        if filtered_leaves != 0 && selected && path_match =>
                    {
                        let record = super::text::authenticated_text_source(
                            self,
                            owner,
                            Some(source_ref),
                            path_mode,
                            &mut context,
                        )?;
                        path_leaf_mask(
                            &record.path,
                            path_leaves,
                            plan.options.case_mode(),
                            context.budget,
                        )?
                    }
                    _ => 0,
                };
                let mut path_present = 0u8;
                for ordinal in 0..source_path_count {
                    let matched = overlay_row(owner).map_or_else(
                        || {
                            if path_filtered(Some(ordinal)) {
                                mask_bit(filtered_paths, ordinal)
                            } else {
                                census_set(Some(ordinal)).is_some_and(|set| set.contains(owner))
                            }
                        },
                        |row| row.path_leaf(Some(ordinal)),
                    );
                    if matched {
                        path_present |= 1u8
                            .checked_shl(
                                u32::try_from(ordinal)
                                    .map_err(|_width| IndexError::ResourceLimited)?,
                            )
                            .ok_or(IndexError::ResourceLimited)?;
                    }
                }
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
                            heads
                                .get(*index)
                                .and_then(Option::as_ref)
                                .filter(|hit| hit.owner == owner)
                                .ok_or_else(|| {
                                    IndexError::BackendViolation(
                                        "phrase candidate lacks validated posting".into(),
                                    )
                                })?
                                .positions()?
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
                    let positions = positional.get(..count).ok_or(IndexError::ResourceLimited)?;
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
                        path_present,
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
                        path_present,
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
                    let mut hit = NativeTextRankedHit {
                        owner,
                        source,
                        score: value,
                        code_search: None,
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
                        if let Some(rank) = code_search.as_ref() {
                            // A body term was ranked on the verified body; a
                            // path-only query is ranked on the authenticated path.
                            let ranked = if rank.reads_content() {
                                body_ranked.or(regex_ranked)
                            } else {
                                rank.score(None, &record.path, context.budget)?
                            }
                            .ok_or_else(|| {
                                IndexError::BackendViolation(
                                    "a matching CodeSearch file lacks its Legacy witness".into(),
                                )
                            })?;
                            hit.score = f64::from(ranked.score.total());
                            hit.code_search = Some(ranked.matched);
                        }
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
                        if let Some(capture) = order_capture.as_mut() {
                            capture.push(
                                TextRankEntry {
                                    score: FiniteScore::new(value).map_err(|_error| {
                                        IndexError::CorruptObject(
                                            "nonfinite native ranked score".into(),
                                        )
                                    })?,
                                    owner,
                                    code_search: hit.code_search,
                                },
                                context.budget,
                            )?;
                        }
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
            for (index, (cursor, head)) in cursors.iter_mut().zip(heads.iter_mut()).enumerate() {
                if head.as_ref().is_some_and(|hit| hit.owner == owner) {
                    *head = observation.posting(
                        cursor,
                        &mut occurrences,
                        &mut context,
                        *positions_required
                            .get(index)
                            .ok_or(IndexError::ResourceLimited)?,
                    )?;
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
        if universe.as_ref().is_some_and(CandidateUniverse::is_full)
            && heads.iter().any(Option::is_some)
        {
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
            let code_search: fn(&PathGroupPayload) -> Option<CodeSearchMatch> =
                |(hit, _, _)| hit.code_search;
            let groups = groups.finish(
                capacity,
                path_rank_after,
                order_capture.as_mut().map(|capture| (capture, code_search)),
                context.budget,
            )?;
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
                order: order_capture.and_then(|capture| capture.finish(member.artifact, true)),
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
        let order = order_capture.and_then(|mut capture| {
            capture.sort_by(|a, b| compare_rank(a.score.get(), a.owner, b.score.get(), b.owner));
            capture.finish(member.artifact, false)
        });
        let mut ranked = NativeTextRanking {
            view,
            artifact: member.artifact,
            matched,
            remaining,
            hits: Vec::new(),
            order,
            _hit_memory: hit_memory,
        };
        ranked
            .hits
            .try_reserve_exact(heap.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        match (heap, preview) {
            (TextHeap::Ranked(heap), None) => {
                ranked.hits.extend(heap.drain().map(|hit| hit.0));
                ranked
                    .hits
                    .sort_unstable_by(|a, b| compare_rank(a.score, a.owner, b.score, b.owner));
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

mod preview_projection;
mod preview_truth;

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

    /// A term's text, whether it is a regex, and its Legacy scope.
    type Shaped<'a> = Option<Vec<(&'a str, bool, TermScope)>>;

    fn shaped<'a>(terms: &[ShapeTerm<'a>]) -> Vec<(&'a str, bool, TermScope)> {
        terms
            .iter()
            .map(|term| (term.text, term.regex, term.scope()))
            .collect()
    }

    #[test]
    fn code_search_shape_maps_leaves_to_legacy_scopes() -> Result<(), IndexError> {
        use TermScope::{Both, Content, Path};
        let contains = [Some("alpha"), Some("beta")];
        let regexes = [Some("a.c"), Some("x+")];
        let paths = [
            PathLeafSource::Contains("alpha"),
            PathLeafSource::Contains("gamma"),
            PathLeafSource::Regex("a.c"),
            PathLeafSource::Regex("b+"),
        ];
        let shape =
            |program: &[Clause]| -> Result<Shaped<'static>, IndexError> {
                Ok(code_search_terms(program, &contains, &regexes, &paths)?
                    .map(|terms| shaped(&terms)))
            };
        check_eq!(
            shape(&[Clause::Contains(0)])?,
            Some(vec![("alpha", false, Content)])
        );
        check_eq!(
            shape(&[Clause::PathContains(1)])?,
            Some(vec![("gamma", false, Path)])
        );
        check_eq!(
            shape(&[Clause::Regex(0)])?,
            Some(vec![("a.c", true, Content)])
        );
        check_eq!(
            shape(&[Clause::PathContains(3)])?,
            Some(vec![("b+", true, Path)])
        );
        for program in [
            [Clause::Contains(0), Clause::PathContains(0), Clause::Any(2)],
            [Clause::PathContains(0), Clause::Contains(0), Clause::Any(2)],
        ] {
            check_eq!(shape(&program)?, Some(vec![("alpha", false, Both)]));
        }
        for program in [
            [Clause::Regex(0), Clause::PathContains(2), Clause::Any(2)],
            [Clause::PathContains(2), Clause::Regex(0), Clause::Any(2)],
        ] {
            check_eq!(shape(&program)?, Some(vec![("a.c", true, Both)]));
        }
        check_eq!(
            shape(&[
                Clause::Contains(0),
                Clause::PathContains(0),
                Clause::Any(2),
                Clause::Contains(1),
                Clause::PathContains(1),
                Clause::Regex(1),
                Clause::PathContains(3),
                Clause::All(5),
            ])?,
            Some(vec![
                ("alpha", false, Both),
                ("beta", false, Content),
                ("gamma", false, Path),
                ("x+", true, Content),
                ("b+", true, Path),
            ])
        );
        // Different texts or kinds, wider or nested alternatives, and every
        // other clause keep the BM25 ranking.
        for program in [
            &[Clause::Contains(1), Clause::PathContains(0), Clause::Any(2)][..],
            &[Clause::Regex(1), Clause::PathContains(2), Clause::Any(2)][..],
            &[Clause::Contains(0), Clause::PathContains(2), Clause::Any(2)][..],
            &[Clause::Regex(0), Clause::PathContains(0), Clause::Any(2)][..],
            &[Clause::Contains(0), Clause::Contains(1), Clause::Any(2)][..],
            &[
                Clause::Contains(0),
                Clause::PathContains(0),
                Clause::Contains(1),
                Clause::Any(3),
            ][..],
            &[
                Clause::Contains(0),
                Clause::PathContains(0),
                Clause::Any(2),
                Clause::Contains(1),
                Clause::Any(2),
            ][..],
            &[Clause::Contains(0), Clause::Contains(1), Clause::All(3)][..],
            &[Clause::Term(0)][..],
            &[Clause::Contains(0), Clause::Not][..],
            &[Clause::Contains(0), Clause::Repo(0), Clause::All(2)][..],
            &[Clause::PathFilters][..],
        ] {
            check_eq!(shape(program)?, None);
        }
        Ok(())
    }

    /// The planner keeps a body-or-path term as one two-leaf alternative, in
    /// either spelling order, inside the conjunction.
    #[test]
    fn planned_body_or_path_terms_are_code_search_shaped() -> Result<(), IndexError> {
        for (query, expected) in [
            (
                "file.contains('alpha') OR path.contains('alpha')",
                vec![("alpha", false, TermScope::Both)],
            ),
            (
                "(path.contains('alpha') OR file.contains('alpha')) file.contains('beta')",
                vec![
                    ("alpha", false, TermScope::Both),
                    ("beta", false, TermScope::Content),
                ],
            ),
            (
                "(/a[.]b/ OR path.regex('a[.]b')) path.regex('c+') /d/",
                vec![
                    ("a[.]b", true, TermScope::Both),
                    ("c+", true, TermScope::Path),
                    ("d", true, TermScope::Content),
                ],
            ),
        ] {
            let parsed = quanta_index_contract::parse_native_content_query(query)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
            let plan = crate::planner::LexicalPlanner::plan(&parsed)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
            let mut program = Vec::new();
            let mut contains = Vec::new();
            let mut regexes = Vec::new();
            let mut paths = Vec::new();
            let _anchored = compile_clauses_with_repo(
                &plan.root,
                0,
                &mut Complexity::default(),
                &mut program,
                &mut |_term| Err(IndexError::BackendViolation("unexpected term".into())),
                &mut |_raw, _phrase| Err(IndexError::BackendViolation("unexpected phrase".into())),
                &mut |source| {
                    regexes.push(Some(source));
                    Ok(regexes.len() - 1)
                },
                &mut |needle| {
                    contains.push(Some(needle));
                    Ok(contains.len() - 1)
                },
                &mut |leaf| {
                    paths.push(leaf);
                    Ok(paths.len() - 1)
                },
                &mut |_repo| Err(IndexError::BackendViolation("unexpected repo".into())),
            )?;
            let mut terms = code_search_terms(&program, &contains, &regexes, &paths)?
                .map(|terms| shaped(&terms))
                .ok_or_else(|| IndexError::BackendViolation(format!("{query} is not shaped")))?;
            terms.sort_unstable_by_key(|(text, _, _)| *text);
            check_eq!(terms, expected);
        }
        Ok(())
    }

    #[test]
    fn structural_bound_follows_candidate_set_algebra() -> Result<(), IndexError> {
        // Fixed DFs: t0 = 4,096, t1 = 1, t2 = 7. Phrase slots remap 0->0,
        // 1->2 and 2->1, so a phrase over slots 0..2 names t0 and t2.
        let dfs = [4_096u64, 1, 7];
        let df = |index: usize| dfs.get(index).copied().ok_or(IndexError::ResourceLimited);
        let remap = [0usize, 2, 1];
        let term = Clause::Term;
        let phrase = Clause::Phrase {
            first: 0,
            count: 2,
            cap: 1,
        };
        for (program, bound) in [
            (vec![term(0)], Some(4_096)),
            (vec![term(0), term(1), Clause::All(2)], Some(1)),
            (vec![term(0), term(1), Clause::Any(2)], Some(4_097)),
            (vec![term(1), term(0), Clause::Not, Clause::All(2)], Some(1)),
            (vec![term(1), Clause::Regex(0), Clause::All(2)], Some(1)),
            (vec![term(1), Clause::Regex(0), Clause::Any(2)], None),
            (vec![term(1), Clause::Not], None),
            (
                vec![Clause::Contains(0), Clause::Repo(0), Clause::All(2)],
                None,
            ),
            (
                vec![term(0), term(1), Clause::All(2), term(2), Clause::Any(2)],
                Some(8),
            ),
            (vec![phrase], Some(7)),
            (vec![phrase, term(1), Clause::Any(2)], Some(8)),
        ] {
            check_eq!(
                candidate_bound(&program, &remap, df, |_| Ok(None), |_| Ok(Some(0)))?,
                bound
            );
        }
        let overflow = [u64::MAX, 1];
        check!(matches!(
            candidate_bound(
                &[term(0), term(1), Clause::Any(2)],
                &remap,
                |index| {
                    overflow
                        .get(index)
                        .copied()
                        .ok_or(IndexError::ResourceLimited)
                },
                |_| Ok(None),
                |_| Ok(Some(0))
            ),
            Err(IndexError::ResourceLimited)
        ));
        // Membership: t0 AND t1 needs both; a complement and a regex filter
        // only; a phrase needs every named term.
        let conjunction = [term(0), term(1), Clause::All(2)];
        check_eq!(
            structural_candidate(&conjunction, &remap, 0b01, |_| None, |_| Some(false))?,
            Some(false)
        );
        check_eq!(
            structural_candidate(&conjunction, &remap, 0b11, |_| None, |_| Some(false))?,
            Some(true)
        );
        let negated = [term(1), term(0), Clause::Not, Clause::All(2)];
        check_eq!(
            structural_candidate(&negated, &remap, 0b11, |_| None, |_| Some(false))?,
            Some(true)
        );
        check_eq!(
            structural_candidate(&negated, &remap, 0b01, |_| None, |_| Some(false))?,
            Some(false)
        );
        let open = [term(1), Clause::Regex(0), Clause::Any(2)];
        check_eq!(
            structural_candidate(&open, &remap, 0, |_| None, |_| Some(false))?,
            None
        );
        check_eq!(
            structural_candidate(&[phrase], &remap, 0b101, |_| None, |_| Some(false))?,
            Some(true)
        );
        check_eq!(
            structural_candidate(&[phrase], &remap, 0b011, |_| None, |_| Some(false))?,
            Some(false)
        );
        Ok(())
    }

    #[test]
    fn anchored_file_contains_is_a_finite_candidate_supplier() -> Result<(), IndexError> {
        // Fixed bounds: t0 has DF 4,096; contains #0 is anchored with 3
        // Literal candidates; contains #1 is unanchored.
        let df = |index: usize| {
            [4_096u64]
                .get(index)
                .copied()
                .ok_or(IndexError::ResourceLimited)
        };
        let anchor = |index: usize| Ok([Some(3_u64), None].get(index).copied().flatten());
        let term = Clause::Term;
        for (program, bound) in [
            (vec![Clause::Contains(0)], Some(3)),
            (vec![Clause::Contains(1)], None),
            (vec![term(0), Clause::Contains(0), Clause::All(2)], Some(3)),
            (
                vec![term(0), Clause::Contains(0), Clause::Any(2)],
                Some(4_099),
            ),
            (
                vec![Clause::Contains(0), Clause::Contains(1), Clause::Any(2)],
                None,
            ),
            (
                vec![Clause::Contains(0), Clause::Contains(1), Clause::All(2)],
                Some(3),
            ),
            (vec![Clause::Contains(0), Clause::Not], None),
        ] {
            check_eq!(
                candidate_bound(&program, &[], df, anchor, |_| Ok(Some(0)))?,
                bound
            );
            check_eq!(
                structurally_anchored(&program, |index| index == 0, true)?,
                bound.is_some()
            );
        }
        // Path leaves are finite from their exact census: path #0 has 2
        // matching owners and the path-only filter census has 5.
        let paths = |leaf: Option<usize>| -> Result<Option<u64>, IndexError> {
            Ok(Some(if leaf == Some(0) { 2 } else { 5 }))
        };
        for (program, bound) in [
            (vec![Clause::PathContains(0)], Some(2)),
            (vec![Clause::PathFilters], Some(5)),
            (
                vec![Clause::Contains(1), Clause::PathContains(0), Clause::Any(2)],
                None,
            ),
            (
                vec![Clause::Contains(0), Clause::PathContains(0), Clause::Any(2)],
                Some(5),
            ),
            (
                vec![term(0), Clause::PathContains(0), Clause::All(2)],
                Some(2),
            ),
            (vec![Clause::PathContains(0), Clause::Not], None),
        ] {
            check_eq!(candidate_bound(&program, &[], df, anchor, paths)?, bound);
            check_eq!(
                structurally_anchored(&program, |index| index == 0, true)?,
                bound.is_some()
            );
        }
        // Membership follows the anchored set; an unanchored leaf filters.
        let either = [Clause::Contains(0), Clause::Contains(1), Clause::All(2)];
        check_eq!(
            structural_candidate(
                &either,
                &[],
                0,
                |index| (index == 0).then_some(true),
                |_| Some(false)
            )?,
            Some(true)
        );
        check_eq!(
            structural_candidate(
                &either,
                &[],
                0,
                |index| (index == 0).then_some(false),
                |_| Some(false)
            )?,
            Some(false)
        );
        Ok(())
    }

    /// A generated query tree; its postfix form is the compiled program.
    #[derive(Clone, Debug)]
    enum Tree {
        Term(usize),
        Contains(usize),
        Path(usize),
        Regex,
        Repo,
        Not(Box<Tree>),
        All(Vec<Tree>),
        Any(Vec<Tree>),
    }

    /// A fixed-seed linear congruential generator; tests need no entropy.
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0
                .checked_shr(33)
                .and_then(|high| high.checked_rem(bound.max(1)))
                .map_or(0, |value| value)
        }
    }

    #[expect(
        clippy::arithmetic_side_effects,
        clippy::disallowed_methods,
        reason = "fixed small generator bounds; a failed index conversion picks leaf 0"
    )]
    fn generate(random: &mut Lcg, depth: u32) -> Tree {
        let leaf = depth == 0 || random.below(3) == 0;
        if leaf {
            return match random.below(6) {
                0 | 1 => Tree::Term(usize::try_from(random.below(3)).unwrap_or(0)),
                2 => Tree::Contains(usize::try_from(random.below(2)).unwrap_or(0)),
                3 => Tree::Path(usize::try_from(random.below(2)).unwrap_or(0)),
                4 => Tree::Regex,
                _ => Tree::Repo,
            };
        }
        match random.below(3) {
            0 => Tree::Not(Box::new(generate(random, depth - 1))),
            choice => {
                let width = 2 + random.below(2);
                let children = (0..width).map(|_| generate(random, depth - 1)).collect();
                if choice == 1 {
                    Tree::All(children)
                } else {
                    Tree::Any(children)
                }
            }
        }
    }

    fn postfix(tree: &Tree, program: &mut Vec<Clause>) {
        match tree {
            Tree::Term(index) => program.push(Clause::Term(*index)),
            Tree::Contains(index) => program.push(Clause::Contains(*index)),
            Tree::Path(index) => program.push(Clause::PathContains(*index)),
            Tree::Regex => program.push(Clause::Regex(0)),
            Tree::Repo => program.push(Clause::Repo(0)),
            Tree::Not(child) => {
                postfix(child, program);
                program.push(Clause::Not);
            }
            Tree::All(children) | Tree::Any(children) => {
                for child in children {
                    postfix(child, program);
                }
                program.push(if matches!(tree, Tree::All(_)) {
                    Clause::All(children.len())
                } else {
                    Clause::Any(children.len())
                });
            }
        }
    }

    /// The oracle: whether `owner` satisfies the tree when the leaves the
    /// structure cannot see (regex, repository, unanchored body) take
    /// `free`, one bit per free leaf in visit order.
    #[expect(
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::option_if_let_else,
        reason = "the oracle indexes its own fixed leaf sets and counts at most 64 free leaves"
    )]
    fn holds(tree: &Tree, owner: usize, sets: &Sets, free: u64, next_free: &mut u32) -> bool {
        let mut free_bit = || {
            let bit = (free >> *next_free) & 1 == 1;
            *next_free += 1;
            bit
        };
        match tree {
            Tree::Term(index) => sets.terms[*index].contains(&owner),
            Tree::Contains(index) => match &sets.contains[*index] {
                Some(set) => set.contains(&owner),
                None => free_bit(),
            },
            Tree::Path(index) => sets.paths[*index].contains(&owner),
            Tree::Regex | Tree::Repo => free_bit(),
            Tree::Not(child) => !holds(child, owner, sets, free, next_free),
            Tree::All(children) => children.iter().fold(true, |all, child| {
                holds(child, owner, sets, free, next_free) && all
            }),
            Tree::Any(children) => children.iter().fold(false, |any, child| {
                holds(child, owner, sets, free, next_free) || any
            }),
        }
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "the oracle indexes its own fixed leaf sets"
    )]
    fn free_leaves(tree: &Tree, sets: &Sets) -> u32 {
        match tree {
            Tree::Term(_) | Tree::Path(_) => 0,
            Tree::Contains(index) => u32::from(sets.contains[*index].is_none()),
            Tree::Regex | Tree::Repo => 1,
            Tree::Not(child) => free_leaves(child, sets),
            Tree::All(children) | Tree::Any(children) => {
                children.iter().map(|child| free_leaves(child, sets)).sum()
            }
        }
    }

    struct Sets {
        terms: [Vec<usize>; 3],
        contains: [Option<Vec<usize>>; 2],
        paths: [Vec<usize>; 2],
    }

    /// Random programs over random leaf sets: the structural bound covers
    /// every owner that can satisfy the program under some outcome of the
    /// leaves it cannot see, and a structural non-candidate never can.
    #[test]
    #[expect(
        clippy::indexing_slicing,
        clippy::disallowed_methods,
        reason = "the oracle indexes its own fixed leaf sets; widths are below 64"
    )]
    fn structural_bound_and_membership_cover_a_brute_force_oracle() -> Result<(), IndexError> {
        const OWNERS: usize = 10;
        let mut random = Lcg(0x5eed_0009_0002);
        for _case in 0..400 {
            let set = |random: &mut Lcg| -> Vec<usize> {
                (0..OWNERS).filter(|_| random.below(3) == 0).collect()
            };
            let sets = Sets {
                terms: [set(&mut random), set(&mut random), set(&mut random)],
                contains: [
                    (random.below(2) == 0).then(|| set(&mut random)),
                    (random.below(2) == 0).then(|| set(&mut random)),
                ],
                paths: [set(&mut random), set(&mut random)],
            };
            let tree = generate(&mut random, 3);
            let mut program = Vec::new();
            postfix(&tree, &mut program);
            let width = |values: &[usize]| u64::try_from(values.len()).unwrap_or(u64::MAX);
            let bound = candidate_bound(
                &program,
                &[],
                |index| Ok(width(&sets.terms[index])),
                |index| Ok(sets.contains[index].as_deref().map(width)),
                |leaf| Ok(leaf.map(|index| width(&sets.paths[index]))),
            )?;
            check_eq!(
                structurally_anchored(&program, |index| sets.contains[index].is_some(), true)?,
                bound.is_some()
            );
            let free = free_leaves(&tree, &sets);
            let mut satisfiable = 0u64;
            for owner in 0..OWNERS {
                let can = (0..(1u64 << free)).any(|outcome| {
                    let mut next = 0;
                    holds(&tree, owner, &sets, outcome, &mut next)
                });
                satisfiable += u64::from(can);
                let present = (0..3).fold(0u64, |mask, index| {
                    mask | (u64::from(sets.terms[index].contains(&owner)) << index)
                });
                let member = structural_candidate(
                    &program,
                    &[],
                    present,
                    |index| {
                        sets.contains[index]
                            .as_ref()
                            .map(|set| set.contains(&owner))
                    },
                    |leaf| leaf.map(|index| sets.paths[index].contains(&owner)),
                )?;
                check!(!can || member != Some(false));
            }
            if let Some(bound) = bound {
                check!(satisfiable <= bound);
            }
        }
        Ok(())
    }

    #[test]
    fn structural_non_candidates_never_satisfy_any_source_outcome() -> Result<(), IndexError> {
        let term = Clause::Term;
        let programs = [
            vec![term(0), term(1), Clause::All(2)],
            vec![term(0), term(1), Clause::Not, Clause::All(2)],
            vec![
                term(0),
                Clause::Regex(0),
                Clause::All(2),
                term(2),
                Clause::Any(2),
            ],
            vec![
                term(0),
                Clause::Contains(0),
                Clause::Repo(0),
                Clause::Any(2),
                Clause::All(2),
            ],
            vec![term(0), Clause::Regex(0), Clause::Not, Clause::All(2)],
        ];
        for program in &programs {
            for present in 0..8u64 {
                let member =
                    structural_candidate(program, &[], present, |_| None, |_| Some(false))?;
                for source in 0..2u8 {
                    let matched = evaluate_boolean_with_repo(
                        program,
                        present,
                        source,
                        source,
                        source,
                        source,
                        |_ordinal, _matched| Ok(()),
                        |_ordinal, _first, _count, _cap| {
                            Err(IndexError::BackendViolation(
                                "unexpected phrase in structural gold".into(),
                            ))
                        },
                    )?
                    .matched;
                    check!(!matched || member != Some(false));
                }
            }
        }
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
