//! File-level product code search on the generation's complete source bytes.
//!
//! The public request lowers to the existing `LqQuery` IR with a dedicated
//! pattern type. This executor does not reinterpret Native LQ raw-string
//! leaves: its AND and ranking unit is one immutable source file.

mod ranking;

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::ops::Range;
use std::time::Instant;

use quanta_index_contract::{
    CODE_SEARCH_IDENTIFIER_TYPO_PREDICATE, CODE_SEARCH_SYMBOL_COMPONENTS_PREDICATE, HighlightSpan,
    LexicalCandidate, LexicalCursor, LexicalRowOrderKey, LqExpr, LqFilter, LqLeaf, LqPatternType,
    LqPredicateArg, LqQuery, LqSelect, MAX_CODE_SEARCH_TERM_BYTES, MAX_CODE_SEARCH_TERMS,
    PreviewByteRange, PreviewKind, PreviewMetadata, PreviewUnavailableReason, QueryConstraintSetV1,
    SourceFileKey, SymbolCoverage, SymbolNameSourcePolicyV1, valid_code_search_typo_identifier,
};
use quanta_index_core::{
    CodeSearchExecutionModeV1, CodeSearchExecutionStatsV1, CodeSearchScoreComponentsV1,
    LexicalCandidateExplanationV1, LexicalScoreEngineV1, LexicalScoreTraceV1,
};
use quanta_index_core::{CoreError, LexicalPageSpec, LexicalSearchPageV1, RequestBudgetV1};
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticSpanGuardV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};
use quanta_index_lq_regex::RegexExecutor;
#[cfg(test)]
use quanta_index_lq_trigram::TrigramIndex;
use quanta_index_lq_trigram::{MAX_CANDIDATE_PRE_VERIFY, NecessaryGramExpr, Trigram, trigrams_of};
use sha2::{Digest as _, Sha256};
use tantivy::Term;
use tantivy::query::{BooleanQuery, Occur, TermQuery};
use tantivy::schema::{IndexRecordOption, TantivyDocument, Value as _};

use crate::TantivySearcher;
use crate::file_authority::paged::{
    FileMetadata as SourceFileMetadata, PagedFileAuthority as FileAuthority,
};
use crate::file_authority::{QueryWork, SourceFile};
use crate::regex::RegexPolicy;
use crate::searcher::planner_errors::map_regex_plan_error;
use quanta_index_lq_text_normalizer::{self as normalize, CaseMode, MappedText, MappingError};

const MAX_SHORT_SCAN_FILES: usize = 10_000;
const MAX_SHORT_SCAN_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PREVIEW_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_REGEX_SCAN_FILES: usize = 10_000;
const MAX_REGEX_SCAN_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_REGEX_MATCHES_PER_SURFACE: usize = 4_096;
const MAX_REGEX_TERMS: usize = 4;
const MAX_TYPO_TOKEN_COMPARISONS: usize = 1_000_000;
const MAX_TYPO_POSTING_VISITS: usize = 2_000_000;

fn checked_count_u64(value: usize, field: &str) -> Result<u64, CoreError> {
    u64::try_from(value)
        .map_err(|error| CoreError::Storage(format!("lexical: {field} exceeds u64: {error}")))
}

fn observed_ns(started: Instant) -> Result<u64, CoreError> {
    u64::try_from(started.elapsed().as_nanos()).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: observed duration exceeds u64 nanoseconds: {error}"
        ))
    })
}

fn add_observed_ns(total: &mut u64, started: Instant, stage: &str) -> Result<(), CoreError> {
    *total = total
        .checked_add(observed_ns(started)?)
        .ok_or_else(|| CoreError::Storage(format!("lexical: {stage} duration exceeds u64")))?;
    Ok(())
}

/// Emit the work completed before a verifier returns, including a partial
/// match set when a later candidate or budget check fails.
fn finish_verify_observation(
    diagnostic: Option<&DiagnosticContextV1>,
    span: Option<DiagnosticSpanGuardV1>,
    attempted: u64,
    matched: u64,
    succeeded: bool,
) {
    if let (Some(context), Some(span)) = (diagnostic, span) {
        context.counter(&span, DiagnosticCounterV1::CandidatesVerified, attempted);
        context.counter(&span, DiagnosticCounterV1::Matches, matched);
        let _finished = span.finish(if succeeded {
            DiagnosticTerminalV1::Ok
        } else {
            DiagnosticTerminalV1::Error { code: 1 }
        });
    }
}

// Request-local bounded memoization; this is neither persisted nor externally
// ordered, so hashing keeps repeated-token distance lookup inexpensive.
#[expect(
    clippy::disallowed_types,
    reason = "bounded request-local memoization is not persisted or externally ordered"
)]
type TypoDistanceCache = std::collections::HashMap<Vec<u8>, Option<u8>>;

#[derive(Clone, Copy)]
enum Scope {
    Both,
    Content,
    Path,
}

pub(crate) struct CodeSearchTerm {
    text: String,
    needle: String,
    scope: Scope,
    regex: Option<RegexExecutor>,
    necessary_grams: Option<NecessaryGramExpr>,
}

/// Validated executor plan derived from the canonical `LqQuery`, not a wire IR.
pub(crate) struct CodeSearchPlan {
    terms: Vec<CodeSearchTerm>,
    case: CaseMode,
    typo: Option<String>,
    components: Option<Vec<String>>,
}

fn unsupported(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
        message: format!("lexical code search: {reason}"),
    }
}

fn is_query_guard_refusal(error: &CoreError) -> bool {
    matches!(
        error,
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded
                | quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded
                | quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
            ..
        }
    )
}

fn metadata_for_id(
    authority: &FileAuthority,
    id: u64,
    budget: &RequestBudgetV1,
) -> Result<SourceFileMetadata, CoreError> {
    authority.metadata_for_id(id, Some(budget))?.ok_or_else(|| {
        CoreError::Storage("lexical: candidate ID is absent from file authority".into())
    })
}

fn metadata_for_key(
    authority: &FileAuthority,
    key: &SourceFileKey,
    budget: &RequestBudgetV1,
) -> Result<SourceFileMetadata, CoreError> {
    authority
        .metadata_for_key(key, Some(budget))?
        .ok_or_else(|| {
            CoreError::Storage("lexical: candidate key is absent from file authority".into())
        })
}

fn byte_offset(value: usize) -> Result<u64, CoreError> {
    u64::try_from(value)
        .map_err(|error| CoreError::Storage(format!("lexical: byte offset overflow: {error}")))
}

/// Detect case flags as regex syntax, not bytes inside a literal character class.
/// A malformed pattern keeps its normal typed parse error in the regex planner.
fn regex_has_case_override(pattern: &str) -> bool {
    use regex_syntax::ast::{Ast, Flag};

    fn contains_case_flag(ast: &Ast) -> bool {
        match ast {
            Ast::Flags(flags) => flags.flags.flag_state(Flag::CaseInsensitive).is_some(),
            Ast::Group(group) => {
                group
                    .flags()
                    .is_some_and(|flags| flags.flag_state(Flag::CaseInsensitive).is_some())
                    || contains_case_flag(&group.ast)
            }
            Ast::Repetition(repetition) => contains_case_flag(&repetition.ast),
            Ast::Concat(concat) => concat.asts.iter().any(contains_case_flag),
            Ast::Alternation(alternation) => alternation.asts.iter().any(contains_case_flag),
            Ast::Empty(_)
            | Ast::Literal(_)
            | Ast::Dot(_)
            | Ast::Assertion(_)
            | Ast::ClassUnicode(_)
            | Ast::ClassPerl(_)
            | Ast::ClassBracketed(_) => false,
        }
    }

    pattern.contains("(?")
        && regex_syntax::ast::parse::Parser::new()
            .parse(pattern)
            .is_ok_and(|ast| contains_case_flag(&ast))
}

impl CodeSearchPlan {
    pub(crate) fn parse(query: &LqQuery, regex_policy: &RegexPolicy) -> Result<Self, CoreError> {
        if query.options.pattern_type != LqPatternType::CodeSearch {
            return Err(unsupported("request is not code_search"));
        }
        if !query.directives.is_empty()
            || query.options.count.is_some()
            || query.options.timeout_ms.is_some()
            || query.options.index_mode.is_some()
            || query.options.boost_millis.is_some()
        {
            return Err(unsupported(
                "code_search does not admit native directives or ranking options",
            ));
        }
        if query.filters.as_slice()
            != [LqFilter::Select {
                dim: LqSelect::File,
            }]
        {
            return Err(unsupported("code_search requires exactly select:file"));
        }
        let parts: &[LqExpr] = match &query.expr {
            LqExpr::All(parts) => parts,
            leaf @ LqExpr::Leaf(_) => std::slice::from_ref(leaf),
            LqExpr::Empty | LqExpr::Not(_) | LqExpr::Any(_) => {
                return Err(unsupported("code_search requires positive literal leaves"));
            }
        };
        if parts.is_empty() || parts.len() > MAX_CODE_SEARCH_TERMS {
            return Err(unsupported("code_search term count must be 1..=32"));
        }
        if let [LqExpr::Leaf(LqLeaf::Predicate { name, args })] = parts
            && name == CODE_SEARCH_IDENTIFIER_TYPO_PREDICATE
        {
            let [LqPredicateArg::RawString(identifier)] = args.as_slice() else {
                return Err(unsupported("typo: requires one raw identifier"));
            };
            if !valid_code_search_typo_identifier(identifier) {
                return Err(unsupported(
                    "typo: requires an ASCII identifier of 3..=64 bytes",
                ));
            }
            return Ok(Self {
                terms: Vec::new(),
                case: query.options.case_mode(),
                typo: Some(identifier.clone()),
                components: None,
            });
        }
        if let [LqExpr::Leaf(LqLeaf::Predicate { name, args })] = parts
            && name == CODE_SEARCH_SYMBOL_COMPONENTS_PREDICATE
        {
            let [LqPredicateArg::RawString(value)] = args.as_slice() else {
                return Err(unsupported(
                    "components: requires one raw component sequence",
                ));
            };
            if query.options.case_mode() != CaseMode::Folded {
                return Err(unsupported("components: accepts folded case only"));
            }
            let components = crate::symbol_components::query_components(value)
                .ok_or_else(|| unsupported("components: requires canonical lower-case words"))?;
            return Ok(Self {
                terms: Vec::new(),
                case: CaseMode::Folded,
                typo: None,
                components: Some(components),
            });
        }
        let regex_terms = parts
            .iter()
            .filter(|part| match part {
                LqExpr::Leaf(LqLeaf::Regex(_)) => true,
                LqExpr::Leaf(LqLeaf::Predicate { name, .. }) => name.ends_with("_regex"),
                LqExpr::Leaf(
                    LqLeaf::Keyword(_)
                    | LqLeaf::Phrase(_)
                    | LqLeaf::RawString(_)
                    | LqLeaf::StructuralBlock(_),
                )
                | LqExpr::Empty
                | LqExpr::Not(_)
                | LqExpr::All(_)
                | LqExpr::Any(_) => false,
            })
            .count();
        if regex_terms > MAX_REGEX_TERMS {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                message: format!(
                    "lexical code search accepts at most {MAX_REGEX_TERMS} regex terms"
                ),
            });
        }
        let case = query.options.case_mode();
        let mut terms = Vec::with_capacity(parts.len());
        for part in parts {
            let LqExpr::Leaf(leaf) = part else {
                return Err(unsupported(
                    "code_search supports only positive literal leaves",
                ));
            };
            let (text, scope, regex) = match leaf {
                LqLeaf::RawString(text) => (text, Scope::Both, false),
                LqLeaf::Regex(text) => (text, Scope::Both, true),
                LqLeaf::Predicate { name, args } if args.len() == 1 => {
                    let [LqPredicateArg::RawString(text)] = args.as_slice() else {
                        return Err(unsupported("code_search scoped literal must be raw text"));
                    };
                    let scope = match name.as_str() {
                        "code_search.content" | "code_search.content_regex" => Scope::Content,
                        "code_search.path" | "code_search.path_regex" => Scope::Path,
                        _ => return Err(unsupported("unknown code_search scope")),
                    };
                    (text, scope, name.ends_with("_regex"))
                }
                LqLeaf::Keyword(_)
                | LqLeaf::Phrase(_)
                | LqLeaf::StructuralBlock(_)
                | LqLeaf::Predicate { .. } => {
                    return Err(unsupported("code_search leaf is unsupported"));
                }
            };
            if text.is_empty()
                || text.len() > MAX_CODE_SEARCH_TERM_BYTES
                || text.chars().any(char::is_control)
            {
                return Err(unsupported(
                    "code_search literal must be nonempty printable text of at most 256 bytes",
                ));
            }
            let source = normalize::nfc(text).into_owned();
            let (compiled, necessary_grams) = if regex {
                // The normalizer owns an unscoped leading (?i) flag. Scoped
                // predicates do not have a regex-typed argument. Admit an
                // equivalent leading flag under case:no and refuse all other
                // inline case overrides, so case:yes stays exact.
                let body = if let Some(rest) = source.strip_prefix("(?i)") {
                    if case == CaseMode::Sensitive {
                        return Err(unsupported("case:yes conflicts with (?i) regex"));
                    }
                    rest
                } else {
                    source.as_str()
                };
                if regex_has_case_override(body) {
                    return Err(unsupported("regex inline case overrides are unsupported"));
                }
                let effective = TantivySearcher::regex_source_for_options(body, &query.options);
                let (plan, prepared) =
                    crate::regex::plan_regex_prepared(&effective, &query.options, regex_policy)
                        .map_err(map_regex_plan_error)?;
                let necessary_grams = plan.necessary_gram_expr().clone();
                let executor = RegexExecutor::compile_prepared(prepared).map_err(|error| {
                    CoreError::Typed {
                        code: crate::query_errors::regex_wire_code(error.code),
                        message: format!("lexical code search regex: {error}"),
                    }
                })?;
                if executor.hir().properties().minimum_len() == Some(0) {
                    return Err(unsupported("zero-width regex matches are unsupported"));
                }
                (Some(executor), Some(necessary_grams))
            } else {
                (None, None)
            };
            terms.push(CodeSearchTerm {
                needle: normalize::apply_case(&source, case).into_owned(),
                text: source,
                scope,
                regex: compiled,
                necessary_grams,
            });
        }
        Ok(Self {
            terms,
            case,
            typo: None,
            components: None,
        })
    }

    fn auto_typo_on_empty(&self) -> Option<&str> {
        let [term] = self.terms.as_slice() else {
            return None;
        };
        (self.case == CaseMode::Folded
            && matches!(term.scope, Scope::Both)
            && term.regex.is_none()
            && valid_code_search_typo_identifier(&term.text))
        .then_some(term.text.as_str())
    }

    /// Bounded structural counts from the already admitted regex plan. They
    /// describe syntax and necessary prefilter leaves, not match cardinality.
    fn diagnostic_regex_shape(&self) -> (u64, u64, u64) {
        let mut literals = 0_u64;
        let mut alternatives = 0_u64;
        let mut grams = 0_u64;
        for term in &self.terms {
            if let Some(regex) = &term.regex {
                count_hir_shape(regex.hir(), &mut literals, &mut alternatives);
            }
            if let Some(expr) = &term.necessary_grams {
                count_necessary_grams(expr, &mut grams);
            }
        }
        (literals, alternatives, grams)
    }
}

fn count_hir_shape(hir: &regex_syntax::hir::Hir, literals: &mut u64, alternatives: &mut u64) {
    use regex_syntax::hir::HirKind;

    match hir.kind() {
        HirKind::Literal(_) => *literals = literals.saturating_add(1),
        HirKind::Alternation(children) => {
            for child in children {
                *alternatives = alternatives.saturating_add(1);
                count_hir_shape(child, literals, alternatives);
            }
        }
        HirKind::Concat(children) => {
            for child in children {
                count_hir_shape(child, literals, alternatives);
            }
        }
        HirKind::Capture(capture) => count_hir_shape(&capture.sub, literals, alternatives),
        HirKind::Repetition(repetition) => {
            count_hir_shape(&repetition.sub, literals, alternatives);
        }
        HirKind::Empty | HirKind::Class(_) | HirKind::Look(_) => {}
    }
}

fn count_necessary_grams(expr: &NecessaryGramExpr, grams: &mut u64) {
    match expr {
        NecessaryGramExpr::Gram(_) => *grams = grams.saturating_add(1),
        NecessaryGramExpr::And(children) | NecessaryGramExpr::Or(children) => {
            for child in children {
                count_necessary_grams(child, grams);
            }
        }
        NecessaryGramExpr::AllDocs => {}
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum HitSurface {
    Content,
    Path,
}

struct Witness {
    surface: HitSurface,
    normalized: Range<usize>,
    score: u32,
    occurrences: u8,
    mapping_case: CaseMode,
}

/// File rows have fixed line columns (zero) until selected-page preview
/// projection. Keep only the best page keys and their exact-match witnesses.
struct FilePageRow {
    source: SourceFileKey,
    candidate_id: String,
    score: f32,
    witness: Option<Witness>,
    component_name: Option<String>,
}

impl FilePageRow {
    fn order_key(&self) -> LexicalRowOrderKey<'_> {
        LexicalRowOrderKey {
            source_repo_id: self.source.source_repo_id.as_str(),
            score: self.score,
            repo_relative_path: self.source.repo_relative_path.as_str(),
            start_line: 0,
            end_line: 0,
            candidate_id: &self.candidate_id,
        }
    }
}

struct LatestFileRow(FilePageRow);

impl PartialEq for LatestFileRow {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for LatestFileRow {}

impl PartialOrd for LatestFileRow {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LatestFileRow {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.order_key().order(&other.0.order_key())
    }
}

struct FilePageCollector<'a> {
    limit: usize,
    after: Option<&'a LexicalCursor>,
    cursor_eligible: u64,
    rows: BinaryHeap<LatestFileRow>,
}

impl<'a> FilePageCollector<'a> {
    fn new(limit: usize, after: Option<&'a LexicalCursor>) -> Self {
        Self {
            limit,
            after,
            cursor_eligible: 0,
            rows: BinaryHeap::new(),
        }
    }

    fn consider(
        &mut self,
        source: &SourceFileKey,
        score: f32,
        witness: Option<Witness>,
        component_name: Option<String>,
    ) -> Result<(), CoreError> {
        let candidate_id = file_candidate_id(
            source.source_repo_id.as_str(),
            source.repo_relative_path.as_str(),
        )?;
        let key = LexicalRowOrderKey {
            source_repo_id: source.source_repo_id.as_str(),
            score,
            repo_relative_path: source.repo_relative_path.as_str(),
            start_line: 0,
            end_line: 0,
            candidate_id: &candidate_id,
        };
        if self.after.is_some_and(|after| !after.admits(&key)) {
            return Ok(());
        }
        self.cursor_eligible = self.cursor_eligible.checked_add(1).ok_or_else(|| {
            CoreError::Storage("lexical: cursor eligible file count overflow".into())
        })?;
        if self.limit == 0
            || (self.rows.len() == self.limit
                && self.rows.peek().is_some_and(|worst| {
                    key.order(&worst.0.order_key()) != std::cmp::Ordering::Less
                }))
        {
            return Ok(());
        }
        let row = LatestFileRow(FilePageRow {
            source: source.clone(),
            candidate_id,
            score,
            witness,
            component_name,
        });
        if self.rows.len() == self.limit {
            let mut worst = self.rows.peek_mut().ok_or_else(|| {
                CoreError::Storage("lexical: full file page heap has no boundary".into())
            })?;
            *worst = row;
        } else {
            self.rows.try_reserve(1).map_err(|error| {
                CoreError::Storage(format!("lexical: file page allocation failed: {error}"))
            })?;
            self.rows.push(row);
        }
        Ok(())
    }

    fn into_sorted(self) -> (u64, Vec<LatestFileRow>) {
        // BinaryHeap sorts in ascending Ord order in place. Our Ord is the
        // canonical page order, so this needs no second page-sized allocation.
        (self.cursor_eligible, self.rows.into_sorted_vec())
    }
}

fn selected_candidate_buffer(len: usize) -> Result<Vec<LexicalCandidate>, CoreError> {
    let mut candidates = Vec::new();
    candidates.try_reserve_exact(len).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: selected file page allocation failed: {error}"
        ))
    })?;
    Ok(candidates)
}

struct ScoredMatch {
    components: CodeSearchScoreComponentsV1,
    primary: Witness,
    primary_term: usize,
}

#[derive(Clone, Copy)]
enum TermsToScore {
    Literals,
    Regex,
    All,
}

#[derive(Clone, Copy)]
struct CodeSearchTypoRequest<'a> {
    query: &'a LqQuery,
    constraints: &'a QueryConstraintSetV1,
    page: &'a LexicalPageSpec,
    budget: &'a RequestBudgetV1,
    diagnostic: Option<&'a DiagnosticContextV1>,
}

fn add_witness_score(
    scored: &mut Option<ScoredMatch>,
    indexed_text: Option<&str>,
    indexed_path: &str,
    term: &CodeSearchTerm,
    case: CaseMode,
    term_index: usize,
    witness: Witness,
) {
    let case_exact = if term.regex.is_some() || !matches!(case, CaseMode::Folded) {
        false
    } else {
        match witness.surface {
            HitSurface::Content => indexed_text.is_some_and(|text| text.contains(&term.text)),
            HitSurface::Path => indexed_path.contains(&term.text),
        }
    };
    let components = CodeSearchScoreComponentsV1 {
        boundary_and_path: witness.score,
        occurrence: u32::from(witness.occurrences.saturating_sub(1)).saturating_mul(2),
        exact_case: if case_exact { 5 } else { 0 },
        proximity: 0,
    };
    match scored {
        Some(prior) => {
            prior.components.boundary_and_path = prior
                .components
                .boundary_and_path
                .saturating_add(components.boundary_and_path);
            prior.components.occurrence = prior
                .components
                .occurrence
                .saturating_add(components.occurrence);
            prior.components.exact_case = prior
                .components
                .exact_case
                .saturating_add(components.exact_case);
            if witness.score > prior.primary.score
                || (witness.score == prior.primary.score && term_index < prior.primary_term)
            {
                prior.primary = witness;
                prior.primary_term = term_index;
            }
        }
        None => {
            *scored = Some(ScoredMatch {
                components,
                primary: witness,
                primary_term: term_index,
            });
        }
    }
}

fn score_terms(
    file: &SourceFile,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    scored: Option<ScoredMatch>,
    selection: TermsToScore,
    budget: &RequestBudgetV1,
) -> Result<Option<ScoredMatch>, CoreError> {
    score_terms_observed(file, terms, case, scored, selection, budget, None)
}

