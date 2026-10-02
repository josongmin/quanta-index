//! File-level product code search on the generation's complete source bytes.
//!
//! The public request lowers to the existing `LqQuery` IR with a dedicated
//! pattern type. This executor does not reinterpret Native LQ raw-string
//! leaves: its AND and ranking unit is one immutable source file.

use std::collections::BTreeSet;
use std::ops::Range;

use quanta_index_contract::{
    HighlightSpan, LexicalCandidate, LqExpr, LqFilter, LqLeaf, LqPatternType, LqPredicateArg,
    LqQuery, LqSelect, MAX_CODE_SEARCH_TERM_BYTES, MAX_CODE_SEARCH_TERMS, PreviewByteRange,
    PreviewKind, PreviewMetadata, PreviewUnavailableReason, QueryConstraintSetV1,
};
use quanta_index_core::{CoreError, LexicalPageSpec, LexicalSearchPageV1, RequestBudgetV1};
use quanta_index_lq_regex::RegexExecutor;
use quanta_index_lq_trigram::{
    DocId, MAX_CANDIDATE_PRE_VERIFY, Trigram, TrigramIndex, TrigramIntersectionError, trigrams_of,
};
use sha2::{Digest as _, Sha256};

use crate::TantivySearcher;
use crate::file_authority::{FileAuthority, SourceFile};
use crate::query_errors::map_trigram_error;
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
}

/// Validated executor plan derived from the canonical `LqQuery`, not a wire IR.
pub(crate) struct CodeSearchPlan {
    terms: Vec<CodeSearchTerm>,
    case: CaseMode,
}

fn unsupported(reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
        message: format!("lexical code search: {reason}"),
    }
}

fn file_id_at(index: usize) -> Result<u64, CoreError> {
    let one_based = index
        .checked_add(1)
        .ok_or_else(|| CoreError::Storage("lexical: file index overflow".into()))?;
    u64::try_from(one_based)
        .map_err(|error| CoreError::Storage(format!("lexical: file id overflow: {error}")))
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
            let compiled = if regex {
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
                let _plan = crate::regex::plan_regex(&effective, &query.options, regex_policy)
                    .map_err(map_regex_plan_error)?;
                let executor =
                    RegexExecutor::compile(&effective).map_err(|error| CoreError::Typed {
                        code: crate::query_errors::regex_wire_code(error.code),
                        message: format!("lexical code search regex: {error}"),
                    })?;
                if executor.hir().properties().minimum_len() == Some(0) {
                    return Err(unsupported("zero-width regex matches are unsupported"));
                }
                Some(executor)
            } else {
                None
            };
            terms.push(CodeSearchTerm {
                needle: normalize::apply_case(&source, case).into_owned(),
                text: source,
                scope,
                regex: compiled,
            });
        }
        Ok(Self { terms, case })
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

fn posting_sources(authority: &FileAuthority, scope: Scope) -> [Option<&TrigramIndex>; 2] {
    match scope {
        Scope::Both => [
            Some(&authority.content_folded),
            Some(&authority.path_folded),
        ],
        Scope::Content => [Some(&authority.content_folded), None],
        Scope::Path => [Some(&authority.path_folded), None],
    }
}

struct LiteralPrefilter<'a> {
    term: &'a CodeSearchTerm,
    trigrams: Vec<Trigram>,
}

impl LiteralPrefilter<'_> {
    fn possible_in(&self, authority: &FileAuthority, id: DocId) -> bool {
        posting_sources(authority, self.term.scope)
            .into_iter()
            .flatten()
            .any(|index| {
                self.trigrams
                    .iter()
                    .all(|trigram| index.lookup(*trigram).binary_search(&id).is_ok())
            })
    }

    fn seed_size(&self, authority: &FileAuthority) -> usize {
        posting_sources(authority, self.term.scope)
            .into_iter()
            .flatten()
            .map(|index| {
                self.trigrams
                    .iter()
                    .map(|trigram| index.lookup(*trigram).len())
                    .min()
                    .unwrap_or(0)
            })
            .sum()
    }
}