fn score_terms_observed(
    file: &SourceFile,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    mut scored: Option<ScoredMatch>,
    selection: TermsToScore,
    budget: &RequestBudgetV1,
    mut stats: Option<&mut CodeSearchExecutionStatsV1>,
) -> Result<Option<ScoredMatch>, CoreError> {
    for (index, term) in terms.iter().enumerate() {
        if (matches!(selection, TermsToScore::Literals) && term.regex.is_some())
            || (matches!(selection, TermsToScore::Regex) && term.regex.is_none())
        {
            continue;
        }
        if let Some(stats) = stats.as_deref_mut() {
            let surface_case = if term.regex.is_some() {
                CaseMode::Sensitive
            } else {
                case
            };
            stats.source_surface_bytes_considered = stats
                .source_surface_bytes_considered
                .saturating_add(checked_count_u64(
                    scanned_bytes(file, term.scope, surface_case),
                    "source surface bytes",
                )?);
        }
        let Some(witness) = choose_witness(file, term, case, budget)? else {
            return Ok(None);
        };
        add_witness_score(
            &mut scored,
            file.indexed_text.as_deref(),
            &file.indexed_path,
            term,
            case,
            index,
            witness,
        );
    }
    Ok(scored)
}

fn score_path_terms_observed(
    file: &SourceFileMetadata,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    mut scored: Option<ScoredMatch>,
    selection: TermsToScore,
    budget: &RequestBudgetV1,
    stats: &mut CodeSearchExecutionStatsV1,
) -> Result<Option<ScoredMatch>, CoreError> {
    for (index, term) in terms.iter().enumerate() {
        if !matches!(term.scope, Scope::Path) {
            return Err(CoreError::Storage(
                "lexical: path-only scorer received content scope".into(),
            ));
        }
        if (matches!(selection, TermsToScore::Literals) && term.regex.is_some())
            || (matches!(selection, TermsToScore::Regex) && term.regex.is_none())
        {
            continue;
        }
        let text = if term.regex.is_some() || case == CaseMode::Sensitive {
            file.indexed_path.as_str()
        } else {
            file.folded_path.as_str()
        };
        stats.source_surface_bytes_considered = stats
            .source_surface_bytes_considered
            .saturating_add(checked_count_u64(text.len(), "source surface bytes")?);
        let witness = match &term.regex {
            Some(regex) => best_regex_in(text, regex, HitSurface::Path, budget)?,
            None => best_in(text, &term.needle, HitSurface::Path, case, budget)?,
        };
        let Some(witness) = witness else {
            return Ok(None);
        };
        add_witness_score(
            &mut scored,
            None,
            &file.indexed_path,
            term,
            case,
            index,
            witness,
        );
    }
    Ok(scored)
}

fn finish_path_score(
    file: &SourceFileMetadata,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    scored: &mut ScoredMatch,
    budget: &RequestBudgetV1,
) -> Result<f32, CoreError> {
    let path = match case {
        CaseMode::Sensitive => file.indexed_path.as_str(),
        CaseMode::Folded => file.folded_path.as_str(),
    };
    scored.components.proximity = match min_cover_gap(path, terms, HitSurface::Path, budget)? {
        Some(gap) if gap < 32 => 32_u32.saturating_sub(u32::try_from(gap).map_err(|error| {
            CoreError::Storage(format!("lexical: path proximity gap overflow: {error}"))
        })?),
        _ => 0,
    };
    let total = u16::try_from(scored.components.total()).map_err(|error| {
        CoreError::Storage(format!("lexical: code search score overflow: {error}"))
    })?;
    Ok(f32::from(total))
}

fn finish_score(
    file: &SourceFile,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    scored: &mut ScoredMatch,
    budget: &RequestBudgetV1,
) -> Result<(f32, CodeSearchScoreComponentsV1), CoreError> {
    scored.components.proximity = proximity_bonus(file, terms, case, budget)?;
    let score = u16::try_from(scored.components.total()).map_err(|error| {
        CoreError::Storage(format!("lexical: code search score overflow: {error}"))
    })?;
    Ok((f32::from(score), scored.components))
}

fn boundary_score(text: &str, span: Range<usize>, path: bool) -> u32 {
    let before = text
        .get(..span.start)
        .and_then(|prefix| prefix.chars().next_back());
    let after = text
        .get(span.end..)
        .and_then(|suffix| suffix.chars().next());
    let left = before.is_none_or(|ch| !normalize::is_token_char(ch));
    let right = after.is_none_or(|ch| !normalize::is_token_char(ch));
    let base: u32 = if left && right {
        100
    } else if left {
        75
    } else {
        40
    };
    let basename = path && span.start >= text.rfind('/').map_or(0, |index| index.saturating_add(1));
    base.saturating_add(if basename {
        35
    } else if path {
        15
    } else {
        0
    })
}

/// Walk every literal witness, including matches that overlap a previous one.
///
/// Both strings are UTF-8, so a matched needle starts at a scalar boundary;
/// advancing by one scalar retains overlaps without slicing through UTF-8.
struct OverlappingMatches<'a> {
    text: &'a str,
    finder: memchr::memmem::Finder<'a>,
    from: usize,
}

impl<'a> OverlappingMatches<'a> {
    fn new(text: &'a str, needle: &'a str) -> Self {
        Self {
            text,
            finder: memchr::memmem::Finder::new(needle.as_bytes()),
            from: 0,
        }
    }
}

impl Iterator for OverlappingMatches<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.text.as_bytes().get(self.from..)?;
        let offset = self.finder.find(remaining)?;
        let start = self.from.checked_add(offset)?;
        let next_scalar = self.text.get(start..)?.chars().next()?;
        self.from = start.checked_add(next_scalar.len_utf8())?;
        Some(start)
    }
}

fn best_in(
    text: &str,
    needle: &str,
    surface: HitSurface,
    case: CaseMode,
    budget: &RequestBudgetV1,
) -> Result<Option<Witness>, CoreError> {
    // A finder may scan an admitted 8 MiB file without yielding when there
    // is no match. Check both sides of that bounded native scan.
    budget.checkpoint("lexical:code-search-occurrence-start")?;
    let mut best: Option<Witness> = None;
    let mut occurrences = 0_u8;
    for (index, start) in OverlappingMatches::new(text, needle).enumerate() {
        occurrences = occurrences.saturating_add(1).min(4);
        if index % 256 == 0 {
            budget.checkpoint("lexical:code-search-occurrence")?;
        }
        let end = start
            .checked_add(needle.len())
            .ok_or_else(|| CoreError::Storage("lexical: occurrence span overflow".into()))?;
        let span = start..end;
        let score = boundary_score(text, span.clone(), matches!(surface, HitSurface::Path));
        if best.as_ref().is_none_or(|prior| score > prior.score) {
            best = Some(Witness {
                surface,
                normalized: span,
                score,
                occurrences: 0,
                mapping_case: case,
            });
        }
    }
    budget.checkpoint("lexical:code-search-occurrence-end")?;
    if let Some(best) = &mut best {
        best.occurrences = occurrences;
    }
    Ok(best)
}

fn best_regex_in(
    text: &str,
    executor: &RegexExecutor,
    surface: HitSurface,
    budget: &RequestBudgetV1,
) -> Result<Option<Witness>, CoreError> {
    budget.checkpoint("lexical:code-search-regex-start")?;
    let matches = executor
        .find_ranges_bounded(
            text.as_bytes(),
            crate::file_authority::MAX_FILE_BYTES,
            MAX_REGEX_MATCHES_PER_SURFACE + 1,
            &|| budget.interruption().is_some(),
        )
        .map_err(|error| {
            budget
                .interrupted_at("lexical:code-search-regex")
                .unwrap_or_else(|| CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                    message: format!("lexical code search regex scan: {error}"),
                })
        })?;
    budget.checkpoint("lexical:code-search-regex-end")?;
    if !matches.exhausted || matches.ranges.len() > MAX_REGEX_MATCHES_PER_SURFACE {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
            message: "lexical code search regex match count exceeds per-file cap".into(),
        });
    }
    let occurrences = u8::try_from(matches.ranges.len().min(4)).map_err(|error| {
        CoreError::Storage(format!("lexical: bounded regex occurrence count: {error}"))
    })?;
    let mut best: Option<Witness> = None;
    for span in matches.ranges {
        if span.is_empty() || text.get(span.clone()).is_none() {
            return Err(unsupported(
                "regex match is zero-width or splits a UTF-8 scalar",
            ));
        }
        let score = boundary_score(text, span.clone(), matches!(surface, HitSurface::Path));
        if best.as_ref().is_none_or(|prior| score > prior.score) {
            best = Some(Witness {
                surface,
                normalized: span,
                score,
                occurrences,
                mapping_case: CaseMode::Sensitive,
            });
        }
    }
    Ok(best)
}

fn choose_witness(
    file: &SourceFile,
    term: &CodeSearchTerm,
    case: CaseMode,
    budget: &RequestBudgetV1,
) -> Result<Option<Witness>, CoreError> {
    let content = if term.regex.is_some() || case == CaseMode::Sensitive {
        file.indexed_text.as_deref()
    } else {
        file.folded_text.as_deref()
    };
    let path = if term.regex.is_some() || case == CaseMode::Sensitive {
        file.indexed_path.as_str()
    } else {
        file.folded_path.as_str()
    };
    let mut best: Option<Witness> = if matches!(term.scope, Scope::Both | Scope::Content)
        && let Some(text) = content
    {
        match &term.regex {
            Some(regex) => best_regex_in(text, regex, HitSurface::Content, budget)?,
            None => best_in(text, &term.needle, HitSurface::Content, case, budget)?,
        }
    } else {
        None
    };
    if matches!(term.scope, Scope::Both | Scope::Path)
        && let Some(path_match) = match &term.regex {
            Some(regex) => best_regex_in(path, regex, HitSurface::Path, budget)?,
            None => best_in(path, &term.needle, HitSurface::Path, case, budget)?,
        }
        && best
            .as_ref()
            .is_none_or(|prior| path_match.score > prior.score)
    {
        best = Some(path_match);
    }
    Ok(best)
}

#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "the first mismatch and equal-length branches prove the accessed byte ranges"
)]
fn typo_distance(needle: &[u8], token: &[u8], case: CaseMode) -> Option<u8> {
    let same = |left: u8, right: u8| match case {
        CaseMode::Sensitive => left == right,
        CaseMode::Folded => left.eq_ignore_ascii_case(&right),
    };
    let (short, long) = if needle.len() <= token.len() {
        (needle, token)
    } else {
        (token, needle)
    };
    if long.len().saturating_sub(short.len()) > 1 {
        return None;
    }
    let first = (0..short.len()).find(|&index| !same(short[index], long[index]));
    if needle.len() == token.len() {
        let Some(index) = first else { return Some(0) };
        if short[index + 1..]
            .iter()
            .zip(&long[index + 1..])
            .all(|(&left, &right)| same(left, right))
        {
            return Some(1);
        }
        if index + 1 < short.len()
            && same(short[index], long[index + 1])
            && same(short[index + 1], long[index])
            && short[index + 2..]
                .iter()
                .zip(&long[index + 2..])
                .all(|(&left, &right)| same(left, right))
        {
            return Some(1);
        }
        return None;
    }
    let index = first.unwrap_or(short.len());
    short[index..]
        .iter()
        .zip(&long[index + 1..])
        .all(|(&left, &right)| same(left, right))
        .then_some(1)
}

fn scan_typo_token_spans(
    chars: impl Iterator<Item = (usize, bool)>,
    text_len: usize,
    budget: &RequestBudgetV1,
    mut check_token: impl FnMut(Range<usize>) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let mut start = None;
    for (ordinal, (index, is_token)) in chars.enumerate() {
        if ordinal.is_multiple_of(16_384) {
            budget.checkpoint("lexical:code-search-typo-scan")?;
        }
        match (start, is_token) {
            (None, true) => start = Some(index),
            (Some(from), false) => {
                check_token(from..index)?;
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        check_token(from..text_len)?;
    }
    Ok(())
}

fn typo_text_is_ascii(text: &str, budget: &RequestBudgetV1) -> Result<bool, CoreError> {
    for chunk in text.as_bytes().chunks(16_384) {
        budget.checkpoint("lexical:code-search-typo-ascii-detect")?;
        if !chunk.is_ascii() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn typo_witness(
    text: &str,
    needle: &str,
    case: CaseMode,
    comparisons: &mut usize,
    distance_cache: &mut TypoDistanceCache,
    budget: &RequestBudgetV1,
) -> Result<Option<(Witness, u8)>, CoreError> {
    let mut best: Option<(Witness, u8)> = None;
    let mut check_token = |span: Range<usize>| -> Result<(), CoreError> {
        let token = text
            .get(span.clone())
            .ok_or_else(|| CoreError::Storage("lexical: typo token span invalid".into()))?;
        if !token.is_ascii()
            || !token
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
            || token.len().abs_diff(needle.len()) > 1
        {
            return Ok(());
        }
        let distance = if let Some(cached) = distance_cache.get(token.as_bytes()) {
            *cached
        } else {
            *comparisons = comparisons.saturating_add(1);
            if *comparisons > MAX_TYPO_TOKEN_COMPARISONS {
                return Err(CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                    message: "lexical code search: typo token comparison budget exceeded".into(),
                });
            }
            if (*comparisons).is_multiple_of(256) {
                budget.checkpoint("lexical:code-search-typo-token")?;
            }
            let distance = typo_distance(needle.as_bytes(), token.as_bytes(), case);
            let _previous = distance_cache.insert(token.as_bytes().to_vec(), distance);
            distance
        };
        if let Some(distance) = distance {
            match &mut best {
                Some((witness, prior)) if distance == *prior => {
                    witness.occurrences = witness.occurrences.saturating_add(1).min(4);
                }
                Some((_, prior)) if distance > *prior => {}
                _ => {
                    best = Some((
                        Witness {
                            surface: HitSurface::Content,
                            normalized: span,
                            score: 100,
                            occurrences: 1,
                            mapping_case: CaseMode::Sensitive,
                        },
                        distance,
                    ));
                }
            }
        }
        Ok(())
    };
    budget.checkpoint("lexical:code-search-typo-file-start")?;
    if typo_text_is_ascii(text, budget)? {
        // ASCII byte offsets are scalar ordinals, and the token predicate is
        // exactly ASCII alphanumeric or underscore. A non-ASCII scalar may
        // join an ASCII run, so mixed text retains the Unicode tokenizer.
        scan_typo_token_spans(
            text.bytes()
                .enumerate()
                .map(|(index, byte)| (index, byte.is_ascii_alphanumeric() || byte == b'_')),
            text.len(),
            budget,
            &mut check_token,
        )?;
    } else {
        scan_typo_token_spans(
            text.char_indices()
                .map(|(index, ch)| (index, normalize::is_token_char(ch))),
            text.len(),
            budget,
            &mut check_token,
        )?;
    }
    budget.checkpoint("lexical:code-search-typo-file-end")?;
    Ok(best)
}

/// Conservatively shortlist files with shared trigrams.
///
/// One OSA edit disturbs at most four distinct query byte trigrams for ASCII
/// identifiers. File postings are a superset; source tokens remain the truth.
#[cfg(test)]
fn typo_candidates(
    index: &TrigramIndex,
    identifier: &str,
    eligible: Option<&BTreeSet<u64>>,
    max_posting_visits: usize,
    budget: &RequestBudgetV1,
) -> Result<Option<BTreeSet<u64>>, CoreError> {
    typo_candidates_observed(
        identifier,
        eligible,
        max_posting_visits,
        budget,
        None,
        |grams| {
            Ok(grams
                .iter()
                .map(|gram| index.lookup(*gram).iter().map(|id| id.0).collect())
                .collect())
        },
    )
}

fn typo_candidates_observed<F>(
    identifier: &str,
    eligible: Option<&BTreeSet<u64>>,
    max_posting_visits: usize,
    budget: &RequestBudgetV1,
    mut stats: Option<&mut CodeSearchExecutionStatsV1>,
    mut load_lists: F,
) -> Result<Option<BTreeSet<u64>>, CoreError>
where
    F: FnMut(&[Trigram]) -> Result<Vec<Vec<u64>>, CoreError>,
{
    let mut grams: Vec<Trigram> = trigrams_of(identifier.to_ascii_lowercase().as_bytes()).collect();
    grams.sort_unstable();
    grams.dedup();
    let Some(threshold) = grams.len().checked_sub(4).filter(|count| *count > 0) else {
        return Ok(None);
    };
    let lists = load_lists(&grams)?;
    if lists.len() != grams.len() {
        return Err(CoreError::Storage(
            "lexical: typo posting list count differs from grams".into(),
        ));
    }
    if let Some(eligible) = eligible {
        let probes = eligible.len().saturating_mul(lists.len());
        if probes > max_posting_visits {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                message: "lexical code search: typo eligible posting probes exceeded".into(),
            });
        }
        if let Some(stats) = stats.as_mut() {
            stats.posting_probes = stats
                .posting_probes
                .saturating_add(checked_count_u64(probes, "typo posting probes")?);
        }
        let mut candidates = BTreeSet::new();
        for (ordinal, &id) in eligible.iter().enumerate() {
            if ordinal.is_multiple_of(256) {
                budget.checkpoint("lexical:code-search-typo-eligible-postings")?;
            }
            let count = lists
                .iter()
                .filter(|list| list.binary_search(&id).is_ok())
                .count();
            if count >= threshold {
                if candidates.len() >= MAX_CANDIDATE_PRE_VERIFY {
                    return Err(CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                        message: "lexical code search: typo candidate set budget exceeded".into(),
                    });
                }
                let _inserted = candidates.insert(id);
            }
        }
        budget.checkpoint("lexical:code-search-typo-eligible-postings-end")?;
        return Ok(Some(candidates));
    }
    let total_visits = lists
        .iter()
        .fold(0_usize, |sum, list| sum.saturating_add(list.len()));
    if total_visits > max_posting_visits {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
            message: "lexical code search: typo posting walk budget exceeded".into(),
        });
    }
    if let Some(stats) = stats.as_mut() {
        stats.posting_probes = stats
            .posting_probes
            .saturating_add(checked_count_u64(total_visits, "typo posting visits")?);
    }
    let mut heap = BinaryHeap::new();
    for (list_index, list) in lists.iter().enumerate() {
        if let Some(&id) = list.first() {
            heap.push(Reverse((id, list_index, 0_usize)));
        }
    }
    let mut candidates = BTreeSet::new();
    let mut visited = 0_usize;
    while let Some(Reverse((id, list_index, offset))) = heap.pop() {
        if visited.is_multiple_of(256) {
            budget.checkpoint("lexical:code-search-typo-postings")?;
        }
        visited = visited.saturating_add(1);
        let next_offset = offset.saturating_add(1);
        if let Some(&next) = lists.get(list_index).and_then(|list| list.get(next_offset)) {
            heap.push(Reverse((next, list_index, next_offset)));
        }
        let mut count = 1_usize;
        while heap.peek().is_some_and(|Reverse((next, _, _))| *next == id) {
            let Some(Reverse((_, next_list, next_offset))) = heap.pop() else {
                break;
            };
            visited = visited.saturating_add(1);
            let following_offset = next_offset.saturating_add(1);
            if let Some(&next) = lists
                .get(next_list)
                .and_then(|list| list.get(following_offset))
            {
                heap.push(Reverse((next, next_list, following_offset)));
            }
            count = count.saturating_add(1);
        }
        if count >= threshold {
            if candidates.len() >= MAX_CANDIDATE_PRE_VERIFY {
                return Err(CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                    message: "lexical code search: typo candidate set budget exceeded".into(),
                });
            }
            let _inserted = candidates.insert(id);
        }
    }
    budget.checkpoint("lexical:code-search-typo-postings-end")?;
    Ok(Some(candidates))
}

/// Smallest byte gap covering each term on one surface.
///
/// A cursor per term is sufficient: advancing the leftmost current
/// occurrence visits the minimum covering windows without materializing all
/// occurrences. Mixed path/content scoped terms have no shared proximity.
fn min_cover_gap(
    text: &str,
    terms: &[CodeSearchTerm],
    surface: HitSurface,
    budget: &RequestBudgetV1,
) -> Result<Option<usize>, CoreError> {
    if terms.len() < 2
        || terms.iter().any(|term| term.regex.is_some())
        || terms.iter().any(|term| {
            matches!(
                (surface, term.scope),
                (HitSurface::Content, Scope::Path) | (HitSurface::Path, Scope::Content)
            )
        })
    {
        return Ok(None);
    }
    budget.checkpoint("lexical:code-search-proximity-start")?;
    let mut streams: Vec<_> = terms
        .iter()
        .map(|term| OverlappingMatches::new(text, term.needle.as_str()))
        .collect();
    let mut current = Vec::with_capacity(streams.len());
    for (stream, term) in streams.iter_mut().zip(terms) {
        budget.checkpoint("lexical:code-search-proximity-seed")?;
        let Some(start) = stream.next() else {
            budget.checkpoint("lexical:code-search-proximity-absent")?;
            return Ok(None);
        };
        let end = start
            .checked_add(term.needle.len())
            .ok_or_else(|| CoreError::Storage("lexical: proximity span overflow".into()))?;
        current.push((start, end));
    }
    let term_bytes: usize = terms.iter().map(|term| term.needle.len()).sum();
    let mut smallest = usize::MAX;
    let mut steps = 0_usize;
    loop {
        if steps.is_multiple_of(256) {
            budget.checkpoint("lexical:code-search-proximity-walk")?;
        }
        steps = steps.saturating_add(1);
        let (leftmost, &(start, _)) = current
            .iter()
            .enumerate()
            .min_by_key(|(_, (start, _))| *start)
            .ok_or_else(|| CoreError::Storage("lexical: empty proximity window".into()))?;
        let end = current
            .iter()
            .map(|(_, end)| *end)
            .max()
            .ok_or_else(|| CoreError::Storage("lexical: empty proximity window".into()))?;
        smallest = smallest.min(end.saturating_sub(start).saturating_sub(term_bytes));
        if smallest == 0 {
            break;
        }
        let stream = streams.get_mut(leftmost).ok_or_else(|| {
            CoreError::Storage("lexical: proximity stream outside term set".into())
        })?;
        let Some(next_start) = stream.next() else {
            break;
        };
        let term = terms
            .get(leftmost)
            .ok_or_else(|| CoreError::Storage("lexical: proximity term outside term set".into()))?;
        let next_end = next_start
            .checked_add(term.needle.len())
            .ok_or_else(|| CoreError::Storage("lexical: proximity span overflow".into()))?;
        let slot = current.get_mut(leftmost).ok_or_else(|| {
            CoreError::Storage("lexical: proximity window outside term set".into())
        })?;
        *slot = (next_start, next_end);
    }
    budget.checkpoint("lexical:code-search-proximity-end")?;
    Ok(Some(smallest))
}

fn proximity_bonus(
    file: &SourceFile,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    budget: &RequestBudgetV1,
) -> Result<u32, CoreError> {
    let content = match case {
        CaseMode::Sensitive => file.indexed_text.as_deref(),
        CaseMode::Folded => file.folded_text.as_deref(),
    };
    let path = match case {
        CaseMode::Sensitive => file.indexed_path.as_str(),
        CaseMode::Folded => file.folded_path.as_str(),
    };
    let mut best = if let Some(text) = content {
        min_cover_gap(text, terms, HitSurface::Content, budget)?
    } else {
        None
    };
    if let Some(path_gap) = min_cover_gap(path, terms, HitSurface::Path, budget)? {
        best = Some(best.map_or(path_gap, |gap: usize| gap.min(path_gap)));
    }
    let Some(gap) = best else {
        return Ok(0);
    };
    if gap >= 32 {
        return Ok(0);
    }
    let gap = u32::try_from(gap)
        .map_err(|error| CoreError::Storage(format!("lexical: proximity gap overflow: {error}")))?;
    Ok(32_u32.saturating_sub(gap))
}

/// Generate one file-level AND set. A broad individual posting is never
/// charged as a completed candidate before the remaining terms filter it.
#[cfg(test)]
fn candidate_ids(
    authority: &FileAuthority,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    eligible: Option<&BTreeSet<u64>>,
    budget: &RequestBudgetV1,
) -> Result<BTreeMap<u64, ScoredMatch>, CoreError> {
    let mut work = QueryWork::default();
    candidate_ids_observed(
        authority,
        terms,
        case,
        eligible,
        budget,
        &mut work,
        &mut CodeSearchExecutionStatsV1::default(),
        MAX_CANDIDATE_PRE_VERIFY,
    )
}

#[cfg(test)]
fn candidate_ids_observed(
    authority: &FileAuthority,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    eligible: Option<&BTreeSet<u64>>,
    budget: &RequestBudgetV1,
    work: &mut QueryWork,
    stats: &mut CodeSearchExecutionStatsV1,
    candidate_cap: usize,
) -> Result<BTreeMap<u64, ScoredMatch>, CoreError> {
    let mut hits = BTreeMap::new();
    for_each_candidate_observed(
        authority,
        terms,
        case,
        eligible,
        budget,
        work,
        stats,
        candidate_cap,
        |id, scored, _file, _stats| {
            let _previous = hits.insert(id, scored);
            Ok(())
        },
    )?;
    Ok(hits)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one literal source-verification pass with caller-owned exact-match consumption"
)]
fn for_each_candidate_observed<F>(
    authority: &FileAuthority,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    eligible: Option<&BTreeSet<u64>>,
    budget: &RequestBudgetV1,
    work: &mut QueryWork,
    stats: &mut CodeSearchExecutionStatsV1,
    candidate_cap: usize,
    mut on_match: F,
) -> Result<(), CoreError>
where
    F: FnMut(
        u64,
        ScoredMatch,
        Option<&SourceFile>,
        &mut CodeSearchExecutionStatsV1,
    ) -> Result<(), CoreError>,
{
    let literals: Vec<_> = terms.iter().filter(|term| term.regex.is_none()).collect();
    let mut prepared = Vec::new();
    for term in &literals {
        // Per-character fold preserves every sensitive substring. The
        // original NFC text remains the final matching authority.
        let needle = match case {
            CaseMode::Sensitive => normalize::fold(&term.needle),
            CaseMode::Folded => term.needle.clone(),
        };
        if needle.len() < 3 {
            continue;
        }
        let mut grams: Vec<_> = trigrams_of(needle.as_bytes()).collect();
        grams.sort_unstable();
        grams.dedup();
        prepared.push((*term, grams));
    }
    let mut verified_count = 0_usize;
    if !prepared.is_empty() {
        let mut possible: Option<Vec<u64>> = None;
        for (term, grams) in prepared {
            budget.checkpoint("lexical:code-search-literal-postings")?;
            let posting_before = work.posting_memberships();
            let mut conjuncts = Vec::new();
            conjuncts.try_reserve(grams.len()).map_err(|error| {
                CoreError::Storage(format!("lexical: reserve literal gram plan: {error}"))
            })?;
            conjuncts.extend(grams.into_iter().map(NecessaryGramExpr::Gram));
            let expression = NecessaryGramExpr::And(conjuncts);
            let content = if matches!(term.scope, Scope::Both | Scope::Content) {
                authority.necessary_candidates(
                    crate::file_authority::PostingSurface::Content,
                    &expression,
                    work,
                    budget,
                )?
            } else {
                Some(Vec::new())
            };
            let path = if matches!(term.scope, Scope::Both | Scope::Path) {
                authority.necessary_candidates(
                    crate::file_authority::PostingSurface::Path,
                    &expression,
                    work,
                    budget,
                )?
            } else {
                Some(Vec::new())
            };
            let (Some(content), Some(path)) = (content, path) else {
                return Err(CoreError::Storage(
                    "lexical: nonempty literal gram conjunction became universal".into(),
                ));
            };
            let posting_visits = work
                .posting_memberships()
                .checked_sub(posting_before)
                .ok_or_else(|| CoreError::Storage("lexical: posting work regressed".into()))?;
            stats.posting_probes = stats.posting_probes.saturating_add(posting_visits);
            let term_possible = union_sorted_ids(content, path, budget)?;
            possible = Some(match possible {
                None => term_possible,
                Some(prior) => intersect_sorted_ids(&prior, &term_possible, budget)?,
            });
            if possible.as_ref().is_some_and(Vec::is_empty) {
                break;
            }
        }
        for (visits, id) in possible.into_iter().flatten().enumerate() {
            if visits.is_multiple_of(256) {
                budget.checkpoint("lexical:code-search-trigram-posting")?;
            }
            if eligible.is_some_and(|ids| !ids.contains(&id)) {
                continue;
            }
            stats.literal_source_verification_attempts = stats
                .literal_source_verification_attempts
                .checked_add(1)
                .ok_or_else(|| {
                    CoreError::Storage("lexical: verification work count overflow".into())
                })?;
            let path_only = terms.iter().all(|term| matches!(term.scope, Scope::Path));
            let mut full = None;
            let scored = if path_only {
                score_path_terms_observed(
                    &metadata_for_id(authority, id, budget)?,
                    terms,
                    case,
                    None,
                    TermsToScore::Literals,
                    budget,
                    stats,
                )?
            } else {
                full = Some(authority.load_file_for_id(id, budget)?);
                let file = full.as_ref().ok_or_else(|| {
                    CoreError::Storage("lexical: literal source load disappeared".into())
                })?;
                score_terms_observed(
                    file,
                    terms,
                    case,
                    None,
                    TermsToScore::Literals,
                    budget,
                    Some(stats),
                )?
            };
            let Some(scored) = scored else {
                continue;
            };
            if verified_count >= candidate_cap {
                return Err(CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                    message: "lexical code search: verified file candidate set exceeds cap".into(),
                });
            }
            verified_count = verified_count.saturating_add(1);
            on_match(id, scored, full.as_ref(), stats)?;
        }
        return Ok(());
    }

    let scope = scope_union(literals.iter().map(|term| term.scope))
        .ok_or_else(|| CoreError::Storage("lexical: no literal candidate seed".into()))?;
    let source_bytes = source_bytes_checked(authority, scope, case, eligible, budget)?;
    let all_files = usize::try_from(authority.len())
        .map_err(|error| CoreError::Storage(format!("lexical: file count width: {error}")))?;
    if eligible.map_or(all_files, BTreeSet::len) > MAX_SHORT_SCAN_FILES
        || source_bytes > MAX_SHORT_SCAN_SOURCE_BYTES
    {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
            message: format!(
                "lexical code search: short literal exceeds {MAX_SHORT_SCAN_FILES} files or {MAX_SHORT_SCAN_SOURCE_BYTES} source bytes"
            ),
        });
    }
    authority.scan_sources(budget, &mut |metadata| {
        let id = metadata.logical_id;
        if eligible.is_some_and(|ids| !ids.contains(&id)) {
            return Ok(());
        }
        stats.literal_source_verification_attempts = stats
            .literal_source_verification_attempts
            .checked_add(1)
            .ok_or_else(|| {
                CoreError::Storage("lexical: verification work count overflow".into())
            })?;
        let mut full = None;
        let scored = if terms.iter().all(|term| matches!(term.scope, Scope::Path)) {
            score_path_terms_observed(
                &metadata,
                terms,
                case,
                None,
                TermsToScore::Literals,
                budget,
                stats,
            )?
        } else {
            full = Some(authority.load_file_for_key(&metadata.source.file, budget)?);
            let file = full.as_ref().ok_or_else(|| {
                CoreError::Storage("lexical: short literal source load disappeared".into())
            })?;
            score_terms_observed(
                file,
                terms,
                case,
                None,
                TermsToScore::Literals,
                budget,
                Some(stats),
            )?
        };
        if let Some(scored) = scored {
            if verified_count >= candidate_cap {
                return Err(CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                    message: "lexical code search: verified file candidate set exceeds cap".into(),
                });
            }
            verified_count = verified_count.saturating_add(1);
            on_match(id, scored, full.as_ref(), stats)?;
        }
        Ok(())
    })?;
    Ok(())
}
fn source_bytes_checked(
    authority: &FileAuthority,
    scope: Scope,
    case: CaseMode,
    eligible: Option<&BTreeSet<u64>>,
    budget: &RequestBudgetV1,
) -> Result<usize, CoreError> {
    let mut total = 0_usize;
    authority.scan_sources(budget, &mut |file| {
        if eligible.is_none_or(|ids| ids.contains(&file.logical_id)) {
            total = total.saturating_add(metadata_scanned_bytes(&file, scope, case));
        }
        Ok(())
    })?;
    budget.checkpoint("lexical:code-search-source-size-end")?;
    Ok(total)
}

fn scanned_bytes(file: &SourceFile, scope: Scope, case: CaseMode) -> usize {
    let content = match case {
        CaseMode::Sensitive => file.indexed_text.as_ref(),
        CaseMode::Folded => file.folded_text.as_ref(),
    }
    .map_or(0, |text| text.len());
    let path = match case {
        CaseMode::Sensitive => file.indexed_path.len(),
        CaseMode::Folded => file.folded_path.len(),
    };
    match scope {
        Scope::Both => content.saturating_add(path),
        Scope::Content => content,
        Scope::Path => path,
    }
}

fn metadata_scanned_bytes(file: &SourceFileMetadata, scope: Scope, case: CaseMode) -> usize {
    let content = match case {
        CaseMode::Sensitive => file.indexed_text_bytes,
        CaseMode::Folded => file.folded_text_bytes,
    };
    let path = match case {
        CaseMode::Sensitive => file.indexed_path.len(),
        CaseMode::Folded => file.folded_path.len(),
    };
    match scope {
        Scope::Both => content.saturating_add(path),
        Scope::Content => content,
        Scope::Path => path,
    }
}

fn scope_union(scopes: impl Iterator<Item = Scope>) -> Option<Scope> {
    let mut content = false;
    let mut path = false;
    for scope in scopes {
        match scope {
            Scope::Both => {
                content = true;
                path = true;
            }
            Scope::Content => content = true,
            Scope::Path => path = true,
        }
    }
    match (content, path) {
        (false, false) => None,
        (true, false) => Some(Scope::Content),
        (false, true) => Some(Scope::Path),
        (true, true) => Some(Scope::Both),
    }
}

fn regex_scan_scope(terms: &[CodeSearchTerm]) -> Option<Scope> {
    scope_union(
        terms
            .iter()
            .filter(|term| term.regex.is_some())
            .map(|term| term.scope),
    )
}

/// Adapt a raw regex condition to F15's folded posting bytes.
///
/// ASCII bytes within a raw match survive the per-scalar fold as lowercase
/// bytes. A raw gram containing non-ASCII bytes has no such byte-level
/// guarantee, so it contributes no filter. Preserve AND/OR structure so the
/// weakening remains sound for optional branches and alternations.
fn folded_regex_grams(expr: &NecessaryGramExpr) -> NecessaryGramExpr {
    match expr {
        NecessaryGramExpr::Gram(gram) if gram.is_ascii() => {
            NecessaryGramExpr::Gram(gram.map(|byte| byte.to_ascii_lowercase()))
        }
        NecessaryGramExpr::AllDocs | NecessaryGramExpr::Gram(_) => NecessaryGramExpr::AllDocs,
        NecessaryGramExpr::And(parts) => {
            NecessaryGramExpr::And(parts.iter().map(folded_regex_grams).collect())
        }
        NecessaryGramExpr::Or(parts) => {
            NecessaryGramExpr::Or(parts.iter().map(folded_regex_grams).collect())
        }
    }
}

fn union_sorted_ids(
    left: Vec<u64>,
    right: Vec<u64>,
    budget: &RequestBudgetV1,
) -> Result<Vec<u64>, CoreError> {
    if left.is_empty() {
        return Ok(right);
    }
    if right.is_empty() {
        return Ok(left);
    }
    let capacity = left
        .len()
        .checked_add(right.len())
        .ok_or_else(|| CoreError::Storage("lexical: regex candidate union size overflow".into()))?;
    let mut merged = Vec::new();
    merged.try_reserve_exact(capacity).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: regex candidate union allocation failed: {error}"
        ))
    })?;
    let (mut a, mut b) = (0_usize, 0_usize);
    while a < left.len() || b < right.len() {
        if merged.len().is_multiple_of(256) {
            budget.checkpoint("lexical:code-search-regex-posting-union")?;
        }
        let next = match (left.get(a), right.get(b)) {
            (Some(&x), Some(&y)) if x <= y => {
                a = a.saturating_add(1);
                x
            }
            (_, Some(&y)) => {
                b = b.saturating_add(1);
                y
            }
            (Some(&x), None) => {
                a = a.saturating_add(1);
                x
            }
            (None, None) => break,
        };
        if merged.last() != Some(&next) {
            merged.push(next);
        }
    }
    budget.checkpoint("lexical:code-search-regex-posting-union-end")?;
    Ok(merged)
}

fn intersect_sorted_ids(
    left: &[u64],
    right: &[u64],
    budget: &RequestBudgetV1,
) -> Result<Vec<u64>, CoreError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(left.len().min(right.len()))
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: regex candidate intersection allocation failed: {error}"
            ))
        })?;
    let (mut a, mut b) = (0_usize, 0_usize);
    while let (Some(&x), Some(&y)) = (left.get(a), right.get(b)) {
        if (a.saturating_add(b)).is_multiple_of(256) {
            budget.checkpoint("lexical:code-search-regex-posting-intersection")?;
        }
        match x.cmp(&y) {
            std::cmp::Ordering::Less => a = a.saturating_add(1),
            std::cmp::Ordering::Greater => b = b.saturating_add(1),
            std::cmp::Ordering::Equal => {
                result.push(x);
                a = a.saturating_add(1);
                b = b.saturating_add(1);
            }
        }
    }
    budget.checkpoint("lexical:code-search-regex-posting-intersection-end")?;
    Ok(result)
}

fn regex_candidate_ids(
    authority: &FileAuthority,
    terms: &[CodeSearchTerm],
    work: &mut QueryWork,
    budget: &RequestBudgetV1,
) -> Result<Option<Vec<u64>>, CoreError> {
    let mut joined: Option<Vec<u64>> = None;
    for term in terms.iter().filter(|term| term.regex.is_some()) {
        budget.checkpoint("lexical:code-search-regex-postings")?;
        let expr = term.necessary_grams.as_ref().ok_or_else(|| {
            CoreError::Storage("lexical: compiled regex lacks necessary gram plan".into())
        })?;
        let folded = folded_regex_grams(expr);
        let content = if matches!(term.scope, Scope::Both | Scope::Content) {
            authority.necessary_candidates(
                crate::file_authority::PostingSurface::Content,
                &folded,
                work,
                budget,
            )?
        } else {
            Some(Vec::new())
        };
        let path = if matches!(term.scope, Scope::Both | Scope::Path) {
            authority.necessary_candidates(
                crate::file_authority::PostingSurface::Path,
                &folded,
                work,
                budget,
            )?
        } else {
            Some(Vec::new())
        };
        let term_ids = match (content, path) {
            (None, _) if matches!(term.scope, Scope::Both | Scope::Content) => None,
            (_, None) if matches!(term.scope, Scope::Both | Scope::Path) => None,
            (Some(left), Some(right)) => Some(union_sorted_ids(left, right, budget)?),
            _ => None,
        };
        if let Some(term_ids) = term_ids {
            joined = Some(match joined {
                None => term_ids,
                Some(current) => intersect_sorted_ids(&current, &term_ids, budget)?,
            });
        }
    }
    Ok(joined)
}

fn language_eligible_ids(
    authority: &FileAuthority,
    constraints: &QueryConstraintSetV1,
    budget: &RequestBudgetV1,
) -> Result<Option<BTreeSet<u64>>, CoreError> {
    if constraints.language_any_of.is_empty() {
        return Ok(None);
    }
    let mut ids = BTreeSet::new();
    authority.scan_sources(budget, &mut |file| {
        if constraints.language_any_of.contains(&file.language) {
            let _inserted = ids.insert(file.logical_id);
        }
        Ok(())
    })?;
    budget.checkpoint("lexical:code-search-language-filter-end")?;
    Ok(Some(ids))
}

fn admit_regex_scan(
    authority: &FileAuthority,
    ids: impl Iterator<Item = u64>,
    scope: Scope,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    let mut bytes = 0_usize;
    for (position, id) in ids.enumerate() {
        if position % 256 == 0 {
            budget.checkpoint("lexical:code-search-regex-admission")?;
        }
        let file = metadata_for_id(authority, id, budget)?;
        bytes = bytes.saturating_add(metadata_scanned_bytes(&file, scope, CaseMode::Sensitive));
        if position >= MAX_REGEX_SCAN_FILES || bytes > MAX_REGEX_SCAN_SOURCE_BYTES {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                message: "lexical code search regex requires a bounded file scan".into(),
            });
        }
    }
    budget.checkpoint("lexical:code-search-regex-admission-end")?;
    Ok(())
}

pub(crate) fn file_candidate_id(repo: &str, path: &str) -> Result<String, CoreError> {
    let mut hash = Sha256::new();
    hash.update(b"quanta-index:code-search-file:v1\0");
    for value in [repo, path] {
        hash.update(byte_offset(value.len())?.to_le_bytes());
        hash.update(value.as_bytes());
    }
    let digest: [u8; 32] = hash.finalize().into();
    let mut id = String::from("file:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(id, "{byte:02x}").map_err(|error| {
            CoreError::Storage(format!("lexical: candidate id formatting: {error}"))
        })?;
    }
    Ok(id)
}

fn path_highlight(
    raw: &str,
    indexed_path: &str,
    witness: &Witness,
    budget: &RequestBudgetV1,
) -> Result<Option<HighlightSpan>, CoreError> {
    budget.checkpoint("lexical:code-search-path-preview")?;
    let mapped = MappedText::new(
        raw,
        indexed_path,
        witness.mapping_case,
        raw.len().saturating_mul(4).max(1),
        raw.len().saturating_mul(8).max(4),
        &|| budget.interruption().is_some(),
    );
    let mapped = match mapped {
        Ok(mapped) => mapped,
        Err(
            MappingError::ByteLimit | MappingError::EntryLimit | MappingError::AllocationRefused,
        ) => {
            budget.checkpoint("lexical:code-search-path-preview-limit")?;
            return Ok(None);
        }
        Err(MappingError::Interrupted) => {
            return Err(budget
                .interrupted_at("lexical:code-search-path-preview-map")
                .unwrap_or_else(|| {
                    CoreError::Storage("lexical: unobserved path preview interruption".into())
                }));
        }
        Err(MappingError::SourceMismatch | MappingError::InvalidSpan) => {
            return Err(CoreError::Storage(
                "lexical: path preview source/NFC provenance mismatch".into(),
            ));
        }
    };
    let original = mapped
        .source_range(witness.normalized.clone())
        .map_err(|error| {
            CoreError::Storage(format!("lexical: path preview focus mapping: {error}"))
        })?;
    if raw.get(original.clone()).is_none() {
        return Err(CoreError::Storage(
            "lexical: path preview focus is not UTF-8 aligned".into(),
        ));
    }
    let start = u32::try_from(original.start)
        .map_err(|error| CoreError::Storage(format!("lexical: path highlight offset: {error}")))?;
    let len =
        u32::try_from(original.end.checked_sub(original.start).ok_or_else(|| {
            CoreError::Storage("lexical: path highlight range is reversed".into())
        })?)
        .map_err(|error| CoreError::Storage(format!("lexical: path highlight length: {error}")))?;
    budget.checkpoint("lexical:code-search-path-preview-end")?;
    Ok(Some(HighlightSpan { start, len }))
}

// NFC source bytes already have identity provenance. Only case-fold expansion
// can move a focus offset. Non-NFC source keeps the full provenance mapper.
fn nfc_identity_focus(
    raw: &str,
    indexed: &str,
    folded: &str,
    witness: &Witness,
    budget: &RequestBudgetV1,
) -> Result<Option<Range<usize>>, CoreError> {
    nfc_identity_range(
        raw,
        indexed,
        folded,
        &witness.normalized,
        witness.mapping_case,
        budget,
    )
}

// Shared by previews and rank features. NFC identity permits a bounded scalar
// walk for folding offsets without allocating a whole-source provenance map.
fn nfc_identity_range(
    raw: &str,
    indexed: &str,
    folded: &str,
    normalized: &Range<usize>,
    mapping_case: CaseMode,
    budget: &RequestBudgetV1,
) -> Result<Option<Range<usize>>, CoreError> {
    if raw != indexed {
        return Ok(None);
    }
    let surface = match mapping_case {
        CaseMode::Sensitive => indexed,
        CaseMode::Folded => folded,
    };
    if normalized.is_empty() || surface.get(normalized.clone()).is_none() {
        return Err(CoreError::Storage(
            "lexical: NFC file focus is not UTF-8 aligned".into(),
        ));
    }
    if mapping_case == CaseMode::Sensitive || raw.is_ascii() {
        return Ok(Some(normalized.clone()));
    }
    budget.checkpoint("lexical:code-search-preview-nfc-focus")?;
    let mut folded_offset = 0_usize;
    let mut original_start = None;
    let mut original_end = 0_usize;
    let mut next_checkpoint = 4096_usize;
    for (start, scalar) in raw.char_indices() {
        if start >= next_checkpoint {
            budget.checkpoint("lexical:code-search-preview-nfc-walk")?;
            next_checkpoint = start.saturating_add(4096);
        }
        // ASCII folding preserves byte length. For other scalars, ask the
        // normalizer that built the immutable folded authority for its length.
        let folded_len = if scalar.is_ascii() {
            1
        } else {
            let mut buffer = [0_u8; 4];
            normalize::apply_case(scalar.encode_utf8(&mut buffer), CaseMode::Folded).len()
        };
        let end = folded_offset
            .checked_add(folded_len)
            .ok_or_else(|| CoreError::Storage("lexical: folded preview offset overflow".into()))?;
        if end > normalized.start && folded_offset < normalized.end {
            if original_start.is_none() {
                original_start = Some(start);
            }
            original_end = start.saturating_add(scalar.len_utf8());
        }
        folded_offset = end;
        if folded_offset >= normalized.end {
            break;
        }
    }
    budget.checkpoint("lexical:code-search-preview-nfc-focus-end")?;
    let start = original_start
        .ok_or_else(|| CoreError::Storage("lexical: folded preview focus outside source".into()))?;
    let original = start..original_end;
    Ok(Some(original))
}