fn file_for_id(authority: &FileAuthority, id: DocId) -> Result<&SourceFile, CoreError> {
    let position = usize::try_from(id.0.saturating_sub(1))
        .map_err(|error| CoreError::Storage(format!("lexical: file id overflow: {error}")))?;
    let key = authority
        .ordered_keys
        .get(position)
        .ok_or_else(|| CoreError::Storage("lexical: file posting outside authority".into()))?;
    authority
        .files
        .get(key)
        .ok_or_else(|| CoreError::Storage("lexical: file posting has no source".into()))
}

/// Generate one file-level AND set. A broad individual posting is never
/// charged as a completed candidate before the remaining terms filter it.
fn candidate_ids(
    authority: &FileAuthority,
    terms: &[CodeSearchTerm],
    case: CaseMode,
    eligible: Option<&BTreeSet<u64>>,
    budget: &RequestBudgetV1,
) -> Result<BTreeSet<u64>, CoreError> {
    let literals: Vec<_> = terms.iter().filter(|term| term.regex.is_none()).collect();
    let mut indexed = Vec::new();
    for term in &literals {
        // Per-character fold preserves every sensitive substring. The
        // original NFC text remains the final matching authority.
        let needle = match case {
            CaseMode::Sensitive => normalize::fold(&term.needle),
            CaseMode::Folded => term.needle.clone(),
        };
        if needle.len() >= 3 {
            let mut trigrams: Vec<_> = trigrams_of(needle.as_bytes()).collect();
            trigrams.sort_unstable();
            trigrams.dedup();
            indexed.push(LiteralPrefilter { term, trigrams });
        }
    }
    let mut hits = BTreeSet::new();
    if let Some((seed_position, seed)) = indexed
        .iter()
        .enumerate()
        .min_by_key(|(_, term)| term.seed_size(authority))
    {
        for index in posting_sources(authority, seed.term.scope)
            .into_iter()
            .flatten()
        {
            budget.checkpoint("lexical:code-search-trigram")?;
            let candidates = index
                .intersect_trigrams_filtered_with_checkpoint(
                    &seed.trigrams,
                    |id| {
                        if eligible.is_some_and(|ids| !ids.contains(&id.0)) {
                            return Ok(false);
                        }
                        for (position, term) in indexed.iter().enumerate() {
                            if position != seed_position {
                                budget.checkpoint("lexical:code-search-term-postings")?;
                                if !term.possible_in(authority, id) {
                                    return Ok(false);
                                }
                            }
                        }
                        let file = file_for_id(authority, id)?;
                        for term in &literals {
                            if choose_witness(file, term, case, budget)?.is_none() {
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    },
                    || budget.checkpoint("lexical:code-search-trigram-posting"),
                )
                .map_err(|error| match error {
                    TrigramIntersectionError::Index(error) => {
                        map_trigram_error("code search file prefilter", &error)
                    }
                    TrigramIntersectionError::Checkpoint(error) => error,
                })?;
            for id in candidates {
                budget.checkpoint("lexical:code-search-verify")?;
                if hits.len() >= MAX_CANDIDATE_PRE_VERIFY && !hits.contains(&id.0) {
                    return Err(CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                        message: "lexical code search: verified file candidate set exceeds cap".into(),
                    });
                }
                let _inserted = hits.insert(id.0);
            }
        }
        return Ok(hits);
    }

    let scope = scope_union(literals.iter().map(|term| term.scope))
        .ok_or_else(|| CoreError::Storage("lexical: no literal candidate seed".into()))?;
    let source_bytes = source_bytes_checked(authority, scope, case, eligible, budget)?;
    if eligible.map_or(authority.ordered_keys.len(), BTreeSet::len) > MAX_SHORT_SCAN_FILES
        || source_bytes > MAX_SHORT_SCAN_SOURCE_BYTES
    {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
            message: format!(
                "lexical code search: short literal exceeds {MAX_SHORT_SCAN_FILES} files or {MAX_SHORT_SCAN_SOURCE_BYTES} source bytes"
            ),
        });
    }
    for (position, key) in authority.ordered_keys.iter().enumerate() {
        budget.checkpoint("lexical:code-search-short-verify")?;
        let id = file_id_at(position)?;
        if eligible.is_some_and(|ids| !ids.contains(&id)) {
            continue;
        }
        let file = authority
            .files
            .get(key)
            .ok_or_else(|| CoreError::Storage("lexical: file authority id has no source".into()))?;
        let mut matched = true;
        for term in &literals {
            if choose_witness(file, term, case, budget)?.is_none() {
                matched = false;
                break;
            }
        }
        if matched {
            let _inserted = hits.insert(id);
        }
    }
    Ok(hits)
}