fn content_witness_lines(
    file: &SourceFile,
    witness: &Witness,
    budget: &RequestBudgetV1,
) -> Result<(u32, u32), CoreError> {
    let text = match witness.mapping_case {
        CaseMode::Sensitive => file.indexed_text.as_deref(),
        CaseMode::Folded => file.folded_text.as_deref(),
    }
    .ok_or_else(|| CoreError::Storage("lexical: content witness lacks indexed text".into()))?;
    let start = witness.normalized.start;
    let end = witness.normalized.end;
    let bytes = text.as_bytes();
    if start >= end || end > bytes.len() {
        return Err(CoreError::Storage(
            "lexical: content witness is outside indexed text".into(),
        ));
    }
    // NFC and case folding preserve line separators, including when their
    // byte offsets differ from source. The preview mapping owns byte spans.
    let mut line = 1_u32;
    let mut start_line = None;
    let mut end_line = None;
    let last = end
        .checked_sub(1)
        .ok_or_else(|| CoreError::Storage("lexical: empty content witness".into()))?;
    let prefix = bytes
        .get(..end)
        .ok_or_else(|| CoreError::Storage("lexical: content witness outside source".into()))?;
    let mut position = 0_usize;
    for chunk in prefix.chunks(64 * 1024) {
        budget.checkpoint("lexical:code-search-line-position")?;
        for byte in chunk {
            if position == start {
                start_line = Some(line);
            }
            if position == last {
                end_line = Some(line);
                break;
            }
            if *byte == b'\n' {
                line = line.checked_add(1).ok_or_else(|| {
                    CoreError::Storage("lexical: source line count overflow".into())
                })?;
            }
            position = position
                .checked_add(1)
                .ok_or_else(|| CoreError::Storage("lexical: line offset overflow".into()))?;
        }
    }
    Ok((
        start_line.ok_or_else(|| CoreError::Storage("lexical: missing start line".into()))?,
        end_line.ok_or_else(|| CoreError::Storage("lexical: missing end line".into()))?,
    ))
}

fn base_file_candidate(
    owner: &TantivySearcher,
    source: &quanta_index_contract::SourceFileRevision,
    score: f32,
) -> Result<LexicalCandidate, CoreError> {
    Ok(LexicalCandidate {
        source_repo_id: source.file.source_repo_id.clone(),
        source: Some(source.clone()),
        preview: None,
        candidate_id: file_candidate_id(
            source.file.source_repo_id.as_str(),
            source.file.repo_relative_path.as_str(),
        )?,
        repo_id: owner.repo_id.clone(),
        revision_id: owner.revision_id.clone(),
        manifest_generation: owner.generation,
        repo_relative_path: source.file.repo_relative_path.clone(),
        start_line: 0,
        end_line: 0,
        score,
        snippet: String::new(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    })
}

fn set_path_preview(
    candidate: &mut LexicalCandidate,
    source: &quanta_index_contract::SourceFileRevision,
    indexed_path: &str,
    witness: &Witness,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    let Some(highlight) = path_highlight(
        source.file.repo_relative_path.as_str(),
        indexed_path,
        witness,
        budget,
    )?
    else {
        candidate.preview = Some(PreviewMetadata::unavailable(
            PreviewKind::Path,
            PreviewUnavailableReason::WorkBudget,
            Some(source.clone()),
        ));
        return Ok(());
    };
    candidate.snippet = source.file.repo_relative_path.as_str().to_string();
    candidate.snippet_hit_offset = Some(highlight.start);
    candidate.highlights.push(highlight);
    candidate.preview = Some(PreviewMetadata {
        kind: PreviewKind::Path,
        source: Some(source.clone()),
        chunk_start_byte: None,
        original_focus: None,
        original_context: None,
        normalized_focus: None,
        normalization_equivalent: false,
        unavailable_reason: None,
    });
    candidate.validate_source_metadata().map_err(|error| {
        CoreError::Storage(format!("lexical: path candidate invariant: {error}"))
    })?;
    Ok(())
}

fn path_file_candidate(
    owner: &TantivySearcher,
    file: &SourceFileMetadata,
    score: f32,
    witness: Option<&Witness>,
    budget: &RequestBudgetV1,
) -> Result<LexicalCandidate, CoreError> {
    let mut candidate = base_file_candidate(owner, &file.source, score)?;
    let Some(witness) = witness else {
        candidate.preview = Some(PreviewMetadata::unavailable(
            PreviewKind::SourceFile,
            PreviewUnavailableReason::NoPositiveWitness,
            Some(file.source.clone()),
        ));
        return Ok(candidate);
    };
    if !matches!(witness.surface, HitSurface::Path) {
        return Err(CoreError::Storage(
            "lexical: path-only candidate has content witness".into(),
        ));
    }
    set_path_preview(
        &mut candidate,
        &file.source,
        &file.indexed_path,
        witness,
        budget,
    )?;
    Ok(candidate)
}

fn file_candidate(
    owner: &TantivySearcher,
    file: &SourceFile,
    score: f32,
    focus: Option<&Witness>,
    materialize_preview: bool,
    budget: &RequestBudgetV1,
) -> Result<LexicalCandidate, CoreError> {
    let source = file.source.clone();
    let mut candidate = base_file_candidate(owner, &source, score)?;
    if !materialize_preview {
        return Ok(candidate);
    }
    let Some(witness) = focus else {
        candidate.preview = Some(PreviewMetadata::unavailable(
            PreviewKind::SourceFile,
            PreviewUnavailableReason::NoPositiveWitness,
            Some(source),
        ));
        return Ok(candidate);
    };
    // Each file has one row. Score/repo/path already give distinct order
    // keys, so line lookup can stay on the selected page without changing
    // sort or cursor order.
    if matches!(witness.surface, HitSurface::Content) {
        (candidate.start_line, candidate.end_line) = content_witness_lines(file, witness, budget)?;
    }
    if matches!(witness.surface, HitSurface::Path) {
        set_path_preview(&mut candidate, &source, &file.indexed_path, witness, budget)?;
        return Ok(candidate);
    }
    let unavailable = |reason| {
        PreviewMetadata::unavailable(PreviewKind::SourceFile, reason, Some(source.clone()))
    };
    if file.bytes.len() > MAX_PREVIEW_SOURCE_BYTES {
        candidate.preview = Some(unavailable(PreviewUnavailableReason::WorkBudget));
        return Ok(candidate);
    }
    budget.checkpoint("lexical:code-search-preview")?;
    let raw = std::str::from_utf8(&file.bytes).map_err(|error| {
        CoreError::Storage(format!("lexical: admitted file is not UTF-8: {error}"))
    })?;
    let indexed = file
        .indexed_text
        .as_deref()
        .ok_or_else(|| CoreError::Storage("lexical: content witness lacks indexed text".into()))?;
    // The source digest and indexed NFC bytes were verified when this
    // authority opened. Reordering/composition in non-NFC source still needs
    // the full provenance map; NFC source only needs a scalar-length walk.
    let folded = file
        .folded_text
        .as_deref()
        .ok_or_else(|| CoreError::Storage("lexical: content witness lacks folded text".into()))?;
    let (original, normalized) = if let Some(original) =
        nfc_identity_focus(raw, indexed, folded, witness, budget)?
    {
        (original.clone(), original)
    } else {
        let mapped = MappedText::new(
            raw,
            indexed,
            witness.mapping_case,
            MAX_PREVIEW_SOURCE_BYTES,
            raw.len().saturating_mul(8).max(1),
            &|| budget.interruption().is_some(),
        );
        let Ok(mapped) = mapped else {
            budget.checkpoint("lexical:code-search-preview-map")?;
            candidate.preview = Some(unavailable(PreviewUnavailableReason::WorkBudget));
            return Ok(candidate);
        };
        let original = mapped
            .source_range(witness.normalized.clone())
            .map_err(|error| CoreError::Storage(format!("lexical: file focus mapping: {error}")))?;
        let normalized = mapped
            .normalized_range(witness.normalized.clone())
            .map_err(|error| {
                CoreError::Storage(format!("lexical: file normalized focus: {error}"))
            })?;
        (original, normalized)
    };
    let mut context_start = original.start.saturating_sub(120);
    while raw.get(context_start..original.start).is_none() {
        context_start = context_start
            .checked_add(1)
            .ok_or_else(|| CoreError::Storage("lexical: preview start overflow".into()))?;
    }
    let mut context_end = original.end.saturating_add(120).min(raw.len());
    while raw.get(original.end..context_end).is_none() {
        context_end = context_end
            .checked_sub(1)
            .ok_or_else(|| CoreError::Storage("lexical: preview end underflow".into()))?;
    }
    let snippet = raw.get(context_start..context_end).ok_or_else(|| {
        CoreError::Storage("lexical: file preview context is not UTF-8 aligned".into())
    })?;
    let span_start =
        u32::try_from(original.start.checked_sub(context_start).ok_or_else(|| {
            CoreError::Storage("lexical: preview highlight precedes context".into())
        })?)
        .map_err(|error| CoreError::Storage(format!("lexical: highlight offset: {error}")))?;
    let span_len = u32::try_from(original.end.checked_sub(original.start).ok_or_else(|| {
        CoreError::Storage("lexical: preview highlight range is reversed".into())
    })?)
    .map_err(|error| CoreError::Storage(format!("lexical: highlight length: {error}")))?;
    candidate.snippet = snippet.to_string();
    candidate.snippet_hit_offset = Some(span_start);
    candidate.highlights.push(HighlightSpan {
        start: span_start,
        len: span_len,
    });
    candidate.preview = Some(PreviewMetadata {
        kind: PreviewKind::SourceFile,
        source: Some(source),
        chunk_start_byte: None,
        original_focus: Some(PreviewByteRange {
            start: byte_offset(original.start)?,
            end: byte_offset(original.end)?,
        }),
        original_context: Some(PreviewByteRange {
            start: byte_offset(context_start)?,
            end: byte_offset(context_end)?,
        }),
        normalized_focus: Some(PreviewByteRange {
            start: byte_offset(normalized.start)?,
            end: byte_offset(normalized.end)?,
        }),
        normalization_equivalent: super::snippets::normalization_changed_for_focus(
            raw, indexed, original, normalized,
        ),
        unavailable_reason: None,
    });
    candidate.validate_source_metadata().map_err(|error| {
        CoreError::Storage(format!("lexical: file candidate invariant: {error}"))
    })?;
    Ok(candidate)
}

/// Under `RawAsciiLocalName`, every requested component of a matching ASCII
/// name must appear in the immutable source bytes. One absent component is a
/// sufficient exclusion proof for an uncensused file.
fn source_proves_component_absence(
    raw: &[u8],
    components: &[String],
    budget: &RequestBudgetV1,
) -> Result<bool, CoreError> {
    budget.checkpoint("lexical:component-coverage-source-proof")?;
    let mut folded = Vec::with_capacity(raw.len());
    for chunk in raw.chunks(64 * 1024) {
        budget.checkpoint("lexical:component-coverage-source-proof")?;
        folded.extend(chunk.iter().map(u8::to_ascii_lowercase));
    }
    let mut unique_components = BTreeSet::new();
    unique_components.extend(components.iter());
    for component in unique_components {
        if memchr::memmem::find(&folded, component.as_bytes()).is_none() {
            return Ok(true);
        }
        budget.checkpoint("lexical:component-coverage-source-proof")?;
    }
    Ok(false)
}

impl TantivySearcher {
    /// File candidates are source identities, not stored chunk document IDs.
    pub(crate) fn code_search_file_by_id(
        &self,
        candidate_id: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Option<SourceFileMetadata>, CoreError> {
        let Some(authority) = &self.file_authority else {
            return Ok(None);
        };
        let scoped = authority.query_scope();
        Self::code_search_file_by_id_in(&scoped, candidate_id, budget)
    }

    fn code_search_file_by_id_in(
        authority: &FileAuthority,
        candidate_id: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Option<SourceFileMetadata>, CoreError> {
        let mut found = None;
        authority.scan_sources(budget, &mut |file| {
            if file_candidate_id(
                file.source.file.source_repo_id.as_str(),
                file.source.file.repo_relative_path.as_str(),
            )? == candidate_id
            {
                found = Some(file);
            }
            Ok(())
        })?;
        Ok(found)
    }

    pub(crate) fn explain_code_file(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        candidate_id: &str,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalCandidateExplanationV1, CoreError> {
        budget.checkpoint("lexical:code-search-explain-start")?;
        let parsed = CodeSearchPlan::parse(query, &self.regex_policy)?;
        let retained = self.file_authority.as_ref().ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
            message: "lexical code search requires rebuilt full-file source authority".into(),
        })?;
        let scoped = retained.query_scope();
        let authority = &scoped;
        let Some(file) = Self::code_search_file_by_id_in(authority, candidate_id, budget)? else {
            return Ok(LexicalCandidateExplanationV1::NotIndexed);
        };
        let not_matched = || LexicalCandidateExplanationV1::NotMatched {
            reason: "the source file does not match the code-search request".into(),
        };
        if constraints
            .repo_relative_path_exact
            .as_ref()
            .is_some_and(|path| file.source.file.repo_relative_path.as_str() != path.as_str())
            || (!constraints.language_any_of.is_empty()
                && !constraints.language_any_of.contains(&file.language))
        {
            return Ok(not_matched());
        }
        let file = authority.load_file_for_key(&file.source.file, budget)?;
        if parsed.typo.is_none()
            && parsed.components.is_none()
            && let Some(mut scored) = score_terms(
                &file,
                &parsed.terms,
                parsed.case,
                None,
                TermsToScore::All,
                budget,
            )?
        {
            let (score, components) =
                finish_score(&file, &parsed.terms, parsed.case, &mut scored, budget)?;
            let (study, study_refusal) = match ranking::study(
                self,
                &file,
                &parsed.terms,
                parsed.case,
                components,
                budget,
            ) {
                Ok(study) => (Some(study), None),
                Err(CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded,
                    ..
                }) => {
                    budget.checkpoint("lexical:code-search-explain-diagnostics")?;
                    (None, Some(quanta_index_contract::SearchPlaneErrorCodeV2::LexicalCollectionBudgetExceeded))
                }
                Err(error) => return Err(error),
            };
            return Ok(LexicalCandidateExplanationV1::Matched(
                LexicalScoreTraceV1 {
                    engine: LexicalScoreEngineV1::CodeSearchFile,
                    engine_score: score,
                    boost_factor: 1.0,
                    emitted_score: score,
                    code_search_components: Some(components),
                    code_search_rank_study: study,
                    code_search_rank_study_refusal: study_refusal,
                },
            ));
        }
        // Auto-typo eligibility depends on the original scoped literal set.
        // Reuse that executor for recovery/components, without a top-k cap or
        // narrowing the scope to this file (which could create a false fallback).
        let fetch = u32::try_from(authority.len().max(1)).map_err(|error| {
            CoreError::Storage(format!("lexical: explain file cardinality: {error}"))
        })?;
        let result = self.search_code_files_plan(
            authority,
            &parsed,
            query,
            constraints,
            &LexicalPageSpec::first(fetch),
            budget,
        )?;
        Ok(result
            .candidates
            .into_iter()
            .find(|row| row.candidate_id == candidate_id)
            .map_or_else(not_matched, |row| {
                LexicalCandidateExplanationV1::Matched(LexicalScoreTraceV1 {
                    engine: LexicalScoreEngineV1::CodeSearchFile,
                    engine_score: row.score,
                    boost_factor: 1.0,
                    emitted_score: row.score,
                    code_search_components: None,
                    code_search_rank_study: None,
                    code_search_rank_study_refusal: None,
                })
            }))
    }

    /// Match ordered components in one stored symbol name before projecting
    /// the result to its immutable source file. The posting conjunction is a
    /// bounded superset; only the stored name decides membership.
    #[expect(
        clippy::too_many_arguments,
        reason = "one component query and its optional request-local candidate observation"
    )]
    fn search_code_files_components(
        &self,
        authority: &FileAuthority,
        query: &LqQuery,
        components: &[String],
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
        attempted_candidates: Option<&mut u64>,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        let candidate_started = Instant::now();
        let mut stats = CodeSearchExecutionStatsV1 {
            mode: CodeSearchExecutionModeV1::Components,
            ..CodeSearchExecutionStatsV1::default()
        };
        // CodeSearch has a file result domain, so the ordinary lexical plan
        // does not infer symbol authority. An incomplete census may be
        // excluded only under an explicit raw-name producer attestation and
        // a source-byte proof for this exact query.
        crate::causal_profile::observe_result(diagnostic, DiagnosticStageV1::QueryVerify, |_| {
            quanta_index_core::domains::lexical::require_complete_symbol_coverage(
                self.source_coverage.as_ref(),
                |entry| {
                    let path = entry.source.file.repo_relative_path.as_str();
                    if !Self::manual_exact_path_allows(path, constraints)
                        || (!constraints.language_any_of.is_empty()
                            && !constraints.language_any_of.contains(&entry.language))
                    {
                        return Ok(false);
                    }
                    if matches!(entry.symbols, SymbolCoverage::Complete { .. }) {
                        return Ok(true);
                    }
                    if entry.symbol_name_source_policy
                        != SymbolNameSourcePolicyV1::RawAsciiLocalName
                    {
                        return Ok(true);
                    }
                    let file = metadata_for_key(authority, &entry.source.file, budget)?;
                    if file.source != entry.source {
                        return Err(CoreError::Storage(
                            "lexical: component coverage source revision differs".into(),
                        ));
                    }
                    let body = authority.load_file_for_key(&entry.source.file, budget)?;
                    Ok(!source_proves_component_absence(
                        &body.bytes,
                        components,
                        budget,
                    )?)
                },
                budget,
            )
        })?;
        let mut clauses = Vec::with_capacity(components.len());
        let mut unique_components = BTreeSet::new();
        unique_components.extend(components.iter());
        for component in unique_components {
            let term = Term::from_field_text(self.fields.symbol_component_folded, component);
            let term_query: Box<dyn tantivy::query::Query> =
                Box::new(TermQuery::new(term, IndexRecordOption::Basic));
            clauses.push((Occur::Must, term_query));
        }
        let compiled =
            self.with_doc_kind(Box::new(BooleanQuery::new(clauses)), crate::SYMBOL_DOC_KIND);
        let searcher = self.reader.searcher();
        let rows = self.collect_whole_set_observed(
            &searcher,
            &*compiled,
            1.0,
            "code search symbol components",
            budget,
            attempted_candidates,
        )?;
        let mut files: BTreeMap<quanta_index_contract::SourceFileKey, (u16, String)> =
            BTreeMap::new();
        let verify_span = diagnostic.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryVerify)
        });
        let verify_result = (|| {
            for row in rows.iter() {
                budget.checkpoint("lexical:code-search-symbol-component-verify")?;
                stats.final_candidate_visits = stats.final_candidate_visits.saturating_add(1);
                let doc = searcher
                    .doc::<TantivyDocument>(row.address)
                    .map_err(|error| {
                        CoreError::Storage(format!("lexical: read component symbol: {error}"))
                    })?;
                let mut names = doc.get_all(self.fields.symbol_local_name);
                let name = names
                    .next()
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        CoreError::Storage("lexical: component symbol lacks local name".into())
                    })?;
                if names.next().is_some() {
                    return Err(CoreError::Storage(
                        "lexical: component symbol has duplicate local names".into(),
                    ));
                }
                let have = crate::symbol_components::name_components(name);
                if !crate::symbol_components::contains_ordered_components(&have, components) {
                    continue;
                }
                let symbol = self.document_to_symbol_candidate_identity(&doc, 0.0)?;
                let source = symbol.source.ok_or_else(|| {
                    CoreError::Storage("lexical: component symbol lacks source identity".into())
                })?;
                let file = metadata_for_key(authority, &source.file, budget)?;
                if file.source != source {
                    return Err(CoreError::Storage(
                        "lexical: component symbol source revision differs from file authority"
                            .into(),
                    ));
                }
                if constraints
                    .repo_relative_path_exact
                    .as_ref()
                    .is_some_and(|path| path.as_str() != source.file.repo_relative_path.as_str())
                    || (!constraints.language_any_of.is_empty()
                        && !constraints.language_any_of.contains(&file.language))
                {
                    continue;
                }
                // An exact local-name component sequence precedes a subrun in a
                // longer name. No body-frequency score enters this typed mode.
                let score = if have == components { 200 } else { 100 };
                match files.entry(source.file) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        let _inserted = entry.insert((score, name.to_owned()));
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        let prior = entry.get();
                        if score > prior.0 || (score == prior.0 && name < prior.1.as_str()) {
                            let _replaced = entry.insert((score, name.to_owned()));
                        }
                    }
                }
            }
            Ok::<(), CoreError>(())
        })();
        finish_verify_observation(
            diagnostic,
            verify_span,
            stats.final_candidate_visits,
            checked_count_u64(files.len(), "component matching files")?,
            verify_result.is_ok(),
        );
        verify_result?;
        stats.verified_matching_files = checked_count_u64(files.len(), "component matching files")?;
        let after = self.page_boundary(page)?;
        let limit = Self::page_limit(
            query,
            usize::try_from(page.fetch).map_err(|error| {
                CoreError::Storage(format!("lexical: component fetch overflow: {error}"))
            })?,
        );
        let mut page_rows = FilePageCollector::new(limit, after.as_deref());
        let rank_span = diagnostic.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryRank)
        });
        let mut rank_attempts = 0_u64;
        let rank_result = (|| {
            for (key, (score, name)) in files {
                rank_attempts = rank_attempts.saturating_add(1);
                budget.checkpoint("lexical:code-search-symbol-component-file")?;
                let _file = metadata_for_key(authority, &key, budget)?;
                page_rows.consider(&key, f32::from(score), None, Some(name))?;
            }
            stats.candidate_ns = observed_ns(candidate_started)?;
            let sort_started = Instant::now();
            let (cursor_eligible, page_rows) = page_rows.into_sorted();
            let exact_total = Some(cursor_eligible);
            stats.cursor_eligible_files = cursor_eligible;
            stats.fetched_files = u64::try_from(page_rows.len()).map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: component fetched file count overflow: {error}"
                ))
            })?;
            stats.sort_page_ns = observed_ns(sort_started)?;
            Ok::<_, CoreError>((exact_total, page_rows))
        })();
        if let (Some(context), Some(span)) = (diagnostic, rank_span) {
            context.counter(&span, DiagnosticCounterV1::Candidates, rank_attempts);
            let _finished = span.finish(DiagnosticTerminalV1::from_result(&rank_result));
        }
        let (exact_total, page_rows) = rank_result?;
        let preview_started = Instant::now();
        let preview_span = diagnostic.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryProject)
        });
        let mut preview_attempts = 0_u64;
        let mut preview_files_decoded = 0_u64;
        let preview_result = (|| {
            let mut candidates = selected_candidate_buffer(page_rows.len())?;
            for LatestFileRow(row) in page_rows {
                preview_attempts = preview_attempts.saturating_add(1);
                budget.checkpoint("lexical:code-search-symbol-component-preview")?;
                let file = authority.load_preview_file_for_key(
                    &row.source,
                    budget,
                    &mut preview_files_decoded,
                )?;
                let witness = file
                    .folded_text
                    .as_deref()
                    .map(|text| {
                        let name = row.component_name.as_deref().ok_or_else(|| {
                            CoreError::Storage("lexical: selected component name is absent".into())
                        })?;
                        best_in(
                            text,
                            &name.to_ascii_lowercase(),
                            HitSurface::Content,
                            CaseMode::Folded,
                            budget,
                        )
                    })
                    .transpose()?
                    .flatten();
                if let Some(text) = file.folded_text.as_deref() {
                    stats.source_surface_bytes_considered = stats
                        .source_surface_bytes_considered
                        .saturating_add(checked_count_u64(text.len(), "source surface bytes")?);
                }
                candidates.push(file_candidate(
                    self,
                    &file,
                    row.score,
                    witness.as_ref(),
                    true,
                    budget,
                )?);
                stats.materialized_files = stats.materialized_files.saturating_add(1);
                stats.preview_attempted_files = stats.preview_attempted_files.saturating_add(1);
            }
            stats.preview_ns = observed_ns(preview_started)?;
            Ok::<_, CoreError>(candidates)
        })();
        if let (Some(context), Some(span)) = (diagnostic, preview_span) {
            context.counter(&span, DiagnosticCounterV1::Candidates, preview_attempts);
            context.counter(
                &span,
                DiagnosticCounterV1::FilesDecoded,
                preview_files_decoded,
            );
            let _finished = span.finish(DiagnosticTerminalV1::from_result(&preview_result));
        }
        let candidates = preview_result?;
        Ok(LexicalSearchPageV1 {
            code_search_stats: Some(stats),
            candidates,
            exact_total,
        })
    }

    fn search_code_files_typo(
        &self,
        authority: &FileAuthority,
        identifier: &str,
        case: CaseMode,
        request: CodeSearchTypoRequest<'_>,
        mut stats: CodeSearchExecutionStatsV1,
        posting_work: &mut QueryWork,
        candidate_started: Instant,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        let CodeSearchTypoRequest {
            query,
            constraints,
            page,
            budget,
            diagnostic,
        } = request;
        // Fallback keeps the original candidate clock across the failed
        // literal pass. The typo shortlist clock starts here so that prior
        // ordinary work remains visible only as unallocated outer time.
        let typo_started = Instant::now();
        let eligible = language_eligible_ids(authority, constraints, budget)?;
        let possible = if constraints.repo_relative_path_exact.is_some() {
            None
        } else {
            typo_candidates_observed(
                identifier,
                eligible.as_ref(),
                MAX_TYPO_POSTING_VISITS,
                budget,
                Some(&mut stats),
                |grams| {
                    let loaded = authority.posting_lists(
                        crate::file_authority::PostingSurface::Content,
                        grams,
                        posting_work,
                        budget,
                    )?;
                    grams
                        .iter()
                        .map(|gram| {
                            loaded.get(gram).cloned().ok_or_else(|| {
                                CoreError::Storage(
                                    "lexical: missing requested typo posting gram".into(),
                                )
                            })
                        })
                        .collect()
                },
            )?
        };
        let mut selected = Vec::new();
        let mut source_bytes = 0_usize;
        authority.scan_sources(budget, &mut |file| {
            let id = file.logical_id;
            let key = &file.source.file;
            if possible.as_ref().is_some_and(|ids| !ids.contains(&id))
                || eligible.as_ref().is_some_and(|ids| !ids.contains(&id))
                || constraints
                    .repo_relative_path_exact
                    .as_ref()
                    .is_some_and(|path| key.repo_relative_path.as_str() != path.as_str())
            {
                return Ok(());
            }
            if !file.text_admitted {
                return Ok(());
            }
            source_bytes = source_bytes.saturating_add(file.indexed_text_bytes);
            selected.push(key.clone());
            if selected.len() > MAX_SHORT_SCAN_FILES || source_bytes > MAX_SHORT_SCAN_SOURCE_BYTES {
                return Err(CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                    message: "lexical code search: typo scan exceeds file or source-byte budget"
                        .into(),
                });
            }
            Ok(())
        })?;
        stats.typo_shortlist_admission_ns = observed_ns(typo_started)?;
        let declaration_started = Instant::now();
        let declarations = crate::causal_profile::observe_result(
            diagnostic,
            DiagnosticStageV1::QueryVerify,
            |_| {
                ranking::typo_declaration_distances(
                    self, authority, &selected, identifier, case, budget,
                )
            },
        )?;
        // Source-bound symbol verification is part of candidate materialization.
        add_observed_ns(
            &mut stats.typo_materialize_ns,
            declaration_started,
            "typo declaration materialization",
        )?;
        let mut comparisons = 0;
        let mut distance_cache = TypoDistanceCache::new();
        let after = self.page_boundary(page)?;
        let limit = Self::page_limit(
            query,
            usize::try_from(page.fetch).map_err(|error| {
                CoreError::Storage(format!("lexical: typo fetch overflow: {error}"))
            })?,
        );
        let mut page_rows = FilePageCollector::new(limit, after.as_deref());
        let prior_verified_visits = stats.final_candidate_visits;
        let mut verified_matches = 0_u64;
        let verify_span = diagnostic.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryVerify)
        });
        let verify_result = (|| {
            for key in selected {
                budget.checkpoint("lexical:code-search-typo-file")?;
                stats.final_candidate_visits = stats.final_candidate_visits.saturating_add(1);
                let file = authority.load_file_for_key(&key, budget)?;
                let content = file.indexed_text.as_deref().ok_or_else(|| {
                    CoreError::Storage("lexical: admitted typo file has no content".into())
                })?;
                stats.source_surface_bytes_considered = stats
                    .source_surface_bytes_considered
                    .saturating_add(checked_count_u64(content.len(), "source surface bytes")?);
                let scan_started = Instant::now();
                let witness = typo_witness(
                    content,
                    identifier,
                    case,
                    &mut comparisons,
                    &mut distance_cache,
                    budget,
                )?;
                add_observed_ns(
                    &mut stats.typo_source_token_scan_ns,
                    scan_started,
                    "typo source-token scan",
                )?;
                let Some((witness, distance)) = witness else {
                    continue;
                };
                let score = f32::from(
                    200_u16
                        .saturating_sub(u16::from(distance).saturating_mul(100))
                        // Lexicographic policy: edit distance, attested declaration,
                        // occurrence. Eight dominates the bounded occurrence 0..6
                        // while remaining strictly below the next distance tier.
                        .saturating_add(if declarations.get(&key) == Some(&distance) {
                            8
                        } else {
                            0
                        })
                        .saturating_add(
                            u16::from(witness.occurrences.saturating_sub(1)).saturating_mul(2),
                        ),
                );
                verified_matches = verified_matches.saturating_add(1);
                let materialize_started = Instant::now();
                page_rows.consider(&key, score, Some(witness), None)?;
                add_observed_ns(
                    &mut stats.typo_materialize_ns,
                    materialize_started,
                    "typo candidate materialization",
                )?;
                stats.verified_matching_files = stats.verified_matching_files.saturating_add(1);
            }
            Ok::<(), CoreError>(())
        })();
        finish_verify_observation(
            diagnostic,
            verify_span,
            stats
                .final_candidate_visits
                .saturating_sub(prior_verified_visits),
            verified_matches,
            verify_result.is_ok(),
        );
        verify_result?;
        stats.typo_token_comparisons = checked_count_u64(comparisons, "typo token comparisons")?;
        stats.candidate_ns = observed_ns(candidate_started)?;
        let rank_span = diagnostic.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryRank)
        });
        let ranked_rows = checked_count_u64(page_rows.rows.len(), "ranked rows")?;
        let rank_result = (|| {
            let sort_started = Instant::now();
            let (cursor_eligible, page_rows) = page_rows.into_sorted();
            let exact_total = Some(cursor_eligible);
            stats.cursor_eligible_files = cursor_eligible;
            stats.fetched_files = u64::try_from(page_rows.len()).map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: typo fetched file count overflow: {error}"
                ))
            })?;
            stats.sort_page_ns = observed_ns(sort_started)?;
            Ok::<_, CoreError>((exact_total, page_rows))
        })();
        if let (Some(context), Some(span)) = (diagnostic, rank_span) {
            context.counter(&span, DiagnosticCounterV1::Candidates, ranked_rows);
            let _finished = span.finish(DiagnosticTerminalV1::from_result(&rank_result));
        }
        let (exact_total, page_rows) = rank_result?;
        let preview_started = Instant::now();
        let preview_span = diagnostic.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryProject)
        });
        let mut preview_attempts = 0_u64;
        let mut preview_files_decoded = 0_u64;
        let preview_result = (|| {
            let mut candidates = selected_candidate_buffer(page_rows.len())?;
            for LatestFileRow(row) in page_rows {
                preview_attempts = preview_attempts.saturating_add(1);
                budget.checkpoint("lexical:code-search-typo-preview")?;
                let file = authority.load_preview_file_for_key(
                    &row.source,
                    budget,
                    &mut preview_files_decoded,
                )?;
                candidates.push(file_candidate(
                    self,
                    &file,
                    row.score,
                    row.witness.as_ref(),
                    true,
                    budget,
                )?);
                stats.materialized_files = stats.materialized_files.saturating_add(1);
                stats.preview_attempted_files = stats.preview_attempted_files.saturating_add(1);
            }
            stats.preview_ns = observed_ns(preview_started)?;
            Ok::<_, CoreError>(candidates)
        })();
        if let (Some(context), Some(span)) = (diagnostic, preview_span) {
            context.counter(&span, DiagnosticCounterV1::Candidates, preview_attempts);
            context.counter(
                &span,
                DiagnosticCounterV1::FilesDecoded,
                preview_files_decoded,
            );
            let _finished = span.finish(DiagnosticTerminalV1::from_result(&preview_result));
        }
        let candidates = preview_result?;
        Ok(LexicalSearchPageV1 {
            code_search_stats: Some(stats),
            candidates,
            exact_total,
        })
    }

    pub(crate) fn search_code_files(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        let diagnostic = budget.diagnostic_context();
        let parse_span = diagnostic.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryParse)
        });
        let parsed = CodeSearchPlan::parse(query, &self.regex_policy);
        if let (Some(context), Some(span)) = (diagnostic, parse_span) {
            if let Ok(plan) = &parsed {
                let (literals, alternatives, grams) = plan.diagnostic_regex_shape();
                context.counter(&span, DiagnosticCounterV1::RegexLiteralCount, literals);
                context.counter(
                    &span,
                    DiagnosticCounterV1::RegexAlternativeCount,
                    alternatives,
                );
                context.counter(&span, DiagnosticCounterV1::RegexNecessaryGramCount, grams);
            }
            if parsed.as_ref().is_err_and(is_query_guard_refusal) {
                context.counter(&span, DiagnosticCounterV1::RejectedAtGuard, 1);
            }
            let outcome = DiagnosticTerminalV1::from_result(&parsed);
            let _finished = span.finish(outcome);
        }
        let parsed = parsed?;
        let retained = self.file_authority.as_ref().ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
            message: "lexical code search requires rebuilt full-file source authority".into(),
        })?;
        let scoped = retained.query_scope();
        self.search_code_files_plan(&scoped, &parsed, query, constraints, page, budget)
    }

    fn search_code_files_plan(
        &self,
        authority: &FileAuthority,
        parsed: &CodeSearchPlan,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        let mut posting_work = QueryWork::default();
        let mut component_attempts = 0_u64;
        let diagnostic = budget.diagnostic_context();
        let span = diagnostic.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryCandidate)
        });
        let candidate_child = diagnostic
            .zip(span.as_ref())
            .map(|(context, span)| context.child_context(span));
        let phase_diagnostic = candidate_child.as_ref();
        if let (Some(context), Some(span)) = (diagnostic, span.as_ref()) {
            let policy = crate::file_authority::policy();
            context.counter(
                span,
                DiagnosticCounterV1::QueryAdmittedLimit,
                checked_count_u64(
                    self.execution_budget.max_examined_candidates(),
                    "query admitted limit",
                )?,
            );
            let limit = crate::file_authority::query_physical_read_limit(policy)?;
            context.counter(span, DiagnosticCounterV1::QueryReadLimit, limit);
            context.counter(
                span,
                DiagnosticCounterV1::QueryByteLimit,
                policy.query_decoded_bytes,
            );
        }
        let result = (|| {
            if let Some(identifier) = parsed.typo.as_deref() {
                let candidate_started = Instant::now();
                return self.search_code_files_typo(
                    authority,
                    identifier,
                    parsed.case,
                    CodeSearchTypoRequest {
                        query,
                        constraints,
                        page,
                        budget,
                        diagnostic: phase_diagnostic,
                    },
                    CodeSearchExecutionStatsV1 {
                        mode: CodeSearchExecutionModeV1::TypoExplicit,
                        ..CodeSearchExecutionStatsV1::default()
                    },
                    &mut posting_work,
                    candidate_started,
                );
            }
            if let Some(components) = parsed.components.as_deref() {
                return self.search_code_files_components(
                    authority,
                    query,
                    components,
                    constraints,
                    page,
                    budget,
                    phase_diagnostic,
                    phase_diagnostic.map(|_| &mut component_attempts),
                );
            }
            let candidate_started = Instant::now();
            let eligible = language_eligible_ids(authority, constraints, budget)?;
            let eligible = eligible.as_ref();
            let mut stats = CodeSearchExecutionStatsV1::default();
            let literal_stream = constraints.repo_relative_path_exact.is_none()
                && parsed.terms.iter().any(|term| term.regex.is_none());
            stats.literal_prefilter_executed = literal_stream;
            let ids: Option<BTreeMap<u64, Option<ScoredMatch>>> =
                if let Some(path) = &constraints.repo_relative_path_exact {
                    let mut matching = BTreeMap::new();
                    authority.scan_sources(budget, &mut |file| {
                        if file.source.file.repo_relative_path.as_str() == path.as_str()
                            && eligible.is_none_or(|ids| ids.contains(&file.logical_id))
                        {
                            let _previous = matching.insert(file.logical_id, None);
                        }
                        Ok(())
                    })?;
                    Some(matching)
                } else {
                    None
                };
            let regex_scope = regex_scan_scope(&parsed.terms);
            // An existing literal/path proof or a small language-restricted set
            // can go directly to the bounded exact verifier. Decide before any
            // posting work is charged; a later posting refusal stays a failure.
            let admitted_language_scan = if ids.is_none() && !literal_stream {
                match (regex_scope, eligible) {
                    (Some(_), Some(eligible)) if eligible.len() > MAX_REGEX_SCAN_FILES => false,
                    (Some(scope), Some(eligible)) => {
                        match admit_regex_scan(authority, eligible.iter().copied(), scope, budget) {
                        Ok(()) => true,
                        Err(CoreError::Typed {
                            code:
                                quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                            ..
                        }) => false,
                        Err(error) => return Err(error),
                    }
                    }
                    _ => false,
                }
            } else {
                false
            };
            let regex_possible = if regex_scope.is_some()
                && ids.is_none()
                && !literal_stream
                && !admitted_language_scan
            {
                regex_candidate_ids(authority, &parsed.terms, &mut posting_work, budget)?
            } else {
                None
            };
            let all_files = usize::try_from(authority.len()).map_err(|error| {
                CoreError::Storage(format!("lexical: file count width: {error}"))
            })?;
            let ids = match ids {
                Some(ids) => ids,
                None if literal_stream => BTreeMap::new(),
                None if regex_scope.is_some() => {
                    budget.checkpoint("lexical:code-search-regex-all-files")?;
                    // Bound allocation before materializing all ids. The shared
                    // regex admission below checks the source-byte budget once.
                    if regex_possible.as_ref().map_or_else(
                        || eligible.map_or(all_files, BTreeSet::len),
                        |ids| {
                            ids.iter()
                                .filter(|id| eligible.is_none_or(|eligible| eligible.contains(id)))
                                .take(MAX_REGEX_SCAN_FILES.saturating_add(1))
                                .count()
                        },
                    ) > MAX_REGEX_SCAN_FILES
                    {
                        return Err(CoreError::Typed {
                        code:
                            quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                        message: "lexical code search regex requires a bounded file scan".into(),
                    });
                    }
                    if let Some(possible) = regex_possible {
                        possible
                            .into_iter()
                            .filter(|id| eligible.is_none_or(|ids| ids.contains(id)))
                            .map(|id| (id, None))
                            .collect()
                    } else if let Some(ids) = eligible {
                        ids.iter().copied().map(|id| (id, None)).collect()
                    } else {
                        let mut all = BTreeMap::new();
                        authority.scan_sources(budget, &mut |file| {
                            let _previous = all.insert(file.logical_id, None);
                            Ok(())
                        })?;
                        all
                    }
                }
                None => {
                    return Err(CoreError::Typed {
                    code:
                        quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                    message: "lexical code search: no term provides a bounded candidate set".into(),
                });
                }
            };
            if let Some(scope) = regex_scope
                && !admitted_language_scan
                && !literal_stream
            {
                admit_regex_scan(authority, ids.keys().copied(), scope, budget)?;
            }
            let after = self.page_boundary(page)?;
            let limit = Self::page_limit(
                query,
                usize::try_from(page.fetch).map_err(|error| {
                    CoreError::Storage(format!("lexical: file fetch overflow: {error}"))
                })?,
            );
            let mut page_rows = FilePageCollector::new(limit, after.as_deref());
            let path_only = parsed
                .terms
                .iter()
                .all(|term| matches!(term.scope, Scope::Path));
            let mut verified_matches = 0_u64;
            let mut consume = |id: u64,
                               preverified: Option<ScoredMatch>,
                               borrowed_full: Option<&SourceFile>,
                               stats: &mut CodeSearchExecutionStatsV1|
             -> Result<(), CoreError> {
                budget.checkpoint("lexical:code-search-file")?;
                stats.final_candidate_visits =
                    stats.final_candidate_visits.checked_add(1).ok_or_else(|| {
                        CoreError::Storage("lexical: final work count overflow".into())
                    })?;
                let metadata = metadata_for_id(authority, id, budget)?;
                let selection = if preverified.is_some() {
                    TermsToScore::Regex
                } else {
                    TermsToScore::All
                };
                let loaded_full = if path_only || borrowed_full.is_some() {
                    None
                } else {
                    Some(authority.load_file_for_id(id, budget)?)
                };
                let full = borrowed_full.or(loaded_full.as_ref());
                let scored = if let Some(file) = full {
                    score_terms_observed(
                        file,
                        &parsed.terms,
                        parsed.case,
                        preverified,
                        selection,
                        budget,
                        Some(stats),
                    )?
                } else {
                    score_path_terms_observed(
                        &metadata,
                        &parsed.terms,
                        parsed.case,
                        preverified,
                        selection,
                        budget,
                        stats,
                    )?
                };
                let Some(mut scored) = scored else {
                    return Ok(());
                };
                let score = if let Some(file) = full {
                    finish_score(file, &parsed.terms, parsed.case, &mut scored, budget)?.0
                } else {
                    finish_path_score(&metadata, &parsed.terms, parsed.case, &mut scored, budget)?
                };
                verified_matches = verified_matches.saturating_add(1);
                page_rows.consider(&metadata.source.file, score, Some(scored.primary), None)?;
                stats.verified_matching_files = stats.verified_matching_files.saturating_add(1);
                Ok(())
            };
            let verify_span = phase_diagnostic.and_then(|context| {
                crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryVerify)
            });
            let verify_result = (|| {
                if literal_stream {
                    // The literal verifier yields each file once. Mixed regex plans
                    // charge the exact verified literal set as they consume it; a
                    // late admission refusal discards the private page heap.
                    let mut regex_files = 0_usize;
                    let mut regex_bytes = 0_usize;
                    for_each_candidate_observed(
                        authority,
                        &parsed.terms,
                        parsed.case,
                        eligible,
                        budget,
                        &mut posting_work,
                        &mut stats,
                        MAX_CANDIDATE_PRE_VERIFY,
                        |id, scored, file, stats| {
                            stats.literal_verified_files =
                                stats.literal_verified_files.checked_add(1).ok_or_else(|| {
                                    CoreError::Storage(
                                        "lexical: literal match count overflow".into(),
                                    )
                                })?;
                            if let Some(scope) = regex_scope {
                                budget.checkpoint("lexical:code-search-regex-admission")?;
                                let file = metadata_for_id(authority, id, budget)?;
                                regex_files = regex_files.saturating_add(1);
                                regex_bytes = regex_bytes.saturating_add(metadata_scanned_bytes(
                                    &file,
                                    scope,
                                    CaseMode::Sensitive,
                                ));
                                if regex_files > MAX_REGEX_SCAN_FILES
                                    || regex_bytes > MAX_REGEX_SCAN_SOURCE_BYTES
                                {
                                    return Err(CoreError::Typed {
                                code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                                message: "lexical code search regex requires a bounded file scan"
                                    .into(),
                            });
                                }
                            }
                            consume(id, Some(scored), file, stats)
                        },
                    )?;
                } else {
                    for (id, preverified) in ids {
                        consume(id, preverified, None, &mut stats)?;
                    }
                }
                Ok::<(), CoreError>(())
            })();
            finish_verify_observation(
                phase_diagnostic,
                verify_span,
                stats.final_candidate_visits,
                verified_matches,
                verify_result.is_ok(),
            );
            verify_result?;
            // A bare identifier with no literal file match may be misspelled.
            // Preserve every exact result and its cursor order; only an empty
            // result takes the existing bounded, source-verified OSA1 route.
            if stats.verified_matching_files == 0
                && let Some(identifier) = parsed.auto_typo_on_empty()
            {
                stats.mode = CodeSearchExecutionModeV1::TypoFallback;
                return self.search_code_files_typo(
                    authority,
                    identifier,
                    parsed.case,
                    CodeSearchTypoRequest {
                        query,
                        constraints,
                        page,
                        budget,
                        diagnostic: phase_diagnostic,
                    },
                    stats,
                    &mut posting_work,
                    candidate_started,
                );
            }
            stats.candidate_ns = observed_ns(candidate_started)?;
            let rank_span = phase_diagnostic.and_then(|context| {
                crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryRank)
            });
            let ranked_rows = checked_count_u64(page_rows.rows.len(), "ranked rows")?;
            let rank_result = (|| {
                let sort_started = Instant::now();
                let (cursor_eligible, page_rows) = page_rows.into_sorted();
                let exact_total = Some(cursor_eligible);
                stats.cursor_eligible_files = exact_total.ok_or_else(|| {
                    CoreError::Storage("lexical: observed file total disappeared".into())
                })?;
                stats.fetched_files = u64::try_from(page_rows.len()).map_err(|error| {
                    CoreError::Storage(format!("lexical: fetched file count overflow: {error}"))
                })?;
                stats.sort_page_ns = observed_ns(sort_started)?;
                Ok::<_, CoreError>((exact_total, page_rows))
            })();
            if let (Some(context), Some(span)) = (phase_diagnostic, rank_span) {
                context.counter(&span, DiagnosticCounterV1::Candidates, ranked_rows);
                let _finished = span.finish(DiagnosticTerminalV1::from_result(&rank_result));
            }
            let (exact_total, page_rows) = rank_result?;
            let preview_started = Instant::now();
            let preview_span = phase_diagnostic.and_then(|context| {
                crate::causal_profile::enter_span(context, DiagnosticStageV1::QueryProject)
            });
            let mut preview_attempts = 0_u64;
            let mut preview_files_decoded = 0_u64;
            let preview_result = (|| {
                let mut candidates = selected_candidate_buffer(page_rows.len())?;
                for LatestFileRow(row) in page_rows {
                    preview_attempts = preview_attempts.saturating_add(1);
                    budget.checkpoint("lexical:code-search-selected-preview")?;
                    if path_only {
                        let file = metadata_for_key(authority, &row.source, budget)?;
                        candidates.push(path_file_candidate(
                            self,
                            &file,
                            row.score,
                            row.witness.as_ref(),
                            budget,
                        )?);
                    } else {
                        let file = authority.load_preview_file_for_key(
                            &row.source,
                            budget,
                            &mut preview_files_decoded,
                        )?;
                        candidates.push(file_candidate(
                            self,
                            &file,
                            row.score,
                            row.witness.as_ref(),
                            true,
                            budget,
                        )?);
                    }
                    stats.materialized_files = stats.materialized_files.saturating_add(1);
                    stats.preview_attempted_files = stats.preview_attempted_files.saturating_add(1);
                }
                stats.preview_ns = observed_ns(preview_started)?;
                Ok::<_, CoreError>(candidates)
            })();
            if let (Some(context), Some(span)) = (phase_diagnostic, preview_span) {
                context.counter(&span, DiagnosticCounterV1::Candidates, preview_attempts);
                context.counter(
                    &span,
                    DiagnosticCounterV1::FilesDecoded,
                    preview_files_decoded,
                );
                let _finished = span.finish(DiagnosticTerminalV1::from_result(&preview_result));
            }
            let candidates = preview_result?;
            Ok(LexicalSearchPageV1 {
                code_search_stats: Some(stats),
                candidates,
                exact_total,
            })
        })();
        finish_query_candidate_observation(
            diagnostic,
            span,
            &posting_work,
            component_attempts,
            authority.query_io_snapshot(),
            &result,
        );
        result
    }
}