fn source_bytes_checked(
    authority: &FileAuthority,
    scope: Scope,
    case: CaseMode,
    eligible: Option<&BTreeSet<u64>>,
    budget: &RequestBudgetV1,
) -> Result<usize, CoreError> {
    let mut total = 0_usize;
    for (position, key) in authority.ordered_keys.iter().enumerate() {
        if position % 256 == 0 {
            budget.checkpoint("lexical:code-search-source-size")?;
        }
        let id = file_id_at(position)?;
        if eligible.is_some_and(|ids| !ids.contains(&id)) {
            continue;
        }
        let file = authority
            .files
            .get(key)
            .ok_or_else(|| CoreError::Storage("lexical: file authority id has no source".into()))?;
        total = total.saturating_add(scanned_bytes(file, scope, case));
    }
    budget.checkpoint("lexical:code-search-source-size-end")?;
    Ok(total)
}

fn scanned_bytes(file: &SourceFile, scope: Scope, case: CaseMode) -> usize {
    let content = match case {
        CaseMode::Sensitive => file.indexed_text.as_ref(),
        CaseMode::Folded => file.folded_text.as_ref(),
    }
    .map_or(0, String::len);
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

fn language_eligible_ids(
    authority: &FileAuthority,
    constraints: &QueryConstraintSetV1,
    budget: &RequestBudgetV1,
) -> Result<Option<BTreeSet<u64>>, CoreError> {
    if constraints.language_any_of.is_empty() {
        return Ok(None);
    }
    let mut ids = BTreeSet::new();
    for (index, key) in authority.ordered_keys.iter().enumerate() {
        if index % 256 == 0 {
            budget.checkpoint("lexical:code-search-language-filter")?;
        }
        let file = authority
            .files
            .get(key)
            .ok_or_else(|| CoreError::Storage("lexical: file authority id has no source".into()))?;
        if constraints.language_any_of.contains(&file.language) {
            let id = file_id_at(index)?;
            let _inserted = ids.insert(id);
        }
    }
    budget.checkpoint("lexical:code-search-language-filter-end")?;
    Ok(Some(ids))
}

fn admit_regex_scan(
    authority: &FileAuthority,
    ids: &BTreeSet<u64>,
    scope: Scope,
    budget: &RequestBudgetV1,
) -> Result<(), CoreError> {
    let mut bytes = 0_usize;
    for (position, id) in ids.iter().enumerate() {
        if position % 256 == 0 {
            budget.checkpoint("lexical:code-search-regex-admission")?;
        }
        let index = usize::try_from(id.saturating_sub(1))
            .map_err(|error| CoreError::Storage(format!("lexical: file id overflow: {error}")))?;
        let key = authority
            .ordered_keys
            .get(index)
            .ok_or_else(|| CoreError::Storage("lexical: regex file id outside authority".into()))?;
        let file = authority
            .files
            .get(key)
            .ok_or_else(|| CoreError::Storage("lexical: regex file id has no source".into()))?;
        bytes = bytes.saturating_add(scanned_bytes(file, scope, CaseMode::Sensitive));
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
    file: &SourceFile,
    witness: &Witness,
    budget: &RequestBudgetV1,
) -> Result<Option<HighlightSpan>, CoreError> {
    budget.checkpoint("lexical:code-search-path-preview")?;
    let raw = file.source.file.repo_relative_path.as_str();
    let mapped = MappedText::new(
        raw,
        &file.indexed_path,
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
    if raw != indexed {
        return Ok(None);
    }
    let surface = match witness.mapping_case {
        CaseMode::Sensitive => indexed,
        CaseMode::Folded => folded,
    };
    if witness.normalized.is_empty() || surface.get(witness.normalized.clone()).is_none() {
        return Err(CoreError::Storage(
            "lexical: NFC file focus is not UTF-8 aligned".into(),
        ));
    }
    if witness.mapping_case == CaseMode::Sensitive || raw.is_ascii() {
        return Ok(Some(witness.normalized.clone()));
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
        if end > witness.normalized.start && folded_offset < witness.normalized.end {
            if original_start.is_none() {
                original_start = Some(start);
            }
            original_end = start.saturating_add(scalar.len_utf8());
        }
        folded_offset = end;
        if folded_offset >= witness.normalized.end {
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

fn file_candidate(
    owner: &TantivySearcher,
    file: &SourceFile,
    score: f32,
    focus: Option<&Witness>,
    materialize_preview: bool,
    budget: &RequestBudgetV1,
) -> Result<LexicalCandidate, CoreError> {
    let source = file.source.clone();
    let mut candidate = LexicalCandidate {
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
    };
    let Some(witness) = focus else {
        return Err(CoreError::Storage(
            "lexical: matched file has no positive witness".into(),
        ));
    };
    if !materialize_preview {
        return Ok(candidate);
    }
    // Each file has one row. Score/repo/path already give distinct order
    // keys, so line lookup can stay on the selected page without changing
    // sort or cursor order.
    if matches!(witness.surface, HitSurface::Content) {
        (candidate.start_line, candidate.end_line) = content_witness_lines(file, witness, budget)?;
    }
    if matches!(witness.surface, HitSurface::Path) {
        let Some(highlight) = path_highlight(file, witness, budget)? else {
            candidate.preview = Some(PreviewMetadata::unavailable(
                PreviewKind::Path,
                PreviewUnavailableReason::WorkBudget,
                Some(source),
            ));
            return Ok(candidate);
        };
        candidate.snippet = source.file.repo_relative_path.as_str().to_string();
        candidate.snippet_hit_offset = Some(highlight.start);
        candidate.highlights.push(highlight);
        candidate.preview = Some(PreviewMetadata {
            kind: PreviewKind::Path,
            source: Some(source),
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

impl TantivySearcher {
    pub(crate) fn search_code_files(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        page: &LexicalPageSpec,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        let parsed = CodeSearchPlan::parse(query, &self.regex_policy)?;
        let authority = self.file_authority.as_ref().ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported,
            message: "lexical code search requires rebuilt full-file source authority".into(),
        })?;
        let eligible = language_eligible_ids(authority, constraints, budget)?;
        let eligible = eligible.as_ref();
        let ids: Option<BTreeSet<u64>> = if let Some(path) = &constraints.repo_relative_path_exact {
            let mut matching = BTreeSet::new();
            for (index, key) in authority.ordered_keys.iter().enumerate() {
                budget.checkpoint("lexical:code-search-exact-path")?;
                if key.repo_relative_path.as_str() == path.as_str() {
                    let id = file_id_at(index)?;
                    if eligible.is_none_or(|ids| ids.contains(&id)) {
                        let _inserted = matching.insert(id);
                    }
                }
            }
            Some(matching)
        } else if parsed.terms.iter().any(|term| term.regex.is_none()) {
            // A regex cannot safely supply a literal trigram unless its
            // dialect proves that literal mandatory. Join all literal terms
            // first, then verify regex over the admitted file set.
            budget.checkpoint("lexical:code-search-terms")?;
            Some(candidate_ids(
                authority,
                &parsed.terms,
                parsed.case,
                eligible,
                budget,
            )?)
        } else {
            None
        };
        let regex_scope = regex_scan_scope(&parsed.terms);
        let ids = match ids {
            Some(ids) => ids,
            None if regex_scope.is_some() => {
                budget.checkpoint("lexical:code-search-regex-all-files")?;
                let scope = regex_scope.ok_or_else(|| {
                    CoreError::Storage("lexical: regex scan scope disappeared".into())
                })?;
                if eligible.map_or(authority.ordered_keys.len(), BTreeSet::len)
                    > MAX_REGEX_SCAN_FILES
                    || source_bytes_checked(
                        authority,
                        scope,
                        CaseMode::Sensitive,
                        eligible,
                        budget,
                    )? > MAX_REGEX_SCAN_SOURCE_BYTES
                {
                    return Err(CoreError::Typed {
                        code:
                            quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
                        message: "lexical code search regex requires a bounded file scan".into(),
                    });
                }
                if let Some(ids) = eligible {
                    ids.clone()
                } else {
                    (1..=authority.ordered_keys.len())
                        .map(|position| {
                            u64::try_from(position).map_err(|error| {
                                CoreError::Storage(format!("lexical: file id overflow: {error}"))
                            })
                        })
                        .collect::<Result<BTreeSet<_>, _>>()?
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
        if let Some(scope) = regex_scope {
            admit_regex_scan(authority, &ids, scope, budget)?;
        }
        let mut ranked = Vec::new();
        for id in ids {
            budget.checkpoint("lexical:code-search-file")?;
            let position = usize::try_from(id.saturating_sub(1)).map_err(|error| {
                CoreError::Storage(format!("lexical: file id overflow: {error}"))
            })?;
            let key = authority
                .ordered_keys
                .get(position)
                .ok_or_else(|| CoreError::Storage("lexical: file id outside authority".into()))?;
            let file = authority
                .files
                .get(key)
                .ok_or_else(|| CoreError::Storage("lexical: file id has no source".into()))?;
            let mut score = 0_u32;
            let mut primary: Option<Witness> = None;
            for term in &parsed.terms {
                let Some(witness) = choose_witness(file, term, parsed.case, budget)? else {
                    primary = None;
                    break;
                };
                score = score.saturating_add(witness.score);
                score = score.saturating_add(
                    u32::from(witness.occurrences.saturating_sub(1)).saturating_mul(2),
                );
                let case_exact = if term.regex.is_some() {
                    false
                } else {
                    match witness.surface {
                        HitSurface::Content => file
                            .indexed_text
                            .as_ref()
                            .is_some_and(|text| text.contains(&term.text)),
                        HitSurface::Path => file.indexed_path.contains(&term.text),
                    }
                };
                if matches!(parsed.case, CaseMode::Folded) && case_exact {
                    score = score.saturating_add(5);
                }
                if primary
                    .as_ref()
                    .is_none_or(|best| witness.score > best.score)
                {
                    primary = Some(witness);
                }
            }
            let Some(witness) = primary else {
                continue;
            };
            score =
                score.saturating_add(proximity_bonus(file, &parsed.terms, parsed.case, budget)?);
            // At most 32 terms contribute <= 146 points each, plus a 32 point
            // proximity bonus. This fits u16 and converts to f32 exactly.
            let score = u16::try_from(score).map_err(|error| {
                CoreError::Storage(format!("lexical: code search score overflow: {error}"))
            })?;
            let candidate =
                file_candidate(self, file, f32::from(score), Some(&witness), false, budget)?;
            ranked.push((candidate, witness));
        }
        ranked.sort_by(|(left, _), (right, _)| left.order_key().order(&right.order_key()));
        let after = self.page_boundary(page)?;
        if let Some(after) = after {
            ranked.retain(|(candidate, _)| after.admits(&candidate.order_key()));
        }
        let exact_total = Some(u64::try_from(ranked.len()).map_err(|error| {
            CoreError::Storage(format!("lexical: file count overflow: {error}"))
        })?);
        ranked.truncate(Self::page_limit(
            query,
            usize::try_from(page.fetch).map_err(|error| {
                CoreError::Storage(format!("lexical: file fetch overflow: {error}"))
            })?,
        ));
        for (candidate, witness) in &mut ranked {
            budget.checkpoint("lexical:code-search-selected-preview")?;
            let source = candidate
                .source
                .as_ref()
                .ok_or_else(|| CoreError::Storage("lexical: file candidate lost source".into()))?;
            let file = authority.files.get(&source.file).ok_or_else(|| {
                CoreError::Storage("lexical: selected file outside authority".into())
            })?;
            *candidate = file_candidate(self, file, candidate.score, Some(witness), true, budget)?;
        }
        Ok(LexicalSearchPageV1 {
            candidates: ranked.into_iter().map(|(candidate, _)| candidate).collect(),
            exact_total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CodeSearchTerm, HitSurface, OverlappingMatches, Scope, best_in, boundary_score,
        candidate_ids, content_witness_lines, file_candidate_id, language_eligible_ids,
        min_cover_gap, nfc_identity_focus, path_highlight, regex_scan_scope, scanned_bytes,
        source_bytes_checked,
    };
    use quanta_index_contract::lex::LanguageCode;
    use quanta_index_contract::{
        QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2,
        SourceFileKey, SourceFileRevision,
    };
    use quanta_index_core::{CoreError, RequestBudgetV1};
    use quanta_index_lq_regex::RegexExecutor;
    use quanta_index_lq_trigram::{DocId, TrigramIndexBuilder, TrigramIntersectionError};
    use std::collections::{BTreeMap, BTreeSet};

    use crate::file_authority::{FileAuthority, SourceFile, from_verified_files};
    use quanta_index_lq_text_normalizer::{self as normalize, CaseMode, MappedText};

    fn fixture_postings(path: &str, content: Option<&str>) -> u32 {
        let path = normalize::nfc(path).into_owned();
        let path_folded = normalize::fold(&path);
        let content = content.map(|value| normalize::nfc(value).into_owned());
        let content_folded = content.as_deref().map(normalize::fold);
        let surfaces = [Some(path_folded.as_str()), content_folded.as_deref()];
        let count: usize = surfaces
            .into_iter()
            .flatten()
            .map(|surface| {
                surface
                    .as_bytes()
                    .windows(3)
                    .map(|window| <[u8; 3]>::try_from(window).expect("trigram"))
                    .collect::<BTreeSet<_>>()
                    .len()
            })
            .sum();
        u32::try_from(count).expect("fixture count")
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
            bytes: text.as_bytes().to_vec(),
            text_admitted: true,
            language: LanguageCode::new("rust").expect("language"),
            indexed_text: None,
            folded_text: None,
            indexed_path: String::new(),
            folded_path: String::new(),
            expected_postings: fixture_postings(path, Some(text)),
        };
        let authority = from_verified_files(vec![file], None).expect("authority");
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
            bytes: raw.into_bytes(),
            text_admitted: true,
            language: LanguageCode::new("rust").expect("language"),
            indexed_text: Some(indexed),
            folded_text: Some(folded),
            indexed_path: "large.rs".into(),
            folded_path: "large.rs".into(),
            expected_postings: 0,
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
    fn overlapping_literal_witnesses_preserve_minimum_file_proximity() {
        let budget = RequestBudgetV1::unbounded();
        let terms = ["aaa", "b"].map(|text| CodeSearchTerm {
            text: text.into(),
            needle: text.into(),
            scope: Scope::Content,
            regex: None,
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
                        source_sha256: [0; 32],
                    },
                    bytes: value.as_bytes().to_vec(),
                    text_admitted: true,
                    language: if index == 0 { go.clone() } else { txt.clone() },
                    indexed_text: None,
                    folded_text: None,
                    indexed_path: String::new(),
                    folded_path: String::new(),
                    expected_postings: fixture_postings(&path, Some(value)),
                }
            })
            .collect();
        let authority = from_verified_files(files, None).expect("file authority");
        let term = CodeSearchTerm {
            text: "x".into(),
            needle: "x".into(),
            scope: Scope::Content,
            regex: None,
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
        assert_eq!(matches.into_iter().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn broad_individual_terms_admit_their_small_file_and_intersection() {
        let repo = RepoId::new("fixture").expect("repo");
        let revision = RevisionId::new("fixture-revision").expect("revision");
        let language = LanguageCode::new("text").expect("language");
        let mut content_index = TrigramIndexBuilder::new(1).expect("generation");
        let path_index = TrigramIndexBuilder::new(1).expect("generation");
        let mut files = BTreeMap::new();
        let mut ordered_keys = Vec::new();
        for position in 0..=200_000_u64 {
            let text = if position == 200_000 {
                "aaa bbb"
            } else if position < 100_000 {
                "aaa"
            } else {
                "bbb"
            };
            let key = SourceFileKey {
                source_repo_id: repo.clone(),
                repo_relative_path: RepoRelativePath::new(format!("src/{position:06}.txt")),
            };
            content_index.add_doc(DocId(position + 1), text.as_bytes());
            ordered_keys.push(key.clone());
            let _previous = files.insert(
                key.clone(),
                SourceFile {
                    source: SourceFileRevision {
                        file: key,
                        revision_id: revision.clone(),
                        source_sha256: [0; 32],
                    },
                    bytes: text.as_bytes().to_vec(),
                    text_admitted: true,
                    language: language.clone(),
                    indexed_text: Some(text.to_string()),
                    folded_text: Some(text.to_string()),
                    indexed_path: String::new(),
                    folded_path: String::new(),
                    expected_postings: 0,
                },
            );
        }
        let authority = FileAuthority {
            files,
            ordered_keys,
            content_folded: content_index.finish(),
            path_folded: path_index.finish(),
        };
        let terms = ["aaa", "bbb"].map(|text| CodeSearchTerm {
            text: text.into(),
            needle: text.into(),
            scope: Scope::Content,
            regex: None,
        });
        let budget = RequestBudgetV1::unbounded();
        assert!(matches!(
            candidate_ids(
                &authority,
                std::slice::from_ref(&terms[0]),
                CaseMode::Sensitive,
                None,
                &budget,
            ),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::LexTrigramPlanLimitExceeded,
                ..
            })
        ));
        assert_eq!(
            candidate_ids(&authority, &terms, CaseMode::Sensitive, None, &budget)
                .expect("joint candidate set"),
            BTreeSet::from([200_001]),
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
            source_sha256: [0; 32],
        };
        let authority = from_verified_files(
            vec![SourceFile {
                source,
                bytes: "İ".as_bytes().to_vec(),
                text_admitted: true,
                language: LanguageCode::new("go").expect("language"),
                indexed_text: None,
                folded_text: None,
                indexed_path: String::new(),
                folded_path: String::new(),
                expected_postings: fixture_postings("src/İ.go", Some("İ")),
            }],
            None,
        )
        .expect("file authority");
        let file = authority.files.values().next().expect("file");
        assert_eq!(file.folded_text.as_deref(), Some("i\u{307}"));
        assert_eq!(file.folded_path, "src/i\u{307}.go");
        assert_eq!(scanned_bytes(file, Scope::Content, CaseMode::Sensitive), 2);
        assert_eq!(scanned_bytes(file, Scope::Content, CaseMode::Folded), 3);
        assert_eq!(scanned_bytes(file, Scope::Both, CaseMode::Folded), 13);
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
            source_sha256: [0; 32],
        };
        let authority = from_verified_files(
            vec![SourceFile {
                source,
                bytes: Vec::new(),
                text_admitted: false,
                language: LanguageCode::new("go").expect("language"),
                indexed_text: None,
                folded_text: None,
                indexed_path: String::new(),
                folded_path: String::new(),
                expected_postings: fixture_postings("src/İ.go", None),
            }],
            None,
        )
        .expect("file authority");
        let file = authority.files.values().next().expect("file");
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
        let highlight = path_highlight(file, &witness, &budget)
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
            path_highlight(file, &invalid, &budget),
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