fn finish_query_candidate_observation(
    diagnostic: Option<&DiagnosticContextV1>,
    span: Option<DiagnosticSpanGuardV1>,
    posting_work: &QueryWork,
    component_attempts: u64,
    (physical_bytes, physical_reads): (u64, u64),
    result: &Result<LexicalSearchPageV1, CoreError>,
) {
    if let (Some(context), Some(span)) = (diagnostic, span) {
        let (probes, list_reads, posting_ids, decoded_bytes, _decoded_ids) =
            posting_work.diagnostic_snapshot();
        context.counter(&span, DiagnosticCounterV1::FilesVisited, probes);
        context.counter(&span, DiagnosticCounterV1::FilesRead, physical_reads);
        context.counter(&span, DiagnosticCounterV1::BytesRead, physical_bytes);
        context.counter(
            &span,
            DiagnosticCounterV1::Candidates,
            posting_ids.saturating_add(component_attempts),
        );
        context.counter(&span, DiagnosticCounterV1::FilesDecoded, list_reads);
        context.counter(&span, DiagnosticCounterV1::BytesDecoded, decoded_bytes);
        if result.as_ref().is_err_and(is_query_guard_refusal) {
            context.counter(&span, DiagnosticCounterV1::RejectedAtGuard, 1);
        }
        let outcome = DiagnosticTerminalV1::from_result(result);
        let _finished = span.finish(outcome);
    }
}

#[cfg(test)]
#[expect(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing,
    clippy::needless_range_loop,
    clippy::string_slice,
    reason = "small exhaustive independent DP oracle and fixed ASCII fixtures use checked dimensions"
)]
mod tests {
    use super::super::paging::observe_whole_set_collection;
    use super::{
        CodeSearchPlan, CodeSearchTerm, FilePageCollector, HitSurface, MAX_TYPO_POSTING_VISITS,
        MAX_TYPO_TOKEN_COMPARISONS, OverlappingMatches, Scope, TermsToScore, TypoDistanceCache,
        best_in, boundary_score, candidate_ids, content_witness_lines, file_candidate_id,
        finish_query_candidate_observation, finish_verify_observation, folded_regex_grams,
        is_query_guard_refusal, language_eligible_ids, min_cover_gap, nfc_identity_focus,
        path_highlight, regex_candidate_ids, regex_scan_scope, scan_typo_token_spans,
        scanned_bytes, score_terms, source_bytes_checked, typo_candidates, typo_distance,
        typo_text_is_ascii, typo_witness,
    };
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        LQ_VERSION_TAG, LexicalCursor, LexicalRowOrderKey, LqExpr, LqFilter, LqLeaf, LqOptions,
        LqPatternType, LqQuery, LqSelect, LqSpan, ManifestGeneration, QueryConstraintSetV1, RepoId,
        RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2, SourceFileKey, SourceFileRevision,
    };
    use quanta_index_core::{
        CoreError, DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1,
        DiagnosticCounterV1, DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1,
        DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticStageV1, DiagnosticTerminalV1,
        LexicalArtifactIdentityV1, LexicalExecutionBudgetV1, LexicalPageSpec, LexicalSearchPageV1,
        RegexMatchCachePolicy, RepoMetadataAuthoritiesV1, RequestBudgetV1, TextNormalizerVersionV1,
    };
    use quanta_index_lq_regex::RegexExecutor;
    use quanta_index_lq_trigram::{
        DocId, NecessaryGramExpr, TrigramIndexBuilder, TrigramIntersectionError,
    };
    use std::collections::BTreeSet;
    use std::ops::Range;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use crate::file_authority::paged::{FileAuthorityMutation, FileSourceEdit, PagedFileAuthority};
    use crate::file_authority::{QueryWork, SourceFile};
    use crate::lexical_allocation::DocVersionReservation;
    use crate::lexical_store::{AttemptBinding, ObjectStore};
    use crate::ranked_keys::RankedKeyTables;
    use crate::regex::RegexPolicy;
    use crate::regex_match_cache::RegexMatchCache;
    use crate::{SchemaFields, TantivySearcher};
    use quanta_index_contract::{SourceFileCoverage, SymbolCoverage, SymbolNameSourcePolicyV1};
    use quanta_index_lq_text_normalizer::{self as normalize, CaseMode, MappedText};
    use tantivy::{Index, ReloadPolicy};

    #[derive(Debug)]
    struct FixedClock;

    impl DiagnosticClockV1 for FixedClock {
        fn now_ns(&self) -> u64 {
            1
        }
    }

    #[derive(Debug, Default)]
    struct RecordingSink(Mutex<Vec<DiagnosticEmitV1>>);

    impl DiagnosticEventSinkV1 for RecordingSink {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            self.0.lock().expect("recording sink lock").push(event);
            true
        }
    }

    fn preview_test_searcher(authority: PagedFileAuthority, state_root: &Path) -> TantivySearcher {
        let fields = SchemaFields::build();
        let index = Index::create_in_ram(fields.schema.clone());
        let mut writer = index
            .writer::<tantivy::TantivyDocument>(15_000_000)
            .expect("empty test writer");
        let _commit_id = writer.commit().expect("empty test commit");
        drop(writer);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .expect("empty test reader");
        let live_bm25 = Arc::new(
            crate::sealed_generation::live_bm25::LiveBm25Statistics::build(
                &index,
                &fields,
                [0; 32],
                state_root,
                None,
                &[],
            )
            .expect("empty test BM25"),
        );
        TantivySearcher {
            source_coverage: None,
            source_publication_event: None,
            repo_id: RepoId::new("fixture").expect("repo"),
            revision_id: RevisionId::new("revision").expect("revision"),
            generation: ManifestGeneration::new(1),
            fields,
            reader,
            ranked_keys: Arc::new(RankedKeyTables::bind(Vec::new(), &[]).expect("empty ranks")),
            live_bm25,
            repo_metadata: None,
            regex_match_cache: Arc::new(Mutex::new(RegexMatchCache::new(
                RegexMatchCachePolicy::DEFAULT,
            ))),
            regex_policy: RegexPolicy::defaults(),
            execution_budget: LexicalExecutionBudgetV1::DEFAULT,
            text_authority: None,
            file_authority: Some(authority),
            repo_commit_recency: None,
            repo_meta: None,
            repo_topic: None,
            repo_description: None,
            file_ownership: None,
            file_contributor: None,
            resident_bytes_estimate: 0,
            artifact_identity: LexicalArtifactIdentityV1 {
                manifest_digest: "preview-test-generation".into(),
                normalizer: TextNormalizerVersionV1 {
                    major: crate::normalize::TEXT_NORMALIZER_VERSION.major,
                    minor: crate::normalize::TEXT_NORMALIZER_VERSION.minor,
                },
                repo_metadata: RepoMetadataAuthoritiesV1::NONE,
            },
        }
    }

    fn preview_query() -> LqQuery {
        let mut options = LqOptions::defaults();
        options.pattern_type = LqPatternType::CodeSearch;
        LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr: LqExpr::Leaf(LqLeaf::RawString("needle".into())),
            filters: vec![LqFilter::Select {
                dim: LqSelect::File,
            }],
            options,
            directives: Vec::new(),
            source_span: LqSpan::eof(0),
        }
    }

    fn project_observation(
        events: &[DiagnosticEmitV1],
    ) -> (Vec<(DiagnosticCounterV1, u64)>, DiagnosticTerminalV1) {
        let span_id = events
            .iter()
            .find_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    stage: DiagnosticStageV1::QueryProject,
                    ..
                } = event.kind
                {
                    Some(span_id)
                } else {
                    None
                }
            })
            .expect("query project span");
        let counters = events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::Counter {
                    span_id: owner,
                    key,
                    value,
                } = event.kind
                {
                    (owner == span_id).then_some((key, value))
                } else {
                    None
                }
            })
            .collect();
        let outcome = events
            .iter()
            .find_map(|event| {
                if let DiagnosticEventKindV1::SpanEnd {
                    span_id: owner,
                    outcome,
                } = event.kind
                {
                    (owner == span_id).then_some(outcome)
                } else {
                    None
                }
            })
            .expect("query project end");
        (counters, outcome)
    }

    fn preview_test_file(path: &str, body: &[u8]) -> SourceFile {
        SourceFile {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("fixture").expect("repo"),
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new("revision").expect("revision"),
                source_sha256: <sha2::Sha256 as sha2::Digest>::digest(body).into(),
            },
            bytes: body.to_vec().into(),
            text_admitted: true,
            language: LanguageCode::new("rust").expect("language"),
            indexed_text: None,
            folded_text: None,
            indexed_path: String::new(),
            folded_path: String::new(),
        }
    }

    fn object_containing(state_root: &Path, payload: &[u8]) -> PathBuf {
        let objects = state_root.join("lexical-object-pool/objects");
        for shard in std::fs::read_dir(objects).expect("object shards") {
            let shard = shard.expect("object shard");
            if !shard.file_type().expect("object shard type").is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(shard.path()).expect("object files") {
                let path = entry.expect("object entry").path();
                let bytes = std::fs::read(&path).expect("object bytes");
                if bytes.windows(payload.len()).any(|part| part == payload) {
                    return path;
                }
            }
        }
        panic!("preview body object is absent")
    }

    #[derive(Debug)]
    struct CorruptOnProjectSink {
        events: Mutex<Vec<DiagnosticEmitV1>>,
        object: PathBuf,
        payload: &'static [u8],
    }

    impl DiagnosticEventSinkV1 for CorruptOnProjectSink {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            if matches!(
                event.kind,
                DiagnosticEventKindV1::SpanBegin {
                    stage: DiagnosticStageV1::QueryProject,
                    ..
                }
            ) {
                let mut bytes = std::fs::read(&self.object).expect("selected body before fault");
                let offset = bytes
                    .windows(self.payload.len())
                    .position(|part| part == self.payload)
                    .expect("unique selected body payload");
                bytes[offset] ^= 1;
                std::fs::write(&self.object, bytes).expect("mutate second preview body");
            }
            self.events.lock().expect("recording sink lock").push(event);
            true
        }
    }

    #[test]
    fn query_project_counts_selected_bodies_and_preserves_partial_count_after_late_fault() {
        let temp = tempfile::tempdir().expect("state root");
        let bodies: [(&str, &'static [u8]); 2] =
            [("src/a.rs", b"needle alpha"), ("src/z.rs", b"needle omega")];
        let authority = from_test_files(
            bodies
                .iter()
                .map(|(path, body)| preview_test_file(path, body))
                .collect(),
            temp.path(),
        )
        .expect("file authority");
        let searcher = preview_test_searcher(authority, temp.path());
        let query = preview_query();
        let constraints = QueryConstraintSetV1::default();
        let page = LexicalPageSpec::first(2);

        let successful_sink = Arc::new(RecordingSink::default());
        let successful_context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([11; 16]),
            Arc::new(FixedClock),
            successful_sink.clone(),
        );
        let successful_root = successful_context
            .enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(1))
            .expect("successful request root");
        let successful_budget = RequestBudgetV1::unbounded()
            .with_diagnostic_context(successful_context.child_context(&successful_root));
        let successful = searcher
            .search_code_files(&query, &constraints, &page, &successful_budget)
            .expect("two selected previews");
        successful_root
            .finish(DiagnosticTerminalV1::Ok)
            .expect("successful request close");
        assert_eq!(successful.candidates.len(), 2);
        let successful_events = successful_sink.0.lock().expect("recording sink lock");
        let (successful_counters, successful_outcome) = project_observation(&successful_events);
        assert_eq!(successful_outcome, DiagnosticTerminalV1::Ok);
        assert_eq!(
            successful_counters,
            vec![
                (DiagnosticCounterV1::Candidates, 2),
                (DiagnosticCounterV1::FilesDecoded, 2),
            ],
            "posting reads and verification reads must not inflate project body decodes"
        );
        drop(successful_events);

        let second_path = successful.candidates[1].repo_relative_path.as_str();
        let second_body = bodies
            .iter()
            .find_map(|(path, body)| (*path == second_path).then_some(*body))
            .expect("second selected body");
        let fault_sink = Arc::new(CorruptOnProjectSink {
            events: Mutex::new(Vec::new()),
            object: object_containing(temp.path(), second_body),
            payload: second_body,
        });
        let fault_context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([12; 16]),
            Arc::new(FixedClock),
            fault_sink.clone(),
        );
        let fault_root = fault_context
            .enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(1))
            .expect("fault request root");
        let fault_budget = RequestBudgetV1::unbounded()
            .with_diagnostic_context(fault_context.child_context(&fault_root));
        assert!(
            searcher
                .search_code_files(&query, &constraints, &page, &fault_budget)
                .is_err(),
            "the second selected body was changed after verification"
        );
        fault_root
            .finish(DiagnosticTerminalV1::Error { code: 1 })
            .expect("fault request close");
        let fault_events = fault_sink.events.lock().expect("recording sink lock");
        let (fault_counters, fault_outcome) = project_observation(&fault_events);
        assert_eq!(fault_outcome, DiagnosticTerminalV1::Error { code: 1 });
        assert_eq!(
            fault_counters,
            vec![
                (DiagnosticCounterV1::Candidates, 2),
                (DiagnosticCounterV1::FilesDecoded, 1),
            ],
            "a later preview failure retains the first completed body decode"
        );
    }

    #[test]
    fn failed_verification_keeps_partial_match_and_attempt_counts() {
        let sink = Arc::new(RecordingSink::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([1; 16]),
            Arc::new(FixedClock),
            sink.clone(),
        );
        let root = context
            .enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(1))
            .expect("request root");
        let child = context.child_context(&root);
        let verification = child
            .enter(DiagnosticStageV1::QueryVerify, DiagnosticLaneV1(2))
            .expect("verification span");
        let verification_id = verification.span_id();
        finish_verify_observation(Some(&child), Some(verification), 3, 1, false);
        root.finish(DiagnosticTerminalV1::Error { code: 1 })
            .expect("request error");

        let events = sink.0.lock().expect("recording sink lock");
        let observed: Vec<_> = events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::Counter {
                    span_id,
                    key,
                    value,
                } = event.kind
                {
                    (span_id == verification_id).then_some((key, value))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            observed,
            vec![
                (DiagnosticCounterV1::CandidatesVerified, 3),
                (DiagnosticCounterV1::Matches, 1),
            ]
        );
        assert!(events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::SpanEnd {
                span_id,
                outcome: DiagnosticTerminalV1::Error { code: 1 },
            } if span_id == verification_id
        )));
        drop(events);
    }

    #[test]
    fn whole_set_rank_observation_keeps_attempts_on_success_and_refusal() {
        for (attempted, succeeds) in [(2, true), (3, false)] {
            let sink = Arc::new(RecordingSink::default());
            let context = DiagnosticContextV1::new(
                DiagnosticCaptureIdV1([attempted as u8; 16]),
                Arc::new(FixedClock),
                sink.clone(),
            );
            let root = context
                .enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(1))
                .expect("request root");
            let child = context.child_context(&root);
            let result = observe_whole_set_collection(Some(&child), |attempts| {
                *attempts.expect("rank span provides its attempt counter") = attempted;
                if succeeds {
                    Ok(())
                } else {
                    Err(CoreError::InvalidContract("collector refused".into()))
                }
            });
            assert_eq!(result.is_ok(), succeeds);
            root.finish(DiagnosticTerminalV1::Ok)
                .expect("request close");

            let events = sink.0.lock().expect("recording sink lock");
            let rank_span_id = events
                .iter()
                .find_map(|event| {
                    if let DiagnosticEventKindV1::SpanBegin {
                        span_id,
                        stage: DiagnosticStageV1::QueryRank,
                        ..
                    } = event.kind
                    {
                        Some(span_id)
                    } else {
                        None
                    }
                })
                .expect("whole-set query-rank span");
            let candidate_attempts: Vec<_> = events
                .iter()
                .filter_map(|event| {
                    if let DiagnosticEventKindV1::Counter {
                        span_id,
                        key: DiagnosticCounterV1::Candidates,
                        value,
                    } = event.kind
                    {
                        (span_id == rank_span_id).then_some(value)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(candidate_attempts, vec![attempted]);
            assert!(events.iter().any(|event| matches!(
                event.kind,
                DiagnosticEventKindV1::SpanEnd { span_id, outcome }
                    if span_id == rank_span_id
                        && outcome == if succeeds {
                            DiagnosticTerminalV1::Ok
                        } else {
                            DiagnosticTerminalV1::Error { code: 1 }
                        }
            )));
            drop(events);
        }
    }

    #[test]
    fn candidate_parent_counts_postings_without_claiming_exact_verification() {
        let sink = Arc::new(RecordingSink::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([4; 16]),
            Arc::new(FixedClock),
            sink.clone(),
        );
        let root = context
            .enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(1))
            .expect("request root");
        let child = context.child_context(&root);
        let span = child
            .enter(DiagnosticStageV1::QueryCandidate, DiagnosticLaneV1(2))
            .expect("candidate span");
        let span_id = span.span_id();
        let result: Result<LexicalSearchPageV1, CoreError> = Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
            message: "regex guard refusal".into(),
        });
        finish_query_candidate_observation(
            Some(&child),
            Some(span),
            &QueryWork::default(),
            5,
            (0, 0),
            &result,
        );
        root.finish(DiagnosticTerminalV1::Ok)
            .expect("request close");

        let events = sink.0.lock().expect("recording sink lock");
        let counters: Vec<_> = events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::Counter {
                    span_id: owner,
                    key,
                    value,
                } = event.kind
                {
                    (owner == span_id).then_some((key, value))
                } else {
                    None
                }
            })
            .collect();
        assert!(counters.contains(&(DiagnosticCounterV1::Candidates, 5)));
        assert!(counters.contains(&(DiagnosticCounterV1::RejectedAtGuard, 1)));
        assert!(
            !counters
                .iter()
                .any(|(key, _)| { *key == DiagnosticCounterV1::CandidatesVerified })
        );
        assert!(events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::SpanEnd {
                span_id: owner,
                outcome: DiagnosticTerminalV1::Error { code: 1 },
            } if owner == span_id
        )));
        drop(events);
    }

    #[test]
    fn query_guard_classification_excludes_unsupported_request_shape() {
        let unsupported = CoreError::Typed {
            code: SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
            message: "unsupported test shape".into(),
        };
        let limit = CoreError::Typed {
            code: SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
            message: "regex work refused".into(),
        };
        assert!(!is_query_guard_refusal(&unsupported));
        assert!(is_query_guard_refusal(&limit));
    }

    #[test]
    fn regex_shape_counts_syntax_and_necessary_prefilter_separately() {
        let plan = CodeSearchPlan {
            terms: vec![CodeSearchTerm {
                text: "abc|def".into(),
                needle: "abc|def".into(),
                scope: Scope::Both,
                regex: Some(RegexExecutor::compile("abc|def").expect("bounded regex")),
                necessary_grams: Some(NecessaryGramExpr::Or(vec![
                    NecessaryGramExpr::Gram(*b"abc"),
                    NecessaryGramExpr::Gram(*b"def"),
                ])),
            }],
            case: CaseMode::Sensitive,
            typo: None,
            components: None,
        };
        let (literals, alternatives, grams) = plan.diagnostic_regex_shape();
        assert!(literals >= 2);
        assert!(alternatives >= 2);
        assert_eq!(grams, 2);
    }

    fn from_test_files(
        mut files: Vec<SourceFile>,
        state_root: &std::path::Path,
    ) -> Result<PagedFileAuthority, CoreError> {
        files.sort_by(|left, right| left.source.file.cmp(&right.source.file));
        let store = ObjectStore::open(state_root)?;
        let binding = AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [8; 32],
            source_event_sha256: [3; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [4; 32],
            format: 17,
        };
        let lease = store.begin_attempt(&binding, &[])?;
        let reservation = DocVersionReservation {
            first: 1,
            count: files.len() as u64,
            scope_sha256: [5; 32],
        };
        let edits = files
            .into_iter()
            .map(|file| FileSourceEdit::Upsert {
                coverage: SourceFileCoverage {
                    source: file.source,
                    language: file.language,
                    producer_policy_sha256: [6; 32],
                    symbol_name_source_policy: SymbolNameSourcePolicyV1::Unspecified,
                    unit_set_sha256: [7; 32],
                    text_admitted: file.text_admitted,
                    symbols: SymbolCoverage::NotRequested,
                },
                bytes: file.bytes.to_vec(),
            })
            .collect();
        let mutation = FileAuthorityMutation {
            edits,
            clear_all: false,
            operations_sha256: binding.operations_sha256,
        };
        let built = crate::file_authority::paged::build_for_seal(
            &store,
            &lease,
            None,
            &reservation,
            &mutation,
        )?;
        PagedFileAuthority::open(&built.root_bytes, store)
    }

    #[test]
    fn file_explain_locator_and_selected_body_share_one_io_meter() {
        let body = b"selected-body";
        let key = SourceFileKey {
            source_repo_id: RepoId::new("fixture").expect("repo"),
            repo_relative_path: RepoRelativePath::new("src/selected.rs"),
        };
        let file = SourceFile {
            source: SourceFileRevision {
                file: key.clone(),
                revision_id: RevisionId::new("revision").expect("revision"),
                source_sha256: <sha2::Sha256 as sha2::Digest>::digest(body).into(),
            },
            bytes: body.to_vec().into(),
            text_admitted: true,
            language: LanguageCode::new("rust").expect("language"),
            indexed_text: None,
            folded_text: None,
            indexed_path: String::new(),
            folded_path: String::new(),
        };
        let temp = tempfile::tempdir().expect("state root");
        let retained = from_test_files(vec![file], temp.path()).expect("file authority");
        let scoped = retained.query_scope();
        let budget = RequestBudgetV1::unbounded();
        let candidate_id = file_candidate_id("fixture", "src/selected.rs").expect("candidate id");
        let located =
            crate::TantivySearcher::code_search_file_by_id_in(&scoped, &candidate_id, &budget)
                .expect("scoped locator")
                .expect("selected source");
        assert_eq!(located.source.file, key);
        let after_locator = scoped.query_io_snapshot();
        assert!(after_locator.0 > 0 && after_locator.1 > 0);
        let mut preview_files_decoded = 0_u64;
        let selected = scoped
            .load_preview_file_for_key(&key, &budget, &mut preview_files_decoded)
            .expect("selected body");
        assert_eq!(&*selected.bytes, body);
        assert_eq!(preview_files_decoded, 1);
        let after_body = scoped.query_io_snapshot();
        assert!(after_body.0 > after_locator.0 && after_body.1 > after_locator.1);
        let _postings = scoped
            .necessary_candidates(
                crate::file_authority::PostingSurface::Content,
                &NecessaryGramExpr::Gram(*b"sel"),
                &mut QueryWork::default(),
                &budget,
            )
            .expect("posting read");
        assert_eq!(preview_files_decoded, 1);
        let missing = SourceFileKey {
            source_repo_id: RepoId::new("fixture").expect("repo"),
            repo_relative_path: RepoRelativePath::new("src/missing.rs"),
        };
        assert!(
            scoped
                .load_preview_file_for_key(&missing, &budget, &mut preview_files_decoded)
                .is_err()
        );
        assert_eq!(preview_files_decoded, 1);
    }

    #[test]
    fn file_page_collector_keeps_exact_cursor_tail_with_bounded_rows() {
        let repo = RepoId::new("repo").expect("repo");
        let key = |path: &str| SourceFileKey {
            source_repo_id: repo.clone(),
            repo_relative_path: RepoRelativePath::new(path),
        };
        let boundary_id = file_candidate_id("repo", "b.rs").expect("candidate id");
        let boundary = LexicalCursor::at(
            ManifestGeneration::new(7),
            LexicalRowOrderKey {
                source_repo_id: "repo",
                score: 100.0,
                repo_relative_path: "b.rs",
                start_line: 0,
                end_line: 0,
                candidate_id: &boundary_id,
            },
        );
        let mut collector = FilePageCollector::new(2, Some(&boundary));
        for path in ["e.rs", "a.rs", "d.rs", "b.rs", "c.rs"] {
            collector
                .consider(&key(path), 100.0, None, None)
                .expect("row");
            assert!(collector.rows.len() <= 2);
        }
        let (tail_count, rows) = collector.into_sorted();
        assert_eq!(tail_count, 3);
        assert_eq!(
            rows.iter()
                .map(|row| row.0.source.repo_relative_path.as_str())
                .collect::<Vec<_>>(),
            ["c.rs", "d.rs"]
        );
    }

    #[test]
    fn regex_gram_fold_weakens_only_unproved_bytes() {
        assert_eq!(
            folded_regex_grams(&NecessaryGramExpr::Gram(*b"AbC")),
            NecessaryGramExpr::Gram(*b"abc")
        );
        assert_eq!(
            folded_regex_grams(&NecessaryGramExpr::Or(vec![
                NecessaryGramExpr::Gram(*b"AbC"),
                NecessaryGramExpr::Gram([0xc3, 0xa9, b'x']),
            ])),
            NecessaryGramExpr::Or(vec![
                NecessaryGramExpr::Gram(*b"abc"),
                NecessaryGramExpr::AllDocs,
            ])
        );
    }

    #[test]
    fn regex_necessary_gram_keeps_uppercase_source_through_folded_postings() {
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("repo").expect("repo"),
                repo_relative_path: RepoRelativePath::new("src/answer.rs"),
            },
            revision_id: RevisionId::new("revision").expect("revision"),
            source_sha256: <sha2::Sha256 as sha2::Digest>::digest(b"MUST match").into(),
        };
        let file = SourceFile {
            source: source.clone(),
            bytes: b"MUST match".to_vec().into(),
            text_admitted: true,
            language: LanguageCode::new("rust").expect("language"),
            indexed_text: None,
            folded_text: None,
            indexed_path: String::new(),
            folded_path: String::new(),
        };
        let temp = tempfile::tempdir().expect("generation directory");
        let authority = from_test_files(vec![file], temp.path()).expect("authority");
        let term = CodeSearchTerm {
            text: "MUST".into(),
            needle: "MUST".into(),
            scope: Scope::Content,
            regex: Some(RegexExecutor::compile("MUST").expect("regex")),
            necessary_grams: Some(NecessaryGramExpr::Gram(*b"MUS")),
        };
        let ids = regex_candidate_ids(
            &authority,
            &[term],
            &mut QueryWork::default(),
            &RequestBudgetV1::unbounded(),
        )
        .expect("necessary candidates")
        .expect("gram plan");
        assert_eq!(
            ids,
            vec![
                authority
                    .id_for_key(&source.file, None)
                    .expect("source id")
                    .expect("source present")
            ]
        );
    }

    #[test]
    fn sparse_literal_and_regex_avoid_common_gram_read_exhaustion() {
        const FILES: usize = 2_048;
        let query = "sharedlongprefixrareZ9";
        let other = "sharedlongprefixotherQ7";
        let grams: BTreeSet<_> = query.as_bytes().windows(3).collect();
        let common_gram_memberships = grams
            .iter()
            .filter(|gram| other.as_bytes().windows(3).any(|window| window == **gram))
            .count()
            * (FILES - 1);
        assert!(common_gram_memberships > 16_384);

        let repo = RepoId::new("fixture").expect("repo");
        let revision = RevisionId::new("revision").expect("revision");
        let language = LanguageCode::new("rust").expect("language");
        let target_language = LanguageCode::new("go").expect("target language");
        let files = (0..FILES)
            .map(|index| {
                let body = if index == 0 { query } else { other };
                let bytes = body.as_bytes().to_vec();
                SourceFile {
                    source: SourceFileRevision {
                        file: SourceFileKey {
                            source_repo_id: repo.clone(),
                            repo_relative_path: RepoRelativePath::new(if index == 1 {
                                format!("src/{query}.rs")
                            } else {
                                format!("src/{index:04}.rs")
                            }),
                        },
                        revision_id: revision.clone(),
                        source_sha256: <sha2::Sha256 as sha2::Digest>::digest(&bytes).into(),
                    },
                    bytes: bytes.into(),
                    text_admitted: true,
                    language: if index == 0 {
                        target_language.clone()
                    } else {
                        language.clone()
                    },
                    indexed_text: None,
                    folded_text: None,
                    indexed_path: String::new(),
                    folded_path: String::new(),
                }
            })
            .collect();
        let temp = tempfile::tempdir().expect("state root");
        let authority = from_test_files(files, temp.path()).expect("file authority");
        let expected_key = SourceFileKey {
            source_repo_id: repo.clone(),
            repo_relative_path: RepoRelativePath::new("src/0000.rs"),
        };
        let expected_id = authority
            .id_for_key(&expected_key, None)
            .expect("target locator")
            .expect("target ID");
        let path_key = SourceFileKey {
            source_repo_id: repo,
            repo_relative_path: RepoRelativePath::new(format!("src/{query}.rs")),
        };
        let path_id = authority
            .id_for_key(&path_key, None)
            .expect("path locator")
            .expect("path ID");
        let budget = RequestBudgetV1::unbounded();
        let policy = crate::file_authority::policy();

        let literal_scope = authority.query_scope();
        let literal = CodeSearchTerm {
            text: query.into(),
            needle: query.into(),
            scope: Scope::Content,
            regex: None,
            necessary_grams: None,
        };
        let literal_hits = super::candidate_ids_observed(
            &literal_scope,
            std::slice::from_ref(&literal),
            CaseMode::Sensitive,
            None,
            &budget,
            &mut QueryWork::default(),
            &mut super::CodeSearchExecutionStatsV1::default(),
            10,
        )
        .expect("literal candidates and exact verification");
        assert_eq!(
            literal_hits.into_keys().collect::<Vec<_>>(),
            vec![expected_id]
        );
        let literal_io = literal_scope.query_io_snapshot();
        assert!(literal_io.1 > 0 && literal_io.1 <= policy.query_list_reads);
        assert!(literal_io.0 <= policy.query_decoded_bytes);

        let both_scope = authority.query_scope();
        let mut both = literal;
        both.scope = Scope::Both;
        let both_hits = candidate_ids(
            &both_scope,
            std::slice::from_ref(&both),
            CaseMode::Sensitive,
            None,
            &budget,
        )
        .expect("content or path candidates");
        assert_eq!(
            both_hits.into_keys().collect::<BTreeSet<_>>(),
            BTreeSet::from([expected_id, path_id])
        );
        assert!(both_scope.query_io_snapshot().1 <= policy.query_list_reads);

        let language_scope = authority.query_scope();
        let mut constraints = QueryConstraintSetV1::unconstrained();
        let _inserted = constraints.language_any_of.insert(target_language);
        let eligible = language_eligible_ids(&language_scope, &constraints, &budget)
            .expect("language eligibility")
            .expect("language filter");
        let language_hits = candidate_ids(
            &language_scope,
            std::slice::from_ref(&both),
            CaseMode::Sensitive,
            Some(&eligible),
            &budget,
        )
        .expect("language-scoped candidates");
        assert_eq!(
            language_hits.into_keys().collect::<Vec<_>>(),
            vec![expected_id]
        );
        assert!(language_scope.query_io_snapshot().1 <= policy.query_list_reads);

        let (plan, prepared) = crate::regex::plan_regex_prepared(
            query,
            &quanta_index_contract::LqOptions::defaults(),
            &crate::regex::RegexPolicy::defaults(),
        )
        .expect("regex plan");
        let regex = CodeSearchTerm {
            text: query.into(),
            needle: query.into(),
            scope: Scope::Content,
            regex: Some(RegexExecutor::compile_prepared(prepared).expect("regex executor")),
            necessary_grams: Some(plan.necessary_gram_expr().clone()),
        };
        let regex_scope = authority.query_scope();
        let regex_ids = regex_candidate_ids(
            &regex_scope,
            std::slice::from_ref(&regex),
            &mut QueryWork::default(),
            &budget,
        )
        .expect("regex candidates")
        .expect("bounded regex gram plan");
        let mut verified = Vec::new();
        for id in regex_ids {
            let file = regex_scope
                .load_file_for_id(id, &budget)
                .expect("regex source verification");
            if regex
                .regex
                .as_ref()
                .expect("compiled regex")
                .verify(file.bytes.as_ref())
            {
                verified.push(id);
            }
        }
        assert_eq!(verified, vec![expected_id]);
        let regex_io = regex_scope.query_io_snapshot();
        assert!(regex_io.1 > 0 && regex_io.1 <= policy.query_list_reads);
        assert!(regex_io.0 <= policy.query_decoded_bytes);
    }

    fn osa_oracle(left: &[u8], right: &[u8]) -> usize {
        let mut rows = vec![vec![0; right.len() + 1]; left.len() + 1];
        for (index, row) in rows.iter_mut().enumerate() {
            row[0] = index;
        }
        for index in 0..=right.len() {
            rows[0][index] = index;
        }
        for i in 1..=left.len() {
            for j in 1..=right.len() {
                rows[i][j] = (rows[i - 1][j] + 1)
                    .min(rows[i][j - 1] + 1)
                    .min(rows[i - 1][j - 1] + usize::from(left[i - 1] != right[j - 1]));
                if i > 1 && j > 1 && left[i - 1] == right[j - 2] && left[i - 2] == right[j - 1] {
                    rows[i][j] = rows[i][j].min(rows[i - 2][j - 2] + 1);
                }
            }
        }
        rows[left.len()][right.len()]
    }

    fn reference_typo_witness(
        text: &str,
        needle: &str,
        case: CaseMode,
    ) -> (Option<(Range<usize>, u8, u8)>, usize) {
        let mut spans = Vec::new();
        let mut start = None;
        for (index, ch) in text.char_indices() {
            if normalize::is_token_char(ch) {
                if start.is_none() {
                    start = Some(index);
                }
            } else if let Some(from) = start.take() {
                spans.push(from..index);
            }
        }
        if let Some(from) = start {
            spans.push(from..text.len());
        }

        let mut distinct = BTreeSet::new();
        let mut best: Option<(Range<usize>, u8, u8)> = None;
        for span in spans {
            let token = text
                .get(span.clone())
                .expect("reference char_indices spans are UTF-8 boundaries");
            if !token.is_ascii()
                || !token
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
                || token.len().abs_diff(needle.len()) > 1
            {
                continue;
            }
            let _inserted = distinct.insert(token.as_bytes().to_vec());
            let (left, right) = if case == CaseMode::Folded {
                (
                    token.to_ascii_lowercase().into_bytes(),
                    needle.to_ascii_lowercase().into_bytes(),
                )
            } else {
                (token.as_bytes().to_vec(), needle.as_bytes().to_vec())
            };
            let distance = osa_oracle(&left, &right);
            if distance > 1 {
                continue;
            }
            let distance = u8::try_from(distance).expect("one-edit oracle distance fits u8");
            match &mut best {
                Some((_, prior, occurrences)) if distance == *prior => {
                    *occurrences = occurrences.saturating_add(1).min(4);
                }
                Some((_, prior, _)) if distance > *prior => {}
                _ => best = Some((span, distance, 1)),
            }
        }
        (best, distinct.len())
    }

    #[test]
    fn typo_ascii_scan_matches_char_tokenizer_and_exhaustive_osa_goldens() {
        let cases = [
            ("load_json", "load_jjson", CaseMode::Folded),
            ("load_json", "load_jso", CaseMode::Folded),
            ("load_json", "load_jsom", CaseMode::Folded),
            ("load_json", "laod_json", CaseMode::Folded),
            (
                "2load_json load_json2 _load_json",
                "load_json",
                CaseMode::Folded,
            ),
            ("x POST", "POST", CaseMode::Sensitive),
            ("LOAD_JSON load_json", "load_jsom", CaseMode::Sensitive),
            ("LOAD_JSON load_json", "load_jsom", CaseMode::Folded),
            ("fooαbar", "foo", CaseMode::Folded),
            ("foo\u{0301}bar", "foo", CaseMode::Folded),
            ("한load_json", "load_json", CaseMode::Folded),
            (
                "load_json load_json load_json load_json load_json",
                "load_jsom",
                CaseMode::Folded,
            ),
        ];
        let budget = RequestBudgetV1::unbounded();
        for (text, needle, case) in cases {
            let (expected, expected_comparisons) = reference_typo_witness(text, needle, case);
            let mut comparisons = 0;
            let actual = typo_witness(
                text,
                needle,
                case,
                &mut comparisons,
                &mut TypoDistanceCache::new(),
                &budget,
            )
            .expect("bounded fixture scan");
            let actual = actual
                .map(|(witness, distance)| (witness.normalized, distance, witness.occurrences));
            assert_eq!(actual, expected, "{text:?} {needle:?} {case:?}");
            assert_eq!(comparisons, expected_comparisons, "{text:?} {needle:?}");
        }
        // Every ASCII scalar must yield the same whole-token result as the
        // independent char_indices/Unicode-token reference, including NUL,
        // whitespace, punctuation, digits, and underscore at the boundary.
        for separator in 0_u8..=127 {
            let text = format!("foo{}bar", char::from(separator));
            let (expected, expected_comparisons) =
                reference_typo_witness(&text, "foo", CaseMode::Folded);
            let mut comparisons = 0;
            let actual = typo_witness(
                &text,
                "foo",
                CaseMode::Folded,
                &mut comparisons,
                &mut TypoDistanceCache::new(),
                &budget,
            )
            .expect("ASCII boundary scan")
            .map(|(witness, distance)| (witness.normalized, distance, witness.occurrences));
            assert_eq!(actual, expected, "separator {separator}");
            assert_eq!(comparisons, expected_comparisons, "separator {separator}");
        }
    }

    #[test]
    fn typo_ascii_long_cached_scan_keeps_cap_and_cancel_checkpoint() {
        let text = "load_json ".repeat(4_096);
        let budget = RequestBudgetV1::unbounded();
        assert!(typo_text_is_ascii(&text, &budget).expect("ASCII prepass"));
        assert!(
            !typo_text_is_ascii(&(text.clone() + "αbar"), &budget).expect("Unicode tail prepass")
        );
        let mut comparisons = 0;
        let (witness, distance) = typo_witness(
            &text,
            "load_jsom",
            CaseMode::Folded,
            &mut comparisons,
            &mut TypoDistanceCache::new(),
            &budget,
        )
        .expect("long ASCII fixture")
        .expect("one-edit witness");
        assert_eq!(witness.normalized, 0.."load_json".len());
        assert_eq!(witness.occurrences, 4);
        assert_eq!(distance, 1);
        assert_eq!(comparisons, 1, "repeated tokens share one distance check");

        let cancelled = RequestBudgetV1::unbounded();
        cancelled.cancel_handle().cancel();
        assert!(matches!(
            typo_witness(
                &text,
                "load_jsom",
                CaseMode::Folded,
                &mut 0,
                &mut TypoDistanceCache::new(),
                &cancelled,
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            })
        ));

        let mid_scan_budget = RequestBudgetV1::unbounded();
        let cancel_mid_scan = mid_scan_budget.cancel_handle();
        let mut visited_spans = 0_usize;
        let interrupted = scan_typo_token_spans(
            text.bytes()
                .enumerate()
                .map(|(index, byte)| (index, byte.is_ascii_alphanumeric() || byte == b'_')),
            text.len(),
            &mid_scan_budget,
            |_span| {
                visited_spans += 1;
                if visited_spans == 1 {
                    cancel_mid_scan.cancel();
                }
                Ok(())
            },
        );
        assert!(matches!(
            interrupted,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            })
        ));
        assert!(visited_spans > 1 && visited_spans < 4_096);
    }

    #[test]
    fn typo_distance_matches_independent_osa_oracle() {
        let alphabet = [b'a', b'b', b'c'];
        let mut words = vec![Vec::new()];
        for length in 1..=5 {
            for mut index in 0..3_usize.pow(length) {
                let mut word = vec![b'a'; length as usize];
                for byte in &mut word {
                    *byte = alphabet[index % 3];
                    index /= 3;
                }
                words.push(word);
            }
        }
        for left in &words {
            for right in &words {
                let expected = osa_oracle(left, right);
                let actual = typo_distance(left, right, CaseMode::Sensitive);
                assert_eq!(
                    actual,
                    (expected <= 1).then_some(expected as u8),
                    "{left:?} {right:?}"
                );
            }
        }
    }

    #[test]
    fn typo_trigram_prefilter_retains_every_single_edit_token() {
        let originals = ["load_json", "publish_event", "collect_index", "aaaaabaaaa"];
        let budget = RequestBudgetV1::unbounded();
        for original in originals {
            let mut builder = TrigramIndexBuilder::new(1).expect("index");
            builder.add_doc(DocId(1), original.as_bytes());
            let index = builder.finish();
            let bytes = original.as_bytes();
            let mut queries = Vec::new();
            for position in 0..bytes.len() {
                let mut substitute = bytes.to_vec();
                substitute[position] = if bytes[position] == b'z' { b'y' } else { b'z' };
                queries.push(substitute);
                let mut deletion = bytes.to_vec();
                let _removed = deletion.remove(position);
                queries.push(deletion);
                let mut insertion = bytes.to_vec();
                insertion.insert(position, b'z');
                queries.push(insertion);
                if position + 1 < bytes.len() {
                    let mut transpose = bytes.to_vec();
                    transpose.swap(position, position + 1);
                    queries.push(transpose);
                }
            }
            for query in queries {
                assert!(osa_oracle(bytes, &query) <= 1);
                let query = String::from_utf8(query).expect("ASCII");
                let candidates =
                    typo_candidates(&index, &query, None, MAX_TYPO_POSTING_VISITS, &budget)
                        .expect("prefilter");
                assert!(
                    candidates.as_ref().is_none_or(|ids| ids.contains(&1)),
                    "{original} {query}"
                );
            }
        }
        let mut content_only = TrigramIndexBuilder::new(1).expect("content index");
        content_only.add_doc(DocId(1), b"unrelated content");
        let path_only = typo_candidates(
            &content_only.finish(),
            "load_jsom",
            None,
            MAX_TYPO_POSTING_VISITS,
            &budget,
        )
        .expect("content prefilter")
        .expect("query has enough distinct trigrams");
        assert!(
            path_only.is_empty(),
            "a path-only hit must not enter content typo search"
        );
    }

    #[test]
    fn typo_language_eligibility_precedes_global_posting_budget() {
        let mut builder = TrigramIndexBuilder::new(1).expect("index");
        for id in 1..=20 {
            builder.add_doc(DocId(id), b"load_json");
        }
        let index = builder.finish();
        let budget = RequestBudgetV1::unbounded();
        assert!(matches!(
            typo_candidates(&index, "load_jsom", None, 8, &budget),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
        let one = BTreeSet::from([1_u64]);
        assert_eq!(
            typo_candidates(&index, "load_jsom", Some(&one), 8, &budget)
                .expect("one eligible file")
                .expect("indexed candidate set"),
            one
        );
        let empty = BTreeSet::new();
        assert!(
            typo_candidates(&index, "load_jsom", Some(&empty), 0, &budget)
                .expect("no eligible files")
                .expect("indexed candidate set")
                .is_empty()
        );
    }

    #[test]
    fn typo_witness_requires_a_whole_content_identifier() {
        let budget = RequestBudgetV1::unbounded();
        let mut comparisons = 0;
        let absent_tokens = ["load_jsxx", "load_jsom_suffix", "unrelated"];
        for token in absent_tokens {
            assert!(osa_oracle(b"load_jsom", token.as_bytes()) > 1, "{token}");
        }
        assert!(
            typo_witness(
                &absent_tokens.join(" "),
                "load_jsom",
                CaseMode::Folded,
                &mut comparisons,
                &mut TypoDistanceCache::new(),
                &budget,
            )
            .expect("independent no-answer fixture")
            .is_none()
        );
        let mut exhausted = MAX_TYPO_TOKEN_COMPARISONS;
        assert!(matches!(
            typo_witness(
                "load_json",
                "load_jsom",
                CaseMode::Folded,
                &mut exhausted,
                &mut TypoDistanceCache::new(),
                &budget
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
        for absent in [
            "// load_jsomized",
            "load_jsxx",
            "load_jsom_suffix",
            "한load_json",
        ] {
            assert!(
                typo_witness(
                    absent,
                    "load_jsom",
                    CaseMode::Folded,
                    &mut comparisons,
                    &mut TypoDistanceCache::new(),
                    &budget
                )
                .expect("scan")
                .is_none(),
                "{absent}"
            );
        }
        let text = "// load_json and LOAD_JSON";
        let (witness, distance) = typo_witness(
            text,
            "load_jsom",
            CaseMode::Folded,
            &mut comparisons,
            &mut TypoDistanceCache::new(),
            &budget,
        )
        .expect("scan")
        .expect("match");
        assert_eq!(distance, 1);
        assert_eq!(&text[witness.normalized], "load_json");
        assert_eq!(witness.occurrences, 2);
        let exact = "load_json load_jsom";
        let (exact_witness, exact_distance) = typo_witness(
            exact,
            "load_jsom",
            CaseMode::Folded,
            &mut comparisons,
            &mut TypoDistanceCache::new(),
            &budget,
        )
        .expect("scan")
        .expect("exact match");
        assert_eq!(exact_distance, 0);
        assert_eq!(&exact[exact_witness.normalized], "load_jsom");
        assert!(
            typo_witness(
                "LOAD_JSON",
                "load_jsom",
                CaseMode::Sensitive,
                &mut comparisons,
                &mut TypoDistanceCache::new(),
                &budget
            )
            .expect("scan")
            .is_none()
        );
    }

    #[test]
    fn typo_distance_budget_counts_distinct_tokens_across_files() {
        let budget = RequestBudgetV1::unbounded();
        let mut comparisons = MAX_TYPO_TOKEN_COMPARISONS - 1;
        let mut distance_cache = TypoDistanceCache::new();
        let first = typo_witness(
            "load_json",
            "load_jsom",
            CaseMode::Folded,
            &mut comparisons,
            &mut distance_cache,
            &budget,
        )
        .expect("first file")
        .expect("one-edit witness");
        assert_eq!(first.1, 1);
        assert_eq!(comparisons, MAX_TYPO_TOKEN_COMPARISONS);

        let repeated = typo_witness(
            "load_json load_json",
            "load_jsom",
            CaseMode::Folded,
            &mut comparisons,
            &mut distance_cache,
            &budget,
        )
        .expect("repeated token does not repeat the distance computation")
        .expect("one-edit witness");
        assert_eq!(repeated.0.occurrences, 2);
        assert_eq!(repeated.1, 1);
        assert_eq!(comparisons, MAX_TYPO_TOKEN_COMPARISONS);

        assert!(matches!(
            typo_witness(
                "load_jsob",
                "load_jsom",
                CaseMode::Folded,
                &mut comparisons,
                &mut distance_cache,
                &budget,
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
    }

    #[test]
    fn shared_folded_prefilter_keeps_sensitive_unicode_hits_and_rejects_case_collisions() {
        let text = "ABC İΣß ẞ";
        let path = "src/ABC.rs";
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("repo").expect("repo"),
                repo_relative_path: RepoRelativePath::new(path),
            },
            revision_id: RevisionId::new("revision").expect("revision"),
            source_sha256: <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()).into(),
        };
        let file = SourceFile {
            source,
            bytes: text.as_bytes().to_vec().into(),
            text_admitted: true,
            language: LanguageCode::new("rust").expect("language"),
            indexed_text: None,
            folded_text: None,
            indexed_path: String::new(),
            folded_path: String::new(),
        };
        let temp = tempfile::tempdir().expect("generation directory");
        let authority = from_test_files(vec![file], temp.path()).expect("authority");
        let budget = RequestBudgetV1::unbounded();
        for (needle, scope, expected) in [
            ("ABC", Scope::Content, true),
            ("abc", Scope::Content, false),
            ("İΣß", Scope::Content, true),
            ("i\u{307}σß", Scope::Content, false),
            ("ẞ", Scope::Content, true),
            ("ABC", Scope::Path, true),
            ("abc", Scope::Path, false),
        ] {
            let term = CodeSearchTerm {
                text: needle.to_owned(),
                needle: needle.to_owned(),
                scope,
                regex: None,
                necessary_grams: None,
            };
            let hits = candidate_ids(
                &authority,
                std::slice::from_ref(&term),
                CaseMode::Sensitive,
                None,
                &budget,
            )
            .expect("sensitive search");
            assert_eq!(!hits.is_empty(), expected, "{needle}");
        }
    }

    #[test]
    fn nfc_preview_fast_path_matches_independent_provenance_oracle() {
        let raw = "A İ Σ café क् 한글 x\u{301} 끝";
        assert_eq!(normalize::nfc(raw).as_ref(), raw);
        let budget = RequestBudgetV1::unbounded();
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let surface = normalize::apply_case(raw, case);
            let oracle = MappedText::new(raw, raw, case, 4096, 16384, &|| false)
                .expect("independent provenance oracle");
            assert_eq!(surface.as_ref(), oracle.text());
            let mut offsets: Vec<_> = surface.char_indices().map(|(at, _)| at).collect();
            offsets.push(surface.len());
            for &start in &offsets {
                for &end in offsets.iter().filter(|&&end| end > start) {
                    let witness = super::Witness {
                        surface: HitSurface::Content,
                        normalized: start..end,
                        score: 100,
                        occurrences: 1,
                        mapping_case: case,
                    };
                    let original = nfc_identity_focus(
                        raw,
                        raw,
                        normalize::apply_case(raw, CaseMode::Folded).as_ref(),
                        &witness,
                        &budget,
                    )
                    .expect("fast path")
                    .expect("NFC identity");
                    assert_eq!(original, oracle.source_range(start..end).expect("source"));
                    assert_eq!(original, oracle.normalized_range(start..end).expect("NFC"));
                }
            }
        }
    }

    #[test]
    fn non_nfc_preview_keeps_full_mapping_and_nfc_walk_observes_cancel() {
        let raw = "cafe\u{301} İ";
        let indexed = normalize::nfc(raw);
        let folded = normalize::apply_case(indexed.as_ref(), CaseMode::Folded);
        let witness = super::Witness {
            surface: HitSurface::Content,
            normalized: 0..1,
            score: 100,
            occurrences: 1,
            mapping_case: CaseMode::Folded,
        };
        assert!(
            nfc_identity_focus(
                raw,
                indexed.as_ref(),
                folded.as_ref(),
                &witness,
                &RequestBudgetV1::unbounded(),
            )
            .expect("non-NFC selection")
            .is_none()
        );
        let budget = RequestBudgetV1::unbounded();
        budget.cancel_handle().cancel();
        assert!(matches!(
            nfc_identity_focus("İ", "İ", "i\u{307}", &witness, &budget),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            })
        ));
    }

    #[test]
    fn large_source_line_location_uses_folded_match_position() {
        let raw = format!(
            "head\n{}\nİneedle",
            "x".repeat(super::MAX_PREVIEW_SOURCE_BYTES)
        );
        let indexed = normalize::nfc(&raw).into_owned();
        let folded = normalize::fold(&indexed);
        let needle = "i\u{307}needle";
        let start = folded.find(needle).expect("folded match");
        let witness = super::Witness {
            surface: HitSurface::Content,
            normalized: start..start + needle.len(),
            score: 100,
            occurrences: 1,
            mapping_case: CaseMode::Folded,
        };
        let file = SourceFile {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("repo").expect("repo"),
                    repo_relative_path: RepoRelativePath::new("large.rs"),
                },
                revision_id: RevisionId::new("revision").expect("revision"),
                source_sha256: <sha2::Sha256 as sha2::Digest>::digest(raw.as_bytes()).into(),
            },
            bytes: raw.into_bytes().into(),
            text_admitted: true,
            language: LanguageCode::new("rust").expect("language"),
            indexed_text: Some(indexed.into()),
            folded_text: Some(folded.into()),
            indexed_path: "large.rs".into(),
            folded_path: "large.rs".into(),
        };
        assert!(file.bytes.len() > super::MAX_PREVIEW_SOURCE_BYTES);
        assert_eq!(
            content_witness_lines(&file, &witness, &RequestBudgetV1::unbounded())
                .expect("line span"),
            (3, 3),
        );
        let across_newline = super::Witness {
            surface: HitSurface::Content,
            normalized: 2..6,
            score: 100,
            occurrences: 1,
            mapping_case: CaseMode::Sensitive,
        };
        assert_eq!(
            content_witness_lines(&file, &across_newline, &RequestBudgetV1::unbounded())
                .expect("multi-line span"),
            (1, 2),
        );
    }

    #[test]
    fn exact_identifier_beats_prefix_and_infix() {
        assert!(boundary_score("foo", 0..3, false) > boundary_score("foobar", 0..3, false));
        assert!(boundary_score("foobar", 0..3, false) > boundary_score("afoobar", 1..4, false));
    }

    #[test]
    fn cached_literal_score_preserves_earlier_regex_witness_and_rejection() {
        let text = "abc abc";
        let file = SourceFile {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("fixture").expect("repo"),
                    repo_relative_path: RepoRelativePath::new("file.rs"),
                },
                revision_id: RevisionId::new("revision").expect("revision"),
                source_sha256: [0; 32],
            },
            bytes: text.as_bytes().to_vec().into(),
            text_admitted: true,
            language: LanguageCode::new("rust").expect("language"),
            indexed_text: Some(text.into()),
            folded_text: Some(text.into()),
            indexed_path: "file.rs".into(),
            folded_path: "file.rs".into(),
        };
        let terms = [
            CodeSearchTerm {
                text: "abc".into(),
                needle: "abc".into(),
                scope: Scope::Content,
                regex: Some(RegexExecutor::compile("abc").expect("regex")),
                necessary_grams: None,
            },
            CodeSearchTerm {
                text: "abc".into(),
                needle: "abc".into(),
                scope: Scope::Content,
                regex: None,
                necessary_grams: None,
            },
        ];
        let budget = RequestBudgetV1::unbounded();
        let cached = score_terms(
            &file,
            &terms,
            CaseMode::Sensitive,
            None,
            TermsToScore::Literals,
            &budget,
        )
        .expect("literal score")
        .expect("literal match");
        assert_eq!(cached.primary_term, 1);
        let reused = score_terms(
            &file,
            &terms,
            CaseMode::Sensitive,
            Some(cached),
            TermsToScore::Regex,
            &budget,
        )
        .expect("cached score")
        .expect("complete match");
        let fresh = score_terms(
            &file,
            &terms,
            CaseMode::Sensitive,
            None,
            TermsToScore::All,
            &budget,
        )
        .expect("fresh score")
        .expect("complete match");
        assert_eq!(reused.components, fresh.components);
        assert_eq!(reused.primary_term, 0);
        assert_eq!(reused.primary.normalized, fresh.primary.normalized);

        let missing_regex = [
            CodeSearchTerm {
                text: "missing".into(),
                needle: "missing".into(),
                scope: Scope::Content,
                regex: Some(RegexExecutor::compile("missing").expect("regex")),
                necessary_grams: None,
            },
            CodeSearchTerm {
                text: "abc".into(),
                needle: "abc".into(),
                scope: Scope::Content,
                regex: None,
                necessary_grams: None,
            },
        ];
        let cached = score_terms(
            &file,
            &missing_regex,
            CaseMode::Sensitive,
            None,
            TermsToScore::Literals,
            &budget,
        )
        .expect("literal score")
        .expect("literal match");
        assert!(
            score_terms(
                &file,
                &missing_regex,
                CaseMode::Sensitive,
                Some(cached),
                TermsToScore::Regex,
                &budget,
            )
            .expect("regex score")
            .is_none()
        );
    }

    #[test]
    fn overlapping_literal_witnesses_preserve_minimum_file_proximity() {
        let budget = RequestBudgetV1::unbounded();
        let terms = ["aaa", "b"].map(|text| CodeSearchTerm {
            text: text.into(),
            needle: text.into(),
            scope: Scope::Content,
            regex: None,
            necessary_grams: None,
        });
        assert_eq!(
            OverlappingMatches::new("aaaaab", "aaa").collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            OverlappingMatches::new("가가가", "가가").collect::<Vec<_>>(),
            vec![0, 3]
        );
        assert_eq!(
            min_cover_gap("aaaaab", &terms, HitSurface::Content, &budget).expect("minimum cover"),
            Some(0),
            "the final aaa occupies bytes 2..5 and b occupies 5..6"
        );
        assert_eq!(
            best_in(
                "aaaaab",
                "aaa",
                HitSurface::Content,
                CaseMode::Sensitive,
                &budget,
            )
            .expect("literal scan")
            .expect("literal witness")
            .occurrences,
            3
        );
    }

    #[test]
    fn combining_mark_is_not_an_identifier_boundary() {
        let exact = boundary_score("म", 0.."म".len(), false);
        let prefix = boundary_score("म्", 0.."म".len(), false);
        assert_eq!(exact, 100);
        assert_eq!(prefix, 75);
        assert!(exact > prefix);
    }

    #[test]
    fn language_constraint_applies_before_short_literal_scan_limit() {
        let go = LanguageCode::new("go").expect("language");
        let txt = LanguageCode::new("text").expect("language");
        let repo = RepoId::new("fixture").expect("repo");
        let revision = RevisionId::new("fixture-revision").expect("revision");
        let files = (0..=10_000)
            .map(|index| {
                let value = if index == 0 { "x" } else { "y" };
                let path = format!("src/{index:05}.txt");
                SourceFile {
                    source: SourceFileRevision {
                        file: SourceFileKey {
                            source_repo_id: repo.clone(),
                            repo_relative_path: RepoRelativePath::new(&path),
                        },
                        revision_id: revision.clone(),
                        source_sha256: <sha2::Sha256 as sha2::Digest>::digest(value.as_bytes())
                            .into(),
                    },
                    bytes: value.as_bytes().to_vec().into(),
                    text_admitted: true,
                    language: if index == 0 { go.clone() } else { txt.clone() },
                    indexed_text: None,
                    folded_text: None,
                    indexed_path: String::new(),
                    folded_path: String::new(),
                }
            })
            .collect();
        let temp = tempfile::tempdir().expect("generation directory");
        let authority = from_test_files(files, temp.path()).expect("file authority");
        let term = CodeSearchTerm {
            text: "x".into(),
            needle: "x".into(),
            scope: Scope::Content,
            regex: None,
            necessary_grams: None,
        };
        let budget = RequestBudgetV1::unbounded();
        assert!(matches!(
            candidate_ids(
                &authority,
                std::slice::from_ref(&term),
                CaseMode::Sensitive,
                None,
                &budget,
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
        let mut constraints = QueryConstraintSetV1::unconstrained();
        let _inserted = constraints.language_any_of.insert(go);
        let eligible = language_eligible_ids(&authority, &constraints, &budget)
            .expect("language filter")
            .expect("constrained ids");
        assert_eq!(eligible.len(), 1);
        let matches = candidate_ids(
            &authority,
            std::slice::from_ref(&term),
            CaseMode::Sensitive,
            Some(&eligible),
            &budget,
        )
        .expect("eligible short scan");
        assert_eq!(matches.into_keys().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn broad_individual_terms_admit_their_small_file_and_intersection() {
        let repo = RepoId::new("fixture").expect("repo");
        let revision = RevisionId::new("fixture-revision").expect("revision");
        let language = LanguageCode::new("text").expect("language");
        // This scales the product candidate cap down for a fixed 21-file
        // oracle. The F15 source-file ceiling is 32,768, so the former
        // 200,001-file fixture could never reach a serving generation.
        let mut files = Vec::new();
        for position in 0..=20_u64 {
            let text = if position == 20 {
                "aaa bbb"
            } else if position < 10 {
                "aaa"
            } else {
                "bbb"
            };
            let key = SourceFileKey {
                source_repo_id: repo.clone(),
                repo_relative_path: RepoRelativePath::new(format!("src/{position:06}.txt")),
            };
            files.push(SourceFile {
                source: SourceFileRevision {
                    file: key,
                    revision_id: revision.clone(),
                    source_sha256: <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()).into(),
                },
                bytes: text.as_bytes().to_vec().into(),
                text_admitted: true,
                language: language.clone(),
                indexed_text: None,
                folded_text: None,
                indexed_path: String::new(),
                folded_path: String::new(),
            });
        }
        let temp = tempfile::tempdir().expect("generation directory");
        let authority = from_test_files(files, temp.path()).expect("F15 authority");
        let terms = ["aaa", "bbb"].map(|text| CodeSearchTerm {
            text: text.into(),
            needle: text.into(),
            scope: Scope::Content,
            regex: None,
            necessary_grams: None,
        });
        let budget = RequestBudgetV1::unbounded();
        let run = |terms: &[CodeSearchTerm]| {
            super::candidate_ids_observed(
                &authority,
                terms,
                CaseMode::Sensitive,
                None,
                &budget,
                &mut super::QueryWork::default(),
                &mut super::CodeSearchExecutionStatsV1::default(),
                10,
            )
        };
        assert!(matches!(
            run(std::slice::from_ref(&terms[0])),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
        assert_eq!(
            run(&terms)
                .expect("joint candidate set")
                .into_keys()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([21]),
        );
    }

    #[test]
    fn short_scan_admission_counts_folded_content_and_path_bytes() {
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("fixture").expect("repo"),
                repo_relative_path: RepoRelativePath::new("src/İ.go"),
            },
            revision_id: RevisionId::new("revision").expect("revision"),
            source_sha256: <sha2::Sha256 as sha2::Digest>::digest("İ".as_bytes()).into(),
        };
        let temp = tempfile::tempdir().expect("generation directory");
        let authority = from_test_files(
            vec![SourceFile {
                source,
                bytes: "İ".as_bytes().to_vec().into(),
                text_admitted: true,
                language: LanguageCode::new("go").expect("language"),
                indexed_text: None,
                folded_text: None,
                indexed_path: String::new(),
                folded_path: String::new(),
            }],
            temp.path(),
        )
        .expect("file authority");
        let file = authority
            .load_file_for_key(
                &authority
                    .metadata_for_id(1, None)
                    .expect("file metadata")
                    .expect("file present")
                    .source
                    .file,
                &RequestBudgetV1::unbounded(),
            )
            .expect("file");
        assert_eq!(file.folded_text.as_deref(), Some("i\u{307}"));
        assert_eq!(file.folded_path, "src/i\u{307}.go");
        assert_eq!(scanned_bytes(&file, Scope::Content, CaseMode::Sensitive), 2);
        assert_eq!(scanned_bytes(&file, Scope::Content, CaseMode::Folded), 3);
        assert_eq!(scanned_bytes(&file, Scope::Both, CaseMode::Folded), 13);
        let measured = source_bytes_checked(
            &authority,
            Scope::Both,
            CaseMode::Folded,
            None,
            &RequestBudgetV1::unbounded(),
        )
        .expect("admitted scan size");
        assert_eq!(measured, 13);
    }

    #[test]
    fn regex_scan_scope_charges_only_surfaces_the_regex_reads() {
        let term = |scope| CodeSearchTerm {
            text: "needle".into(),
            needle: "needle".into(),
            scope,
            regex: Some(RegexExecutor::compile("needle").expect("regex")),
            necessary_grams: None,
        };
        assert!(matches!(
            regex_scan_scope(&[term(Scope::Content)]),
            Some(Scope::Content)
        ));
        assert!(matches!(
            regex_scan_scope(&[term(Scope::Path)]),
            Some(Scope::Path)
        ));
        assert!(matches!(
            regex_scan_scope(&[term(Scope::Content), term(Scope::Path)]),
            Some(Scope::Both)
        ));
        assert!(regex_scan_scope(&[]).is_none());
    }

    #[test]
    fn folded_unicode_path_hit_highlights_original_utf8_bytes() {
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: RepoId::new("fixture").expect("repo"),
                repo_relative_path: RepoRelativePath::new("src/İ.go"),
            },
            revision_id: RevisionId::new("revision").expect("revision"),
            source_sha256: <sha2::Sha256 as sha2::Digest>::digest([]).into(),
        };
        let temp = tempfile::tempdir().expect("generation directory");
        let authority = from_test_files(
            vec![SourceFile {
                source,
                bytes: Vec::new().into(),
                text_admitted: false,
                language: LanguageCode::new("go").expect("language"),
                indexed_text: None,
                folded_text: None,
                indexed_path: String::new(),
                folded_path: String::new(),
            }],
            temp.path(),
        )
        .expect("file authority");
        let file = authority
            .load_file_for_key(
                &authority
                    .metadata_for_id(1, None)
                    .expect("file metadata")
                    .expect("file present")
                    .source
                    .file,
                &RequestBudgetV1::unbounded(),
            )
            .expect("file");
        let budget = RequestBudgetV1::unbounded();
        let witness = best_in(
            &file.folded_path,
            "i",
            HitSurface::Path,
            CaseMode::Folded,
            &budget,
        )
        .expect("folded path scan")
        .expect("folded hit");
        let highlight = path_highlight(
            file.source.file.repo_relative_path.as_str(),
            &file.indexed_path,
            &witness,
            &budget,
        )
        .expect("path mapping")
        .expect("path highlight");
        assert_eq!(highlight.start, 4);
        assert_eq!(highlight.len, 2);
        assert_eq!(
            file.source.file.repo_relative_path.as_str().get(4..6),
            Some("İ")
        );
        assert!(
            best_in(
                &file.indexed_path,
                "i",
                HitSurface::Path,
                CaseMode::Sensitive,
                &budget,
            )
            .expect("sensitive path scan")
            .is_none()
        );
        let invalid = super::Witness {
            normalized: 5..6,
            ..witness
        };
        assert!(matches!(
            path_highlight(
                file.source.file.repo_relative_path.as_str(),
                &file.indexed_path,
                &invalid,
                &budget,
            ),
            Err(CoreError::Storage(_))
        ));
    }

    #[test]
    fn file_candidate_id_frames_repo_and_path() {
        assert_ne!(
            file_candidate_id("ab", "c").expect("first id"),
            file_candidate_id("a", "bc").expect("second id")
        );
        assert_eq!(
            file_candidate_id("repo", "src/a.rs").expect("first id"),
            file_candidate_id("repo", "src/a.rs").expect("second id")
        );
    }

    #[test]
    fn ranking_considers_exact_boundary_after_thirty_two_prefix_occurrences() {
        let text = format!("{}foo", "foobar ".repeat(33));
        let witness = best_in(
            &text,
            "foo",
            HitSurface::Content,
            CaseMode::Sensitive,
            &RequestBudgetV1::unbounded(),
        )
        .expect("bounded search")
        .expect("witness");
        assert_eq!(witness.normalized.start, "foobar ".len() * 33);
        assert_eq!(witness.score, boundary_score("foo", 0..3, false));
    }

    #[test]
    fn code_search_posting_callback_preserves_request_cancel_code() {
        let mut builder = TrigramIndexBuilder::new(1).expect("generation");
        for id in 1..=10_000 {
            builder.add_doc(DocId(id), b"abc");
        }
        let index = builder.finish();
        let budget = RequestBudgetV1::unbounded();
        let cancel = budget.cancel_handle();
        let mut checkpoints = 0;
        let result = index.intersect_trigrams_with_checkpoint(&[*b"abc"], || {
            checkpoints += 1;
            if checkpoints == 3 {
                cancel.cancel();
            }
            budget.checkpoint("lexical:code-search-trigram-posting")
        });
        assert!(matches!(
            result,
            Err(TrigramIntersectionError::Checkpoint(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            }))
        ));
        assert_eq!(checkpoints, 3);
    }

    #[test]
    fn bounded_no_match_scan_checks_expired_budget() {
        let deadline = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(1))
            .expect("one millisecond before current instant");
        let budget = RequestBudgetV1::until(deadline);
        let haystack = "x".repeat(8 * 1024 * 1024);
        assert!(matches!(
            best_in(
                &haystack,
                "needle",
                HitSurface::Content,
                CaseMode::Sensitive,
                &budget
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
                ..
            })
        ));
    }
}
